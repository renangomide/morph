// SPDX-License-Identifier: GPL-3.0-or-later
//! Argument parsing and the option-override model (`--options`, `--format`, `--set`).

use crate::strings;
use clap::{Args, Parser, Subcommand};
use morph_formats::actions;
use serde_json::{Map, Value};
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(name = "morph-cli", version, about = strings::ABOUT)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    #[command(about = strings::ABOUT_CONVERT)]
    Convert(ConvertArgs),
    #[command(about = strings::ABOUT_ACTIONS)]
    Actions,
    #[command(about = strings::ABOUT_OPTIONS)]
    Options {
        #[arg(help = strings::HELP_ACTION_ID)]
        id: String,
    },
    #[command(about = strings::ABOUT_ENGINES)]
    Engines,
    #[command(about = strings::ABOUT_FFMPEG)]
    Ffmpeg,
}

/// `--engine` values (Settings → Office engine).
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum EngineArg {
    Auto,
    #[value(name = "microsoft_office")]
    MicrosoftOffice,
    #[value(name = "libreoffice")]
    LibreOffice,
}

impl EngineArg {
    pub fn preference(self) -> morph_core::office::EnginePreference {
        use morph_core::office::EnginePreference as P;
        match self {
            EngineArg::Auto => P::Auto,
            EngineArg::MicrosoftOffice => P::MicrosoftOffice,
            EngineArg::LibreOffice => P::LibreOffice,
        }
    }
}

#[derive(Debug, Args)]
pub struct ConvertArgs {
    #[arg(required = true, num_args = 1.., value_name = "FILES", help = strings::HELP_FILES)]
    pub files: Vec<PathBuf>,
    #[arg(long, value_name = "ID", help = strings::HELP_ACTION)]
    pub action: String,
    #[arg(long, value_name = "FMT", help = strings::HELP_FORMAT)]
    pub format: Option<String>,
    #[arg(long, value_name = "N", value_parser = clap::value_parser!(u8).range(1..=100), help = strings::HELP_QUALITY)]
    pub quality: Option<u8>,
    #[arg(long = "set", value_name = "KEY=VALUE", help = strings::HELP_SET)]
    pub set: Vec<String>,
    #[arg(long = "target-size", value_name = "SIZE", value_parser = parse_size,
        conflicts_with_all = ["percent", "width", "height"], help = strings::HELP_TARGET_SIZE)]
    pub target_size: Option<u64>,
    #[arg(long, value_name = "N", value_parser = clap::value_parser!(u32).range(1..),
        conflicts_with_all = ["percent", "target_size"], help = strings::HELP_WIDTH)]
    pub width: Option<u32>,
    #[arg(long, value_name = "N", value_parser = clap::value_parser!(u32).range(1..),
        conflicts_with_all = ["percent", "target_size"], help = strings::HELP_HEIGHT)]
    pub height: Option<u32>,
    #[arg(long, value_name = "P", value_parser = parse_percent,
        conflicts_with_all = ["width", "height", "target_size"], help = strings::HELP_PERCENT)]
    pub percent: Option<f64>,
    #[arg(long, value_name = "JSON", help = strings::HELP_OPTIONS)]
    pub options: Option<String>,
    #[arg(long, value_name = "FOLDER", help = strings::HELP_OUT)]
    pub out: Option<PathBuf>,
    #[arg(long, value_name = "N", help = strings::HELP_THREADS)]
    pub threads: Option<usize>,
    #[arg(long, help = strings::HELP_JSON)]
    pub json: bool,
    #[arg(long, value_enum, value_name = "ENGINE", help = strings::HELP_ENGINE)]
    pub engine: Option<EngineArg>,
}

/// Parse `key=value`. The value is JSON when it parses (numbers, booleans,
/// objects, quoted strings), otherwise the raw text as a string.
pub fn parse_set(arg: &str) -> Result<(String, Value), String> {
    let (key, raw) = arg.split_once('=').ok_or_else(|| strings::bad_set(arg))?;
    let key = key.trim();
    if key.is_empty() {
        return Err(strings::bad_set(arg));
    }
    let value =
        serde_json::from_str::<Value>(raw).unwrap_or_else(|_| Value::String(raw.to_owned()));
    Ok((key.to_owned(), value))
}

