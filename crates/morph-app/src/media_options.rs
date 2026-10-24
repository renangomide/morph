// SPDX-License-Identifier: GPL-3.0-or-later
//! Video and audio option panels (SPEC §4.3, §4.4) as plain Rust, apart from Slint.
//!
//! Same shape as [`crate::office_options`]: [`MediaForm`] keeps one options
//! struct per media action, [`MediaValues`] is the flat widget view the window
//! binds to, and [`MediaValues::apply`] turns it back into real options.
//!
//! A selection with both videos and audio files shows two tabs; one Convert runs
//! the video tab's action on the videos and Audio → Convert on the audio files.

use crate::strings;
use morph_core::media::audio::is_lossy_input;
use morph_core::media::options::{
    AudioConvertOpts, AudioFormat, Channels, EncoderChoice, GIF_FPS, GIF_MAX_LENGTH_S, GIF_WIDTHS,
    Mp3Mode, Resolution, SampleRate, VideoAudioCodec, VideoCodec, VideoCompressOpts,
    VideoCompressPreset, VideoContainer, VideoConvertOpts, VideoQuality, VideoToGifOpts,
};
use morph_core::state::State;
use morph_core::{Action, Job, OutputSpec, Task, TaskId};
use morph_formats::{Category, actions};
use serde_json::{Map, Value};
use std::path::PathBuf;

/// The video actions of the action selector, in display order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum VideoKind {
    #[default]
    Convert,
    Compress,
    ExtractAudio,
    ToGif,
}

impl VideoKind {
    pub const ALL: [VideoKind; 4] = [
        VideoKind::Convert,
        VideoKind::Compress,
        VideoKind::ExtractAudio,
        VideoKind::ToGif,
    ];

    pub fn id(self) -> &'static str {
        match self {
            VideoKind::Convert => actions::VIDEO_CONVERT,
            VideoKind::Compress => actions::VIDEO_COMPRESS,
            VideoKind::ExtractAudio => actions::VIDEO_EXTRACT_AUDIO,
            VideoKind::ToGif => actions::VIDEO_TO_GIF,
        }
    }

    pub fn from_id(id: &str) -> Option<VideoKind> {
        VideoKind::ALL.into_iter().find(|k| k.id() == id)
    }

    pub fn index(self) -> i32 {
        index_of(&VideoKind::ALL, &self)
    }

    /// Out-of-range indexes mean Convert.
    pub fn from_index(index: i32) -> VideoKind {
        pick(&VideoKind::ALL, index).unwrap_or_default()
    }

    pub fn label(self) -> &'static str {
        match self {
            VideoKind::Convert => strings::VIDEO_ACTION_CONVERT,
            VideoKind::Compress => strings::VIDEO_ACTION_COMPRESS,
            VideoKind::ExtractAudio => strings::VIDEO_ACTION_EXTRACT_AUDIO,
            VideoKind::ToGif => strings::VIDEO_ACTION_TO_GIF,
        }
    }
}

/// Which tab the window shows (both only for a video + audio selection).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MediaTab {
    #[default]
    Video,
    Audio,
}

impl MediaTab {
    pub fn index(self) -> i32 {
        match self {
            MediaTab::Video => 0,
            MediaTab::Audio => 1,
        }
    }

    pub fn from_index(index: i32) -> MediaTab {
        if index == 1 {
            MediaTab::Audio
        } else {
            MediaTab::Video
        }
    }
}

// Widget lists, in the order of the matching `strings::*` arrays.
pub const VIDEO_CODECS: [VideoCodec; 5] = [
    VideoCodec::Auto,
    VideoCodec::H264,
    VideoCodec::H265,
    VideoCodec::Av1,
    VideoCodec::Vp9,
];
pub const VIDEO_AUDIO_CODECS: [VideoAudioCodec; 4] = [
    VideoAudioCodec::Auto,
    VideoAudioCodec::Aac,
    VideoAudioCodec::Opus,
    VideoAudioCodec::Mp3,
];
pub const VIDEO_QUALITIES: [VideoQuality; 3] = [
    VideoQuality::High,
    VideoQuality::Standard,
    VideoQuality::Small,
];
pub const ENCODERS: [EncoderChoice; 2] = [EncoderChoice::Auto, EncoderChoice::Cpu];
pub const PRESETS: [VideoCompressPreset; 6] = [
    VideoCompressPreset::Best,
    VideoCompressPreset::Balanced,
    VideoCompressPreset::Smaller,
    VideoCompressPreset::Discord,
    VideoCompressPreset::Email,
    VideoCompressPreset::Custom,
];
pub const RESOLUTIONS: [Resolution; 4] = [
    Resolution::Keep,
    Resolution::P1080,
    Resolution::P720,
    Resolution::P480,
];
pub const MP3_MODES: [Mp3Mode; 3] = [Mp3Mode::Cbr, Mp3Mode::V0, Mp3Mode::V2];
pub const SAMPLE_RATES: [SampleRate; 3] =
    [SampleRate::Keep, SampleRate::Hz44100, SampleRate::Hz48000];
