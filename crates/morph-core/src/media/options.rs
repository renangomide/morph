// SPDX-License-Identifier: GPL-3.0-or-later
//! Video and audio action options (SPEC §4.3, §4.4). `Default` = the Settings
//! defaults: Convert → MP4 with fast mode, Compress "Best (automatic)", Keep
//! resolution, encoder Auto, audio MP3 192 kbps, GIF 0:00 + 10 s at 480 px, 15 fps.

use serde::{Deserialize, Serialize};
use std::path::Path;

// ---------------------------------------------------------------- Video convert

/// Video output containers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VideoContainer {
    #[default]
    Mp4,
    Mkv,
    Webm,
    Mov,
    Avi,
}

impl VideoContainer {
    pub const ALL: [VideoContainer; 5] = [
        VideoContainer::Mp4,
        VideoContainer::Mkv,
        VideoContainer::Webm,
        VideoContainer::Mov,
        VideoContainer::Avi,
    ];

    /// File extension (no dot).
    pub fn extension(self) -> &'static str {
        match self {
            VideoContainer::Mp4 => "mp4",
            VideoContainer::Mkv => "mkv",
            VideoContainer::Webm => "webm",
            VideoContainer::Mov => "mov",
            VideoContainer::Avi => "avi",
        }
    }

    /// The container an existing file already is, if it is one we write.
    pub fn of_path(path: &Path) -> Option<VideoContainer> {
        let ext = path.extension()?.to_str()?.to_ascii_lowercase();
        VideoContainer::ALL
            .into_iter()
            .find(|c| c.extension() == ext || (ext == "m4v" && *c == VideoContainer::Mp4))
    }
}

/// Video codec for a re-encode. `Auto` = the SPEC §4.3 table: MP4/MOV/MKV → H.264,
/// WebM → VP9, AVI → H.264.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VideoCodec {
    #[default]
    Auto,
    H264,
    H265,
    Av1,
    Vp9,
}

/// Audio codec for a re-encode. `Auto` = the SPEC §4.3 table: MP4/MOV/MKV → AAC,
/// WebM → Opus, AVI → MP3.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VideoAudioCodec {
    #[default]
    Auto,
    Aac,
    Opus,
    Mp3,
}

/// Re-encode quality (Convert → Advanced). Mapped to each codec's
/// CRF/CQ scale; `Standard` ≈ H.264 CRF 23.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VideoQuality {
    High,
    #[default]
    Standard,
    Small,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct VideoConvertOpts {
    pub format: VideoContainer,
    /// Remux with `-c copy` when the streams already fit the target container.
    pub fast: bool,
    pub video_codec: VideoCodec,
    pub audio_codec: VideoAudioCodec,
    pub quality: VideoQuality,
    pub encoder: EncoderChoice,
}

impl Default for VideoConvertOpts {
    fn default() -> Self {
        VideoConvertOpts {
            format: VideoContainer::Mp4,
            fast: true,
            video_codec: VideoCodec::Auto,
            audio_codec: VideoAudioCodec::Auto,
            quality: VideoQuality::Standard,
            encoder: EncoderChoice::Auto,
        }
    }
}

// ---------------------------------------------------------------- Video compress

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VideoCompressPreset {
    /// VMAF-guided H.264, never bigger than the original (approved 2026-10-20).
    #[default]
    Best,
    /// H.264 CRF 23-equivalent.
    Balanced,
    /// H.265 CRF 28.
    Smaller,
    /// ≤ 10 MB, two-pass target bitrate.
    Discord,
    /// ≤ 25 MB, two-pass target bitrate.
    Email,
    /// `target_mb`, two-pass target bitrate.
    Custom,
}

impl VideoCompressPreset {
    /// Target size in MB for the size presets (`custom_mb` for `Custom`).
    pub fn target_mb(self, custom_mb: u32) -> Option<u32> {
        match self {
            VideoCompressPreset::Discord => Some(10),
            VideoCompressPreset::Email => Some(25),
            VideoCompressPreset::Custom => Some(custom_mb.max(1)),
            _ => None,
        }
    }
}

/// Maximum output height. Never upscales: a smaller input keeps its size.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum Resolution {
    #[default]
    #[serde(rename = "keep")]
    Keep,
    #[serde(rename = "1080p")]
    P1080,
    #[serde(rename = "720p")]
    P720,
    #[serde(rename = "480p")]
    P480,
}

impl Resolution {
    /// Height limit in pixels, `None` for Keep.
    pub fn max_height(self) -> Option<u32> {
        match self {
            Resolution::Keep => None,
            Resolution::P1080 => Some(1080),
            Resolution::P720 => Some(720),
            Resolution::P480 => Some(480),
        }
    }
}

/// Settings → video encoder.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EncoderChoice {
    /// GPU if one works (NVENC → QSV → AMF), else CPU (x264/x265/SVT-AV1).
    #[default]
    Auto,
    /// Always the CPU encoders.
    Cpu,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct VideoCompressOpts {
    pub preset: VideoCompressPreset,
    /// Target size in MB; used only with [`VideoCompressPreset::Custom`].
    pub target_mb: u32,
    pub resolution: Resolution,
    pub encoder: EncoderChoice,
}

impl Default for VideoCompressOpts {
    fn default() -> Self {
        VideoCompressOpts {
            preset: VideoCompressPreset::Best,
            target_mb: 50,
            resolution: Resolution::Keep,
            encoder: EncoderChoice::Auto,
        }
    }
}

// ---------------------------------------------------------------- Audio

/// Audio output formats (Audio → Convert and Video → Extract audio).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AudioFormat {
    #[default]
    Mp3,
    /// AAC in an MP4 container.
    M4a,
    Wav,
    Flac,
    /// Vorbis.
    Ogg,
    Opus,
}

