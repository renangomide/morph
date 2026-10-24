// SPDX-License-Identifier: GPL-3.0-or-later
//! Video → Compress "Best (automatic)" (SPEC §4.3, approved 2026-10-20): the
//! smallest file with no visible quality loss, no settings needed.
//!
//! 1. Keep resolution (unless the Resolution option caps it) and container
//!    ([`super::compress::container_for`]: MP4/MOV/MKV/WebM; others → MP4).
//!    Video H.264; WebM inputs stay WebM with VP9 + Opus. Audio copied when it
//!    already is AAC/Opus at ≤ 192 kbps, else AAC 160 kbps (Opus 128 kbps in WebM).
//! 2. Quality search: [`sample_ranges`] picks 3–5 samples of 2 s spread across
//!    the clip (the whole clip when it is shorter than [`WHOLE_CLIP_BELOW_S`]);
//!    each probe encodes every sample at one quality value and measures VMAF
//!    against the original (`libvmaf`, JSON log in the task temp folder). The
//!    probe's score is the *lowest* sample score. [`search`] keeps the highest
//!    quality value (= CRF, or the GPU encoder's own scale) whose score is ≥
//!    [`TARGET_VMAF`]: interpolating between probes, at most [`MAX_PROBES`].
//!    A GPU encoder that fails during the search is dropped and the search
//!    starts over on the CPU encoder (different quality scale).
//! 3. One full encode at that value. Never bigger: unless the result is at
//!    least 5% smaller than the original, nothing is written and the note says
//!    "Already well compressed" (SPEC §4.3 "Never bigger").
//!
//! Progress: search 0–30%, full encode 30–100%.

use super::encode::{self, AudioPlan, Backend, Codec, EncodeSettings, Encoder, Range, RateControl};
use super::options::{VideoCompressOpts, VideoContainer};
use super::probe::MediaInfo;
use super::{ffmpeg, gpu};
use crate::converter::{TaskCtx, TaskOutput};
use crate::error::ConvertError;
use crate::job::Task;
use crate::naming::Suffix;
use std::ffi::OsString;
use std::path::Path;

/// "No visible quality loss" (SPEC §4.3).
pub const TARGET_VMAF: f64 = 93.0;
/// A pass this close to the target is good enough: the next step would fail.
const CLOSE_ENOUGH: f64 = 0.5;
/// Most quality values tried per file.
pub const MAX_PROBES: usize = 4;
/// Seconds per sample.
const SAMPLE_S: f64 = 2.0;
/// Shorter clips are measured whole.
pub const WHOLE_CLIP_BELOW_S: f64 = 8.0;
/// Write only when the result is at most this fraction of the original size.
const MAX_RATIO: f64 = 0.95;
/// Share of the progress bar for the search.
const SEARCH_SHARE: f32 = 0.3;
/// Audio re-encode bitrates (SPEC §4.3).
const AAC_KBPS: u32 = 160;
const OPUS_KBPS: u32 = 128;
/// Audio at or under this is copied.
const COPY_AUDIO_MAX_BPS: u64 = 192_000;

/// Note when nothing was written.
pub const NOTE_ALREADY_COMPRESSED: &str = "Already well compressed";

/// The quality knob of one encoder: higher = smaller, worse.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct QualityScale {
    pub min: u8,
    pub max: u8,
    /// First value tried.
    pub start: u8,
    /// Rough VMAF points lost per step, for the first guess after one probe.
    pub slope: f64,
}

impl QualityScale {
    /// The searched range for `encoder` (x264 `-crf`, VP9 `-crf`, NVENC `-cq`,
    /// QSV `-global_quality`, AMF `-qp_*`), inside the useful ranges documented
    /// in `media/encode.rs`.
    pub fn of(encoder: Encoder) -> QualityScale {
        match (encoder.codec, encoder.backend) {
            (Codec::Vp9, _) => QualityScale {
                min: 20,
                max: 45,
                start: 33,
                slope: 0.5,
            },
            (_, Backend::Cpu) => QualityScale {
                min: 18,
                max: 34,
                start: 26,
                slope: 1.0,
            },
            _ => QualityScale {
                min: 18,
                max: 36,
                start: 27,
                slope: 1.0,
            },
        }
    }
}

