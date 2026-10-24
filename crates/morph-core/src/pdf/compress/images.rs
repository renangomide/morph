// SPDX-License-Identifier: GPL-3.0-or-later
//! Re-encoding one image XObject: decode (DCT, or Flate/LZW/RunLength/ASCII
//! filters with 8-bit gray or RGB samples), downscale to the target DPI (Lanczos3),
//! then JPEG (photos) or Flate with PNG predictors (soft masks, flat graphics).
//! Anything we can't handle exactly is left as it is.

use crate::image::common::resize_lanczos;
use crate::image::io::{self, EncodeOptions};
use crate::image::options::OutputFormat;
use image::{DynamicImage, GrayImage, RgbImage};
use lopdf::{Dictionary, Document, Object, Stream};

/// Images above this many pixels are left alone (memory).
pub(super) const MAX_PIXELS: u64 = 100_000_000;

/// Only downscale when the image is noticeably above the target (resampling an
/// image from 160 to 150 DPI costs sharpness for almost no saving).
const DOWNSCALE_MARGIN: f32 = 1.2;

/// A JPEG kept at its size is only re-encoded when that saves at least 10%
/// (re-compressing JPEG artefacts for a few percent isn't worth the loss).
const JPEG_REENCODE_GAIN: f32 = 0.9;

/// Placements smaller than this (points) are treated as unknown sizes.
const MIN_PLACED_PT: f32 = 1.0;

#[derive(Debug, Clone, Copy)]
pub(super) struct Target {
    pub max_dpi: f32,
    pub quality: u8,
}

/// What happened to one image.
#[derive(Debug)]
pub(super) enum Outcome {
    /// New stream, smaller than the old one.
    Replaced(Stream),
    /// Handled, but the result wasn't smaller.
    Unchanged,
    /// Left as it is: a format or colour model we don't rewrite.
    Skipped(&'static str),
}

/// What the image is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Role {
    /// Drawn with `Do`.
    Image,
    /// Another image's `/SMask` (alpha): always lossless, gray.
    SoftMask,
}

fn int(dict: &Dictionary, key: &[u8]) -> Option<i64> {
    dict.get(key).ok().and_then(|o| o.as_i64().ok())
}

/// Components per pixel for colour spaces whose samples we can keep as they are.
fn components(doc: &Document, dict: &Dictionary) -> Option<u8> {
    let cs = doc.dereference(dict.get(b"ColorSpace").ok()?).ok()?.1;
    if let Ok(name) = cs.as_name() {
        return match name {
            b"DeviceGray" | b"G" => Some(1),
            b"DeviceRGB" | b"RGB" => Some(3),
            _ => None,
        };
    }
    let arr = cs.as_array().ok()?;
    match arr.first()?.as_name().ok()? {
        b"CalGray" => Some(1),
        b"CalRGB" => Some(3),
        b"ICCBased" => {
            let icc = doc.dereference(arr.get(1)?).ok()?.1.as_stream().ok()?;
            match icc.dict.get(b"N").ok()?.as_i64().ok()? {
                1 => Some(1),
                3 => Some(3),
                _ => None,
            }
        }
        _ => None,
    }
}

/// Number of colour components in a JPEG's frame header.
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
        if (0xC0..=0xCF).contains(&marker) && !matches!(marker, 0xC4 | 0xC8 | 0xCC) {
            // Only baseline/extended/progressive Huffman (SOF0-2) at 8 bits.
            if !matches!(marker, 0xC0..=0xC2) || *bytes.get(i + 4)? != 8 {
                return None;
            }
            return bytes.get(i + 9).copied();
        }
        if marker == 0xDA || len < 2 {
            return None;
        }
        i += 2 + len;
    }
}

#[derive(Debug, Clone, Copy)]
enum Source {
    Jpeg,
    Lossless,
}

const LOSSLESS_FILTERS: [&[u8]; 5] = [
    b"FlateDecode",
    b"LZWDecode",
    b"ASCII85Decode",
    b"ASCIIHexDecode",
    b"RunLengthDecode",
];

