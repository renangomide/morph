// SPDX-License-Identifier: GPL-3.0-or-later
//! Video → GIF (SPEC §4.3). One ffmpeg run: `fps`, `scale` (never
//! upscale), then `palettegen` (stats_mode=diff) + `paletteuse`
//! (sierra2_4a dither, only changed rectangles redrawn) in one filtergraph.

use super::ffmpeg;
use super::options::{GIF_MAX_LENGTH_S, VideoToGifOpts};
use crate::action::Action;
use crate::converter::{Converter, TaskCtx, TaskOutput};
use crate::error::{ConvertError, ErrorKind};
use crate::job::Task;
use crate::naming::Suffix;
use std::ffi::OsString;

/// `video.to_gif`.
pub struct VideoToGif;

/// The `-vf` filtergraph for these options.
pub fn filtergraph(opts: &VideoToGifOpts) -> String {
    let fps = opts.fps.clamp(1, 50);
    let scale = if opts.width == 0 {
        String::new()
    } else {
        format!("scale='min({},iw)':-2:flags=lanczos,", opts.width)
    };
    format!(
        "fps={fps},{scale}split[a][b];[a]palettegen=stats_mode=diff[p];\
         [b][p]paletteuse=dither=sierra2_4a:diff_mode=rectangle"
    )
}

impl Converter for VideoToGif {
    fn run(&self, task: &Task, ctx: &TaskCtx<'_>) -> Result<TaskOutput, ConvertError> {
        let Action::VideoToGif(opts) = &task.action else {
            return Err(super::wrong_action("VideoToGif"));
        };
        let (input, info) = super::open(task, ctx)?;
        // The picture, not cover art (`0:v:0` can pick an attached picture).
        let video = info.video().ok_or_else(super::no_video_track)?;
        let start = f64::from(opts.start_s);
        let mut length = f64::from(opts.length_s.clamp(1, GIF_MAX_LENGTH_S));
        if let Some(d) = info.duration_s {
            if start >= d {
                return Err(ConvertError::new(
                    ErrorKind::BadInput,
                    "The start time is after the end of the video.",
                    format!("start {start} s, video {d:.2} s"),
                ));
            }
            length = length.min(d - start);
        }

        let out = super::output_for(task, input, "gif", Suffix::Converted)?;
        let mut args: Vec<OsString> = vec!["-ss".into(), start.to_string().into()];
        args.extend(["-t".into(), format!("{length:.3}").into(), "-i".into()]);
        args.push(ffmpeg::path_arg(input));
        args.extend(["-map".into(), format!("0:{}", video.index).into()]);
        for a in ["-an", "-sn", "-dn", "-vf"] {
            args.push(a.into());
        }
        args.push(filtergraph(opts).into());
        for a in ["-loop", "0", "-f", "gif"] {
            args.push(a.into());
        }
        args.push(ffmpeg::path_arg(out.temp_path()));
        ffmpeg::run(&args, Some(length), ctx, &mut |f| ctx.progress(f, None))?;
        Ok(TaskOutput::single(out.commit()?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filtergraph_keeps_or_caps_width() {
        let g = filtergraph(&VideoToGifOpts::default());
        assert!(g.starts_with("fps=15,scale='min(480,iw)':-2"), "{g}");
        assert!(g.contains("palettegen=stats_mode=diff"));
        let keep = filtergraph(&VideoToGifOpts {
            width: 0,
            ..Default::default()
        });
        assert!(keep.starts_with("fps=15,split"), "{keep}");
    }
}
