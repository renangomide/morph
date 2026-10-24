// SPDX-License-Identifier: GPL-3.0-or-later
//! Office action options (SPEC §4.5). `Default` = the Settings defaults.
//!
//! - `office.convert` ([`OfficeConvertOpts`]): any document (Word-, Excel-,
//!   PowerPoint- or text-like) to another document format.
//! - `office.to_images` ([`OfficeToImagesOpts`]): Word pages / PowerPoint slides → images.
//!
//! [`Family`] says which formats each input family can become.

use crate::image::options::OutputFormat;
use crate::pdf::options::PageImageFormat;
use morph_formats::Category;
use serde::{Deserialize, Serialize};
use std::path::Path;

// ---------------------------------------------------------------- formats

/// Document formats Morph can write.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DocFormat {
    #[default]
    Pdf,
    Docx,
    Odt,
    Rtf,
    Txt,
    Html,
    /// Markdown.
    Md,
    Xlsx,
    Ods,
    Csv,
    Pptx,
    Odp,
}

impl DocFormat {
    pub const ALL: [DocFormat; 12] = [
        DocFormat::Pdf,
        DocFormat::Docx,
        DocFormat::Odt,
        DocFormat::Rtf,
        DocFormat::Txt,
        DocFormat::Html,
        DocFormat::Md,
        DocFormat::Xlsx,
        DocFormat::Ods,
        DocFormat::Csv,
        DocFormat::Pptx,
        DocFormat::Odp,
    ];

    /// File extension of the output (no dot).
    pub fn extension(self) -> &'static str {
        match self {
            DocFormat::Pdf => "pdf",
            DocFormat::Docx => "docx",
            DocFormat::Odt => "odt",
            DocFormat::Rtf => "rtf",
            DocFormat::Txt => "txt",
            DocFormat::Html => "html",
            DocFormat::Md => "md",
            DocFormat::Xlsx => "xlsx",
            DocFormat::Ods => "ods",
            DocFormat::Csv => "csv",
            DocFormat::Pptx => "pptx",
            DocFormat::Odp => "odp",
        }
    }

    /// Short name for messages ("PDF", "Markdown").
    pub fn label(self) -> &'static str {
        match self {
            DocFormat::Pdf => "PDF",
            DocFormat::Docx => "DOCX",
            DocFormat::Odt => "ODT",
            DocFormat::Rtf => "RTF",
            DocFormat::Txt => "TXT",
            DocFormat::Html => "HTML",
            DocFormat::Md => "Markdown",
            DocFormat::Xlsx => "XLSX",
            DocFormat::Ods => "ODS",
            DocFormat::Csv => "CSV",
            DocFormat::Pptx => "PPTX",
            DocFormat::Odp => "ODP",
        }
    }

    /// The format a file already is, from its extension (`htm` → Html,
    /// `markdown` → Md). `None` for inputs Morph can't write (doc, xls, ppt).
    pub fn of_path(path: &Path) -> Option<DocFormat> {
        let ext = path.extension()?.to_str()?.to_ascii_lowercase();
        match ext.as_str() {
            "htm" => Some(DocFormat::Html),
            "markdown" => Some(DocFormat::Md),
            _ => DocFormat::ALL
                .into_iter()
                .find(|f| f.extension() == ext.as_str()),
        }
    }
}

/// Input families (SPEC §4.5 table).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Family {
    /// doc, docx, odt, rtf.
    Word,
    /// xls, xlsx, ods, csv.
    Excel,
    /// ppt, pptx, odp.
    PowerPoint,
    /// md, markdown, txt, html, htm.
    Text,
}

impl Family {
    pub fn from_category(category: Category) -> Option<Family> {
        match category {
            Category::Word => Some(Family::Word),
            Category::Excel => Some(Family::Excel),
            Category::PowerPoint => Some(Family::PowerPoint),
            Category::Text => Some(Family::Text),
            _ => None,
        }
    }

    pub fn of_path(path: &Path) -> Option<Family> {
        Family::from_category(Category::from_path(path)?)
    }

    /// Formats this family converts to (SPEC §4.5). The input's own format is
    /// listed too; converting a file to its own format is refused separately.
    pub fn outputs(self) -> &'static [DocFormat] {
        use DocFormat as F;
        match self {
            Family::Word => &[F::Pdf, F::Docx, F::Odt, F::Rtf, F::Txt, F::Html, F::Md],
            Family::Excel => &[F::Pdf, F::Xlsx, F::Ods, F::Csv, F::Md],
            Family::PowerPoint => &[F::Pdf, F::Pptx, F::Odp, F::Md],
            Family::Text => &[F::Docx, F::Pdf, F::Odt, F::Html, F::Md, F::Txt],
        }
    }

    pub fn can_export(self, format: DocFormat) -> bool {
        self.outputs().contains(&format)
    }

    /// "document", "spreadsheet", "presentation", "text file" (for messages).
    pub fn noun(self) -> &'static str {
        match self {
            Family::Word => "document",
            Family::Excel => "spreadsheet",
            Family::PowerPoint => "presentation",
            Family::Text => "text file",
        }
    }

    /// The family has page/slide images ("Export as images…").
    pub fn has_images(self) -> bool {
        matches!(self, Family::Word | Family::PowerPoint)
    }
}

// ---------------------------------------------------------------- convert

/// CSV field separator. `Auto`: detect when reading; comma when writing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CsvDelimiter {
    #[default]
    Auto,
    Comma,
    Semicolon,
    Tab,
}

