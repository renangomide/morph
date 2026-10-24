// SPDX-License-Identifier: GPL-3.0-or-later
//! `image.convert` action (SPEC §4.1): any input → the chosen output format.
//!
//! EXIF orientation is applied first; the ICC profile is always kept, EXIF/XMP
//! only with `keep_metadata`. Animated inputs: first frame only, with a note.

use crate::action::Action;
use crate::converter::{Converter, TaskCtx, TaskOutput};
use crate::error::ConvertError;
use crate::image::common::{
    ANIMATED_NOTE, check_webp_input, join_notes, metadata_to_keep, quality_for, task_input,
    wrong_action,
};
use crate::image::io::{self, DecodeOptions, EncodeOptions};
use crate::image::options::OutputFormat;
use crate::job::Task;
use crate::naming::{self, Suffix};

pub struct ImageConvert;

impl Converter for ImageConvert {
    fn run(&self, task: &Task, ctx: &TaskCtx<'_>) -> Result<TaskOutput, ConvertError> {
        let Action::ImageConvert(opts) = &task.action else {
            return Err(wrong_action("image.convert", task));
        };
        let input = task_input(task)?;
        ctx.check_cancel()?;

        let decode_opts = DecodeOptions {
            svg_width: opts.svg_width.max(1),
        };
        if opts.format == OutputFormat::Webp {
            check_webp_input(input, &decode_opts)?;
        }
        let decoded = io::decode(input, &decode_opts)?;
        ctx.progress(0.3, None);
        ctx.check_cancel()?;

        let meta = metadata_to_keep(&decoded.meta, opts.keep_metadata, opts.remove_gps);
        let animated = decoded.animated;
        let image = io::apply_orientation(decoded.image, decoded.orientation);
        ctx.check_cancel()?;

        let format = opts.format;
        let encode_opts = EncodeOptions {
            quality: quality_for(format, opts.quality),
            lossless: opts.lossless,
            background: opts.background,
            ico_sizes: opts.ico_sizes.clone(),
            ..EncodeOptions::default()
        };
        let bytes = io::encode(&image, format, &encode_opts, Some(&meta))?;
        ctx.progress(0.9, None);
        ctx.check_cancel()?;

        let name = naming::output_name(input, format.extension(), Suffix::Converted);
        let path = naming::write_output(&task.output.folder_for(input), &name, &bytes)?;
        ctx.progress(1.0, None);

        let mut notes = Vec::new();
        if animated {
            notes.push(ANIMATED_NOTE.to_owned());
        }
        Ok(TaskOutput {
            outputs: vec![path],
            note: join_notes(notes),
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::action::Action;
    use crate::image::common::test_util::*;
    use crate::image::options::{ImageConvertOpts, OutputFormat};
    use image::ImageFormat;

    fn convert_to(format: OutputFormat) -> Action {
        Action::ImageConvert(ImageConvertOpts {
            format,
            ..Default::default()
        })
    }

    #[test]
    fn png_to_jpg_flattens_on_white_and_renames() {
        let dir = tempfile::tempdir().unwrap();
        let input = save(dir.path(), "logo.png", &half_transparent(40, 20));
        let before = std::fs::read(&input).unwrap();

        let outcome = run_one(convert_to(OutputFormat::Jpg), &input);
        let out = outcome.output();
        assert_eq!(file_name(out), "logo.jpg");
        let (img, format) = open(out);
        assert_eq!(format, ImageFormat::Jpeg);
        assert_eq!((img.width(), img.height()), (40, 20));
        let px = img.to_rgb8().get_pixel(2, 10).0;
        assert!(
            px.iter().all(|&c| c > 240),
            "transparent → white, got {px:?}"
        );
        assert_eq!(std::fs::read(&input).unwrap(), before, "input untouched");
        assert!(outcome.note().is_none());
    }

    #[test]
    fn custom_background_is_used() {
        let dir = tempfile::tempdir().unwrap();
        let input = save(dir.path(), "a.png", &half_transparent(20, 20));
        let action = Action::ImageConvert(ImageConvertOpts {
            format: OutputFormat::Bmp,
            background: [0, 0, 255],
            ..Default::default()
        });
        let out = run_one(action, &input);
        let (img, format) = open(out.output());
        assert_eq!(format, ImageFormat::Bmp);
        assert_eq!(img.to_rgb8().get_pixel(1, 1).0, [0, 0, 255]);
    }

    #[test]
    fn same_format_gets_suffix_and_collisions_get_numbers() {
        let dir = tempfile::tempdir().unwrap();
        let input = save(dir.path(), "photo.png", &noisy_rgb(16, 16));
        let first = run_one(convert_to(OutputFormat::Png), &input);
        let second = run_one(convert_to(OutputFormat::Png), &input);
        assert_eq!(file_name(first.output()), "photo (converted).png");
        assert_eq!(file_name(second.output()), "photo (converted) (1).png");
    }

    #[test]
    fn existing_target_is_never_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let input = save(dir.path(), "pic.bmp", &noisy_rgb(10, 10));
        std::fs::write(dir.path().join("pic.png"), b"keep me").unwrap();
        let out = run_one(convert_to(OutputFormat::Png), &input);
        assert_eq!(file_name(out.output()), "pic (1).png");
        assert_eq!(
            std::fs::read(dir.path().join("pic.png")).unwrap(),
            b"keep me"
        );
    }

    #[test]
    fn unicode_names_work() {
        let dir = tempfile::tempdir().unwrap();
        let input = save(dir.path(), "Férias 夏.png", &noisy_rgb(12, 8));
        let out = run_one(convert_to(OutputFormat::Jpg), &input);
        assert_eq!(file_name(out.output()), "Férias 夏.jpg");
        let (img, _) = open(out.output());
        assert_eq!((img.width(), img.height()), (12, 8));
    }

    #[test]
    fn batch_of_several_files() {
        let dir = tempfile::tempdir().unwrap();
        let files = vec![
            save(dir.path(), "a.png", &noisy_rgb(8, 8)),
            save_jpeg(dir.path(), "b.jpg", &noisy_rgb(9, 7), 90),
            save(dir.path(), "c.bmp", &noisy_rgb(5, 5)),
        ];
        let outcomes = run(convert_to(OutputFormat::Tiff), &files);
        let names: Vec<String> = outcomes.iter().map(|o| file_name(o.output())).collect();
        assert_eq!(names, ["a.tiff", "b.tiff", "c.tiff"]);
        for o in &outcomes {
            assert_eq!(open(o.output()).1, ImageFormat::Tiff);
        }
    }

    #[test]
    fn damaged_input_fails_with_plain_message() {
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("broken.png");
        std::fs::write(&input, b"not an image").unwrap();
        let outcome = run_one(convert_to(OutputFormat::Jpg), &input);
        match outcome {
            Outcome::Failed { error, .. } => assert!(error.contains("damaged")),
            Outcome::Done { .. } => panic!("should fail"),
        }
        assert_eq!(
            std::fs::read_dir(dir.path()).unwrap().count(),
            1,
            "no output"
        );
    }

    #[test]
    fn png_to_ico() {
        let dir = tempfile::tempdir().unwrap();
        let input = save(dir.path(), "icon.png", &half_transparent(64, 32));
        let out = run_one(convert_to(OutputFormat::Ico), &input);
        assert_eq!(file_name(out.output()), "icon.ico");
        assert_eq!(open(out.output()).1, ImageFormat::Ico);
    }

    #[test]
    fn ico_output_has_requested_square_sizes() {
        let dir = tempfile::tempdir().unwrap();
        // Opaque RGB input, non-square: must be padded and stored as RGBA.
        let input = save(dir.path(), "icon.png", &noisy_rgb(64, 32));
        let action = Action::ImageConvert(ImageConvertOpts {
            format: OutputFormat::Ico,
            ico_sizes: vec![16, 32],
            ..Default::default()
        });
        let out = run_one(action, &input);
        let file = std::fs::File::open(out.output()).unwrap();
        let icon = ico::IconDir::read(file).unwrap();
        let mut sizes: Vec<(u32, u32)> = icon
            .entries()
            .iter()
            .map(|e| (e.width(), e.height()))
            .collect();
        sizes.sort();
        assert_eq!(sizes, [(16, 16), (32, 32)]);
        assert_eq!(open(out.output()).1, ImageFormat::Ico);
    }

    #[test]
    fn animated_gif_converts_first_frame_with_note() {
        use image::codecs::gif::GifEncoder;
        use image::{Frame, Rgba, RgbaImage};
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("anim.gif");
        {
            let file = std::fs::File::create(&input).unwrap();
            let mut enc = GifEncoder::new(file);
            let frames = [Rgba([255, 0, 0, 255]), Rgba([0, 255, 0, 255])]
                .map(|c| Frame::new(RgbaImage::from_pixel(10, 10, c)));
            enc.encode_frames(frames).unwrap();
        }
        let out = run_one(convert_to(OutputFormat::Png), &input);
        assert_eq!(out.note(), Some(crate::image::common::ANIMATED_NOTE));
        let (img, _) = open(out.output());
        assert_eq!(img.to_rgba8().get_pixel(0, 0).0, [255, 0, 0, 255]);
    }

    #[test]
    fn png_to_webp_and_avif() {
        let dir = tempfile::tempdir().unwrap();
        let input = save(dir.path(), "x.png", &half_transparent(30, 20));
        let webp = run_one(convert_to(OutputFormat::Webp), &input);
        let (img, format) = open(webp.output());
        assert_eq!(format, ImageFormat::WebP);
        assert_eq!((img.width(), img.height()), (30, 20));
        assert!(img.color().has_alpha(), "WebP keeps transparency");

        let lossless = Action::ImageConvert(ImageConvertOpts {
            format: OutputFormat::Webp,
            lossless: true,
            ..Default::default()
        });
        let out = run_one(lossless, &input);
        let (img, _) = open(out.output());
        assert!(
            img.to_rgba8() == half_transparent(30, 20).to_rgba8(),
            "lossless WebP changed pixels"
        );

        let avif = run_one(convert_to(OutputFormat::Avif), &input);
        assert_eq!(file_name(avif.output()), "x.avif");
        let decoded = crate::image::io::decode(avif.output(), &Default::default()).unwrap();
        assert_eq!((decoded.image.width(), decoded.image.height()), (30, 20));
    }

    #[test]
    fn too_wide_for_webp_fails_before_decoding() {
        let dir = tempfile::tempdir().unwrap();
        // Header only: decoding would fail as "damaged", so the WebP check came first.
        let input = dir.path().join("wide.png");
        std::fs::write(&input, crate::image::io::tests::png_header_only(20000, 10)).unwrap();
        match run_one(convert_to(OutputFormat::Webp), &input) {
            Outcome::Failed { error, .. } => {
                assert!(error.contains("too large for WebP"), "{error}")
            }
            Outcome::Done { .. } => panic!("should fail"),
        }
    }

    #[test]
    fn exif_orientation_is_applied() {
        let dir = tempfile::tempdir().unwrap();
        let input = save_jpeg(dir.path(), "side.jpg", &noisy_rgb(40, 20), 90);
        add_exif_to_jpeg(&input, exif_bytes(6, false));
        let out = run_one(convert_to(OutputFormat::Png), &input);
        let (img, _) = open(out.output());
        assert_eq!((img.width(), img.height()), (20, 40), "rotated upright");
    }

    #[test]
    fn metadata_dropped_when_not_kept() {
        let dir = tempfile::tempdir().unwrap();
        let input = save_jpeg(dir.path(), "m.jpg", &noisy_rgb(20, 20), 90);
        add_exif_to_jpeg(&input, exif_bytes(1, true));
        let keep = run_one(convert_to(OutputFormat::Jpg), &input);
        assert!(
            !exif_fields(keep.output()).is_empty(),
            "EXIF kept by default"
        );
        let drop = Action::ImageConvert(ImageConvertOpts {
            format: OutputFormat::Jpg,
            keep_metadata: false,
            ..Default::default()
        });
        let out = run_one(drop, &input);
        assert!(exif_fields(out.output()).is_empty());
    }

    #[test]
    fn remove_gps_keeps_other_metadata() {
        let dir = tempfile::tempdir().unwrap();
        let input = save_jpeg(dir.path(), "g.jpg", &noisy_rgb(20, 20), 90);
        add_exif_to_jpeg(&input, exif_bytes(1, true));
        let has_gps = |p: &std::path::Path| {
            exif_fields(p)
                .iter()
                .any(|(t, _)| *t == exif::Tag::GPSLatitudeRef)
        };
        assert!(has_gps(&input), "fixture has GPS");
        let action = Action::ImageConvert(ImageConvertOpts {
            format: OutputFormat::Webp,
            remove_gps: true,
            ..Default::default()
        });
        let out = run_one(action, &input);
        assert!(!has_gps(out.output()));
        assert!(!exif_fields(out.output()).is_empty(), "other EXIF kept");
    }
}
