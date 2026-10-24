// SPDX-License-Identifier: GPL-3.0-or-later
//! Helpers shared by the four image actions (private to `image`).

use crate::error::ConvertError;
use crate::image::io::{self, Metadata};
use crate::image::options::{DEFAULT_ICO_SIZES, OutputFormat};
use crate::job::Task;
use fast_image_resize::{ResizeAlg, ResizeOptions, Resizer};
use image::DynamicImage;
use std::path::Path;

/// Note for animated inputs (SPEC §4.1).
pub(crate) const ANIMATED_NOTE: &str = "Animated image: only the first frame was converted.";

/// Inputs above this many pixels get [`large_image_note`] (SPEC §10).
pub(crate) const LARGE_IMAGE_PIXELS: u64 = 100_000_000;

/// Warning for very large inputs (SPEC §10).
pub(crate) fn large_image_note(w: u32, h: u32) -> String {
    format!("Very large image ({w} × {h}): this used a lot of memory and time.")
}

/// Largest output we agree to allocate (pixels). Protects against absurd resize values.
const MAX_PIXELS: u64 = 1_000_000_000;

/// The task's input file, or an error if the task has none.
pub(crate) fn task_input(task: &Task) -> Result<&Path, ConvertError> {
    task.input()
        .ok_or_else(|| ConvertError::other("No file to convert.", "task has no inputs"))
}

/// Error for a task whose action doesn't match the converter.
pub(crate) fn wrong_action(converter: &str, task: &Task) -> ConvertError {
    ConvertError::other(
        "Something went wrong while converting this file.",
        format!("{converter} got action '{}'", task.action.id()),
    )
}

/// Fail early (before decoding) when the output is WebP and the input is larger
/// than WebP allows. Inputs whose header can't be read are left to the decoder.
pub(crate) fn check_webp_input(path: &Path, opts: &io::DecodeOptions) -> Result<(), ConvertError> {
    match io::probe_dimensions_with(path, opts) {
        Ok((w, h)) if w > io::WEBP_MAX_SIDE || h > io::WEBP_MAX_SIDE => {
            Err(io::webp_too_large(w, h))
        }
        _ => Ok(()),
    }
}

/// The input file's lowercase extension ("" if none).
pub(crate) fn extension_of(path: &Path) -> String {
    path.extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default()
}

/// Output format that keeps the input's format when we can write it.
///
/// Returns the format and, when the input format can't be written (HEIC, SVG,
/// TGA, QOI…), a note explaining the fallback: JPG for opaque images, PNG when
/// the image has transparency.
pub(crate) fn same_or_fallback_format(
    input_ext: &str,
    has_transparency: bool,
) -> (OutputFormat, Option<String>) {
    if let Some(format) = OutputFormat::from_extension(input_ext) {
        return (format, None);
    }
    let format = if has_transparency {
        OutputFormat::Png
    } else {
        OutputFormat::Jpg
    };
    let shown = if input_ext.is_empty() {
        "This format".to_owned()
    } else {
        format!("{} files", input_ext.to_ascii_uppercase())
    };
    let note = format!(
        "{shown} can't be saved, so the result is a {}.",
        format.extension().to_ascii_uppercase()
    );
    (format, Some(note))
}

/// True when at least one pixel is not fully opaque.
pub(crate) fn has_transparency(img: &DynamicImage) -> bool {
    if !img.color().has_alpha() {
        return false;
    }
    match img {
        DynamicImage::ImageLumaA8(buf) => buf.pixels().any(|p| p[1] < u8::MAX),
        DynamicImage::ImageRgba8(buf) => buf.pixels().any(|p| p[3] < u8::MAX),
        DynamicImage::ImageLumaA16(buf) => buf.pixels().any(|p| p[1] < u16::MAX),
        DynamicImage::ImageRgba16(buf) => buf.pixels().any(|p| p[3] < u16::MAX),
        other => other.to_rgba8().pixels().any(|p| p[3] < u8::MAX),
    }
}

/// Metadata to embed: the ICC profile always; EXIF/XMP only when `keep_all`.
pub(crate) fn metadata_to_keep(meta: &Metadata, keep_all: bool, remove_gps: bool) -> Metadata {
    if keep_all && remove_gps {
        // EXIF/XMP that can't be cleaned are dropped, so location never survives.
        Metadata {
            exif: meta.exif.as_deref().and_then(io::strip_gps),
            icc: meta.icc.clone(),
            xmp: meta.xmp.as_deref().and_then(io::strip_xmp_gps),
        }
    } else if keep_all {
        meta.clone()
    } else {
        Metadata {
            exif: None,
            icc: meta.icc.clone(),
            xmp: None,
        }
    }
}

