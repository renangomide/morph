// SPDX-License-Identifier: GPL-3.0-or-later
//! `image.compress` action (SPEC §4.1).
//!
//! - Keeps the input format (or WebP with `convert_to_webp`; JPG/PNG when the input
//!   format can't be written).
//! - Best mode (default): the smallest file that still looks identical, see
//!   [`crate::image::best`]. JPG/WebP/AVIF: search for the lowest quality that
//!   passes (max 5 trial encodes; on the sample mosaic for large images, then one
//!   full encode to confirm). JPG keeps the input's 4:4:4 colour, and tries
//!   4:4:4 when 4:2:0 can't pass. PNG: lossless (oxipng), or palette
//!   quantization when that is smaller and passes. Other formats: one re-encode.
//!   Never writes a candidate that failed the check or isn't smaller than the
//!   input; if metadata must go, it is removed without re-encoding instead.
//! - Quality mode: one re-encode. Target-size mode: binary search on quality
//!   (max 8 encodes), then Lanczos3 downscaling if even the lowest quality is too big
//!   (at most 12 encodes in all).
//! - When the result isn't smaller than an input of the same format, no file is written.

use crate::action::Action;
use crate::converter::{Converter, TaskCtx, TaskOutput};
use crate::error::ConvertError;
use crate::image::best::{
    self, BEST_QUALITY_RANGE, CONFIRM_STEP, FIRST_GUESS, MAX_BEST_TRIES, MAX_CONFIRM_RETRIES, Trial,
};
use crate::image::common::{
    ANIMATED_NOTE, check_webp_input, extension_of, has_transparency, ico_sizes_for, join_notes,
    metadata_to_keep, quality_for, resize_lanczos, same_or_fallback_format, task_input,
    wrong_action,
};
use crate::image::io::{self, DecodeOptions, EncodeOptions, Metadata};
use crate::image::options::{CompressMode, ImageCompressOpts, OutputFormat};
use crate::job::Task;
use crate::naming::{self, Suffix};
use image::DynamicImage;

pub struct ImageCompress;

/// Note when compressing would not save anything.
pub const ALREADY_OPTIMIZED_NOTE: &str = "Already optimized; no smaller file was made.";

/// Encodes in the full-size quality search (SPEC: max 8 iterations).
pub const MAX_QUALITY_TRIES: u32 = 8;
/// Encodes in total for lossy formats, downscaling included.
pub const MAX_TOTAL_TRIES: u32 = 12;
/// Downscale rounds after the quality search fails.
pub const MAX_DOWNSCALES: u32 = 4;
/// Quality range searched in target-size mode.
pub const MIN_QUALITY: u8 = 10;
pub const MAX_QUALITY: u8 = 95;

/// A result that fits the target size.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fit {
    pub bytes: Vec<u8>,
    /// Quality used (0 for formats without a quality setting).
    pub quality: u8,
    /// Pixel dimensions of the result.
    pub dims: (u32, u32),
}

/// Outcome of [`fit_target_size`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FitResult {
    Fit(Fit),
    /// Nothing fits, even at the lowest quality and after downscaling.
    TooBig {
        smallest: u64,
    },
}

/// Binary search on quality in `lo..=hi`, at most `max_tries` encodes. Returns the
/// highest-quality result that is ≤ `target`, and the smallest size seen.
/// An encoding and the quality that produced it.
type Encoded = (u8, Vec<u8>);

fn search_quality<E>(
    target: u64,
    (mut lo, mut hi): (u8, u8),
    max_tries: u32,
    encode: &mut E,
) -> Result<(Option<Encoded>, u64), ConvertError>
where
    E: FnMut(u8) -> Result<Vec<u8>, ConvertError>,
{
    let mut best: Option<Encoded> = None;
    let mut smallest = u64::MAX;
    let mut tries = 0;
    while lo <= hi && tries < max_tries {
        let mid = lo + (hi - lo) / 2;
        let bytes = encode(mid)?;
        tries += 1;
        let len = bytes.len() as u64;
        smallest = smallest.min(len);
        if len <= target {
            best = Some((mid, bytes));
            match mid.checked_add(1) {
                Some(next) => lo = next,
                None => break,
            }
        } else {
            match mid.checked_sub(1) {
                Some(prev) => hi = prev,
                None => break,
            }
        }
    }
    Ok((best, smallest))
}

