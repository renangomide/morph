// SPDX-License-Identifier: GPL-3.0-or-later
//! Merge / "Combine into one PDF" (SPEC §4.2): every task input (PDFs and
//! images), in `task.inputs` order, into one `Merged.pdf` in the first input's
//! folder. Images become pages (Fit to image / A4 / Letter, margin). Optional
//! bookmark per source file (PDFium can't write outlines: add them with lopdf).
//!
//! How images are embedded (quality first, then size):
//! - **JPEG** (1 or 3 colour channels): the file's bytes go in as they are
//!   (DCTDecode), no re-encoding. EXIF orientation is applied with the
//!   placement matrix, so the pixels stay untouched.
//! - **Lossless sources** (PNG, BMP, TIFF, GIF, ICO, TGA, QOI, SVG, lossless
//!   WebP): decoded, turned upright, embedded losslessly (PDFium Flate);
//!   transparency is kept (soft mask).
//! - **Lossy sources** (HEIC/HEIF, AVIF, lossy WebP, CMYK JPEG): decoded,
//!   turned upright, flattened on white, embedded as a high-quality JPEG
//!   (mozjpeg q92, 4:4:4). These are already lossy; a lossless copy would be
//!   many times bigger for no visible gain.
//!
//! Page geometry: *Fit to image* = the image at 96 DPI plus the margin on each
//! side (capped at the 200-inch PDF page limit). A4 / Letter: portrait, or
//! landscape when the image is wider than tall; the image is scaled (up or
//! down) to fit inside the margins and centred. Margins on A4/Letter are capped
//! at a quarter of the short side. PDF inputs: every page copied as is.
//!
//! Nothing is written until every input is in: a password-protected or damaged
//! input fails the whole task with a message naming the file, and no
//! `Merged.pdf` is left behind. Cancel is checked between inputs.

use super::options::{PageSize, PdfMergeOpts};
use super::pdfium;
use crate::action::Action;
use crate::converter::{Converter, TaskCtx, TaskOutput};
use crate::error::ConvertError;
use crate::image::io::{self, DecodeOptions, EncodeOptions};
use crate::image::options::OutputFormat;
use crate::job::Task;
use crate::naming::{MERGED_FILE_NAME, OutputFile};
use image::{DynamicImage, GenericImageView};
use morph_formats::Category;
use pdfium_render::prelude::*;
use std::io::Cursor;
use std::path::Path;

pub struct PdfMerge;

/// Pixels per inch for *Fit to image* pages.
const FIT_DPI: f32 = 96.0;
/// Largest page side most viewers accept (200 inches).
const MAX_PAGE_POINTS: f32 = 14_400.0;
/// JPEG quality for lossy sources that must be re-encoded.
const REENCODE_QUALITY: u8 = 92;

impl Converter for PdfMerge {
    fn run(&self, task: &Task, ctx: &TaskCtx<'_>) -> Result<TaskOutput, ConvertError> {
        let Action::PdfMerge(opts) = &task.action else {
            return Err(super::wrong_action("PdfMerge"));
        };
        let first = task
            .input()
            .ok_or_else(|| ConvertError::other("No files to combine.", "task has no input"))?;
        let total = task.inputs.len() + 1;
        let mut doc = pdfium::new_document()?;
        // (bookmark title, 0-based first page) per source that added pages.
        let mut starts: Vec<(String, u32)> = Vec::new();
        for (i, input) in task.inputs.iter().enumerate() {
            ctx.check_cancel()?;
            let before = pdfium::page_count(&doc);
            add_input(&mut doc, input, opts).map_err(|e| about(input, e))?;
            if pdfium::page_count(&doc) > before {
                starts.push((stem(input), before));
            }
            ctx.progress((i + 1) as f32 / total as f32, None);
        }
        ctx.check_cancel()?;
        if pdfium::page_count(&doc) == 0 {
            return Err(ConvertError::unsupported(
                "There's nothing to combine: the files have no pages.",
                "merged document has 0 pages",
            ));
        }
        let bytes = doc
            .save_to_bytes()
            .map_err(|e| pdfium::map_error(e, "save merged document"))?;
        drop(doc);
        let (bytes, note) = if opts.bookmarks {
            match add_bookmarks(&bytes, &starts) {
                Ok(with) => (with, None),
                // The pages are fine; don't lose the merge over its outline.
                Err(_) => (bytes, Some("Bookmarks couldn't be added.")),
            }
        } else {
            (bytes, None)
        };
        ctx.check_cancel()?;
        let mut out = OutputFile::create(&task.output.folder_for(first), MERGED_FILE_NAME)?;
        out.write_all(&bytes)?;
        let path = out.commit()?;
        ctx.progress(1.0, None);
        let output = TaskOutput::single(path);
        Ok(match note {
            Some(n) => output.with_note(n),
            None => output,
        })
    }
}

