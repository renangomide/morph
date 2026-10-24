// SPDX-License-Identifier: GPL-3.0-or-later
//! Video → Compress "Best (automatic)" (SPEC §4.3) against the real FFmpeg.
//! Clips are generated with lavfi at test time. Prints `SKIPPED:` and passes
//! when FFmpeg isn't there.

use morph_core::error::ErrorKind;
use morph_core::media::best::{self, NOTE_ALREADY_COMPRESSED};
use morph_core::media::options::{VideoCompressOpts, VideoContainer};
use morph_core::media::{ffmpeg, probe};
use morph_core::{Action, CancelToken, Converter, OutputSpec, Task, TaskCtx, TaskId, TaskOutput};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

fn have_ffmpeg() -> bool {
    if ffmpeg::find().is_none() {
        println!("SKIPPED: FFmpeg not found (run scripts\\fetch-deps.ps1)");
        return false;
    }
    true
}

/// Run ffmpeg directly (test helper): `args` then `out`.
fn ffmpeg_to(args: &[&str], out: &Path) {
    let tools = ffmpeg::find().unwrap();
    let status = std::process::Command::new(&tools.ffmpeg)
        .args(["-hide_banner", "-loglevel", "error", "-y"])
        .args(args)
        .arg(out)
        .status()
        .unwrap();
    assert!(status.success(), "generating {}", out.display());
}

/// `video` lavfi source, `secs` long, + a sine track, encoded with `venc` + `aenc`.
fn make_clip(
    dir: &Path,
    name: &str,
    video: &str,
    secs: u32,
    venc: &[&str],
    aenc: &[&str],
) -> PathBuf {
    let out = dir.join(name);
    // The duration goes on the source, before any `,filter`.
    let v = match video.split_once(',') {
        Some((src, filters)) => format!("{src}:d={secs},{filters}"),
        None => format!("{video}:d={secs}"),
    };
    let a = format!("sine=frequency=440:duration={secs}");
    let mut args = vec!["-f", "lavfi", "-i", &v, "-f", "lavfi", "-i", &a];
    args.extend(venc);
    args.extend(["-pix_fmt", "yuv420p"]);
    args.extend(aenc);
    args.push("-shortest");
    ffmpeg_to(&args, &out);
    out
}

fn compress(input: &Path, cancel: &CancelToken) -> Result<TaskOutput, morph_core::ConvertError> {
    let task = Task {
        id: TaskId(1),
        inputs: vec![input.to_path_buf()],
        action: Action::VideoCompress(VideoCompressOpts::default()),
        output: OutputSpec::default(),
    };
    let temp = tempfile::tempdir().unwrap();
    let ctx = TaskCtx::new(cancel, &|_, _| {}, temp.path());
    let before = std::fs::metadata(input).unwrap().len();
    let r = morph_core::media::VideoCompress.run(&task, &ctx);
    assert_eq!(
        std::fs::metadata(input).unwrap().len(),
        before,
        "input changed"
    );
    r
}

/// Quality value the search picks for `input` (default options).
fn quality_for(input: &Path) -> u8 {
    let cancel = CancelToken::new();
    let temp = tempfile::tempdir().unwrap();
    let ctx = TaskCtx::new(&cancel, &|_, _| {}, temp.path());
    let info = probe::probe(input, &cancel).unwrap();
    let settings = best::plan(input, &info, &VideoCompressOpts::default(), &ctx).unwrap();
    best::pick_quality(input, &info, &settings, &ctx, &mut |_| {}).unwrap()
}

