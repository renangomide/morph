// SPDX-License-Identifier: GPL-3.0-or-later
//! PDF action options (SPEC §4.2). `Default` = the Settings defaults (SPEC §3.4):
//! compression preset Medium, PDF → images at 150 DPI.

use crate::image::options::OutputFormat;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------- Compress

/// Compression strength. Each preset caps the resolution of embedded raster
/// images and sets the JPEG quality they are re-encoded with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompressPreset {
    /// Images ≤ 300 DPI, JPEG q85.
    Low,
    /// Images ≤ 150 DPI, JPEG q75.
    #[default]
    Medium,
    /// Images ≤ 96 DPI, JPEG q60.
    High,
    /// `dpi` and `quality` from the options.
    Custom,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PdfCompressOpts {
    pub preset: CompressPreset,
    /// Maximum image resolution; used only with [`CompressPreset::Custom`].
    pub dpi: u32,
    /// JPEG quality 1–100; used only with [`CompressPreset::Custom`].
    pub quality: u8,
}

impl Default for PdfCompressOpts {
    fn default() -> Self {
        PdfCompressOpts {
            preset: CompressPreset::Medium,
            dpi: 150,
            quality: 75,
        }
    }
}

impl PdfCompressOpts {
    /// `(maximum image DPI, JPEG quality)` for the chosen preset.
    pub fn target(&self) -> (u32, u8) {
        match self.preset {
            CompressPreset::Low => (300, 85),
            CompressPreset::Medium => (150, 75),
            CompressPreset::High => (96, 60),
            CompressPreset::Custom => (self.dpi, self.quality),
        }
    }
}

// ---------------------------------------------------------------- To images

/// Image formats PDF pages can be rendered to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PageImageFormat {
    #[default]
    Png,
    Jpg,
    Webp,
}

impl PageImageFormat {
    pub const ALL: [PageImageFormat; 3] = [
        PageImageFormat::Png,
        PageImageFormat::Jpg,
        PageImageFormat::Webp,
    ];

    /// The image encoder format (encode with `crate::image::io::encode`;
    /// quality 0 = the Settings default for JPG/WebP).
    pub fn output_format(self) -> OutputFormat {
        match self {
            PageImageFormat::Png => OutputFormat::Png,
            PageImageFormat::Jpg => OutputFormat::Jpg,
            PageImageFormat::Webp => OutputFormat::Webp,
        }
    }

    pub fn extension(self) -> &'static str {
        self.output_format().extension()
    }
}

/// DPI choices shown in the window; any other value is "Custom".
pub const DPI_PRESETS: [u32; 3] = [72, 150, 300];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PdfToImagesOpts {
    pub format: PageImageFormat,
    pub dpi: u32,
    /// Page range like `1-3,7,10-`; empty = all pages. Parse with
    /// [`crate::pdf::pages::parse_ranges`].
    pub pages: String,
}

impl Default for PdfToImagesOpts {
    fn default() -> Self {
        PdfToImagesOpts {
            format: PageImageFormat::Png,
            dpi: 150,
            pages: String::new(),
        }
    }
}

// ---------------------------------------------------------------- Extract text

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PdfToTextOpts {
    /// Put a `--- Page N ---` line before each page's text.
    pub page_separators: bool,
}

// ---------------------------------------------------------------- Split

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SplitMode {
    /// One file per page.
    #[default]
    EveryPage,
    /// One file per `every` pages.
    EveryN,
    /// One file per range in `ranges` (e.g. `1-3,4-10,11-`).
    Ranges,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PdfSplitOpts {
    pub mode: SplitMode,
    /// Pages per file for [`SplitMode::EveryN`].
    pub every: u32,
    /// Ranges for [`SplitMode::Ranges`]; each range becomes one file.
    pub ranges: String,
}

impl Default for PdfSplitOpts {
    fn default() -> Self {
        PdfSplitOpts {
            mode: SplitMode::EveryPage,
            every: 2,
            ranges: String::new(),
        }
    }
}

// ---------------------------------------------------------------- Merge

/// Page size for pages made from images (PDF pages are copied as they are).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PageSize {
    /// The page is exactly the image (plus margins), at the image's DPI (96 if unknown).
    #[default]
    FitToImage,
    A4,
    Letter,
}