/// Find the best encoding ≤ `target` bytes for an image of `src` dimensions.
///
/// `encode(dims, quality)` encodes the image scaled to `dims` (quality is ignored
/// when `lossy` is false). Lossy formats: binary search on quality (max
/// [`MAX_QUALITY_TRIES`] encodes); if nothing fits, the image is downscaled by about
/// √(target / last size) and tried at the lowest quality, up to [`MAX_DOWNSCALES`]
/// times; once a size fits, the encodes left (of [`MAX_TOTAL_TRIES`]) search for
/// a higher quality. `on_try(done, max)` is called before every encode.
pub fn fit_target_size<E, P>(
    target: u64,
    src: (u32, u32),
    lossy: bool,
    mut encode: E,
    mut on_try: P,
) -> Result<FitResult, ConvertError>
where
    E: FnMut((u32, u32), u8) -> Result<Vec<u8>, ConvertError>,
    P: FnMut(u32, u32),
{
    let max_tries = if lossy {
        MAX_TOTAL_TRIES
    } else {
        MAX_DOWNSCALES + 1
    };
    let mut tries = 0u32;
    let mut run = |dims: (u32, u32), q: u8, tries: &mut u32| {
        *tries += 1;
        on_try(*tries, max_tries);
        encode(dims, q)
    };
    let fit = |bytes, quality, dims| {
        Ok(FitResult::Fit(Fit {
            bytes,
            quality,
            dims,
        }))
    };

    // Full size: binary search on quality.
    let mut dims = (src.0.max(1), src.1.max(1));
    let (best, mut last) = if lossy {
        search_quality(
            target,
            (MIN_QUALITY, MAX_QUALITY),
            MAX_QUALITY_TRIES,
            &mut |q| run(dims, q, &mut tries),
        )?
    } else {
        let bytes = run(dims, 0, &mut tries)?;
        let len = bytes.len() as u64;
        ((len <= target).then_some((0, bytes)), len)
    };
    if let Some((quality, bytes)) = best {
        return fit(bytes, quality, dims);
    }
    let mut smallest = last;

    // Too big even at the lowest quality: downscale, one encode at the lowest
    // quality per size; once one fits, the encodes left raise the quality.
    let lowest = if lossy { MIN_QUALITY } else { 0 };
    for _ in 0..MAX_DOWNSCALES {
        if tries >= max_tries || dims == (1, 1) {
            break;
        }
        // Size scales roughly with pixel count: shrink each side by √(target/size),
        // with a 5% margin, and always by at least 5%.
        let ratio = target as f64 / last.max(1) as f64;
        let scale = (ratio.sqrt() * 0.95).clamp(0.0, 0.95);
        dims = (
            ((f64::from(dims.0) * scale).round() as u32).max(1),
            ((f64::from(dims.1) * scale).round() as u32).max(1),
        );
        let bytes = run(dims, lowest, &mut tries)?;
        last = bytes.len() as u64;
        smallest = smallest.min(last);
        if last <= target {
            let mut best = (lowest, bytes);
            let left = max_tries.saturating_sub(tries);
            if lossy && left > 0 {
                let (better, _) =
                    search_quality(target, (MIN_QUALITY + 1, MAX_QUALITY), left, &mut |q| {
                        run(dims, q, &mut tries)
                    })?;
                if let Some(b) = better {
                    best = b;
                }
            }
            return fit(best.1, best.0, dims);
        }
    }
    Ok(FitResult::TooBig { smallest })
}

/// Output format for compress (see module docs) and the fallback note, if any.
fn choose_format(
    input_ext: &str,
    has_transparency: bool,
    convert_to_webp: bool,
) -> (OutputFormat, Option<String>) {
    if convert_to_webp {
        (OutputFormat::Webp, None)
    } else {
        same_or_fallback_format(input_ext, has_transparency)
    }
}

struct Encoder<'a> {
    format: OutputFormat,
    opts: &'a ImageCompressOpts,
    meta: &'a Metadata,
}

impl Encoder<'_> {
    fn encode(
        &self,
        img: &DynamicImage,
        quality: u8,
        png_lossy: bool,
    ) -> Result<Vec<u8>, ConvertError> {
        let opts = EncodeOptions {
            quality,
            png_level: self.opts.png_level.min(6),
            png_lossy,
            ico_sizes: ico_sizes_for(img.width(), img.height()),
            ..EncodeOptions::default()
        };
        io::encode(img, self.format, &opts, Some(self.meta))
    }

    /// Lossless WebP, with metadata.
    fn encode_webp_lossless(&self, img: &DynamicImage) -> Result<Vec<u8>, ConvertError> {
        let opts = EncodeOptions {
            lossless: true,
            ..EncodeOptions::default()
        };
        io::encode(img, OutputFormat::Webp, &opts, Some(self.meta))
    }

    /// JPG/WebP/AVIF at `quality`. `trial`: a quick encode of the sample, only
    /// to judge its pixels (no metadata, JPG draft mode).
    fn encode_lossy(
        &self,
        img: &DynamicImage,
        quality: u8,
        chroma_444: bool,
        trial: bool,
    ) -> Result<Vec<u8>, ConvertError> {
        let opts = EncodeOptions {
            quality,
            chroma_444,
            draft: trial,
            ..EncodeOptions::default()
        };
        io::encode(img, self.format, &opts, (!trial).then_some(self.meta))
    }
}