/// Parse a size such as `500KB`, `2MB`, `1.5MB`, `300000` or `1 GB` (1024-based) into bytes.
pub fn parse_size(text: &str) -> Result<u64, String> {
    let t = text.trim();
    let split = t
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(t.len());
    let (num, unit) = t.split_at(split);
    let factor: f64 = match unit.trim().to_ascii_lowercase().as_str() {
        "" | "b" => 1.0,
        "k" | "kb" | "kib" => 1024.0,
        "m" | "mb" | "mib" => 1024.0 * 1024.0,
        "g" | "gb" | "gib" => 1024.0 * 1024.0 * 1024.0,
        _ => return Err(strings::bad_size(text)),
    };
    let value: f64 = num.parse().map_err(|_| strings::bad_size(text))?;
    let bytes = (value * factor).round();
    if !bytes.is_finite() || bytes < 1.0 || bytes > u64::MAX as f64 {
        return Err(strings::bad_size(text));
    }
    Ok(bytes as u64)
}

fn parse_percent(text: &str) -> Result<f64, String> {
    match text.trim().trim_end_matches('%').parse::<f64>() {
        Ok(p) if p.is_finite() && p > 0.0 => Ok(p),
        _ => Err(strings::bad_percent(text)),
    }
}

/// Set `value` at the dotted `path` (`mode.type`), creating or replacing objects on the way.
fn set_path(map: &mut Map<String, Value>, path: &[&str], value: Value) {
    match path {
        [] => {}
        [last] => {
            map.insert((*last).to_owned(), value);
        }
        [first, rest @ ..] => {
            let entry = map
                .entry((*first).to_owned())
                .or_insert_with(|| Value::Object(Map::new()));
            if !entry.is_object() {
                *entry = Value::Object(Map::new());
            }
            if let Value::Object(inner) = entry {
                set_path(inner, rest, value);
            }
        }
    }
}

/// `--set sample_rate=48000` parses as a number; where the action's default is
/// a string, use the text instead (nested objects too).
pub fn match_default_types(map: &mut Map<String, Value>, defaults: &Map<String, Value>) {
    for (key, value) in map.iter_mut() {
        match (defaults.get(key), &mut *value) {
            (Some(Value::String(_)), Value::Number(_) | Value::Bool(_)) => {
                *value = Value::String(value.to_string());
            }
            (Some(Value::Object(d)), Value::Object(inner)) => match_default_types(inner, d),
            _ => {}
        }
    }
}

