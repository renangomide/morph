// SPDX-License-Identifier: GPL-3.0-or-later
//! Decoding of every input format (SPEC §4.1). The format is detected from the
//! content first; the extension is only a fallback (TGA has no magic number).

use super::{DecodeOptions, Decoded, Metadata, exif_edit, heif};
use crate::error::ConvertError;
use image::codecs::gif::GifDecoder;
use image::codecs::webp::WebPDecoder;
use image::{
    AnimationDecoder, DynamicImage, ImageDecoder, ImageError, ImageFormat, ImageReader, Limits,
    RgbaImage,
};
use resvg::{tiny_skia, usvg};
use std::io::{BufReader, Cursor, Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::{Arc, OnceLock};

/// Largest SVG raster side we accept (keeps a typo from allocating gigabytes).
const SVG_MAX_SIDE: u32 = 16384;

/// Largest image we decode (pixels): about 1 gigapixel.
pub(crate) const MAX_PIXELS: u64 = 1_000_000_000;
/// Most memory a decoder may allocate: 1 gigapixel of RGBA8 plus some room.
const MAX_ALLOC: u64 = 4 * MAX_PIXELS + (256 << 20);

pub(crate) fn too_large(w: u32, h: u32) -> ConvertError {
    ConvertError::unsupported(
        format!("This image is too large to process ({w} × {h})."),
        format!("{w}x{h} pixels; the limit is {MAX_PIXELS} pixels"),
    )
}

/// Refuse images above [`MAX_PIXELS`] before anything big is allocated.
pub(crate) fn check_size(w: u32, h: u32) -> Result<(), ConvertError> {
    if u64::from(w) * u64::from(h) > MAX_PIXELS {
        Err(too_large(w, h))
    } else {
        Ok(())
    }
}

fn limits() -> Limits {
    let mut l = Limits::default();
    l.max_alloc = Some(MAX_ALLOC);
    l
}

/// Decoder error, reported as "too large" when a limit was hit.
fn decode_err(e: ImageError, (w, h): (u32, u32)) -> ConvertError {
    match e {
        ImageError::Limits(_) => too_large(w, h),
        e => ConvertError::damaged(e),
    }
}

pub(crate) fn decode_bytes(
    data: &[u8],
    path: Option<&Path>,
    opts: &DecodeOptions,
) -> Result<Decoded, ConvertError> {
    if data.is_empty() {
        return Err(ConvertError::damaged("the file is empty"));
    }
    if heif::is_heif_family(data) {
        return heif::decode(data);
    }
    let ext = path
        .and_then(|p| p.extension())
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase);
    let guessed = image::guess_format(data).ok();
    if guessed.is_none() && (looks_like_svg(data) || is_svgz(data, ext.as_deref())) {
        return decode_svg(data, path, opts);
    }
    let format = guessed
        .or_else(|| ext.as_deref().and_then(ImageFormat::from_extension))
        .ok_or_else(|| ConvertError::damaged("unknown image format"))?;
    match format {
        ImageFormat::Gif => decode_gif(data),
        ImageFormat::WebP => decode_webp(data),
        ImageFormat::Ico => decode_ico(data).or_else(|_| decode_generic(data, format)),
        _ => decode_generic(data, format),
    }
}

/// Width and height from a JPEG's frame header, skipping the segments before it.
fn jpeg_frame_size<R: Read + Seek>(r: &mut R) -> Option<(u32, u32)> {
    let mut b = [0u8; 2];
    r.read_exact(&mut b).ok()?;
    if b != [0xFF, 0xD8] {
        return None;
    }
    loop {
        r.read_exact(&mut b).ok()?;
        if b[0] != 0xFF {
            return None;
        }
        let mut m = b[1];
        while m == 0xFF {
            let mut one = [0u8; 1];
            r.read_exact(&mut one).ok()?;
            m = one[0];
        }
        if matches!(m, 0x01 | 0xD0..=0xD7) {
            continue;
        }
        if matches!(m, 0xD9 | 0xDA) {
            return None; // no frame header before the image data
        }
        r.read_exact(&mut b).ok()?;
        let len = u16::from_be_bytes(b);
        if len < 2 {
            return None;
        }
        let is_sof = matches!(m, 0xC0..=0xCF) && !matches!(m, 0xC4 | 0xC8 | 0xCC);
        if is_sof {
            let mut f = [0u8; 5];
            r.read_exact(&mut f).ok()?;
            let h = u32::from(u16::from_be_bytes([f[1], f[2]]));
            let w = u32::from(u16::from_be_bytes([f[3], f[4]]));
            return (w > 0 && h > 0).then_some((w, h));
        }
        r.seek(SeekFrom::Current(i64::from(len) - 2)).ok()?;
    }
}

