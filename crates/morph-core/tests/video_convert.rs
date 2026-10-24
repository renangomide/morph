// SPDX-License-Identifier: GPL-3.0-or-later
//! Acceptance (SPEC §4.3): Video → Convert, Compress presets,
//! GPU detection. Clips are generated with lavfi at test time. Prints `SKIPPED:`
//! and passes when FFmpeg isn't there.

use morph_core::cancel::CancelToken;
use morph_core::converter::{Converter, TaskCtx, TaskOutput};
use morph_core::error::{ConvertError, ErrorKind};
use morph_core::media::compress::WELL_COMPRESSED_NOTE;
use morph_core::media::convert::COPIED_NOTE;
use morph_core::media::options::{
    EncoderChoice, Resolution, VideoCodec, VideoCompressOpts, VideoCompressPreset, VideoContainer,
    VideoConvertOpts,
};
use morph_core::media::probe::{self, MediaInfo};
use morph_core::media::{self, ffmpeg, gpu};
use morph_core::{Action, Job, OutputSpec};
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

fn os(list: &[&str]) -> Vec<OsString> {
    list.iter().map(OsString::from).collect()
}

/// Generate `folder/name`: testsrc2 `w`×`h` for `secs` + a sine, encoded with `codec_args`.
fn make_clip(
    folder: &Path,
    name: &str,
    (w, h, secs): (u32, u32, u32),
    codec_args: &[&str],
) -> PathBuf {
    let cancel = CancelToken::new();
    let work = tempfile::tempdir().unwrap();
    let ctx = TaskCtx::new(&cancel, &|_, _| {}, work.path());
    let ext = Path::new(name).extension().unwrap().to_str().unwrap();
    let out = media::output_file(folder, name).unwrap();
    let mut a = os(&[
        "-f",
        "lavfi",
        "-i",
        &format!("testsrc2=duration={secs}:size={w}x{h}:rate=25"),
        "-f",
        "lavfi",
        "-i",
        &format!("sine=frequency=440:duration={secs}"),
        "-shortest",
    ]);
    a.extend(os(codec_args));
    let muxer = if ext == "mts" {
        "mpegts"
    } else {
        ffmpeg::muxer(ext).unwrap()
    };
    a.extend(os(&["-f", muxer]));
    a.push(ffmpeg::path_arg(out.temp_path()));
    ffmpeg::run(&a, None, &ctx, &mut |_| {}).unwrap();
    out.commit().unwrap()
}

const H264_AAC: &[&str] = &[
    "-c:v",
    "libx264",
    "-preset",
    "ultrafast",
    "-pix_fmt",
    "yuv420p",
    "-c:a",
    "aac",
];

fn run_with(
    action: Action,
    input: &Path,
    cancel: &CancelToken,
) -> Result<TaskOutput, ConvertError> {
    let job = Job::for_files(&[input.to_path_buf()], &action, &OutputSpec::default());
    assert_eq!(job.tasks.len(), 1, "{:?}", job.skipped);
    let work = tempfile::tempdir().unwrap();
    let ctx = TaskCtx::new(cancel, &|_, _| {}, work.path());
    match action {
        Action::VideoConvert(_) => media::VideoConvert.run(&job.tasks[0], &ctx),
        Action::VideoCompress(_) => media::VideoCompress.run(&job.tasks[0], &ctx),
        _ => unreachable!(),
    }
}

/// Run and check: the input is unchanged and no temp file is left.
fn run(action: Action, input: &Path) -> Result<TaskOutput, ConvertError> {
    let before = std::fs::read(input).unwrap();
    let result = run_with(action, input, &CancelToken::new());
    assert_eq!(std::fs::read(input).unwrap(), before, "input changed");
    assert_no_temp(input.parent().unwrap());
    result
}

fn assert_no_temp(folder: &Path) {
    let temp: Vec<_> = std::fs::read_dir(folder)
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with(".~morph-"))
        .collect();
    assert!(temp.is_empty(), "temp files left: {temp:?}");
}