impl CsvDelimiter {
    pub const ALL: [CsvDelimiter; 4] = [
        CsvDelimiter::Auto,
        CsvDelimiter::Comma,
        CsvDelimiter::Semicolon,
        CsvDelimiter::Tab,
    ];

    /// The byte to use, `None` for `Auto` (detect from the file).
    pub fn byte(self) -> Option<u8> {
        match self {
            CsvDelimiter::Auto => None,
            CsvDelimiter::Comma => Some(b','),
            CsvDelimiter::Semicolon => Some(b';'),
            CsvDelimiter::Tab => Some(b'\t'),
        }
    }

    /// The byte to write: `Auto` writes commas.
    pub fn write_byte(self) -> u8 {
        self.byte().unwrap_or(b',')
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct OfficeConvertOpts {
    /// Target format. Must be one of the input family's [`Family::outputs`].
    pub format: DocFormat,
    /// CSV separator: for reading CSV inputs (`Auto` = detect) and writing CSV
    /// outputs (`Auto` = comma).
    pub delimiter: CsvDelimiter,
    /// Start CSV outputs with a UTF-8 BOM, so Excel reads accents correctly.
    pub csv_bom: bool,
    /// Markdown outputs: save pictures into a `name (images)` folder next to the
    /// `.md` and link them. Off: no folder; a picture becomes its alt text (if any).
    pub images: bool,
    /// Spreadsheet → Markdown: at most this many data rows per sheet (0 = all).
    /// When rows are cut, the result says so in a note.
    pub max_rows: u32,
    /// Presentation → Markdown: include speaker notes (`### Notes:` per slide).
    pub slide_notes: bool,
}

impl Default for OfficeConvertOpts {
    fn default() -> Self {
        OfficeConvertOpts {
            format: DocFormat::Pdf,
            delimiter: CsvDelimiter::Auto,
            // Lead decision (M5): BOM on, so Excel shows accents on a double-click.
            csv_bom: true,
            images: true,
            max_rows: 0,
            slide_notes: true,
        }
    }
}

// ---------------------------------------------------------------- to images

/// Image formats for "Export as images…" (SPEC §4.5: PNG/JPG).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DocImageFormat {
    #[default]
    Png,
    Jpg,
}

impl DocImageFormat {
    pub const ALL: [DocImageFormat; 2] = [DocImageFormat::Png, DocImageFormat::Jpg];

    /// Same format for the PDF page renderer (`crate::pdf`).
    pub fn page_format(self) -> PageImageFormat {
        match self {
            DocImageFormat::Png => PageImageFormat::Png,
            DocImageFormat::Jpg => PageImageFormat::Jpg,
        }
    }

    pub fn output_format(self) -> OutputFormat {
        self.page_format().output_format()
    }

    pub fn extension(self) -> &'static str {
        self.page_format().extension()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct OfficeToImagesOpts {
    pub format: DocImageFormat,
    /// Render resolution (Settings default 150, same as PDF → images).
    pub dpi: u32,
}

impl Default for OfficeToImagesOpts {
    fn default() -> Self {
        OfficeToImagesOpts {
            format: DocImageFormat::Png,
            dpi: 150,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_from_paths() {
        assert_eq!(
            DocFormat::of_path(Path::new("a.HTM")),
            Some(DocFormat::Html)
        );
        assert_eq!(
            DocFormat::of_path(Path::new("a.markdown")),
            Some(DocFormat::Md)
        );
        assert_eq!(
            DocFormat::of_path(Path::new("a.xlsx")),
            Some(DocFormat::Xlsx)
        );
        assert_eq!(DocFormat::of_path(Path::new("a.doc")), None);
        for f in DocFormat::ALL {
            let name = format!("x.{}", f.extension());
            assert_eq!(DocFormat::of_path(Path::new(&name)), Some(f));
        }
    }

    #[test]
    fn families_follow_the_spec_table() {
        assert_eq!(Family::of_path(Path::new("a.rtf")), Some(Family::Word));
        assert_eq!(Family::of_path(Path::new("a.csv")), Some(Family::Excel));
        assert_eq!(
            Family::of_path(Path::new("a.odp")),
            Some(Family::PowerPoint)
        );
        assert_eq!(Family::of_path(Path::new("a.md")), Some(Family::Text));
        assert_eq!(Family::of_path(Path::new("a.pdf")), None);
        assert!(Family::Word.can_export(DocFormat::Md));
        assert!(!Family::Excel.can_export(DocFormat::Docx));
        assert!(!Family::PowerPoint.can_export(DocFormat::Txt));
        assert!(Family::Excel.can_export(DocFormat::Md));
        assert!(Family::PowerPoint.can_export(DocFormat::Md));
        let o = OfficeConvertOpts::default();
        assert!(o.images && o.slide_notes && o.max_rows == 0);
        assert!(!Family::Text.can_export(DocFormat::Rtf));
        assert!(Family::PowerPoint.has_images() && !Family::Excel.has_images());
    }

    #[test]
    fn csv_delimiters() {
        assert_eq!(CsvDelimiter::Auto.byte(), None);
        assert_eq!(CsvDelimiter::Auto.write_byte(), b',');
        assert_eq!(CsvDelimiter::Semicolon.write_byte(), b';');
        assert_eq!(DocImageFormat::Jpg.extension(), "jpg");
    }
}