/// The samples must be exactly `w * h * n` bytes: anything else means the
/// filters weren't undone the way the image expects (e.g. a predictor).
fn pixels(n: u8, w: u32, h: u32, raw: Vec<u8>) -> Option<DynamicImage> {
    let len = usize::try_from(u64::from(w) * u64::from(h) * u64::from(n)).ok()?;
    if raw.len() != len {
        return None;
    }
    if n == 1 {
        GrayImage::from_raw(w, h, raw).map(DynamicImage::ImageLuma8)
    } else {
        RgbImage::from_raw(w, h, raw).map(DynamicImage::ImageRgb8)
    }
}

/// lopdf only applies `/DecodeParms` when it is an inline dictionary (and then
/// to every filter in a chain). Rewrite the other forms PDF allows into that one:
/// a reference, a one-element array, or nulls. `None` = keep the stream as is.
/// Anything else (parameters on a chain of filters) is refused.
fn inline_decode_parms(
    doc: &Document,
    stream: &Stream,
    filters: usize,
) -> Result<Option<Stream>, &'static str> {
    const CHAINED: &str = "chained filters with parameters";
    let Ok(parms) = stream.dict.get(b"DecodeParms") else {
        return Ok(None);
    };
    let resolve = |o: &Object| -> Result<Option<Dictionary>, &'static str> {
        match doc.dereference(o).map_err(|_| "bad decode parameters")?.1 {
            Object::Null => Ok(None),
            Object::Dictionary(d) => Ok(Some(d.clone())),
            _ => Err("bad decode parameters"),
        }
    };
    let dict = match parms {
        Object::Dictionary(_) if filters > 1 => return Err(CHAINED),
        Object::Dictionary(_) => return Ok(None),
        Object::Array(arr) => {
            let mut found = None;
            for (i, item) in arr.iter().enumerate() {
                if let Some(d) = resolve(item)? {
                    if filters != 1 || i != 0 || arr.len() != 1 {
                        return Err(CHAINED);
                    }
                    found = Some(d);
                }
            }
            found
        }
        other => {
            let d = resolve(other)?;
            if d.is_some() && filters > 1 {
                return Err(CHAINED);
            }
            d
        }
    };
    let mut fixed = stream.clone();
    match dict {
        Some(d) => fixed.dict.set("DecodeParms", d),
        None => {
            fixed.dict.remove(b"DecodeParms");
        }
    }
    Ok(Some(fixed))
}

fn decode(
    doc: &Document,
    stream: &Stream,
    n: u8,
    w: u32,
    h: u32,
) -> Result<(DynamicImage, Source), &'static str> {
    let filters: Vec<&[u8]> = if stream.dict.has(b"Filter") {
        stream.filters().map_err(|_| "bad filter")?
    } else {
        Vec::new()
    };
    match filters.as_slice() {
        [b"DCTDecode"] => {
            if stream.dict.has(b"DecodeParms") {
                return Err("JPEG with decode parameters");
            }
            if jpeg_components(&stream.content) != Some(n) {
                return Err("unusual JPEG");
            }
            let img =
                image::load_from_memory_with_format(&stream.content, image::ImageFormat::Jpeg)
                    .map_err(|_| "unreadable JPEG")?;
            if (img.width(), img.height()) != (w, h) {
                return Err("JPEG size differs from the image dictionary");
            }
            let img = if n == 1 {
                DynamicImage::ImageLuma8(img.to_luma8())
            } else {
                DynamicImage::ImageRgb8(img.to_rgb8())
            };
            Ok((img, Source::Jpeg))
        }
        fs if fs.iter().all(|f| LOSSLESS_FILTERS.contains(f)) => {
            let fixed = inline_decode_parms(doc, stream, fs.len())?;
            let stream = fixed.as_ref().unwrap_or(stream);
            let expected = u64::from(w) * u64::from(h) * u64::from(n);
            let limit = usize::try_from(expected + 65_536).map_err(|_| "too large")?;
            let raw = stream
                .get_plain_content_with_limit(limit)
                .map_err(|_| "undecodable samples")?;
            let img = pixels(n, w, h, raw).ok_or("truncated samples")?;
            Ok((img, Source::Lossless))
        }
        _ => Err("JPEG 2000, JBIG2, CCITT or other filter"),
    }
}

