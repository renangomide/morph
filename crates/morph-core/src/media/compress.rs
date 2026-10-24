// SPDX-License-Identifier: GPL-3.0-or-later
//! Video → Compress (SPEC §4.3): presets Balanced / Smaller / Discord / Email /
//! Custom size. "Best (automatic)" is in `best.rs`.
//!
//! - Balanced (H.264, CRF 23-equivalent) and Smaller (H.265 CRF 28) never write
//!   a file that isn't ≥ 5% smaller: note "Already well compressed", no output.
//! - Size presets: skip with "Already under 10 MB" when the original fits;
//!   else a target bitrate from [`plan_target`] (two-pass on the CPU), one retry
//!   ≥ 10% lower ([`retry_kbps`]) if the result is still over the target.
//! - Output: [`container_for`] (MP4/MOV/MKV/WebM kept, everything else → MP4);
//!   these presets write H.264/H.265, so WebM becomes MP4 here (Best keeps it).
//! - Audio (Balanced/Smaller): [`super::best::audio_plan`], like Best.

use super::encode::{self, AudioPlan, Codec, EncodeSettings, RateControl};
use super::gpu;
use super::options::{VideoCompressOpts, VideoCompressPreset, VideoContainer, VideoQuality};
use super::probe::MediaInfo;
use crate::action::Action;
use crate::converter::{Converter, TaskCtx, TaskOutput};
use crate::error::{ConvertError, ErrorKind};
use crate::job::Task;
use crate::naming::{OutputFile, Suffix};
use std::path::Path;

/// `video.compress`.
pub struct VideoCompress;

/// Note when a quality preset couldn't save at least 5%.
pub const WELL_COMPRESSED_NOTE: &str = "Already well compressed";
/// Bytes per "MB" of a size preset (Discord's 10 MB = 10 000 000 bytes).
const MB: u64 = 1_000_000;
/// Share of the target given to the streams (the rest: container overhead, rate misses).
const AIM: f64 = 0.96;
/// Below this many bits per pixel per frame the picture falls apart: step the resolution down.
const MIN_BPP: f64 = 0.05;
/// Lowest useful video bitrate, kbps.
const MIN_VIDEO_KBPS: f64 = 100.0;
/// Highest video bitrate we ask for, kbps (a huge target on a short clip).
const MAX_VIDEO_KBPS: f64 = 200_000.0;
/// Step-down ladder (heights).
const LADDER: [u32; 4] = [1080, 720, 480, 360];

impl Converter for VideoCompress {
    fn run(&self, task: &Task, ctx: &TaskCtx<'_>) -> Result<TaskOutput, ConvertError> {
        let Action::VideoCompress(opts) = &task.action else {
            return Err(super::wrong_action("VideoCompress"));
        };
        if opts.preset == VideoCompressPreset::Best {
            return super::best::compress_best(task, opts, ctx);
        }
        let (input, info) = super::open(task, ctx)?;
        if !info.has_video() {
            return Err(super::no_video_track());
        }
        let size = std::fs::metadata(input).map(|m| m.len()).map_err(|e| {
            ConvertError::new(ErrorKind::BadInput, "The file was moved or deleted.", e)
        })?;
        let container = match container_for(input) {
            VideoContainer::Webm => VideoContainer::Mp4,
            c => c,
        };
        let out = super::output_for(task, input, container.extension(), Suffix::Compressed)?;
        let job = Compress {
            input,
            info: &info,
            size,
            opts,
            container,
            ctx,
        };
        match opts.preset.target_mb(opts.target_mb) {
            Some(mb) => job.to_size(mb, out),
            None => job.by_quality(out),
        }
    }
}

/// Compress output container (all presets): MP4/MOV/MKV/WebM stay, the rest
/// (AVI, WMV, MTS, …) becomes MP4.
pub fn container_for(input: &Path) -> VideoContainer {
    match VideoContainer::of_path(input) {
        Some(
            c @ (VideoContainer::Mp4
            | VideoContainer::Mov
            | VideoContainer::Mkv
            | VideoContainer::Webm),
        ) => c,
        _ => VideoContainer::Mp4,
    }
}

struct Compress<'a> {
    input: &'a Path,
    info: &'a MediaInfo,
    size: u64,
    opts: &'a VideoCompressOpts,
    container: VideoContainer,
    ctx: &'a TaskCtx<'a>,
}

