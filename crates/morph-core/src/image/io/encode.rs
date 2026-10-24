// SPDX-License-Identifier: GPL-3.0-or-later
//! Encoders for every output format (SPEC §4.1, §7).

use super::container::{ColorSpace, matching_icc};
use super::{EncodeOptions, Metadata, OtherMetadata, container, exif_edit, flatten, isobmff, xmp};
use crate::error::ConvertError;
use crate::image::options::{DEFAULT_ICO_SIZES, OutputFormat};
use fast_image_resize::{FilterType, ResizeAlg, ResizeOptions, Resizer};
use image::{ColorType, DynamicImage, ExtendedColorType, ImageEncoder, ImageFormat, RgbaImage};
use std::io::Cursor;

fn failed(details: impl ToString) -> ConvertError {
    ConvertError::other("Couldn't encode the image.", details)
}

/// Metadata as it is written: EXIF without prefix and with orientation 1;
/// unparseable EXIF and empty blocks are dropped.
fn prepare(meta: Option<&Metadata>) -> Metadata {
    let Some(m) = meta else {
        return Metadata::default();
    };
    Metadata {
        exif: m
            .exif
            .as_deref()
            .map(exif_edit::without_prefix)
            .filter(|e| exif_edit::is_valid(e))
            .map(exif_edit::with_orientation_reset),
        icc: m.icc.clone().filter(|v| !v.is_empty()),
        xmp: m
            .xmp
            .as_deref()
            .filter(|v| !v.is_empty())
            .map(xmp::with_orientation_reset),
    }
}

fn has_any(m: &Metadata) -> bool {
    m.exif.is_some() || m.icc.is_some() || m.xmp.is_some()
}

pub(crate) fn encode(
    img: &DynamicImage,
    format: OutputFormat,
    opts: &EncodeOptions,
    meta: Option<&Metadata>,
) -> Result<Vec<u8>, ConvertError> {
    if img.width() == 0 || img.height() == 0 {
        return Err(failed("image has no pixels"));
    }
    let quality = match opts.quality {
        0 => format.default_quality(),
        q => q.min(100),
    };
    let meta = prepare(meta);
    match format {
        OutputFormat::Jpg => {
            // The ICC profile is checked against the encoded colour space by the rewrite.
            let out = jpeg(img, quality, opts)?;
            if has_any(&meta) {
                container::rewrite_jpeg(&out, &meta, OtherMetadata::Keep)
            } else {
                Ok(out)
            }
        }
        OutputFormat::Webp => {
            let out = webp(img, quality, opts.lossless)?;
            if has_any(&meta) {
                container::rewrite_webp(&out, &meta, OtherMetadata::Keep)
            } else {
                Ok(out)
            }
        }
        OutputFormat::Png => {
            let out = png(img, opts.png_level, opts.png_lossy)?;
            if has_any(&meta) {
                container::rewrite_png(&out, &meta, OtherMetadata::Keep)
            } else {
                Ok(out)
            }
        }
        OutputFormat::Avif => {
            let out = avif(img, quality, opts.lossless, meta.exif.as_deref())?;
            // AVIF pixels are always written as colour (RGB).
            Ok(
                match matching_icc(meta.icc.as_deref(), Some(ColorSpace::Rgb)) {
                    Some(icc) => isobmff::avif_with_icc(&out, &icc).unwrap_or(out),
                    None => out,
                },
            )
        }
        OutputFormat::Bmp => {
            let rgb = DynamicImage::ImageRgb8(flatten(img, opts.background));
            write_with_image(&rgb, ImageFormat::Bmp)
        }
        OutputFormat::Tiff => {
            // Gray + alpha is written as RGBA (see `tiff`).
            let space = match img.color() {
                ColorType::L8 | ColorType::L16 => ColorSpace::Gray,
                _ => ColorSpace::Rgb,
            };
            let icc = matching_icc(meta.icc.as_deref(), Some(space));
            tiff(img, icc)
        }
        OutputFormat::Gif => gif(img),
        OutputFormat::Ico => ico(img, &opts.ico_sizes),
    }
}

