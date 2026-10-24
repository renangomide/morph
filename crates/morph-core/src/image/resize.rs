// SPDX-License-Identifier: GPL-3.0-or-later
//! `image.resize` action (SPEC §4.1): percent / max width / max height / exact W×H,
//! Lanczos3. Keeps the input format when it can be written (else JPG/PNG).

use crate::action::Action;
use crate::converter::{Converter, TaskCtx, TaskOutput};
use crate::error::ConvertError;
use crate::image::common::{
    ANIMATED_NOTE, extension_of, has_transparency, join_notes, metadata_to_keep, resize_lanczos,
    same_or_fallback_format, task_input, wrong_action,
};
use crate::image::io::{self, DecodeOptions, EncodeOptions};
use crate::image::options::{OutputFormat, ResizeMode};
use crate::job::Task;
use crate::naming::{self, Suffix};

pub struct ImageResize;

/// Note when the size doesn't change (no file is written).
pub const UNCHANGED_NOTE: &str = "Already the requested size; no new file was made.";

/// Largest side we compute (keeps the math far from overflow; allocation limits apply later).
const MAX_SIDE: f64 = 1_000_000.0;

fn side(v: f64) -> u32 {
    if v.is_finite() {
        v.round().clamp(1.0, MAX_SIDE) as u32
    } else {
        1
    }
}

/// Output dimensions for a source of `src` (width, height). Never returns a 0-pixel side.
///
/// - `Percent`: both sides scaled.
/// - `MaxWidth` / `MaxHeight`: that side set to the limit, aspect kept. With `no_enlarge`
///   smaller images are left as they are.
/// - `Exact`: with `keep_aspect`, the largest size that fits inside W×H; otherwise
///   stretched to W×H.
/// - `no_enlarge`: no side grows beyond the source. When stretching, a W×H that
///   would enlarge either side is scaled down uniformly until it fits (so the
///   requested shape is kept).
pub fn target_dimensions(
    src: (u32, u32),
    mode: &ResizeMode,
    keep_aspect: bool,
    no_enlarge: bool,
) -> (u32, u32) {
    let (sw, sh) = (f64::from(src.0.max(1)), f64::from(src.1.max(1)));
    let cap = |scale: f64| if no_enlarge { scale.min(1.0) } else { scale };
    let scaled = |scale: f64| (side(sw * scale), side(sh * scale));
    match *mode {
        ResizeMode::Percent { percent } => {
            let p = f64::from(percent);
            let scale = if p.is_finite() {
                p.max(0.0) / 100.0
            } else {
                1.0
            };
            scaled(cap(scale))
        }
        ResizeMode::MaxWidth { width } => scaled(cap(f64::from(width.max(1)) / sw)),
        ResizeMode::MaxHeight { height } => scaled(cap(f64::from(height.max(1)) / sh)),
        ResizeMode::Exact { width, height } => {
            let (w, h) = (f64::from(width.max(1)), f64::from(height.max(1)));
            if keep_aspect {
                scaled(cap((w / sw).min(h / sh)))
            } else if no_enlarge && (w > sw || h > sh) {
                // Keep the requested shape, scaled down until neither side grows.
                let s = (sw / w).min(sh / h);
                (side(w * s), side(h * s))
            } else {
                (side(w), side(h))
            }
        }
    }
}

