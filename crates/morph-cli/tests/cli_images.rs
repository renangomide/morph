// SPDX-License-Identifier: GPL-3.0-or-later
//! Runs the built `morph-cli` over `tests/fixtures/`.
//!
//! Tests that need the real image actions / image I/O are `#[ignore]`d until
//! those land; run them with `cargo test -p morph-cli -- --ignored`.

use morph_core::image::io::{DecodeOptions, Decoded, decode};
use morph_core::image::options::OutputFormat;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// Every input fixture except `broken.png`, with its upright size in pixels
/// when converted with default options (SVG: 1024 px wide by default).
const INPUTS: &[(&str, u32, u32)] = &[
    ("sample.jpg", 96, 64),
    ("sample.jpeg", 96, 64),
    ("sample.jfif", 96, 64),
    ("sample.png", 96, 64),
    ("sample.webp", 96, 64),
    ("sample.avif", 96, 64),
    ("sample.heic", 96, 64),
    ("sample.heif", 96, 64),
    ("sample.bmp", 96, 64),
    ("sample.tif", 96, 64),
    ("sample.tiff", 96, 64),
    ("sample.gif", 64, 48),
    ("sample.ico", 48, 48),
    ("sample.tga", 96, 64),
    ("sample.qoi", 96, 64),
    ("sample.svg", 1024, 512),
    ("orientation6.jpg", 32, 64),
    ("exif-gps.jpg", 96, 64),
    ("icc-profile.jpg", 64, 48),
    ("Férias 夏.jpg", 64, 48),
    ("café.png", 48, 48),
];

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures")
}

fn fixture(name: &str) -> PathBuf {
    fixtures().join(name)
}

fn cli<I, S>(args: I) -> Output
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    Command::new(env!("CARGO_BIN_EXE_morph-cli"))
        .args(args)
        .output()
        .expect("morph-cli should start")
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn code(o: &Output) -> Option<i32> {
    o.status.code()
}

/// Run `convert` on one fixture into a fresh folder and return the folder's files.
fn run_one(
    tmp: &Path,
    input: &str,
    action: &str,
    extra: &[&str],
) -> Result<(Output, Vec<PathBuf>), String> {
    let out_dir = tmp.join(format!(
        "{}-{}",
        input.replace(['.', ' '], "_"),
        extra.join("_").replace(['"', '{', '}', ':', ',', '='], "")
    ));
    std::fs::create_dir_all(&out_dir).map_err(|e| e.to_string())?;
    let mut args: Vec<std::ffi::OsString> = vec!["convert".into(), fixture(input).into_os_string()];
    args.extend(["--action", action, "--out"].map(Into::into));
    args.push(out_dir.clone().into_os_string());
    args.extend(extra.iter().map(|s| (*s).into()));
    let output = cli(args);
    let mut files: Vec<PathBuf> = std::fs::read_dir(&out_dir)
        .map_err(|e| e.to_string())?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .collect();
    files.sort();
    Ok((output, files))
}