fn stem(path: &Path) -> String {
    path.file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Name the input in an error: `Couldn't add "a.pdf": This PDF is password-protected.`
fn about(input: &Path, mut e: ConvertError) -> ConvertError {
    if e.is_cancelled() {
        return e;
    }
    let name = input
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    e.message = format!("Couldn't add \"{name}\": {}", e.message);
    e.details = format!("{}: {}", input.display(), e.details);
    e
}

fn add_input(
    doc: &mut PdfDocument<'static>,
    input: &Path,
    opts: &PdfMergeOpts,
) -> Result<(), ConvertError> {
    match Category::from_path(input) {
        Some(Category::Pdf) => {
            let src = pdfium::open(input)?;
            if pdfium::page_count(&src) > 0 {
                doc.pages_mut()
                    .append(&src)
                    .map_err(|e| pdfium::map_error(e, "copy pages"))?;
            }
            Ok(())
        }
        Some(Category::Image) => add_image_page(doc, input, opts.page_size, opts.margin_mm),
        _ => Err(ConvertError::unsupported(
            "Only PDFs and images can be combined.",
            "not a PDF or image",
        )),
    }
}

/// How an image goes into the PDF.
enum Embed {
    /// JPEG bytes as they are; `orientation` applied by the placement matrix.
    Jpeg { bytes: Vec<u8>, orientation: u8 },
    /// Upright pixels, embedded losslessly (alpha kept).
    Lossless(DynamicImage),
}

/// Add the image file `input` as a new last page of `doc` (embedding rules and
/// page geometry in the module docs). Also used by Image → PDF
/// (`crate::image::to_pdf`), one document per image.
pub(crate) fn add_image_page(
    doc: &mut PdfDocument<'static>,
    input: &Path,
    page_size: PageSize,
    margin_mm: u32,
) -> Result<(), ConvertError> {
    let data = std::fs::read(input)?;
    let decoded = io::decode_bytes(&data, Some(input), &DecodeOptions::default())?;
    let (stored_w, stored_h) = decoded.image.dimensions();
    let embed = if matches!(jpeg_components(&data), Some(1 | 3)) {
        Embed::Jpeg {
            bytes: data,
            orientation: decoded.orientation,
        }
    } else {
        let upright = io::apply_orientation(decoded.image, decoded.orientation);
        if is_lossy_source(input, &data) {
            let rgb = io::flatten(&upright, [255, 255, 255]);
            let jpeg = io::encode(
                &DynamicImage::ImageRgb8(rgb),
                OutputFormat::Jpg,
                &EncodeOptions {
                    quality: REENCODE_QUALITY,
                    chroma_444: true,
                    ..EncodeOptions::default()
                },
                None,
            )?;
            Embed::Jpeg {
                bytes: jpeg,
                orientation: 1,
            }
        } else {
            Embed::Lossless(upright)
        }
    };
    let orientation = match &embed {
        Embed::Jpeg { orientation, .. } => *orientation,
        Embed::Lossless(_) => 1,
    };
    // Displayed size: orientations 5-8 swap width and height.
    let (shown_w, shown_h) = match &embed {
        Embed::Lossless(img) => img.dimensions(),
        Embed::Jpeg { .. } if orientation >= 5 => (stored_h, stored_w),
        Embed::Jpeg { .. } => (stored_w, stored_h),
    };
    let layout = layout(page_size, margin_mm, shown_w, shown_h);

    let engine = |e| pdfium::map_error(e, "add image");
    let mut object = match &embed {
        Embed::Jpeg { bytes, .. } => {
            PdfPageImageObject::new_from_jpeg_reader(doc, Cursor::new(bytes.as_slice()))
        }
        Embed::Lossless(img) => PdfPageImageObject::new(doc, img),
    }
    .map_err(engine)?;
    let [a, b, c, d, e, f] = placement(orientation, &layout);
    object.transform(a, b, c, d, e, f).map_err(engine)?;
    let mut page = doc
        .pages_mut()
        .create_page_at_end(PdfPagePaperSize::Custom(
            PdfPoints::new(layout.page_w),
            PdfPoints::new(layout.page_h),
        ))
        .map_err(engine)?;
    page.objects_mut()
        .add_image_object(object)
        .map_err(engine)?;
    Ok(())
}

/// Number of colour components in a JPEG's frame header, or `None` if `bytes`
/// isn't a readable baseline/progressive JPEG.
fn jpeg_components(bytes: &[u8]) -> Option<u8> {
    if bytes.get(0..2)? != [0xFF, 0xD8] {
        return None;
    }
    let mut i = 2;
    loop {
        while *bytes.get(i)? == 0xFF && *bytes.get(i + 1)? == 0xFF {
            i += 1;
        }
        if *bytes.get(i)? != 0xFF {
            return None;
        }
        let marker = *bytes.get(i + 1)?;
        let len = usize::from(u16::from_be_bytes([*bytes.get(i + 2)?, *bytes.get(i + 3)?]));
        match marker {
            // Baseline, extended sequential, progressive (Huffman, 8-bit only:
            // PDF readers handle these).
            0xC0..=0xC2 => {
                let precision = *bytes.get(i + 4)?;
                return (precision == 8).then_some(*bytes.get(i + 9)?);
            }
            // Lossless, hierarchical, arithmetic: not safe to pass through.
            0xC3 | 0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF => return None,
            0xDA => return None,
            _ if len < 2 => return None,
            _ => i += 2 + len,
        }
    }
}

/// The source format is lossy (re-encode as high-quality JPEG rather than
/// storing its decoded pixels losslessly).
fn is_lossy_source(path: &Path, data: &[u8]) -> bool {
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "heic" | "heif" | "avif" | "jpg" | "jpeg" | "jfif" => true,
        "webp" => !webp_is_lossless(data),
        _ => false,
    }
}