/// ICO sizes for re-encoding an existing image without upscaling it:
/// the default sizes up to the image's larger side (or just that side if smaller than 16).
pub(crate) fn ico_sizes_for(width: u32, height: u32) -> Vec<u32> {
    let side = width.max(height).clamp(1, 256);
    let sizes: Vec<u32> = DEFAULT_ICO_SIZES
        .iter()
        .copied()
        .filter(|&s| s <= side)
        .collect();
    if sizes.is_empty() { vec![side] } else { sizes }
}

/// Quality for a format: `requested` if any (clamped to 1–100), else the format default.
pub(crate) fn quality_for(format: OutputFormat, requested: Option<u8>) -> u8 {
    match requested {
        Some(q) if format.is_lossy() => q.clamp(1, 100),
        _ => format.default_quality(),
    }
}

/// Join optional notes into one line.
pub(crate) fn join_notes(notes: Vec<String>) -> Option<String> {
    if notes.is_empty() {
        None
    } else {
        Some(notes.join(" "))
    }
}

/// Resize with Lanczos3. Unsupported pixel layouts are converted to RGBA8 first.
pub(crate) fn resize_lanczos(
    img: &DynamicImage,
    width: u32,
    height: u32,
) -> Result<DynamicImage, ConvertError> {
    let (width, height) = (width.max(1), height.max(1));
    if (img.width(), img.height()) == (width, height) {
        return Ok(img.clone());
    }
    if u64::from(width) * u64::from(height) > MAX_PIXELS {
        return Err(ConvertError::unsupported(
            "The resized image would be too large.",
            format!("{width}×{height} exceeds {MAX_PIXELS} pixels"),
        ));
    }
    let options = ResizeOptions::new().resize_alg(ResizeAlg::Convolution(
        fast_image_resize::FilterType::Lanczos3,
    ));
    let mut resizer = Resizer::new();
    let mut dst = DynamicImage::new(width, height, img.color());
    match resizer.resize(img, &mut dst, &options) {
        Ok(()) => Ok(dst),
        Err(_) => {
            let src = DynamicImage::ImageRgba8(img.to_rgba8());
            let mut dst = DynamicImage::new_rgba8(width, height);
            resizer
                .resize(&src, &mut dst, &options)
                .map_err(|e| ConvertError::other("Couldn't resize the image.", e))?;
            Ok(dst)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writable_formats_are_kept() {
        assert_eq!(
            same_or_fallback_format("png", true),
            (OutputFormat::Png, None)
        );
        assert_eq!(
            same_or_fallback_format("JFIF", false),
            (OutputFormat::Jpg, None)
        );
        assert_eq!(
            same_or_fallback_format("tif", false),
            (OutputFormat::Tiff, None)
        );
    }

    #[test]
    fn unwritable_formats_fall_back_by_transparency() {
        for ext in ["heic", "heif", "svg", "tga", "qoi"] {
            let (f, note) = same_or_fallback_format(ext, false);
            assert_eq!(f, OutputFormat::Jpg, "{ext}");
            assert!(note.is_some_and(|n| n.contains("JPG")));
            let (f, note) = same_or_fallback_format(ext, true);
            assert_eq!(f, OutputFormat::Png, "{ext}");
            assert!(note.is_some_and(|n| n.contains("PNG")));
        }
    }

    #[test]
    fn transparency_checks_pixels_not_just_channels() {
        let opaque = DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            2,
            2,
            image::Rgba([1, 2, 3, 255]),
        ));
        assert!(!has_transparency(&opaque));
        assert!(has_transparency(&DynamicImage::new_rgba8(2, 2)));
        assert!(!has_transparency(&DynamicImage::new_rgb8(2, 2)));
    }

    #[test]
    fn metadata_keeps_icc_always() {
        let meta = Metadata {
            exif: Some(vec![1]),
            icc: Some(vec![2]),
            xmp: Some(vec![3]),
        };
        let kept = metadata_to_keep(&meta, false, false);
        assert_eq!(kept.icc, Some(vec![2]));
        assert!(kept.exif.is_none() && kept.xmp.is_none());
        assert_eq!(metadata_to_keep(&meta, true, false), meta);
    }

    #[test]
    fn ico_sizes_never_upscale() {
        assert_eq!(ico_sizes_for(40, 20), vec![16, 32]);
        assert_eq!(ico_sizes_for(8, 8), vec![8]);
        assert_eq!(ico_sizes_for(4000, 3000), DEFAULT_ICO_SIZES.to_vec());
    }

    #[test]
    fn resize_handles_odd_pixel_types() {
        let img = DynamicImage::new_rgb8(10, 6);
        let out = resize_lanczos(&img, 5, 3).unwrap();
        assert_eq!((out.width(), out.height()), (5, 3));
        let img = DynamicImage::ImageLumaA8(image::GrayAlphaImage::new(10, 6));
        let out = resize_lanczos(&img, 3, 2).unwrap();
        assert_eq!((out.width(), out.height()), (3, 2));
    }
}