pub const CHANNELS: [Channels; 2] = [Channels::Keep, Channels::Mono];
/// Extract audio offers every audio format but OGG (SPEC §4.3).
pub const EXTRACT_FORMATS: [AudioFormat; 5] = [
    AudioFormat::Mp3,
    AudioFormat::M4a,
    AudioFormat::Wav,
    AudioFormat::Flac,
    AudioFormat::Opus,
];
/// Audio → Convert formats.
pub const AUDIO_FORMATS: [AudioFormat; 6] = AudioFormat::ALL;

/// Options of every media action, plus the selected tab and video action.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct MediaForm {
    pub tab: MediaTab,
    pub video_kind: VideoKind,
    pub convert: VideoConvertOpts,
    pub compress: VideoCompressOpts,
    pub extract: AudioConvertOpts,
    pub gif: VideoToGifOpts,
    pub audio: AudioConvertOpts,
}

impl MediaForm {
    /// Pre-fill the window for a request of action `id`: every action gets
    /// defaults → remembered options (`state`) → and, for `id` only, the menu
    /// preset on top. A bitrate that doesn't fit its format becomes the format's
    /// default. `files` decide the starting tab of a mixed selection.
    pub fn prefill(
        state: &State,
        id: &str,
        preset: &Map<String, Value>,
        files: &[PathBuf],
    ) -> MediaForm {
        let mut form = MediaForm::default();
        let empty = Map::new();
        for other in VideoKind::ALL
            .iter()
            .map(|k| k.id())
            .chain([actions::AUDIO_CONVERT])
        {
            let p = if other == id { preset } else { &empty };
            if let Some(action) = state.action_for(other, p) {
                form.set_action(action);
            }
        }
        form.video_kind = VideoKind::from_id(id).unwrap_or_default();
        let has_video = count(files, Category::Video) > 0;
        form.tab =
            if id == actions::AUDIO_CONVERT || (!has_video && VideoKind::from_id(id).is_none()) {
                MediaTab::Audio
            } else {
                MediaTab::Video
            };
        fit_bitrate(&mut form.extract, &EXTRACT_FORMATS);
        fit_bitrate(&mut form.audio, &AUDIO_FORMATS);
        form
    }

    /// Store `action`'s options (does not change the selection).
    pub fn set_action(&mut self, action: Action) {
        match action {
            Action::VideoConvert(o) => self.convert = o,
            Action::VideoCompress(o) => self.compress = o,
            Action::VideoExtractAudio(o) => self.extract = o,
            Action::VideoToGif(o) => self.gif = o,
            Action::AudioConvert(o) => self.audio = o,
            // Not a media action: nothing to store here.
            _ => {}
        }
    }

    /// The video tab's selected action.
    pub fn video_action(&self) -> Action {
        match self.video_kind {
            VideoKind::Convert => Action::VideoConvert(self.convert.clone()),
            VideoKind::Compress => Action::VideoCompress(self.compress.clone()),
            VideoKind::ExtractAudio => Action::VideoExtractAudio(self.extract.clone()),
            VideoKind::ToGif => Action::VideoToGif(self.gif.clone()),
        }
    }

    pub fn audio_action(&self) -> Action {
        Action::AudioConvert(self.audio.clone())
    }

