// SPDX-License-Identifier: GPL-3.0-or-later
//! Reader for the `[menu]` section of `config.toml`.
//!
//! The shell DLL only needs to know which categories are enabled, so instead of
//! pulling in a full TOML parser we read just `key = true|false` lines inside
//! `[menu]`. The app writes this file with a real TOML serializer.

use crate::category::Category;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Mutex;
use std::time::SystemTime;

/// Which file types appear in the right-click menu (Settings → File types).
/// Serialized as the `[menu]` section of `config.toml` (plain `key = bool` lines,
/// which [`EnabledCategories::parse_menu_section`] reads without a TOML parser).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct EnabledCategories {
    pub images: bool,
    pub pdf: bool,
    pub video: bool,
    pub audio: bool,
    pub office: bool,
}

impl Default for EnabledCategories {
    fn default() -> Self {
        EnabledCategories {
            images: true,
            pdf: true,
            video: true,
            audio: true,
            office: true,
        }
    }
}

impl EnabledCategories {
    pub fn is_enabled(&self, category: Category) -> bool {
        match category {
            Category::Image => self.images,
            Category::Pdf => self.pdf,
            Category::Video => self.video,
            Category::Audio => self.audio,
            // Text-like files (md/txt/html) belong to "Office" in Settings → File types.
            Category::Word | Category::Excel | Category::PowerPoint | Category::Text => self.office,
        }
    }

    /// Parse the `[menu]` section. Unknown keys and malformed lines are ignored;
    /// missing keys keep their default (enabled).
    pub fn parse_menu_section(text: &str) -> EnabledCategories {
        let mut result = EnabledCategories::default();
        let mut in_menu = false;
        for raw in text.lines() {
            let line = strip_comment(raw).trim();
            if line.is_empty() {
                continue;
            }
            if line.starts_with('[') {
                in_menu = line == "[menu]";
                continue;
            }
            if !in_menu {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let value = match value.trim() {
                "true" => true,
                "false" => false,
                _ => continue,
            };
            match key.trim().trim_matches('"') {
                "images" => result.images = value,
                "pdf" => result.pdf = value,
                "video" => result.video = value,
                "audio" => result.audio = value,
                "office" => result.office = value,
                _ => {}
            }
        }
        result
    }
}

fn strip_comment(line: &str) -> &str {
    match line.find('#') {
        Some(i) => &line[..i],
        None => line,
    }
}

static CACHE: Mutex<Option<(SystemTime, EnabledCategories)>> = Mutex::new(None);

/// A real `config.toml` is a few hundred bytes; anything bigger isn't read
/// (Explorer must never wait on it).
const MAX_CONFIG_BYTES: u64 = 64 * 1024;

/// Read the enabled categories from `config_path`, cached by modified time.
/// Any error (missing file, unreadable, too big, poisoned lock) falls back to defaults.
pub fn cached_menu_config(config_path: &Path) -> EnabledCategories {
    let Ok(meta) = std::fs::metadata(config_path) else {
        return EnabledCategories::default();
    };
    let Ok(modified) = meta.modified() else {
        return EnabledCategories::default();
    };
    if meta.len() > MAX_CONFIG_BYTES {
        return EnabledCategories::default();
    }
    let Ok(mut guard) = CACHE.lock() else {
        return EnabledCategories::default();
    };
    if let Some((cached_time, cached)) = *guard
        && cached_time == modified
    {
        return cached;
    }
    let parsed = std::fs::read_to_string(config_path)
        .map(|text| EnabledCategories::parse_menu_section(&text))
        .unwrap_or_default();
    *guard = Some((modified, parsed));
    parsed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_menu_section_only() {
        let text = r#"
            [general]
            images = false   # not the menu section

            [menu]
            images = false
            video = false # trailing comment
            office = maybe
            "pdf" = true

            [other]
            audio = false
        "#;
        let e = EnabledCategories::parse_menu_section(text);
        assert!(!e.images);
        assert!(!e.video);
        assert!(e.office, "malformed value keeps default");
        assert!(e.pdf);
        assert!(e.audio, "keys outside [menu] are ignored");
    }

    #[test]
    fn empty_config_enables_everything() {
        assert_eq!(
            EnabledCategories::parse_menu_section(""),
            EnabledCategories::default()
        );
    }

    #[test]
    fn oversized_file_gives_defaults_without_reading() {
        let path = std::env::temp_dir().join(format!(
            "morph-config-size-test-{}.toml",
            std::process::id()
        ));
        let mut text = String::from("[menu]\nimages = false\n");
        let small = std::fs::write(&path, &text).map(|()| cached_menu_config(&path));
        text.push_str(&"#".repeat(MAX_CONFIG_BYTES as usize));
        let big = std::fs::write(&path, &text).map(|()| cached_menu_config(&path));
        let _ = std::fs::remove_file(&path);
        assert!(!small.unwrap().images);
        assert_eq!(big.unwrap(), EnabledCategories::default());
    }

    #[test]
    fn missing_file_gives_defaults() {
        let e = cached_menu_config(Path::new(r"C:\definitely\not\here\config.toml"));
        assert_eq!(e, EnabledCategories::default());
    }
}