/// Best (automatic) mode: the bytes to write, or `None` when nothing smaller
/// than `cap` bytes looks identical (see the module docs). A candidate that
/// failed the "looks identical" check is never returned.
fn best_bytes(
    encoder: &Encoder<'_>,
    image: &DynamicImage,
    cap: u64,
    input_444: bool,
    ctx: &TaskCtx<'_>,
) -> Result<Option<Vec<u8>>, ConvertError> {
    let format = encoder.format;
    if !format.is_lossy() && format != OutputFormat::Png {
        // BMP, TIFF, GIF, ICO: nothing to search, re-encode as before.
        return encoder.encode(image, 0, encoder.opts.png_lossy).map(Some);
    }
    let reference = best::Reference::new(
        image,
        format.supports_alpha(),
        EncodeOptions::default().background,
    )?;
    ctx.check_cancel()?;
    let fits = |bytes: &[u8]| (bytes.len() as u64) < cap;
    // Decode a full-size candidate and judge it. One decoded candidate at a time.
    let passes = |bytes: &[u8]| -> Result<bool, ConvertError> {
        let decoded = io::decode_bytes(bytes, None, &DecodeOptions::default())?;
        Ok(best::looks_identical(reference.score(&decoded.image)?))
    };

    if format == OutputFormat::Png {
        let lossless = encoder.encode(image, 0, false)?;
        ctx.progress(0.5, None);
        ctx.check_cancel()?;
        let quantized = encoder.encode(image, 0, true)?;
        ctx.progress(0.8, None);
        ctx.check_cancel()?;
        if quantized.len() < lossless.len() && fits(&quantized) && passes(&quantized)? {
            return Ok(Some(quantized));
        }
        return Ok(fits(&lossless).then_some(lossless));
    }

    // JPG: keep the input's colour resolution; 4:2:0 that can't pass (e.g. red
    // text on blue) gets a second search at 4:4:4.
    let chromas: &[bool] = match (format, input_444) {
        (OutputFormat::Jpg, false) => &[false, true],
        (OutputFormat::Jpg, true) => &[true],
        _ => &[false],
    };
    // Large images: search on the sample mosaic, then confirm on the full image.
    let sample = reference.sample(image);
    let rounds = chromas.len() as f32;
    for (round, &chroma_444) in chromas.iter().enumerate() {
        let mut tries = 0u32;
        let mut tick = || {
            tries += 1;
            let done = (round as f32 + tries as f32 / MAX_BEST_TRIES as f32) / rounds;
            ctx.progress(0.1 + 0.8 * done.min(1.0), None);
        };
        let found = match &sample {
            None => best::search_lowest_passing(
                BEST_QUALITY_RANGE,
                Some(FIRST_GUESS),
                MAX_BEST_TRIES,
                |q| {
                    ctx.check_cancel()?;
                    let bytes = encoder.encode_lossy(image, q, chroma_444, false)?;
                    tick();
                    if !fits(&bytes) {
                        return Ok(Trial::TooBig);
                    }
                    ctx.check_cancel()?;
                    Ok(if passes(&bytes)? {
                        Trial::Pass(bytes)
                    } else {
                        Trial::Fail
                    })
                },
            )?
            .map(|(_, bytes)| bytes),
            Some(sample) => {
                let found = best::search_lowest_passing(
                    BEST_QUALITY_RANGE,
                    Some(FIRST_GUESS),
                    MAX_BEST_TRIES,
                    |q| {
                        ctx.check_cancel()?;
                        let bytes = encoder.encode_lossy(sample, q, chroma_444, true)?;
                        let decoded = io::decode_bytes(&bytes, None, &DecodeOptions::default())?;
                        tick();
                        let ok = best::looks_identical(reference.score_sample(&decoded.image)?);
                        Ok(if ok {
                            Trial::Pass(Vec::new())
                        } else {
                            Trial::Fail
                        })
                    },
                )?;
                match found {
                    Some((q, _)) => confirm(encoder, image, q, chroma_444, &fits, &passes, ctx)?,
                    None => None,
                }
            }
        };
        if found.is_some() {
            return Ok(found);
        }
    }
    Ok(None)
}

/// Best mode when the output format differs from the input's (Convert to WebP,
/// or a format that can't be written): there is no "keep the original", so a
/// file is always made. WebP: lossless when nothing lossy looks identical, or
/// when the input was lossless (screenshots, graphics) and lossless is smaller.
/// Other formats: the highest searched quality (4:4:4 for JPG).
fn converted_bytes(
    encoder: &Encoder<'_>,
    image: &DynamicImage,
    found: Option<Vec<u8>>,
    lossless_input: bool,
) -> Result<Vec<u8>, ConvertError> {
    if encoder.format == OutputFormat::Webp {
        return match found {
            Some(lossy) if !lossless_input => Ok(lossy),
            found => {
                let lossless = encoder.encode_webp_lossless(image)?;
                Ok(match found {
                    Some(lossy) if lossy.len() <= lossless.len() => lossy,
                    _ => lossless,
                })
            }
        };
    }
    match found {
        Some(bytes) => Ok(bytes),
        None if encoder.format.is_lossy() => {
            encoder.encode_lossy(image, BEST_QUALITY_RANGE.1, true, false)
        }
        None => encoder.encode(image, 0, false),
    }
}

/// Encode the full image at the quality found on the sample and check it;
/// on a miss, raise the quality a little and check again (bounded).
fn confirm(
    encoder: &Encoder<'_>,
    image: &DynamicImage,
    mut quality: u8,
    chroma_444: bool,
    fits: &dyn Fn(&[u8]) -> bool,
    passes: &dyn Fn(&[u8]) -> Result<bool, ConvertError>,
    ctx: &TaskCtx<'_>,
) -> Result<Option<Vec<u8>>, ConvertError> {
    for _ in 0..=MAX_CONFIRM_RETRIES {
        ctx.check_cancel()?;
        let bytes = encoder.encode_lossy(image, quality, chroma_444, false)?;
        if !fits(&bytes) {
            // Higher qualities are bigger still.
            return Ok(None);
        }
        ctx.check_cancel()?;
        if passes(&bytes)? {
            return Ok(Some(bytes));
        }
        if quality >= BEST_QUALITY_RANGE.1 {
            break;
        }
        quality = quality
            .saturating_add(CONFIRM_STEP)
            .min(BEST_QUALITY_RANGE.1);
    }
    Ok(None)
}

