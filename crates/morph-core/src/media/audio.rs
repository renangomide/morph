// SPDX-License-Identifier: GPL-3.0-or-later
//! Audio → Convert (SPEC §4.4). Also the shared audio encode used by
//! Video → Extract audio ([`super::extract_audio`]).
//!
//! | Target | Encoder | Rate | Cover art |
//! |---|---|---|---|
//! | MP3 | `libmp3lame` | CBR `-b:a`, or VBR V0/V2 `-q:a 0/2` | yes |
//! | M4A | `aac` (`ipod` muxer) | `-b:a` | yes |
//! | WAV | `pcm_s16le` (`pcm_s24le` for 24/32-bit PCM sources) | – | no |
//! | FLAC | `flac` | – | yes |
//! | OGG | `libvorbis` | `-b:a` | no |
//! | Opus | `libopus` (always 48 kHz) | `-b:a` | no |
//!
//! Tags: `-map_metadata 0` when `keep_tags`, else `-map_metadata -1`.

use super::ffmpeg;
use super::options::{AudioConvertOpts, AudioFormat, Channels, Mp3Mode};
use super::probe::{MediaInfo, StreamInfo};
use crate::action::Action;
use crate::converter::{Converter, TaskCtx, TaskOutput};
use crate::error::ConvertError;
use crate::job::Task;
use crate::naming::Suffix;
use std::ffi::OsString;
use std::path::Path;

/// Note on a lossy → lossless result (SPEC §4.4).
pub const LOSSLESS_NOTE: &str = "Converting to a lossless format won't improve quality.";

/// `audio.convert`.
pub struct AudioConvert;

impl Converter for AudioConvert {
    fn run(&self, task: &Task, ctx: &TaskCtx<'_>) -> Result<TaskOutput, ConvertError> {
        let Action::AudioConvert(opts) = &task.action else {
            return Err(super::wrong_action("AudioConvert"));
        };
        let (input, info) = super::open(task, ctx)?;
        let Some(stream) = info.audio() else {
            return Err(super::damaged_media("no audio stream"));
        };
        let out = convert_audio(input, &info, stream, opts, false, task, ctx)?;
        Ok(out)
    }
}

/// Whether a file is a lossy audio format, by extension (mp3/aac/m4a/ogg/opus/wma).
/// For the window's "won't improve quality" warning.
pub fn is_lossy_input(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| {
        ["mp3", "aac", "m4a", "ogg", "opus", "wma"]
            .iter()
            .any(|l| e.eq_ignore_ascii_case(l))
    })
}

/// Codecs that keep every sample (anything else counts as lossy).
fn is_lossless_codec(codec: &str) -> bool {
    codec.starts_with("pcm_")
        || matches!(
            codec,
            "flac" | "alac" | "wavpack" | "ape" | "tta" | "truehd" | "mlp" | "wmalossless"
        )
}

/// Source codec already is what `format` writes, so `-c:a copy` works.
fn copy_fits(codec: &str, format: AudioFormat) -> bool {
    match format {
        AudioFormat::Mp3 => codec == "mp3",
        AudioFormat::M4a => codec == "aac",
        AudioFormat::Wav => matches!(codec, "pcm_s16le" | "pcm_s24le"),
        AudioFormat::Flac => codec == "flac",
        AudioFormat::Ogg => codec == "vorbis",
        AudioFormat::Opus => codec == "opus",
    }
}

/// Probe-free 24-bit guess: PCM sources wider than 16 bits.
fn wants_24_bit(codec: &str) -> bool {
    codec.starts_with("pcm_") && ["24", "32", "f32", "f64"].iter().any(|b| codec.contains(b))
}

fn os(list: &[&str]) -> Vec<OsString> {
    list.iter().map(OsString::from).collect()
}

