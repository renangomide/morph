// SPDX-License-Identifier: GPL-3.0-or-later
//! Audio → Convert, Video → Extract audio, Video → GIF against the real
//! FFmpeg. Media is generated at test time with lavfi (`sine`, `anoisesrc`,
//! `testsrc2`, `color`). Prints `SKIPPED:` and passes when FFmpeg isn't there.

use morph_core::error::ErrorKind;
use morph_core::media::options::{
    AudioConvertOpts, AudioFormat, Channels, Mp3Mode, SampleRate, VideoToGifOpts,
};
use morph_core::media::{self, ffmpeg, probe};
use morph_core::{Action, CancelToken, ConvertError, Converter, OutputSpec, Task, TaskCtx, TaskId};
use morph_core::{TaskOutput, media::audio::is_lossy_input};
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

/// Write `folder/name` with ffmpeg `args` (inputs + codecs, without the output).
fn make(folder: &Path, name: &str, args: &[&str]) -> PathBuf {
    let cancel = CancelToken::new();
    let work = folder.join("gen-work");
    let ctx = TaskCtx::new(&cancel, &|_, _| {}, &work);
    let ext = Path::new(name).extension().unwrap().to_str().unwrap();
    let out = media::output_file(folder, name).unwrap();
    let mut a = os(args);
    a.extend(os(&["-f", ffmpeg::muxer(ext).unwrap()]));
    a.push(ffmpeg::path_arg(out.temp_path()));
    ffmpeg::run(&a, None, &ctx, &mut |_| {}).unwrap();
    let path = out.commit().unwrap();
    let _ = std::fs::remove_dir_all(&work);
    path
}

fn run_with(
    conv: &dyn Converter,
    action: Action,
    input: &Path,
    cancel: &CancelToken,
) -> Result<TaskOutput, ConvertError> {
    let work = tempfile::tempdir().unwrap();
    let ctx = TaskCtx::new(cancel, &|_, _| {}, work.path());
    let task = Task {
        id: TaskId(0),
        inputs: vec![input.to_path_buf()],
        action,
        output: OutputSpec::default(),
    };
    let before = std::fs::read(input).unwrap();
    let result = conv.run(&task, &ctx);
    assert_eq!(std::fs::read(input).unwrap(), before, "input changed");
    result
}

fn run(conv: &dyn Converter, action: Action, input: &Path) -> Result<TaskOutput, ConvertError> {
    run_with(conv, action, input, &CancelToken::new())
}

fn audio(format: AudioFormat) -> AudioConvertOpts {
    AudioConvertOpts {
        format,
        bitrate_kbps: format.default_bitrate(),
        ..Default::default()
    }
}

fn ffprobe(args: &[&str], file: &Path) -> String {
    let tools = ffmpeg::tools().unwrap();
    let mut a = os(&["-v", "error"]);
    a.extend(os(args));
    a.push(ffmpeg::path_arg(file));
    let out = ffmpeg::capture(
        &tools.ffprobe,
        &a,
        &CancelToken::new(),
        Duration::from_secs(60),
    )
    .unwrap();
    assert!(out.success, "{}", out.log);
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

/// `title` tag (format or first stream: Ogg keeps tags on the stream).
fn title(file: &Path) -> String {
    let t = ffprobe(
        &[
            "-show_entries",
            "format_tags=title",
            "-of",
            "default=nw=1:nk=1",
        ],
        file,
    );
    if !t.is_empty() {
        return t;
    }
    ffprobe(
        &[
            "-select_streams",
            "a:0",
            "-show_entries",
            "stream_tags=title",
            "-of",
            "default=nw=1:nk=1",
        ],
        file,
    )
}

/// Decodes with no errors at all.
fn assert_decodes(file: &Path) {
    let tools = ffmpeg::tools().unwrap();
    let mut a = os(&["-v", "error", "-i"]);
    a.push(ffmpeg::path_arg(file));
    a.extend(os(&["-f", "null", "-"]));
    let out = ffmpeg::capture(
        &tools.ffmpeg,
        &a,
        &CancelToken::new(),
        Duration::from_secs(120),
    )
    .unwrap();
    assert!(
        out.success && out.log.trim().is_empty(),
        "{file:?}: {}",
        out.log
    );
}

fn no_temp_files(dir: &Path) {
    let left: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with(".~morph-"))
        .collect();
    assert!(left.is_empty(), "temp files left: {left:?}");
}

