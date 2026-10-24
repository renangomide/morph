// SPDX-License-Identifier: GPL-3.0-or-later
//! `config.toml`: the Settings page (SPEC §3.4, §5.5).
//!
//! ```toml
//! [menu]            # Settings → File types; the shell DLL reads only this section
//! images = true     # (morph_formats::config, plain `key = bool` lines, cached)
//! [defaults]        # Settings → Defaults
//! jpeg_quality = 85
//! [output]          # next to originals / fixed folder, open folder, keep dates
//! [office]          # engine = "auto" | "microsoft_office" | "libreoffice"
//! [performance]     # image_jobs (0 = cores − 1), use_gpu
//! [notifications]   # toast
//! [appearance]      # theme = "system" | "light" | "dark"
//! ```
//!
//! **Settings → Defaults vs. "Remember these settings"**: Settings → Defaults are
//! what an action starts with when nothing is remembered for it (or Remember is
//! off). Remembered options (`state.toml`) win when present; the menu preset
//! ("Convert to PNG…") wins over both. [`crate::state::State::action_for`] does
//! the layering once [`crate::state::State::set_defaults`] got these settings.
//!
//! A few defaults depend on the file and can't be a plain option value: image
//! quality per format (the options say `quality: None` = "the format's
//! default") and "use GPU". [`Settings::apply_to_job`] fills those in right
//! before a job runs, together with "Keep original file dates".
//!
//! A missing file means defaults; a bad value only resets that value. A file
//! that isn't TOML at all keeps its `[menu]` (read like the shell DLL does, so
//! app and menu agree) and is renamed to `config.toml.bad` on the next save.
//! Writes are atomic (unique temp file + rename).

use crate::action::Action;
use crate::image::options::{CompressMode, OutputFormat};
use crate::job::Job;
use crate::media::options::{AudioFormat, EncoderChoice, VideoCompressPreset};
use crate::office::EnginePreference;
use crate::pdf::options::CompressPreset;
use crate::scheduler::JobConfig;
use morph_formats::EnabledCategories;
use morph_formats::actions;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::io;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// File types: a type turned off disappears from the right-click menu and
    /// its files are skipped in a mixed selection / drop.
    pub menu: EnabledCategories,
    pub defaults: Defaults,
    pub output: OutputSettings,
    pub office: OfficeSettings,
    pub performance: Performance,
    pub notifications: Notifications,
    pub appearance: Appearance,
}

/// Settings → Defaults (SPEC §3.4). Every value here equals the matching
/// options struct's `Default`, so untouched settings change nothing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Defaults {
    pub jpeg_quality: u8,
    pub webp_quality: u8,
    pub avif_quality: u8,
    /// oxipng level 0–6 (Compress).
    pub png_level: u8,
    pub pdf_preset: CompressPreset,
    pub video_preset: VideoCompressPreset,
    /// Video encoder: Auto (GPU if one works, else CPU) or CPU.
    pub video_encoder: EncoderChoice,
    /// MP3 bitrate for Audio → Convert and Extract audio.
    pub mp3_bitrate_kbps: u32,
    /// PDF → images DPI.
    pub pdf_dpi: u32,
}

impl Default for Defaults {
    fn default() -> Self {
        Defaults {
            jpeg_quality: OutputFormat::Jpg.default_quality(),
            webp_quality: OutputFormat::Webp.default_quality(),
            avif_quality: OutputFormat::Avif.default_quality(),
            png_level: 2,
            pdf_preset: CompressPreset::Medium,
            video_preset: VideoCompressPreset::Best,
            video_encoder: EncoderChoice::Auto,
            mp3_bitrate_kbps: AudioFormat::Mp3.default_bitrate(),
            pdf_dpi: 150,
        }
    }
}

/// Settings → Output.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct OutputSettings {
    /// `true`: outputs go to `folder` by default ("fixed folder"); `false`: next
    /// to the originals. The window's "Save to" starts from this unless a
    /// remembered choice exists (`state.toml`, Remember on).
    pub use_folder: bool,
    pub folder: Option<PathBuf>,
    /// Open the output folder in Explorer when a job ends (off).
    pub open_folder_when_done: bool,
    /// Give outputs the input's modified/created dates (off).
    pub keep_file_dates: bool,
}

