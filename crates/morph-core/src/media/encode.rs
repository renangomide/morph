// SPDX-License-Identifier: GPL-3.0-or-later
//! The video encode seam shared by Convert/Compress and Compress
//! "Best (automatic)". [`encode`] is implemented here;
//! the types and the `encode` signature are the contract between the
//! callers: change them together.
//!
//! ## Quality scales ([`RateControl::Quality`]), lower = better + bigger
//!
//! | Backend | FFmpeg options | Useful range | ≈ H.264 CRF 23 |
//! |---|---|---|---|
//! | CPU x264 / x265 | `-crf q` (`-preset medium`) | 16–34 (x265 ~ x264 + 4) | 23 (x265: 26–28) |
//! | CPU SVT-AV1 | `-crf q` (`-preset 8`) | 20–50 | 34 |
//! | CPU VP9 | `-crf q -b:v 0` (`-deadline good -cpu-used 4`) | 20–45 | 33 |
//! | NVENC | `-rc vbr -cq q -b:v 0` (`-preset p5`) | 18–36 | 25 |
//! | QSV | `-global_quality q` (ICQ) | 18–36 | 25 |
//! | AMF | `-rc cqp -qp_i q -qp_p q (-qp_b q)` | 18–36 | 24 |
//!
//! [`RateControl::Preset`] uses [`preset_quality`] (CPU: H.264 20/23/28, H.265
//! 22/26/30, AV1 28/34/40, VP9 28/33/38; GPU slightly higher numbers, as their
//! scales give bigger files at the same number). GPU AV1/VP9 are never picked by
//! `gpu::pick_encoder` (only H.264/H.265 are tested).
//!
//! [`RateControl::TargetKbps`]: CPU H.264/H.265/VP9 = two-pass (`-pass 1/2`,
//! logs in `ctx.temp_dir()`, progress 0–0.5 / 0.5–1); SVT-AV1 = one-pass VBR;
//! GPU = one-pass VBR with `-maxrate` 1.5× and `-bufsize` 2×.
//!
//! A GPU encode that fails (driver error, out of GPU memory, …) is retried once
//! on the CPU encoder of the same codec (`Quality(q)` keeps the same number),
//! and that GPU encoder is not used again by this process ([`usable`]).

use super::ffmpeg;
use super::options::{VideoContainer, VideoQuality};
use super::probe::{MediaInfo, StreamKind};
use crate::converter::TaskCtx;
use crate::error::{ConvertError, ErrorKind};
use std::ffi::OsString;
use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering};

/// Concrete video codec of an encode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Codec {
    H264,
    H265,
    Av1,
    Vp9,
}

/// Where the encode runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Backend {
    /// libx264 / libx265 / libsvtav1 / libvpx-vp9.
    Cpu,
    Nvenc,
    Qsv,
    Amf,
}

/// One FFmpeg video encoder.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Encoder {
    pub codec: Codec,
    pub backend: Backend,
}

impl Encoder {
    pub const fn cpu(codec: Codec) -> Encoder {
        Encoder {
            codec,
            backend: Backend::Cpu,
        }
    }

    /// FFmpeg encoder name (`-c:v`). `None` for a combination FFmpeg doesn't have.
    pub fn ffmpeg_name(self) -> Option<&'static str> {
        use Backend::*;
        use Codec::*;
        Some(match (self.codec, self.backend) {
            (H264, Cpu) => "libx264",
            (H265, Cpu) => "libx265",
            (Av1, Cpu) => "libsvtav1",
            (Vp9, Cpu) => "libvpx-vp9",
            (H264, Nvenc) => "h264_nvenc",
            (H265, Nvenc) => "hevc_nvenc",
            (Av1, Nvenc) => "av1_nvenc",
            (H264, Qsv) => "h264_qsv",
            (H265, Qsv) => "hevc_qsv",
            (Av1, Qsv) => "av1_qsv",
            (H264, Amf) => "h264_amf",
            (H265, Amf) => "hevc_amf",
            (Av1, Amf) => "av1_amf",
            (Vp9, Nvenc | Qsv | Amf) => return None,
        })
    }
}

/// How the video bitrate is chosen.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RateControl {
    /// Constant quality in the encoder's own scale: `-crf` for x264/x265/SVT-AV1/VP9,
    /// `-cq` for NVENC, `-global_quality` for QSV, `-qp_i/-qp_p` for AMF (the encoder
    /// documents the exact mapping and the useful range per backend).
    Quality(u8),
    /// The Convert → Advanced quality, mapped per codec.
    Preset(VideoQuality),
    /// Two-pass average bitrate (CPU) / VBR with that target (GPU), kbps.
    TargetKbps(u32),
}