/// Flat colour areas or few colours: a diagram, screenshot or logo. JPEG would
/// blur its edges, so it stays lossless.
fn looks_like_graphic(img: &DynamicImage) -> bool {
    use std::collections::HashSet;
    let rgb;
    let (raw, bpp): (&[u8], usize) = match img {
        DynamicImage::ImageLuma8(g) => (g.as_raw(), 1),
        DynamicImage::ImageRgb8(c) => (c.as_raw(), 3),
        other => {
            rgb = other.to_rgb8();
            (rgb.as_raw(), 3)
        }
    };
    let w = img.width() as usize;
    let h = img.height() as usize;
    if w < 2 || h == 0 {
        return true;
    }
    // About 256 sampled rows.
    let step = (h / 256).max(1);
    let mut colors: HashSet<[u8; 3]> = HashSet::new();
    let (mut same, mut total) = (0u64, 0u64);
    for y in (0..h).step_by(step) {
        let row = &raw[y * w * bpp..(y + 1) * w * bpp];
        let mut prev: Option<&[u8]> = None;
        for px in row.chunks_exact(bpp) {
            if bpp == 3 && colors.len() <= 256 {
                let mut c = [0u8; 3];
                c[..bpp].copy_from_slice(px);
                colors.insert(c);
            }
            if let Some(p) = prev {
                total += 1;
                same += u64::from(p == px);
            }
            prev = Some(px);
        }
    }
    // A grayscale image has at most 256 levels anyway: judge it by flat areas only.
    (bpp == 3 && colors.len() <= 256) || (total > 0 && same * 10 >= total * 4)
}

/// PNG-style row prediction (PDF `/Predictor 15`): per row, the filter with the
/// smallest sum of absolute values, as libpng does.
fn predict(raw: &[u8], width: usize, bpp: usize) -> Vec<u8> {
    let stride = width * bpp;
    let rows = raw.len().checked_div(stride).unwrap_or(0);
    let mut out = Vec::with_capacity(raw.len() + rows);
    let zero = vec![0u8; stride];
    let mut cand: [Vec<u8>; 5] = std::array::from_fn(|_| vec![0u8; stride]);
    for r in 0..rows {
        let cur = &raw[r * stride..(r + 1) * stride];
        let up = if r == 0 {
            &zero[..]
        } else {
            &raw[(r - 1) * stride..r * stride]
        };
        for i in 0..stride {
            let a = if i >= bpp { cur[i - bpp] } else { 0 };
            let b = up[i];
            let c = if i >= bpp { up[i - bpp] } else { 0 };
            let x = cur[i];
            cand[0][i] = x;
            cand[1][i] = x.wrapping_sub(a);
            cand[2][i] = x.wrapping_sub(b);
            cand[3][i] = x.wrapping_sub(((u16::from(a) + u16::from(b)) / 2) as u8);
            cand[4][i] = x.wrapping_sub(paeth(a, b, c));
        }
        let score =
            |v: &Vec<u8>| -> u64 { v.iter().map(|&b| u64::from((b as i8).unsigned_abs())).sum() };
        let (best, _) = cand
            .iter()
            .enumerate()
            .map(|(k, v)| (k, score(v)))
            .min_by_key(|&(_, s)| s)
            .unwrap_or((0, 0));
        out.push(best as u8);
        out.extend_from_slice(&cand[best]);
    }
    out
}

fn paeth(a: u8, b: u8, c: u8) -> u8 {
    let p = i16::from(a) + i16::from(b) - i16::from(c);
    let (pa, pb, pc) = (
        (p - i16::from(a)).abs(),
        (p - i16::from(b)).abs(),
        (p - i16::from(c)).abs(),
    );
    if pa <= pb && pa <= pc {
        a
    } else if pb <= pc {
        b
    } else {
        c
    }
}