fn write_with_image(img: &DynamicImage, fmt: ImageFormat) -> Result<Vec<u8>, ConvertError> {
    let mut out = Cursor::new(Vec::new());
    img.write_to(&mut out, fmt).map_err(failed)?;
    Ok(out.into_inner())
}

// ---------------------------------------------------------------- JPEG

fn jpeg(img: &DynamicImage, quality: u8, opts: &EncodeOptions) -> Result<Vec<u8>, ConvertError> {
    let (w, h) = (img.width() as usize, img.height() as usize);
    // Borrow the pixels when they are already in the right layout.
    let converted: Vec<u8>;
    let (space, pixels): (_, &[u8]) = match img {
        DynamicImage::ImageLuma8(i) => (mozjpeg::ColorSpace::JCS_GRAYSCALE, i.as_raw()),
        DynamicImage::ImageRgb8(i) => (mozjpeg::ColorSpace::JCS_RGB, i.as_raw()),
        other if matches!(other.color(), ColorType::L16) => {
            converted = other.to_luma8().into_raw();
            (mozjpeg::ColorSpace::JCS_GRAYSCALE, &converted)
        }
        other => {
            converted = flatten(other, opts.background).into_raw();
            (mozjpeg::ColorSpace::JCS_RGB, &converted)
        }
    };
    // mozjpeg reports libjpeg errors by panicking (unwinding error manager).
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(
        || -> std::io::Result<Vec<u8>> {
            let mut c = mozjpeg::Compress::new(space);
            c.set_size(w, h);
            c.set_quality(f32::from(quality.max(1)));
            if opts.chroma_444 && space == mozjpeg::ColorSpace::JCS_RGB {
                c.set_chroma_sampling_pixel_sizes((1, 1), (1, 1));
            }
            if !opts.draft {
                c.set_progressive_mode();
                c.set_optimize_coding(true);
            }
            let mut started = c.start_compress(Vec::new())?;
            started.write_scanlines(pixels)?;
            started.finish()
        },
    ))
    .map_err(|_| failed("mozjpeg failed"))?
    .map_err(failed)
}

// ---------------------------------------------------------------- WebP

fn webp(img: &DynamicImage, quality: u8, lossless: bool) -> Result<Vec<u8>, ConvertError> {
    let (w, h) = (img.width(), img.height());
    if w > super::WEBP_MAX_SIDE || h > super::WEBP_MAX_SIDE {
        return Err(super::webp_too_large(w, h));
    }
    let alpha = img.color().has_alpha();
    let converted: Vec<u8>;
    let buf: &[u8] = match img {
        DynamicImage::ImageRgba8(i) => i.as_raw(),
        DynamicImage::ImageRgb8(i) => i.as_raw(),
        other if alpha => {
            converted = other.to_rgba8().into_raw();
            &converted
        }
        other => {
            converted = other.to_rgb8().into_raw();
            &converted
        }
    };
    let enc = if alpha {
        webp::Encoder::from_rgba(buf, w, h)
    } else {
        webp::Encoder::from_rgb(buf, w, h)
    };
    let mut config = webp::WebPConfig::new().map_err(|_| failed("libwebp config init"))?;
    config.lossless = i32::from(lossless);
    config.quality = if lossless { 75.0 } else { f32::from(quality) };
    config.method = 4;
    config.thread_level = 1;
    config.alpha_compression = i32::from(!lossless);
    // Lossless keeps the exact colors under transparent pixels.
    config.exact = i32::from(lossless);
    let mem = enc
        .encode_advanced(&config)
        .map_err(|e| failed(format!("libwebp: {e:?}")))?;
    Ok(mem.to_vec())
}

// ---------------------------------------------------------------- AVIF

