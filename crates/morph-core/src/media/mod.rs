// SPDX-License-Identifier: GPL-3.0-or-later
//! Video and audio (SPEC §4.3, §4.4) through the bundled FFmpeg (`ffmpeg.exe`,
//! `ffprobe.exe`). Every task runs on the FFmpeg queue (one child at a time,
//! configurable 1–2; `scheduler::JobConfig::ffmpeg_jobs`).
//!
//! | File | What | Lane |
//! |---|---|---|
//! | `options.rs` | option structs (serde, defaults) | foundation |
//! | `ffmpeg.rs` | find the exes, run with progress/cancel/kill, error mapping, muxers | foundation |
//! | `probe.rs` | `probe(path)` → [`probe::MediaInfo`] | foundation |
//! | `encode.rs` | the video encode seam: `Encoder`, `EncodeSettings`, `encode()` | A (B uses it) |
//! | `gpu.rs` | GPU encoder detection + cache | A |
//! | `convert.rs`, `compress.rs` | Video → Convert, Compress presets | A |
//! | `best.rs` | Compress "Best (automatic)" | B |
//! | `audio.rs`, `extract_audio.rs`, `gif.rs` | Audio → Convert, Extract audio, Video → GIF | C |

pub mod audio;
pub mod best;
pub mod compress;
pub mod convert;
pub mod encode;
pub mod extract_audio;
pub mod ffmpeg;
pub mod gif;
pub mod gpu;
pub mod options;
pub mod probe;

use crate::converter::TaskCtx;
use crate::error::{ConvertError, ErrorKind};
use crate::job::Task;
use crate::naming::{self, OutputFile, Suffix};
use std::path::Path;

/// The task's input file, probed.
pub(crate) fn open<'t>(
    task: &'t Task,
    ctx: &TaskCtx<'_>,
) -> Result<(&'t Path, probe::MediaInfo), ConvertError> {
    let input = task
        .input()
        .ok_or_else(|| ConvertError::other("No file was given.", "no input"))?;
    Ok((input, probe::probe(input, ctx.cancel)?))
}

/// [`output_file`] for `input`'s output (`<name>.<ext>`, with `suffix` when
/// that would clash with the input) in the task's output folder.
pub(crate) fn output_for(
    task: &Task,
    input: &Path,
    ext: &str,
    suffix: Suffix,
) -> Result<OutputFile, ConvertError> {
    output_file(
        &task.output.folder_for(input),
        &naming::output_name(input, ext, suffix),
    )
}

/// Start an output that ffmpeg writes by path: a `.~morph-*.tmp` file in
/// `folder` that becomes `name` (or `name (1)`, …) on [`OutputFile::commit`];
/// dropped uncommitted, it is deleted. Give ffmpeg `-f <muxer>` +
/// `ffmpeg::path_arg(out.temp_path())` (the temp name has no media extension).
///
/// The temp file is *not* hidden: ffmpeg opens outputs with truncate, which
/// Windows refuses on a hidden file ("Permission denied").
pub fn output_file(folder: &Path, name: &str) -> Result<OutputFile, ConvertError> {
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::Storage::FileSystem::{FILE_ATTRIBUTE_NORMAL, SetFileAttributesW};
    let mut out = OutputFile::create(folder, name)?;
    out.close_handle();
    let wide: Vec<u16> = out
        .temp_path()
        .as_os_str()
        .encode_wide()
        .chain([0])
        .collect();
    // SAFETY: NUL-terminated path. On failure ffmpeg reports the open error.
    let _ =
        unsafe { SetFileAttributesW(windows::core::PCWSTR(wide.as_ptr()), FILE_ATTRIBUTE_NORMAL) };
    Ok(out)
}

pub use audio::AudioConvert;
pub use compress::VideoCompress;
pub use convert::VideoConvert;
pub use extract_audio::ExtractAudio;
pub use gif::VideoToGif;

/// The video has no sound track (Extract audio, SPEC §4.3).
pub fn no_audio_track() -> ConvertError {
    ConvertError::new(
        ErrorKind::BadInput,
        "This video has no sound track.",
        "no audio stream",
    )
}

/// The file has no picture (a video action on an audio-only file).
pub fn no_video_track() -> ConvertError {
    ConvertError::new(
        ErrorKind::BadInput,
        "This file has no video picture.",
        "no video stream",
    )
}

/// The input isn't a video/audio file FFmpeg can read.
pub fn damaged_media(details: impl ToString) -> ConvertError {
    ConvertError::new(
        ErrorKind::BadInput,
        "This file seems to be damaged or isn't a supported video or audio file.",
        details,
    )
}

/// A converter got an action that isn't its own (a routing bug).
pub(crate) fn wrong_action(converter: &str) -> ConvertError {
    ConvertError::other(
        "Something went wrong while converting this file.",
        format!("{converter}: wrong action"),
    )
}