    /// The actions that run for `files`: the video action when there are
    /// videos, Audio → Convert when there are audio files. Never empty (the
    /// shown tab's action for a selection with neither).
    pub fn actions(&self, files: &[PathBuf]) -> Vec<Action> {
        let mut list = Vec::new();
        if count(files, Category::Video) > 0 {
            list.push(self.video_action());
        }
        if count(files, Category::Audio) > 0 {
            list.push(self.audio_action());
        }
        if list.is_empty() {
            list.push(match self.tab {
                MediaTab::Video => self.video_action(),
                MediaTab::Audio => self.audio_action(),
            });
        }
        list
    }
}

/// A lossy bitrate that isn't offered for the format → the format's default.
fn fit_bitrate(o: &mut AudioConvertOpts, formats: &[AudioFormat]) {
    if !formats.contains(&o.format) {
        o.format = AudioFormat::default();
    }
    let list = o.format.bitrates();
    if !list.is_empty() && !list.contains(&o.bitrate_kbps) {
        o.bitrate_kbps = o.format.default_bitrate();
    }
}

/// How many of `files` are in `category`.
pub fn count(files: &[PathBuf], category: Category) -> usize {
    files
        .iter()
        .filter(|f| Category::from_path(f) == Some(category))
        .count()
}

/// The selection has both videos and audio files (show the tabs).
pub fn is_mixed(files: &[PathBuf]) -> bool {
    count(files, Category::Video) > 0 && count(files, Category::Audio) > 0
}

/// Any selected audio file is lossy (the lossless warning applies).
pub fn has_lossy_audio(files: &[PathBuf]) -> bool {
    files
        .iter()
        .any(|f| Category::from_path(f) == Some(Category::Audio) && is_lossy_input(f))
}

/// One job for the whole selection: one task per file with the action its
/// category takes (task IDs in order), everything else skipped.
pub fn build_job(files: &[PathBuf], actions: &[Action], output: &OutputSpec) -> Job {
    let mut job = Job::default();
    let mut tasks: Vec<Task> = Vec::new();
    for file in files {
        let category = Category::from_path(file);
        match actions
            .iter()
            .find(|a| category.is_some_and(|c| a.accepts(c)))
        {
            Some(action) => tasks.push(Task {
                id: TaskId(0),
                inputs: vec![file.clone()],
                action: action.clone(),
                output: output.clone(),
            }),
            None => job
                .skipped
                .push((file.clone(), strings::NOT_SUPPORTED_HERE.to_owned())),
        }
    }
    // Videos first, then audio files: rows read by type, like the tabs.
    tasks.sort_by_key(|t| matches!(t.action, Action::AudioConvert(_)));
    for (i, task) in tasks.iter_mut().enumerate() {
        task.id = TaskId(i as u64);
    }
    job.tasks = tasks;
    job
}

// ---------------------------------------------------------------- time

/// "1:30" → 90. Also "90" (seconds) and "1:02:03". Seconds (and minutes
/// after an hour) must be below 60.
pub fn parse_time(text: &str) -> Option<u32> {
    let text = text.trim();
    if text.is_empty() {
        return Some(0);
    }
    let parts: Vec<&str> = text.split(':').map(str::trim).collect();
    if parts.len() > 3 || parts.iter().any(|p| p.is_empty()) {
        return None;
    }
    let nums: Vec<u32> = parts
        .iter()
        .map(|p| {
            p.chars()
                .all(|c| c.is_ascii_digit())
                .then(|| p.parse().ok())
                .flatten()
        })
        .collect::<Option<_>>()?;
    let mut total: u32 = 0;
    for (i, n) in nums.iter().enumerate() {
        if i > 0 && *n >= 60 {
            return None;
        }
        total = total.checked_mul(60)?.checked_add(*n)?;
    }
    Some(total)
}