/// Called by `VideoCompress` for [`super::options::VideoCompressPreset::Best`].
pub fn compress_best(
    task: &Task,
    opts: &VideoCompressOpts,
    ctx: &TaskCtx<'_>,
) -> Result<TaskOutput, ConvertError> {
    let (input, info) = super::open(task, ctx)?;
    let settings = plan(input, &info, opts, ctx)?;

    let quality = pick_quality(input, &info, &settings, ctx, &mut |f| {
        ctx.progress(f * SEARCH_SHARE, None)
    })?;
    ctx.check_cancel()?;

    let ext = output_extension(input, settings.container);
    let out = super::output_for(task, input, &ext, Suffix::Compressed)?;
    let full = EncodeSettings {
        // The CPU one if the GPU encoder failed during the search.
        encoder: encode::usable(settings.encoder),
        rate: RateControl::Quality(quality),
        ..settings
    };
    encode::encode(input, &info, &full, None, out.temp_path(), ctx, &mut |f| {
        ctx.progress(SEARCH_SHARE + f * (1.0 - SEARCH_SHARE), None)
    })?;

    let original = file_len(input)?;
    let result = file_len(out.temp_path())?;
    if (result as f64) > original as f64 * MAX_RATIO {
        drop(out); // deletes the temp file
        return Ok(TaskOutput::default().with_note(NOTE_ALREADY_COMPRESSED));
    }
    Ok(TaskOutput::single(out.commit()?))
}

/// Encoder, audio and container for this input (quality still open).
pub fn plan(
    input: &Path,
    info: &MediaInfo,
    opts: &VideoCompressOpts,
    ctx: &TaskCtx<'_>,
) -> Result<EncodeSettings, ConvertError> {
    if info.video().is_none() {
        return Err(super::no_video_track());
    }
    let container = super::compress::container_for(input);
    let codec = if container == VideoContainer::Webm {
        Codec::Vp9
    } else {
        Codec::H264
    };
    Ok(EncodeSettings {
        encoder: gpu::pick_encoder(codec, opts.encoder, ctx),
        rate: RateControl::Quality(QualityScale::of(Encoder::cpu(codec)).start),
        max_height: opts.resolution.max_height(),
        audio: audio_plan(info, container),
        container,
    })
}

/// Audio of every Compress quality preset (Best, Balanced, Smaller), SPEC §4.3:
/// copy AAC/Opus ≤ 192 kbps (when the container takes it), else AAC 160 kbps
/// (Opus 128 kbps for WebM). Copied only when every track qualifies (one plan
/// for all tracks).
pub fn audio_plan(info: &MediaInfo, container: VideoContainer) -> AudioPlan {
    if !info.has_audio() {
        return AudioPlan::None;
    }
    let copyable = |audio: &super::probe::StreamInfo| {
        // Unknown stream bitrate (common in MKV/WebM): copying is the safe bet.
        let small = audio.bit_rate.is_none_or(|b| b <= COPY_AUDIO_MAX_BPS);
        let fits = match (audio.codec.as_str(), container) {
            ("opus", VideoContainer::Webm | VideoContainer::Mp4 | VideoContainer::Mkv) => true,
            ("aac", c) => c != VideoContainer::Webm,
            _ => false,
        };
        small && fits
    };
    if info.audio_streams().all(copyable) {
        AudioPlan::Copy
    } else if container == VideoContainer::Webm {
        AudioPlan::Opus { kbps: OPUS_KBPS }
    } else {
        AudioPlan::Aac { kbps: AAC_KBPS }
    }
}

/// The input's own extension when we write that container (keeps `.m4v`), else
/// the container's.
fn output_extension(input: &Path, container: VideoContainer) -> String {
    match VideoContainer::of_path(input) {
        Some(c) if c == container => input
            .extension()
            .and_then(|e| e.to_str())
            .map_or_else(|| container.extension().to_owned(), str::to_ascii_lowercase),
        _ => container.extension().to_owned(),
    }
}

/// Where the samples are: `None` = measure the whole clip.
pub fn sample_ranges(duration_s: Option<f64>) -> Option<Vec<Range>> {
    let d = duration_s.filter(|d| d.is_finite() && *d >= WHOLE_CLIP_BELOW_S)?;
    let n = if d < 120.0 {
        3
    } else if d < 600.0 {
        4
    } else {
        5
    };
    Some(
        (1..=n)
            .map(|i| Range {
                // Centers at 1/(n+1), 2/(n+1), …: never the very start or end.
                start_s: d * i as f64 / (n + 1) as f64 - SAMPLE_S / 2.0,
                length_s: SAMPLE_S,
            })
            .collect(),
    )
}

/// Search the quality value for `settings` (its `rate` is ignored): the highest
/// one whose lowest sample VMAF is ≥ [`TARGET_VMAF`]. `on_progress` 0–1.
///
/// The value is for `encode::usable(settings.encoder)` after the call: when the
/// GPU encoder fails midway, the search starts over on the CPU encoder.
pub fn pick_quality(
    input: &Path,
    info: &MediaInfo,
    settings: &EncodeSettings,
    ctx: &TaskCtx<'_>,
    on_progress: &mut dyn FnMut(f32),
) -> Result<u8, ConvertError> {
    let encoder = encode::usable(settings.encoder);
    let first = EncodeSettings {
        encoder,
        ..settings.clone()
    };
    match search_samples(input, info, &first, ctx, on_progress) {
        Err(e) if e.details == SWITCHED_TO_CPU => {
            let cpu = EncodeSettings {
                encoder: Encoder::cpu(encoder.codec),
                ..first
            };
            on_progress(0.0);
            search_samples(input, info, &cpu, ctx, on_progress)
        }
        other => other,
    }
}

