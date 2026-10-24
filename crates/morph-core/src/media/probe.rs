// SPDX-License-Identifier: GPL-3.0-or-later
//! `ffprobe -show_format -show_streams -of json` → [`MediaInfo`].

use super::ffmpeg;
use crate::cancel::CancelToken;
use crate::error::ConvertError;
use serde_json::Value;
use std::ffi::OsString;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamKind {
    Video,
    Audio,
    Subtitle,
    Data,
    Attachment,
    Other,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StreamInfo {
    /// Stream index in the file (`-map 0:<index>`).
    pub index: u32,
    pub kind: StreamKind,
    /// FFmpeg codec name: `h264`, `hevc`, `aac`, `mp3`, `opus`, `mjpeg`, …
    pub codec: String,
    pub profile: Option<String>,
    pub pix_fmt: Option<String>,
    /// Bits per second, if known (often missing for MKV streams).
    pub bit_rate: Option<u64>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    /// Frames per second (`avg_frame_rate`, else `r_frame_rate`).
    pub fps: Option<f64>,
    pub sample_rate: Option<u32>,
    pub channels: Option<u32>,
    /// Display rotation in degrees (phone videos: 90 / -90 / 180), 0 if none.
    pub rotation: i32,
    /// Cover art (`disposition.attached_pic`), not a real video track.
    pub is_cover: bool,
}

impl StreamInfo {
    /// Width × height as shown (rotation applied).
    pub fn display_size(&self) -> Option<(u32, u32)> {
        let (w, h) = (self.width?, self.height?);
        Some(if self.rotation.rem_euclid(180) == 90 {
            (h, w)
        } else {
            (w, h)
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct MediaInfo {
    /// Seconds, if known.
    pub duration_s: Option<f64>,
    /// `format_name`, e.g. `mov,mp4,m4a,3gp,3g2,mj2` or `matroska,webm`.
    pub container: String,
    pub size_bytes: Option<u64>,
    /// Overall bits per second.
    pub bit_rate: Option<u64>,
    pub streams: Vec<StreamInfo>,
}

impl MediaInfo {
    /// The first real video stream (cover art doesn't count).
    pub fn video(&self) -> Option<&StreamInfo> {
        self.streams
            .iter()
            .find(|s| s.kind == StreamKind::Video && !s.is_cover)
    }

    /// The first audio stream.
    pub fn audio(&self) -> Option<&StreamInfo> {
        self.streams.iter().find(|s| s.kind == StreamKind::Audio)
    }

    /// Every audio track, in file order.
    pub fn audio_streams(&self) -> impl Iterator<Item = &StreamInfo> {
        self.streams.iter().filter(|s| s.kind == StreamKind::Audio)
    }

    pub fn has_video(&self) -> bool {
        self.video().is_some()
    }

    pub fn has_audio(&self) -> bool {
        self.audio().is_some()
    }

    /// Cover art stream (audio files), if any.
    pub fn cover(&self) -> Option<&StreamInfo> {
        self.streams.iter().find(|s| s.is_cover)
    }
}

/// Probe a file. Errors: FFmpeg missing, or a plain "damaged" message.
pub fn probe(path: &Path, cancel: &CancelToken) -> Result<MediaInfo, ConvertError> {
    let tools = ffmpeg::tools()?;
    let args: Vec<OsString> = [
        "-hide_banner",
        "-v",
        "error",
        "-show_format",
        "-show_streams",
        "-of",
        "json",
    ]
    .iter()
    .map(OsString::from)
    .chain([ffmpeg::path_arg(path)])
    .collect();
    let out = ffmpeg::capture(&tools.ffprobe, &args, cancel, ffmpeg::PROBE_TIMEOUT)?;
    if !out.success {
        return Err(match ffmpeg::failed(&out.log) {
            e if e.kind == crate::error::ErrorKind::Other => super::damaged_media(&out.log),
            e => e,
        });
    }
    let info = parse(&out.stdout).map_err(|e| super::damaged_media(format!("ffprobe: {e}")))?;
    if info.streams.is_empty() {
        return Err(super::damaged_media("no streams"));
    }
    Ok(info)
}

/// Parse ffprobe's JSON.
pub fn parse(json: &[u8]) -> Result<MediaInfo, String> {
    let root: Value = serde_json::from_slice(json).map_err(|e| e.to_string())?;
    let format = root.get("format").cloned().unwrap_or(Value::Null);
    let streams = root
        .get("streams")
        .and_then(Value::as_array)
        .map(|list| list.iter().map(parse_stream).collect())
        .unwrap_or_default();
    Ok(MediaInfo {
        duration_s: num(&format, "duration").filter(|d| *d > 0.0),
        container: text(&format, "format_name").unwrap_or_default(),
        size_bytes: int(&format, "size"),
        bit_rate: int(&format, "bit_rate"),
        streams,
    })
}

fn parse_stream(s: &Value) -> StreamInfo {
    let kind = match s.get("codec_type").and_then(Value::as_str) {
        Some("video") => StreamKind::Video,
        Some("audio") => StreamKind::Audio,
        Some("subtitle") => StreamKind::Subtitle,
        Some("data") => StreamKind::Data,
        Some("attachment") => StreamKind::Attachment,
        _ => StreamKind::Other,
    };
    let fps = rate(s, "avg_frame_rate").or_else(|| rate(s, "r_frame_rate"));
    let rotation = s
        .get("side_data_list")
        .and_then(Value::as_array)
        .and_then(|l| l.iter().find_map(|d| d.get("rotation")?.as_f64()))
        .or_else(|| s.get("tags").and_then(|t| num(t, "rotate")))
        .map_or(0, |r| r.round() as i32);
    StreamInfo {
        index: int(s, "index").unwrap_or(0) as u32,
        kind,
        codec: text(s, "codec_name").unwrap_or_default(),
        profile: text(s, "profile"),
        pix_fmt: text(s, "pix_fmt"),
        bit_rate: int(s, "bit_rate"),
        width: int(s, "width").map(|v| v as u32),
        height: int(s, "height").map(|v| v as u32),
        fps,
        sample_rate: int(s, "sample_rate").map(|v| v as u32),
        channels: int(s, "channels").map(|v| v as u32),
        rotation,
        is_cover: s
            .get("disposition")
            .and_then(|d| d.get("attached_pic"))
            .and_then(Value::as_i64)
            == Some(1),
    }
}

fn text(v: &Value, key: &str) -> Option<String> {
    v.get(key)?
        .as_str()
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

/// ffprobe writes many numbers as strings (`"duration": "3.000000"`).
fn num(v: &Value, key: &str) -> Option<f64> {
    let field = v.get(key)?;
    field
        .as_f64()
        .or_else(|| field.as_str()?.trim().parse().ok())
        .filter(|n: &f64| n.is_finite())
}

fn int(v: &Value, key: &str) -> Option<u64> {
    let field = v.get(key)?;
    field
        .as_u64()
        .or_else(|| field.as_str()?.trim().parse().ok())
}

/// `"30000/1001"` → 29.97; `"0/0"` → None.
fn rate(v: &Value, key: &str) -> Option<f64> {
    let (n, d) = v.get(key)?.as_str()?.split_once('/')?;
    let (n, d): (f64, f64) = (n.parse().ok()?, d.parse().ok()?);
    (n > 0.0 && d > 0.0).then(|| n / d)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{
      "streams": [
        { "index": 0, "codec_name": "h264", "profile": "High", "codec_type": "video",
          "width": 1920, "height": 1080, "pix_fmt": "yuv420p",
          "r_frame_rate": "30/1", "avg_frame_rate": "30000/1001", "bit_rate": "4500000",
          "side_data_list": [ { "side_data_type": "Display Matrix", "rotation": -90 } ],
          "disposition": { "default": 1, "attached_pic": 0 } },
        { "index": 1, "codec_name": "aac", "codec_type": "audio", "sample_rate": "48000",
          "channels": 2, "bit_rate": "128000", "disposition": { "attached_pic": 0 } },
        { "index": 2, "codec_name": "mjpeg", "codec_type": "video", "width": 600, "height": 600,
          "avg_frame_rate": "0/0", "disposition": { "attached_pic": 1 } }
      ],
      "format": { "format_name": "mov,mp4,m4a,3gp,3g2,mj2", "duration": "12.500000",
                  "size": "7340032", "bit_rate": "4697620" }
    }"#;

    #[test]
    fn parses_ffprobe_json() {
        let info = parse(SAMPLE.as_bytes()).unwrap();
        assert_eq!(info.duration_s, Some(12.5));
        assert_eq!(info.container, "mov,mp4,m4a,3gp,3g2,mj2");
        assert_eq!(info.size_bytes, Some(7_340_032));
        assert_eq!(info.streams.len(), 3);
        let v = info.video().unwrap();
        assert_eq!(
            (v.codec.as_str(), v.width, v.height),
            ("h264", Some(1920), Some(1080))
        );
        assert!((v.fps.unwrap() - 29.97).abs() < 0.01);
        assert_eq!(v.bit_rate, Some(4_500_000));
        assert_eq!(v.rotation, -90);
        assert_eq!(v.display_size(), Some((1080, 1920)));
        let a = info.audio().unwrap();
        assert_eq!((a.sample_rate, a.channels), (Some(48_000), Some(2)));
        assert!(info.has_audio() && info.has_video());
        assert_eq!(info.cover().map(|c| c.index), Some(2));
        assert_eq!(info.streams[2].fps, None);
    }

    #[test]
    fn audio_only_and_missing_fields() {
        let info = parse(
            br#"{"streams":[{"index":0,"codec_type":"audio","codec_name":"flac"}],
                 "format":{"format_name":"flac","duration":"N/A"}}"#,
        )
        .unwrap();
        assert!(!info.has_video() && info.has_audio());
        assert_eq!(info.duration_s, None);
        assert_eq!(info.audio().unwrap().sample_rate, None);
        assert!(parse(b"not json").is_err());
        assert!(parse(b"{}").unwrap().streams.is_empty());
    }
}