fn avif(
    img: &DynamicImage,
    quality: u8,
    lossless: bool,
    exif: Option<&[u8]>,
) -> Result<Vec<u8>, ConvertError> {
    use ravif::{AlphaColorMode, ColorModel, Encoder, Img, RGB8, RGBA8};
    let (w, h) = (img.width() as usize, img.height() as usize);
    let mut enc = Encoder::new().with_speed(6);
    enc = if lossless {
        // Near-lossless: AV1 at quantizer 0 with RGB (identity) coefficients; colors
        // under fully transparent pixels are kept.
        enc.with_quality(100.0)
            .with_alpha_quality(100.0)
            .with_internal_color_model(ColorModel::RGB)
            .with_alpha_color_mode(AlphaColorMode::UnassociatedDirty)
    } else {
        let q = f32::from(quality.clamp(1, 100));
        enc.with_quality(q).with_alpha_quality(q.clamp(60.0, 100.0))
    };
    if let Some(exif) = exif {
        enc = enc.with_exif(exif.to_vec());
    }
    // Read the pixels in place when they are already 8-bit RGB(A).
    let converted: Vec<u8>;
    let res = if img.color().has_alpha() {
        let raw: &[u8] = match img {
            DynamicImage::ImageRgba8(i) => i.as_raw(),
            other => {
                converted = other.to_rgba8().into_raw();
                &converted
            }
        };
        let px: Vec<RGBA8> = raw
            .as_chunks::<4>()
            .0
            .iter()
            .map(|c| RGBA8::new(c[0], c[1], c[2], c[3]))
            .collect();
        enc.encode_rgba(Img::new(&px[..], w, h))
    } else {
        let raw: &[u8] = match img {
            DynamicImage::ImageRgb8(i) => i.as_raw(),
            other => {
                converted = other.to_rgb8().into_raw();
                &converted
            }
        };
        let px: Vec<RGB8> = raw
            .as_chunks::<3>()
            .0
            .iter()
            .map(|c| RGB8::new(c[0], c[1], c[2]))
            .collect();
        enc.encode_rgb(Img::new(&px[..], w, h))
    };
    res.map(|r| r.avif_file)
        .map_err(|e| failed(format!("ravif: {e}")))
}

// ---------------------------------------------------------------- PNG

fn png(img: &DynamicImage, level: u8, lossy: bool) -> Result<Vec<u8>, ConvertError> {
    let raw = if lossy {
        quantized_raw(img)?
    } else {
        lossless_raw(img)?
    };
    let opts = oxipng::Options::from_preset(level.min(6));
    raw.create_optimized_png(&opts)
        .map_err(|e| failed(format!("oxipng: {e}")))
}

fn be16(v: &[u16]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_be_bytes()).collect()
}

fn lossless_raw(img: &DynamicImage) -> Result<oxipng::RawImage, ConvertError> {
    use oxipng::{BitDepth::*, ColorType as C};
    let (w, h) = (img.width(), img.height());
    let (ct, depth, data) = match img {
        DynamicImage::ImageLuma8(i) => (
            C::Grayscale {
                transparent_shade: None,
            },
            Eight,
            i.as_raw().clone(),
        ),
        DynamicImage::ImageLumaA8(i) => (C::GrayscaleAlpha, Eight, i.as_raw().clone()),
        DynamicImage::ImageRgb8(i) => (
            C::RGB {
                transparent_color: None,
            },
            Eight,
            i.as_raw().clone(),
        ),
        DynamicImage::ImageRgba8(i) => (C::RGBA, Eight, i.as_raw().clone()),
        DynamicImage::ImageLuma16(i) => (
            C::Grayscale {
                transparent_shade: None,
            },
            Sixteen,
            be16(i.as_raw()),
        ),
        DynamicImage::ImageLumaA16(i) => (C::GrayscaleAlpha, Sixteen, be16(i.as_raw())),
        DynamicImage::ImageRgb16(i) => (
            C::RGB {
                transparent_color: None,
            },
            Sixteen,
            be16(i.as_raw()),
        ),
        DynamicImage::ImageRgba16(i) => (C::RGBA, Sixteen, be16(i.as_raw())),
        other if other.color().has_alpha() => (C::RGBA, Sixteen, be16(other.to_rgba16().as_raw())),
        other => (
            C::RGB {
                transparent_color: None,
            },
            Sixteen,
            be16(other.to_rgb16().as_raw()),
        ),
    };
    oxipng::RawImage::new(w, h, ct, depth, data).map_err(|e| failed(format!("oxipng: {e}")))
}