fn convert(format: VideoContainer) -> Action {
    Action::VideoConvert(VideoConvertOpts {
        format,
        encoder: EncoderChoice::Cpu,
        ..Default::default()
    })
}

fn compress(preset: VideoCompressPreset, resolution: Resolution, encoder: EncoderChoice) -> Action {
    Action::VideoCompress(VideoCompressOpts {
        preset,
        target_mb: 1,
        resolution,
        encoder,
    })
}

fn probe(path: &Path) -> MediaInfo {
    probe::probe(path, &CancelToken::new()).unwrap()
}

fn codecs(info: &MediaInfo) -> (String, String) {
    (
        info.video().unwrap().codec.clone(),
        info.audio().map(|a| a.codec.clone()).unwrap_or_default(),
    )
}

/// `ffmpeg -v error -i out -f null -` succeeds with an empty stderr.
fn assert_decodes(path: &Path) {
    let tools = ffmpeg::tools().unwrap();
    let mut a = os(&["-hide_banner", "-nostdin", "-v", "error", "-i"]);
    a.push(ffmpeg::path_arg(path));
    a.extend(os(&["-f", "null", "-"]));
    let out = ffmpeg::capture(
        &tools.ffmpeg,
        &a,
        &CancelToken::new(),
        Duration::from_secs(600),
    )
    .unwrap();
    assert!(
        out.success && out.log.trim().is_empty(),
        "{}: {}",
        path.display(),
        out.log
    );
}

#[test]
fn mp4_to_mkv_remuxes() {
    if !have_ffmpeg() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let clip = make_clip(dir.path(), "Vídeo 報告.mp4", (320, 240, 2), H264_AAC);
    let started = Instant::now();
    let out = run(convert(VideoContainer::Mkv), &clip).unwrap();
    assert!(started.elapsed() < Duration::from_secs(10));
    assert_eq!(out.note.as_deref(), Some(COPIED_NOTE));
    assert_eq!(out.outputs[0].file_name().unwrap(), "Vídeo 報告.mkv");
    let info = probe(&out.outputs[0]);
    assert!(info.container.contains("matroska"));
    assert_eq!(codecs(&info), ("h264".into(), "aac".into()));
    assert_decodes(&out.outputs[0]);

    // Fast mode into the container it already is: nothing written.
    let out = run(convert(VideoContainer::Mp4), &clip).unwrap();
    assert!(out.outputs.is_empty(), "{out:?}");
    assert_eq!(out.note.as_deref(), Some("Already MP4"));
}

#[test]
fn reencode_keeps_every_audio_track() {
    if !have_ffmpeg() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let mut args = vec!["-map", "0:v", "-map", "1:a", "-map", "1:a"];
    args.extend(H264_AAC);
    let clip = make_clip(dir.path(), "two.mp4", (160, 120, 1), &args);
    let tracks = |p: &Path| probe(p).audio_streams().count();
    assert_eq!(tracks(&clip), 2);
    let reencode = |format| {
        Action::VideoConvert(VideoConvertOpts {
            format,
            fast: false,
            encoder: EncoderChoice::Cpu,
            ..Default::default()
        })
    };
    let out = run(reencode(VideoContainer::Mkv), &clip).unwrap();
    assert_eq!(tracks(&out.outputs[0]), 2);
    let out = run(reencode(VideoContainer::Avi), &clip).unwrap();
    assert_eq!(tracks(&out.outputs[0]), 1, "AVI keeps the first track");
}