/// 90 → "1:30", 3723 → "1:02:03".
pub fn format_time(seconds: u32) -> String {
    let (h, m, s) = (seconds / 3600, seconds / 60 % 60, seconds % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// Live check of the GIF start box: `None` = fine, else the message.
pub fn start_error(text: &str) -> Option<&'static str> {
    parse_time(text).is_none().then_some(strings::BAD_GIF_START)
}

// ---------------------------------------------------------------- widgets

/// Flat values of the audio option widgets (Audio → Convert, Extract audio).
#[derive(Debug, Clone, PartialEq)]
pub struct AudioValues {
    /// Index in the panel's format list.
    pub format: i32,
    /// Index in the format's `bitrates()`.
    pub bitrate: i32,
    pub mp3_mode: i32,
    pub sample_rate: i32,
    pub channels: i32,
    pub keep_tags: bool,
}

impl AudioValues {
    fn from_opts(o: &AudioConvertOpts, formats: &[AudioFormat]) -> AudioValues {
        AudioValues {
            format: index_of(formats, &o.format),
            bitrate: index_of(o.format.bitrates(), &o.bitrate_kbps),
            mp3_mode: index_of(&MP3_MODES, &o.mp3_mode),
            sample_rate: index_of(&SAMPLE_RATES, &o.sample_rate),
            channels: index_of(&CHANNELS, &o.channels),
            keep_tags: o.keep_tags,
        }
    }

    fn apply(&self, o: &mut AudioConvertOpts, formats: &[AudioFormat]) {
        let format = pick(formats, self.format).unwrap_or(o.format);
        let bitrate_kbps = match format.bitrates() {
            [] => o.bitrate_kbps,
            list => pick(list, self.bitrate).unwrap_or(format.default_bitrate()),
        };
        *o = AudioConvertOpts {
            format,
            bitrate_kbps,
            mp3_mode: pick(&MP3_MODES, self.mp3_mode).unwrap_or(o.mp3_mode),
            sample_rate: pick(&SAMPLE_RATES, self.sample_rate).unwrap_or(o.sample_rate),
            channels: pick(&CHANNELS, self.channels).unwrap_or(o.channels),
            keep_tags: self.keep_tags,
        };
    }
}

/// Flat values of the media option widgets.
#[derive(Debug, Clone, PartialEq)]
pub struct MediaValues {
    pub tab: i32,
    pub video_kind: i32,
    // Convert
    pub container: i32,
    pub fast: bool,
    pub video_codec: i32,
    pub audio_codec: i32,
    pub quality: i32,
    pub convert_encoder: i32,
    // Compress
    pub preset: i32,
    pub target_mb: String,
    pub resolution: i32,
    pub compress_encoder: i32,
    // Extract audio / Audio → Convert
    pub extract: AudioValues,
    pub audio: AudioValues,
    // GIF
    pub gif_start: String,
    pub gif_length: i32,
    /// Index in `GIF_WIDTHS`.
    pub gif_width: i32,
    /// Index in `GIF_FPS`.
    pub gif_fps: i32,
}

impl MediaValues {
    pub fn from_form(form: &MediaForm) -> MediaValues {
        let c = &form.convert;
        let k = &form.compress;
        let g = &form.gif;
        MediaValues {
            tab: form.tab.index(),
            video_kind: form.video_kind.index(),
            container: index_of(&VideoContainer::ALL, &c.format),
            fast: c.fast,
            video_codec: index_of(&VIDEO_CODECS, &c.video_codec),
            audio_codec: index_of(&VIDEO_AUDIO_CODECS, &c.audio_codec),
            quality: index_of(&VIDEO_QUALITIES, &c.quality),
            convert_encoder: index_of(&ENCODERS, &c.encoder),
            preset: index_of(&PRESETS, &k.preset),
            target_mb: k.target_mb.to_string(),
            resolution: index_of(&RESOLUTIONS, &k.resolution),
            compress_encoder: index_of(&ENCODERS, &k.encoder),
            extract: AudioValues::from_opts(&form.extract, &EXTRACT_FORMATS),
            audio: AudioValues::from_opts(&form.audio, &AUDIO_FORMATS),
            gif_start: format_time(g.start_s),
            gif_length: to_i32(g.length_s),
            gif_width: index_of(&GIF_WIDTHS, &g.width),
            gif_fps: index_of(&GIF_FPS, &g.fps),
        }
    }

    /// Write the widget values into `form`. Only the actions that run for
    /// `files` are checked; an error is a plain-English message for the window.
    pub fn apply(&self, form: &mut MediaForm, files: &[PathBuf]) -> Result<(), String> {
        form.tab = MediaTab::from_index(self.tab);
        form.video_kind = VideoKind::from_index(self.video_kind);
        let c = &mut form.convert;
        *c = VideoConvertOpts {
            format: pick(&VideoContainer::ALL, self.container).unwrap_or(c.format),
            fast: self.fast,
            video_codec: pick(&VIDEO_CODECS, self.video_codec).unwrap_or(c.video_codec),
            audio_codec: pick(&VIDEO_AUDIO_CODECS, self.audio_codec).unwrap_or(c.audio_codec),
            quality: pick(&VIDEO_QUALITIES, self.quality).unwrap_or(c.quality),
            encoder: pick(&ENCODERS, self.convert_encoder).unwrap_or(c.encoder),
        };
        let k = &mut form.compress;
        let preset = pick(&PRESETS, self.preset).unwrap_or(k.preset);
        let target_mb = match self.target_mb.trim().parse::<u32>() {
            Ok(mb) if mb > 0 => Ok(mb),
            _ => Err(strings::BAD_TARGET_MB),
        };
        *k = VideoCompressOpts {
            preset,
            target_mb: target_mb.unwrap_or(k.target_mb),
            resolution: pick(&RESOLUTIONS, self.resolution).unwrap_or(k.resolution),
            encoder: pick(&ENCODERS, self.compress_encoder).unwrap_or(k.encoder),
        };
        self.extract.apply(&mut form.extract, &EXTRACT_FORMATS);
        self.audio.apply(&mut form.audio, &AUDIO_FORMATS);
        let start = parse_time(&self.gif_start).ok_or(strings::BAD_GIF_START);
        let length = u32::try_from(self.gif_length)
            .ok()
            .filter(|l| (1..=GIF_MAX_LENGTH_S).contains(l))
            .ok_or(strings::BAD_GIF_LENGTH);
        let g = &mut form.gif;
        *g = VideoToGifOpts {
            start_s: *start.as_ref().unwrap_or(&g.start_s),
            length_s: *length.as_ref().unwrap_or(&g.length_s),
            width: pick(&GIF_WIDTHS, self.gif_width).unwrap_or(g.width),
            fps: pick(&GIF_FPS, self.gif_fps).unwrap_or(g.fps),
        };
        let videos_run =
            count(files, Category::Video) > 0 || (files.is_empty() && form.tab == MediaTab::Video);
        if !videos_run {
            return Ok(());
        }
        match form.video_kind {
            VideoKind::Compress if preset == VideoCompressPreset::Custom => {
                target_mb.map(|_| ()).map_err(str::to_owned)
            }
            VideoKind::ToGif => start.and(length).map(|_| ()).map_err(str::to_owned),
            _ => Ok(()),
        }
    }
}

fn index_of<T: PartialEq>(list: &[T], value: &T) -> i32 {
    list.iter()
        .position(|v| v == value)
        .and_then(|i| i32::try_from(i).ok())
        .unwrap_or(0)
}

fn pick<T: Copy>(list: &[T], index: i32) -> Option<T> {
    usize::try_from(index)
        .ok()
        .and_then(|i| list.get(i).copied())
}

fn to_i32(v: u32) -> i32 {
    i32::try_from(v).unwrap_or(i32::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn files(names: &[&str]) -> Vec<PathBuf> {
        names
            .iter()
            .map(|n| PathBuf::from(r"C:\in").join(n))
            .collect()
    }

    fn preset(k: &str, v: &str) -> Map<String, Value> {
        let mut m = Map::new();
        m.insert(k.into(), Value::from(v));
        m
    }

    #[test]
    fn menu_entries_preselect_action_and_format() {
        let state = State::default();
        let clip = files(&["a.mp4"]);
        let form = MediaForm::prefill(
            &state,
            actions::VIDEO_CONVERT,
            &preset("format", "webm"),
            &clip,
        );
        assert_eq!(
            (form.tab, form.video_kind),
            (MediaTab::Video, VideoKind::Convert)
        );
        assert_eq!(form.convert.format, VideoContainer::Webm);
        assert!(form.convert.fast);
        let form = MediaForm::prefill(&state, actions::VIDEO_COMPRESS, &Map::new(), &clip);
        assert_eq!(form.video_kind, VideoKind::Compress);
        assert_eq!(
            form.compress.preset,
            VideoCompressPreset::Best,
            "SPEC default"
        );
        let form = MediaForm::prefill(&state, actions::VIDEO_EXTRACT_AUDIO, &Map::new(), &clip);
        assert_eq!(form.video_kind, VideoKind::ExtractAudio);
        assert_eq!(
            (form.extract.format, form.extract.bitrate_kbps),
            (AudioFormat::Mp3, 192)
        );
        let form = MediaForm::prefill(&state, actions::VIDEO_TO_GIF, &Map::new(), &clip);
        assert_eq!(form.video_kind, VideoKind::ToGif);
        assert_eq!(form.gif, VideoToGifOpts::default());
        let song = files(&["a.flac"]);
        let form = MediaForm::prefill(
            &state,
            actions::AUDIO_CONVERT,
            &preset("format", "m4a"),
            &song,
        );
        assert_eq!(form.tab, MediaTab::Audio);
        assert_eq!(form.audio.format, AudioFormat::M4a);
        // Mixed: videos first.
        let both = files(&["a.mp3", "b.mkv"]);
        let form = MediaForm::prefill(&state, actions::MIXED_CONVERT, &Map::new(), &both);
        assert_eq!(form.tab, MediaTab::Video);
        let form = MediaForm::prefill(&state, actions::MIXED_CONVERT, &Map::new(), &song);
        assert_eq!(form.tab, MediaTab::Audio);
    }

    #[test]
    fn remembered_bitrate_that_does_not_fit_the_preset_format_resets() {
        let mut state = State::default();
        state.remember_action(&Action::AudioConvert(AudioConvertOpts {
            format: AudioFormat::Mp3,
            bitrate_kbps: 320,
            keep_tags: false,
            ..Default::default()
        }));
        let song = files(&["a.wav"]);
        let form = MediaForm::prefill(&state, actions::AUDIO_CONVERT, &Map::new(), &song);
        assert_eq!(form.audio.bitrate_kbps, 320);
        assert!(!form.audio.keep_tags);
        // "Convert to M4A…": 320 isn't an AAC choice.
        let form = MediaForm::prefill(
            &state,
            actions::AUDIO_CONVERT,
            &preset("format", "m4a"),
            &song,
        );
        assert_eq!(
            (form.audio.format, form.audio.bitrate_kbps),
            (AudioFormat::M4a, 192)
        );
        assert!(!form.audio.keep_tags, "other remembered options stay");
        // OGG isn't offered by Extract audio.
        let mut o = AudioConvertOpts {
            format: AudioFormat::Ogg,
            ..Default::default()
        };
        fit_bitrate(&mut o, &EXTRACT_FORMATS);
        assert_eq!(o.format, AudioFormat::Mp3);
    }

    fn sample_form() -> MediaForm {
        MediaForm {
            tab: MediaTab::Audio,
            video_kind: VideoKind::ToGif,
            convert: VideoConvertOpts {
                format: VideoContainer::Mkv,
                fast: false,
                video_codec: VideoCodec::Av1,
                audio_codec: VideoAudioCodec::Opus,
                quality: VideoQuality::High,
                encoder: EncoderChoice::Cpu,
            },
            compress: VideoCompressOpts {
                preset: VideoCompressPreset::Custom,
                target_mb: 8,
                resolution: Resolution::P720,
                encoder: EncoderChoice::Cpu,
            },
            extract: AudioConvertOpts {
                format: AudioFormat::Opus,
                bitrate_kbps: 96,
                channels: Channels::Mono,
                ..Default::default()
            },
            gif: VideoToGifOpts {
                start_s: 75,
                length_s: 4,
                width: 320,
                fps: 24,
            },
            audio: AudioConvertOpts {
                format: AudioFormat::Mp3,
                bitrate_kbps: 256,
                mp3_mode: Mp3Mode::V2,
                sample_rate: SampleRate::Hz48000,
                channels: Channels::Keep,
                keep_tags: false,
            },
        }
    }

    #[test]
    fn widget_values_round_trip() {
        let both = files(&["a.mp4", "b.mp3"]);
        for form in [MediaForm::default(), sample_form()] {
            let v = MediaValues::from_form(&form);
            let mut back = MediaForm::default();
            v.apply(&mut back, &both).unwrap();
            assert_eq!(back, form);
        }
        let v = MediaValues::from_form(&sample_form());
        assert_eq!(v.gif_start, "1:15");
        assert_eq!(v.gif_width, 3);
        // Lossless format: the bitrate box is hidden, the old value is kept.
        let mut form = sample_form();
        let mut v = MediaValues::from_form(&form);
        v.audio.format = index_of(&AUDIO_FORMATS, &AudioFormat::Flac);
        v.apply(&mut form, &both).unwrap();
        assert_eq!(
            (form.audio.format, form.audio.bitrate_kbps),
            (AudioFormat::Flac, 256)
        );
        // A new lossy format with a bitrate index out of its list: its default.
        v.audio.format = index_of(&AUDIO_FORMATS, &AudioFormat::Opus);
        v.audio.bitrate = 9;
        v.apply(&mut form, &both).unwrap();
        assert_eq!(form.audio.bitrate_kbps, 128);
    }

    #[test]
    fn remembered_settings_round_trip_through_state_toml() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.toml");
        let form = sample_form();
        let mut state = State::default();
        for kind in VideoKind::ALL {
            state.remember_action(
                &MediaForm {
                    video_kind: kind,
                    ..form.clone()
                }
                .video_action(),
            );
        }
        state.remember_action(&form.audio_action());
        state.save(&path).unwrap();
        let loaded = State::load(&path);
        let song = files(&["a.ogg"]);
        let back = MediaForm::prefill(&loaded, actions::VIDEO_TO_GIF, &Map::new(), &song);
        assert_eq!(
            back,
            MediaForm {
                tab: MediaTab::Video,
                ..form.clone()
            }
        );
        let mut off = loaded.clone();
        off.remember = false;
        let back = MediaForm::prefill(&off, actions::VIDEO_TO_GIF, &Map::new(), &song);
        assert_eq!(back.gif, VideoToGifOpts::default());
        assert_eq!(back.audio, AudioConvertOpts::default());
    }

    #[test]
    fn validation_only_for_what_runs() {
        let clip = files(&["a.mp4"]);
        let song = files(&["a.mp3"]);
        let mut form = MediaForm {
            video_kind: VideoKind::Compress,
            compress: VideoCompressOpts {
                preset: VideoCompressPreset::Custom,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut v = MediaValues::from_form(&form);
        for bad in ["0", "", "-3", "1.5", "abc"] {
            v.target_mb = bad.into();
            assert_eq!(
                v.apply(&mut form, &clip),
                Err(strings::BAD_TARGET_MB.to_owned()),
                "{bad:?}"
            );
            assert_eq!(form.compress.target_mb, 50, "keeps the old value");
        }
        assert_eq!(v.apply(&mut form, &song), Ok(()), "no video runs");
        v.preset = index_of(&PRESETS, &VideoCompressPreset::Discord);
        assert_eq!(v.apply(&mut form, &clip), Ok(()), "MB only for Custom");
        v.preset = index_of(&PRESETS, &VideoCompressPreset::Custom);
        v.target_mb = " 12 ".into();
        v.apply(&mut form, &clip).unwrap();
        assert_eq!(form.compress.target_mb, 12);

        v.video_kind = VideoKind::ToGif.index();
        v.target_mb = "0".into();
        assert_eq!(v.apply(&mut form, &clip), Ok(()), "MB only for Compress");
        for (start, length, ok) in [
            ("0:00", 10, true),
            ("1:30", 60, true),
            ("", 1, true),
            ("90", 5, true),
            ("1:75", 5, false),
            ("1:", 5, false),
            ("a:10", 5, false),
            ("0:10", 0, false),
            ("0:10", 61, false),
        ] {
            v.gif_start = start.into();
            v.gif_length = length;
            assert_eq!(v.apply(&mut form, &clip).is_ok(), ok, "{start:?} {length}");
        }
        v.gif_start = "2:05".into();
        v.gif_length = 3;
        v.apply(&mut form, &clip).unwrap();
        assert_eq!((form.gif.start_s, form.gif.length_s), (125, 3));
        v.gif_start = "x".into();
        assert_eq!(
            v.apply(&mut form, &clip),
            Err(strings::BAD_GIF_START.into())
        );
        v.gif_start = "0:01".into();
        v.gif_length = 99;
        assert_eq!(
            v.apply(&mut form, &clip),
            Err(strings::BAD_GIF_LENGTH.into())
        );
    }

    #[test]
    fn times() {
        assert_eq!(parse_time("0:00"), Some(0));
        assert_eq!(parse_time(" 1:30 "), Some(90));
        assert_eq!(parse_time("01:02:03"), Some(3723));
        assert_eq!(parse_time("75"), Some(75));
        assert_eq!(parse_time("1:60"), None);
        assert_eq!(parse_time("1:2:3:4"), None);
        assert_eq!(parse_time("+5"), None);
        assert_eq!(format_time(0), "0:00");
        assert_eq!(format_time(90), "1:30");
        assert_eq!(format_time(3723), "1:02:03");
        assert_eq!(start_error("1:30"), None);
        assert_eq!(start_error("1:3x"), Some(strings::BAD_GIF_START));
    }

    #[test]
    fn mixed_selection_runs_both_actions_in_one_job() {
        let sel = files(&["song.mp3", "clip.mp4", "doc.pdf", "b.wav", "c.mkv"]);
        assert!(is_mixed(&sel));
        assert!(!is_mixed(&files(&["a.mp4"])));
        let form = MediaForm {
            video_kind: VideoKind::Compress,
            ..Default::default()
        };
        let list = form.actions(&sel);
        assert_eq!(
            list.iter().map(Action::id).collect::<Vec<_>>(),
            [actions::VIDEO_COMPRESS, actions::AUDIO_CONVERT]
        );
        let job = build_job(&sel, &list, &OutputSpec::default());
        let ids: Vec<(u64, &str, String)> = job
            .tasks
            .iter()
            .map(|t| {
                (
                    t.id.0,
                    t.action.id(),
                    t.inputs[0]
                        .file_name()
                        .unwrap()
                        .to_string_lossy()
                        .into_owned(),
                )
            })
            .collect();
        assert_eq!(
            ids,
            [
                (0, actions::VIDEO_COMPRESS, "clip.mp4".into()),
                (1, actions::VIDEO_COMPRESS, "c.mkv".into()),
                (2, actions::AUDIO_CONVERT, "song.mp3".into()),
                (3, actions::AUDIO_CONVERT, "b.wav".into()),
            ]
        );
        assert_eq!(
            job.skipped,
            [(sel[2].clone(), strings::NOT_SUPPORTED_HERE.to_owned())]
        );
        // Only videos: only the video action.
        let clip = files(&["a.mov"]);
        assert_eq!(form.actions(&clip).len(), 1);
        assert_eq!(form.actions(&[]).len(), 1, "never empty");
    }

    #[test]
    fn lossy_inputs() {
        assert!(has_lossy_audio(&files(&["a.wav", "b.M4A"])));
        assert!(!has_lossy_audio(&files(&["a.wav", "b.flac", "c.mp4"])));
    }

    #[test]
    fn indexes() {
        for k in VideoKind::ALL {
            assert_eq!(VideoKind::from_index(k.index()), k);
            assert_eq!(VideoKind::from_id(k.id()), Some(k));
            assert_eq!(Action::default_for(k.id()).map(|a| a.id()), Some(k.id()));
        }
        assert_eq!(VideoKind::from_index(9), VideoKind::Convert);
        assert_eq!(
            MediaTab::from_index(MediaTab::Audio.index()),
            MediaTab::Audio
        );
        assert_eq!(strings::VIDEO_PRESETS.len(), PRESETS.len());
        assert_eq!(strings::VIDEO_PRESET_HINTS.len(), PRESETS.len());
        assert_eq!(strings::VIDEO_CODECS.len(), VIDEO_CODECS.len());
        assert_eq!(strings::VIDEO_AUDIO_CODECS.len(), VIDEO_AUDIO_CODECS.len());
        assert_eq!(strings::VIDEO_QUALITIES.len(), VIDEO_QUALITIES.len());
        assert_eq!(strings::VIDEO_ENCODERS.len(), ENCODERS.len());
        assert_eq!(strings::VIDEO_RESOLUTIONS.len(), RESOLUTIONS.len());
        assert_eq!(strings::VIDEO_CONTAINERS.len(), VideoContainer::ALL.len());
        assert_eq!(strings::MP3_MODES.len(), MP3_MODES.len());
        assert_eq!(strings::SAMPLE_RATES.len(), SAMPLE_RATES.len());
        assert_eq!(strings::CHANNEL_CHOICES.len(), CHANNELS.len());
    }
}
