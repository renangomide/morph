// SPDX-License-Identifier: GPL-3.0-or-later
//! Image decoding/encoding and metadata helpers.
//!
//! **The public signatures in this file are a frozen contract** used by the
//! image actions. Implementations may change freely; signatures only together
//! with every caller.
//!
//! Engines (SPEC §7): `image` (JPEG/PNG/GIF/BMP/TIFF/TGA/QOI/WebP decode,
//! BMP/TIFF/GIF encode), libheif via vcpkg (HEIC/HEIF + AVIF decode), resvg (SVG),
//! `ico` (ICO read/write), mozjpeg (JPEG), libwebp (WebP), ravif (AVIF),
//! oxipng + imagequant (PNG), img-parts + our own TIFF/RIFF edits (metadata).

mod container;
mod decode;
mod encode;
mod exif_edit;
mod heif;
mod isobmff;
mod xmp;

use crate::error::ConvertError;
use crate::image::options::OutputFormat;
use image::DynamicImage;
use std::path::Path;

/// Metadata blocks carried from input to output (raw bytes).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Metadata {
    /// Raw EXIF (TIFF header onwards, no "Exif\0\0" prefix).
    pub exif: Option<Vec<u8>>,
    /// ICC color profile.
    pub icc: Option<Vec<u8>>,
    /// XMP packet (UTF-8 XML).
    pub xmp: Option<Vec<u8>>,
}