/// Encode (or, when `allow_copy` and nothing asks for a change, stream-copy) the
/// first audio track of `input` to `opts.format`, next to the input.
pub(crate) fn convert_audio(
    input: &Path,
    info: &MediaInfo,
    stream: &StreamInfo,
    opts: &AudioConvertOpts,
    allow_copy: bool,
    task: &Task,
    ctx: &TaskCtx<'_>,
) -> Result<TaskOutput, ConvertError> {
    let format = opts.format;
    let ext = format.extension();
    let rate = if format == AudioFormat::Opus {
        Some(48_000)
    } else {
        opts.sample_rate.hz()
    };
    let rate_kept = rate.is_none() || rate == stream.sample_rate;
    let channels_kept = opts.channels == Channels::Keep || stream.channels == Some(1);
    let copy = allow_copy && rate_kept && channels_kept && copy_fits(&stream.codec, format);

    let out = super::output_for(task, input, ext, Suffix::Converted)?;

    let mut args = os(&["-i"]);
    args.push(ffmpeg::path_arg(input));
    args.extend(os(&["-map", &format!("0:{}", stream.index)]));
    let cover = info.cover().filter(|_| opts.keep_tags).filter(|_| {
        matches!(
            format,
            AudioFormat::Mp3 | AudioFormat::M4a | AudioFormat::Flac
        )
    });
    match cover {
        Some(c) => args.extend(os(&[
            "-map",
            &format!("0:{}", c.index),
            "-c:v",
            "copy",
            "-disposition:v:0",
            "attached_pic",
        ])),
        None => args.extend(os(&["-vn"])),
    }
    args.extend(os(&["-sn", "-dn", "-map_metadata"]));
    args.push(if opts.keep_tags { "0" } else { "-1" }.into());
    args.extend(os(&["-map_chapters", "-1"]));

    if copy {
        args.extend(os(&["-c:a", "copy"]));
    } else {
        let bitrate = format
            .bitrates()
            .first()
            .zip(format.bitrates().last())
            .map(|(lo, hi)| opts.bitrate_kbps.clamp(*lo, *hi));
        let b = |args: &mut Vec<OsString>| {
            if let Some(k) = bitrate {
                args.extend(os(&["-b:a", &format!("{k}k")]));
            }
        };
        match format {
            AudioFormat::Mp3 => {
                args.extend(os(&["-c:a", "libmp3lame"]));
                match opts.mp3_mode {
                    Mp3Mode::Cbr => b(&mut args),
                    Mp3Mode::V0 => args.extend(os(&["-q:a", "0"])),
                    Mp3Mode::V2 => args.extend(os(&["-q:a", "2"])),
                }
            }
            AudioFormat::M4a => {
                args.extend(os(&["-c:a", "aac"]));
                b(&mut args);
            }
            AudioFormat::Wav => {
                let pcm = if wants_24_bit(&stream.codec) {
                    "pcm_s24le"
                } else {
                    "pcm_s16le"
                };
                args.extend(os(&["-c:a", pcm]));
            }
            AudioFormat::Flac => args.extend(os(&["-c:a", "flac"])),
            AudioFormat::Ogg => {
                args.extend(os(&["-c:a", "libvorbis"]));
                b(&mut args);
            }
            AudioFormat::Opus => {
                args.extend(os(&["-c:a", "libopus"]));
                b(&mut args);
            }
        }
        if let Some(hz) = rate {
            args.extend(os(&["-ar", &hz.to_string()]));
        }
        if opts.channels == Channels::Mono {
            args.extend(os(&["-ac", "1"]));
        }
    }
    if format == AudioFormat::Mp3 {
        // ID3v2.3: what Windows Explorer and most players read.
        args.extend(os(&["-id3v2_version", "3"]));
    }
    args.extend(os(&["-f", ffmpeg::muxer(ext).unwrap_or(ext)]));
    args.push(ffmpeg::path_arg(out.temp_path()));

    ffmpeg::run(&args, info.duration_s, ctx, &mut |f| ctx.progress(f, None))?;
    let mut result = TaskOutput::single(out.commit()?);
    if copy {
        result = result.with_note("Copied the original sound track (no quality loss).");
    } else if format.is_lossless() && !is_lossless_codec(&stream.codec) {
        result = result.with_note(LOSSLESS_NOTE);
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lossy_by_extension() {
        assert!(is_lossy_input(Path::new("a.MP3")));
        assert!(is_lossy_input(Path::new("c:\\x\\b.opus")));
        assert!(!is_lossy_input(Path::new("a.flac")));
        assert!(!is_lossy_input(Path::new("a")));
    }

    #[test]
    fn copy_and_lossless_tables() {
        assert!(copy_fits("aac", AudioFormat::M4a));
        assert!(!copy_fits("aac", AudioFormat::Mp3));
        assert!(copy_fits("opus", AudioFormat::Opus));
        assert!(is_lossless_codec("pcm_s16le") && is_lossless_codec("flac"));
        assert!(!is_lossless_codec("mp3"));
        assert!(wants_24_bit("pcm_s24le") && wants_24_bit("pcm_f32le"));
        assert!(!wants_24_bit("pcm_s16le") && !wants_24_bit("flac"));
    }
}
