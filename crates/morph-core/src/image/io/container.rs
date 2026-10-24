// SPDX-License-Identifier: GPL-3.0-or-later
//! Replace the EXIF / ICC / XMP blocks of an encoded JPEG, PNG or WebP file
//! without touching the compressed pixel data.

use super::{Metadata, OtherMetadata};
use crate::error::ConvertError;
use img_parts::jpeg::{Jpeg, JpegSegment, markers};
use img_parts::png::{Png, PngChunk};
use img_parts::{Bytes, ImageEXIF, ImageICC};

const XMP_JPEG_NS: &[u8] = b"http://ns.adobe.com/xap/1.0/\0";
const XMP_JPEG_EXT_NS: &[u8] = b"http://ns.adobe.com/xmp/extension/\0";
const XMP_PNG_KEYWORD: &[u8] = b"XML:com.adobe.xmp\0";
const EXIF_JPEG_NS: &[u8] = b"Exif\0";
const ICC_JPEG_NS: &[u8] = b"ICC_PROFILE\0";
const MPF_JPEG_NS: &[u8] = b"MPF\0";
const JFIF_NS: &[u8] = b"JFIF\0";
/// ImageMagick/exiftool keep EXIF, XMP, IPTC… as hex text under these keywords.
const PNG_RAW_PROFILE: &[u8] = b"Raw profile type ";
/// A JPEG segment holds at most 65533 bytes of contents.
const JPEG_SEGMENT_MAX: usize = 65533;

fn bytes(v: &Option<Vec<u8>>) -> Option<Bytes> {
    v.as_ref().map(|v| Bytes::copy_from_slice(v))
}

fn damaged(e: impl std::fmt::Display) -> ConvertError {
    ConvertError::damaged(format!("metadata rewrite: {e}"))
}

/// Colour space of pixel data, or of an ICC profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ColorSpace {
    Gray,
    Rgb,
    Cmyk,
}

/// The colour space an ICC profile is made for (`None`: not a valid profile).
pub(crate) fn icc_space(icc: &[u8]) -> Option<ColorSpace> {
    if icc.len() < 128 || icc.get(36..40) != Some(b"acsp") {
        return None;
    }
    match icc.get(16..20)? {
        b"GRAY" => Some(ColorSpace::Gray),
        b"RGB " => Some(ColorSpace::Rgb),
        b"CMYK" => Some(ColorSpace::Cmyk),
        _ => None,
    }
}

/// The ICC profile, only if it is valid and made for `pixels` (a profile for
/// another colour space would show wrong colours, so it is dropped).
pub(crate) fn matching_icc(icc: Option<&[u8]>, pixels: Option<ColorSpace>) -> Option<Vec<u8>> {
    let icc = icc?;
    let space = icc_space(icc)?;
    pixels.is_none_or(|p| p == space).then(|| icc.to_vec())
}

/// Start-of-frame markers (SOF0–SOF15 except DHT, JPG and DAC).
fn is_sof(marker: u8) -> bool {
    matches!(marker, 0xC0..=0xCF) && !matches!(marker, 0xC4 | 0xC8 | 0xCC)
}

/// Colour space of a JPEG's frame (from the SOF component count).
fn jpeg_space(jpeg: &Jpeg) -> Option<ColorSpace> {
    let sof = jpeg.segments().iter().find(|s| is_sof(s.marker()))?;
    match sof.contents().get(5)? {
        1 => Some(ColorSpace::Gray),
        3 => Some(ColorSpace::Rgb),
        4 => Some(ColorSpace::Cmyk),
        _ => None,
    }
}

/// Is this APP/COM segment one that "remove all metadata" keeps?
/// Kept: JFIF (APP0), EXIF/XMP (replaced separately), ICC (APP2, replaced
/// separately) and Adobe (APP14, colour transform needed to decode).
fn jpeg_segment_is_essential(marker: u8, contents: &[u8]) -> bool {
    match marker {
        markers::APP0 => contents.starts_with(JFIF_NS),
        markers::APP1 => {
            contents.starts_with(EXIF_JPEG_NS)
                || contents.starts_with(XMP_JPEG_NS)
                || contents.starts_with(XMP_JPEG_EXT_NS)
        }
        markers::APP2 => contents.starts_with(ICC_JPEG_NS),
        markers::APP14 => true,
        0xE3..=markers::APP13 | 0xEF | markers::COM => false,
        _ => true, // not metadata (tables, frame/scan headers…)
    }
}