/// VMAF of `distorted` against `reference` (whole clips).
fn vmaf(distorted: &Path, reference: &Path) -> f64 {
    let tools = ffmpeg::find().unwrap();
    let out = std::process::Command::new(&tools.ffmpeg)
        .args(["-hide_banner", "-nostats", "-i"])
        .arg(distorted)
        .arg("-i")
        .arg(reference)
        .args(["-lavfi", "[0:v][1:v]libvmaf=n_threads=8", "-f", "null", "-"])
        .output()
        .unwrap();
    let log = String::from_utf8_lossy(&out.stderr);
    let line = log
        .lines()
        .find(|l| l.contains("VMAF score:"))
        .unwrap_or_else(|| panic!("{log}"));
    line.rsplit(':').next().unwrap().trim().parse().unwrap()
}

fn decodes_cleanly(path: &Path) {
    let tools = ffmpeg::find().unwrap();
    let out = std::process::Command::new(&tools.ffmpeg)
        .args(["-v", "error", "-i"])
        .arg(path)
        .args(["-f", "null", "-"])
        .output()
        .unwrap();
    assert!(
        out.status.success() && out.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn names_in(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    v.sort();
    v
}

const X264_HQ: &[&str] = &["-c:v", "libx264", "-preset", "ultrafast", "-crf", "8"];
const AAC_256: &[&str] = &["-c:a", "aac", "-b:a", "256k"];

#[test]
fn easy_clip_gets_a_higher_crf_than_a_noisy_one() {
    if !have_ffmpeg() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let easy = make_clip(
        dir.path(),
        "easy.mp4",
        "smptebars=s=1280x720:r=25",
        10,
        X264_HQ,
        AAC_256,
    );
    let noisy = make_clip(
        dir.path(),
        "noisy.mp4",
        "testsrc2=s=1280x720:r=25,noise=alls=20:allf=t",
        10,
        X264_HQ,
        AAC_256,
    );
    let t = Instant::now();
    let q_easy = quality_for(&easy);
    let t_easy = t.elapsed();
    let t = Instant::now();
    let q_noisy = quality_for(&noisy);
    println!(
        "CRF easy {q_easy} ({t_easy:?}), noisy {q_noisy} ({:?})",
        t.elapsed()
    );
    assert!(q_easy > q_noisy, "easy {q_easy} noisy {q_noisy}");
}

#[test]
fn high_bitrate_clip_gets_smaller_and_looks_the_same() {
    if !have_ffmpeg() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let input = make_clip(
        dir.path(),
        "Vídeo 報告.mp4",
        "testsrc2=s=1280x720:r=30,noise=alls=6:allf=t",
        20,
        &[
            "-c:v", "libx264", "-preset", "veryfast", "-b:v", "25M", "-maxrate", "25M", "-bufsize",
            "50M",
        ],
        AAC_256,
    );
    let t = Instant::now();
    let out = compress(&input, &CancelToken::new()).unwrap();
    println!("compressed in {:?}", t.elapsed());
    assert_eq!(out.note, None);
    assert_eq!(out.outputs.len(), 1);
    let output = &out.outputs[0];
    assert_eq!(output.file_name().unwrap(), "Vídeo 報告 (compressed).mp4");
    let (a, b) = (
        std::fs::metadata(&input).unwrap().len(),
        std::fs::metadata(output).unwrap().len(),
    );
    println!("{a} -> {b} bytes ({:.1}%)", b as f64 * 100.0 / a as f64);
    assert!((b as f64) <= a as f64 * 0.95);

    let info = probe::probe(output, &CancelToken::new()).unwrap();
    assert!(info.container.contains("mp4"), "{}", info.container);
    let v = info.video().unwrap();
    assert_eq!(
        (v.codec.as_str(), v.width, v.height),
        ("h264", Some(1280), Some(720))
    );
    let au = info.audio().unwrap();
    assert_eq!(au.codec, "aac");
    assert!(
        au.bit_rate.unwrap() < 192_000,
        "re-encoded at 160k: {:?}",
        au.bit_rate
    );
    assert!(
        (info.duration_s.unwrap() - 20.0).abs() < 0.3,
        "{:?}",
        info.duration_s
    );
    decodes_cleanly(output);
    let score = vmaf(output, &input);
    println!("full-output VMAF {score:.2}");
    assert!(score >= 92.0, "{score}");
    assert_eq!(names_in(dir.path()).len(), 2, "{:?}", names_in(dir.path()));
}

#[test]
fn already_small_clip_writes_nothing() {
    if !have_ffmpeg() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let input = make_clip(
        dir.path(),
        "small.mp4",
        "testsrc2=s=640x360:r=25",
        10,
        &["-c:v", "libx264", "-crf", "35"],
        &["-c:a", "aac", "-b:a", "96k"],
    );
    let out = compress(&input, &CancelToken::new()).unwrap();
    assert!(out.outputs.is_empty(), "{out:?}");
    assert_eq!(out.note.as_deref(), Some(NOTE_ALREADY_COMPRESSED));
    assert_eq!(names_in(dir.path()), ["small.mp4"]);
}

#[test]
fn webm_stays_webm_with_vp9_and_opus() {
    if !have_ffmpeg() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let input = make_clip(
        dir.path(),
        "clip.webm",
        "testsrc2=s=640x360:r=25",
        10,
        &[
            "-c:v",
            "libvpx-vp9",
            "-b:v",
            "8M",
            "-deadline",
            "realtime",
            "-cpu-used",
            "8",
        ],
        &["-c:a", "libopus", "-b:a", "96k"],
    );
    let settings = {
        let cancel = CancelToken::new();
        let temp = tempfile::tempdir().unwrap();
        let ctx = TaskCtx::new(&cancel, &|_, _| {}, temp.path());
        let info = probe::probe(&input, &cancel).unwrap();
        best::plan(&input, &info, &VideoCompressOpts::default(), &ctx).unwrap()
    };
    assert_eq!(settings.container, VideoContainer::Webm);
    let out = compress(&input, &CancelToken::new()).unwrap();
    assert_eq!(out.outputs.len(), 1, "{out:?}");
    let output = &out.outputs[0];
    assert_eq!(output.file_name().unwrap(), "clip (compressed).webm");
    let info = probe::probe(output, &CancelToken::new()).unwrap();
    assert!(info.container.contains("webm"), "{}", info.container);
    assert_eq!(info.video().unwrap().codec, "vp9");
    assert_eq!(info.audio().unwrap().codec, "opus");
    decodes_cleanly(output);
}

#[test]
fn short_clip_is_measured_whole() {
    if !have_ffmpeg() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let input = make_clip(
        dir.path(),
        "short.mp4",
        "testsrc2=s=640x360:r=25",
        1,
        X264_HQ,
        AAC_256,
    );
    let out = compress(&input, &CancelToken::new()).unwrap();
    assert_eq!(out.outputs.len(), 1, "{out:?}");
    decodes_cleanly(&out.outputs[0]);
}

#[test]
fn broken_file_fails_plainly() {
    if !have_ffmpeg() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("broken.mp4");
    std::fs::write(&input, b"not a video").unwrap();
    let err = compress(&input, &CancelToken::new()).unwrap_err();
    assert_eq!(err.kind, ErrorKind::BadInput, "{err}");
    assert_eq!(std::fs::read(&input).unwrap(), b"not a video");
    assert_eq!(names_in(dir.path()), ["broken.mp4"]);
}

#[test]
fn cancel_stops_and_leaves_nothing() {
    if !have_ffmpeg() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let input = make_clip(
        dir.path(),
        "clip.mp4",
        "testsrc2=s=1280x720:r=25",
        30,
        X264_HQ,
        AAC_256,
    );
    for delay in [Duration::from_millis(800), Duration::from_secs(6)] {
        let cancel = CancelToken::new();
        let c = cancel.clone();
        std::thread::spawn(move || {
            std::thread::sleep(delay);
            c.cancel();
        });
        let t = Instant::now();
        let err = compress(&input, &cancel).unwrap_err();
        assert!(err.is_cancelled(), "{err}");
        assert!(
            t.elapsed() < delay + Duration::from_secs(3),
            "{:?}",
            t.elapsed()
        );
        assert_eq!(names_in(dir.path()), ["clip.mp4"]);
    }
}

/// SPEC acceptance: a 60 s 1080p clip in < 2× a plain CRF 23 encode. Run with
/// `cargo test --release -p morph-core --test video_best -- --ignored --nocapture`.
#[test]
#[ignore = "perf: 60 s 1080p, several minutes"]
fn perf_best_vs_plain_crf23() {
    if !have_ffmpeg() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let input = make_clip(
        dir.path(),
        "big.mp4",
        "testsrc2=s=1920x1080:r=30,noise=alls=6:allf=t",
        60,
        &[
            "-c:v", "libx264", "-preset", "veryfast", "-b:v", "25M", "-maxrate", "25M", "-bufsize",
            "50M",
        ],
        AAC_256,
    );
    let plain_out = dir.path().join("plain.mkv");
    let t = Instant::now();
    let plain_in = input.to_string_lossy().into_owned();
    ffmpeg_to(
        &[
            "-i", &plain_in, "-c:v", "libx264", "-crf", "23", "-pix_fmt", "yuv420p", "-c:a", "aac",
            "-b:a", "160k",
        ],
        &plain_out,
    );
    let plain = t.elapsed();
    let t = Instant::now();
    let out = compress(&input, &CancelToken::new()).unwrap();
    let ours = t.elapsed();
    let sizes = |p: &Path| std::fs::metadata(p).unwrap().len();
    println!(
        "plain CRF 23: {plain:?} {} B; best: {ours:?} {:?} ({:.2}x) input {} B",
        sizes(&plain_out),
        out.outputs.first().map(|p| sizes(p)),
        ours.as_secs_f64() / plain.as_secs_f64(),
        sizes(&input),
    );
    assert!(ours < plain * 2, "{ours:?} vs {plain:?}");
}

/// Best on the GPU encoder Auto picks (NVENC/QSV/AMF): prints the quality value,
/// size, VMAF and time. Run with
/// `cargo test --release -p morph-core --test video_best gpu -- --ignored --nocapture`.
#[test]
#[ignore = "needs a GPU encoder"]
fn gpu_best_numbers() {
    use morph_core::media::encode::Backend;
    if !have_ffmpeg() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let input = make_clip(
        dir.path(),
        "gpu.mp4",
        "testsrc2=s=1920x1080:r=30,noise=alls=6:allf=t",
        30,
        &[
            "-c:v", "libx264", "-preset", "veryfast", "-b:v", "25M", "-maxrate", "25M", "-bufsize",
            "50M",
        ],
        AAC_256,
    );
    let encoder = {
        let cancel = CancelToken::new();
        let temp = tempfile::tempdir().unwrap();
        let ctx = TaskCtx::new(&cancel, &|_, _| {}, temp.path());
        let info = probe::probe(&input, &cancel).unwrap();
        best::plan(&input, &info, &VideoCompressOpts::default(), &ctx)
            .unwrap()
            .encoder
    };
    if encoder.backend == Backend::Cpu {
        println!("SKIPPED: no GPU encoder");
        return;
    }
    let t = Instant::now();
    let q = quality_for(&input);
    let search = t.elapsed();
    let t = Instant::now();
    let out = compress(&input, &CancelToken::new()).unwrap();
    let total = t.elapsed();
    let output = &out.outputs[0];
    let (a, b) = (
        std::fs::metadata(&input).unwrap().len(),
        std::fs::metadata(output).unwrap().len(),
    );
    println!(
        "{encoder:?}: q {q}, search {search:?}, compress {total:?}, {a} -> {b} B ({:.1}%), VMAF {:.2}",
        b as f64 * 100.0 / a as f64,
        vmaf(output, &input)
    );
}
