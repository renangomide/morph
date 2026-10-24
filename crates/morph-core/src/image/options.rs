// SPDX-License-Identifier: GPL-3.0-or-later
//! Image action options (SPEC §4.1). `Default` = the Settings defaults (SPEC §3.4).

use serde::{Deserialize, Serialize};

/// Image output formats.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OutputFormat {
    Jpg,
    Png,
    Webp,
    Avif,
    Bmp,
    Tiff,
    Gif,
    Ico,
}

impl OutputFormat {
    pub const ALL: [OutputFormat; 8] = [
        OutputFormat::Jpg,
        OutputFormat::Png,
        OutputFormat::Webp,
        OutputFormat::Avif,
        OutputFormat::Bmp,
        OutputFormat::Tiff,
        OutputFormat::Gif,
        OutputFormat::Ico,
    ];

    /// File extension (no dot) used for outputs.
    pub fn extension(self) -> &'static str {
        match self {
            OutputFormat::Jpg => "jpg",
            OutputFormat::Png => "png",
            OutputFormat::Webp => "webp",
            OutputFormat::Avif => "avif",
            OutputFormat::Bmp => "bmp",
            OutputFormat::Tiff => "tiff",
            OutputFormat::Gif => "gif",
            OutputFormat::Ico => "ico",
        }
    }

    /// Format for an existing file's extension, if it is one we can write.
    pub fn from_extension(ext: &str) -> Option<OutputFormat> {
        Some(match ext.to_ascii_lowercase().as_str() {
            "jpg" | "jpeg" | "jfif" => OutputFormat::Jpg,
            "png" => OutputFormat::Png,
            "webp" => OutputFormat::Webp,
            "avif" => OutputFormat::Avif,
            "bmp" => OutputFormat::Bmp,
            "tif" | "tiff" => OutputFormat::Tiff,
            "gif" => OutputFormat::Gif,
            "ico" => OutputFormat::Ico,
            _ => return None,
        })
    }

    /// Has a quality slider.
    pub fn is_lossy(self) -> bool {
        matches!(
            self,
            OutputFormat::Jpg | OutputFormat::Webp | OutputFormat::Avif
        )
    }

    /// Supports the "Lossless" toggle.
    pub fn has_lossless_mode(self) -> bool {
        matches!(self, OutputFormat::Webp | OutputFormat::Avif)
    }

    /// Default quality (Settings → Defaults). 0 for formats without quality.
    pub fn default_quality(self) -> u8 {
        match self {
            OutputFormat::Jpg => 85,
            OutputFormat::Webp => 80,
            OutputFormat::Avif => 60,
            _ => 0,
        }
    }

    pub fn supports_alpha(self) -> bool {
        !matches!(self, OutputFormat::Jpg | OutputFormat::Bmp)
    }
}

pub const DEFAULT_ICO_SIZES: [u32; 6] = [16, 32, 48, 64, 128, 256];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ImageConvertOpts {
    pub format: OutputFormat,
    /// 1–100 for lossy formats; `None` = the format's default.
    pub quality: Option<u8>,
    /// WebP/AVIF lossless mode.
    pub lossless: bool,
    /// Fill color when transparency can't be kept (e.g. → JPG). RGB.
    pub background: [u8; 3],
    /// ICO output sizes (square, padded).
    pub ico_sizes: Vec<u32>,
    /// Width in pixels for SVG input.
    pub svg_width: u32,
    pub keep_metadata: bool,
    /// With `keep_metadata`: still remove the location (GPS) from EXIF/XMP.
    pub remove_gps: bool,
}

impl Default for ImageConvertOpts {
    fn default() -> Self {
        ImageConvertOpts {
            format: OutputFormat::Png,
            quality: None,
            lossless: false,
            background: [255, 255, 255],
            ico_sizes: DEFAULT_ICO_SIZES.to_vec(),
            svg_width: 1024,
            keep_metadata: true,
            remove_gps: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CompressMode {
    /// Smallest file that still looks identical (perceptual check), same format.
    /// The default; see `image::best`.
    #[default]
    Best,
    /// Re-encode at this quality (`None` = format default).
    Quality { quality: Option<u8> },
    /// Binary search on quality (max 8 tries), then downscale, to get ≤ `bytes`.
    TargetSize { bytes: u64 },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ImageCompressOpts {
    pub mode: CompressMode,
    /// Output WebP instead of keeping the format.
    pub convert_to_webp: bool,
    /// PNG: lossy palette quantization (imagequant) on top of lossless optimization.
    /// Quality and target-size modes only; Best decides by itself.
    pub png_lossy: bool,
    /// PNG lossless optimization level 0–6 (oxipng). Settings default: 2.
    pub png_level: u8,
    pub keep_metadata: bool,
    /// With `keep_metadata`: still remove the location (GPS) from EXIF/XMP.
    pub remove_gps: bool,
}

impl Default for ImageCompressOpts {
    fn default() -> Self {
        ImageCompressOpts {
            mode: CompressMode::Best,
            convert_to_webp: false,
            png_lossy: false,
            png_level: 2,
            keep_metadata: true,
            remove_gps: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ResizeMode {
    Percent { percent: f32 },
    MaxWidth { width: u32 },
    MaxHeight { height: u32 },
    Exact { width: u32, height: u32 },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ImageResizeOpts {
    pub mode: ResizeMode,
    /// Only for `Exact`: keep the aspect ratio by fitting inside W×H.
    pub keep_aspect: bool,
    /// Never make an image bigger.
    pub no_enlarge: bool,
    pub keep_metadata: bool,
    /// With `keep_metadata`: still remove the location (GPS) from EXIF/XMP.
    pub remove_gps: bool,
}

impl Default for ImageResizeOpts {
    fn default() -> Self {
        ImageResizeOpts {
            mode: ResizeMode::Percent { percent: 50.0 },
            keep_aspect: true,
            no_enlarge: true,
            keep_metadata: true,
            remove_gps: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StripMode {
    /// Remove all EXIF and XMP (the ICC color profile is kept so colors stay right).
    All,
    /// Remove only GPS location.
    GpsOnly,
    /// Keep metadata.
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Rotation {
    None,
    R90,
    R180,
    R270,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ImageCleanOpts {
    pub strip: StripMode,
    pub rotate: Rotation,
    pub flip_horizontal: bool,
    pub flip_vertical: bool,
}

impl Default for ImageCleanOpts {
    fn default() -> Self {
        ImageCleanOpts {
            strip: StripMode::All,
            rotate: Rotation::None,
            flip_horizontal: false,
            flip_vertical: false,
        }
    }
}

/// Image → PDF, one PDF per image (SPEC §4.6). Same page options as Merge's
/// image pages (`crate::pdf::merge`); combining several images into one PDF is
/// `pdf.merge`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ImageToPdfOpts {
    pub page_size: crate::pdf::options::PageSize,
    /// Margin around the image, in millimetres (0 = none).
    pub margin_mm: u32,
}

impl Default for ImageToPdfOpts {
    fn default() -> Self {
        ImageToPdfOpts {
            page_size: crate::pdf::options::PageSize::FitToImage,
            margin_mm: 0,
        }
    }
}