/// End of the primary image (just after its EOI marker). Anything after it is
/// extra data, e.g. the MPF secondary images (with their own EXIF).
fn jpeg_primary_end(b: &[u8]) -> Option<usize> {
    let mut i = 2;
    loop {
        if *b.get(i)? != 0xFF {
            return None;
        }
        while b.get(i + 1) == Some(&0xFF) {
            i += 1;
        }
        let m = *b.get(i + 1)?;
        i += 2;
        match m {
            markers::EOI => return Some(i),
            0x01 | 0xD0..=0xD7 => continue,
            _ => {}
        }
        let len = usize::from(u16::from_be_bytes([*b.get(i)?, *b.get(i + 1)?]));
        i += len;
        if m == markers::SOS {
            // Entropy-coded data: 0xFF is followed by 0x00 (stuffing) or RSTn.
            while i + 1 < b.len() {
                if b[i] == 0xFF && !matches!(b[i + 1], 0x00 | 0xD0..=0xD7 | 0xFF) {
                    break;
                }
                i += 1;
            }
        }
    }
}

pub(crate) fn rewrite_jpeg(
    data: &[u8],
    meta: &Metadata,
    other: OtherMetadata,
) -> Result<Vec<u8>, ConvertError> {
    let mut jpeg = Jpeg::from_bytes(Bytes::copy_from_slice(data)).map_err(damaged)?;
    let icc = matching_icc(meta.icc.as_deref(), jpeg_space(&jpeg));
    jpeg.set_icc_profile(icc.map(Bytes::from));
    jpeg.set_exif(bytes(&meta.exif));

    // Anything after the primary image's EOI goes: MPF extra images, motion-photo
    // videos and vendor trailers (each can carry its own EXIF/GPS).
    let drop_trailer = other != OtherMetadata::Keep;
    jpeg.segments_mut().retain(|s| {
        let c = s.contents();
        let xmp = s.marker() == markers::APP1
            && (c.starts_with(XMP_JPEG_NS) || c.starts_with(XMP_JPEG_EXT_NS));
        let keep = match other {
            OtherMetadata::Keep => true,
            // IPTC/Photoshop (may hold a location and EXIF copies) and the
            // MPF index of extra images (each with its own EXIF).
            OtherMetadata::DropGps => {
                s.marker() != markers::APP13
                    && !(s.marker() == markers::APP2 && c.starts_with(MPF_JPEG_NS))
            }
            OtherMetadata::DropAll => jpeg_segment_is_essential(s.marker(), c),
        };
        keep && !xmp
    });
    if let Some(xmp) = &meta.xmp
        && XMP_JPEG_NS.len() + xmp.len() <= JPEG_SEGMENT_MAX
    {
        let mut contents = XMP_JPEG_NS.to_vec();
        contents.extend_from_slice(xmp);
        let segs = jpeg.segments();
        // After the APP0/APP1(EXIF)/APP2(ICC) segments, before anything else.
        let pos = segs
            .iter()
            .position(|s| !matches!(s.marker(), markers::APP0 | markers::APP1 | markers::APP2))
            .unwrap_or(segs.len());
        jpeg.segments_mut().insert(
            pos,
            JpegSegment::new_with_contents(markers::APP1, Bytes::from(contents)),
        );
    }
    let mut out = jpeg.encoder().bytes().to_vec();
    if drop_trailer && let Some(end) = jpeg_primary_end(&out) {
        out.truncate(end);
    }
    Ok(out)
}

/// Colour space of a PNG (from the IHDR colour type).
fn png_space(png: &Png) -> Option<ColorSpace> {
    match png.chunk_by_type(*b"IHDR")?.contents().get(9)? {
        0 | 4 => Some(ColorSpace::Gray),
        2 | 3 | 6 => Some(ColorSpace::Rgb),
        _ => None,
    }
}

/// Is this a PNG text chunk (tEXt/zTXt/iTXt)?
fn png_is_text(kind: [u8; 4]) -> bool {
    matches!(&kind, b"tEXt" | b"zTXt" | b"iTXt")
}

