// SPDX-License-Identifier: GPL-3.0-or-later
//! Settings page values as the UI holds them (switches, slider values, list
//! indices) and their mapping to and from [`Settings`]. No UI code here, so
//! the mapping is unit-tested.

use morph_core::Settings;
use morph_core::media::options::{EncoderChoice, VideoCompressPreset};
use morph_core::office::EnginePreference;
use morph_core::pdf::options::CompressPreset;
use morph_core::settings::{MAX_IMAGE_JOBS, Theme};
use std::path::PathBuf;

pub const PDF_PRESETS: [CompressPreset; 3] = [
    CompressPreset::Low,
    CompressPreset::Medium,
    CompressPreset::High,
];
/// Same order as `strings::DEFAULT_VIDEO_PRESETS`. "Custom size" needs a size, so it
/// isn't offered as a default.
pub const VIDEO_PRESETS: [VideoCompressPreset; 5] = [
    VideoCompressPreset::Best,
    VideoCompressPreset::Balanced,
    VideoCompressPreset::Smaller,
    VideoCompressPreset::Discord,
    VideoCompressPreset::Email,
];
/// No forced GPU (user decision 2026-10-21): Auto or CPU.
pub const VIDEO_ENCODERS: [EncoderChoice; 2] = [EncoderChoice::Auto, EncoderChoice::Cpu];
pub const MP3_BITRATES: [u32; 4] = [128, 192, 256, 320];
pub const PDF_DPIS: [u32; 3] = [72, 150, 300];
pub const ENGINES: [EnginePreference; 3] = EnginePreference::ALL;
pub const THEMES: [Theme; 3] = [Theme::System, Theme::Light, Theme::Dark];

/// Every editable value of the Settings page.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Form {
    pub images: bool,
    pub pdf: bool,
    pub video: bool,
    pub audio: bool,
    pub office: bool,
    pub jpeg_quality: i32,
    pub webp_quality: i32,
    pub avif_quality: i32,
    pub png_level: i32,
    pub pdf_preset: i32,
    pub video_preset: i32,
    pub video_encoder: i32,
    pub mp3_bitrate: i32,
    pub pdf_dpi: i32,
    /// 0 = next to the originals, 1 = fixed folder.
    pub output_mode: i32,
    pub folder: Option<PathBuf>,
    pub open_folder_when_done: bool,
    pub keep_file_dates: bool,
    pub engine: i32,
    /// 0 = automatic, n = n parallel jobs.
    pub image_jobs: i32,
    pub use_gpu: bool,
    pub toast: bool,
    pub theme: i32,
}

/// Index of `value` in `list`, else of `fallback`.
fn index_of<T: PartialEq>(list: &[T], value: &T, fallback: &T) -> i32 {
    let i = list
        .iter()
        .position(|v| v == value)
        .or_else(|| list.iter().position(|v| v == fallback))
        .unwrap_or(0);
    i32::try_from(i).unwrap_or(0)
}

/// `list[index]`, clamped to the list.
fn at<T: Copy>(list: &[T], index: i32) -> T {
    let last = list.len().saturating_sub(1);
    list[usize::try_from(index).unwrap_or(0).min(last)]
}

/// Nearest entry of `list` (for values typed into `config.toml` by hand).
fn nearest(list: &[u32], value: u32) -> i32 {
    let i = (0..list.len())
        .min_by_key(|&i| list[i].abs_diff(value))
        .unwrap_or(0);
    i32::try_from(i).unwrap_or(0)
}

fn quality(v: i32) -> u8 {
    u8::try_from(v.clamp(1, 100)).unwrap_or(80)
}

impl Form {
    pub fn from_settings(s: &Settings) -> Form {
        let d = &s.defaults;
        Form {
            images: s.menu.images,
            pdf: s.menu.pdf,
            video: s.menu.video,
            audio: s.menu.audio,
            office: s.menu.office,
            jpeg_quality: d.jpeg_quality.into(),
            webp_quality: d.webp_quality.into(),
            avif_quality: d.avif_quality.into(),
            png_level: d.png_level.min(6).into(),
            pdf_preset: index_of(&PDF_PRESETS, &d.pdf_preset, &CompressPreset::Medium),
            video_preset: index_of(&VIDEO_PRESETS, &d.video_preset, &VideoCompressPreset::Best),
            video_encoder: index_of(&VIDEO_ENCODERS, &d.video_encoder, &EncoderChoice::Auto),
            mp3_bitrate: nearest(&MP3_BITRATES, d.mp3_bitrate_kbps),
            pdf_dpi: nearest(&PDF_DPIS, d.pdf_dpi),
            output_mode: i32::from(s.output.fixed_folder().is_some()),
            folder: s.output.folder.clone(),
            open_folder_when_done: s.output.open_folder_when_done,
            keep_file_dates: s.output.keep_file_dates,
            engine: index_of(&ENGINES, &s.office.engine, &EnginePreference::Auto),
            image_jobs: i32::try_from(s.performance.image_jobs.min(MAX_IMAGE_JOBS)).unwrap_or(0),
            use_gpu: s.performance.use_gpu,
            toast: s.notifications.toast,
            theme: index_of(&THEMES, &s.appearance.theme, &Theme::System),
        }
    }

