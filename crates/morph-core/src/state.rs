// SPDX-License-Identifier: GPL-3.0-or-later
//! `state.toml`: what the conversion window remembers between runs (SPEC §3.2, §5.5):
//! last-used options per action, the "Save to" choice, and the
//! "Remember these settings" checkbox itself.
//!
//! A missing or damaged file means "nothing remembered"; it never stops the app.

use crate::action::Action;
use crate::media::gpu::GpuCache;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct State {
    /// "Remember these settings" (on by default, SPEC §14).
    pub remember: bool,
    /// Where outputs go.
    pub output: OutputChoice,
    /// Last-used options, by action ID (`"image.convert"` → `{format = "webp", …}`).
    pub actions: BTreeMap<String, Map<String, Value>>,
    /// GPU video encoder detection result (`media::gpu`). Written by the core
    /// during a job with [`State::save_gpu`]; [`State::save`] keeps the one on disk.
    pub gpu: Option<GpuCache>,
    /// Settings → Defaults, the base layer of [`State::action_for`]. Not
    /// stored here: set from `config.toml` with [`State::set_defaults`].
    #[serde(skip)]
    settings: Option<crate::settings::Settings>,
}

impl Default for State {
    fn default() -> Self {
        State {
            remember: true,
            output: OutputChoice::default(),
            actions: BTreeMap::new(),
            gpu: None,
            settings: None,
        }
    }
}

/// "Save to" in the conversion window.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct OutputChoice {
    /// `false`: next to the original files.
    pub use_folder: bool,
    /// Last chosen folder (kept even while `use_folder` is off, to prefill the box).
    pub folder: Option<PathBuf>,
}

impl OutputChoice {
    /// The folder outputs go to, if one is chosen and in use.
    pub fn folder(&self) -> Option<&Path> {
        self.folder.as_deref().filter(|_| self.use_folder)
    }
}

impl State {
    /// Read `state.toml`. Missing or unreadable → defaults.
    pub fn load(path: &Path) -> State {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|text| toml::from_str(&text).ok())
            .unwrap_or_default()
    }

    /// Write `state.toml` atomically (temp file + rename). `gpu` is not written
    /// from here: the one on disk is kept (a window that loaded state.toml
    /// before a job detected the GPU must not drop or roll back the result).
    pub fn save(&self, path: &Path) -> io::Result<()> {
        let mut clean = self.clone();
        clean.gpu = State::load(path).gpu;
        clean.write(path)
    }

    /// Store a GPU detection result, keeping everything else on disk.
    pub fn save_gpu(path: &Path, gpu: GpuCache) -> io::Result<()> {
        let mut state = State::load(path);
        state.gpu = Some(gpu);
        state.write(path)
    }

    fn write(mut self, path: &Path) -> io::Result<()> {
        for options in self.actions.values_mut() {
            strip_nulls(options);
        }
        let text = toml::to_string(&self).map_err(io::Error::other)?;
        write_atomic(path, text.as_bytes())
    }

    /// Use Settings → Defaults as the base layer of [`State::action_for`]
    /// (call after loading both files, and again when Settings change).
    pub fn set_defaults(&mut self, settings: &crate::settings::Settings) {
        self.settings = Some(settings.clone());
    }

    /// Remember `action`'s options (call after Convert when `remember` is on).
    pub fn remember_action(&mut self, action: &Action) {
        self.actions
            .insert(action.id().to_owned(), action.options_json());
    }

    /// The action to pre-fill the window with: built-in defaults, then
    /// Settings → Defaults ([`State::set_defaults`]), then the remembered
    /// options (if `remember` is on), then the menu preset on top
    /// (e.g. "Convert to PNG…" always means PNG).
    ///
    /// Never fails for a known ID: remembered options that no longer fit (an
    /// older version, a hand-edited file) are dropped key by key.
    pub fn action_for(&self, id: &str, preset: &Map<String, Value>) -> Option<Action> {
        let mut merged = self
            .settings
            .as_ref()
            .map(|s| s.option_defaults(id, preset))
            .unwrap_or_default();
        if self.remember
            && let Some(saved) = self.actions.get(id)
        {
            for (k, v) in saved {
                let mut one = Map::new();
                one.insert(k.clone(), v.clone());
                if Action::from_id(id, &one).is_ok() {
                    merged.insert(k.clone(), v.clone());
                }
            }
        }
        if let Ok(action) = Action::from_id(id, &merge(merged.clone(), preset)) {
            return Some(action);
        }
        if let Ok(action) = Action::from_id(id, preset) {
            return Some(action);
        }
        Action::default_for(id)
    }
}

fn merge(mut base: Map<String, Value>, top: &Map<String, Value>) -> Map<String, Value> {
    for (k, v) in top {
        base.insert(k.clone(), v.clone());
    }
    base
}

/// Write `bytes` to `path` so a reader never sees half a file: a temp file
/// with a unique name next to it (two writers never share one), then rename.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(format!(".{}.tmp", uuid::Uuid::new_v4().simple()));
    let tmp = PathBuf::from(tmp);
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