/// PNG chunks kept by "Remove all": image data and colour/display info only.
/// (`eXIf` and XMP are controlled by `meta`.)
fn png_chunk_is_essential(kind: [u8; 4]) -> bool {
    matches!(
        &kind,
        b"IHDR"
            | b"PLTE"
            | b"IDAT"
            | b"IEND"
            | b"tRNS"
            | b"gAMA"
            | b"cHRM"
            | b"sRGB"
            | b"iCCP"
            | b"sBIT"
            | b"pHYs"
            | b"bKGD"
            | b"cICP"
            | b"mDCv"
            | b"cLLi"
            | b"acTL"
            | b"fcTL"
            | b"fdAT"
            | b"eXIf"
    )
}

/// Legacy/other PNG chunks that can hold EXIF (with GPS) or provenance data.
fn png_chunk_may_hold_location(kind: [u8; 4]) -> bool {
    matches!(&kind, b"exIf" | b"zxIf" | b"caBX")
}

pub(crate) fn rewrite_png(
    data: &[u8],
    meta: &Metadata,
    other: OtherMetadata,
) -> Result<Vec<u8>, ConvertError> {
    let mut png = Png::from_bytes(Bytes::copy_from_slice(data)).map_err(damaged)?;
    let icc = matching_icc(meta.icc.as_deref(), png_space(&png));
    png.set_icc_profile(icc.map(Bytes::from));
    png.set_exif(bytes(&meta.exif));

    png.chunks_mut().retain(|c| {
        let text = png_is_text(c.kind());
        let xmp = c.kind() == *b"iTXt" && c.contents().starts_with(XMP_PNG_KEYWORD);
        let keep = match other {
            OtherMetadata::Keep => true,
            // Raw EXIF/XMP/IPTC profiles stored as text can hold the location.
            OtherMetadata::DropGps => {
                !(text && c.contents().starts_with(PNG_RAW_PROFILE))
                    && !png_chunk_may_hold_location(c.kind())
            }
            OtherMetadata::DropAll => png_chunk_is_essential(c.kind()),
        };
        keep && !xmp
    });
    if let Some(xmp) = &meta.xmp {
        // keyword, compression flag 0, method 0, empty language, empty translated keyword
        let mut contents = XMP_PNG_KEYWORD.to_vec();
        contents.extend_from_slice(&[0, 0, 0, 0]);
        contents.extend_from_slice(xmp);
        // Before the image data: many readers only look at chunks before IDAT.
        let chunks = png.chunks();
        let pos = chunks
            .iter()
            .position(|c| c.kind() == *b"IDAT" || c.kind() == *b"IEND")
            .unwrap_or(chunks.len());
        png.chunks_mut()
            .insert(pos, PngChunk::new(*b"iTXt", Bytes::from(contents)));
    }
    Ok(png.encoder().bytes().to_vec())
}