fn decode_file(path: &Path) -> Result<Decoded, String> {
    decode(path, &DecodeOptions::default()).map_err(|e| format!("{e}"))
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

/// Convert every input to `format` and check exit code, output file, decode and size.
fn convert_all_to(format: OutputFormat) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let ext = format.extension();
    let mut failures = Vec::new();
    for (name, w, h) in INPUTS {
        let (output, files) = match run_one(tmp.path(), name, "image.convert", &["--format", ext]) {
            Ok(r) => r,
            Err(e) => {
                failures.push(format!("{name} -> {ext}: {e}"));
                continue;
            }
        };
        if code(&output) != Some(0) {
            failures.push(format!(
                "{name} -> {ext}: exit {:?}\n{}",
                code(&output),
                stdout(&output)
            ));
            continue;
        }
        let [file] = files.as_slice() else {
            failures.push(format!(
                "{name} -> {ext}: expected one output, got {files:?}"
            ));
            continue;
        };
        if file.extension().and_then(OsStr::to_str) != Some(ext) {
            failures.push(format!("{name} -> {ext}: wrong extension {file:?}"));
            continue;
        }
        match decode_file(file) {
            Ok(d) => {
                // ICO outputs always carry the 256 px entry, padded to a square.
                let want = if format == OutputFormat::Ico {
                    (256, 256)
                } else {
                    (*w, *h)
                };
                let got = (d.image.width(), d.image.height());
                if got != want {
                    failures.push(format!("{name} -> {ext}: size {got:?}, expected {want:?}"));
                }
                if d.orientation != 1 {
                    failures.push(format!(
                        "{name} -> {ext}: output still has EXIF orientation {}",
                        d.orientation
                    ));
                }
            }
            Err(e) => failures.push(format!("{name} -> {ext}: output does not decode: {e}")),
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

macro_rules! convert_to {
    ($($test:ident => $format:expr),* $(,)?) => {$(
        #[test]
        fn $test() {
            convert_all_to($format);
        }
    )*};
}

convert_to! {
    convert_all_to_jpg => OutputFormat::Jpg,
    convert_all_to_png => OutputFormat::Png,
    convert_all_to_webp => OutputFormat::Webp,
    convert_all_to_avif => OutputFormat::Avif,
    convert_all_to_bmp => OutputFormat::Bmp,
    convert_all_to_tiff => OutputFormat::Tiff,
    convert_all_to_gif => OutputFormat::Gif,
    convert_all_to_ico => OutputFormat::Ico,
}

#[test]
fn broken_png_fails_with_plain_english_error() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let (output, files) = run_one(
        tmp.path(),
        "broken.png",
        "image.convert",
        &["--format", "jpg"],
    )
    .expect("run");
    let text = stdout(&output);
    assert_eq!(code(&output), Some(1), "{text}");
    assert!(files.is_empty(), "no output may be left behind: {files:?}");
    let line = text
        .lines()
        .find(|l| l.contains("broken.png") && l.starts_with("FAIL"))
        .unwrap_or_else(|| panic!("no failure line in:\n{text}"));
    for debug_noise in ["Kind(", "Os {", "panicked", "unwrap", "{:?}"] {
        assert!(!line.contains(debug_noise), "not plain English: {line}");
    }
}

#[test]
fn broken_png_error_says_the_image_is_damaged() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let (output, _) = run_one(
        tmp.path(),
        "broken.png",
        "image.convert",
        &["--format", "jpg"],
    )
    .expect("run");
    let text = stdout(&output).to_lowercase();
    assert_eq!(code(&output), Some(1));
    assert!(
        text.contains("damaged") || text.contains("corrupt"),
        "unexpected message:\n{text}"
    );
}

#[test]
fn compress_jpg_through_the_cli() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let (output, files) = run_one(
        tmp.path(),
        "sample.jpg",
        "image.compress",
        &["--set", r#"mode={"type":"quality","quality":40}"#],
    )
    .expect("run");
    assert_eq!(code(&output), Some(0), "{}", stdout(&output));
    let [file] = files.as_slice() else {
        panic!("expected one output, got {files:?}");
    };
    let d = decode_file(file).expect("output decodes");
    assert_eq!((d.image.width(), d.image.height()), (96, 64));
}

#[test]
fn compress_best_mode_through_the_cli() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let (output, files) = run_one(
        tmp.path(),
        "sample.png",
        "image.compress",
        &["--set", "mode.type=best"],
    )
    .expect("run");
    assert_eq!(code(&output), Some(0), "{}", stdout(&output));
    // Either a smaller file, or "Already optimized" and nothing written.
    for file in &files {
        let d = decode_file(file).expect("output decodes");
        assert_eq!((d.image.width(), d.image.height()), (96, 64));
    }
}

#[test]
fn resize_through_the_cli() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let (output, files) = run_one(
        tmp.path(),
        "sample.jpg",
        "image.resize",
        &["--set", r#"mode={"type":"max_width","width":48}"#],
    )
    .expect("run");
    assert_eq!(code(&output), Some(0), "{}", stdout(&output));
    let [file] = files.as_slice() else {
        panic!("expected one output, got {files:?}");
    };
    let d = decode_file(file).expect("output decodes");
    assert_eq!((d.image.width(), d.image.height()), (48, 32));
}