impl Converter for ImageCompress {
    fn run(&self, task: &Task, ctx: &TaskCtx<'_>) -> Result<TaskOutput, ConvertError> {
        let Action::ImageCompress(opts) = &task.action else {
            return Err(wrong_action("image.compress", task));
        };
        let input = task_input(task)?;
        ctx.check_cancel()?;
        if opts.convert_to_webp {
            check_webp_input(input, &DecodeOptions::default())?;
        }

        let data = std::fs::read(input)?;
        let input_size = data.len() as u64;
        let decoded = io::decode_bytes(&data, Some(input), &DecodeOptions::default())?;
        ctx.progress(0.1, None);
        ctx.check_cancel()?;

        let meta = metadata_to_keep(&decoded.meta, opts.keep_metadata, opts.remove_gps);
        // Removing metadata (or GPS) is a change worth a new file even when it isn't smaller.
        let strips_metadata = meta != decoded.meta;
        let animated = decoded.animated;
        let orientation = decoded.orientation;
        let image = io::apply_orientation(decoded.image, orientation);
        ctx.check_cancel()?;

        let ext = extension_of(input);
        let (format, fallback_note) =
            choose_format(&ext, has_transparency(&image), opts.convert_to_webp);
        let encoder = Encoder {
            format,
            opts,
            meta: &meta,
        };

        let mut notes = Vec::new();
        if animated {
            notes.push(ANIMATED_NOTE.to_owned());
        }
        notes.extend(fallback_note);
        let same_format = OutputFormat::from_extension(&ext) == Some(format);
        let already_optimized = |mut notes: Vec<String>| {
            ctx.progress(1.0, None);
            notes.insert(0, ALREADY_OPTIMIZED_NOTE.to_owned());
            Ok(TaskOutput {
                outputs: Vec::new(),
                note: join_notes(notes),
            })
        };

        let bytes = match opts.mode {
            CompressMode::Best => {
                let cap = if same_format { input_size } else { u64::MAX };
                let input_444 = io::jpeg_is_444(&data).unwrap_or(false);
                let found = best_bytes(&encoder, &image, cap, input_444, ctx)?;
                if !same_format {
                    // A new format was asked for (or needed): always make the file.
                    ctx.check_cancel()?;
                    let lossless_input = !matches!(
                        ext.as_str(),
                        "jpg" | "jpeg" | "jfif" | "heic" | "heif" | "avif" | "webp"
                    );
                    converted_bytes(&encoder, &image, found, lossless_input)?
                } else {
                    match found {
                        Some(bytes) => bytes,
                        // Nothing smaller looks the same. Metadata to remove: rewrite
                        // it on the original bytes (pixels untouched), when the
                        // orientation survives (as in Clean).
                        None if same_format
                            && strips_metadata
                            && (orientation <= 1 || meta.exif.is_some()) =>
                        {
                            match io::rewrite_metadata(&data, format, &meta)? {
                                Some(bytes) => bytes,
                                None => return already_optimized(notes),
                            }
                        }
                        None => return already_optimized(notes),
                    }
                }
            }
            CompressMode::Quality { quality } => {
                let bytes = encoder.encode(&image, quality_for(format, quality), opts.png_lossy)?;
                ctx.progress(0.9, None);
                bytes
            }
            CompressMode::TargetSize { bytes: target } => {
                if target == 0 {
                    return Err(ConvertError::unsupported(
                        "The target size must be more than 0 bytes.",
                        "target size 0",
                    ));
                }
                let src = (image.width(), image.height());
                let lossy = format.is_lossy();
                // PNG has no quality knob: use palette quantization.
                let png_lossy = opts.png_lossy || format == OutputFormat::Png;
                let mut scaled: Option<DynamicImage> = None;
                let result = fit_target_size(
                    target,
                    src,
                    lossy,
                    |dims, quality| {
                        ctx.check_cancel()?;
                        let img = if dims == src {
                            &image
                        } else {
                            if scaled
                                .as_ref()
                                .is_none_or(|s| (s.width(), s.height()) != dims)
                            {
                                scaled = Some(resize_lanczos(&image, dims.0, dims.1)?);
                            }
                            scaled.as_ref().unwrap_or(&image)
                        };
                        let quality = if lossy { quality } else { 0 };
                        encoder.encode(img, quality, png_lossy)
                    },
                    |done, max| ctx.progress(0.1 + 0.8 * done as f32 / max as f32, None),
                )?;
                match result {
                    FitResult::Fit(fit) => {
                        if fit.dims != src {
                            notes.push(format!(
                                "Scaled down to {}×{} to reach the target size.",
                                fit.dims.0, fit.dims.1
                            ));
                        }
                        fit.bytes
                    }
                    FitResult::TooBig { smallest } => {
                        return Err(ConvertError::other(
                            "Couldn't make the file that small, even at low quality and a smaller size.",
                            format!("target {target} bytes; smallest result {smallest} bytes"),
                        ));
                    }
                }
            }
        };
        ctx.check_cancel()?;

        if same_format && !strips_metadata && bytes.len() as u64 >= input_size {
            return already_optimized(notes);
        }

        let name = naming::output_name(input, format.extension(), Suffix::Compressed);
        let path = naming::write_output(&task.output.folder_for(input), &name, &bytes)?;
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
    use image::ImageFormat;

    /// One call to the encoder: (dims, quality).
    type Call = ((u32, u32), u8);

    /// Fake encoder: size grows with quality and pixel count.
    fn fake(dims: (u32, u32), quality: u8) -> Vec<u8> {
        let pixels = u64::from(dims.0) * u64::from(dims.1);
        vec![0; (pixels * (u64::from(quality) + 10) / 100) as usize]
    }

    fn search(target: u64, src: (u32, u32), lossy: bool) -> (FitResult, Vec<Call>, u32) {
        let mut calls = Vec::new();
        let mut last_try = 0;
        let r = fit_target_size(
            target,
            src,
            lossy,
            |d, q| {
                calls.push((d, q));
                Ok(fake(d, q))
            },
            |done, max| {
                assert!(done <= max);
                last_try = done;
            },
        )
        .unwrap();
        (r, calls, last_try)
    }

    #[test]
    fn finds_highest_quality_that_fits() {
        // 100×100 → size = 100 * (q + 10). Target 6000 → q ≤ 50.
        let (r, calls, tries) = search(6000, (100, 100), true);
        let FitResult::Fit(fit) = r else {
            panic!("should fit")
        };
        assert_eq!(fit.quality, 50);
        assert_eq!(fit.dims, (100, 100));
        assert!(fit.bytes.len() <= 6000);
        assert!(calls.len() as u32 <= MAX_QUALITY_TRIES);
        assert_eq!(tries, calls.len() as u32);
    }

    #[test]
    fn generous_target_uses_max_quality() {
        let (r, calls, _) = search(1_000_000, (100, 100), true);
        let FitResult::Fit(fit) = r else {
            panic!("should fit")
        };
        assert_eq!(fit.quality, MAX_QUALITY);
        assert!(calls.len() as u32 <= MAX_QUALITY_TRIES);
    }

    #[test]
    fn downscales_when_lowest_quality_is_too_big() {
        // At q=10, 1000×1000 gives 200 000 bytes; target 50 000 needs downscaling.
        let (r, calls, _) = search(50_000, (1000, 1000), true);
        let FitResult::Fit(fit) = r else {
            panic!("should fit after downscaling")
        };
        assert!(fit.bytes.len() <= 50_000);
        assert!(fit.dims.0 < 1000 && fit.dims.1 < 1000);
        assert_eq!(fit.dims.0, fit.dims.1, "aspect kept");
        assert!(calls.iter().any(|&(_, q)| q == MIN_QUALITY));
        assert!(
            calls.len() as u32 <= MAX_TOTAL_TRIES,
            "{} encodes",
            calls.len()
        );
        // The encodes left after downscaling raise the quality where possible.
        assert!(fit.quality > MIN_QUALITY, "quality {}", fit.quality);
    }

    #[test]
    fn total_encodes_are_capped() {
        // From an easy fit to hopeless, the search never exceeds the budget, and the
        // full-size round stays within the SPEC's 8 iterations.
        for target in [
            1, 1_000, 30_000, 50_000, 150_000, 200_000, 1_000_000, 10_000_000,
        ] {
            let (_, calls, tries) = search(target, (1000, 1000), true);
            assert!(
                calls.len() as u32 <= MAX_TOTAL_TRIES,
                "target {target}: {calls:?}"
            );
            let full = calls.iter().filter(|(d, _)| *d == (1000, 1000)).count();
            assert!(full as u32 <= MAX_QUALITY_TRIES, "target {target}: {full}");
            assert_eq!(tries, calls.len() as u32);
        }
    }

    #[test]
    fn lossless_formats_only_downscale() {
        // q is ignored by callers for lossless; the fake still uses it (0 → 10%).
        let (r, calls, _) = search(5_000, (500, 500), false);
        let FitResult::Fit(fit) = r else {
            panic!("should fit")
        };
        assert!(fit.bytes.len() <= 5_000);
        assert!(calls.iter().all(|&(_, q)| q == 0));
        assert!(calls.len() as u32 <= MAX_DOWNSCALES + 1);
    }

    #[test]
    fn impossible_target_reports_too_big() {
        let mut calls = 0;
        let r = fit_target_size(
            10,
            (100, 100),
            true,
            |_, _| {
                calls += 1;
                Ok(vec![0; 1000]) // never shrinks
            },
            |_, _| {},
        )
        .unwrap();
        assert_eq!(r, FitResult::TooBig { smallest: 1000 });
        assert!(calls <= MAX_TOTAL_TRIES, "{calls} encodes");
    }

    #[test]
    fn encoder_errors_propagate() {
        let r = fit_target_size(
            10,
            (10, 10),
            true,
            |_, _| Err(ConvertError::cancelled()),
            |_, _| {},
        );
        assert!(r.is_err_and(|e| e.is_cancelled()));
    }

    #[test]
    fn format_choice() {
        assert_eq!(
            choose_format("jpg", false, false),
            (OutputFormat::Jpg, None)
        );
        assert_eq!(choose_format("png", true, true), (OutputFormat::Webp, None));
        assert_eq!(
            choose_format("heic", false, true),
            (OutputFormat::Webp, None)
        );
        assert_eq!(choose_format("heic", false, false).0, OutputFormat::Jpg);
        assert_eq!(choose_format("svg", true, false).0, OutputFormat::Png);
        assert_eq!(
            choose_format("jfif", false, false),
            (OutputFormat::Jpg, None)
        );
    }

    fn compress(mode: CompressMode) -> Action {
        Action::ImageCompress(ImageCompressOpts {
            mode,
            ..Default::default()
        })
    }

    #[test]
    fn quality_mode_makes_smaller_jpg() {
        let dir = tempfile::tempdir().unwrap();
        let input = save_jpeg(dir.path(), "big.jpg", &noisy_rgb(200, 150), 100);
        let out = run_one(
            compress(CompressMode::Quality { quality: Some(40) }),
            &input,
        );
        let path = out.output();
        assert_eq!(file_name(path), "big (compressed).jpg");
        assert!(std::fs::metadata(path).unwrap().len() < std::fs::metadata(&input).unwrap().len());
        let (img, format) = open(path);
        assert_eq!(format, ImageFormat::Jpeg);
        assert_eq!((img.width(), img.height()), (200, 150));
    }

    #[test]
    fn not_smaller_writes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        // BMP re-encodes to the same size: nothing to gain.
        let input = save(dir.path(), "flat.bmp", &noisy_rgb(30, 30));
        let out = run_one(compress(CompressMode::Quality { quality: None }), &input);
        assert!(out.outputs().is_empty());
        assert_eq!(out.note(), Some(ALREADY_OPTIMIZED_NOTE));
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn target_size_is_met_with_unicode_name_and_collision() {
        let dir = tempfile::tempdir().unwrap();
        let input = save_jpeg(dir.path(), "Férias 夏.jpg", &noisy_rgb(400, 300), 95);
        let size = std::fs::metadata(&input).unwrap().len();
        let target = size / 4;
        let action = compress(CompressMode::TargetSize { bytes: target });
        let first = run_one(action.clone(), &input);
        let second = run_one(action, &input);
        assert_eq!(file_name(first.output()), "Férias 夏 (compressed).jpg");
        assert_eq!(file_name(second.output()), "Férias 夏 (compressed) (1).jpg");
        let len = std::fs::metadata(first.output()).unwrap().len();
        assert!(len <= target, "{len} > {target}");
        open(first.output());
    }

    #[test]
    fn png_target_size_downscales() {
        let dir = tempfile::tempdir().unwrap();
        let input = save(dir.path(), "noise.png", &noisy_rgb(300, 300));
        let size = std::fs::metadata(&input).unwrap().len();
        let target = size / 3;
        let out = run_one(compress(CompressMode::TargetSize { bytes: target }), &input);
        let path = out.output();
        assert!(std::fs::metadata(path).unwrap().len() <= target);
        let (img, format) = open(path);
        assert_eq!(format, ImageFormat::Png);
        assert!(img.width() <= 300 && img.width() == img.height());
    }

    #[test]
    fn convert_to_webp_option() {
        let dir = tempfile::tempdir().unwrap();
        let input = save(dir.path(), "shot.png", &noisy_rgb(120, 80));
        let action = Action::ImageCompress(ImageCompressOpts {
            convert_to_webp: true,
            ..Default::default()
        });
        let out = run_one(action, &input);
        assert_eq!(file_name(out.output()), "shot.webp");
        let (img, format) = open(out.output());
        assert_eq!(format, ImageFormat::WebP);
        assert_eq!((img.width(), img.height()), (120, 80));
    }

    #[test]
    fn png_lossless_optimization_is_smaller() {
        let dir = tempfile::tempdir().unwrap();
        // Flat colors: the image crate's PNG is far from optimal, oxipng shrinks it.
        let input = save(dir.path(), "flat.png", &half_transparent(256, 256));
        let action = Action::ImageCompress(ImageCompressOpts {
            png_level: 4,
            ..Default::default()
        });
        let out = run_one(action, &input);
        let path = out.output();
        assert!(std::fs::metadata(path).unwrap().len() < std::fs::metadata(&input).unwrap().len());
        let (img, _) = open(path);
        assert!(
            img.to_rgba8() == half_transparent(256, 256).to_rgba8(),
            "pixels changed"
        );
    }

    /// A smooth photo-like scene with light grain.
    fn photo_like(width: u32, height: u32) -> DynamicImage {
        let mut seed: u32 = 0x9e37_79b9;
        DynamicImage::ImageRgb8(image::RgbImage::from_fn(width, height, |x, y| {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            let n = (seed % 7) as f32 - 3.0;
            let (fx, fy) = (x as f32 / width as f32, y as f32 / height as f32);
            let r = 120.0 + 80.0 * (fx * 6.0).sin() * (fy * 3.0).cos() + n;
            let g = 110.0 + 70.0 * (fy * 5.0 + fx * 2.0).sin() + n;
            let b = 90.0 + 60.0 * ((fx - 0.5).hypot(fy - 0.5) * 12.0).cos() + n;
            image::Rgb([r as u8, g as u8, b as u8])
        }))
    }

    /// SSIMULACRA2 of `output` against `input` (both decoded as files).
    fn score(input: &std::path::Path, output: &std::path::Path) -> f64 {
        let (original, _) = open(input);
        let (result, _) = open(output);
        let format = OutputFormat::from_extension(&extension_of(output)).unwrap();
        best::Reference::new(&original, format.supports_alpha(), [255; 3])
            .unwrap()
            .score(&result)
            .unwrap()
    }

    fn best() -> Action {
        // Best is the default mode.
        Action::ImageCompress(ImageCompressOpts::default())
    }

    #[test]
    fn best_is_the_default_and_old_state_still_loads() {
        assert_eq!(ImageCompressOpts::default().mode, CompressMode::Best);
        let old: ImageCompressOpts =
            serde_json::from_str(r#"{"mode":{"type":"quality","quality":70}}"#).unwrap();
        assert_eq!(old.mode, CompressMode::Quality { quality: Some(70) });
        let missing: ImageCompressOpts = serde_json::from_str("{}").unwrap();
        assert_eq!(missing.mode, CompressMode::Best);
        let json = serde_json::to_value(CompressMode::Best).unwrap();
        assert_eq!(json, serde_json::json!({"type": "best"}));
    }

    #[test]
    fn best_jpeg_is_smaller_and_looks_identical() {
        let dir = tempfile::tempdir().unwrap();
        let input = save_jpeg(dir.path(), "photo.jpg", &photo_like(640, 480), 95);
        let out = run_one(best(), &input);
        let path = out.output();
        assert_eq!(file_name(path), "photo (compressed).jpg");
        let (before, after) = (
            std::fs::metadata(&input).unwrap().len(),
            std::fs::metadata(path).unwrap().len(),
        );
        assert!(after < before, "{after} >= {before}");
        let s = score(&input, path);
        assert!(s >= best::LOOKS_IDENTICAL_SCORE, "score {s}");
    }

    #[test]
    fn best_on_optimized_file_writes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        // A small low-quality JPEG: anything that looks like it is bigger.
        let bytes = io::encode(
            &photo_like(320, 240),
            OutputFormat::Jpg,
            &EncodeOptions {
                quality: 30,
                ..EncodeOptions::default()
            },
            None,
        )
        .unwrap();
        let input = dir.path().join("tiny.jpg");
        std::fs::write(&input, bytes).unwrap();
        let out = run_one(best(), &input);
        assert!(out.outputs().is_empty());
        assert_eq!(out.note(), Some(ALREADY_OPTIMIZED_NOTE));
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    /// PNG color type from the IHDR chunk (3 = palette).
    fn png_color_type(path: &std::path::Path) -> u8 {
        std::fs::read(path).unwrap()[25]
    }

    #[test]
    fn best_png_with_few_colors_is_quantized_and_keeps_transparency() {
        let dir = tempfile::tempdir().unwrap();
        // Four flat areas with ±1 jitter: thousands of exact colors, but a
        // palette looks the same. The left quarter is fully transparent.
        let mut seed: u32 = 7;
        let img = DynamicImage::ImageRgba8(image::RgbaImage::from_fn(256, 256, |x, y| {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            let j = (seed % 3) as u8;
            match (x / 64, y / 128) {
                (0, _) => image::Rgba([0, 0, 0, 0]),
                (1, 0) => image::Rgba([200 + j, 40 + j, 40, 255]),
                (1, _) => image::Rgba([40, 180 + j, 60 + j, 255]),
                (_, 0) => image::Rgba([30 + j, 60, 200 + j, 255]),
                _ => image::Rgba([240, 230 + j, 120 + j, 255]),
            }
        }));
        let input = save(dir.path(), "chart.png", &img);
        let out = run_one(best(), &input);
        let path = out.output();
        assert_eq!(png_color_type(path), 3, "palette PNG");
        assert!(std::fs::metadata(path).unwrap().len() < std::fs::metadata(&input).unwrap().len());
        assert!(score(&input, path) >= best::LOOKS_IDENTICAL_SCORE);
        let (result, _) = open(path);
        let result = result.to_rgba8();
        assert_eq!(
            result.get_pixel(10, 10)[3],
            0,
            "transparent stays transparent"
        );
        assert_eq!(result.get_pixel(100, 10)[3], 255, "opaque stays opaque");
    }

    #[test]
    fn best_png_photo_stays_lossless() {
        let dir = tempfile::tempdir().unwrap();
        let img = photo_like(256, 192);
        let input = save(dir.path(), "photo.png", &img);
        let out = run_one(best(), &input);
        let path = out.output();
        assert_ne!(png_color_type(path), 3, "not quantized");
        let (result, _) = open(path);
        assert!(result.to_rgb8() == img.to_rgb8(), "pixels changed");
    }

    /// Thin saturated red strokes on blue: chroma subsampling smears them.
    fn red_text_on_blue(width: u32, height: u32) -> DynamicImage {
        DynamicImage::ImageRgb8(image::RgbImage::from_fn(width, height, |x, y| {
            let stroke = (x % 9 < 2 && y % 24 < 16) || (y % 24 == 7 && x % 36 < 20);
            if stroke {
                image::Rgb([230, 20, 20])
            } else {
                image::Rgb([20, 40, 210])
            }
        }))
    }

    fn encode_jpeg(img: &DynamicImage, quality: u8, chroma_444: bool) -> Vec<u8> {
        let opts = EncodeOptions {
            quality,
            chroma_444,
            ..EncodeOptions::default()
        };
        io::encode(img, OutputFormat::Jpg, &opts, None).unwrap()
    }

    #[test]
    fn draft_jpeg_has_the_same_pixels() {
        let img = photo_like(256, 192);
        for chroma_444 in [false, true] {
            let full = encode_jpeg(&img, 80, chroma_444);
            let opts = EncodeOptions {
                quality: 80,
                chroma_444,
                draft: true,
                ..EncodeOptions::default()
            };
            let draft = io::encode(&img, OutputFormat::Jpg, &opts, None).unwrap();
            let decode = |b: &[u8]| {
                io::decode_bytes(b, None, &DecodeOptions::default())
                    .unwrap()
                    .image
                    .to_rgb8()
            };
            assert!(decode(&full) == decode(&draft), "444: {chroma_444}");
        }
    }

    #[test]
    fn jpeg_subsampling_is_detected() {
        let img = red_text_on_blue(64, 48);
        assert_eq!(io::jpeg_is_444(&encode_jpeg(&img, 90, true)), Some(true));
        assert_eq!(io::jpeg_is_444(&encode_jpeg(&img, 90, false)), Some(false));
        let gray = DynamicImage::ImageLuma8(img.to_luma8());
        let gray = io::encode(&gray, OutputFormat::Jpg, &EncodeOptions::default(), None).unwrap();
        assert_eq!(io::jpeg_is_444(&gray), Some(true));
        assert_eq!(io::jpeg_is_444(b"\x89PNG...."), None);
        assert_eq!(io::jpeg_is_444(&[0xFF, 0xD8, 0xFF]), None);
    }

    #[test]
    fn best_red_text_on_blue_never_writes_a_failed_candidate() {
        let dir = tempfile::tempdir().unwrap();
        // A small 4:2:0 JPEG: the 4:4:4 that would look the same is bigger.
        let input = dir.path().join("slide.jpg");
        std::fs::write(&input, encode_jpeg(&red_text_on_blue(320, 240), 60, true)).unwrap();
        let out = run_one(best(), &input);
        for path in out.outputs() {
            assert!(
                score(&input, path) >= best::LOOKS_IDENTICAL_SCORE,
                "failed candidate"
            );
        }
        if out.outputs().is_empty() {
            assert_eq!(out.note(), Some(ALREADY_OPTIMIZED_NOTE));
            assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
        }
    }

    #[test]
    fn best_convert_to_webp_always_makes_a_webp_that_looks_the_same() {
        let dir = tempfile::tempdir().unwrap();
        let input = save(dir.path(), "slide.png", &red_text_on_blue(320, 240));
        // Lossy WebP is always 4:2:0 and can't keep the red strokes: lossless it is.
        let action = Action::ImageCompress(ImageCompressOpts {
            convert_to_webp: true,
            ..ImageCompressOpts::default()
        });
        let out = run_one(action, &input);
        let path = out.output();
        assert_eq!(file_name(path), "slide.webp");
        assert_eq!(open(path).1, ImageFormat::WebP);
        assert!(score(&input, path) >= best::LOOKS_IDENTICAL_SCORE);
    }

    #[test]
    fn best_keeps_444_for_saturated_colour() {
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("slide.jpg");
        std::fs::write(&input, encode_jpeg(&red_text_on_blue(320, 240), 100, true)).unwrap();
        let out = run_one(best(), &input);
        let path = out.output();
        let bytes = std::fs::read(path).unwrap();
        assert_eq!(io::jpeg_is_444(&bytes), Some(true), "4:4:4 kept");
        assert!(bytes.len() < std::fs::read(&input).unwrap().len());
        assert!(score(&input, path) >= best::LOOKS_IDENTICAL_SCORE);
    }

    #[test]
    fn best_tries_444_when_420_cannot_pass() {
        // Input not known to be 4:4:4 (e.g. HEIC → JPG): 4:2:0 is searched first,
        // can't keep the red strokes, then 4:4:4 passes.
        let img = red_text_on_blue(160, 120);
        let meta = Metadata::default();
        let opts = ImageCompressOpts::default();
        let encoder = Encoder {
            format: OutputFormat::Jpg,
            opts: &opts,
            meta: &meta,
        };
        let cancel = crate::cancel::CancelToken::new();
        let progress = |_: f32, _| {};
        let temp = tempfile::tempdir().unwrap();
        let ctx = TaskCtx::new(&cancel, &progress, temp.path());
        let bytes = best_bytes(&encoder, &img, u64::MAX, false, &ctx)
            .unwrap()
            .expect("4:4:4 passes");
        assert_eq!(io::jpeg_is_444(&bytes), Some(true));
    }

    #[test]
    fn best_strips_metadata_without_growing_a_low_quality_file() {
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("low.jpg");
        std::fs::write(&input, encode_jpeg(&photo_like(320, 240), 30, false)).unwrap();
        add_exif_to_jpeg(&input, exif_bytes(1, true));
        let before = std::fs::metadata(&input).unwrap().len();
        let action = Action::ImageCompress(ImageCompressOpts {
            keep_metadata: false,
            ..ImageCompressOpts::default()
        });
        let out = run_one(action, &input);
        let path = out.output();
        assert!(std::fs::metadata(path).unwrap().len() <= before);
        assert!(exif_fields(path).is_empty(), "metadata removed");
        // Pixels untouched: the metadata was removed without re-encoding.
        assert!(open(path).0.to_rgb8() == open(&input).0.to_rgb8());
    }

    #[test]
    fn best_stops_when_cancelled_mid_search() {
        use crate::cancel::CancelToken;
        use crate::job::{Job, OutputSpec};
        let dir = tempfile::tempdir().unwrap();
        let input = save_jpeg(dir.path(), "photo.jpg", &photo_like(320, 240), 95);
        let job = Job::for_files(
            std::slice::from_ref(&input),
            &best(),
            &OutputSpec::default(),
        );
        let cancel = CancelToken::new();
        let calls = std::sync::atomic::AtomicU32::new(0);
        // Cancel once the first candidate has been judged.
        let progress = |fraction: f32, _| {
            if fraction > 0.1 {
                calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                cancel.cancel();
            }
        };
        let temp = tempfile::tempdir().unwrap();
        let ctx = TaskCtx::new(&cancel, &progress, temp.path());
        let r = ImageCompress.run(&job.tasks[0], &ctx);
        assert!(r.is_err_and(|e| e.is_cancelled()));
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn exif_orientation_applied_on_compress() {
        let dir = tempfile::tempdir().unwrap();
        let input = save_jpeg(dir.path(), "side.jpg", &noisy_rgb(60, 30), 100);
        add_exif_to_jpeg(&input, exif_bytes(8, false));
        let out = run_one(
            compress(CompressMode::Quality { quality: Some(50) }),
            &input,
        );
        let (img, _) = open(out.output());
        assert_eq!((img.width(), img.height()), (30, 60));
    }
}