fn expected_codec(f: AudioFormat) -> &'static str {
    match f {
        AudioFormat::Mp3 => "mp3",
        AudioFormat::M4a => "aac",
        AudioFormat::Wav => "pcm_s16le",
        AudioFormat::Flac => "flac",
        AudioFormat::Ogg => "vorbis",
        AudioFormat::Opus => "opus",
    }
}

#[test]
fn every_audio_format_from_wav() {
    if !have_ffmpeg() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let wav = make(
        dir.path(),
        "Música 報告.wav",
        &[
            "-f",
            "lavfi",
            "-i",
            "anoisesrc=d=4:c=pink:a=0.3:r=44100",
            "-ac",
            "2",
            "-c:a",
            "pcm_s16le",
            "-metadata",
            "title=Noise",
        ],
    );
    let cancel = CancelToken::new();
    for format in AudioFormat::ALL {
        let res = run(
            &media::AudioConvert,
            Action::AudioConvert(audio(format)),
            &wav,
        )
        .unwrap();
        let out = &res.outputs[0];
        let expected_name = if format == AudioFormat::Wav {
            "Música 報告 (converted).wav".to_owned()
        } else {
            format!("Música 報告.{}", format.extension())
        };
        assert_eq!(out.file_name().unwrap().to_str().unwrap(), expected_name);
        assert_eq!(res.note, None, "{format:?}: lossless source, no note");
        let info = probe::probe(out, &cancel).unwrap();
        let a = info.audio().unwrap();
        assert_eq!(a.codec, expected_codec(format), "{format:?}");
        let rate = if format == AudioFormat::Opus {
            48_000
        } else {
            44_100
        };
        assert_eq!(a.sample_rate, Some(rate), "{format:?}");
        assert_eq!(a.channels, Some(2));
        assert!(
            (info.duration_s.unwrap() - 4.0).abs() < 0.2,
            "{format:?} {info:?}"
        );
        if !format.is_lossless() {
            let want = f64::from(format.default_bitrate()) * 1000.0;
            let got = a.bit_rate.or(info.bit_rate).unwrap() as f64;
            // Opus is VBR and spends less on this (dual-mono) noise.
            let tolerance = if format == AudioFormat::Opus {
                0.4
            } else {
                0.10
            };
            assert!(
                (got - want).abs() / want < tolerance,
                "{format:?}: {got} vs {want}"
            );
        }
        assert_eq!(title(out), "Noise", "{format:?}");
        assert_decodes(out);
    }

    // Options: MP3 V2, 48 kHz, mono.
    let opts = AudioConvertOpts {
        mp3_mode: Mp3Mode::V2,
        sample_rate: SampleRate::Hz48000,
        channels: Channels::Mono,
        ..Default::default()
    };
    let res = run(&media::AudioConvert, Action::AudioConvert(opts), &wav).unwrap();
    assert!(res.outputs[0].ends_with("Música 報告 (1).mp3"), "{res:?}");
    let a = probe::probe(&res.outputs[0], &cancel).unwrap();
    let a = a.audio().unwrap();
    assert_eq!((a.sample_rate, a.channels), (Some(48_000), Some(1)));

    // Bitrate outside the list is clamped (MP3 max 320).
    let opts = AudioConvertOpts {
        bitrate_kbps: 999,
        ..Default::default()
    };
    let res = run(&media::AudioConvert, Action::AudioConvert(opts), &wav).unwrap();
    let a = probe::probe(&res.outputs[0], &cancel).unwrap();
    assert_eq!(a.audio().unwrap().bit_rate, Some(320_000));
    no_temp_files(dir.path());
}