/// What happens to the audio tracks (the same for every track; AVI keeps only the first).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioPlan {
    /// No audio in the output.
    None,
    /// `-c:a copy`.
    Copy,
    Aac {
        kbps: u32,
    },
    Opus {
        kbps: u32,
    },
    Mp3 {
        kbps: u32,
    },
}

/// A part of the input, seconds.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Range {
    pub start_s: f64,
    pub length_s: f64,
}

/// Everything [`encode`] needs.
#[derive(Debug, Clone, PartialEq)]
pub struct EncodeSettings {
    pub encoder: Encoder,
    pub rate: RateControl,
    /// Scale down to at most this height (keeps aspect, even sizes); never upscales.
    pub max_height: Option<u32>,
    pub audio: AudioPlan,
    /// Picks the muxer (`-f`) and container flags (e.g. `+faststart` for MP4/MOV).
    pub container: VideoContainer,
}

/// Encode `input` (all of it, or `range`) to `output` (a temp path, e.g. an
/// [`crate::naming::OutputFile`] temp file or a file in `ctx.temp_dir()`),
/// keeping metadata and, where the container allows, subtitles.
/// `on_progress` gets 0–1 over the whole encode (both passes for two-pass).
/// Returns the stderr tail (for notes/details).
pub fn encode(
    input: &Path,
    info: &MediaInfo,
    settings: &EncodeSettings,
    range: Option<Range>,
    output: &Path,
    ctx: &TaskCtx<'_>,
    on_progress: &mut dyn FnMut(f32),
) -> Result<String, ConvertError> {
    let cpu = EncodeSettings {
        encoder: Encoder::cpu(settings.encoder.codec),
        ..settings.clone()
    };
    if usable(settings.encoder) != settings.encoder {
        return encode_once(input, info, &cpu, range, output, ctx, on_progress);
    }
    match encode_once(input, info, settings, range, output, ctx, on_progress) {
        Err(e) if settings.encoder.backend != Backend::Cpu && gpu_may_be_at_fault(&e) => {
            on_progress(0.0);
            let retry = encode_once(input, info, &cpu, range, output, ctx, on_progress);
            // The CPU managed what the GPU didn't: skip that GPU encoder from now on.
            if retry.is_ok()
                && let Ok(mut bad) = FAILED_GPU.lock()
                && !bad.contains(&settings.encoder)
            {
                bad.push(settings.encoder);
            }
            retry
        }
        other => other,
    }
}

/// GPU encoders that failed an encode in this process.
static FAILED_GPU: Mutex<Vec<Encoder>> = Mutex::new(Vec::new());

/// The encoder [`encode`] really uses for `encoder`: the CPU one of the same
/// codec once that GPU encoder has failed in this process, else `encoder`.
pub fn usable(encoder: Encoder) -> Encoder {
    let failed = FAILED_GPU.lock().is_ok_and(|bad| bad.contains(&encoder));
    if failed {
        Encoder::cpu(encoder.codec)
    } else {
        encoder
    }
}

/// Worth retrying on the CPU: not cancel, not a bad input, not a full/locked disk.
fn gpu_may_be_at_fault(e: &ConvertError) -> bool {
    !matches!(e.kind, ErrorKind::Cancelled | ErrorKind::BadInput)
        && e.message != ffmpeg::DISK_FULL
        && e.message != ConvertError::not_writable("").message
}

fn encode_once(
    input: &Path,
    info: &MediaInfo,
    settings: &EncodeSettings,
    range: Option<Range>,
    output: &Path,
    ctx: &TaskCtx<'_>,
    on_progress: &mut dyn FnMut(f32),
) -> Result<String, ConvertError> {
    let duration = match range {
        Some(r) => Some(match info.duration_s {
            Some(d) => r.length_s.min((d - r.start_s).max(0.1)),
            None => r.length_s,
        }),
        None => info.duration_s,
    };
    if !two_pass(settings) {
        let args = build_args(input, info, settings, range, output, Pass::Single)?;
        return Ok(ffmpeg::run(&args, duration, ctx, on_progress)?.log);
    }
    static LOGS: AtomicU32 = AtomicU32::new(0);
    // Relative: ffmpeg runs in ctx.temp_dir() (and x265's stats path can't hold "C:").
    let log = format!("pass-{}", LOGS.fetch_add(1, Ordering::Relaxed));
    let first = build_args(
        input,
        info,
        settings,
        range,
        output,
        Pass::First(log.clone()),
    )?;
    ffmpeg::run(&first, duration, ctx, &mut |f| on_progress(f * 0.5))?;
    ctx.check_cancel()?;
    let second = build_args(input, info, settings, range, output, Pass::Second(log))?;
    let out = ffmpeg::run(&second, duration, ctx, &mut |f| on_progress(0.5 + f * 0.5))?;
    Ok(out.log)
}