impl PageSize {
    /// `(width, height)` in PDF points (1/72 inch), portrait. `None` for
    /// [`PageSize::FitToImage`].
    pub fn points(self) -> Option<(f32, f32)> {
        match self {
            PageSize::FitToImage => None,
            PageSize::A4 => Some((595.276, 841.89)),
            PageSize::Letter => Some((612.0, 792.0)),
        }
    }
}

/// Merge (also "Combine into one PDF"): N inputs → one `Merged.pdf`, in task input order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PdfMergeOpts {
    /// Page size for image inputs.
    pub page_size: PageSize,
    /// Margin around images, in millimetres (0 = none).
    pub margin_mm: u32,
    /// One bookmark per source file, pointing at its first page.
    pub bookmarks: bool,
}

impl Default for PdfMergeOpts {
    fn default() -> Self {
        PdfMergeOpts {
            page_size: PageSize::FitToImage,
            margin_mm: 0,
            bookmarks: true,
        }
    }
}

// ---------------------------------------------------------------- To Word

/// PDF → Word (DOCX). No options yet; the engine is Settings → Office engine.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PdfToWordOpts {}

// ---------------------------------------------------------------- To Markdown / HTML

/// Output of `pdf.to_markdown` (SPEC §4.6). Same strings as `DocFormat`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PdfMarkupFormat {
    /// Markdown (`name.md`).
    #[default]
    Md,
    /// HTML (`name.html`): the same reader → Markdown → HTML page.
    Html,
}

impl PdfMarkupFormat {
    pub const ALL: [PdfMarkupFormat; 2] = [PdfMarkupFormat::Md, PdfMarkupFormat::Html];

    pub fn extension(self) -> &'static str {
        match self {
            PdfMarkupFormat::Md => "md",
            PdfMarkupFormat::Html => "html",
        }
    }

    /// Short name for messages ("Markdown", "HTML").
    pub fn label(self) -> &'static str {
        match self {
            PdfMarkupFormat::Md => "Markdown",
            PdfMarkupFormat::Html => "HTML",
        }
    }
}

/// PDF → Markdown / HTML with the native layout reader (`crate::pdf::to_markdown`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PdfToMarkdownOpts {
    pub format: PdfMarkupFormat,
    /// Save pictures into a `name (images)` folder next to the output and link
    /// them. Off: pictures are left out.
    pub images: bool,
}

impl Default for PdfToMarkdownOpts {
    fn default() -> Self {
        PdfToMarkdownOpts {
            format: PdfMarkupFormat::Md,
            images: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_follow_spec() {
        assert_eq!(PdfCompressOpts::default().target(), (150, 75));
        let c = PdfCompressOpts {
            preset: CompressPreset::High,
            ..Default::default()
        };
        assert_eq!(c.target(), (96, 60));
        let i = PdfToImagesOpts::default();
        assert_eq!(
            (i.format, i.dpi, i.pages.as_str()),
            (PageImageFormat::Png, 150, "")
        );
        assert_eq!(PdfSplitOpts::default().mode, SplitMode::EveryPage);
        let m = PdfMergeOpts::default();
        assert!(m.bookmarks);
        assert_eq!(m.page_size, PageSize::FitToImage);
    }

    #[test]
    fn partial_json_keeps_defaults() {
        let o: PdfToImagesOpts = serde_json::from_str(r#"{"format":"webp"}"#).unwrap();
        assert_eq!(o.format, PageImageFormat::Webp);
        assert_eq!(o.dpi, 150);
        let s: PdfSplitOpts = serde_json::from_str(r#"{"mode":"every_n","every":5}"#).unwrap();
        assert_eq!((s.mode, s.every), (SplitMode::EveryN, 5));
    }
}