impl Converter for ImageResize {
    fn run(&self, task: &Task, ctx: &TaskCtx<'_>) -> Result<TaskOutput, ConvertError> {
        let Action::ImageResize(opts) = &task.action else {
            return Err(wrong_action("image.resize", task));
        };
        let input = task_input(task)?;
        if let ResizeMode::Percent { percent } = opts.mode
            && !(percent.is_finite() && percent > 0.0)
        {
            return Err(ConvertError::unsupported(
                "The resize percentage must be more than 0.",
                format!("percent {percent}"),
            ));
        }
        ctx.check_cancel()?;

        let decoded = io::decode(input, &DecodeOptions::default())?;
        ctx.progress(0.25, None);
        ctx.check_cancel()?;

        let animated = decoded.animated;
        let mut notes = Vec::new();
        if animated {
            notes.push(ANIMATED_NOTE.to_owned());
        }
        // Upright size, without rotating the pixels yet.
        let (sw, sh) = (decoded.image.width(), decoded.image.height());
        let src = if (5..=8).contains(&decoded.orientation) {
            (sh, sw)
        } else {
            (sw, sh)
        };
        let (w, h) = target_dimensions(src, &opts.mode, opts.keep_aspect, opts.no_enlarge);
        if (w, h) == src {
            // Nothing to do: no file is written (like compress's "already optimized").
            ctx.progress(1.0, None);
            notes.insert(0, UNCHANGED_NOTE.to_owned());
            return Ok(TaskOutput {
                outputs: Vec::new(),
                note: join_notes(notes),
            });
        }

        let meta = metadata_to_keep(&decoded.meta, opts.keep_metadata, opts.remove_gps);
        let image = io::apply_orientation(decoded.image, decoded.orientation);
        ctx.check_cancel()?;
        let image = resize_lanczos(&image, w, h)?;
        ctx.progress(0.6, None);
        ctx.check_cancel()?;

        let (format, fallback_note) =
            same_or_fallback_format(&extension_of(input), has_transparency(&image));
        let encode_opts = EncodeOptions {
            quality: format.default_quality(),
            ico_sizes: if format == OutputFormat::Ico {
                vec![w.max(h).min(256)]
            } else {
                EncodeOptions::default().ico_sizes
            },
            ..EncodeOptions::default()
        };
        let bytes = io::encode(&image, format, &encode_opts, Some(&meta))?;
        ctx.progress(0.9, None);
        ctx.check_cancel()?;

        let name = naming::output_name(input, format.extension(), Suffix::Resized);
        let path = naming::write_output(&task.output.folder_for(input), &name, &bytes)?;
        ctx.progress(1.0, None);
        notes.extend(fallback_note);
        Ok(TaskOutput {
            outputs: vec![path],
            note: join_notes(notes),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::image::common::test_util::*;
    use crate::image::options::ImageResizeOpts;
    use image::ImageFormat;

    fn dims(src: (u32, u32), mode: ResizeMode, keep_aspect: bool, no_enlarge: bool) -> (u32, u32) {
        target_dimensions(src, &mode, keep_aspect, no_enlarge)
    }

    #[test]
    fn percent() {
        let m = |p| ResizeMode::Percent { percent: p };
        assert_eq!(dims((1000, 500), m(50.0), true, true), (500, 250));
        assert_eq!(dims((1000, 500), m(200.0), true, true), (1000, 500));
        assert_eq!(dims((1000, 500), m(200.0), true, false), (2000, 1000));
        assert_eq!(dims((3, 3), m(50.0), true, true), (2, 2)); // 1.5 rounds up
        assert_eq!(dims((1000, 10), m(1.0), true, true), (10, 1)); // never 0
        assert_eq!(dims((10, 10), m(0.0), true, true), (1, 1));
        assert_eq!(dims((10, 10), m(-5.0), true, true), (1, 1));
        assert_eq!(dims((10, 10), m(f32::NAN), true, false), (10, 10));
    }

    #[test]
    fn max_width_and_height() {
        let mw = |w| ResizeMode::MaxWidth { width: w };
        let mh = |h| ResizeMode::MaxHeight { height: h };
        assert_eq!(dims((4000, 3000), mw(1920), true, true), (1920, 1440));
        assert_eq!(dims((800, 600), mw(1920), true, true), (800, 600));
        assert_eq!(dims((800, 600), mw(1600), true, false), (1600, 1200));
        assert_eq!(dims((4000, 3000), mh(300), true, true), (400, 300));
        assert_eq!(dims((10000, 10), mh(1), true, true), (1000, 1));
        assert_eq!(dims((10000, 1), mw(10), true, true), (10, 1)); // height stays ≥ 1
        assert_eq!(dims((100, 100), mw(0), true, true), (1, 1));
    }

    #[test]
    fn exact_fit_and_stretch() {
        let ex = |w, h| ResizeMode::Exact {
            width: w,
            height: h,
        };
        // Fit inside, aspect kept.
        assert_eq!(dims((4000, 3000), ex(800, 800), true, true), (800, 600));
        assert_eq!(dims((3000, 4000), ex(800, 800), true, true), (600, 800));
        // Fit with no_enlarge: small image stays.
        assert_eq!(dims((400, 300), ex(800, 800), true, true), (400, 300));
        assert_eq!(dims((400, 300), ex(800, 800), true, false), (800, 600));
        // Stretch.
        assert_eq!(dims((4000, 3000), ex(800, 800), false, true), (800, 800));
        // no_enlarge: 800×200 would widen 400×300, so it shrinks uniformly to 400×100.
        assert_eq!(dims((400, 300), ex(800, 200), false, true), (400, 100));
        assert_eq!(dims((400, 300), ex(100, 600), false, true), (50, 300));
        assert_eq!(dims((400, 300), ex(200, 300), false, true), (200, 300));
        assert_eq!(dims((400, 300), ex(800, 200), false, false), (800, 200));
        assert_eq!(dims((400, 300), ex(0, 0), false, false), (1, 1));
    }

    #[test]
    fn zero_sized_source_never_divides_by_zero() {
        assert_eq!(
            dims((0, 0), ResizeMode::MaxWidth { width: 10 }, true, false),
            (10, 10)
        );
    }

    fn resize_action(mode: ResizeMode) -> Action {
        Action::ImageResize(ImageResizeOpts {
            mode,
            ..Default::default()
        })
    }

    #[test]
    fn resizes_png_by_percent_keeping_format() {
        let dir = tempfile::tempdir().unwrap();
        let input = save(dir.path(), "shot.png", &half_transparent(200, 100));
        let out = run_one(resize_action(ResizeMode::Percent { percent: 25.0 }), &input);
        assert_eq!(file_name(out.output()), "shot (resized).png");
        let (img, format) = open(out.output());
        assert_eq!(format, ImageFormat::Png);
        assert_eq!((img.width(), img.height()), (50, 25));
        assert!(img.color().has_alpha());
        assert!(out.note().is_none());
    }

    #[test]
    fn resizes_jpg_to_max_width() {
        let dir = tempfile::tempdir().unwrap();
        let input = save_jpeg(dir.path(), "photo.jpg", &noisy_rgb(300, 200), 90);
        let out = run_one(resize_action(ResizeMode::MaxWidth { width: 150 }), &input);
        assert_eq!(file_name(out.output()), "photo (resized).jpg");
        let (img, format) = open(out.output());
        assert_eq!(format, ImageFormat::Jpeg);
        assert_eq!((img.width(), img.height()), (150, 100));
    }

    #[test]
    fn exact_stretch_and_collision() {
        let dir = tempfile::tempdir().unwrap();
        let input = save(dir.path(), "a.bmp", &noisy_rgb(64, 64));
        let action = Action::ImageResize(ImageResizeOpts {
            mode: ResizeMode::Exact {
                width: 32,
                height: 10,
            },
            keep_aspect: false,
            ..Default::default()
        });
        let first = run_one(action.clone(), &input);
        let second = run_one(action, &input);
        assert_eq!(file_name(first.output()), "a (resized).bmp");
        assert_eq!(file_name(second.output()), "a (resized) (1).bmp");
        let (img, _) = open(second.output());
        assert_eq!((img.width(), img.height()), (32, 10));
    }

    #[test]
    fn no_change_writes_nothing_with_note() {
        let dir = tempfile::tempdir().unwrap();
        let input = save(dir.path(), "Férias 夏.png", &noisy_rgb(40, 30));
        let out = run_one(resize_action(ResizeMode::MaxWidth { width: 1000 }), &input);
        assert!(out.outputs().is_empty());
        assert_eq!(out.note(), Some(UNCHANGED_NOTE));
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn zero_or_negative_percent_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let input = save(dir.path(), "a.png", &noisy_rgb(10, 10));
        for p in [0.0, -5.0, f32::NAN] {
            match run_one(resize_action(ResizeMode::Percent { percent: p }), &input) {
                Outcome::Failed { error, .. } => assert!(error.contains("percentage"), "{error}"),
                Outcome::Done { .. } => panic!("percent {p} should fail"),
            }
        }
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn orientation_applied_before_resizing() {
        let dir = tempfile::tempdir().unwrap();
        let input = save_jpeg(dir.path(), "side.jpg", &noisy_rgb(400, 200), 90);
        add_exif_to_jpeg(&input, exif_bytes(6, false));
        // Upright it is 200×400, so max width 100 → 100×200.
        let out = run_one(resize_action(ResizeMode::MaxWidth { width: 100 }), &input);
        let (img, _) = open(out.output());
        assert_eq!((img.width(), img.height()), (100, 200));
    }
}