#[test]
fn vp9_mkv_to_mp4_reencodes_and_webm_gets_vp9_opus() {
    if !have_ffmpeg() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let vp9 = &[
        "-c:v",
        "libvpx-vp9",
        "-deadline",
        "realtime",
        "-c:a",
        "libopus",
    ];
    let clip = make_clip(dir.path(), "clip.mkv", (320, 240, 2), vp9);
    let out = run(convert(VideoContainer::Mp4), &clip).unwrap();
    assert_eq!(out.note, None);
    let info = probe(&out.outputs[0]);
    assert_eq!(codecs(&info), ("h264".into(), "aac".into()));
    assert!((info.duration_s.unwrap() - 2.0).abs() < 0.3, "{info:?}");
    assert_decodes(&out.outputs[0]);

    let mp4 = make_clip(dir.path(), "h264.mp4", (320, 240, 2), H264_AAC);
    let out = run(convert(VideoContainer::Webm), &mp4).unwrap();
    assert_eq!(
        codecs(&probe(&out.outputs[0])),
        ("vp9".into(), "opus".into())
    );
    assert_decodes(&out.outputs[0]);

    // Same container → "(converted)"; Advanced codec that doesn't fit → plain error.
    let out = run(
        Action::VideoConvert(VideoConvertOpts {
            fast: false,
            video_codec: VideoCodec::H265,
            encoder: EncoderChoice::Cpu,
            ..Default::default()
        }),
        &mp4,
    )
    .unwrap();
    assert_eq!(out.outputs[0].file_name().unwrap(), "h264 (converted).mp4");
    let info = probe(&out.outputs[0]);
    assert_eq!(codecs(&info), ("hevc".into(), "aac".into()));
    let err = run(
        Action::VideoConvert(VideoConvertOpts {
            format: VideoContainer::Avi,
            video_codec: VideoCodec::H265,
            ..Default::default()
        }),
        &mp4,
    )
    .unwrap_err();
    assert_eq!(err.kind, ErrorKind::Unsupported);
    assert_eq!(err.message, "H.265 video isn't supported in AVI files.");
}

#[test]
fn mts_and_avi_inputs_work() {
    if !have_ffmpeg() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let mts = make_clip(dir.path(), "cam.mts", (320, 240, 2), H264_AAC);
    let out = run(convert(VideoContainer::Mp4), &mts).unwrap();
    assert_eq!(out.note.as_deref(), Some(COPIED_NOTE));
    assert_decodes(&out.outputs[0]);

    let avi_args = &["-c:v", "mpeg4", "-q:v", "5", "-c:a", "libmp3lame"];
    let avi = make_clip(dir.path(), "old.avi", (320, 240, 2), avi_args);
    let out = run(convert(VideoContainer::Mp4), &avi).unwrap();
    assert_eq!(
        codecs(&probe(&out.outputs[0])),
        ("mpeg4".into(), "mp3".into())
    );
    assert_decodes(&out.outputs[0]);
    let out = run(convert(VideoContainer::Webm), &avi).unwrap();
    assert_eq!(
        codecs(&probe(&out.outputs[0])),
        ("vp9".into(), "opus".into())
    );
    let out = run(convert(VideoContainer::Avi), &mts).unwrap();
    assert_decodes(&out.outputs[0]);
}

#[test]
fn broken_file_fails_plainly() {
    if !have_ffmpeg() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let broken = dir.path().join("broken.mp4");
    std::fs::write(&broken, b"not a video").unwrap();
    for action in [
        convert(VideoContainer::Mkv),
        compress(
            VideoCompressPreset::Discord,
            Resolution::Keep,
            EncoderChoice::Cpu,
        ),
    ] {
        let err = run(action, &broken).unwrap_err();
        assert_eq!(err.kind, ErrorKind::BadInput, "{err}");
        assert!(err.message.contains("damaged"), "{}", err.message);
    }
    let names = std::fs::read_dir(dir.path()).unwrap().count();
    assert_eq!(names, 1);
}

