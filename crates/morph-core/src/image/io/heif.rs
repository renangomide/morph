// SPDX-License-Identifier: GPL-3.0-or-later
//! HEIC/HEIF and AVIF decoding with libheif (libde265 for HEVC, libaom for AV1),
//! linked statically from vcpkg (`vcpkg.json` at the repo root).

use super::{Decoded, Metadata};
use crate::error::ConvertError;
use image::{DynamicImage, RgbImage, RgbaImage};
use libheif_rs::{
    ColorProfile, ColorSpace, DecodingOptions, HeifContext, LibHeif, RgbChroma, StreamReader,
    color_profile_types,
};

/// Is this an ISO-BMFF file with a HEIF-family brand (HEIC, HEIF, AVIF)?
pub(crate) fn is_heif_family(data: &[u8]) -> bool {
    if data.len() < 12 || &data[4..8] != b"ftyp" {
        return false;
    }
    let box_len = u32::from_be_bytes([data[0], data[1], data[2], data[3]]) as usize;
    let end = box_len.clamp(12, data.len());
    // major brand at 8..12, compatible brands from 16 on
    let mut brands = vec![&data[8..12]];
    let mut p = 16;
    while p + 4 <= end {
        brands.push(&data[p..p + 4]);
        p += 4;
    }
    brands.iter().any(|b| {
        matches!(
            *b,
            b"heic"
                | b"heix"
                | b"hevc"
                | b"hevx"
                | b"heim"
                | b"heis"
                | b"mif1"
                | b"msf1"
                | b"avif"
                | b"avis"
                | b"mif2"
        )
    })
}

/// Does the major/compatible brand list say AVIF (for error messages)?
fn is_avif(data: &[u8]) -> bool {
    data.len() >= 12 && (&data[8..12] == b"avif" || &data[8..12] == b"avis")
}

fn heif_err(data: &[u8], e: libheif_rs::HeifError) -> ConvertError {
    let kind = if is_avif(data) { "AVIF" } else { "HEIF" };
    // A missing decoder plugin is not damage: say what's going on.
    if matches!(
        e.code,
        libheif_rs::HeifErrorCode::UnsupportedFeature
            | libheif_rs::HeifErrorCode::UnsupportedFileType
    ) {
        return ConvertError::unsupported(
            format!("This {kind} photo uses a feature that isn't supported yet."),
            format!("libheif ({kind}): {e}"),
        );
    }
    ConvertError::damaged(format!("libheif ({kind}): {e}"))
}

/// Size of the primary image (after its rotation), reading only the boxes libheif needs.
pub(crate) fn probe(file: std::fs::File, len: u64) -> Result<(u32, u32), ConvertError> {
    let reader = StreamReader::new(std::io::BufReader::new(file), len);
    let ctx = HeifContext::read_from_reader(Box::new(reader))
        .map_err(|e| ConvertError::damaged(format!("libheif: {e}")))?;
    let handle = ctx
        .primary_image_handle()
        .map_err(|e| ConvertError::damaged(format!("libheif: {e}")))?;
    Ok((handle.width(), handle.height()))
}

pub(crate) fn decode(data: &[u8]) -> Result<Decoded, ConvertError> {
    let lib = LibHeif::new();
    let ctx = HeifContext::read_from_bytes(data).map_err(|e| heif_err(data, e))?;
    let handle = ctx.primary_image_handle().map_err(|e| heif_err(data, e))?;
    super::decode::check_size(handle.width(), handle.height())?;
    let has_alpha = handle.has_alpha_channel();

    let mut meta = Metadata::default();
    if let Some(p) = handle.color_profile_raw() {
        let t = p.profile_type();
        if t == color_profile_types::PROF || t == color_profile_types::R_ICC {
            meta.icc = Some(p.data);
        }
    }
    for m in handle.all_metadata() {
        if m.item_type.0 == *b"Exif" && meta.exif.is_none() {
            // 4-byte big-endian offset to the TIFF header, then the TIFF data.
            if m.raw_data.len() > 4 {
                let off = u32::from_be_bytes([
                    m.raw_data[0],
                    m.raw_data[1],
                    m.raw_data[2],
                    m.raw_data[3],
                ]) as usize;
                if let Some(tiff) = m.raw_data.get(4 + off..) {
                    let tiff = super::exif_edit::without_prefix(tiff);
                    if super::exif_edit::is_valid(tiff) {
                        meta.exif = Some(tiff.to_vec());
                    }
                }
            }
        } else if m.item_type.0 == *b"mime"
            && m.content_type == "application/rdf+xml"
            && meta.xmp.is_none()
        {
            meta.xmp = Some(m.raw_data);
        }
    }

    let mut opts = DecodingOptions::new();
    if let Some(o) = opts.as_mut() {
        o.set_convert_hdr_to_8bit(true);
    }
    let chroma = if has_alpha {
        RgbChroma::Rgba
    } else {
        RgbChroma::Rgb
    };
    let img = lib
        .decode(&handle, ColorSpace::Rgb(chroma), opts)
        .map_err(|e| heif_err(data, e))?;
    let planes = img.planes();
    let plane = planes
        .interleaved
        .ok_or_else(|| ConvertError::damaged("libheif: no interleaved plane"))?;
    let (w, h) = (plane.width, plane.height);
    let channels = if has_alpha { 4 } else { 3 };
    let row = w as usize * channels;
    let mut buf = Vec::with_capacity(row * h as usize);
    for y in 0..h as usize {
        let start = y * plane.stride;
        let line = plane
            .data
            .get(start..start + row)
            .ok_or_else(|| ConvertError::damaged("libheif: short plane"))?;
        buf.extend_from_slice(line);
    }
    let image = if has_alpha {
        RgbaImage::from_raw(w, h, buf).map(DynamicImage::ImageRgba8)
    } else {
        RgbImage::from_raw(w, h, buf).map(DynamicImage::ImageRgb8)
    }
    .ok_or_else(|| ConvertError::damaged("libheif: bad buffer size"))?;

    // Image sequences (animated AVIF / HEIF): only the primary still image is used.
    let animated = ctx.has_sequence();
    Ok(Decoded {
        image,
        // libheif already applied the HEIF rotation/mirror transforms (irot/imir),
        // which are what viewers use; the EXIF tag must not be applied again.
        orientation: 1,
        meta,
        animated,
    })
}