/// WebP (RIFF) rewrite, done by hand so the VP8X header flags are always right.
pub(crate) fn rewrite_webp(
    data: &[u8],
    meta: &Metadata,
    other: OtherMetadata,
) -> Result<Vec<u8>, ConvertError> {
    if data.len() < 12 || &data[0..4] != b"RIFF" || &data[8..12] != b"WEBP" {
        return Err(ConvertError::damaged("not a WebP (RIFF/WEBP) file"));
    }
    let mut chunks: Vec<([u8; 4], &[u8])> = Vec::new();
    let mut pos = 12;
    while pos + 8 <= data.len() {
        let id: [u8; 4] = [data[pos], data[pos + 1], data[pos + 2], data[pos + 3]];
        let len = u32::from_le_bytes([data[pos + 4], data[pos + 5], data[pos + 6], data[pos + 7]])
            as usize;
        let start = pos + 8;
        let end = start
            .checked_add(len)
            .filter(|&e| e <= data.len())
            .ok_or_else(|| ConvertError::damaged("truncated WebP chunk"))?;
        chunks.push((id, &data[start..end]));
        pos = end + (len & 1);
    }

    let old_vp8x = chunks
        .iter()
        .find(|(id, _)| id == b"VP8X")
        .map(|(_, d)| *d)
        .filter(|d| d.len() >= 10);
    let canvas = match old_vp8x {
        Some(d) => Some((
            u32::from_le_bytes([d[4], d[5], d[6], 0]) + 1,
            u32::from_le_bytes([d[7], d[8], d[9], 0]) + 1,
        )),
        None => bitstream_size(&chunks),
    };
    let old_flags = old_vp8x.map_or(0, |d| d[0]);
    // WebP pixels are always RGB.
    let icc = matching_icc(meta.icc.as_deref(), Some(ColorSpace::Rgb));
    let meta = &Metadata {
        icc,
        exif: meta.exif.clone(),
        xmp: meta.xmp.clone(),
    };

    let body: Vec<([u8; 4], &[u8])> = chunks
        .into_iter()
        .filter(|(id, _)| !matches!(id, b"VP8X" | b"ICCP" | b"EXIF" | b"XMP "))
        // "Remove all": unknown chunks go too (only image data stays).
        .filter(|(id, _)| {
            other != OtherMetadata::DropAll
                || matches!(id, b"VP8 " | b"VP8L" | b"ALPH" | b"ANIM" | b"ANMF")
        })
        .collect();

    let has = |id: &[u8; 4]| body.iter().any(|(i, _)| i == id);
    let vp8l_alpha = body
        .iter()
        .find(|(id, _)| id == b"VP8L")
        .is_some_and(|(_, d)| d.len() >= 5 && d[0] == 0x2f && d[4] & 0x10 != 0);
    let alpha = old_flags & 0x10 != 0 || has(b"ALPH") || vp8l_alpha;
    let anim = old_flags & 0x02 != 0 || has(b"ANIM");
    let extra = body.iter().any(|(id, _)| !matches!(id, b"VP8 " | b"VP8L"));
    let extended = meta.icc.is_some()
        || meta.exif.is_some()
        || meta.xmp.is_some()
        || anim
        || extra
        || (alpha && !has(b"VP8L"));

    let mut out_chunks: Vec<([u8; 4], &[u8])> = Vec::new();
    let mut vp8x = [0u8; 10];
    if extended {
        let (w, h) = canvas.ok_or_else(|| ConvertError::damaged("WebP size unknown"))?;
        let mut flags = 0u8;
        if meta.icc.is_some() {
            flags |= 0x20;
        }
        if alpha {
            flags |= 0x10;
        }
        if meta.exif.is_some() {
            flags |= 0x08;
        }
        if meta.xmp.is_some() {
            flags |= 0x04;
        }
        if anim {
            flags |= 0x02;
        }
        vp8x[0] = flags;
        vp8x[4..7].copy_from_slice(&(w - 1).to_le_bytes()[..3]);
        vp8x[7..10].copy_from_slice(&(h - 1).to_le_bytes()[..3]);
    }
    if extended {
        out_chunks.push((*b"VP8X", &vp8x));
    }
    if let Some(icc) = &meta.icc {
        out_chunks.push((*b"ICCP", icc));
    }
    out_chunks.extend(body.iter().copied());
    if let Some(exif) = &meta.exif {
        out_chunks.push((*b"EXIF", exif));
    }
    if let Some(xmp) = &meta.xmp {
        out_chunks.push((*b"XMP ", xmp));
    }

    let payload: usize = out_chunks
        .iter()
        .map(|(_, d)| 8 + d.len() + (d.len() & 1))
        .sum();
    let riff_len = u32::try_from(4 + payload)
        .map_err(|_| ConvertError::other("Couldn't save the image.", "WebP too large"))?;
    let mut out = Vec::with_capacity(8 + 4 + payload);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&riff_len.to_le_bytes());
    out.extend_from_slice(b"WEBP");
    for (id, d) in out_chunks {
        out.extend_from_slice(&id);
        let len = u32::try_from(d.len())
            .map_err(|_| ConvertError::other("Couldn't save the image.", "WebP chunk too large"))?;
        out.extend_from_slice(&len.to_le_bytes());
        out.extend_from_slice(d);
        if d.len() & 1 == 1 {
            out.push(0);
        }
    }
    Ok(out)
}

/// Canvas size from the VP8 / VP8L bitstream header.
fn bitstream_size(chunks: &[([u8; 4], &[u8])]) -> Option<(u32, u32)> {
    for (id, d) in chunks {
        match id {
            b"VP8 " if d.len() >= 10 && d[3..6] == [0x9d, 0x01, 0x2a] => {
                let w = u32::from(u16::from_le_bytes([d[6], d[7]]) & 0x3fff);
                let h = u32::from(u16::from_le_bytes([d[8], d[9]]) & 0x3fff);
                return Some((w, h));
            }
            b"VP8L" if d.len() >= 5 && d[0] == 0x2f => {
                let b = u32::from_le_bytes([d[1], d[2], d[3], d[4]]);
                return Some(((b & 0x3fff) + 1, ((b >> 14) & 0x3fff) + 1));
            }
            _ => {}
        }
    }
    None
}