#[test]
fn resolution_caps_but_never_upscales() {
    if !have_ffmpeg() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    // High quality (big) so Balanced saves ≥ 5%.
    let big = &[
        "-c:v",
        "libx264",
        "-preset",
        "ultrafast",
        "-crf",
        "8",
        "-pix_fmt",
        "yuv420p",
        "-c:a",
        "aac",
    ];
    let fhd = make_clip(dir.path(), "fhd.mp4", (1920, 1080, 2), big);
    let out = run(
        compress(
            VideoCompressPreset::Balanced,
            Resolution::P720,
            EncoderChoice::Cpu,
        ),
        &fhd,
    )
    .unwrap();
    assert_eq!(out.outputs[0].file_name().unwrap(), "fhd (compressed).mp4");
    let v = probe(&out.outputs[0]);
    assert_eq!(
        (v.video().unwrap().width, v.video().unwrap().height),
        (Some(1280), Some(720))
    );
    assert_decodes(&out.outputs[0]);

    let sd = make_clip(dir.path(), "sd.mp4", (854, 480, 2), big);
    let out = run(
        compress(
            VideoCompressPreset::Smaller,
            Resolution::P720,
            EncoderChoice::Cpu,
        ),
        &sd,
    )
    .unwrap();
    let v = probe(&out.outputs[0]);
    assert_eq!(v.video().unwrap().height, Some(480));
    assert_eq!(v.video().unwrap().codec, "hevc");
}

#[test]
fn never_bigger_and_already_under_target() {
    if !have_ffmpeg() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    // Already tiny: Balanced can't save 5%.
    let small = &[
        "-c:v", "libx264", "-preset", "veryslow", "-crf", "40", "-pix_fmt", "yuv420p", "-c:a",
        "aac", "-b:a", "32k",
    ];
    let clip = make_clip(dir.path(), "small.mp4", (320, 240, 2), small);
    let out = run(
        compress(
            VideoCompressPreset::Balanced,
            Resolution::Keep,
            EncoderChoice::Cpu,
        ),
        &clip,
    )
    .unwrap();
    assert_eq!(
        (out.outputs.len(), out.note.as_deref()),
        (0, Some(WELL_COMPRESSED_NOTE))
    );
    let out = run(
        compress(
            VideoCompressPreset::Discord,
            Resolution::Keep,
            EncoderChoice::Cpu,
        ),
        &clip,
    )
    .unwrap();
    assert_eq!(
        (out.outputs.len(), out.note.as_deref()),
        (0, Some("Already under 10 MB"))
    );
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
}

#[test]
fn custom_size_fits_target_two_pass() {
    if !have_ffmpeg() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let big = &[
        "-c:v",
        "libx264",
        "-preset",
        "ultrafast",
        "-crf",
        "5",
        "-pix_fmt",
        "yuv420p",
        "-c:a",
        "aac",
    ];
    let clip = make_clip(dir.path(), "clip.mov", (1280, 720, 10), big);
    assert!(std::fs::metadata(&clip).unwrap().len() > 1_000_000);
    // target_mb = 1 (see `compress`).
    let out = run(
        compress(
            VideoCompressPreset::Custom,
            Resolution::Keep,
            EncoderChoice::Cpu,
        ),
        &clip,
    )
    .unwrap();
    let size = std::fs::metadata(&out.outputs[0]).unwrap().len();
    assert!(size <= 1_000_000, "{size}");
    assert!(size >= 700_000, "far under target: {size}");
    // 1 MB / 10 s ≈ 670 kbps video: 720p is too few bits → stepped down.
    assert_eq!(
        out.note.as_deref(),
        Some("Resolution lowered to 480p to fit 1 MB.")
    );
    let info = probe(&out.outputs[0]);
    assert!(info.container.contains("mov"));
    assert_eq!(info.video().unwrap().height, Some(480));
    assert!(info.has_audio());
    assert_decodes(&out.outputs[0]);
}

#[test]
fn cancel_mid_encode_leaves_nothing() {
    if !have_ffmpeg() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let clip = make_clip(dir.path(), "long.mp4", (1280, 720, 20), H264_AAC);
    let cancel = CancelToken::new();
    let canceller = cancel.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(1500));
        canceller.cancel();
    });
    let started = Instant::now();
    let action = Action::VideoConvert(VideoConvertOpts {
        fast: false,
        video_codec: VideoCodec::H265,
        encoder: EncoderChoice::Cpu,
        ..Default::default()
    });
    let err = run_with(action, &clip, &cancel).unwrap_err();
    assert!(err.is_cancelled(), "{err}");
    assert!(
        started.elapsed() < Duration::from_secs(8),
        "{:?}",
        started.elapsed()
    );
    assert_no_temp(dir.path());
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
}