impl OutputSettings {
    /// The fixed folder, if one is chosen and in use.
    pub fn fixed_folder(&self) -> Option<&Path> {
        self.folder.as_deref().filter(|_| self.use_folder)
    }

    /// Same "Save to" default (mode + folder); the other switches don't count.
    pub fn same_save_to(&self, other: &OutputSettings) -> bool {
        (self.use_folder, &self.folder) == (other.use_folder, &other.folder)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct OfficeSettings {
    pub engine: EnginePreference,
}

/// Settings → Performance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Performance {
    /// Parallel CPU (image/PDF) tasks; 0 = automatic (cores − 1).
    pub image_jobs: usize,
    /// Use the GPU encoder for video when one works. Off = always CPU.
    pub use_gpu: bool,
}

impl Default for Performance {
    fn default() -> Self {
        Performance {
            image_jobs: 0,
            use_gpu: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Notifications {
    /// Toast when a job ends (window hidden or not focused).
    pub toast: bool,
}

impl Default for Notifications {
    fn default() -> Self {
        Notifications { toast: true }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Appearance {
    pub theme: Theme,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Theme {
    #[default]
    System,
    Light,
    Dark,
}

/// Most parallel CPU tasks the Settings page offers.
pub const MAX_IMAGE_JOBS: usize = 64;

impl Settings {
    /// Read `config.toml`. Missing or unreadable → defaults; a value that
    /// doesn't parse → that value's default (its siblings are kept).
    pub fn load(path: &Path) -> Settings {
        std::fs::read_to_string(path)
            .map(|text| Settings::from_toml(&text))
            .unwrap_or_default()
    }

    pub fn from_toml(text: &str) -> Settings {
        let Ok(table) = text.parse::<toml::Table>() else {
            // Not TOML: the shell DLL's line reader still sees `[menu]`, so
            // the app uses the same file types.
            return Settings {
                menu: EnabledCategories::parse_menu_section(text),
                ..Settings::default()
            };
        };
        fn parse<T: DeserializeOwned>(table: &toml::Table) -> Option<T> {
            toml::Value::Table(table.clone()).try_into().ok()
        }
        /// The section, keeping every valid value: a bad one gets its default.
        fn section<T: DeserializeOwned + Serialize + Default>(table: &toml::Table, key: &str) -> T {
            let Some(toml::Value::Table(stored)) = table.get(key) else {
                return T::default();
            };
            if let Some(value) = parse(stored) {
                return value;
            }
            let mut merged = match toml::Value::try_from(T::default()) {
                Ok(toml::Value::Table(t)) => t,
                _ => toml::Table::new(),
            };
            for (k, v) in stored {
                let mut trial = merged.clone();
                trial.insert(k.clone(), v.clone());
                if parse::<T>(&trial).is_some() {
                    merged = trial;
                }
            }
            parse(&merged).unwrap_or_default()
        }
        Settings {
            menu: section(&table, "menu"),
            defaults: section(&table, "defaults"),
            output: section(&table, "output"),
            office: section(&table, "office"),
            performance: section(&table, "performance"),
            notifications: section(&table, "notifications"),
            appearance: section(&table, "appearance"),
        }
    }

    /// Write `config.toml` atomically (unique temp file + rename): the shell
    /// DLL may read it at any moment and must never see half a file.
    /// A file that isn't TOML (edited by hand) is first renamed to
    /// `config.toml.bad` (replacing an older one), so its text isn't lost.
    pub fn save(&self, path: &Path) -> io::Result<()> {
        let text = toml::to_string(self).map_err(io::Error::other)?;
        if let Ok(old) = std::fs::read(path) {
            let is_toml = std::str::from_utf8(&old).is_ok_and(|t| t.parse::<toml::Table>().is_ok());
            if !is_toml {
                let mut bad = path.as_os_str().to_owned();
                bad.push(".bad");
                std::fs::rename(path, bad)?;
            }
        }
        crate::state::write_atomic(path, text.as_bytes())
    }

    /// Scheduler settings: parallel CPU tasks and the Office engine.
    pub fn job_config(&self) -> JobConfig {
        let mut config = JobConfig {
            office_engine: self.office.engine,
            ..JobConfig::default()
        };
        if self.performance.image_jobs > 0 {
            config.cpu_threads = self.performance.image_jobs.min(MAX_IMAGE_JOBS);
        }
        config
    }

    /// Quality a lossy image format starts with (Settings → Defaults).
    pub fn image_quality(&self, format: OutputFormat) -> u8 {
        let d = &self.defaults;
        match format {
            OutputFormat::Jpg => d.jpeg_quality,
            OutputFormat::Webp => d.webp_quality,
            OutputFormat::Avif => d.avif_quality,
            other => other.default_quality(),
        }
        .clamp(1, 100)
    }

    /// Option values Settings → Defaults set for action `id`, as a partial
    /// options object (only keys that differ from the built-in defaults).
    /// `preset` is the menu preset: the MP3 bitrate is left out when the
    /// preset picks another audio format.
    pub fn option_defaults(&self, id: &str, preset: &Map<String, Value>) -> Map<String, Value> {
        let builtin = Defaults::default();
        let d = &self.defaults;
        let mut map = Map::new();
        let mut set = |key: &str, value: Value| {
            map.insert(key.to_owned(), value);
        };
        match id {
            actions::IMAGE_COMPRESS if d.png_level != builtin.png_level => {
                set("png_level", Value::from(d.png_level.min(6)));
            }
            actions::PDF_COMPRESS if d.pdf_preset != builtin.pdf_preset => {
                set("preset", json(&d.pdf_preset));
            }
            actions::PDF_TO_IMAGES if d.pdf_dpi != builtin.pdf_dpi => {
                set("dpi", Value::from(d.pdf_dpi.clamp(36, 1200)));
            }
            actions::VIDEO_COMPRESS => {
                if d.video_preset != builtin.video_preset {
                    set("preset", json(&d.video_preset));
                }
                if d.video_encoder != builtin.video_encoder {
                    set("encoder", json(&d.video_encoder));
                }
            }
            actions::VIDEO_CONVERT if d.video_encoder != builtin.video_encoder => {
                set("encoder", json(&d.video_encoder));
            }
            actions::AUDIO_CONVERT | actions::VIDEO_EXTRACT_AUDIO
                if d.mp3_bitrate_kbps != builtin.mp3_bitrate_kbps =>
            {
                let mp3 = preset
                    .get("format")
                    .is_none_or(|f| f.as_str() == Some("mp3"));
                if mp3 {
                    set("bitrate_kbps", Value::from(d.mp3_bitrate_kbps));
                }
            }
            _ => {}
        }
        map
    }

    /// Fill in what depends on the file or on a global switch: image quality
    /// left at "format default" gets Settings → Defaults' quality; "use GPU"
    /// off forces the CPU video encoder.
    pub fn resolve(&self, action: &mut Action) {
        match action {
            Action::ImageConvert(o) if o.quality.is_none() && o.format.is_lossy() => {
                o.quality = Some(self.image_quality(o.format));
            }
            Action::ImageCompress(o) => {
                // Quality mode without a value: each image keeps its format, so
                // only a WebP conversion has one known format to resolve.
                if o.convert_to_webp
                    && let CompressMode::Quality { quality: q @ None } = &mut o.mode
                {
                    *q = Some(self.image_quality(OutputFormat::Webp));
                }
            }
            Action::VideoConvert(o) if !self.performance.use_gpu => o.encoder = EncoderChoice::Cpu,
            Action::VideoCompress(o) if !self.performance.use_gpu => {
                o.encoder = EncoderChoice::Cpu;
            }
            _ => {}
        }
    }

    /// Apply [`Settings::resolve`] to every task, and "Keep original file dates".
    pub fn apply_to_job(&self, job: &mut Job) {
        for task in &mut job.tasks {
            self.resolve(&mut task.action);
        }
        job.keep_dates = self.output.keep_file_dates;
    }
}

/// A small enum's JSON value (its serde name).
fn json<T: Serialize>(value: &T) -> Value {
    serde_json::to_value(value).unwrap_or(Value::Null)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::State;

    #[test]
    fn defaults_match_the_spec() {
        let s = Settings::default();
        let d = &s.defaults;
        assert_eq!(
            (d.jpeg_quality, d.webp_quality, d.avif_quality, d.png_level),
            (85, 80, 60, 2)
        );
        assert_eq!(d.pdf_preset, CompressPreset::Medium);
        assert_eq!(d.video_preset, VideoCompressPreset::Best);
        assert_eq!(d.video_encoder, EncoderChoice::Auto);
        assert_eq!((d.mp3_bitrate_kbps, d.pdf_dpi), (192, 150));
        assert_eq!(s.menu, EnabledCategories::default());
        assert!(s.output.fixed_folder().is_none());
        assert!(!s.output.open_folder_when_done && !s.output.keep_file_dates);
        assert_eq!(s.office.engine, EnginePreference::Auto);
        assert_eq!(s.performance, Performance::default());
        assert!(s.performance.use_gpu && s.notifications.toast);
        assert_eq!(s.appearance.theme, Theme::System);
        // Untouched settings change no action's defaults.
        for id in [
            actions::IMAGE_COMPRESS,
            actions::PDF_COMPRESS,
            actions::PDF_TO_IMAGES,
            actions::VIDEO_COMPRESS,
            actions::VIDEO_CONVERT,
            actions::AUDIO_CONVERT,
        ] {
            assert!(s.option_defaults(id, &Map::new()).is_empty(), "{id}");
        }
    }

    #[test]
    fn menu_section_is_readable_by_the_shell_dll() {
        let mut s = Settings::default();
        s.menu.video = false;
        s.menu.office = false;
        let text = toml::to_string(&s).unwrap();
        let parsed = EnabledCategories::parse_menu_section(&text);
        assert_eq!(parsed, s.menu);
        assert_eq!(
            morph_formats::config::cached_menu_config(Path::new(r"C:\nope\config.toml")),
            EnabledCategories::default()
        );
    }

    #[test]
    fn round_trip_and_atomic_save() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub").join("config.toml");
        let mut s = Settings::default();
        s.menu.images = false;
        s.defaults.webp_quality = 70;
        s.defaults.video_preset = VideoCompressPreset::Discord;
        s.output = OutputSettings {
            use_folder: true,
            folder: Some(PathBuf::from(r"D:\Saída 夏")),
            open_folder_when_done: true,
            keep_file_dates: true,
        };
        s.office.engine = EnginePreference::LibreOffice;
        s.performance = Performance {
            image_jobs: 3,
            use_gpu: false,
        };
        s.notifications.toast = false;
        s.appearance.theme = Theme::Dark;
        s.save(&path).unwrap();
        assert_eq!(Settings::load(&path), s);
        // The engine reader the core already uses sees the same value.
        assert_eq!(
            EnginePreference::from_config_file(&path),
            EnginePreference::LibreOffice
        );
        assert_eq!(
            std::fs::read_dir(path.parent().unwrap()).unwrap().count(),
            1
        );
        assert_eq!(s.job_config().cpu_threads, 3);
    }

    #[test]
    fn damaged_sections_reset_alone() {
        let s = Settings::from_toml(
            "[menu]\nvideo = false\n[defaults]\npdf_preset = \"nope\"\n[office]\nengine = \"libreoffice\"\n",
        );
        assert!(!s.menu.video);
        assert_eq!(s.defaults, Defaults::default());
        assert_eq!(s.office.engine, EnginePreference::LibreOffice);
        assert_eq!(Settings::from_toml("= = ="), Settings::default());
    }

    #[test]
    fn a_bad_value_keeps_its_siblings() {
        let s = Settings::from_toml(
            "[defaults]\njpeg_quality = 300\nwebp_quality = 70\npdf_preset = \"nope\"\npdf_dpi = 300\n\
             [performance]\nimage_jobs = 48\nuse_gpu = \"yes\"\n",
        );
        let d = &s.defaults;
        assert_eq!(d.jpeg_quality, Defaults::default().jpeg_quality);
        assert_eq!(d.pdf_preset, Defaults::default().pdf_preset);
        assert_eq!((d.webp_quality, d.pdf_dpi), (70, 300));
        assert_eq!(s.performance.image_jobs, 48);
        assert!(s.performance.use_gpu);
    }

    #[test]
    fn not_toml_keeps_the_menu_and_is_kept_as_bad() {
        let text = "[menu]\nvideo = false\n[defaults]\njpeg_quality = = 1\n";
        let s = Settings::from_toml(text);
        assert_eq!(s.menu, EnabledCategories::parse_menu_section(text));
        assert!(!s.menu.video);
        assert_eq!(s.defaults, Defaults::default());

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let bad = dir.path().join("config.toml.bad");
        std::fs::write(&bad, "older").unwrap();
        std::fs::write(&path, text).unwrap();
        let loaded = Settings::load(&path);
        loaded.save(&path).unwrap();
        assert_eq!(std::fs::read_to_string(&bad).unwrap(), text);
        assert_eq!(Settings::load(&path), loaded);
        // A valid file is just replaced.
        loaded.save(&path).unwrap();
        assert_eq!(std::fs::read_to_string(&bad).unwrap(), text);
    }

    #[test]
    fn save_to_ignores_the_other_output_switches() {
        let a = OutputSettings::default();
        let b = OutputSettings {
            open_folder_when_done: true,
            keep_file_dates: true,
            ..a.clone()
        };
        assert!(a.same_save_to(&b));
        let c = OutputSettings {
            folder: Some(PathBuf::from(r"C:\Out")),
            ..a.clone()
        };
        assert!(!a.same_save_to(&c));
        let d = OutputSettings {
            use_folder: true,
            ..c.clone()
        };
        assert!(!c.same_save_to(&d));
    }

    #[test]
    fn settings_defaults_then_remembered_then_preset() {
        use crate::media::options::VideoCompressOpts;
        let mut settings = Settings::default();
        settings.defaults.video_preset = VideoCompressPreset::Smaller;
        settings.defaults.mp3_bitrate_kbps = 320;
        let mut state = State::default();
        state.set_defaults(&settings);

        // Nothing remembered → Settings → Defaults.
        let Some(Action::VideoCompress(o)) = state.action_for(actions::VIDEO_COMPRESS, &Map::new())
        else {
            panic!("wrong action")
        };
        assert_eq!(o.preset, VideoCompressPreset::Smaller);

        // Remembered wins.
        state.remember_action(&Action::VideoCompress(VideoCompressOpts {
            preset: VideoCompressPreset::Email,
            ..Default::default()
        }));
        let Some(Action::VideoCompress(o)) = state.action_for(actions::VIDEO_COMPRESS, &Map::new())
        else {
            panic!("wrong action")
        };
        assert_eq!(o.preset, VideoCompressPreset::Email);

        // Remember off → Settings → Defaults again.
        state.remember = false;
        let Some(Action::VideoCompress(o)) = state.action_for(actions::VIDEO_COMPRESS, &Map::new())
        else {
            panic!("wrong action")
        };
        assert_eq!(o.preset, VideoCompressPreset::Smaller);

        // MP3 bitrate only for MP3.
        let mut preset = Map::new();
        let Some(Action::AudioConvert(o)) = state.action_for(actions::AUDIO_CONVERT, &preset)
        else {
            panic!("wrong action")
        };
        assert_eq!(o.bitrate_kbps, 320);
        preset.insert("format".into(), Value::from("m4a"));
        let Some(Action::AudioConvert(o)) = state.action_for(actions::AUDIO_CONVERT, &preset)
        else {
            panic!("wrong action")
        };
        assert_ne!(o.bitrate_kbps, 320);
    }

    #[test]
    fn resolve_fills_quality_and_gpu() {
        use crate::image::options::ImageConvertOpts;
        use crate::media::options::VideoConvertOpts;
        let mut settings = Settings::default();
        settings.defaults.webp_quality = 70;
        settings.performance.use_gpu = false;
        let mut a = Action::ImageConvert(ImageConvertOpts {
            format: OutputFormat::Webp,
            ..Default::default()
        });
        settings.resolve(&mut a);
        assert!(matches!(a, Action::ImageConvert(ref o) if o.quality == Some(70)));
        let mut chosen = Action::ImageConvert(ImageConvertOpts {
            format: OutputFormat::Webp,
            quality: Some(40),
            ..Default::default()
        });
        settings.resolve(&mut chosen);
        assert!(matches!(chosen, Action::ImageConvert(ref o) if o.quality == Some(40)));
        let mut v = Action::VideoConvert(VideoConvertOpts::default());
        settings.resolve(&mut v);
        assert!(matches!(v, Action::VideoConvert(ref o) if o.encoder == EncoderChoice::Cpu));
    }
}