/// Flate + PNG predictors. `None` if deflate didn't help at all.
fn encode_lossless(img: &DynamicImage, n: u8) -> Option<(Vec<u8>, Dictionary)> {
    let raw: Vec<u8> = if n == 1 {
        img.to_luma8().into_raw()
    } else {
        img.to_rgb8().into_raw()
    };
    let width = img.width() as usize;
    let predicted = predict(&raw, width, usize::from(n));
    let mut s = Stream::new(Dictionary::new(), predicted);
    s.compress().ok()?;
    if !s.dict.has(b"Filter") {
        return None;
    }
    let mut parms = Dictionary::new();
    parms.set("Predictor", 15);
    parms.set("Colors", i64::from(n));
    parms.set("BitsPerComponent", 8);
    parms.set("Columns", i64::from(img.width()));
    Some((s.content, parms))
}

/// Re-encode the image `stream` drawn at most `placed` points large.
/// `placed` is `None` when its size on the page is unknown (no downscaling).
pub(super) fn recompress(
    doc: &Document,
    stream: &Stream,
    placed: Option<(f32, f32)>,
    role: Role,
    target: Target,
) -> Outcome {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        recompress_inner(doc, stream, placed, role, target)
    })) {
        Ok(o) => o,
        Err(_) => Outcome::Skipped("image engine failure"),
    }
}