/// Helpers for the actions' integration tests.
#[cfg(test)]
pub(crate) mod test_util {
    use crate::action::Action;
    use crate::cancel::CancelToken;
    use crate::event::Event;
    use crate::job::{Job, OutputSpec};
    use crate::scheduler::{JobConfig, run_job_blocking};
    use image::{DynamicImage, Rgb, RgbImage, Rgba, RgbaImage};
    use std::path::{Path, PathBuf};

    /// Result of one finished task.
    #[derive(Debug)]
    pub(crate) enum Outcome {
        Done {
            outputs: Vec<PathBuf>,
            note: Option<String>,
        },
        Failed {
            error: String,
            details: String,
        },
    }

    impl Outcome {
        pub(crate) fn outputs(&self) -> &[PathBuf] {
            match self {
                Outcome::Done { outputs, .. } => outputs,
                Outcome::Failed { error, details } => panic!("task failed: {error} ({details})"),
            }
        }

        pub(crate) fn output(&self) -> &Path {
            let outputs = self.outputs();
            assert_eq!(outputs.len(), 1, "expected one output");
            &outputs[0]
        }

        pub(crate) fn note(&self) -> Option<&str> {
            match self {
                Outcome::Done { note, .. } => note.as_deref(),
                Outcome::Failed { .. } => None,
            }
        }
    }

    /// Run `action` on `files` (outputs next to the inputs); one outcome per task, in task order.
    pub(crate) fn run(action: Action, files: &[PathBuf]) -> Vec<Outcome> {
        let job = Job::for_files(files, &action, &OutputSpec::default());
        assert!(
            job.skipped.is_empty(),
            "inputs were skipped: {:?}",
            job.skipped
        );
        let n = job.tasks.len();
        let (tx, rx) = crossbeam_channel::unbounded();
        let temp = tempfile::tempdir().unwrap();
        let config = JobConfig {
            cpu_threads: 2,
            temp_root: temp.path().to_path_buf(),
            memory_limit: None,
            ..JobConfig::default()
        };
        run_job_blocking(job, &config, &CancelToken::new(), &tx);
        drop(tx);
        let mut outcomes: Vec<Option<Outcome>> = (0..n).map(|_| None).collect();
        for event in rx.iter() {
            match event {
                Event::TaskDone {
                    id, outputs, note, ..
                } => outcomes[id.0 as usize] = Some(Outcome::Done { outputs, note }),
                Event::TaskFailed { id, error, details } => {
                    outcomes[id.0 as usize] = Some(Outcome::Failed { error, details })
                }
                Event::TaskSkipped { reason, .. } => panic!("task skipped: {reason}"),
                _ => {}
            }
        }
        outcomes
            .into_iter()
            .map(|o| o.expect("every task reports"))
            .collect()
    }

    /// Run one file and return its outcome.
    pub(crate) fn run_one(action: Action, file: &Path) -> Outcome {
        run(action, &[file.to_path_buf()]).remove(0)
    }

    /// A noisy opaque RGB test image (noise makes compression measurable).
    pub(crate) fn noisy_rgb(width: u32, height: u32) -> DynamicImage {
        let mut seed: u32 = 0x1234_5678;
        DynamicImage::ImageRgb8(RgbImage::from_fn(width, height, |x, y| {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            let n = (seed & 0x3f) as u8;
            Rgb([
                (x * 255 / width.max(1)) as u8 / 2 + n,
                (y * 255 / height.max(1)) as u8 / 2 + n,
                128u8.wrapping_add(n),
            ])
        }))
    }