#[test]
fn mp3_tags_and_cover_survive() {
    if !have_ffmpeg() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let mp3 = make(
        dir.path(),
        "song.mp3",
        &[
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:duration=3",
            "-f",
            "lavfi",
            "-i",
            "color=c=red:s=64x64:d=0.04",
            "-map",
            "0:a",
            "-map",
            "1:v",
            "-frames:v",
            "1",
            "-c:a",
            "libmp3lame",
            "-b:a",
            "192k",
            "-c:v",
            "mjpeg",
            "-disposition:v",
            "attached_pic",
            "-metadata",
            "title=Hello Ünïcode",
            "-id3v2_version",
            "3",
        ],
    );
    let cancel = CancelToken::new();
    assert!(probe::probe(&mp3, &cancel).unwrap().cover().is_some());
    assert!(is_lossy_input(&mp3));
    for format in AudioFormat::ALL {
        let res = run(
            &media::AudioConvert,
            Action::AudioConvert(audio(format)),
            &mp3,
        )
        .unwrap();
        let out = &res.outputs[0];
        let info = probe::probe(out, &cancel).unwrap();
        assert_eq!(info.audio().unwrap().codec, expected_codec(format));
        let wants_cover = matches!(
            format,
            AudioFormat::Mp3 | AudioFormat::M4a | AudioFormat::Flac
        );
        assert_eq!(info.cover().is_some(), wants_cover, "{format:?}: {info:?}");
        if !wants_cover {
            assert!(!info.has_video(), "{format:?}");
        }
        assert_eq!(title(out), "Hello Ünïcode", "{format:?}");
        let note = format
            .is_lossless()
            .then_some(media::audio::LOSSLESS_NOTE.to_owned());
        assert_eq!(res.note, note, "{format:?}");
        assert_decodes(out);
    }

    // keep_tags off: no title, no cover.
    let opts = AudioConvertOpts {
        format: AudioFormat::M4a,
        keep_tags: false,
        ..Default::default()
    };
    let res = run(&media::AudioConvert, Action::AudioConvert(opts), &mp3).unwrap();
    let info = probe::probe(&res.outputs[0], &cancel).unwrap();
    assert!(info.cover().is_none());
    assert_eq!(title(&res.outputs[0]), "");
}