/// TOML has no null: drop null values (they come back as defaults).
fn strip_nulls(map: &mut Map<String, Value>) {
    map.retain(|_, v| !v.is_null());
    for v in map.values_mut() {
        if let Value::Object(inner) = v {
            strip_nulls(inner);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::image::options::{ImageConvertOpts, OutputFormat};
    use morph_formats::actions;

    fn preset(k: &str, v: &str) -> Map<String, Value> {
        let mut m = Map::new();
        m.insert(k.into(), Value::from(v));
        m
    }

    #[test]
    fn missing_or_damaged_file_gives_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.toml");
        assert_eq!(State::load(&path), State::default());
        std::fs::write(&path, "this is = = not toml").unwrap();
        assert_eq!(State::load(&path), State::default());
        assert!(State::default().remember);
    }

    #[test]
    fn round_trips_every_action() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub").join("state.toml");
        let mut state = State::default();
        for id in [
            actions::IMAGE_CONVERT,
            actions::IMAGE_COMPRESS,
            actions::IMAGE_RESIZE,
            actions::IMAGE_CLEAN,
        ] {
            state.remember_action(&Action::default_for(id).unwrap());
        }
        state.output = OutputChoice {
            use_folder: true,
            folder: Some(PathBuf::from(r"C:\Users\x\Pictures\Férias")),
        };
        state.save(&path).unwrap();
        let back = State::load(&path);
        assert_eq!(back.output, state.output);
        for id in state.actions.keys() {
            assert_eq!(
                back.action_for(id, &Map::new()),
                Action::default_for(id),
                "{id}"
            );
        }
    }

    #[test]
    fn window_save_keeps_the_gpu_on_disk() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.toml");
        let gpu = |v: &str| GpuCache {
            ffmpeg_version: v.into(),
            ..Default::default()
        };
        State::save_gpu(&path, gpu("old")).unwrap();
        let window = State::load(&path); // holds "old"
        State::save_gpu(&path, gpu("new")).unwrap();
        let mut window = window;
        window.remember = false;
        window.save(&path).unwrap();
        let back = State::load(&path);
        assert_eq!(back.gpu, Some(gpu("new")));
        assert!(!back.remember);
        // No temp files left behind.
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn remembered_then_preset_on_top() {
        let mut state = State::default();
        let mut opts = preset("format", "avif");
        opts.insert("quality".into(), Value::from(42));
        state.actions.insert(actions::IMAGE_CONVERT.into(), opts);

        let Some(Action::ImageConvert(o)) = state.action_for(actions::IMAGE_CONVERT, &Map::new())
        else {
            panic!("wrong action")
        };
        assert_eq!((o.format, o.quality), (OutputFormat::Avif, Some(42)));

        let Some(Action::ImageConvert(o)) =
            state.action_for(actions::IMAGE_CONVERT, &preset("format", "png"))
        else {
            panic!("wrong action")
        };
        assert_eq!(
            (o.format, o.quality),
            (OutputFormat::Png, Some(42)),
            "menu wins"
        );

        state.remember = false;
        let Some(Action::ImageConvert(o)) = state.action_for(actions::IMAGE_CONVERT, &Map::new())
        else {
            panic!("wrong action")
        };
        assert_eq!(
            o.format,
            ImageConvertOpts::default().format,
            "remember off → defaults"
        );
    }

    #[test]
    fn compress_mode_from_older_state_files() {
        use crate::image::options::CompressMode;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.toml");
        let mode_of = |text: &str| {
            std::fs::write(&path, text).unwrap();
            match State::load(&path).action_for(actions::IMAGE_COMPRESS, &Map::new()) {
                Some(Action::ImageCompress(o)) => o.mode,
                other => panic!("wrong action {other:?}"),
            }
        };
        // A remembered choice from before Best existed is respected.
        assert_eq!(
            mode_of("[actions.\"image.compress\".mode]\ntype = \"quality\"\n"),
            CompressMode::Quality { quality: None }
        );
        assert_eq!(
            mode_of("[actions.\"image.compress\".mode]\ntype = \"quality\"\nquality = 60\n"),
            CompressMode::Quality { quality: Some(60) }
        );
        // Nothing remembered, or a mode this version doesn't know → Best.
        assert_eq!(mode_of(""), CompressMode::Best);
        assert_eq!(
            mode_of("[actions.\"image.compress\".mode]\ntype = \"future\"\n"),
            CompressMode::Best
        );
        assert_eq!(
            mode_of("[actions.\"image.compress\".mode]\ntype = \"best\"\n"),
            CompressMode::Best
        );
    }

    #[test]
    fn stale_remembered_keys_are_dropped() {
        let mut state = State::default();
        let mut opts = preset("format", "jpg");
        opts.insert("old_option".into(), Value::from(true));
        opts.insert("quality".into(), Value::from("not a number"));
        state.actions.insert(actions::IMAGE_CONVERT.into(), opts);
        let Some(Action::ImageConvert(o)) = state.action_for(actions::IMAGE_CONVERT, &Map::new())
        else {
            panic!("wrong action")
        };
        assert_eq!(o.format, OutputFormat::Jpg);
        assert!(state.action_for("nope", &Map::new()).is_none());
    }
}