/// Which run of a (possibly two-pass) encode.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Pass {
    Single,
    /// Analysis pass: video only, to the null muxer; the log name prefix.
    First(String),
    Second(String),
}

fn two_pass(settings: &EncodeSettings) -> bool {
    matches!(settings.rate, RateControl::TargetKbps(_))
        && settings.encoder.backend == Backend::Cpu
        && settings.encoder.codec != Codec::Av1
}

/// Quality number for a Convert → Advanced quality, in the encoder's own scale.
pub fn preset_quality(encoder: Encoder, quality: VideoQuality) -> u8 {
    use Backend::*;
    use Codec::*;
    let table: [u8; 3] = match (encoder.backend, encoder.codec) {
        (Cpu, H264) => [20, 23, 28],
        (Cpu, H265) => [22, 26, 30],
        (Nvenc | Qsv, H264) => [21, 25, 30],
        (Nvenc | Qsv, H265) => [23, 27, 31],
        (Amf, H264) => [20, 24, 29],
        (Amf, H265) => [22, 26, 30],
        (_, Av1) => [28, 34, 40],
        (_, Vp9) => [28, 33, 38],
    };
    table[match quality {
        VideoQuality::High => 0,
        VideoQuality::Standard => 1,
        VideoQuality::Small => 2,
    }]
}

fn os(list: &[&str]) -> Vec<OsString> {
    list.iter().map(OsString::from).collect()
}

fn build_args(
    input: &Path,
    info: &MediaInfo,
    settings: &EncodeSettings,
    range: Option<Range>,
    output: &Path,
    pass: Pass,
) -> Result<Vec<OsString>, ConvertError> {
    let video = info.video().ok_or_else(super::no_video_track)?;
    let name = settings.encoder.ffmpeg_name().ok_or_else(|| {
        ConvertError::unsupported(
            "This encoder isn't available in the included FFmpeg.",
            format!("{:?}", settings.encoder),
        )
    })?;
    let mut a: Vec<OsString> = Vec::new();
    if let Some(r) = range {
        a.extend(os(&["-ss", &format!("{:.3}", r.start_s)]));
        a.extend(os(&["-t", &format!("{:.3}", r.length_s)]));
    }
    a.push("-i".into());
    a.push(ffmpeg::path_arg(input));
    a.extend(os(&["-map", &format!("0:{}", video.index)]));
    let first_pass = matches!(pass, Pass::First(_));
    let audio = info.audio().filter(|_| !first_pass);
    if audio.is_some() && settings.audio != AudioPlan::None {
        // Every audio track (one plan for all), like a remux. AVI: the first
        // only (players rarely offer a track choice there).
        let keep = if settings.container == VideoContainer::Avi {
            1
        } else {
            usize::MAX
        };
        for s in info.audio_streams().take(keep) {
            a.extend(os(&["-map", &format!("0:{}", s.index)]));
        }
    }
    if !first_pass {
        a.extend(subtitle_args(info, settings.container));
        a.extend(os(&["-map_metadata", "0"]));
    }

    // Video.
    a.extend(os(&["-c:v", name]));
    if let Some(h) = settings.max_height {
        let taller = video.display_size().is_none_or(|(_, vh)| vh > h);
        if taller {
            a.extend(os(&["-vf", &format!("scale=-2:'min(ih,{h})'")]));
        }
    }
    let ten_bit = video.pix_fmt.as_deref().is_some_and(|p| p.contains("10"));
    if settings.encoder.codec == Codec::H264 || !ten_bit {
        a.extend(os(&["-pix_fmt", "yuv420p"]));
    }
    a.extend(video_args(settings, &pass));

    // Audio.
    if first_pass || audio.is_none() {
        a.push("-an".into());
    } else {
        a.extend(match settings.audio {
            AudioPlan::None => os(&["-an"]),
            AudioPlan::Copy => os(&["-c:a", "copy"]),
            AudioPlan::Aac { kbps } => os(&["-c:a", "aac", "-b:a", &format!("{kbps}k")]),
            AudioPlan::Opus { kbps } => os(&["-c:a", "libopus", "-b:a", &format!("{kbps}k")]),
            AudioPlan::Mp3 { kbps } => os(&["-c:a", "libmp3lame", "-b:a", &format!("{kbps}k")]),
        });
    }

    if first_pass {
        a.extend(os(&["-f", "null", "-"]));
        return Ok(a);
    }
    let mp4ish = matches!(
        settings.container,
        VideoContainer::Mp4 | VideoContainer::Mov
    );
    if mp4ish {
        a.extend(os(&["-movflags", "+faststart"]));
        if settings.encoder.codec == Codec::H265 {
            // Plays in Windows' Media Player / Apple players.
            a.extend(os(&["-tag:v", "hvc1"]));
        }
    }
    let muxer = ffmpeg::muxer(settings.container.extension()).unwrap_or("mp4");
    a.extend(os(&["-f", muxer]));
    a.push(ffmpeg::path_arg(output));
    Ok(a)
}