/// A WebP whose image data is VP8L (lossless).
fn webp_is_lossless(data: &[u8]) -> bool {
    match data.get(12..16) {
        Some(b"VP8L") => true,
        Some(b"VP8X") => {
            let has = |tag: &[u8]| data.windows(4).any(|w| w == tag);
            has(b"VP8L") && !has(b"VP8 ")
        }
        _ => false,
    }
}

/// Page size and image rectangle, in points (origin bottom-left).
#[derive(Debug, Clone, Copy, PartialEq)]
struct Layout {
    page_w: f32,
    page_h: f32,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
}

fn layout(size: PageSize, margin_mm: u32, img_w: u32, img_h: u32) -> Layout {
    let (img_w, img_h) = (img_w.max(1) as f32, img_h.max(1) as f32);
    let margin = margin_mm as f32 * 72.0 / 25.4;
    match size.points() {
        None => {
            let margin = margin.min(MAX_PAGE_POINTS / 4.0);
            let (mut w, mut h) = (img_w * 72.0 / FIT_DPI, img_h * 72.0 / FIT_DPI);
            let room = MAX_PAGE_POINTS - 2.0 * margin;
            let shrink = (room / w.max(h)).min(1.0);
            w *= shrink;
            h *= shrink;
            Layout {
                page_w: w + 2.0 * margin,
                page_h: h + 2.0 * margin,
                x: margin,
                y: margin,
                w,
                h,
            }
        }
        Some((pw, ph)) => {
            let (page_w, page_h) = if img_w > img_h { (ph, pw) } else { (pw, ph) };
            let margin = margin.min(page_w.min(page_h) / 4.0);
            let (room_w, room_h) = (page_w - 2.0 * margin, page_h - 2.0 * margin);
            let scale = (room_w / img_w).min(room_h / img_h);
            let (w, h) = (img_w * scale, img_h * scale);
            Layout {
                page_w,
                page_h,
                x: (page_w - w) / 2.0,
                y: (page_h - h) / 2.0,
                w,
                h,
            }
        }
    }
}

/// Matrix `[a b c d e f]` that maps the image's unit square onto the layout
/// rectangle, turning stored pixels upright for EXIF `orientation` (1-8).
fn placement(orientation: u8, l: &Layout) -> [f32; 6] {
    let (w, h, x, y) = (l.w, l.h, l.x, l.y);
    match orientation {
        2 => [-w, 0.0, 0.0, h, x + w, y],
        3 => [-w, 0.0, 0.0, -h, x + w, y + h],
        4 => [w, 0.0, 0.0, -h, x, y + h],
        5 => [0.0, -h, -w, 0.0, x + w, y + h],
        6 => [0.0, -h, w, 0.0, x, y + h],
        7 => [0.0, h, w, 0.0, x, y],
        8 => [0.0, h, -w, 0.0, x + w, y],
        _ => [w, 0.0, 0.0, h, x, y],
    }
}

