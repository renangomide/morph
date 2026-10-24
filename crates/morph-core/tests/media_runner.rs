// SPDX-License-Identifier: GPL-3.0-or-later
//! FFmpeg runner + probe against the real `vendor\ffmpeg` exes. Test clips are
//! generated with FFmpeg's `testsrc` / `sine` sources (no committed fixtures).
//! Prints `SKIPPED:` and passes when FFmpeg isn't there (run `scripts\fetch-deps.ps1`).

use morph_core::CancelToken;
use morph_core::converter::TaskCtx;
use morph_core::error::ErrorKind;
use morph_core::media::{self, ffmpeg, probe};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

fn have_ffmpeg() -> bool {
    if ffmpeg::find().is_none() {
        println!("SKIPPED: FFmpeg not found (run scripts\\fetch-deps.ps1)");
        return false;
    }
    true
}

fn args(list: &[&str]) -> Vec<OsString> {
    list.iter().map(OsString::from).collect()
}

/// A 2 s 320×240 H.264 + AAC clip at `folder/name`, written like a converter
/// does: OutputFile temp file, `-f mp4`, commit.
fn make_clip(folder: &Path, name: &str, ctx: &TaskCtx<'_>) -> (PathBuf, Vec<f32>) {
    let out = media::output_file(folder, name).unwrap();
    let mut a = args(&[
        "-f",
        "lavfi",
        "-i",
        "testsrc=duration=2:size=320x240:rate=25",
        "-f",
        "lavfi",
        "-i",
        "sine=frequency=440:duration=2",
        "-c:v",
        "libx264",
        "-preset",
        "ultrafast",
        "-pix_fmt",
        "yuv420p",
        "-c:a",
        "aac",
        "-shortest",
        "-f",
        "mp4",
    ]);
    a.push(ffmpeg::path_arg(out.temp_path()));
    let mut seen = Vec::new();
    ffmpeg::run(&a, Some(2.0), ctx, &mut |f| seen.push(f)).unwrap();
    (out.commit().unwrap(), seen)
}

#[test]
fn generate_probe_and_progress() {
    if !have_ffmpeg() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let work = dir.path().join("work");
    let cancel = CancelToken::new();
    let ctx = TaskCtx::new(&cancel, &|_, _| {}, &work);

    let (clip, seen) = make_clip(dir.path(), "Vídeo 報告.mp4", &ctx);
    assert_eq!(clip.file_name().unwrap(), "Vídeo 報告.mp4");
    assert_eq!(seen.last(), Some(&1.0), "{seen:?}");
    assert!(seen.windows(2).all(|w| w[0] <= w[1]), "{seen:?}");

    let info = probe::probe(&clip, &cancel).unwrap();
    assert!((info.duration_s.unwrap() - 2.0).abs() < 0.2, "{info:?}");
    assert!(info.container.contains("mp4"));
    let v = info.video().unwrap();
    assert_eq!(
        (v.codec.as_str(), v.width, v.height),
        ("h264", Some(320), Some(240))
    );
    assert!((v.fps.unwrap() - 25.0).abs() < 0.01);
    let a = info.audio().unwrap();
    assert_eq!(a.codec, "aac");
    assert!(a.sample_rate.is_some() && a.channels.is_some());

    // Same name again: collision → " (1)", the first file is untouched.
    let before = std::fs::read(&clip).unwrap();
    let (second, _) = make_clip(dir.path(), "Vídeo 報告.mp4", &ctx);
    assert_eq!(second.file_name().unwrap(), "Vídeo 報告 (1).mp4");
    assert_eq!(std::fs::read(&clip).unwrap(), before);
    let leftovers: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with(".~morph-"))
        .collect();
    assert!(leftovers.is_empty(), "temp files left: {leftovers:?}");

    // `9.0.2-…` (release tarball builds) or `n9.0.2-morph-1` (morph-ffmpeg, built from the git tag).
    assert!(
        ffmpeg::version().is_some_and(|v| v.trim_start_matches('n').starts_with(char::is_numeric))
    );
}

#[test]
fn broken_and_missing_inputs_fail_plainly() {
    if !have_ffmpeg() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let broken = dir.path().join("broken.mp4");
    std::fs::write(&broken, b"this is not a video").unwrap();
    let cancel = CancelToken::new();
    let err = probe::probe(&broken, &cancel).unwrap_err();
    assert_eq!(err.kind, ErrorKind::BadInput, "{err}");
    assert!(err.message.contains("damaged"), "{}", err.message);
    assert!(!err.details.is_empty());
    assert_eq!(std::fs::read(&broken).unwrap(), b"this is not a video");

    let err = probe::probe(&dir.path().join("gone.mp4"), &cancel).unwrap_err();
    assert_eq!(err.kind, ErrorKind::BadInput, "{err}");

    // A failing ffmpeg run reports the stderr tail and leaves no output behind.
    let work = dir.path().join("work");
    let ctx = TaskCtx::new(&cancel, &|_, _| {}, &work);
    let out = media::output_file(dir.path(), "x.mp4").unwrap();
    let mut a = args(&["-i"]);
    a.push(ffmpeg::path_arg(&broken));
    a.extend(args(&["-f", "mp4"]));
    a.push(ffmpeg::path_arg(out.temp_path()));
    let err = ffmpeg::run(&a, None, &ctx, &mut |_| {}).unwrap_err();
    assert_eq!(err.kind, ErrorKind::BadInput, "{err}");
    drop(out);
    let names: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .flatten()
        .map(|e| e.file_name())
        .collect();
    assert_eq!(names.len(), 2, "only broken.mp4 + work: {names:?}");
}

#[test]
fn cancel_kills_ffmpeg_quickly() {
    if !have_ffmpeg() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let cancel = CancelToken::new();
    let canceller = cancel.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(500));
        canceller.cancel();
    });
    let work = dir.path().join("work");
    let ctx = TaskCtx::new(&cancel, &|_, _| {}, &work);
    let a = args(&[
        "-f",
        "lavfi",
        "-i",
        "testsrc=duration=3600:size=1280x720:rate=30",
        "-c:v",
        "libx264",
        "-f",
        "null",
        "-",
    ]);
    let started = Instant::now();
    let mut calls = 0;
    let err = ffmpeg::run(&a, Some(3600.0), &ctx, &mut |_| calls += 1).unwrap_err();
    assert!(err.is_cancelled(), "{err}");
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "{:?}",
        started.elapsed()
    );
}