/// Internal: the GPU encoder failed during a search (see [`pick_quality`]).
const SWITCHED_TO_CPU: &str = "best: GPU encoder failed during the search";

fn search_samples(
    input: &Path,
    info: &MediaInfo,
    settings: &EncodeSettings,
    ctx: &TaskCtx<'_>,
    on_progress: &mut dyn FnMut(f32),
) -> Result<u8, ConvertError> {
    let samples: Vec<Option<Range>> = match sample_ranges(info.duration_s) {
        Some(r) => r.into_iter().map(Some).collect(),
        None => vec![None],
    };
    let reference = info.video().and_then(|v| v.display_size());
    let sample_settings = EncodeSettings {
        audio: AudioPlan::None,
        ..settings.clone()
    };
    let ext = settings.container.extension();
    let temp = ctx.temp_dir()?.to_path_buf();
    let steps = (MAX_PROBES * samples.len()) as f32;
    let mut done = 0.0f32;
    let scale = QualityScale::of(settings.encoder);
    search(scale, &mut |q| {
        let s = EncodeSettings {
            rate: RateControl::Quality(q),
            ..sample_settings.clone()
        };
        let mut worst = f64::INFINITY;
        for (i, range) in samples.iter().enumerate() {
            let out = temp.join(format!("sample-{i}.{ext}"));
            let length = range.map(|r| r.length_s).or(info.duration_s);
            encode::encode(input, info, &s, *range, &out, ctx, &mut |f| {
                on_progress((done + f * 0.6) / steps)
            })?;
            if encode::usable(s.encoder) != s.encoder {
                // That sample ran on the CPU: these scores are on another scale.
                return Err(ConvertError::other(
                    "Something went wrong while converting this file.",
                    SWITCHED_TO_CPU,
                ));
            }
            let score = vmaf(&out, input, *range, reference, length, ctx, &mut |f| {
                on_progress((done + 0.6 + f * 0.4) / steps)
            })?;
            let _ = std::fs::remove_file(&out);
            done += 1.0;
            worst = worst.min(score);
        }
        Ok(worst)
    })
}

/// The search itself, over a `measure(q) -> score` that falls as `q` rises.
/// Returns the highest `q` in `scale` scoring ≥ [`TARGET_VMAF`], or `scale.min`
/// when none does (the full encode then decides via "never bigger").
pub fn search(
    scale: QualityScale,
    measure: &mut dyn FnMut(u8) -> Result<f64, ConvertError>,
) -> Result<u8, ConvertError> {
    let (min, max) = (i32::from(scale.min), i32::from(scale.max));
    let mut pass: Option<(i32, f64)> = None;
    let mut fail: Option<(i32, f64)> = None;
    let mut q = i32::from(scale.start).clamp(min, max);
    for _ in 0..MAX_PROBES {
        let score = measure(q as u8)?;
        if score >= TARGET_VMAF {
            pass = Some((q, score));
        } else {
            fail = Some((q, score));
        }
        let low = pass.map_or(min - 1, |p| p.0);
        let high = fail.map_or(max + 1, |f| f.0);
        if high - low <= 1 || pass.is_some_and(|p| p.1 < TARGET_VMAF + CLOSE_ENOUGH) {
            break;
        }
        let guess = match (pass, fail) {
            (Some((qp, sp)), Some((qf, sf))) if sp > sf => {
                qp as f64 + (sp - TARGET_VMAF) * f64::from(qf - qp) / (sp - sf)
            }
            _ => q as f64 + (score - TARGET_VMAF) / scale.slope,
        };
        q = (guess.floor() as i32).clamp(low + 1, high - 1);
    }
    Ok(pass.map_or(scale.min, |p| p.0 as u8))
}