fn recompress_inner(
    doc: &Document,
    stream: &Stream,
    placed: Option<(f32, f32)>,
    role: Role,
    target: Target,
) -> Outcome {
    let dict = &stream.dict;
    if dict
        .get(b"ImageMask")
        .and_then(Object::as_bool)
        .unwrap_or(false)
    {
        return Outcome::Skipped("stencil mask");
    }
    if dict.get(b"Mask").is_ok_and(|m| m.as_array().is_ok()) {
        return Outcome::Skipped("colour-key mask");
    }
    if role == Role::SoftMask && dict.has(b"Matte") {
        return Outcome::Skipped("pre-blended soft mask");
    }
    if role == Role::Image
        && let Ok(smask) = dict.get(b"SMask").and_then(Object::as_reference)
        && doc
            .get_object(smask)
            .and_then(Object::as_stream)
            .is_ok_and(|s| s.dict.has(b"Matte"))
    {
        return Outcome::Skipped("pre-blended soft mask");
    }
    let bpc = int(dict, b"BitsPerComponent").unwrap_or(8);
    if bpc != 8 {
        return Outcome::Skipped("not 8 bits per sample");
    }
    let Some(n) = components(doc, dict) else {
        return Outcome::Skipped("colour space (CMYK, indexed, spot, Lab)");
    };
    if role == Role::SoftMask && n != 1 {
        return Outcome::Skipped("colour soft mask");
    }
    let (Some(w), Some(h)) = (int(dict, b"Width"), int(dict, b"Height")) else {
        return Outcome::Skipped("no size");
    };
    let (Ok(w), Ok(h)) = (u32::try_from(w), u32::try_from(h)) else {
        return Outcome::Skipped("bad size");
    };
    if w == 0 || h == 0 || u64::from(w) * u64::from(h) > MAX_PIXELS {
        return Outcome::Skipped("too large or empty");
    }

    let (img, source) = match decode(doc, stream, n, w, h) {
        Ok(v) => v,
        Err(why) => return Outcome::Skipped(why),
    };

    // Uniform scale so that neither side drops below the target DPI.
    let scale = match placed {
        Some((wpt, hpt)) if wpt >= MIN_PLACED_PT && hpt >= MIN_PLACED_PT => {
            let dpi = (w as f32 * 72.0 / wpt).min(h as f32 * 72.0 / hpt);
            if dpi > target.max_dpi * DOWNSCALE_MARGIN {
                target.max_dpi / dpi
            } else {
                1.0
            }
        }
        _ => 1.0,
    };
    let resized = scale < 1.0;
    let img = if resized {
        let nw = ((w as f32 * scale).round() as u32).max(1);
        let nh = ((h as f32 * scale).round() as u32).max(1);
        match resize_lanczos(&img, nw, nh) {
            Ok(i) => i,
            Err(_) => return Outcome::Skipped("resize failed"),
        }
    } else {
        img
    };

    let lossless = match (role, source) {
        (Role::SoftMask, _) => true,
        (_, Source::Jpeg) => false,
        (_, Source::Lossless) => looks_like_graphic(&img),
    };
    let old_len = stream.content.len();
    let mut new_dict = dict.clone();
    new_dict.remove(b"DecodeParms");
    new_dict.remove(b"Length");
    new_dict.set("Width", i64::from(img.width()));
    new_dict.set("Height", i64::from(img.height()));
    new_dict.set("BitsPerComponent", 8);

    let (content, max_len) = if lossless {
        let Some((content, parms)) = encode_lossless(&img, n) else {
            return Outcome::Unchanged;
        };
        new_dict.set("Filter", "FlateDecode");
        new_dict.set("DecodeParms", parms);
        (content, old_len)
    } else {
        let opts = EncodeOptions {
            quality: target.quality.clamp(1, 100),
            ..EncodeOptions::default()
        };
        let Ok(jpeg) = io::encode(&img, OutputFormat::Jpg, &opts, None) else {
            return Outcome::Skipped("JPEG encoder failed");
        };
        new_dict.set("Filter", "DCTDecode");
        let max_len = if !resized && matches!(source, Source::Jpeg) {
            (old_len as f32 * JPEG_REENCODE_GAIN) as usize
        } else {
            old_len
        };
        (jpeg, max_len)
    };
    if content.len() >= max_len {
        return Outcome::Unchanged;
    }
    let mut s = Stream::new(new_dict, content);
    s.allows_compression = false;
    Outcome::Replaced(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Undo `/Predictor 15` (as a reader would).
    fn unpredict(data: &[u8], width: usize, bpp: usize) -> Vec<u8> {
        let stride = width * bpp;
        let mut out: Vec<u8> = Vec::new();
        for (r, row) in data.chunks(stride + 1).enumerate() {
            let (f, row) = (row[0], &row[1..]);
            let start = out.len();
            for i in 0..stride {
                let a = if i >= bpp { out[start + i - bpp] } else { 0 };
                let b = if r > 0 { out[start + i - stride] } else { 0 };
                let c = if r > 0 && i >= bpp {
                    out[start + i - stride - bpp]
                } else {
                    0
                };
                let p = match f {
                    0 => 0,
                    1 => a,
                    2 => b,
                    3 => ((u16::from(a) + u16::from(b)) / 2) as u8,
                    _ => paeth(a, b, c),
                };
                out.push(row[i].wrapping_add(p));
            }
        }
        out
    }

    #[test]
    fn prediction_round_trips() {
        let (w, h, bpp) = (37usize, 11usize, 3usize);
        let raw: Vec<u8> = (0..w * h * bpp)
            .map(|i| ((i * 7919) % 251) as u8 ^ (i / 13) as u8)
            .collect();
        let p = predict(&raw, w, bpp);
        assert_eq!(p.len(), raw.len() + h);
        assert_eq!(unpredict(&p, w, bpp), raw);
    }

    #[test]
    fn jpeg_header_components() {
        let img = DynamicImage::ImageRgb8(RgbImage::from_pixel(8, 8, image::Rgb([1, 2, 3])));
        let jpg = io::encode(&img, OutputFormat::Jpg, &EncodeOptions::default(), None).unwrap();
        assert_eq!(jpeg_components(&jpg), Some(3));
        let gray = DynamicImage::ImageLuma8(GrayImage::from_pixel(8, 8, image::Luma([9])));
        let jpg = io::encode(&gray, OutputFormat::Jpg, &EncodeOptions::default(), None).unwrap();
        assert_eq!(jpeg_components(&jpg), Some(1));
        assert_eq!(jpeg_components(b"not a jpeg"), None);
    }

    #[test]
    fn graphic_detection() {
        let flat = DynamicImage::ImageRgb8(RgbImage::from_fn(64, 64, |x, _| {
            image::Rgb(if x < 32 { [255, 0, 0] } else { [0, 0, 255] })
        }));
        assert!(looks_like_graphic(&flat));
        let mut seed = 1u32;
        let noisy = DynamicImage::ImageRgb8(RgbImage::from_fn(64, 64, |_, _| {
            seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            let [r, g, b, _] = (seed >> 8).to_le_bytes();
            image::Rgb([r, g, b])
        }));
        assert!(!looks_like_graphic(&noisy));
    }
}