    /// An RGBA image whose left half is transparent.
    pub(crate) fn half_transparent(width: u32, height: u32) -> DynamicImage {
        DynamicImage::ImageRgba8(RgbaImage::from_fn(width, height, |x, _| {
            if x < width / 2 {
                Rgba([0, 0, 0, 0])
            } else {
                Rgba([200, 30, 30, 255])
            }
        }))
    }

    /// Save `img` as `dir/name` (format from the extension).
    pub(crate) fn save(dir: &Path, name: &str, img: &DynamicImage) -> PathBuf {
        let path = dir.join(name);
        img.save(&path).unwrap();
        path
    }

    /// Save a JPEG at a given quality.
    pub(crate) fn save_jpeg(dir: &Path, name: &str, img: &DynamicImage, quality: u8) -> PathBuf {
        let path = dir.join(name);
        let mut bytes = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, quality)
            .encode_image(img)
            .unwrap();
        std::fs::write(&path, bytes).unwrap();
        path
    }

    /// Decode a file, guessing the format from its content.
    pub(crate) fn open(path: &Path) -> (DynamicImage, image::ImageFormat) {
        let reader = image::ImageReader::open(path)
            .unwrap()
            .with_guessed_format()
            .unwrap();
        let format = reader.format().unwrap();
        (reader.decode().unwrap(), format)
    }

    pub(crate) fn file_name(path: &Path) -> String {
        path.file_name().unwrap().to_string_lossy().into_owned()
    }

    /// Minimal little-endian EXIF (TIFF) with an orientation tag and, optionally,
    /// a GPS IFD holding GPSLatitudeRef = "N".
    pub(crate) fn exif_bytes(orientation: u16, with_gps: bool) -> Vec<u8> {
        let mut b: Vec<u8> = vec![b'I', b'I', 42, 0, 8, 0, 0, 0];
        let count: u16 = if with_gps { 2 } else { 1 };
        b.extend_from_slice(&count.to_le_bytes());
        // Orientation (0x0112), SHORT, 1 value.
        b.extend_from_slice(&0x0112u16.to_le_bytes());
        b.extend_from_slice(&3u16.to_le_bytes());
        b.extend_from_slice(&1u32.to_le_bytes());
        b.extend_from_slice(&orientation.to_le_bytes());
        b.extend_from_slice(&[0, 0]);
        let gps_ifd_offset: u32 = 8 + 2 + 12 * u32::from(count) + 4;
        if with_gps {
            // GPSInfo IFD pointer (0x8825), LONG.
            b.extend_from_slice(&0x8825u16.to_le_bytes());
            b.extend_from_slice(&4u16.to_le_bytes());
            b.extend_from_slice(&1u32.to_le_bytes());
            b.extend_from_slice(&gps_ifd_offset.to_le_bytes());
        }
        b.extend_from_slice(&0u32.to_le_bytes()); // no next IFD
        if with_gps {
            b.extend_from_slice(&1u16.to_le_bytes());
            // GPSLatitudeRef (0x0001), ASCII, 2 bytes "N\0".
            b.extend_from_slice(&1u16.to_le_bytes());
            b.extend_from_slice(&2u16.to_le_bytes());
            b.extend_from_slice(&2u32.to_le_bytes());
            b.extend_from_slice(&[b'N', 0, 0, 0]);
            b.extend_from_slice(&0u32.to_le_bytes());
        }
        b
    }

    /// Insert raw EXIF into a JPEG file.
    pub(crate) fn add_exif_to_jpeg(path: &Path, exif: Vec<u8>) {
        use img_parts::ImageEXIF;
        let bytes = std::fs::read(path).unwrap();
        let mut jpeg = img_parts::jpeg::Jpeg::from_bytes(img_parts::Bytes::from(bytes)).unwrap();
        jpeg.set_exif(Some(img_parts::Bytes::from(exif)));
        let mut out = Vec::new();
        jpeg.encoder().write_to(&mut out).unwrap();
        std::fs::write(path, out).unwrap();
    }

    /// Parsed EXIF fields of a file (empty if it has none).
    pub(crate) fn exif_fields(path: &Path) -> Vec<(exif::Tag, exif::In)> {
        let file = std::fs::File::open(path).unwrap();
        let mut reader = std::io::BufReader::new(file);
        match exif::Reader::new().read_from_container(&mut reader) {
            Ok(e) => e.fields().map(|f| (f.tag, f.ifd_num)).collect(),
            Err(_) => Vec::new(),
        }
    }
}