/// Lossy palette quantization (imagequant), at most 256 colors.
fn quantized_raw(img: &DynamicImage) -> Result<oxipng::RawImage, ConvertError> {
    let (w, h) = (img.width(), img.height());
    let px: Vec<imagequant::RGBA> = img
        .to_rgba8()
        .as_chunks::<4>()
        .0
        .iter()
        .map(|c| imagequant::RGBA::new(c[0], c[1], c[2], c[3]))
        .collect();
    let q = |e: imagequant::Error| failed(format!("imagequant: {e}"));
    let mut attr = imagequant::new();
    attr.set_quality(0, 90).map_err(q)?;
    attr.set_speed(4).map_err(q)?;
    let mut liq = attr.new_image(px, w as usize, h as usize, 0.0).map_err(q)?;
    let mut res = attr.quantize(&mut liq).map_err(q)?;
    res.set_dithering_level(1.0).map_err(q)?;
    let (palette, indexes) = res.remapped(&mut liq).map_err(q)?;
    oxipng::RawImage::new(
        w,
        h,
        oxipng::ColorType::Indexed { palette },
        oxipng::BitDepth::Eight,
        indexes,
    )
    .map_err(|e| failed(format!("oxipng: {e}")))
}

// ---------------------------------------------------------------- TIFF / GIF

fn tiff(img: &DynamicImage, icc: Option<Vec<u8>>) -> Result<Vec<u8>, ConvertError> {
    // The TIFF encoder has no gray+alpha mode.
    let converted;
    let img = match img.color() {
        ColorType::La8 => {
            converted = DynamicImage::ImageRgba8(img.to_rgba8());
            &converted
        }
        ColorType::La16 => {
            converted = DynamicImage::ImageRgba16(img.to_rgba16());
            &converted
        }
        ColorType::Rgb32F | ColorType::Rgba32F => {
            converted = DynamicImage::ImageRgba16(img.to_rgba16());
            &converted
        }
        _ => img,
    };
    let mut out = Cursor::new(Vec::new());
    let mut enc = image::codecs::tiff::TiffEncoder::new(&mut out);
    if let Some(icc) = icc {
        let _ = enc.set_icc_profile(icc); // best effort
    }
    enc.write_image(
        img.as_bytes(),
        img.width(),
        img.height(),
        ExtendedColorType::from(img.color()),
    )
    .map_err(failed)?;
    Ok(out.into_inner())
}

fn gif(img: &DynamicImage) -> Result<Vec<u8>, ConvertError> {
    let rgba = img.to_rgba8();
    let mut out = Vec::new();
    {
        // Speed 10 of 1–30: good palettes without taking seconds on photos.
        let mut enc = image::codecs::gif::GifEncoder::new_with_speed(&mut out, 10);
        enc.encode(
            rgba.as_raw(),
            rgba.width(),
            rgba.height(),
            ExtendedColorType::Rgba8,
        )
        .map_err(failed)?;
    }
    Ok(out)
}

// ---------------------------------------------------------------- ICO

/// Lanczos3 resize of an RGBA image (alpha-aware).
pub(crate) fn resize_rgba(src: &RgbaImage, w: u32, h: u32) -> Result<RgbaImage, ConvertError> {
    if src.width() == w && src.height() == h {
        return Ok(src.clone());
    }
    let mut dst = RgbaImage::new(w, h);
    let opts = ResizeOptions::new().resize_alg(ResizeAlg::Convolution(FilterType::Lanczos3));
    Resizer::new()
        .resize(src, &mut dst, &opts)
        .map_err(|e| failed(format!("resize: {e}")))?;
    Ok(dst)
}