/// Encoder speed preset + rate control.
fn video_args(settings: &EncodeSettings, pass: &Pass) -> Vec<OsString> {
    use Backend::*;
    use Codec::*;
    let enc = settings.encoder;
    let mut a = match (enc.backend, enc.codec) {
        (Cpu, H264 | H265) => os(&["-preset", "medium"]),
        (Cpu, Av1) => os(&["-preset", "8"]),
        (Cpu, Vp9) => os(&["-deadline", "good", "-cpu-used", "4", "-row-mt", "1"]),
        (Nvenc, _) => os(&["-preset", "p5"]),
        (Qsv, _) => os(&["-preset", "medium"]),
        (Amf, _) => os(&["-quality", "balanced"]),
    };
    let x265_log = |extra: &str| {
        let params = if extra.is_empty() {
            "log-level=error".to_owned()
        } else {
            format!("log-level=error:{extra}")
        };
        os(&["-x265-params", &params])
    };
    let q = match settings.rate {
        RateControl::Quality(q) => Some(q),
        RateControl::Preset(p) => Some(preset_quality(enc, p)),
        RateControl::TargetKbps(_) => None,
    };
    if let Some(q) = q {
        let q = q.to_string();
        a.extend(match enc.backend {
            Cpu if enc.codec == Vp9 => os(&["-crf", &q, "-b:v", "0"]),
            Cpu => os(&["-crf", &q]),
            Nvenc => os(&["-rc", "vbr", "-cq", &q, "-b:v", "0"]),
            Qsv => os(&["-global_quality", &q]),
            Amf if enc.codec == H264 => os(&["-rc", "cqp", "-qp_i", &q, "-qp_p", &q, "-qp_b", &q]),
            Amf => os(&["-rc", "cqp", "-qp_i", &q, "-qp_p", &q]),
        });
        if enc == Encoder::cpu(H265) {
            a.extend(x265_log(""));
        }
        return a;
    }
    let RateControl::TargetKbps(kbps) = settings.rate else {
        return a;
    };
    let kbps = kbps.max(1);
    a.extend(os(&["-b:v", &format!("{kbps}k")]));
    match enc.backend {
        Cpu => {}
        Nvenc => a.extend(os(&["-rc", "vbr"])),
        Qsv => {}
        Amf => a.extend(os(&["-rc", "vbr_peak"])),
    }
    if enc.backend != Cpu {
        a.extend(os(&[
            "-maxrate",
            &format!("{}k", u64::from(kbps) * 3 / 2),
            "-bufsize",
            &format!("{}k", u64::from(kbps) * 2),
        ]));
    }
    let pass_no = match pass {
        Pass::First(log) => Some((1, log)),
        Pass::Second(log) => Some((2, log)),
        Pass::Single => None,
    };
    match (enc, pass_no) {
        (e, Some((n, log))) if e == Encoder::cpu(H265) => {
            a.extend(x265_log(&format!("pass={n}:stats={log}.log")));
        }
        (_, Some((n, log))) => {
            a.extend(os(&["-pass", &n.to_string(), "-passlogfile", log]));
        }
        (e, None) if e == Encoder::cpu(H265) => a.extend(x265_log("")),
        _ => {}
    }
    a
}