#[test]
fn clean_removes_metadata_and_straightens_the_image() {
    let tmp = tempfile::tempdir().expect("tempdir");

    // Strip everything: upright pixels, no EXIF left.
    let (output, files) = run_one(tmp.path(), "orientation6.jpg", "image.clean", &[]).expect("run");
    assert_eq!(code(&output), Some(0), "{}", stdout(&output));
    let [file] = files.as_slice() else {
        panic!("expected one output, got {files:?}");
    };
    let d = decode_file(file).expect("output decodes");
    assert_eq!((d.image.width(), d.image.height()), (32, 64));
    assert_eq!(d.orientation, 1);

    let (output, files) = run_one(tmp.path(), "exif-gps.jpg", "image.clean", &[]).expect("run");
    assert_eq!(code(&output), Some(0), "{}", stdout(&output));
    let [file] = files.as_slice() else {
        panic!("expected one output, got {files:?}");
    };
    let d = decode_file(file).expect("output decodes");
    assert!(
        d.meta
            .exif
            .as_deref()
            .is_none_or(|e| !contains(e, b"Morph Test")),
        "camera tags must be gone"
    );

    // GPS only: other tags stay, the GPS pointer (tag 0x8825) goes.
    let (output, files) = run_one(
        tmp.path(),
        "exif-gps.jpg",
        "image.clean",
        &["--set", "strip=gps_only"],
    )
    .expect("run");
    assert_eq!(code(&output), Some(0), "{}", stdout(&output));
    let [file] = files.as_slice() else {
        panic!("expected one output, got {files:?}");
    };
    let d = decode_file(file).expect("output decodes");
    let exif = d.meta.exif.expect("EXIF kept when only GPS is removed");
    assert!(contains(&exif, b"Morph Test"), "other tags must stay");
    assert!(
        !contains(&exif, &[0x25, 0x88, 0x04, 0x00]),
        "GPS IFD pointer must be gone"
    );
}

// ---- CLI behavior that does not need the image actions ----

#[test]
fn actions_lists_defaults_as_json() {
    let o = cli(["actions"]);
    assert_eq!(code(&o), Some(0));
    let v: serde_json::Value = serde_json::from_str(&stdout(&o)).expect("valid JSON");
    for id in [
        "image.convert",
        "image.compress",
        "image.resize",
        "image.clean",
    ] {
        assert!(v[id].is_object(), "missing {id}");
    }
}

#[test]
fn options_prints_one_action() {
    let o = cli(["options", "image.convert"]);
    assert_eq!(code(&o), Some(0));
    let v: serde_json::Value = serde_json::from_str(&stdout(&o)).expect("valid JSON");
    assert_eq!(v["format"], "png");

    let o = cli(["options", "image.compress"]);
    assert_eq!(code(&o), Some(0));
    let v: serde_json::Value = serde_json::from_str(&stdout(&o)).expect("valid JSON");
    assert_eq!(
        v["mode"],
        serde_json::json!({"type": "best"}),
        "Best is the default"
    );
}

#[test]
fn unknown_action_exits_with_2() {
    assert_eq!(code(&cli(["options", "nope.nope"])), Some(2));
    let o = cli(["convert", "x.png", "--action", "nope.nope"]);
    assert_eq!(code(&o), Some(2));
    assert!(String::from_utf8_lossy(&o.stderr).contains("unknown action"));
}

#[test]
fn usage_errors_exit_with_2() {
    assert_eq!(code(&cli(["convert", "x.png"])), Some(2));
    assert_eq!(
        code(&cli(["convert", "--action", "image.convert"])),
        Some(2)
    );
    assert_eq!(
        code(&cli([
            "convert",
            "x.png",
            "--action",
            "image.convert",
            "--set",
            "oops"
        ])),
        Some(2)
    );
    assert_eq!(code(&cli(Vec::<&str>::new())), Some(2));
}

#[test]
fn unsupported_file_is_skipped_not_failed() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let txt = tmp.path().join("notes.txt");
    std::fs::write(&txt, "hello").expect("write");
    let o = cli([
        OsStr::new("convert"),
        txt.as_os_str(),
        OsStr::new("--action"),
        OsStr::new("image.convert"),
    ]);
    assert_eq!(code(&o), Some(0), "{}", stdout(&o));
    assert!(stdout(&o).contains("skipped"));
}

#[test]
fn json_mode_prints_one_object_per_line() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let txt = tmp.path().join("notes.txt");
    std::fs::write(&txt, "hello").expect("write");
    let o = cli([
        OsStr::new("convert"),
        txt.as_os_str(),
        OsStr::new("--action"),
        OsStr::new("image.convert"),
        OsStr::new("--json"),
    ]);
    assert_eq!(code(&o), Some(0));
    let text = stdout(&o);
    let events: Vec<serde_json::Value> = text
        .lines()
        .map(|l| serde_json::from_str(l).expect("each line is JSON"))
        .collect();
    assert_eq!(
        events.first().map(|e| e["event"].as_str()),
        Some(Some("task_skipped"))
    );
    assert_eq!(
        events.last().map(|e| e["event"].as_str()),
        Some(Some("job_finished"))
    );
}