impl Compress<'_> {
    fn encode(
        &self,
        settings: &EncodeSettings,
        out: &OutputFile,
        progress: &mut dyn FnMut(f32),
    ) -> Result<u64, ConvertError> {
        let (input, info, ctx) = (self.input, self.info, self.ctx);
        encode::encode(input, info, settings, None, out.temp_path(), ctx, progress)?;
        Ok(std::fs::metadata(out.temp_path()).map_or(u64::MAX, |m| m.len()))
    }

    fn by_quality(&self, out: OutputFile) -> Result<TaskOutput, ConvertError> {
        let (codec, rate) = if self.opts.preset == VideoCompressPreset::Smaller {
            (Codec::H265, RateControl::Quality(28))
        } else {
            (Codec::H264, RateControl::Preset(VideoQuality::Standard))
        };
        let audio = super::best::audio_plan(self.info, self.container);
        let settings = EncodeSettings {
            encoder: gpu::pick_encoder(codec, self.opts.encoder, self.ctx),
            rate,
            max_height: self.opts.resolution.max_height(),
            audio,
            container: self.container,
        };
        let written = self.encode(&settings, &out, &mut |f| self.ctx.progress(f, None))?;
        if written as f64 > self.size as f64 * 0.95 {
            // `out` is dropped here: the temp file is deleted.
            return Ok(TaskOutput::default().with_note(WELL_COMPRESSED_NOTE));
        }
        Ok(TaskOutput::single(out.commit()?))
    }

    fn to_size(&self, mb: u32, out: OutputFile) -> Result<TaskOutput, ConvertError> {
        let target = u64::from(mb) * MB;
        if self.size <= target {
            return Ok(TaskOutput::default().with_note(format!("Already under {mb} MB")));
        }
        let duration = self.info.duration_s.ok_or_else(|| {
            ConvertError::new(
                ErrorKind::BadInput,
                "Couldn't read the length of this video.",
                "no duration",
            )
        })?;
        let video = self.info.video().ok_or_else(super::no_video_track)?;
        let source = Source {
            size: video.display_size().unwrap_or((1920, 1080)),
            fps: video.fps.filter(|f| *f > 0.0).unwrap_or(30.0),
        };
        let plan = plan_target(
            target,
            duration,
            source,
            self.opts.resolution.max_height(),
            audio_tracks(self.info, self.container),
        )
        .ok_or_else(|| {
            ConvertError::new(
                ErrorKind::BadInput,
                format!("This video is too long to fit in {mb} MB."),
                format!("{duration:.1} s into {target} bytes"),
            )
        })?;
        let mut settings = EncodeSettings {
            encoder: gpu::pick_encoder(Codec::H264, self.opts.encoder, self.ctx),
            rate: RateControl::TargetKbps(plan.video_kbps),
            max_height: plan.max_height,
            audio: if plan.audio_kbps == 0 {
                AudioPlan::None
            } else {
                AudioPlan::Aac {
                    kbps: plan.audio_kbps,
                }
            },
            container: self.container,
        };
        let ctx = self.ctx;
        let mut written = self.encode(&settings, &out, &mut |f| ctx.progress(f * 0.9, None))?;
        if written > target {
            // One retry, at least 10% lower, more if it overshot more (GPU
            // one-pass VBR on short clips); the second run gets the rest of the bar.
            settings.rate = RateControl::TargetKbps(retry_kbps(plan.video_kbps, written, target));
            written = self.encode(&settings, &out, &mut |f| ctx.progress(0.9 + f * 0.1, None))?;
            if written > target {
                return Err(ConvertError::other(
                    format!("Couldn't make this video small enough to fit in {mb} MB."),
                    format!("{written} bytes > {target} bytes at {:?}", settings.rate),
                ));
            }
        }
        let output = TaskOutput::single(out.commit()?);
        Ok(match plan.lowered_to {
            Some(h) => output.with_note(format!("Resolution lowered to {h}p to fit {mb} MB.")),
            None => output,
        })
    }
}

/// The picture being compressed.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Source {
    /// Displayed width × height.
    pub size: (u32, u32),
    pub fps: f64,
}

/// Bitrates and size for a size preset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TargetPlan {
    pub video_kbps: u32,
    /// Per audio track; 0 = no audio track.
    pub audio_kbps: u32,
    /// Scale cap for the encode (`None` = keep).
    pub max_height: Option<u32>,
    /// Set when we went below the source/option height to fit (for the note).
    pub lowered_to: Option<u32>,
}

/// Audio tracks an encode to `container` keeps (see `encode::AudioPlan`).
fn audio_tracks(info: &MediaInfo, container: VideoContainer) -> u32 {
    let n = info.audio_streams().count() as u32;
    if container == VideoContainer::Avi {
        n.min(1)
    } else {
        n
    }
}

/// `total_kbps = target × 96% × 8 / duration`; audio AAC 128 kbps per track (96
/// when the total is under 1 Mbit/s); video gets the rest (at most
/// [`MAX_VIDEO_KBPS`]). Steps the height down
/// 1080 → 720 → 480 → 360 while the video has under [`MIN_BPP`] bits per pixel
/// per frame; never upscales. `None`: under 100 kbps of video (too long).
pub fn plan_target(
    target_bytes: u64,
    duration_s: f64,
    source: Source,
    cap: Option<u32>,
    audio_tracks: u32,
) -> Option<TargetPlan> {
    let total_kbps = target_bytes as f64 * AIM * 8.0 / 1000.0 / duration_s.max(0.1);
    let audio_kbps = match (audio_tracks, total_kbps < 1000.0) {
        (0, _) => 0,
        (_, true) => 96,
        (_, false) => 128,
    };
    let video_kbps =
        (total_kbps - f64::from(audio_kbps) * f64::from(audio_tracks)).min(MAX_VIDEO_KBPS);
    if video_kbps < MIN_VIDEO_KBPS {
        return None;
    }
    let (w, h) = (source.size.0.max(1), source.size.1.max(1));
    let start = cap.map_or(h, |c| c.min(h));
    let bpp = |height: u32| {
        let width = f64::from(w) * f64::from(height) / f64::from(h);
        video_kbps * 1000.0 / (width * f64::from(height) * source.fps)
    };
    let candidates = std::iter::once(start).chain(LADDER.into_iter().filter(|&l| l < start));
    let mut height = start;
    for c in candidates {
        height = c;
        if bpp(c) >= MIN_BPP {
            break;
        }
    }
    Some(TargetPlan {
        video_kbps: video_kbps as u32,
        audio_kbps,
        max_height: (height < h).then_some(height),
        lowered_to: (height < start).then_some(height),
    })
}

