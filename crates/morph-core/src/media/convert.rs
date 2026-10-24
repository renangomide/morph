// SPDX-License-Identifier: GPL-3.0-or-later
//! Video → Convert (SPEC §4.3). Fast mode: when every stream already
//! fits the target container, remux with `-c copy` (note "Copied without
//! re-encoding."); a failed remux is retried once as a re-encode. Re-encode:
//! MP4/MOV/MKV → H.264 + AAC, WebM → VP9 + Opus, AVI → H.264 + MP3, unless
//! Advanced picks other codecs.

use super::encode::{self, AudioPlan, Codec, EncodeSettings, RateControl};
use super::options::{VideoAudioCodec, VideoCodec, VideoContainer};
use super::probe::{MediaInfo, StreamInfo, StreamKind};
use super::{ffmpeg, gpu};
use crate::action::Action;
use crate::converter::{Converter, TaskCtx, TaskOutput};
use crate::error::ConvertError;
use crate::job::Task;
use crate::naming::Suffix;
use std::ffi::OsString;
use std::path::Path;

/// `video.convert`.
pub struct VideoConvert;

/// Note for a remuxed output.
pub const COPIED_NOTE: &str = "Copied without re-encoding.";

impl Converter for VideoConvert {
    fn run(&self, task: &Task, ctx: &TaskCtx<'_>) -> Result<TaskOutput, ConvertError> {
        let Action::VideoConvert(opts) = &task.action else {
            return Err(super::wrong_action("VideoConvert"));
        };
        let (input, info) = super::open(task, ctx)?;
        if !info.has_video() {
            return Err(super::no_video_track());
        }
        let container = opts.format;
        // Check the Advanced choices before any work.
        let codec = video_codec(opts.video_codec, container)?;
        let audio = audio_plan(opts.audio_codec, container, &info)?;

        let fast = opts.fast && fits(container, &info);
        if fast && already(input, container) {
            // A remux would write a copy of the original: nothing to do.
            return Ok(TaskOutput::default().with_note(already_note(container)));
        }
        let out = super::output_for(task, input, container.extension(), Suffix::Converted)?;
        let mut progress = |f| ctx.progress(f, None);

        if fast {
            let args = remux_args(input, &info, container, out.temp_path());
            match ffmpeg::run(&args, info.duration_s, ctx, &mut progress) {
                Ok(_) => return Ok(TaskOutput::single(out.commit()?).with_note(COPIED_NOTE)),
                Err(e) if e.is_cancelled() => return Err(e),
                Err(_) => progress(0.0), // odd timestamps etc.: re-encode below
            }
        }
        let settings = EncodeSettings {
            encoder: gpu::pick_encoder(codec, opts.encoder, ctx),
            rate: RateControl::Preset(opts.quality),
            max_height: None,
            audio,
            container,
        };
        encode::encode(
            input,
            &info,
            &settings,
            None,
            out.temp_path(),
            ctx,
            &mut progress,
        )?;
        Ok(TaskOutput::single(out.commit()?))
    }
}

fn codec_label(c: Codec) -> &'static str {
    match c {
        Codec::H264 => "H.264",
        Codec::H265 => "H.265",
        Codec::Av1 => "AV1",
        Codec::Vp9 => "VP9",
    }
}

fn container_label(c: VideoContainer) -> String {
    c.extension().to_ascii_uppercase()
}

/// The input already has `container`'s extension (`.m4v` → MP4 still converts).
fn already(input: &Path, container: VideoContainer) -> bool {
    input
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case(container.extension()))
}

/// Note for a Fast-mode convert to the container the file already is
/// (no output, like "Already well compressed").
pub fn already_note(container: VideoContainer) -> String {
    format!("Already {}", container_label(container))
}

/// The video codec of a re-encode (Auto = the SPEC table), or "isn't supported".
pub fn video_codec(choice: VideoCodec, container: VideoContainer) -> Result<Codec, ConvertError> {
    use VideoContainer::*;
    let codec = match choice {
        VideoCodec::Auto => {
            return Ok(if container == Webm {
                Codec::Vp9
            } else {
                Codec::H264
            });
        }
        VideoCodec::H264 => Codec::H264,
        VideoCodec::H265 => Codec::H265,
        VideoCodec::Av1 => Codec::Av1,
        VideoCodec::Vp9 => Codec::Vp9,
    };
    let ok = match container {
        Mkv => true,
        Mp4 | Mov => codec != Codec::Vp9,
        Webm => matches!(codec, Codec::Vp9 | Codec::Av1),
        Avi => codec == Codec::H264,
    };
    if ok {
        Ok(codec)
    } else {
        Err(ConvertError::unsupported(
            format!(
                "{} video isn't supported in {} files.",
                codec_label(codec),
                container_label(container)
            ),
            format!("{codec:?} in {container:?}"),
        ))
    }
}