impl ConvertArgs {
    /// Option overrides in order: `--options`, then `--format`/`--quality`, then `--set`.
    pub fn overrides(&self) -> Result<Map<String, Value>, String> {
        let mut map = match &self.options {
            Some(text) => match serde_json::from_str::<Value>(text) {
                Ok(Value::Object(obj)) => obj,
                Ok(_) => return Err(strings::options_not_object()),
                Err(e) => return Err(strings::bad_options_json(&e.to_string())),
            },
            None => Map::new(),
        };
        if let Some(format) = &self.format {
            map.insert("format".to_owned(), Value::String(format.clone()));
        }
        if let Some(quality) = self.quality {
            map.insert("quality".to_owned(), Value::from(quality));
        }
        let shortcut_checks: [(bool, &str, &str); 4] = [
            (
                self.target_size.is_some(),
                "--target-size",
                actions::IMAGE_COMPRESS,
            ),
            (self.width.is_some(), "--width", actions::IMAGE_RESIZE),
            (self.height.is_some(), "--height", actions::IMAGE_RESIZE),
            (self.percent.is_some(), "--percent", actions::IMAGE_RESIZE),
        ];
        for (used, flag, only) in shortcut_checks {
            if used && self.action != only {
                return Err(strings::shortcut_not_applicable(flag, &self.action, only));
            }
        }
        let mode = if let Some(bytes) = self.target_size {
            Some(serde_json::json!({"type": "target_size", "bytes": bytes}))
        } else if let Some(p) = self.percent {
            Some(serde_json::json!({"type": "percent", "percent": p}))
        } else {
            match (self.width, self.height) {
                (Some(w), Some(h)) => {
                    Some(serde_json::json!({"type": "exact", "width": w, "height": h}))
                }
                (Some(w), None) => Some(serde_json::json!({"type": "max_width", "width": w})),
                (None, Some(h)) => Some(serde_json::json!({"type": "max_height", "height": h})),
                (None, None) => None,
            }
        };
        if let Some(mode) = mode {
            map.insert("mode".to_owned(), mode);
        }
        for entry in &self.set {
            let (key, value) = parse_set(entry)?;
            let path: Vec<&str> = key.split('.').map(str::trim).collect();
            if path.iter().any(|part| part.is_empty()) {
                return Err(strings::bad_set(entry));
            }
            set_path(&mut map, &path, value);
        }
        Ok(map)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn convert_args(argv: &[&str]) -> ConvertArgs {
        let mut full = vec!["morph-cli", "convert"];
        full.extend_from_slice(argv);
        match Cli::try_parse_from(full) {
            Ok(Cli {
                command: Command::Convert(args),
            }) => args,
            other => panic!("expected convert, got {other:?}"),
        }
    }

    #[test]
    fn set_parses_json_values() {
        assert_eq!(parse_set("quality=80"), Ok(("quality".into(), json!(80))));
        assert_eq!(
            parse_set("lossless=true"),
            Ok(("lossless".into(), json!(true)))
        );
        assert_eq!(
            parse_set(r#"mode={"type":"target_size","bytes":500000}"#),
            Ok((
                "mode".into(),
                json!({"type": "target_size", "bytes": 500000})
            ))
        );
        assert_eq!(
            parse_set("background=[1,2,3]"),
            Ok(("background".into(), json!([1, 2, 3])))
        );
    }

    #[test]
    fn set_falls_back_to_string() {
        assert_eq!(
            parse_set("format=webp"),
            Ok(("format".into(), json!("webp")))
        );
        assert_eq!(parse_set("name=a=b"), Ok(("name".into(), json!("a=b"))));
        assert_eq!(parse_set("empty="), Ok(("empty".into(), json!(""))));
    }

    #[test]
    fn set_rejects_missing_key_or_equals() {
        assert!(parse_set("novalue").is_err());
        assert!(parse_set("=5").is_err());
    }

    #[test]
    fn numbers_become_strings_where_the_default_is_a_string() {
        let defaults =
            json!({"sample_rate": "keep", "bitrate_kbps": 192, "mode": {"type": "best"}});
        let mut map = json!({"sample_rate": 48000, "bitrate_kbps": 256, "mode": {"type": 5}});
        match_default_types(map.as_object_mut().unwrap(), defaults.as_object().unwrap());
        assert_eq!(
            map,
            json!({"sample_rate": "48000", "bitrate_kbps": 256, "mode": {"type": "5"}})
        );
    }

    #[test]
    fn overrides_merge_in_order() {
        let args = convert_args(&[
            "a.png",
            "--action",
            "image.convert",
            "--options",
            r#"{"format":"png","lossless":true,"quality":10}"#,
            "--format",
            "webp",
            "--quality",
            "70",
            "--set",
            "quality=55",
            "--set",
            "svg_width=512",
        ]);
        let map = args.overrides().unwrap();
        assert_eq!(map["format"], json!("webp"));
        assert_eq!(map["quality"], json!(55));
        assert_eq!(map["lossless"], json!(true));
        assert_eq!(map["svg_width"], json!(512));
    }

    #[test]
    fn dotted_keys_build_nested_objects() {
        let args = convert_args(&[
            "a.png",
            "--action",
            "image.compress",
            "--set",
            "mode.type=target_size",
            "--set",
            "mode.bytes=500000",
            "--set",
            "a.b.c=true",
        ]);
        let map = args.overrides().unwrap();
        assert_eq!(map["mode"], json!({"type": "target_size", "bytes": 500000}));
        assert_eq!(map["a"], json!({"b": {"c": true}}));
        let bad = convert_args(&["a.png", "--action", "x", "--set", "mode.=1"]);
        assert!(bad.overrides().is_err());
    }

    #[test]
    fn dotted_key_merges_into_whole_object() {
        let args = convert_args(&[
            "a.png",
            "--action",
            "x",
            "--set",
            r#"mode={"type":"max_width","width":10}"#,
            "--set",
            "mode.width=20",
        ]);
        assert_eq!(
            args.overrides().unwrap()["mode"],
            json!({"type": "max_width", "width": 20})
        );
    }

    #[test]
    fn shortcuts_set_mode() {
        let m = |extra: &[&str]| {
            let action = if extra[0] == "--target-size" {
                "image.compress"
            } else {
                "image.resize"
            };
            let mut v = vec!["a.png", "--action", action];
            v.extend_from_slice(extra);
            convert_args(&v).overrides().unwrap()["mode"].clone()
        };
        assert_eq!(
            m(&["--target-size", "500KB"]),
            json!({"type": "target_size", "bytes": 512_000})
        );
        assert_eq!(
            m(&["--width", "800"]),
            json!({"type": "max_width", "width": 800})
        );
        assert_eq!(
            m(&["--height", "600"]),
            json!({"type": "max_height", "height": 600})
        );
        assert_eq!(
            m(&["--percent", "25"]),
            json!({"type": "percent", "percent": 25.0})
        );
        assert!(
            Cli::try_parse_from([
                "morph-cli",
                "convert",
                "a",
                "--action",
                "image.resize",
                "--percent",
                "5",
                "--width",
                "9"
            ])
            .is_err()
        );
    }

    #[test]
    fn shortcuts_reject_wrong_action() {
        for flags in [
            ["--target-size", "1MB"],
            ["--width", "5"],
            ["--percent", "5"],
        ] {
            let mut v = vec!["a.png", "--action", "image.convert"];
            v.extend_from_slice(&flags);
            let err = convert_args(&v).overrides().unwrap_err();
            assert!(err.contains(flags[0]), "{err}");
        }
        let v = ["a.png", "--action", "image.resize", "--target-size", "1MB"];
        assert!(convert_args(&v).overrides().is_err());
    }

    #[test]
    fn size_parsing() {
        assert_eq!(parse_size("300000"), Ok(300_000));
        assert_eq!(parse_size("500KB"), Ok(512_000));
        assert_eq!(parse_size("500kb"), Ok(512_000));
        assert_eq!(parse_size("2MB"), Ok(2 * 1024 * 1024));
        assert_eq!(parse_size("1.5MB"), Ok(1_572_864));
        assert_eq!(parse_size("1 GB"), Ok(1024 * 1024 * 1024));
        for bad in ["", "abc", "MB", "0", "-5KB", "5XB", "1.2.3MB"] {
            assert!(parse_size(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn bad_options_json_is_an_error() {
        let args = convert_args(&["a.png", "--action", "x", "--options", "{nope"]);
        assert!(args.overrides().is_err());
        let args = convert_args(&["a.png", "--action", "x", "--options", "[1]"]);
        assert!(args.overrides().is_err());
    }

    #[test]
    fn parses_convert_flags() {
        let args = convert_args(&[
            "a.png",
            "b.jpg",
            "--action",
            "image.resize",
            "--out",
            "out",
            "--threads",
            "3",
            "--json",
        ]);
        assert_eq!(args.files.len(), 2);
        assert_eq!(args.action, "image.resize");
        assert_eq!(args.out, Some(PathBuf::from("out")));
        assert_eq!(args.threads, Some(3));
        assert!(args.json);
    }

    #[test]
    fn usage_errors_are_reported() {
        assert!(Cli::try_parse_from(["morph-cli", "convert", "--action", "x"]).is_err());
        assert!(Cli::try_parse_from(["morph-cli", "convert", "a.png"]).is_err());
        assert!(
            Cli::try_parse_from([
                "morph-cli",
                "convert",
                "a.png",
                "--action",
                "x",
                "--quality",
                "0"
            ])
            .is_err()
        );
        assert!(Cli::try_parse_from(["morph-cli"]).is_err());
    }

    #[test]
    fn parses_other_commands() {
        assert!(matches!(
            Cli::try_parse_from(["morph-cli", "actions"]).map(|c| c.command),
            Ok(Command::Actions)
        ));
        assert!(matches!(
            Cli::try_parse_from(["morph-cli", "options", "image.clean"]).map(|c| c.command),
            Ok(Command::Options { .. })
        ));
    }
}