/// Video bitrate for the one retry after `written` came out over `target`:
/// scaled by the overshoot (aiming at 95%), and at least 10% lower.
pub fn retry_kbps(kbps: u32, written: u64, target: u64) -> u32 {
    let scale = (target as f64 * 0.95 / written.max(1) as f64).min(0.9);
    (f64::from(kbps) * scale) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retry_scales_with_overshoot() {
        // 2% over: the 10% floor.
        assert_eq!(retry_kbps(1000, 1_020_000, 1_000_000), 900);
        // 25% over: 0.95 / 1.25 = 0.76.
        assert_eq!(retry_kbps(1000, 1_250_000, 1_000_000), 760);
    }

    const FHD: Source = Source {
        size: (1920, 1080),
        fps: 25.0,
    };

    #[test]
    fn bitrate_math() {
        // 10 MB over 58 s: 1324 kbps total, 128 audio → 1196 video.
        let p = plan_target(10 * MB, 58.0, FHD, None, 1).unwrap();
        assert_eq!((p.audio_kbps, p.video_kbps), (128, 1196));
        // 0.023 bpp at 1080p, 0.052 at 720p.
        assert_eq!(p.max_height, Some(720));
        assert_eq!(p.lowered_to, Some(720));
        // No audio: all to video.
        let p = plan_target(10 * MB, 58.0, FHD, None, 0).unwrap();
        assert_eq!((p.audio_kbps, p.video_kbps), (0, 1324));
    }

    #[test]
    fn tight_budget_uses_96k_audio_and_steps_down() {
        // 10 MB over 120 s: 640 kbps total → 96 audio, 544 video → 480p (0.053 bpp).
        let p = plan_target(10 * MB, 120.0, FHD, None, 1).unwrap();
        assert_eq!((p.audio_kbps, p.video_kbps), (96, 544));
        assert_eq!(p.lowered_to, Some(480));
        // Very long: ends at 360p even below MIN_BPP.
        let p = plan_target(10 * MB, 300.0, FHD, None, 1).unwrap();
        assert_eq!(p.lowered_to, Some(360));
        // Way too long: under 100 kbps of video.
        assert_eq!(plan_target(10 * MB, 900.0, FHD, None, 1), None);
    }

    #[test]
    fn plenty_of_bits_keeps_resolution() {
        let p = plan_target(25 * MB, 20.0, FHD, None, 1).unwrap();
        assert_eq!((p.max_height, p.lowered_to), (None, None));
        // The Resolution option caps without a "lowered" note.
        let p = plan_target(25 * MB, 20.0, FHD, Some(720), 1).unwrap();
        assert_eq!((p.max_height, p.lowered_to), (Some(720), None));
    }

    #[test]
    fn huge_targets_and_several_tracks() {
        // 4 GB in 1 s: capped, no overflow downstream.
        let p = plan_target(4_000 * MB, 1.0, FHD, None, 1).unwrap();
        assert_eq!(p.video_kbps, 200_000);
        // Two tracks: 2 × 128 kbps of audio.
        let p = plan_target(10 * MB, 58.0, FHD, None, 2).unwrap();
        assert_eq!((p.audio_kbps, p.video_kbps), (128, 1068));
    }

    #[test]
    fn containers() {
        let c = |p: &str| container_for(Path::new(p));
        assert_eq!(c("a.MOV"), VideoContainer::Mov);
        assert_eq!(c("a.webm"), VideoContainer::Webm);
        assert_eq!(c("a.m4v"), VideoContainer::Mp4);
        assert_eq!(c("a.avi"), VideoContainer::Mp4);
        assert_eq!(c("a.wmv"), VideoContainer::Mp4);
    }

    #[test]
    fn never_upscales() {
        let small = Source {
            size: (640, 360),
            fps: 30.0,
        };
        // Cap above the source: keep 360.
        let p = plan_target(10 * MB, 60.0, small, Some(1080), 1).unwrap();
        assert_eq!((p.max_height, p.lowered_to), (None, None));
        // Starved 360p source: nothing lower on the ladder, stays.
        let p = plan_target(10 * MB, 600.0, small, None, 0).unwrap();
        assert_eq!((p.max_height, p.lowered_to), (None, None));
    }
}