fn ico(img: &DynamicImage, sizes: &[u32]) -> Result<Vec<u8>, ConvertError> {
    let rgba = img.to_rgba8();
    let (w, h) = rgba.dimensions();
    let side = w.max(h);
    let square = if w == h {
        rgba
    } else {
        let mut canvas = RgbaImage::new(side, side); // transparent
        image::imageops::replace(
            &mut canvas,
            &rgba,
            i64::from((side - w) / 2),
            i64::from((side - h) / 2),
        );
        canvas
    };
    let mut sizes: Vec<u32> = sizes
        .iter()
        .copied()
        .filter(|s| (1..=256).contains(s))
        .collect();
    if sizes.is_empty() {
        sizes = DEFAULT_ICO_SIZES.to_vec();
    }
    sizes.sort_unstable();
    sizes.dedup();

    // Every entry is RGBA: 32-bit BMP below 256 px, RGBA PNG at 256 px (the
    // usual layout; RGB PNG entries can't be read by some decoders).
    let mut entries: Vec<(u32, u16, Vec<u8>)> = Vec::with_capacity(sizes.len());
    for s in sizes {
        let px = resize_rgba(&square, s, s)?;
        let data = if s >= 256 {
            let mut png = Vec::new();
            image::codecs::png::PngEncoder::new(&mut png)
                .write_image(px.as_raw(), s, s, ExtendedColorType::Rgba8)
                .map_err(failed)?;
            png
        } else {
            let icon = ico::IconImage::from_rgba_data(s, s, px.into_raw());
            bmp_entry_32(&icon)?
        };
        entries.push((s, 32, data));
    }
    write_ico_dir(&entries)
}

/// 32-bit BMP icon entry data (forces 32 bpp, so alpha always survives).
fn bmp_entry_32(icon: &ico::IconImage) -> Result<Vec<u8>, ConvertError> {
    let entry = ico::IconDirEntry::encode_as_bmp(icon).map_err(failed)?;
    if entry.bits_per_pixel() == 32 {
        return Ok(entry.data().to_vec());
    }
    // The ico crate picked a smaller depth (opaque image): write 32-bit ourselves.
    let (w, h) = (icon.width(), icon.height());
    let rgba = icon.rgba_data();
    let mask_row = (w.div_ceil(32) * 4) as usize;
    let mut d = Vec::with_capacity(40 + rgba.len() + mask_row * h as usize);
    d.extend_from_slice(&40u32.to_le_bytes());
    d.extend_from_slice(&(w as i32).to_le_bytes());
    d.extend_from_slice(&((2 * h) as i32).to_le_bytes()); // XOR + AND masks
    d.extend_from_slice(&1u16.to_le_bytes());
    d.extend_from_slice(&32u16.to_le_bytes());
    d.extend_from_slice(&[0u8; 24]); // no compression, sizes/resolution/palette 0
    for y in (0..h as usize).rev() {
        for x in 0..w as usize {
            let p = (y * w as usize + x) * 4;
            d.extend_from_slice(&[rgba[p + 2], rgba[p + 1], rgba[p], rgba[p + 3]]);
        }
    }
    d.resize(d.len() + mask_row * h as usize, 0); // AND mask: all visible
    Ok(d)
}

fn write_ico_dir(entries: &[(u32, u16, Vec<u8>)]) -> Result<Vec<u8>, ConvertError> {
    let count = u16::try_from(entries.len()).map_err(|_| failed("too many ICO sizes"))?;
    let mut out = Vec::new();
    out.extend_from_slice(&[0, 0, 1, 0]);
    out.extend_from_slice(&count.to_le_bytes());
    let mut offset = 6 + 16 * entries.len();
    for (s, bpp, data) in entries {
        let side = if *s >= 256 { 0 } else { *s as u8 };
        out.extend_from_slice(&[side, side, 0, 0]);
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&bpp.to_le_bytes());
        let len = u32::try_from(data.len()).map_err(|_| failed("ICO entry too large"))?;
        out.extend_from_slice(&len.to_le_bytes());
        let off = u32::try_from(offset).map_err(|_| failed("ICO too large"))?;
        out.extend_from_slice(&off.to_le_bytes());
        offset += data.len();
    }
    for (_, _, data) in entries {
        out.extend_from_slice(data);
    }
    Ok(out)
}