/// Pixel size from the file header only (see [`super::probe_dimensions`]).
pub(crate) fn probe(path: &Path, opts: &DecodeOptions) -> Result<(u32, u32), ConvertError> {
    let mut file = std::fs::File::open(path)?;
    let len = file.metadata()?.len();
    if len == 0 {
        return Err(ConvertError::damaged("the file is empty"));
    }
    let mut head = Vec::with_capacity(4096);
    (&mut file).take(4096).read_to_end(&mut head)?;
    if heif::is_heif_family(&head) {
        file.seek(SeekFrom::Start(0))?;
        return heif::probe(file, len);
    }
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase);
    let guessed = image::guess_format(&head).ok();
    if guessed.is_none() && (looks_like_svg(&head) || is_svgz(&head, ext.as_deref())) {
        // The size doesn't depend on fonts or external resources.
        let data = std::fs::read(path)?;
        let tree = usvg::Tree::from_data(&data, &usvg::Options::default())
            .map_err(ConvertError::damaged)?;
        return svg_raster_size(tree.size(), opts);
    }
    let format = guessed
        .or_else(|| ext.as_deref().and_then(ImageFormat::from_extension))
        .ok_or_else(|| ConvertError::damaged("unknown image format"))?;
    file.seek(SeekFrom::Start(0))?;
    let mut reader = BufReader::new(file);
    // The JPEG decoder reads the whole file first; walking the markers is cheaper.
    if format == ImageFormat::Jpeg
        && let Some(dims) = jpeg_frame_size(&mut reader)
    {
        return Ok(dims);
    }
    if format == ImageFormat::Ico
        && let Ok(dir) = ico::IconDir::read(&mut reader)
        && let Ok(best) = best_ico_entry(&dir)
    {
        return Ok(ico_entry_dims(best));
    }
    reader.seek(SeekFrom::Start(0))?;
    // Only headers are read, so no allocation limit is needed.
    let mut r = ImageReader::with_format(reader, format);
    r.no_limits();
    r.into_dimensions().map_err(ConvertError::damaged)
}

/// Metadata blocks and orientation from any `image` decoder (best effort).
fn read_meta<D: ImageDecoder>(dec: &mut D) -> (Metadata, u8) {
    let exif = dec
        .exif_metadata()
        .ok()
        .flatten()
        .map(|e| exif_edit::without_prefix(&e).to_vec())
        .filter(|e| exif_edit::is_valid(e));
    let meta = Metadata {
        icc: dec.icc_profile().ok().flatten().filter(|v| !v.is_empty()),
        xmp: dec.xmp_metadata().ok().flatten().filter(|v| !v.is_empty()),
        exif,
    };
    let orientation = meta
        .exif
        .as_deref()
        .and_then(exif_edit::orientation)
        .or_else(|| dec.orientation().ok().map(|o| o.to_exif()))
        .filter(|o| (1..=8).contains(o))
        .unwrap_or(1);
    (meta, orientation)
}

fn decode_generic(data: &[u8], format: ImageFormat) -> Result<Decoded, ConvertError> {
    // Headers first without limits, so a huge image gets the "too large" message.
    let mut reader = ImageReader::with_format(Cursor::new(data), format);
    reader.no_limits();
    let mut dec = reader.into_decoder().map_err(ConvertError::damaged)?;
    let dims = dec.dimensions();
    check_size(dims.0, dims.1)?;
    dec.set_limits(limits()).map_err(|e| decode_err(e, dims))?;
    let (mut meta, orientation) = read_meta(&mut dec);
    if format == ImageFormat::Tiff && meta.icc.is_none() {
        meta.icc = exif_edit::tiff_icc(data);
    }
    let image = DynamicImage::from_decoder(dec).map_err(|e| decode_err(e, dims))?;
    Ok(Decoded {
        image,
        orientation,
        meta,
        animated: false,
    })
}

fn decode_webp(data: &[u8]) -> Result<Decoded, ConvertError> {
    let mut dec = WebPDecoder::new(Cursor::new(data)).map_err(ConvertError::damaged)?;
    let dims = dec.dimensions();
    check_size(dims.0, dims.1)?;
    dec.set_limits(limits()).map_err(|e| decode_err(e, dims))?;
    let animated = dec.has_animation();
    let (meta, orientation) = read_meta(&mut dec);
    let image = DynamicImage::from_decoder(dec).map_err(|e| decode_err(e, dims))?;
    Ok(Decoded {
        image,
        orientation,
        meta,
        animated,
    })
}

fn decode_gif(data: &[u8]) -> Result<Decoded, ConvertError> {
    let mut dec = GifDecoder::new(Cursor::new(data)).map_err(ConvertError::damaged)?;
    let dims = dec.dimensions();
    check_size(dims.0, dims.1)?;
    dec.set_limits(limits()).map_err(|e| decode_err(e, dims))?;
    let (meta, _) = read_meta(&mut dec);
    let mut frames = dec.into_frames();
    let first = frames
        .next()
        .ok_or_else(|| ConvertError::damaged("GIF has no frames"))?
        .map_err(ConvertError::damaged)?;
    let animated = frames.next().is_some();
    Ok(Decoded {
        image: DynamicImage::ImageRgba8(first.into_buffer()),
        orientation: 1,
        meta,
        animated,
    })
}