#[test]
fn gpu_detection_is_cached() {
    if !have_ffmpeg() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let state = dir.path().join("state.toml");
    let cancel = CancelToken::new();
    let first = gpu::detect(Some(&state), &cancel);
    assert!(!first.from_cache);
    println!("working GPU encoders: {:?}", first.working);
    let started = Instant::now();
    let second = gpu::detect(Some(&state), &cancel);
    assert!(second.from_cache, "second call ran test encodes");
    assert!(started.elapsed() < Duration::from_millis(500));
    assert_eq!(first.working, second.working);
    let text = std::fs::read_to_string(&state).unwrap();
    assert!(text.contains("[gpu]"), "{text}");
}

#[test]
#[ignore = "needs a GPU encoder"]
fn gpu_compress_works() {
    if !have_ffmpeg() {
        return;
    }
    if gpu::detect(None, &CancelToken::new()).working.is_empty() {
        println!("SKIPPED: no GPU encoder");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let big = &[
        "-c:v",
        "libx264",
        "-preset",
        "ultrafast",
        "-crf",
        "5",
        "-pix_fmt",
        "yuv420p",
        "-c:a",
        "aac",
    ];
    let clip = make_clip(dir.path(), "clip.mp4", (1280, 720, 5), big);
    for preset in [
        VideoCompressPreset::Balanced,
        VideoCompressPreset::Smaller,
        VideoCompressPreset::Custom,
    ] {
        let out = run(
            compress(preset, Resolution::Keep, EncoderChoice::Auto),
            &clip,
        )
        .unwrap();
        assert_eq!(out.outputs.len(), 1, "{preset:?}: {:?}", out.note);
        assert_decodes(&out.outputs[0]);
        if preset == VideoCompressPreset::Custom {
            assert!(std::fs::metadata(&out.outputs[0]).unwrap().len() <= 1_000_000);
        }
        std::fs::remove_file(&out.outputs[0]).unwrap();
    }
}

/// SPEC §12 M6 acceptance: a ~200 MB 1080p clip → Discord (≤ 10 MB), plays.
#[test]
#[ignore = "perf: generates a 200 MB clip; run in release"]
fn discord_preset_200mb_clip() {
    if !have_ffmpeg() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let gen_args = &[
        "-c:v",
        "libx264",
        "-preset",
        "ultrafast",
        "-b:v",
        "25M",
        "-maxrate",
        "25M",
        "-bufsize",
        "50M",
        "-pix_fmt",
        "yuv420p",
        "-c:a",
        "aac",
        "-b:a",
        "192k",
    ];
    let clip = make_clip(dir.path(), "big.mp4", (1920, 1080, 60), gen_args);
    let in_size = std::fs::metadata(&clip).unwrap().len();
    println!("input: {in_size} bytes");
    assert!(in_size > 150_000_000, "{in_size}");
    let started = Instant::now();
    let out = run(
        compress(
            VideoCompressPreset::Discord,
            Resolution::Keep,
            EncoderChoice::Auto,
        ),
        &clip,
    )
    .unwrap();
    let elapsed = started.elapsed();
    let size = std::fs::metadata(&out.outputs[0]).unwrap().len();
    let info = probe(&out.outputs[0]);
    let encoder = gpu::detect(gpu::default_state_path().as_deref(), &CancelToken::new()).working;
    println!(
        "Discord: {size} bytes in {elapsed:?}, note {:?}, {}x{}, GPU encoders {encoder:?}",
        out.note,
        info.video().unwrap().width.unwrap(),
        info.video().unwrap().height.unwrap()
    );
    assert!(size <= 10_000_000, "{size}");
    assert!(info.has_audio());
    assert!(
        (info.duration_s.unwrap() - 60.0).abs() < 0.5,
        "{:?}",
        info.duration_s
    );
    assert_decodes(&out.outputs[0]);
}