#[test]
fn extract_audio_copies_or_encodes() {
    if !have_ffmpeg() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let mp4 = make(
        dir.path(),
        "Vídeo 報告.mp4",
        &[
            "-f",
            "lavfi",
            "-i",
            "testsrc2=d=3:s=320x240:r=25",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:duration=3",
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-pix_fmt",
            "yuv420p",
            "-c:a",
            "aac",
            "-b:a",
            "128k",
            "-ac",
            "2",
            "-shortest",
        ],
    );
    let cancel = CancelToken::new();
    let extract = |opts: AudioConvertOpts, input: &Path| {
        run(&media::ExtractAudio, Action::VideoExtractAudio(opts), input)
    };

    // AAC → M4A: stream copy (same codec and bitrate, fast).
    let started = Instant::now();
    let res = extract(audio(AudioFormat::M4a), &mp4).unwrap();
    println!("AAC → M4A copy: {:?}", started.elapsed());
    assert!(res.outputs[0].ends_with("Vídeo 報告.m4a"));
    assert_eq!(
        res.note.as_deref(),
        Some("Copied the original sound track (no quality loss).")
    );
    let info = probe::probe(&res.outputs[0], &cancel).unwrap();
    assert!(!info.has_video());
    let a = info.audio().unwrap();
    assert_eq!(a.codec, "aac");
    let src = probe::probe(&mp4, &cancel).unwrap();
    assert_eq!(a.bit_rate, src.audio().unwrap().bit_rate);
    assert_decodes(&res.outputs[0]);

    // Mono asks for a change → re-encode.
    let opts = AudioConvertOpts {
        channels: Channels::Mono,
        ..audio(AudioFormat::M4a)
    };
    let res = extract(opts, &mp4).unwrap();
    assert!(res.outputs[0].ends_with("Vídeo 報告 (1).m4a"));
    assert_eq!(res.note, None);
    let info = probe::probe(&res.outputs[0], &cancel).unwrap();
    assert_eq!(info.audio().unwrap().channels, Some(1));

    // Default: MP3 192 kbps; collisions get " (1)"; lossless target notes.
    let res = extract(AudioConvertOpts::default(), &mp4).unwrap();
    assert!(res.outputs[0].ends_with("Vídeo 報告.mp3"));
    let res2 = extract(AudioConvertOpts::default(), &mp4).unwrap();
    assert!(res2.outputs[0].ends_with("Vídeo 報告 (1).mp3"));
    let res = extract(audio(AudioFormat::Flac), &mp4).unwrap();
    assert_eq!(res.note.as_deref(), Some(media::audio::LOSSLESS_NOTE));

    // WebM (Opus) → MP3 re-encodes; → Opus copies.
    let webm = make(
        dir.path(),
        "clip.webm",
        &[
            "-f",
            "lavfi",
            "-i",
            "testsrc2=d=2:s=160x120:r=15",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:duration=2",
            "-c:v",
            "libvpx",
            "-deadline",
            "realtime",
            "-c:a",
            "libopus",
            "-shortest",
        ],
    );
    let res = extract(AudioConvertOpts::default(), &webm).unwrap();
    assert!(res.outputs[0].ends_with("clip.mp3"));
    assert_eq!(res.note, None);
    let info = probe::probe(&res.outputs[0], &cancel).unwrap();
    assert_eq!(info.audio().unwrap().codec, "mp3");
    assert_decodes(&res.outputs[0]);
    let res = extract(audio(AudioFormat::Opus), &webm).unwrap();
    assert!(res.note.is_some_and(|n| n.contains("Copied")));
    assert_decodes(&res.outputs[0]);

    // No sound track → plain error, nothing written.
    let silent = make(
        dir.path(),
        "silent.mp4",
        &[
            "-f",
            "lavfi",
            "-i",
            "testsrc2=d=1:s=160x120:r=15",
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-pix_fmt",
            "yuv420p",
        ],
    );
    let count = std::fs::read_dir(dir.path()).unwrap().count();
    let err = extract(AudioConvertOpts::default(), &silent).unwrap_err();
    assert_eq!(err.kind, ErrorKind::BadInput);
    assert_eq!(err.message, "This video has no sound track.");
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), count);

    // Broken file → plain error.
    let broken = dir.path().join("broken.mp4");
    std::fs::write(&broken, b"not a video").unwrap();
    let err = extract(AudioConvertOpts::default(), &broken).unwrap_err();
    assert_eq!(err.kind, ErrorKind::BadInput);
    assert!(err.message.contains("damaged"), "{}", err.message);
    let err = run(
        &media::AudioConvert,
        Action::AudioConvert(AudioConvertOpts::default()),
        &broken,
    )
    .unwrap_err();
    assert_eq!(err.kind, ErrorKind::BadInput);
    assert_eq!(std::fs::read(&broken).unwrap(), b"not a video");
    no_temp_files(dir.path());
}

fn gif(opts: VideoToGifOpts, input: &Path) -> Result<TaskOutput, ConvertError> {
    run(&media::VideoToGif, Action::VideoToGif(opts), input)
}

fn frames(file: &Path) -> u64 {
    ffprobe(
        &[
            "-count_frames",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=nb_read_frames",
            "-of",
            "default=nw=1:nk=1",
        ],
        file,
    )
    .parse()
    .unwrap()
}

