// SPDX-License-Identifier: GPL-3.0-or-later
//! `image.clean` action (SPEC §4.1): remove metadata (all / GPS only), auto-rotate
//! from EXIF, rotate 90/180/270, flip.
//!
//! When pixels don't change (no rotate/flip, and no orientation fix needed or the
//! orientation tag survives), only the metadata is rewritten, without re-encoding.
//! Otherwise the image is re-encoded upright (lossy formats at quality 95).

use crate::action::Action;
use crate::converter::{Converter, TaskCtx, TaskOutput};
use crate::error::ConvertError;
use crate::image::common::{
    ANIMATED_NOTE, extension_of, has_transparency, ico_sizes_for, join_notes,
    same_or_fallback_format, task_input, wrong_action,
};
use crate::image::io::{self, DecodeOptions, EncodeOptions, Metadata, OtherMetadata};
use crate::image::options::{ImageCleanOpts, OutputFormat, Rotation, StripMode};
use crate::job::Task;
use crate::naming::{self, Suffix};
use image::DynamicImage;

pub struct ImageClean;

/// Re-encode quality for lossy formats (minimizes loss).
pub const CLEAN_QUALITY: u8 = 95;

/// Metadata after stripping. GPS-only: if the EXIF or XMP can't be parsed, it is
/// dropped entirely, so location data never survives by accident.
fn cleaned_metadata(meta: &Metadata, strip: StripMode) -> Metadata {
    match strip {
        StripMode::None => meta.clone(),
        StripMode::All => Metadata {
            exif: None,
            icc: meta.icc.clone(),
            xmp: None,
        },
        StripMode::GpsOnly => Metadata {
            exif: meta.exif.as_deref().and_then(io::strip_gps),
            icc: meta.icc.clone(),
            xmp: meta.xmp.as_deref().and_then(io::strip_xmp_gps),
        },
    }
}

/// Metadata outside EXIF/ICC/XMP (comments, IPTC, PNG text…) in a lossless rewrite.
fn other_metadata(strip: StripMode) -> OtherMetadata {
    match strip {
        StripMode::None => OtherMetadata::Keep,
        StripMode::All => OtherMetadata::DropAll,
        StripMode::GpsOnly => OtherMetadata::DropGps,
    }
}

fn has_transform(opts: &ImageCleanOpts) -> bool {
    opts.rotate != Rotation::None || opts.flip_horizontal || opts.flip_vertical
}

/// Whether the file can keep its pixels as they are and only get new metadata:
/// no user transform, and either the pixels are already upright or the EXIF
/// orientation tag stays (it is kept whenever EXIF is kept).
fn can_rewrite_losslessly(opts: &ImageCleanOpts, orientation: u8, new_meta: &Metadata) -> bool {
    !has_transform(opts) && (orientation <= 1 || new_meta.exif.is_some())
}

/// Upright pixels, then the user's rotation, then flips.
fn transform(img: DynamicImage, orientation: u8, opts: &ImageCleanOpts) -> DynamicImage {
    let img = io::apply_orientation(img, orientation);
    let img = match opts.rotate {
        Rotation::None => img,
        Rotation::R90 => img.rotate90(),
        Rotation::R180 => img.rotate180(),
        Rotation::R270 => img.rotate270(),
    };
    let img = if opts.flip_horizontal {
        img.fliph()
    } else {
        img
    };
    if opts.flip_vertical { img.flipv() } else { img }
}