/// The audio of a re-encode. Auto copies a track that already is the
/// container's default codec (AAC / Opus / MP3), else encodes it.
pub fn audio_plan(
    choice: VideoAudioCodec,
    container: VideoContainer,
    info: &MediaInfo,
) -> Result<AudioPlan, ConvertError> {
    use VideoContainer::*;
    if !info.has_audio() {
        return Ok(AudioPlan::None);
    }
    let wanted = match choice {
        VideoAudioCodec::Auto => match container {
            Webm => VideoAudioCodec::Opus,
            Avi => VideoAudioCodec::Mp3,
            _ => VideoAudioCodec::Aac,
        },
        other => other,
    };
    let ok = match container {
        Mkv | Mp4 => true,
        Mov => wanted != VideoAudioCodec::Opus,
        Webm => wanted == VideoAudioCodec::Opus,
        Avi => wanted == VideoAudioCodec::Mp3,
    };
    let (plan, codec, label) = match wanted {
        VideoAudioCodec::Opus => (AudioPlan::Opus { kbps: 128 }, "opus", "Opus"),
        VideoAudioCodec::Mp3 => (AudioPlan::Mp3 { kbps: 192 }, "mp3", "MP3"),
        _ => (AudioPlan::Aac { kbps: 192 }, "aac", "AAC"),
    };
    if !ok {
        return Err(ConvertError::unsupported(
            format!(
                "{label} audio isn't supported in {} files.",
                container_label(container)
            ),
            format!("{wanted:?} in {container:?}"),
        ));
    }
    // Every kept track must already be it (`encode` applies one plan to all).
    Ok(if info.audio_streams().all(|s| s.codec == codec) {
        AudioPlan::Copy
    } else {
        plan
    })
}

/// Video and audio streams copied by a remux (no cover art).
fn remux_streams(info: &MediaInfo) -> impl Iterator<Item = &StreamInfo> {
    info.streams
        .iter()
        .filter(|s| (s.kind == StreamKind::Video && !s.is_cover) || s.kind == StreamKind::Audio)
}

/// Every video/audio stream can be copied into `container` as is.
pub fn fits(container: VideoContainer, info: &MediaInfo) -> bool {
    use VideoContainer::*;
    remux_streams(info).all(|s| {
        let c = s.codec.as_str();
        let video = s.kind == StreamKind::Video;
        match container {
            Mkv => true,
            Mp4 | Mov if video => matches!(c, "h264" | "hevc" | "av1" | "mpeg4"),
            Mp4 => matches!(c, "aac" | "mp3" | "ac3" | "alac" | "opus"),
            Mov => matches!(c, "aac" | "mp3" | "ac3" | "alac"),
            Webm if video => matches!(c, "vp8" | "vp9" | "av1"),
            Webm => matches!(c, "opus" | "vorbis"),
            Avi if video => matches!(c, "h264" | "mpeg4" | "mjpeg"),
            Avi => c == "mp3" || c == "ac3" || c.starts_with("pcm_"),
        }
    })
}

fn remux_args(
    input: &Path,
    info: &MediaInfo,
    container: VideoContainer,
    output: &Path,
) -> Vec<OsString> {
    let mut a: Vec<OsString> = vec!["-i".into(), ffmpeg::path_arg(input)];
    for s in remux_streams(info) {
        a.push("-map".into());
        a.push(format!("0:{}", s.index).into());
    }
    a.extend(encode::subtitle_args(info, container));
    for x in ["-map_metadata", "0", "-c:v", "copy", "-c:a", "copy"] {
        a.push(x.into());
    }
    if matches!(container, VideoContainer::Mp4 | VideoContainer::Mov) {
        a.push("-movflags".into());
        a.push("+faststart".into());
        if info.video().is_some_and(|v| v.codec == "hevc") {
            a.push("-tag:v".into());
            a.push("hvc1".into());
        }
    }
    a.push("-f".into());
    a.push(
        ffmpeg::muxer(container.extension())
            .unwrap_or("matroska")
            .into(),
    );
    a.push(ffmpeg::path_arg(output));
    a
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stream(index: u32, kind: StreamKind, codec: &str) -> StreamInfo {
        StreamInfo {
            index,
            kind,
            codec: codec.into(),
            profile: None,
            pix_fmt: None,
            bit_rate: None,
            width: None,
            height: None,
            fps: None,
            sample_rate: None,
            channels: None,
            rotation: 0,
            is_cover: false,
        }
    }

    fn info(v: &str, a: &str) -> MediaInfo {
        MediaInfo {
            duration_s: Some(1.0),
            container: String::new(),
            size_bytes: None,
            bit_rate: None,
            streams: vec![
                stream(0, StreamKind::Video, v),
                stream(1, StreamKind::Audio, a),
            ],
        }
    }

    #[test]
    fn remux_table() {
        use VideoContainer::*;
        assert!(fits(Mp4, &info("h264", "aac")));
        assert!(fits(Mkv, &info("vp9", "opus")));
        assert!(!fits(Mp4, &info("vp9", "opus")));
        assert!(fits(Mp4, &info("hevc", "opus")));
        assert!(!fits(Mov, &info("hevc", "opus")));
        assert!(fits(Webm, &info("vp9", "vorbis")));
        assert!(!fits(Webm, &info("h264", "opus")));
        assert!(fits(Avi, &info("mpeg4", "pcm_s16le")));
        assert!(!fits(Avi, &info("hevc", "mp3")));
    }

    #[test]
    fn codec_table() {
        use VideoContainer::*;
        assert_eq!(video_codec(VideoCodec::Auto, Webm).unwrap(), Codec::Vp9);
        assert_eq!(video_codec(VideoCodec::Auto, Avi).unwrap(), Codec::H264);
        let e = video_codec(VideoCodec::H265, Avi).unwrap_err();
        assert_eq!(e.message, "H.265 video isn't supported in AVI files.");
        assert!(video_codec(VideoCodec::Vp9, Mp4).is_err());
        assert!(video_codec(VideoCodec::Av1, Webm).is_ok());
        let i = info("vp9", "opus");
        assert_eq!(
            audio_plan(VideoAudioCodec::Auto, Mp4, &i).unwrap(),
            AudioPlan::Aac { kbps: 192 }
        );
        assert_eq!(
            audio_plan(VideoAudioCodec::Auto, Webm, &i).unwrap(),
            AudioPlan::Copy
        );
        assert!(audio_plan(VideoAudioCodec::Aac, Webm, &i).is_err());
    }
}