/// ICO: the entry with the most pixels (then the highest bit depth).
fn best_ico_entry(dir: &ico::IconDir) -> Result<&ico::IconDirEntry, ConvertError> {
    dir.entries()
        .iter()
        .max_by_key(|e| {
            (
                u64::from(e.width()) * u64::from(e.height()),
                e.bits_per_pixel(),
            )
        })
        .ok_or_else(|| ConvertError::damaged("ICO has no images"))
}

/// Real size of an ICO entry (a PNG entry may be larger than its directory says).
fn ico_entry_dims(e: &ico::IconDirEntry) -> (u32, u32) {
    let d = e.data();
    if e.is_png() && d.len() >= 24 && &d[12..16] == b"IHDR" {
        let n = |at: usize| u32::from_be_bytes([d[at], d[at + 1], d[at + 2], d[at + 3]]);
        (n(16), n(20))
    } else {
        (e.width(), e.height())
    }
}

fn decode_ico(data: &[u8]) -> Result<Decoded, ConvertError> {
    let dir = ico::IconDir::read(Cursor::new(data)).map_err(ConvertError::damaged)?;
    let best = best_ico_entry(&dir)?;
    let (w, h) = ico_entry_dims(best);
    check_size(w, h)?;
    let icon = best.decode().map_err(ConvertError::damaged)?;
    let (w, h) = (icon.width(), icon.height());
    let image = RgbaImage::from_raw(w, h, icon.into_rgba_data())
        .ok_or_else(|| ConvertError::damaged("ICO entry has a bad size"))?;
    Ok(Decoded {
        image: DynamicImage::ImageRgba8(image),
        orientation: 1,
        meta: Metadata::default(),
        animated: false,
    })
}

fn looks_like_svg(data: &[u8]) -> bool {
    let head = &data[..data.len().min(4096)];
    let head = head.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(head);
    let start = head.iter().position(|b| !b.is_ascii_whitespace());
    let Some(start) = start else { return false };
    head[start] == b'<' && head.windows(4).any(|w| w == b"<svg")
}

fn is_svgz(data: &[u8], ext: Option<&str>) -> bool {
    data.starts_with(&[0x1f, 0x8b]) && matches!(ext, Some("svg" | "svgz"))
}

/// System fonts, loaded once and only for SVGs that contain text.
fn system_fonts() -> Arc<usvg::fontdb::Database> {
    static FONTS: OnceLock<Arc<usvg::fontdb::Database>> = OnceLock::new();
    FONTS
        .get_or_init(|| {
            let mut db = usvg::fontdb::Database::new();
            db.load_system_fonts();
            Arc::new(db)
        })
        .clone()
}

/// Raster size of an SVG of intrinsic size `size` at the requested width.
fn svg_raster_size(size: usvg::Size, opts: &DecodeOptions) -> Result<(u32, u32), ConvertError> {
    let (sw, sh) = (size.width(), size.height());
    if !(sw > 0.0 && sh > 0.0) {
        return Err(ConvertError::damaged("SVG has no size"));
    }
    let w = opts.svg_width.clamp(1, SVG_MAX_SIDE);
    let h = ((w as f32) * sh / sw)
        .round()
        .clamp(1.0, SVG_MAX_SIDE as f32) as u32;
    Ok((w, h))
}

fn decode_svg(
    data: &[u8],
    path: Option<&Path>,
    opts: &DecodeOptions,
) -> Result<Decoded, ConvertError> {
    let mut options = usvg::Options {
        resources_dir: path.and_then(|p| p.parent()).map(Path::to_path_buf),
        ..usvg::Options::default()
    };
    // Gzipped SVGs may contain text too; loading fonts is the safe choice there.
    if data.starts_with(&[0x1f, 0x8b]) || data.windows(5).any(|w| w == b"<text") {
        options.fontdb = system_fonts();
    }
    let tree = usvg::Tree::from_data(data, &options).map_err(ConvertError::damaged)?;
    let size = tree.size();
    let (sw, sh) = (size.width(), size.height());
    let (w, h) = svg_raster_size(size, opts)?;
    let mut pixmap = tiny_skia::Pixmap::new(w, h)
        .ok_or_else(|| ConvertError::other("Couldn't open the SVG.", "pixmap allocation"))?;
    let transform = tiny_skia::Transform::from_scale(w as f32 / sw, h as f32 / sh);
    resvg::render(&tree, transform, &mut pixmap.as_mut());
    let image = RgbaImage::from_raw(w, h, pixmap.take_demultiplied())
        .ok_or_else(|| ConvertError::other("Couldn't open the SVG.", "pixmap size"))?;
    Ok(Decoded {
        image: DynamicImage::ImageRgba8(image),
        orientation: 1,
        meta: Metadata::default(),
        animated: false,
    })
}