impl Converter for ImageClean {
    fn run(&self, task: &Task, ctx: &TaskCtx<'_>) -> Result<TaskOutput, ConvertError> {
        let Action::ImageClean(opts) = &task.action else {
            return Err(wrong_action("image.clean", task));
        };
        let input = task_input(task)?;
        ctx.check_cancel()?;

        let decoded = io::decode(input, &DecodeOptions::default())?;
        ctx.progress(0.3, None);
        ctx.check_cancel()?;

        let new_meta = cleaned_metadata(&decoded.meta, opts.strip);
        let ext = extension_of(input);
        let input_format = OutputFormat::from_extension(&ext);
        let folder = task.output.folder_for(input);
        let mut notes = Vec::new();
        if decoded.animated {
            notes.push(ANIMATED_NOTE.to_owned());
        }

        // Lossless path: only the metadata changes.
        if let Some(format) = input_format
            && can_rewrite_losslessly(opts, decoded.orientation, &new_meta)
        {
            let original = std::fs::read(input)?;
            ctx.check_cancel()?;
            let other = other_metadata(opts.strip);
            if let Some(bytes) = io::rewrite_metadata_with(&original, format, &new_meta, other)? {
                ctx.check_cancel()?;
                let name = naming::output_name(input, format.extension(), Suffix::Clean);
                let path = naming::write_output(&folder, &name, &bytes)?;
                ctx.progress(1.0, None);
                return Ok(TaskOutput {
                    outputs: vec![path],
                    note: join_notes(notes),
                });
            }
        }

        // Re-encode: pixels upright (encode resets any EXIF orientation tag to 1).
        let image = transform(decoded.image, decoded.orientation, opts);
        ctx.progress(0.6, None);
        ctx.check_cancel()?;

        let (format, fallback_note) = same_or_fallback_format(&ext, has_transparency(&image));
        notes.extend(fallback_note);
        let encode_opts = EncodeOptions {
            quality: if format.is_lossy() { CLEAN_QUALITY } else { 0 },
            ico_sizes: ico_sizes_for(image.width(), image.height()),
            ..EncodeOptions::default()
        };
        let bytes = io::encode(&image, format, &encode_opts, Some(&new_meta))?;
        ctx.progress(0.9, None);
        ctx.check_cancel()?;

        let name = naming::output_name(input, format.extension(), Suffix::Clean);
        let path = naming::write_output(&folder, &name, &bytes)?;
        ctx.progress(1.0, None);
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
    use image::{ImageFormat, Rgb, RgbImage};

    fn opts() -> ImageCleanOpts {
        ImageCleanOpts::default()
    }

    fn full_meta() -> Metadata {
        Metadata {
            exif: Some(exif_bytes(6, true)),
            icc: Some(vec![9]),
            xmp: Some(b"<x/>".to_vec()),
        }
    }

    #[test]
    fn strip_all_keeps_only_icc() {
        let m = cleaned_metadata(&full_meta(), StripMode::All);
        assert_eq!(
            m,
            Metadata {
                exif: None,
                icc: Some(vec![9]),
                xmp: None
            }
        );
        assert_eq!(cleaned_metadata(&full_meta(), StripMode::None), full_meta());
    }

    #[test]
    fn gps_only_keeps_xmp_and_icc() {
        let m = cleaned_metadata(&full_meta(), StripMode::GpsOnly);
        assert_eq!(m.icc, Some(vec![9]));
        assert_eq!(m.xmp, Some(b"<x/>".to_vec()));
    }

    #[test]
    fn lossless_rewrite_decision() {
        let with_exif = full_meta();
        let without_exif = cleaned_metadata(&full_meta(), StripMode::All);
        // Upright: always lossless without transforms.
        assert!(can_rewrite_losslessly(&opts(), 1, &without_exif));
        // Sideways and the tag survives: lossless.
        assert!(can_rewrite_losslessly(&opts(), 6, &with_exif));
        // Sideways and the tag is stripped: must re-encode upright.
        assert!(!can_rewrite_losslessly(&opts(), 6, &without_exif));
        // Any user transform: re-encode.
        let rotate = ImageCleanOpts {
            rotate: Rotation::R90,
            ..opts()
        };
        assert!(!can_rewrite_losslessly(&rotate, 1, &with_exif));
        let flip = ImageCleanOpts {
            flip_vertical: true,
            ..opts()
        };
        assert!(!can_rewrite_losslessly(&flip, 1, &with_exif));
    }

    /// 3×2 image with distinct corner colors.
    fn corners() -> DynamicImage {
        let mut img = RgbImage::new(3, 2);
        img.put_pixel(0, 0, Rgb([255, 0, 0]));
        img.put_pixel(2, 0, Rgb([0, 255, 0]));
        img.put_pixel(0, 1, Rgb([0, 0, 255]));
        DynamicImage::ImageRgb8(img)
    }

    #[test]
    fn transform_order_is_orientation_then_rotate_then_flip() {
        let o = ImageCleanOpts {
            rotate: Rotation::R90,
            flip_horizontal: true,
            ..opts()
        };
        // Orientation 3 (180°) + rotate 90° = 270°, then flip H.
        let expected = corners().rotate270().fliph();
        assert_eq!(transform(corners(), 3, &o), expected);
        let o = ImageCleanOpts {
            flip_vertical: true,
            ..opts()
        };
        assert_eq!(transform(corners(), 1, &o), corners().flipv());
    }

    fn clean(o: ImageCleanOpts) -> Action {
        Action::ImageClean(o)
    }

    #[test]
    fn rotates_png_and_names_output() {
        let dir = tempfile::tempdir().unwrap();
        let input = save(dir.path(), "pic.png", &corners());
        let out = run_one(
            clean(ImageCleanOpts {
                rotate: Rotation::R90,
                ..opts()
            }),
            &input,
        );
        assert_eq!(file_name(out.output()), "pic (clean).png");
        let (img, format) = open(out.output());
        assert_eq!(format, ImageFormat::Png);
        assert_eq!(img.to_rgb8(), corners().rotate90().to_rgb8());
    }

    #[test]
    fn flips_bmp_with_collision_and_unicode() {
        let dir = tempfile::tempdir().unwrap();
        let input = save(dir.path(), "Férias 夏.bmp", &corners());
        let action = clean(ImageCleanOpts {
            flip_horizontal: true,
            ..opts()
        });
        let first = run_one(action.clone(), &input);
        let second = run_one(action, &input);
        assert_eq!(file_name(first.output()), "Férias 夏 (clean).bmp");
        assert_eq!(file_name(second.output()), "Férias 夏 (clean) (1).bmp");
        let (img, _) = open(second.output());
        assert_eq!(img.to_rgb8(), corners().fliph().to_rgb8());
    }

    #[test]
    fn strip_all_on_jpg_writes_clean_copy() {
        let dir = tempfile::tempdir().unwrap();
        let input = save_jpeg(dir.path(), "cam.jpg", &noisy_rgb(32, 24), 90);
        let before = std::fs::read(&input).unwrap();
        let out = run_one(clean(opts()), &input);
        assert_eq!(file_name(out.output()), "cam (clean).jpg");
        let (img, format) = open(out.output());
        assert_eq!(format, ImageFormat::Jpeg);
        assert_eq!((img.width(), img.height()), (32, 24));
        assert_eq!(std::fs::read(&input).unwrap(), before, "input untouched");
    }

    #[test]
    fn strip_all_removes_exif_losslessly() {
        let dir = tempfile::tempdir().unwrap();
        let input = save_jpeg(dir.path(), "gps.jpg", &noisy_rgb(32, 24), 90);
        add_exif_to_jpeg(&input, exif_bytes(1, true));
        assert!(!exif_fields(&input).is_empty());
        let out = run_one(clean(opts()), &input);
        assert!(exif_fields(out.output()).is_empty(), "EXIF removed");
        // Lossless: the pixels are bit-identical to the input's.
        assert!(open(out.output()).0 == open(&input).0, "pixels changed");
    }

    #[test]
    fn gps_only_keeps_other_exif() {
        let dir = tempfile::tempdir().unwrap();
        let input = save_jpeg(dir.path(), "gps.jpg", &noisy_rgb(32, 24), 90);
        add_exif_to_jpeg(&input, exif_bytes(1, true));
        let out = run_one(
            clean(ImageCleanOpts {
                strip: StripMode::GpsOnly,
                ..opts()
            }),
            &input,
        );
        let fields = exif_fields(out.output());
        assert!(fields.iter().any(|(t, _)| *t == exif::Tag::Orientation));
        assert!(
            !fields.iter().any(|(t, _)| {
                *t == exif::Tag::GPSInfoIFDPointer || *t == exif::Tag::GPSLatitudeRef
            }),
            "GPS removed: {fields:?}"
        );
        assert!(
            open(out.output()).0 == open(&input).0,
            "lossless: pixels changed"
        );
    }

    const XMP_GPS: &[u8] = br#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><rdf:Description xmlns:exif="http://ns.adobe.com/exif/1.0/" xmlns:dc="http://purl.org/dc/elements/1.1/" exif:GPSLatitude="48,51.5N" exif:GPSLongitude="2,17.4E"><exif:GPSAltitude>35/1</exif:GPSAltitude><dc:creator>Me</dc:creator></rdf:Description></rdf:RDF></x:xmpmeta>"#;

    fn icc_rgb() -> Vec<u8> {
        let mut p = vec![0u8; 132];
        p[0..4].copy_from_slice(&132u32.to_be_bytes());
        p[16..20].copy_from_slice(b"RGB ");
        p[36..40].copy_from_slice(b"acsp");
        p
    }

    fn contains(hay: &[u8], needle: &[u8]) -> bool {
        hay.windows(needle.len()).any(|w| w == needle)
    }

    /// A JPEG with GPS in EXIF and XMP, IPTC (APP13), APP3/APP15, a comment, an
    /// Adobe APP14, an ICC profile, and an MPF index + extra image after EOI.
    fn loaded_jpeg(dir: &std::path::Path) -> std::path::PathBuf {
        use img_parts::jpeg::{Jpeg, JpegSegment, markers};
        use img_parts::{Bytes, ImageEXIF, ImageICC};
        let input = save_jpeg(dir, "loaded.jpg", &noisy_rgb(32, 24), 90);
        let bytes = std::fs::read(&input).unwrap();
        let mut jpeg = Jpeg::from_bytes(Bytes::from(bytes)).unwrap();
        jpeg.set_exif(Some(Bytes::from(exif_bytes(1, true))));
        jpeg.set_icc_profile(Some(Bytes::from(icc_rgb())));
        let seg = |m: u8, c: &[u8]| JpegSegment::new_with_contents(m, Bytes::from(c.to_vec()));
        let mut xmp = b"http://ns.adobe.com/xap/1.0/\0".to_vec();
        xmp.extend_from_slice(XMP_GPS);
        let extra = [
            seg(markers::APP1, &xmp),
            seg(markers::APP2, b"MPF\0II*\0junk"),
            seg(markers::APP3, b"Meta\0GPSthing"),
            seg(markers::APP13, b"Photoshop 3.0\08BIM\x04\x04 GPS city"),
            seg(markers::APP14, b"Adobe\0d\0\0\0\0\x01"),
            seg(0xEF, b"vendor"),
            seg(markers::COM, b"a comment"),
        ];
        let pos = jpeg
            .segments()
            .iter()
            .position(|s| s.marker() == markers::DQT)
            .unwrap();
        for (i, s) in extra.into_iter().enumerate() {
            jpeg.segments_mut().insert(pos + i, s);
        }
        let mut out = jpeg.encoder().bytes().to_vec();
        out.extend_from_slice(b"\xFF\xD8\xFF\xE1\0\x10Exif\0\0GPSGPSGP\xFF\xD9");
        std::fs::write(&input, out).unwrap();
        input
    }

    fn markers_of(path: &std::path::Path) -> Vec<(u8, Vec<u8>)> {
        let b = std::fs::read(path).unwrap();
        let jpeg = img_parts::jpeg::Jpeg::from_bytes(img_parts::Bytes::from(b)).unwrap();
        jpeg.segments()
            .iter()
            .map(|s| (s.marker(), s.contents().to_vec()))
            .collect()
    }

    #[test]
    fn gps_only_strips_xmp_iptc_and_extra_images() {
        let dir = tempfile::tempdir().unwrap();
        let input = loaded_jpeg(dir.path());
        let out = run_one(
            clean(ImageCleanOpts {
                strip: StripMode::GpsOnly,
                ..opts()
            }),
            &input,
        );
        let bytes = std::fs::read(out.output()).unwrap();
        assert!(bytes.ends_with(&[0xFF, 0xD9]), "data after EOI kept");
        assert!(!contains(&bytes, b"GPSGPS") && !contains(&bytes, b"48,51.5N"));
        let segs = markers_of(out.output());
        assert!(!segs.iter().any(|(m, _)| *m == 0xED), "APP13 kept");
        assert!(
            !segs
                .iter()
                .any(|(m, c)| *m == 0xE2 && c.starts_with(b"MPF"))
        );
        assert!(
            segs.iter().any(|(m, _)| *m == 0xFE),
            "comment is not location"
        );
        let d = io::decode(out.output(), &DecodeOptions::default()).unwrap();
        let xmp = String::from_utf8(d.meta.xmp.unwrap()).unwrap();
        assert!(
            !xmp.contains("GPS") && xmp.contains("<dc:creator>Me"),
            "{xmp}"
        );
        assert_eq!(d.meta.icc, Some(icc_rgb()));
        let fields = exif_fields(out.output());
        assert!(!fields.iter().any(|(t, _)| *t == exif::Tag::GPSLatitudeRef));
        assert!(open(out.output()).0 == open(&input).0, "lossless");
    }

    #[test]
    fn remove_all_keeps_only_jfif_icc_and_adobe() {
        let dir = tempfile::tempdir().unwrap();
        let input = loaded_jpeg(dir.path());
        let out = run_one(clean(opts()), &input);
        let bytes = std::fs::read(out.output()).unwrap();
        assert!(bytes.ends_with(&[0xFF, 0xD9]), "data after EOI kept");
        let meta: Vec<(u8, Vec<u8>)> = markers_of(out.output())
            .into_iter()
            .filter(|(m, _)| (0xE0..=0xEF).contains(m) || *m == 0xFE)
            .collect();
        let kinds: Vec<u8> = meta.iter().map(|(m, _)| *m).collect();
        assert_eq!(kinds, vec![0xE0, 0xE2, 0xEE], "{kinds:x?}");
        assert!(meta[0].1.starts_with(b"JFIF\0"));
        assert!(meta[1].1.starts_with(b"ICC_PROFILE\0"));
        assert!(open(out.output()).0 == open(&input).0, "lossless");
    }

    /// Motion photos / vendor trailers append data after EOI without an MPF index.
    #[test]
    fn trailer_without_mpf_is_cut_in_both_modes() {
        let dir = tempfile::tempdir().unwrap();
        let input = save_jpeg(dir.path(), "motion.jpg", &noisy_rgb(32, 24), 90);
        let mut bytes = std::fs::read(&input).unwrap();
        bytes.extend_from_slice(b"   ftypmp42 video with GPS");
        std::fs::write(&input, &bytes).unwrap();
        for strip in [StripMode::All, StripMode::GpsOnly] {
            let out = run_one(clean(ImageCleanOpts { strip, ..opts() }), &input);
            let b = std::fs::read(out.output()).unwrap();
            assert!(b.ends_with(&[0xFF, 0xD9]), "{strip:?}: trailer kept");
            assert!(!contains(&b, b"ftypmp42"), "{strip:?}");
        }
    }

    fn png_chunks(path: &std::path::Path) -> Vec<([u8; 4], Vec<u8>)> {
        let b = std::fs::read(path).unwrap();
        let png = img_parts::png::Png::from_bytes(img_parts::Bytes::from(b)).unwrap();
        png.chunks()
            .iter()
            .map(|c| (c.kind(), c.contents().to_vec()))
            .collect()
    }

    fn loaded_png(dir: &std::path::Path) -> std::path::PathBuf {
        use img_parts::Bytes;
        use img_parts::png::{Png, PngChunk};
        let input = save(dir, "loaded.png", &noisy_rgb(16, 16));
        let b = std::fs::read(&input).unwrap();
        let mut png = Png::from_bytes(Bytes::from(b)).unwrap();
        let mut itxt = b"XML:com.adobe.xmp\0\0\0\0\0".to_vec();
        itxt.extend_from_slice(XMP_GPS);
        let chunk = |k: &[u8; 4], c: &[u8]| PngChunk::new(*k, Bytes::from(c.to_vec()));
        let extra = [
            chunk(b"tEXt", b"Comment\0hello"),
            chunk(b"tEXt", b"Raw profile type exif\0\nexif\n 4\n4750530a\n"),
            chunk(b"zTXt", b"Raw profile type xmp\0\0x\x9c\x03\0\0\0\0\x01"),
            chunk(b"iTXt", &itxt),
            chunk(b"tIME", &[7, 228, 1, 1, 0, 0, 0]),
            chunk(b"exIf", b"II* GPSLEGACY"),
            chunk(b"caBX", b"c2pa manifest"),
            chunk(b"vpAg", b"unknown"),
        ];
        for (i, c) in extra.into_iter().enumerate() {
            png.chunks_mut().insert(1 + i, c);
        }
        std::fs::write(&input, png.encoder().bytes()).unwrap();
        input
    }

    #[test]
    fn png_text_chunks_by_strip_mode() {
        let dir = tempfile::tempdir().unwrap();
        let input = loaded_png(dir.path());
        let gps = run_one(
            clean(ImageCleanOpts {
                strip: StripMode::GpsOnly,
                ..opts()
            }),
            &input,
        );
        let chunks = png_chunks(gps.output());
        assert!(
            !chunks
                .iter()
                .any(|(_, c)| c.starts_with(b"Raw profile type"))
        );
        assert!(chunks.iter().any(|(_, c)| c.starts_with(b"Comment\0")));
        assert!(chunks.iter().any(|(k, _)| k == b"tIME"));
        assert!(!chunks.iter().any(|(k, _)| k == b"exIf" || k == b"caBX"));
        let xmp = chunks
            .iter()
            .find(|(_, c)| c.starts_with(b"XML:com.adobe.xmp"))
            .map(|(_, c)| c.clone())
            .unwrap();
        assert!(!contains(&xmp, b"GPS") && contains(&xmp, b"Me"));

        let all = run_one(clean(opts()), &input);
        let kinds: Vec<[u8; 4]> = png_chunks(all.output()).iter().map(|(k, _)| *k).collect();
        for k in [
            b"tEXt", b"zTXt", b"iTXt", b"tIME", b"exIf", b"caBX", b"vpAg",
        ] {
            assert!(!kinds.contains(k), "{} kept", String::from_utf8_lossy(k));
        }
        assert!(open(all.output()).0 == open(&input).0, "lossless");
    }

    #[test]
    fn stripping_orientation_reencodes_upright() {
        let dir = tempfile::tempdir().unwrap();
        let input = save_jpeg(dir.path(), "side.jpg", &noisy_rgb(40, 20), 90);
        add_exif_to_jpeg(&input, exif_bytes(6, true));
        let out = run_one(clean(opts()), &input);
        let (img, _) = open(out.output());
        assert_eq!((img.width(), img.height()), (20, 40), "pixels upright");
        assert!(exif_fields(out.output()).is_empty());
    }

    #[test]
    fn keeping_orientation_tag_stays_lossless() {
        let dir = tempfile::tempdir().unwrap();
        let input = save_jpeg(dir.path(), "side.jpg", &noisy_rgb(40, 20), 90);
        add_exif_to_jpeg(&input, exif_bytes(6, false));
        let out = run_one(
            clean(ImageCleanOpts {
                strip: StripMode::None,
                ..opts()
            }),
            &input,
        );
        // Pixels as stored (40×20) plus the orientation tag: viewers show it upright.
        let (img, _) = open(out.output());
        assert_eq!((img.width(), img.height()), (40, 20));
        let decoded = io::decode(out.output(), &DecodeOptions::default()).unwrap();
        assert_eq!(decoded.orientation, 6);
    }
}