#[test]
fn gif_from_1080p_clip() {
    if !have_ffmpeg() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let clip = make(
        dir.path(),
        "Vídeo 報告.mp4",
        &[
            "-f",
            "lavfi",
            "-i",
            "testsrc2=d=12:s=1920x1080:r=30",
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-pix_fmt",
            "yuv420p",
        ],
    );
    let cancel = CancelToken::new();

    // Default: 0:00 + 10 s, 480 px, 15 fps.
    let started = Instant::now();
    let res = gif(VideoToGifOpts::default(), &clip).unwrap();
    let elapsed = started.elapsed();
    let out = &res.outputs[0];
    assert!(out.ends_with("Vídeo 報告.gif"));
    let size = std::fs::metadata(out).unwrap().len();
    let info = probe::probe(out, &cancel).unwrap();
    let v = info.video().unwrap();
    assert_eq!((v.codec.as_str(), v.width), ("gif", Some(480)));
    assert_eq!(v.height, Some(270));
    let n = frames(out);
    println!("10 s 1080p → 480 px 15 fps GIF: {size} bytes, {n} frames, {elapsed:?}");
    assert!((145..=155).contains(&n), "{n} frames");
    assert!(size < 10_000_000, "{size}");
    assert!((info.duration_s.unwrap() - 10.0).abs() < 0.2, "{info:?}");
    assert_decodes(out);

    // Start + length honored; length clamped to the clip end.
    let opts = VideoToGifOpts {
        start_s: 3,
        length_s: 4,
        width: 320,
        fps: 10,
    };
    let res = gif(opts, &clip).unwrap();
    let info = probe::probe(&res.outputs[0], &cancel).unwrap();
    assert!((info.duration_s.unwrap() - 4.0).abs() < 0.2, "{info:?}");
    assert_eq!(info.video().unwrap().width, Some(320));
    let opts = VideoToGifOpts {
        start_s: 10,
        length_s: 60,
        ..Default::default()
    };
    let res = gif(opts, &clip).unwrap();
    let info = probe::probe(&res.outputs[0], &cancel).unwrap();
    assert!((info.duration_s.unwrap() - 2.0).abs() < 0.2, "{info:?}");

    // Start after the end → plain error.
    let opts = VideoToGifOpts {
        start_s: 12,
        ..Default::default()
    };
    let err = gif(opts, &clip).unwrap_err();
    assert_eq!(err.message, "The start time is after the end of the video.");
    no_temp_files(dir.path());
}

#[test]
fn gif_never_upscales_and_rejects_bad_inputs() {
    if !have_ffmpeg() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let clip = make(
        dir.path(),
        "small.mp4",
        &[
            "-f",
            "lavfi",
            "-i",
            "testsrc2=d=2:s=320x240:r=25",
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-pix_fmt",
            "yuv420p",
        ],
    );
    let cancel = CancelToken::new();
    for width in [0, 640] {
        let opts = VideoToGifOpts {
            width,
            ..Default::default()
        };
        let res = gif(opts, &clip).unwrap();
        let info = probe::probe(&res.outputs[0], &cancel).unwrap();
        assert_eq!(info.video().unwrap().width, Some(320), "width {width}");
    }

    let wav = make(
        dir.path(),
        "tone.wav",
        &["-f", "lavfi", "-i", "sine=duration=1"],
    );
    let err = gif(VideoToGifOpts::default(), &wav).unwrap_err();
    assert_eq!(err.message, "This file has no video picture.");
    let broken = dir.path().join("broken.mp4");
    std::fs::write(&broken, b"not a video").unwrap();
    let err = gif(VideoToGifOpts::default(), &broken).unwrap_err();
    assert_eq!(err.kind, ErrorKind::BadInput);
}

#[test]
fn cancel_leaves_nothing_behind() {
    if !have_ffmpeg() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let clip = make(
        dir.path(),
        "long.mp4",
        &[
            "-f",
            "lavfi",
            "-i",
            "testsrc2=d=60:s=1280x720:r=30",
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-pix_fmt",
            "yuv420p",
        ],
    );
    let cancel = CancelToken::new();
    let canceller = cancel.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(700));
        canceller.cancel();
    });
    let opts = VideoToGifOpts {
        length_s: 60,
        width: 0,
        fps: 24,
        ..Default::default()
    };
    let started = Instant::now();
    let err = run_with(&media::VideoToGif, Action::VideoToGif(opts), &clip, &cancel).unwrap_err();
    assert!(err.is_cancelled(), "{err}");
    assert!(started.elapsed() < Duration::from_secs(10));
    let names: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .flatten()
        .map(|e| e.file_name())
        .collect();
    assert_eq!(names, vec![OsString::from("long.mp4")]);
}