/// Add one outline entry per source (title, 0-based first page) with lopdf.
fn add_bookmarks(bytes: &[u8], starts: &[(String, u32)]) -> Result<Vec<u8>, ConvertError> {
    let fail = |e: lopdf::Error| ConvertError::other("Couldn't add the bookmarks.", e);
    let mut doc = lopdf::Document::load_mem(bytes).map_err(fail)?;
    let pages = doc.get_pages();
    for (title, start) in starts {
        if let Some(&page) = pages.get(&(start + 1)) {
            doc.add_bookmark(
                lopdf::Bookmark::new(title.clone(), [0.0, 0.0, 0.0], 0, page),
                None,
            );
        }
    }
    if let Some(outline) = doc.build_outline() {
        let catalog = doc.catalog_mut().map_err(fail)?;
        catalog.set("Outlines", lopdf::Object::Reference(outline));
        catalog.set("PageMode", lopdf::Object::Name(b"UseOutlines".to_vec()));
    }
    let mut out = Vec::with_capacity(bytes.len() + 4096);
    doc.save_to(&mut out)
        .map_err(|e| ConvertError::other("Couldn't add the bookmarks.", e))?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn apply(m: [f32; 6], (u, v): (f32, f32)) -> (f32, f32) {
        (m[0] * u + m[2] * v + m[4], m[1] * u + m[3] * v + m[5])
    }

    #[test]
    fn placement_turns_pixels_upright() {
        let l = Layout {
            page_w: 30.0,
            page_h: 20.0,
            x: 5.0,
            y: 5.0,
            w: 20.0,
            h: 10.0,
        };
        // Image-space top-left of the stored pixels is (0, 1).
        // Orientation 1: stays top-left of the box.
        assert_eq!(apply(placement(1, &l), (0.0, 1.0)), (5.0, 15.0));
        // 6 (rotate 90° clockwise to view): stored top-left ends top-right.
        assert_eq!(apply(placement(6, &l), (0.0, 1.0)), (25.0, 15.0));
        // 8 (rotate 90° counter-clockwise): stored top-left ends bottom-left.
        assert_eq!(apply(placement(8, &l), (0.0, 1.0)), (5.0, 5.0));
        // 3: bottom-right.
        assert_eq!(apply(placement(3, &l), (0.0, 1.0)), (25.0, 5.0));
        // Every orientation covers exactly the layout box.
        for o in 1..=8 {
            let m = placement(o, &l);
            let pts = [(0.0, 0.0), (1.0, 0.0), (0.0, 1.0), (1.0, 1.0)].map(|p| apply(m, p));
            let xs = pts.map(|p| p.0);
            let ys = pts.map(|p| p.1);
            let min = |a: [f32; 4]| a.iter().copied().fold(f32::MAX, f32::min);
            let max = |a: [f32; 4]| a.iter().copied().fold(f32::MIN, f32::max);
            assert_eq!((min(xs), max(xs), min(ys), max(ys)), (5.0, 25.0, 5.0, 15.0));
        }
    }

    #[test]
    fn layouts() {
        let fit = layout(PageSize::FitToImage, 0, 96, 192);
        assert_eq!((fit.page_w, fit.page_h), (72.0, 144.0));
        let fit_m = layout(PageSize::FitToImage, 10, 96, 96);
        assert!((fit_m.page_w - (72.0 + 2.0 * 28.346)).abs() < 0.01);
        let huge = layout(PageSize::FitToImage, 0, 100_000, 10);
        assert!(huge.page_w <= MAX_PAGE_POINTS);
        let a4 = layout(PageSize::A4, 10, 1000, 500);
        assert_eq!(
            (a4.page_w, a4.page_h),
            (841.89, 595.276),
            "wide → landscape"
        );
        assert!((a4.w - (841.89 - 2.0 * 28.346)).abs() < 0.01);
        assert!((a4.x - 28.346).abs() < 0.01);
        let letter = layout(PageSize::Letter, 0, 10, 10);
        assert_eq!((letter.page_w, letter.page_h), (612.0, 792.0));
        assert_eq!((letter.w, letter.h), (612.0, 612.0), "enlarged to fit");
        assert_eq!(letter.y, 90.0);
        let silly = layout(PageSize::A4, 10_000, 10, 10);
        assert!(silly.w > 0.0);
    }

    #[test]
    fn jpeg_and_webp_sniffing() {
        let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures");
        let jpg = std::fs::read(fixtures.join("sample.jpg")).unwrap();
        assert_eq!(jpeg_components(&jpg), Some(3));
        assert_eq!(jpeg_components(b"\x89PNG"), None);
        let webp = std::fs::read(fixtures.join("sample.webp")).unwrap();
        assert!(!webp_is_lossless(&webp));
        assert!(is_lossy_source(Path::new("a.HEIC"), b""));
        assert!(!is_lossy_source(Path::new("a.png"), b""));
    }
}