impl AudioFormat {
    pub const ALL: [AudioFormat; 6] = [
        AudioFormat::Mp3,
        AudioFormat::M4a,
        AudioFormat::Wav,
        AudioFormat::Flac,
        AudioFormat::Ogg,
        AudioFormat::Opus,
    ];

    /// File extension (no dot).
    pub fn extension(self) -> &'static str {
        match self {
            AudioFormat::Mp3 => "mp3",
            AudioFormat::M4a => "m4a",
            AudioFormat::Wav => "wav",
            AudioFormat::Flac => "flac",
            AudioFormat::Ogg => "ogg",
            AudioFormat::Opus => "opus",
        }
    }

    pub fn is_lossless(self) -> bool {
        matches!(self, AudioFormat::Wav | AudioFormat::Flac)
    }

    /// Bitrates offered for this format, kbps (empty for lossless). SPEC §4.4.
    pub fn bitrates(self) -> &'static [u32] {
        match self {
            AudioFormat::Mp3 => &[128, 192, 256, 320],
            AudioFormat::M4a => &[128, 160, 192, 256],
            AudioFormat::Ogg => &[128, 160, 192, 256],
            AudioFormat::Opus => &[64, 96, 128, 160, 192],
            AudioFormat::Wav | AudioFormat::Flac => &[],
        }
    }

    /// Default bitrate, kbps (0 for lossless).
    pub fn default_bitrate(self) -> u32 {
        match self {
            AudioFormat::Opus => 128,
            AudioFormat::Wav | AudioFormat::Flac => 0,
            _ => 192,
        }
    }
}

/// MP3 rate control. `Cbr` uses `bitrate_kbps`; VBR ignores it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mp3Mode {
    #[default]
    Cbr,
    /// LAME VBR V0 (~245 kbps).
    V0,
    /// LAME VBR V2 (~190 kbps).
    V2,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum SampleRate {
    #[default]
    #[serde(rename = "keep")]
    Keep,
    #[serde(rename = "44100")]
    Hz44100,
    #[serde(rename = "48000")]
    Hz48000,
}

impl SampleRate {
    pub fn hz(self) -> Option<u32> {
        match self {
            SampleRate::Keep => None,
            SampleRate::Hz44100 => Some(44_100),
            SampleRate::Hz48000 => Some(48_000),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Channels {
    #[default]
    Keep,
    Mono,
}

/// Audio → Convert and Video → Extract audio (SPEC: "same options").
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AudioConvertOpts {
    pub format: AudioFormat,
    /// Lossy formats only; see [`AudioFormat::bitrates`].
    pub bitrate_kbps: u32,
    pub mp3_mode: Mp3Mode,
    pub sample_rate: SampleRate,
    pub channels: Channels,
    /// Copy tags (`-map_metadata 0`) and cover art where the container allows.
    pub keep_tags: bool,
}

impl Default for AudioConvertOpts {
    fn default() -> Self {
        AudioConvertOpts {
            format: AudioFormat::Mp3,
            bitrate_kbps: 192,
            mp3_mode: Mp3Mode::Cbr,
            sample_rate: SampleRate::Keep,
            channels: Channels::Keep,
            keep_tags: true,
        }
    }
}

// ---------------------------------------------------------------- GIF

/// Longest GIF, seconds.
pub const GIF_MAX_LENGTH_S: u32 = 60;
/// Widths offered (0 = keep).
pub const GIF_WIDTHS: [u32; 4] = [0, 640, 480, 320];
/// Frame rates offered.
pub const GIF_FPS: [u32; 3] = [10, 15, 24];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct VideoToGifOpts {
    /// Start, seconds from the beginning (window shows mm:ss).
    pub start_s: u32,
    /// Length, seconds, 1–[`GIF_MAX_LENGTH_S`].
    pub length_s: u32,
    /// Output width in pixels, 0 = keep. Never upscales.
    pub width: u32,
    pub fps: u32,
}

impl Default for VideoToGifOpts {
    fn default() -> Self {
        VideoToGifOpts {
            start_s: 0,
            length_s: 10,
            width: 480,
            fps: 15,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_the_spec() {
        let c = VideoCompressOpts::default();
        assert_eq!(c.preset, VideoCompressPreset::Best);
        assert_eq!(
            (c.resolution, c.encoder),
            (Resolution::Keep, EncoderChoice::Auto)
        );
        let a = AudioConvertOpts::default();
        assert_eq!((a.format, a.bitrate_kbps), (AudioFormat::Mp3, 192));
        let g = VideoToGifOpts::default();
        assert_eq!((g.start_s, g.length_s, g.width, g.fps), (0, 10, 480, 15));
        assert!(VideoConvertOpts::default().fast);
    }

    #[test]
    fn serde_names() {
        let json = serde_json::to_value(VideoCompressOpts {
            resolution: Resolution::P720,
            ..Default::default()
        })
        .unwrap();
        assert_eq!(json["resolution"], "720p");
        assert_eq!(json["preset"], "best");
        let rate: SampleRate = serde_json::from_value("48000".into()).unwrap();
        assert_eq!(rate.hz(), Some(48_000));
        assert_eq!(VideoCompressPreset::Discord.target_mb(0), Some(10));
        assert_eq!(VideoCompressPreset::Custom.target_mb(0), Some(1));
        assert_eq!(VideoCompressPreset::Best.target_mb(5), None);
        assert_eq!(
            VideoContainer::of_path(Path::new("a.M4V")),
            Some(VideoContainer::Mp4)
        );
    }
}