/// A decoded input image.
#[derive(Debug, Clone)]
pub struct Decoded {
    /// Pixels as stored in the file: EXIF orientation is NOT applied yet.
    pub image: DynamicImage,
    /// EXIF orientation 1–8 (1 = upright). Use [`apply_orientation`].
    pub orientation: u8,
    pub meta: Metadata,
    /// The input had several frames (animated GIF/WebP); only the first was decoded.
    pub animated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodeOptions {
    /// Output width for SVG input (height follows the aspect ratio).
    pub svg_width: u32,
}

impl Default for DecodeOptions {
    fn default() -> Self {
        DecodeOptions { svg_width: 1024 }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncodeOptions {
    /// 1–100 for JPG/WebP/AVIF; 0 = the format's default quality.
    pub quality: u8,
    /// WebP/AVIF lossless.
    pub lossless: bool,
    /// Fill for transparent pixels when the format has no alpha (JPG, BMP). RGB.
    pub background: [u8; 3],
    /// ICO sizes (square; non-square images are padded).
    pub ico_sizes: Vec<u32>,
    /// PNG lossless optimization level 0–6 (oxipng); 0 = fastest.
    pub png_level: u8,
    /// PNG lossy palette quantization (imagequant).
    pub png_lossy: bool,
    /// JPG: full-resolution colour (4:4:4) instead of the usual 4:2:0. Keeps
    /// saturated colour edges (e.g. red text on blue) sharp; bigger files.
    pub chroma_444: bool,
    /// JPG: a quick trial encode with the same pixels as the final one, but
    /// a larger file (baseline, standard Huffman tables). For quality searches.
    pub draft: bool,
}

impl Default for EncodeOptions {
    fn default() -> Self {
        EncodeOptions {
            quality: 0,
            lossless: false,
            background: [255, 255, 255],
            ico_sizes: crate::image::options::DEFAULT_ICO_SIZES.to_vec(),
            png_level: 2,
            png_lossy: false,
            chroma_444: false,
            draft: false,
        }
    }
}

/// The JPEG's colour is stored at full resolution (4:4:4, or grayscale), read
/// from the frame header. `None` if `bytes` isn't a readable JPEG.
pub fn jpeg_is_444(bytes: &[u8]) -> Option<bool> {
    if bytes.get(0..2)? != [0xFF, 0xD8] {
        return None;
    }
    let mut i = 2;
    loop {
        // Skip fill bytes before a marker.
        while *bytes.get(i)? == 0xFF && *bytes.get(i + 1)? == 0xFF {
            i += 1;
        }
        if *bytes.get(i)? != 0xFF {
            return None;
        }
        let marker = *bytes.get(i + 1)?;
        let len = usize::from(u16::from_be_bytes([*bytes.get(i + 2)?, *bytes.get(i + 3)?]));
        // SOF0–SOF15 except DHT (C4), JPG (C8) and DAC (CC).
        if (0xC0..=0xCF).contains(&marker) && !matches!(marker, 0xC4 | 0xC8 | 0xCC) {
            let components = usize::from(*bytes.get(i + 9)?);
            let factors: Vec<u8> = (0..components)
                .map(|c| bytes.get(i + 11 + c * 3).copied())
                .collect::<Option<_>>()?;
            return Some(factors.iter().all(|&f| f == factors[0]));
        }
        if marker == 0xDA || len < 2 {
            return None;
        }
        i += 2 + len;
    }
}

/// Read any supported input (SPEC §4.1 input list). Only the first frame of
/// animations; the largest image of an ICO; SVG rasterized at `svg_width`.
///
/// The format is detected from the content (the extension is only a fallback).
/// HEIF/AVIF pixels come out already transformed by libheif, so their
/// `orientation` is 1.
pub fn decode(path: &Path, opts: &DecodeOptions) -> Result<Decoded, ConvertError> {
    let data = std::fs::read(path)?;
    decode_bytes(&data, Some(path), opts)
}

/// Like [`decode`], for a file already in memory. `path` (optional) gives the
/// extension fallback and the base folder for SVG resources.
pub fn decode_bytes(
    data: &[u8],
    path: Option<&Path>,
    opts: &DecodeOptions,
) -> Result<Decoded, ConvertError> {
    guarded(|| decode::decode_bytes(data, path, opts), "decode")
}

/// Run an engine call, turning a panic inside it into a task error.
fn guarded<T>(f: impl FnOnce() -> Result<T, ConvertError>, what: &str) -> Result<T, ConvertError> {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
        Ok(r) => r,
        Err(p) => {
            let msg = p
                .downcast_ref::<&str>()
                .map(|s| s.to_string())
                .or_else(|| p.downcast_ref::<String>().cloned())
                .unwrap_or_default();
            Err(ConvertError::other(
                "The image engine failed on this file.",
                format!("{what} panicked: {msg}"),
            ))
        }
    }
}

/// Turn pixels upright according to an EXIF orientation value (1–8).
pub fn apply_orientation(img: DynamicImage, orientation: u8) -> DynamicImage {
    match orientation {
        2 => img.fliph(),
        3 => img.rotate180(),
        4 => img.flipv(),
        5 => img.rotate90().fliph(),
        6 => img.rotate90(),
        7 => img.rotate270().fliph(),
        8 => img.rotate270(),
        _ => img,
    }
}

/// Encode to bytes. `meta` is embedded where the format supports it (best effort);
/// any EXIF orientation tag (and XMP `tiff:Orientation`) written is reset to 1,
/// since pixels are upright. An ICC profile that isn't valid or doesn't match
/// the written colour space (e.g. a CMYK or gray profile on RGB) is dropped.
/// WebP output larger than [`WEBP_MAX_SIDE`] on a side is an error.
pub fn encode(
    img: &DynamicImage,
    format: OutputFormat,
    opts: &EncodeOptions,
    meta: Option<&Metadata>,
) -> Result<Vec<u8>, ConvertError> {
    guarded(|| encode::encode(img, format, opts, meta), "encode")
}

/// Composite onto an opaque background color.
pub fn flatten(img: &DynamicImage, background: [u8; 3]) -> image::RgbImage {
    if !img.color().has_alpha() {
        return img.to_rgb8();
    }
    let rgba = img.to_rgba8();
    let mut out = image::RgbImage::new(rgba.width(), rgba.height());
    for (dst, src) in out.pixels_mut().zip(rgba.pixels()) {
        let a = u16::from(src[3]);
        for c in 0..3 {
            let v = (u16::from(src[c]) * a + u16::from(background[c]) * (255 - a)) / 255;
            dst[c] = v as u8;
        }
    }
    out
}

/// EXIF without GPS information. `None` if the EXIF can't be parsed.
/// Accepts EXIF with or without the "Exif\0\0" prefix; returns it without.
/// The GPS data is zeroed and unlinked in place, so all other tags (and their
/// offsets, maker notes included) are preserved byte for byte, **including the
/// orientation tag** (clean relies on it; it is not reset here).
pub fn strip_gps(exif: &[u8]) -> Option<Vec<u8>> {
    exif_edit::strip_gps(exif)
}

/// XMP without GPS location: every `GPS*` property of the EXIF namespace
/// (`http://ns.adobe.com/exif/1.0/`, any prefix; also the CIPA `exifEX` one),
/// in attribute or element form, is removed; the rest is kept byte for byte.
/// `None` if the XMP can't be parsed: the caller must then drop it entirely,
/// so location data never survives by accident.
pub fn strip_xmp_gps(xmp: &[u8]) -> Option<Vec<u8>> {
    xmp::strip_gps(xmp)
}

/// Pixel size (width, height) of an input file, read from its header only (no
/// pixel decoding; cheap). Covers every input format. Values are as stored:
/// the EXIF orientation is not applied (HEIF/AVIF: after the file's own
/// rotation). SVG: the raster size at the default width
/// ([`DecodeOptions::default`]). ICO: the entry [`decode`] would pick.
pub fn probe_dimensions(path: &Path) -> Result<(u32, u32), ConvertError> {
    guarded(
        || decode::probe(path, &DecodeOptions::default()),
        "probe dimensions",
    )
}

/// Like [`probe_dimensions`], with the SVG raster width from `opts`.
pub fn probe_dimensions_with(
    path: &Path,
    opts: &DecodeOptions,
) -> Result<(u32, u32), ConvertError> {
    guarded(|| decode::probe(path, opts), "probe dimensions")
}

/// WebP can't be larger than this on either side.
pub const WEBP_MAX_SIDE: u32 = 16383;

/// Error for images too large for WebP output.
pub fn webp_too_large(w: u32, h: u32) -> ConvertError {
    ConvertError::unsupported(
        format!(
            "This image is too large for WebP ({w} × {h}); WebP allows at most {WEBP_MAX_SIDE} × {WEBP_MAX_SIDE} pixels."
        ),
        format!("{w}x{h}"),
    )
}

/// What [`rewrite_metadata_with`] does with metadata other than EXIF/ICC/XMP.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OtherMetadata {
    /// Keep comments, IPTC, PNG text chunks… as they are.
    Keep,
    /// Remove what may hold a location: JPEG APP13 (IPTC/Photoshop) and MPF extra
    /// images (each has its own EXIF); PNG "Raw profile type …" text chunks.
    DropGps,
    /// Remove all of it. JPEG keeps only JFIF (APP0), Adobe (APP14) and the
    /// blocks in `meta`; PNG drops every tEXt/zTXt/iTXt and tIME chunk.
    DropAll,
}

/// Replace only the metadata of an already-encoded file (JPEG/PNG/WebP) without
/// re-encoding pixels. `Ok(None)` if this container isn't supported, so the caller
/// must re-encode instead. Blocks set to `None` in `meta` are removed. Other
/// metadata (comments, IPTC, PNG text…) is kept: see [`rewrite_metadata_with`].
///
/// The EXIF is written as given (orientation is NOT reset: the pixels in the
/// file are untouched). If `bytes` isn't really a file of `format` (e.g. a PNG
/// named .jpg), `Ok(None)` is returned too. An ICC profile that isn't valid or
/// doesn't match the file's colour space (e.g. CMYK on RGB) is not written.
pub fn rewrite_metadata(
    bytes: &[u8],
    format: OutputFormat,
    meta: &Metadata,
) -> Result<Option<Vec<u8>>, ConvertError> {
    rewrite_metadata_with(bytes, format, meta, OtherMetadata::Keep)
}

/// [`rewrite_metadata`], also removing other metadata as `other` says.
pub fn rewrite_metadata_with(
    bytes: &[u8],
    format: OutputFormat,
    meta: &Metadata,
    other: OtherMetadata,
) -> Result<Option<Vec<u8>>, ConvertError> {
    let meta = Metadata {
        exif: meta
            .exif
            .as_deref()
            .map(|e| exif_edit::without_prefix(e).to_vec()),
        icc: meta.icc.clone(),
        xmp: meta.xmp.clone(),
    };
    let is_webp = bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP";
    guarded(
        || match format {
            OutputFormat::Jpg if bytes.starts_with(&[0xFF, 0xD8]) => {
                container::rewrite_jpeg(bytes, &meta, other).map(Some)
            }
            OutputFormat::Png if bytes.starts_with(b"\x89PNG\r\n\x1a\n") => {
                container::rewrite_png(bytes, &meta, other).map(Some)
            }
            OutputFormat::Webp if is_webp => container::rewrite_webp(bytes, &meta, other).map(Some),
            _ => Ok(None),
        },
        "metadata rewrite",
    )
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[test]
    fn orientation_6_rotates_to_portrait() {
        let img = DynamicImage::new_rgb8(40, 20);
        let upright = apply_orientation(img, 6);
        assert_eq!((upright.width(), upright.height()), (20, 40));
    }

    #[test]
    fn flatten_uses_background_for_transparent_pixels() {
        let img = DynamicImage::new_rgba8(2, 2); // fully transparent
        let flat = flatten(&img, [255, 0, 0]);
        assert_eq!(flat.get_pixel(0, 0).0, [255, 0, 0]);
    }

    use crate::error::ErrorKind;
    use exif_edit::test_data::sample_exif;
    use std::path::PathBuf;

    /// A smooth gradient with some detail (so quality settings matter).
    fn sample(w: u32, h: u32, alpha: bool) -> DynamicImage {
        let img = image::RgbaImage::from_fn(w, h, |x, y| {
            let n = ((x * 7919 + y * 104729) % 37) as u8;
            image::Rgba([
                (x * 255 / w.max(1)) as u8 ^ n,
                (y * 255 / h.max(1)) as u8,
                ((x + y) % 256) as u8,
                if alpha { ((x * 3) % 256) as u8 } else { 255 },
            ])
        });
        if alpha {
            DynamicImage::ImageRgba8(img)
        } else {
            DynamicImage::ImageRgb8(DynamicImage::ImageRgba8(img).to_rgb8())
        }
    }

    fn enc(
        img: &DynamicImage,
        f: OutputFormat,
        o: &EncodeOptions,
        m: Option<&Metadata>,
    ) -> Vec<u8> {
        match encode(img, f, o, m) {
            Ok(b) => b,
            Err(e) => panic!("encode {f:?}: {e}"),
        }
    }

    fn dec(bytes: &[u8], ext: &str) -> Decoded {
        let p = PathBuf::from(format!("x.{ext}"));
        match decode_bytes(bytes, Some(&p), &DecodeOptions::default()) {
            Ok(d) => d,
            Err(e) => panic!("decode .{ext}: {e}"),
        }
    }

    fn sample_meta(orientation: u16) -> Metadata {
        Metadata {
            exif: Some(sample_exif(orientation)),
            icc: Some(fake_icc(b"RGB ")),
            xmp: Some(b"<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"></x:xmpmeta>".to_vec()),
        }
    }

    /// A minimal ICC profile header for colour space `space` (e.g. b"RGB ").
    pub(crate) fn fake_icc(space: &[u8; 4]) -> Vec<u8> {
        let mut p = vec![0u8; 132];
        p[0..4].copy_from_slice(&132u32.to_be_bytes());
        p[12..16].copy_from_slice(b"mntr");
        p[16..20].copy_from_slice(space);
        p[20..24].copy_from_slice(b"XYZ ");
        p[36..40].copy_from_slice(b"acsp");
        p
    }

    #[test]
    fn icc_dropped_when_colour_space_does_not_match() {
        let rgb = sample(24, 16, false);
        let gray = DynamicImage::ImageLuma8(rgb.to_luma8());
        let with = |icc: Vec<u8>| Metadata {
            icc: Some(icc),
            ..Metadata::default()
        };
        let o = EncodeOptions::default();
        for f in [
            OutputFormat::Jpg,
            OutputFormat::Png,
            OutputFormat::Webp,
            OutputFormat::Avif,
            OutputFormat::Tiff,
        ] {
            let icc_of = |img: &DynamicImage, icc: Vec<u8>| {
                dec(&enc(img, f, &o, Some(&with(icc))), f.extension())
                    .meta
                    .icc
            };
            let rgb_icc = fake_icc(b"RGB ");
            assert_eq!(
                icc_of(&rgb, rgb_icc.clone()),
                Some(rgb_icc),
                "{f:?} RGB kept"
            );
            assert_eq!(icc_of(&rgb, fake_icc(b"CMYK")), None, "{f:?} CMYK on RGB");
            assert_eq!(icc_of(&rgb, fake_icc(b"GRAY")), None, "{f:?} gray on RGB");
            assert_eq!(icc_of(&rgb, b"not a profile".to_vec()), None, "{f:?} junk");
            // Gray pixels stay gray in JPG/TIFF (and PNG); WebP/AVIF are always RGB.
            let gray_icc = fake_icc(b"GRAY");
            let expect = matches!(
                f,
                OutputFormat::Jpg | OutputFormat::Png | OutputFormat::Tiff
            )
            .then(|| gray_icc.clone());
            assert_eq!(icc_of(&gray, gray_icc), expect, "{f:?} gray on gray");
        }
    }

    #[test]
    fn xmp_orientation_reset_on_encode() {
        let xmp = br#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><rdf:Description xmlns:tiff="http://ns.adobe.com/tiff/1.0/" tiff:Orientation="6"/></rdf:RDF></x:xmpmeta>"#;
        let meta = Metadata {
            xmp: Some(xmp.to_vec()),
            ..Metadata::default()
        };
        let img = sample(20, 10, false);
        for f in [OutputFormat::Jpg, OutputFormat::Png, OutputFormat::Webp] {
            let d = dec(
                &enc(&img, f, &EncodeOptions::default(), Some(&meta)),
                f.extension(),
            );
            let out = String::from_utf8(d.meta.xmp.unwrap_or_default()).unwrap_or_default();
            assert!(out.contains("tiff:Orientation=\"1\""), "{f:?}: {out}");
        }
        // A metadata-only rewrite leaves it alone (pixels are untouched).
        let jpg = enc(&img, OutputFormat::Jpg, &EncodeOptions::default(), None);
        let Ok(Some(re)) = rewrite_metadata(&jpg, OutputFormat::Jpg, &meta) else {
            panic!("rewrite")
        };
        assert_eq!(dec(&re, "jpg").meta.xmp.as_deref(), Some(&xmp[..]));
    }

    #[test]
    fn probe_matches_decode_for_every_format() {
        let dir = match tempfile::tempdir() {
            Ok(d) => d,
            Err(e) => panic!("{e}"),
        };
        let img = sample(37, 21, true);
        let mut files = Vec::new();
        for f in OutputFormat::ALL {
            let p = dir.path().join(format!("x.{}", f.extension()));
            let o = EncodeOptions {
                ico_sizes: vec![16, 32],
                ..EncodeOptions::default()
            };
            if let Err(e) = std::fs::write(&p, enc(&img, f, &o, None)) {
                panic!("{e}");
            }
            files.push(p);
        }
        for (fmt, ext) in [
            (image::ImageFormat::Tga, "tga"),
            (image::ImageFormat::Qoi, "qoi"),
        ] {
            let p = dir.path().join(format!("x.{ext}"));
            if let Err(e) = img.save_with_format(&p, fmt) {
                panic!("{e}");
            }
            files.push(p);
        }
        let svg = dir.path().join("x.svg");
        let svg_src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="300" height="100"/>"#;
        if let Err(e) = std::fs::write(&svg, svg_src) {
            panic!("{e}");
        }
        files.push(svg.clone());
        for p in &files {
            let probed = match probe_dimensions(p) {
                Ok(d) => d,
                Err(e) => panic!("{}: {e}", p.display()),
            };
            let decoded = match decode(p, &DecodeOptions::default()) {
                Ok(d) => d.image.dimensions(),
                Err(e) => panic!("{}: {e}", p.display()),
            };
            assert_eq!(probed, decoded, "{}", p.display());
        }
        assert_eq!(probe_dimensions(&svg).ok(), Some((1024, 341)));
        let wide = DecodeOptions { svg_width: 600 };
        assert_eq!(probe_dimensions_with(&svg, &wide).ok(), Some((600, 200)));
        let missing = dir.path().join("missing.png");
        assert!(probe_dimensions(&missing).is_err());
    }

    /// A PNG that claims `w`×`h` (header only, no pixel data).
    pub(crate) fn png_header_only(w: u32, h: u32) -> Vec<u8> {
        let mut ihdr = w.to_be_bytes().to_vec();
        ihdr.extend_from_slice(&h.to_be_bytes());
        ihdr.extend_from_slice(&[8, 2, 0, 0, 0]);
        let chunk = img_parts::png::PngChunk::new(*b"IHDR", img_parts::Bytes::from(ihdr));
        let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
        out.extend_from_slice(&chunk.encoder().bytes());
        let idat = img_parts::png::PngChunk::new(*b"IDAT", img_parts::Bytes::new());
        out.extend_from_slice(&idat.encoder().bytes());
        let iend = img_parts::png::PngChunk::new(*b"IEND", img_parts::Bytes::new());
        out.extend_from_slice(&iend.encoder().bytes());
        out
    }

    #[test]
    fn huge_images_are_refused_before_decoding() {
        let dir = match tempfile::tempdir() {
            Ok(d) => d,
            Err(e) => panic!("{e}"),
        };
        let p = dir.path().join("huge.png");
        if let Err(e) = std::fs::write(&p, png_header_only(40000, 30000)) {
            panic!("{e}");
        }
        assert_eq!(probe_dimensions(&p).ok(), Some((40000, 30000)));
        match decode(&p, &DecodeOptions::default()) {
            Err(e) => {
                assert_eq!(e.kind, ErrorKind::Unsupported, "{e}");
                assert_eq!(
                    e.message,
                    "This image is too large to process (40000 × 30000)."
                );
            }
            Ok(_) => panic!("decoded a 1.2 GP image"),
        }
    }

    /// Our AVIF with an ICC profile still opens in another decoder (ImageMagick),
    /// which sees the profile.
    #[test]
    fn avif_with_icc_opens_in_magick() {
        let dir = match tempfile::tempdir() {
            Ok(d) => d,
            Err(e) => panic!("{e}"),
        };
        let icc = fake_icc(b"RGB ");
        let meta = Metadata {
            icc: Some(icc.clone()),
            exif: Some(sample_exif(1)),
            ..Metadata::default()
        };
        let bytes = enc(
            &sample(30, 20, true),
            OutputFormat::Avif,
            &EncodeOptions::default(),
            Some(&meta),
        );
        let d = dec(&bytes, "avif");
        assert_eq!(d.meta.icc, Some(icc));
        assert!(d.meta.exif.is_some(), "EXIF item still found");
        let p = dir.path().join("icc.avif");
        if let Err(e) = std::fs::write(&p, &bytes) {
            panic!("{e}");
        }
        let out = std::process::Command::new("magick")
            .args(["identify", "-format", "%w %h [%[profiles]]"])
            .arg(&p)
            .output();
        match out {
            Ok(o) if o.status.success() => {
                let s = String::from_utf8_lossy(&o.stdout);
                assert!(s.starts_with("30 20"), "{s}");
                assert!(s.contains("icc"), "magick sees no ICC: {s}");
            }
            Ok(o) => panic!("magick failed: {}", String::from_utf8_lossy(&o.stderr)),
            Err(_) => println!("note: skipping magick AVIF check (magick not available)"),
        }
    }

    #[test]
    fn webp_size_limit_is_a_clear_error() {
        let img = DynamicImage::new_rgb8(16384, 2);
        match encode(&img, OutputFormat::Webp, &EncodeOptions::default(), None) {
            Err(e) => assert!(e.message.contains("too large for WebP"), "{e}"),
            Ok(_) => panic!("WebP wider than 16383"),
        }
    }

    #[test]
    fn round_trip_every_output_format() {
        for alpha in [false, true] {
            let img = sample(70, 40, alpha);
            for f in OutputFormat::ALL {
                let bytes = enc(&img, f, &EncodeOptions::default(), None);
                let d = dec(&bytes, f.extension());
                let expect = if f == OutputFormat::Ico {
                    (256, 256)
                } else {
                    (70, 40)
                };
                assert_eq!(d.image.dimensions(), expect, "{f:?} alpha={alpha}");
                if alpha && f.supports_alpha() {
                    assert!(d.image.color().has_alpha(), "{f:?} lost alpha");
                }
                // Detected from content, not extension.
                let d2 = dec(&bytes, "bin");
                assert_eq!(d2.image.dimensions(), expect, "{f:?} by content");
            }
        }
    }

    use image::GenericImageView;

    #[test]
    fn lossless_webp_and_png_are_exact() {
        let img = sample(33, 17, true);
        let o = EncodeOptions {
            lossless: true,
            ..EncodeOptions::default()
        };
        for f in [OutputFormat::Webp, OutputFormat::Png] {
            let d = dec(&enc(&img, f, &o, None), f.extension());
            assert!(d.image.to_rgba8() == img.to_rgba8(), "{f:?} not exact");
        }
    }

    #[test]
    fn png_lossy_and_levels() {
        let img = sample(64, 64, true);
        for level in [0, 6] {
            for lossy in [false, true] {
                let o = EncodeOptions {
                    png_level: level,
                    png_lossy: lossy,
                    ..EncodeOptions::default()
                };
                let d = dec(&enc(&img, OutputFormat::Png, &o, None), "png");
                assert_eq!(d.image.dimensions(), (64, 64));
            }
        }
    }

    #[test]
    fn quality_affects_size() {
        let img = sample(160, 120, false);
        for f in [OutputFormat::Jpg, OutputFormat::Webp, OutputFormat::Avif] {
            let at = |q| {
                enc(
                    &img,
                    f,
                    &EncodeOptions {
                        quality: q,
                        ..EncodeOptions::default()
                    },
                    None,
                )
                .len()
            };
            let (lo, hi) = (at(20), at(95));
            assert!(lo < hi, "{f:?}: q20 {lo} B should be < q95 {hi} B");
            // 0 means the format default.
            assert_eq!(at(0), at(f.default_quality()), "{f:?} default quality");
        }
    }

    #[test]
    fn jpeg_flattens_onto_background() {
        let img = DynamicImage::new_rgba8(16, 16);
        let o = EncodeOptions {
            background: [0, 0, 255],
            ..EncodeOptions::default()
        };
        let d = dec(&enc(&img, OutputFormat::Jpg, &o, None), "jpg");
        let p = d.image.to_rgb8().get_pixel(8, 8).0;
        assert!(p[2] > 240 && p[0] < 15 && p[1] < 15, "{p:?}");
    }

    #[test]
    fn metadata_embedded_and_orientation_reset() {
        let img = sample(40, 30, false);
        let meta = sample_meta(6);
        for f in [OutputFormat::Jpg, OutputFormat::Png, OutputFormat::Webp] {
            let d = dec(
                &enc(&img, f, &EncodeOptions::default(), Some(&meta)),
                f.extension(),
            );
            assert_eq!(d.orientation, 1, "{f:?}");
            let exif = d.meta.exif.unwrap_or_default();
            assert_eq!(exif_edit::orientation(&exif), Some(1), "{f:?}");
            assert_eq!(d.meta.icc, meta.icc, "{f:?} icc");
            assert_eq!(d.meta.xmp, meta.xmp, "{f:?} xmp");
        }
    }

    #[test]
    fn decode_reads_orientation() {
        let img = sample(40, 30, false);
        let jpg = enc(&img, OutputFormat::Jpg, &EncodeOptions::default(), None);
        let meta = Metadata {
            exif: Some(sample_exif(6)),
            ..Metadata::default()
        };
        let rewritten = match rewrite_metadata(&jpg, OutputFormat::Jpg, &meta) {
            Ok(Some(b)) => b,
            other => panic!("{other:?}"),
        };
        let d = dec(&rewritten, "jpg");
        assert_eq!(d.orientation, 6);
        assert_eq!(
            apply_orientation(d.image, d.orientation).dimensions(),
            (30, 40)
        );
    }

    #[test]
    fn rewrite_metadata_removes_exif_without_reencoding() {
        let img = sample(48, 32, false);
        let jpg = enc(
            &img,
            OutputFormat::Jpg,
            &EncodeOptions::default(),
            Some(&sample_meta(1)),
        );
        assert!(dec(&jpg, "jpg").meta.exif.is_some());
        let stripped = match rewrite_metadata(&jpg, OutputFormat::Jpg, &Metadata::default()) {
            Ok(Some(b)) => b,
            other => panic!("{other:?}"),
        };
        let d = dec(&stripped, "jpg");
        assert_eq!(d.meta, Metadata::default());
        assert_eq!(d.image.to_rgb8(), dec(&jpg, "jpg").image.to_rgb8());
        // Compressed data (from the first SOS marker on) is byte-identical.
        let sos = |b: &[u8]| {
            b.windows(2)
                .position(|w| w == [0xFF, 0xDA])
                .map(|p| b[p..].to_vec())
        };
        assert_eq!(sos(&stripped), sos(&jpg));
        assert!(stripped.len() < jpg.len());
    }

    #[test]
    fn rewrite_metadata_png_and_webp() {
        let img = sample(20, 20, true);
        for f in [OutputFormat::Png, OutputFormat::Webp] {
            let plain = enc(&img, f, &EncodeOptions::default(), None);
            let meta = sample_meta(3);
            let with = match rewrite_metadata(&plain, f, &meta) {
                Ok(Some(b)) => b,
                other => panic!("{f:?}: {other:?}"),
            };
            let d = dec(&with, f.extension());
            assert_eq!(d.meta, meta, "{f:?}");
            assert_eq!(d.orientation, 3, "{f:?}: rewrite keeps orientation");
            assert_eq!(
                d.image.to_rgba8(),
                dec(&plain, f.extension()).image.to_rgba8()
            );
            let back = match rewrite_metadata(&with, f, &Metadata::default()) {
                Ok(Some(b)) => b,
                other => panic!("{f:?}: {other:?}"),
            };
            assert_eq!(dec(&back, f.extension()).meta, Metadata::default(), "{f:?}");
        }
        // Unsupported container / mismatched content.
        let png = enc(&img, OutputFormat::Png, &EncodeOptions::default(), None);
        assert_eq!(
            rewrite_metadata(&png, OutputFormat::Jpg, &Metadata::default()),
            Ok(None)
        );
        assert_eq!(
            rewrite_metadata(&png, OutputFormat::Bmp, &Metadata::default()),
            Ok(None)
        );
    }

    #[test]
    fn strip_gps_public_api_keeps_orientation() {
        let Some(s) = strip_gps(&sample_exif(6)) else {
            panic!("strip_gps failed")
        };
        assert_eq!(exif_edit::orientation(&s), Some(6));
        let e = match ::exif::Reader::new().read_raw(s) {
            Ok(e) => e,
            Err(e) => panic!("{e}"),
        };
        assert!(
            e.get_field(::exif::Tag::GPSLatitude, ::exif::In::PRIMARY)
                .is_none()
        );
        assert!(
            e.get_field(::exif::Tag::Make, ::exif::In::PRIMARY)
                .is_some()
        );
        assert_eq!(strip_gps(b"garbage"), None);
    }

    #[test]
    fn ico_sizes_padding_and_rgba_entries() {
        // Opaque, non-square input: every entry must still be RGBA and square.
        let img = sample(100, 50, false);
        let o = EncodeOptions {
            ico_sizes: vec![16, 48, 256, 999],
            ..EncodeOptions::default()
        };
        let bytes = enc(&img, OutputFormat::Ico, &o, None);
        let dir = match ico::IconDir::read(std::io::Cursor::new(&bytes)) {
            Ok(d) => d,
            Err(e) => panic!("{e}"),
        };
        let sizes: Vec<(u32, u32, u16)> = dir
            .entries()
            .iter()
            .map(|e| (e.width(), e.height(), e.bits_per_pixel()))
            .collect();
        assert_eq!(sizes, vec![(16, 16, 32), (48, 48, 32), (256, 256, 32)]);
        // The image crate (which can't read RGB PNG entries) decodes it.
        let via_image = match image::load_from_memory_with_format(&bytes, image::ImageFormat::Ico) {
            Ok(i) => i,
            Err(e) => panic!("image crate: {e}"),
        };
        assert_eq!(via_image.dimensions(), (256, 256));
        // Padding is transparent (top rows), content is opaque (middle).
        let rgba = via_image.to_rgba8();
        assert_eq!(rgba.get_pixel(128, 2)[3], 0);
        assert_eq!(rgba.get_pixel(128, 128)[3], 255);
        // Default sizes from any input decode as 256 × 256.
        let d = dec(
            &enc(&img, OutputFormat::Ico, &EncodeOptions::default(), None),
            "ico",
        );
        assert_eq!(d.image.dimensions(), (256, 256));
        // Small BMP entries decode with the image crate too.
        for e in dir.entries() {
            match e.decode() {
                Ok(i) => assert_eq!(i.width(), e.width()),
                Err(err) => panic!("entry {}: {err}", e.width()),
            }
        }
    }

    #[test]
    fn animated_gif_first_frame() {
        let mut bytes = Vec::new();
        {
            let mut g = image::codecs::gif::GifEncoder::new(&mut bytes);
            let frames = (0..2).map(|i| {
                image::Frame::new(image::RgbaImage::from_pixel(
                    8,
                    6,
                    image::Rgba([i * 200, 0, 0, 255]),
                ))
            });
            if let Err(e) = g.encode_frames(frames) {
                panic!("{e}");
            }
        }
        let d = dec(&bytes, "gif");
        assert!(d.animated);
        assert_eq!(d.image.dimensions(), (8, 6));
        assert_eq!(d.image.to_rgba8().get_pixel(0, 0)[0], 0);
        let single = enc(
            &sample(8, 6, false),
            OutputFormat::Gif,
            &EncodeOptions::default(),
            None,
        );
        assert!(!dec(&single, "gif").animated);
    }

    #[test]
    fn tga_qoi_and_svg_inputs() {
        let img = sample(12, 9, true);
        for (fmt, ext) in [
            (image::ImageFormat::Tga, "tga"),
            (image::ImageFormat::Qoi, "qoi"),
        ] {
            let mut c = std::io::Cursor::new(Vec::new());
            if let Err(e) = img.write_to(&mut c, fmt) {
                panic!("{e}");
            }
            assert_eq!(dec(c.get_ref(), ext).image.dimensions(), (12, 9), "{ext}");
        }
        let svg = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="200" height="100" viewBox="0 0 200 100">
  <rect x="0" y="0" width="100" height="100" fill="#ff0000"/>
</svg>"##;
        let p = PathBuf::from("drawing.svg");
        let d = match decode_bytes(svg, Some(&p), &DecodeOptions { svg_width: 400 }) {
            Ok(d) => d,
            Err(e) => panic!("{e}"),
        };
        assert_eq!(d.image.dimensions(), (400, 200));
        let rgba = d.image.to_rgba8();
        assert_eq!(rgba.get_pixel(50, 100).0, [255, 0, 0, 255]);
        assert_eq!(rgba.get_pixel(350, 100)[3], 0);
    }

    #[test]
    fn damaged_input_is_bad_input() {
        for (bytes, ext) in [
            (&b""[..], "jpg"),
            (&b"hello, not an image"[..], "png"),
            (&b"\xFF\xD8\xFF\xE0garbage"[..], "jpg"),
            (&b"\x89PNG\r\n\x1a\nbroken"[..], "png"),
        ] {
            let p = PathBuf::from(format!("x.{ext}"));
            match decode_bytes(bytes, Some(&p), &DecodeOptions::default()) {
                Err(e) => assert_eq!(e.kind, ErrorKind::BadInput, "{e}"),
                Ok(_) => panic!("decoded garbage as .{ext}"),
            }
        }
    }

    /// Run `magick` to make a test input; `None` (test skipped) if it isn't available.
    fn magick(args: &[&str], out: &std::path::Path) -> Option<Vec<u8>> {
        let status = std::process::Command::new("magick")
            .args(args)
            .arg(out)
            .status()
            .ok()?;
        if !status.success() {
            return None;
        }
        std::fs::read(out).ok()
    }

    #[test]
    fn avif_decode_own_output_with_exif() {
        let img = sample(41, 23, true);
        let bytes = enc(
            &img,
            OutputFormat::Avif,
            &EncodeOptions::default(),
            Some(&sample_meta(8)),
        );
        let d = dec(&bytes, "avif");
        assert_eq!(d.image.dimensions(), (41, 23));
        assert!(d.image.color().has_alpha());
        assert_eq!(d.orientation, 1);
        let exif = d.meta.exif.unwrap_or_default();
        assert_eq!(exif_edit::orientation(&exif), Some(1));
        // Lossless (near-lossless) mode stays very close.
        let o = EncodeOptions {
            lossless: true,
            ..EncodeOptions::default()
        };
        let d = dec(&enc(&img, OutputFormat::Avif, &o, None), "avif");
        let (a, b) = (d.image.to_rgba8(), img.to_rgba8());
        let max_diff = a
            .as_raw()
            .iter()
            .zip(b.as_raw())
            .map(|(x, y)| x.abs_diff(*y))
            .max()
            .unwrap_or(0);
        assert!(max_diff <= 8, "near-lossless AVIF max diff {max_diff}");
    }

    #[test]
    fn avif_from_magick() {
        let dir = match tempfile::tempdir() {
            Ok(d) => d,
            Err(e) => panic!("{e}"),
        };
        for ext in ["avif"] {
            let out = dir.path().join(format!("test.{ext}"));
            let Some(bytes) = magick(&["-size", "64x48", "gradient:red-blue"], &out) else {
                println!("note: skipping {ext} decode test (magick couldn't write {ext})");
                continue;
            };
            let d = match decode(&out, &DecodeOptions::default()) {
                Ok(d) => d,
                Err(e) => panic!("{ext}: {e}"),
            };
            assert_eq!(d.image.dimensions(), (64, 48), "{ext}");
            // By content too.
            assert_eq!(dec(&bytes, "bin").image.dimensions(), (64, 48), "{ext}");
        }
    }

    fn bx(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut v = ((8 + body.len()) as u32).to_be_bytes().to_vec();
        v.extend_from_slice(kind);
        v.extend_from_slice(body);
        v
    }

    fn full_bx(kind: &[u8; 4], version: u8, body: &[u8]) -> Vec<u8> {
        let mut b = vec![version, 0, 0, 0];
        b.extend_from_slice(body);
        bx(kind, &b)
    }

    /// Wrap one HEVC (Annex-B) intra frame into a minimal single-image HEIC file.
    fn make_heic(annexb: &[u8], w: u32, h: u32) -> Option<Vec<u8>> {
        // Split on start codes.
        let mut starts = Vec::new();
        let mut i = 0;
        while i + 3 <= annexb.len() {
            if annexb[i..i + 3] == [0, 0, 1] {
                starts.push(i + 3);
                i += 3;
            } else {
                i += 1;
            }
        }
        let mut nals: Vec<&[u8]> = Vec::new();
        for (k, &s) in starts.iter().enumerate() {
            let mut e = starts.get(k + 1).map_or(annexb.len(), |n| n - 3);
            while e > s && annexb[e - 1] == 0 {
                e -= 1;
            }
            nals.push(&annexb[s..e]);
        }
        let typ = |n: &[u8]| (n[0] >> 1) & 0x3f;
        let find = |t: u8| nals.iter().find(|n| typ(n) == t).copied();
        let (vps, sps, pps) = (find(32)?, find(33)?, find(34)?);
        // SPS RBSP without emulation prevention bytes: profile_tier_level at 3..15.
        let mut rbsp = Vec::new();
        let mut zeros = 0;
        for &b in sps {
            if zeros >= 2 && b == 3 {
                zeros = 0;
                continue;
            }
            zeros = if b == 0 { zeros + 1 } else { 0 };
            rbsp.push(b);
        }
        let ptl = rbsp.get(3..15)?;
        let mut hvcc = vec![1];
        hvcc.extend_from_slice(ptl); // profile, compat flags, constraints, level
        hvcc.extend_from_slice(&[0xF0, 0x00, 0xFC, 0xFD, 0xF8, 0xF8, 0, 0, 0x0F, 3]);
        for n in [vps, sps, pps] {
            hvcc.push(0x80 | typ(n));
            hvcc.extend_from_slice(&1u16.to_be_bytes());
            hvcc.extend_from_slice(&(n.len() as u16).to_be_bytes());
            hvcc.extend_from_slice(n);
        }
        let mut data = Vec::new();
        for n in nals.iter().filter(|n| !(32..=34).contains(&typ(n))) {
            data.extend_from_slice(&(n.len() as u32).to_be_bytes());
            data.extend_from_slice(n);
        }

        let ftyp = bx(b"ftyp", b"heic\0\0\0\0mif1heic");
        let mut hdlr = vec![0u8; 4];
        hdlr.extend_from_slice(b"pict");
        hdlr.extend_from_slice(&[0; 13]);
        let hdlr = full_bx(b"hdlr", 0, &hdlr);
        let pitm = full_bx(b"pitm", 0, &1u16.to_be_bytes());
        let mut infe = 1u16.to_be_bytes().to_vec();
        infe.extend_from_slice(&[0, 0]);
        infe.extend_from_slice(b"hvc1\0");
        let mut iinf = 1u16.to_be_bytes().to_vec();
        iinf.extend(full_bx(b"infe", 2, &infe));
        let iinf = full_bx(b"iinf", 0, &iinf);
        let mut ispe = w.to_be_bytes().to_vec();
        ispe.extend_from_slice(&h.to_be_bytes());
        let mut ipco = bx(b"hvcC", &hvcc);
        ipco.extend(full_bx(b"ispe", 0, &ispe));
        let mut ipma = 1u32.to_be_bytes().to_vec();
        ipma.extend_from_slice(&1u16.to_be_bytes());
        ipma.extend_from_slice(&[2, 0x81, 0x02]);
        let mut iprp = bx(b"ipco", &ipco);
        iprp.extend(full_bx(b"ipma", 0, &ipma));
        let iprp = bx(b"iprp", &iprp);
        let iloc_len = 8 + 4 + 2 + 2 + 2 + 2 + 2 + 4 + 4;
        let meta_len = 12 + hdlr.len() + pitm.len() + iloc_len + iinf.len() + iprp.len();
        let mdat_data_off = (ftyp.len() + meta_len + 8) as u32;
        let mut iloc = vec![0x44, 0x00];
        iloc.extend_from_slice(&1u16.to_be_bytes()); // item count
        iloc.extend_from_slice(&1u16.to_be_bytes()); // item id
        iloc.extend_from_slice(&0u16.to_be_bytes()); // data reference
        iloc.extend_from_slice(&1u16.to_be_bytes()); // extents
        iloc.extend_from_slice(&mdat_data_off.to_be_bytes());
        iloc.extend_from_slice(&(data.len() as u32).to_be_bytes());
        let iloc = full_bx(b"iloc", 0, &iloc);
        let mut meta = hdlr;
        meta.extend(pitm);
        meta.extend(iloc);
        meta.extend(iinf);
        meta.extend(iprp);
        let meta = full_bx(b"meta", 0, &meta);
        let mut out = ftyp;
        out.extend(meta);
        out.extend(bx(b"mdat", &data));
        Some(out)
    }

    #[test]
    fn heic_decode() {
        let dir = match tempfile::tempdir() {
            Ok(d) => d,
            Err(e) => panic!("{e}"),
        };
        let hevc = dir.path().join("frame.hevc");
        let ok = std::process::Command::new("ffmpeg")
            .args(["-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi"])
            .args([
                "-i",
                "testsrc=size=64x48:rate=1",
                "-frames:v",
                "1",
                "-c:v",
                "libx265",
            ])
            .args([
                "-pix_fmt",
                "yuv420p",
                "-x265-params",
                "log-level=error",
                "-f",
                "hevc",
            ])
            .arg(&hevc)
            .status()
            .is_ok_and(|s| s.success());
        let Some(annexb) = ok.then(|| std::fs::read(&hevc).ok()).flatten() else {
            println!("note: skipping HEIC decode test (ffmpeg with libx265 not available)");
            return;
        };
        let Some(heic) = make_heic(&annexb, 64, 48) else {
            panic!("couldn't build the HEIC test file")
        };
        let path = dir.path().join("photo.heic");
        if let Err(e) = std::fs::write(&path, &heic) {
            panic!("{e}");
        }
        let d = match decode(&path, &DecodeOptions::default()) {
            Ok(d) => d,
            Err(e) => panic!("heic: {e}"),
        };
        assert_eq!(d.image.dimensions(), (64, 48));
        assert_eq!(probe_dimensions(&path).ok(), Some((64, 48)));
        assert!(!d.animated);
        assert_eq!(d.orientation, 1);
        // testsrc is colorful: the decoded picture is not black.
        let raw = d.image.to_rgb8().into_raw();
        let mean = raw.iter().map(|&c| u64::from(c)).sum::<u64>() / raw.len() as u64;
        assert!(mean > 40, "mean brightness {mean}");
        // Detected by content under a wrong extension.
        assert_eq!(dec(&heic, "jpg").image.dimensions(), (64, 48));
    }

    #[test]
    fn animated_webp_from_magick() {
        let dir = match tempfile::tempdir() {
            Ok(d) => d,
            Err(e) => panic!("{e}"),
        };
        let out = dir.path().join("anim.webp");
        let Some(bytes) = magick(&["-size", "16x16", "xc:red", "xc:blue", "-loop", "0"], &out)
        else {
            println!("note: skipping animated WebP test (magick not available)");
            return;
        };
        let d = dec(&bytes, "webp");
        assert!(d.animated);
        assert_eq!(d.image.dimensions(), (16, 16));
    }

    /// 12 MP JPG → WebP q80 (SPEC §10: < 1.5 s). Run with
    /// `cargo test -p morph-core --release -- --ignored perf_ --nocapture`.
    #[test]
    #[ignore]
    fn perf_12mp_jpg_to_webp() {
        let img = sample(4000, 3000, false);
        let jpg = enc(&img, OutputFormat::Jpg, &EncodeOptions::default(), None);
        let t = std::time::Instant::now();
        let d = dec(&jpg, "jpg");
        let t_dec = t.elapsed();
        let up = apply_orientation(d.image, d.orientation);
        let webp = enc(&up, OutputFormat::Webp, &EncodeOptions::default(), None);
        let total = t.elapsed();
        println!(
            "12 MP JPG ({} KB) -> WebP q80 ({} KB): decode {:?}, total {:?}",
            jpg.len() / 1024,
            webp.len() / 1024,
            t_dec,
            total
        );
    }
}