    /// `config.toml` may ask for more parallel jobs than the list shows
    /// (`shown_max`, this machine's cores); the list then shows its last entry.
    /// While that entry is untouched, keep the stored value.
    pub fn keep_unshown_jobs(&mut self, base: &Settings, shown_max: i32) {
        let stored = Form::from_settings(base).image_jobs;
        if stored > shown_max && self.image_jobs == shown_max {
            self.image_jobs = stored;
        }
    }

    /// `base` with this form's values. Values the page can't show (a hand-made
    /// "Custom" PDF preset, an odd DPI) are kept while their control is untouched.
    pub fn apply(&self, base: &Settings) -> Settings {
        let mut s = base.clone();
        let before = Form::from_settings(base);
        s.menu.images = self.images;
        s.menu.pdf = self.pdf;
        s.menu.video = self.video;
        s.menu.audio = self.audio;
        s.menu.office = self.office;
        let d = &mut s.defaults;
        d.jpeg_quality = quality(self.jpeg_quality);
        d.webp_quality = quality(self.webp_quality);
        d.avif_quality = quality(self.avif_quality);
        if self.png_level != before.png_level {
            d.png_level = u8::try_from(self.png_level.clamp(0, 6)).unwrap_or(2);
        }
        if self.pdf_preset != before.pdf_preset {
            d.pdf_preset = at(&PDF_PRESETS, self.pdf_preset);
        }
        if self.video_preset != before.video_preset {
            d.video_preset = at(&VIDEO_PRESETS, self.video_preset);
        }
        d.video_encoder = at(&VIDEO_ENCODERS, self.video_encoder);
        if self.mp3_bitrate != before.mp3_bitrate {
            d.mp3_bitrate_kbps = at(&MP3_BITRATES, self.mp3_bitrate);
        }
        if self.pdf_dpi != before.pdf_dpi {
            d.pdf_dpi = at(&PDF_DPIS, self.pdf_dpi);
        }
        // "Always in this folder" needs a folder; without one it stays off.
        s.output.folder = self.folder.clone();
        s.output.use_folder = self.output_mode == 1 && self.folder.is_some();
        s.output.open_folder_when_done = self.open_folder_when_done;
        s.output.keep_file_dates = self.keep_file_dates;
        s.office.engine = at(&ENGINES, self.engine);
        s.performance.image_jobs = usize::try_from(self.image_jobs.max(0))
            .unwrap_or(0)
            .min(MAX_IMAGE_JOBS);
        s.performance.use_gpu = self.use_gpu;
        s.notifications.toast = self.toast;
        s.appearance.theme = at(&THEMES, self.theme);
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use morph_core::settings::OutputSettings;

    /// `config.toml` as `Settings::save` writes it.
    fn toml_text(s: &Settings) -> String {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        s.save(&path).unwrap();
        std::fs::read_to_string(path).unwrap()
    }

    #[test]
    fn untouched_form_changes_nothing() {
        let s = Settings::default();
        assert_eq!(Form::from_settings(&s).apply(&s), s);
        let f = Form::from_settings(&s);
        assert_eq!(
            (f.jpeg_quality, f.webp_quality, f.avif_quality, f.png_level),
            (85, 80, 60, 2)
        );
        assert_eq!((f.pdf_preset, f.video_preset, f.video_encoder), (1, 0, 0));
        assert_eq!(
            (
                MP3_BITRATES[f.mp3_bitrate as usize],
                PDF_DPIS[f.pdf_dpi as usize]
            ),
            (192, 150)
        );
    }

    #[test]
    fn every_field_round_trips_through_the_form() {
        let mut s = Settings::default();
        s.menu.video = false;
        s.menu.office = false;
        s.defaults.jpeg_quality = 90;
        s.defaults.webp_quality = 70;
        s.defaults.avif_quality = 50;
        s.defaults.png_level = 6;
        s.defaults.pdf_preset = CompressPreset::High;
        s.defaults.video_preset = VideoCompressPreset::Discord;
        s.defaults.video_encoder = EncoderChoice::Cpu;
        s.defaults.mp3_bitrate_kbps = 320;
        s.defaults.pdf_dpi = 300;
        s.output = OutputSettings {
            use_folder: true,
            folder: Some(PathBuf::from(r"D:\Saída 夏")),
            open_folder_when_done: true,
            keep_file_dates: true,
        };
        s.office.engine = EnginePreference::LibreOffice;
        s.performance.image_jobs = 3;
        s.performance.use_gpu = false;
        s.notifications.toast = false;
        s.appearance.theme = Theme::Dark;
        let form = Form::from_settings(&s);
        assert_eq!(form.apply(&Settings::default()), s);
        // Through config.toml too.
        assert_eq!(
            Form::from_settings(&Settings::from_toml(&toml_text(&s))),
            form
        );
    }

    #[test]
    fn toggles_write_the_menu_keys_the_shell_dll_reads() {
        let s = Settings::default();
        let mut form = Form::from_settings(&s);
        form.video = false;
        form.audio = false;
        let saved = form.apply(&s);
        let text = toml_text(&saved);
        let dll = morph_formats::EnabledCategories::parse_menu_section(&text);
        assert!(dll.images && dll.pdf && dll.office);
        assert!(!dll.video && !dll.audio);
        assert!(text.contains("[menu]") && text.contains("video = false"));
    }

    #[test]
    fn more_jobs_than_cores_survive_a_save() {
        let mut s = Settings::default();
        s.performance.image_jobs = 48;
        let shown_max = 8;
        let mut form = Form::from_settings(&s);
        form.image_jobs = form.image_jobs.min(shown_max); // what the list shows
        form.theme = 2;
        form.keep_unshown_jobs(&s, shown_max);
        assert_eq!(form.apply(&s).performance.image_jobs, 48);
        // Picked another entry: that one is saved.
        form.image_jobs = 4;
        form.keep_unshown_jobs(&s, shown_max);
        assert_eq!(form.apply(&s).performance.image_jobs, 4);
    }

    #[test]
    fn output_change_is_seen_as_an_output_change() {
        // `App::settings_changed` forgets the remembered "Save to" when
        // `OutputSettings::same_save_to` says no; only Output controls may cause that.
        let s = Settings::default();
        let mut form = Form::from_settings(&s);
        form.theme = 2;
        form.jpeg_quality = 70;
        assert_eq!(form.apply(&s).output, s.output);
        form.output_mode = 1;
        form.folder = Some(PathBuf::from(r"C:\Out"));
        let changed = form.apply(&s);
        assert_ne!(changed.output, s.output);
        assert_eq!(
            changed.output.fixed_folder(),
            Some(std::path::Path::new(r"C:\Out"))
        );
        // Fixed folder without a folder stays "next to the originals".
        let mut no_folder = Form::from_settings(&s);
        no_folder.output_mode = 1;
        assert_eq!(no_folder.apply(&s).output, s.output);
    }

    #[test]
    fn encoder_list_is_auto_and_cpu_only() {
        assert_eq!(VIDEO_ENCODERS, [EncoderChoice::Auto, EncoderChoice::Cpu]);
        assert_eq!(
            VIDEO_ENCODERS.len(),
            crate::strings::DEFAULT_VIDEO_ENCODERS.len()
        );
        assert_eq!(
            VIDEO_PRESETS.len(),
            crate::strings::DEFAULT_VIDEO_PRESETS.len()
        );
        assert_eq!(PDF_PRESETS.len(), crate::strings::DEFAULT_PDF_PRESETS.len());
        assert_eq!(
            MP3_BITRATES.len(),
            crate::strings::DEFAULT_MP3_BITRATES.len()
        );
        assert_eq!(PDF_DPIS.len(), crate::strings::DEFAULT_PDF_DPIS.len());
        assert_eq!(ENGINES.len(), crate::strings::ENGINE_CHOICES.len());
        assert_eq!(THEMES.len(), crate::strings::THEME_NAMES.len());
    }

    #[test]
    fn values_the_page_cannot_show_are_kept() {
        let mut s = Settings::default();
        s.defaults.pdf_preset = CompressPreset::Custom;
        s.defaults.pdf_dpi = 200;
        let mut form = Form::from_settings(&s);
        form.toast = false;
        let saved = form.apply(&s);
        assert_eq!(saved.defaults.pdf_preset, CompressPreset::Custom);
        assert_eq!(saved.defaults.pdf_dpi, 200);
        form.pdf_dpi = 2;
        assert_eq!(form.apply(&s).defaults.pdf_dpi, 300);
    }
}