/// Subtitle maps: only into MKV (text subs as-is or `mov_text` → SRT, bitmap subs copied).
pub(crate) fn subtitle_args(info: &MediaInfo, container: VideoContainer) -> Vec<OsString> {
    let mut a = Vec::new();
    if container != VideoContainer::Mkv {
        return a;
    }
    let subs = info
        .streams
        .iter()
        .filter(|s| s.kind == StreamKind::Subtitle);
    for (n, s) in subs
        .filter_map(|s| {
            let codec = match s.codec.as_str() {
                "subrip" | "ass" | "ssa" | "webvtt" | "hdmv_pgs_subtitle" | "dvd_subtitle"
                | "dvb_subtitle" => "copy",
                "mov_text" | "text" => "srt",
                _ => return None,
            };
            Some((s.index, codec))
        })
        .enumerate()
    {
        a.extend(os(&[
            "-map",
            &format!("0:{}", s.0),
            &format!("-c:s:{n}"),
            s.1,
        ]));
    }
    a
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encoder_names() {
        assert_eq!(Encoder::cpu(Codec::H264).ffmpeg_name(), Some("libx264"));
        assert_eq!(Encoder::cpu(Codec::Av1).ffmpeg_name(), Some("libsvtav1"));
        let nv = Encoder {
            codec: Codec::H265,
            backend: Backend::Nvenc,
        };
        assert_eq!(nv.ffmpeg_name(), Some("hevc_nvenc"));
        let vp9_gpu = Encoder {
            codec: Codec::Vp9,
            backend: Backend::Amf,
        };
        assert_eq!(vp9_gpu.ffmpeg_name(), None);
    }

    fn settings(encoder: Encoder, rate: RateControl) -> EncodeSettings {
        EncodeSettings {
            encoder,
            rate,
            max_height: Some(720),
            audio: AudioPlan::Aac { kbps: 128 },
            container: VideoContainer::Mp4,
        }
    }

    fn info() -> MediaInfo {
        let json = br#"{"format":{"duration":"2.0","format_name":"mov"},"streams":[
            {"index":0,"codec_type":"video","codec_name":"h264","width":1920,"height":1080},
            {"index":1,"codec_type":"audio","codec_name":"aac"}]}"#;
        super::super::probe::parse(json).unwrap()
    }

    fn args(s: &EncodeSettings, pass: Pass) -> String {
        let a = build_args(
            Path::new("in.mp4"),
            &info(),
            s,
            None,
            Path::new("out"),
            pass,
        )
        .unwrap();
        a.iter()
            .map(|x| x.to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join(" ")
    }

    #[test]
    fn quality_args_per_backend() {
        let cpu = args(
            &settings(
                Encoder::cpu(Codec::H264),
                RateControl::Preset(VideoQuality::Standard),
            ),
            Pass::Single,
        );
        assert!(
            cpu.contains(
                "-c:v libx264 -vf scale=-2:'min(ih,720)' -pix_fmt yuv420p -preset medium -crf 23"
            ),
            "{cpu}"
        );
        assert!(
            cpu.contains("-c:a aac -b:a 128k -movflags +faststart -f mp4 file:out"),
            "{cpu}"
        );
        let nv = Encoder {
            codec: Codec::H265,
            backend: Backend::Nvenc,
        };
        let a = args(&settings(nv, RateControl::Quality(27)), Pass::Single);
        assert!(
            a.contains("-rc vbr -cq 27 -b:v 0") && a.contains("-tag:v hvc1"),
            "{a}"
        );
        let amf = Encoder {
            codec: Codec::H264,
            backend: Backend::Amf,
        };
        assert!(
            args(&settings(amf, RateControl::Quality(24)), Pass::Single)
                .contains("-rc cqp -qp_i 24 -qp_p 24 -qp_b 24")
        );
    }

    #[test]
    fn two_pass_args() {
        let s = settings(Encoder::cpu(Codec::H264), RateControl::TargetKbps(1000));
        assert!(two_pass(&s));
        let first = args(&s, Pass::First("pass-7".into()));
        assert!(
            first.contains("-b:v 1000k -pass 1 -passlogfile pass-7 -an -f null -"),
            "{first}"
        );
        assert!(!first.contains("-map 0:1"), "{first}");
        let second = args(&s, Pass::Second("pass-7".into()));
        assert!(
            second.contains("-map 0:1") && second.contains("-pass 2"),
            "{second}"
        );
        let x265 = settings(Encoder::cpu(Codec::H265), RateControl::TargetKbps(800));
        assert!(
            args(&x265, Pass::First("p".into()))
                .contains("-x265-params log-level=error:pass=1:stats=p.log")
        );
        let nv = settings(
            Encoder {
                codec: Codec::H264,
                backend: Backend::Nvenc,
            },
            RateControl::TargetKbps(1000),
        );
        assert!(!two_pass(&nv));
        assert!(
            args(&nv, Pass::Single).contains("-b:v 1000k -rc vbr -maxrate 1500k -bufsize 2000k")
        );
    }
}