/// VMAF (pooled mean) of `distorted` against `range` of `reference`.
fn vmaf(
    distorted: &Path,
    reference: &Path,
    range: Option<Range>,
    size: Option<(u32, u32)>,
    length_s: Option<f64>,
    ctx: &TaskCtx<'_>,
    on_progress: &mut dyn FnMut(f32),
) -> Result<f64, ConvertError> {
    let log = "vmaf.json";
    let temp = ctx.temp_dir()?;
    let _ = std::fs::remove_file(temp.join(log));
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
    // Distorted first, reference second; same size and format, both from 0.
    let scale = size.map_or(String::new(), |(w, h)| {
        format!("scale={w}:{h}:flags=bicubic,")
    });
    let graph = format!(
        "[0:v]{scale}format=yuv420p,setpts=PTS-STARTPTS[d];\
         [1:v]format=yuv420p,setpts=PTS-STARTPTS[r];\
         [d][r]libvmaf=n_threads={threads}:shortest=1:log_fmt=json:log_path={log}"
    );
    let mut args: Vec<OsString> = vec!["-i".into(), ffmpeg::path_arg(distorted)];
    if let Some(r) = range {
        args.extend(seek_args(r));
    }
    args.push("-i".into());
    args.push(ffmpeg::path_arg(reference));
    args.extend(["-lavfi", &graph, "-an", "-f", "null", "-"].map(OsString::from));
    ffmpeg::run(&args, length_s, ctx, on_progress)?;
    let json = std::fs::read(temp.join(log))
        .map_err(|e| ConvertError::other("Couldn't measure the video quality.", e))?;
    parse_vmaf(&json)
        .ok_or_else(|| ConvertError::other("Couldn't measure the video quality.", "no VMAF score"))
}

/// `pooled_metrics.vmaf.mean` from libvmaf's JSON log.
pub fn parse_vmaf(json: &[u8]) -> Option<f64> {
    let v: serde_json::Value = serde_json::from_slice(json).ok()?;
    v.get("pooled_metrics")?.get("vmaf")?.get("mean")?.as_f64()
}

fn seek_args(r: Range) -> [OsString; 4] {
    [
        "-ss".into(),
        format!("{:.3}", r.start_s.max(0.0)).into(),
        "-t".into(),
        format!("{:.3}", r.length_s).into(),
    ]
}

fn file_len(path: &Path) -> Result<u64, ConvertError> {
    std::fs::metadata(path)
        .map(|m| m.len())
        .map_err(|e| ConvertError::other("Couldn't read the compressed file.", e))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cpu() -> QualityScale {
        QualityScale::of(Encoder::cpu(Codec::H264))
    }

    /// Runs the search on a linear VMAF curve; returns (answer, probes).
    fn run(curve: impl Fn(u8) -> f64) -> (u8, Vec<u8>) {
        let mut probes = Vec::new();
        let q = search(cpu(), &mut |q| {
            probes.push(q);
            Ok(curve(q))
        })
        .unwrap();
        (q, probes)
    }

    /// The true answer for a curve, by brute force.
    fn truth(curve: impl Fn(u8) -> f64) -> u8 {
        (cpu().min..=cpu().max)
            .rev()
            .find(|&q| curve(q) >= TARGET_VMAF)
            .unwrap_or(cpu().min)
    }

    #[test]
    fn search_finds_the_edge_in_few_probes() {
        for (slope, offset) in [(1.0, 0.0), (1.5, 3.0), (0.6, -4.0), (2.5, 6.0), (1.0, 10.0)] {
            // score(q) = 93 + offset + slope·(26 − q) − a little.
            let curve = |q: u8| TARGET_VMAF + offset + slope * (26.0 - f64::from(q)) - 0.3;
            let (q, probes) = run(curve);
            assert!(probes.len() <= MAX_PROBES, "{probes:?}");
            assert!(
                curve(q) >= TARGET_VMAF,
                "slope {slope} offset {offset}: {q}"
            );
            // Within one step of the best (CLOSE_ENOUGH may stop early).
            assert!(
                truth(curve) - q <= 1,
                "{slope}/{offset}: {q} vs {}",
                truth(curve)
            );
        }
    }

    #[test]
    fn search_bounds() {
        let (q, probes) = run(|_| 99.0);
        assert_eq!(q, 34);
        assert!(probes.len() <= MAX_PROBES, "{probes:?}");
        let (q, probes) = run(|_| 50.0);
        assert_eq!(q, 18);
        assert_eq!(probes.last(), Some(&18), "{probes:?}");
    }

    #[test]
    fn samples_spread_and_skip_the_ends() {
        assert_eq!(sample_ranges(None), None);
        assert_eq!(sample_ranges(Some(5.0)), None);
        let r = sample_ranges(Some(60.0)).unwrap();
        assert_eq!(r.len(), 3);
        assert!(
            r[0].start_s > 5.0 && r[2].start_s + r[2].length_s < 55.0,
            "{r:?}"
        );
        assert_eq!(sample_ranges(Some(3600.0)).unwrap().len(), 5);
    }

    #[test]
    fn vmaf_json() {
        let json = br#"{"version":"3","frames":[],"pooled_metrics":{"vmaf":{"min":90.1,"max":99.0,"mean":95.25,"harmonic_mean":95.0}}}"#;
        assert_eq!(parse_vmaf(json), Some(95.25));
        assert_eq!(parse_vmaf(b"{}"), None);
    }
}
