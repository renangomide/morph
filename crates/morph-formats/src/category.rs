// SPDX-License-Identifier: GPL-3.0-or-later
//! Extension → category table. Decided only from the file name; files are never opened.

use crate::config::EnabledCategories;
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Category {
    Image,
    Pdf,
    Video,
    Audio,
    Word,
    Excel,
    PowerPoint,
    /// Text-like documents: Markdown, plain text, HTML (SPEC §4.5).
    Text,
}

/// Menu-level grouping: Word, Excel, PowerPoint and text-like files share the "Office" menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MenuGroup {
    Image,
    Pdf,
    Video,
    Audio,
    Office,
}

const IMAGE_EXTS: &[&str] = &[
    "jpg", "jpeg", "jfif", "png", "webp", "avif", "heic", "heif", "bmp", "tif", "tiff", "gif",
    "ico", "tga", "qoi", "svg",
];
const PDF_EXTS: &[&str] = &["pdf"];
const VIDEO_EXTS: &[&str] = &[
    "mp4", "mkv", "mov", "avi", "webm", "wmv", "flv", "m4v", "mpg", "mpeg", "ts", "mts", "3gp",
];
const AUDIO_EXTS: &[&str] = &[
    "mp3", "wav", "flac", "aac", "m4a", "ogg", "opus", "wma", "aiff", "aif",
];
const WORD_EXTS: &[&str] = &["doc", "docx", "odt", "rtf"];
const EXCEL_EXTS: &[&str] = &["xls", "xlsx", "ods", "csv"];
const POWERPOINT_EXTS: &[&str] = &["ppt", "pptx", "odp"];
const TEXT_EXTS: &[&str] = &["md", "markdown", "txt", "html", "htm"];

/// Lower-case `ext` into `buf` without allocating. `None` if empty or too long.
fn lower_ext<'a>(ext: &str, buf: &'a mut [u8; 8]) -> Option<&'a str> {
    let bytes = ext.as_bytes();
    if bytes.is_empty() || bytes.len() > buf.len() {
        return None;
    }
    for (dst, src) in buf.iter_mut().zip(bytes) {
        *dst = src.to_ascii_lowercase();
    }
    std::str::from_utf8(&buf[..bytes.len()]).ok()
}

/// Finer kind of a document inside its category, for menu items that depend on
/// the exact format ("never the same format as the input"). Extension only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DocKind {
    /// `.docx`, `.xlsx` or `.pptx`: already the family's modern format.
    Modern,
    /// `.html` / `.htm`.
    Html,
    /// `.md` / `.markdown`.
    Markdown,
    /// `.txt`.
    PlainText,
    /// `.odt`, `.ods` or `.odp`: the family's OpenDocument format.
    OpenDocument,
    /// `.csv`.
    Csv,
    /// Anything else.
    Other,
}

impl DocKind {
    pub fn from_path(path: &Path) -> DocKind {
        let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
            return DocKind::Other;
        };
        let mut buf = [0u8; 8];
        match lower_ext(ext, &mut buf) {
            Some("docx" | "xlsx" | "pptx") => DocKind::Modern,
            Some("html" | "htm") => DocKind::Html,
            Some("md" | "markdown") => DocKind::Markdown,
            Some("txt") => DocKind::PlainText,
            Some("odt" | "ods" | "odp") => DocKind::OpenDocument,
            Some("csv") => DocKind::Csv,
            _ => DocKind::Other,
        }
    }

    fn slot(self) -> Option<usize> {
        match self {
            DocKind::Modern => Some(0),
            DocKind::Html => Some(1),
            DocKind::Markdown => Some(2),
            DocKind::PlainText => Some(3),
            DocKind::OpenDocument => Some(4),
            DocKind::Csv => Some(5),
            DocKind::Other => None,
        }
    }
}

impl Category {
    pub const ALL: [Category; 8] = [
        Category::Image,
        Category::Pdf,
        Category::Video,
        Category::Audio,
        Category::Word,
        Category::Excel,
        Category::PowerPoint,
        Category::Text,
    ];

    /// Category for a bare extension (no dot, any case).
    pub fn from_extension(ext: &str) -> Option<Category> {
        // Extensions are short; avoid allocating for the common case.
        let mut buf = [0u8; 8];
        let lower = lower_ext(ext, &mut buf)?;
        let tables: [(&[&str], Category); 8] = [
            (IMAGE_EXTS, Category::Image),
            (PDF_EXTS, Category::Pdf),
            (VIDEO_EXTS, Category::Video),
            (AUDIO_EXTS, Category::Audio),
            (WORD_EXTS, Category::Word),
            (EXCEL_EXTS, Category::Excel),
            (POWERPOINT_EXTS, Category::PowerPoint),
            (TEXT_EXTS, Category::Text),
        ];
        tables
            .iter()
            .find(|(exts, _)| exts.contains(&lower))
            .map(|(_, cat)| *cat)
    }

    pub fn from_path(path: &Path) -> Option<Category> {
        Category::from_extension(path.extension()?.to_str()?)
    }

    pub fn menu_group(self) -> MenuGroup {
        match self {
            Category::Image => MenuGroup::Image,
            Category::Pdf => MenuGroup::Pdf,
            Category::Video => MenuGroup::Video,
            Category::Audio => MenuGroup::Audio,
            Category::Word | Category::Excel | Category::PowerPoint | Category::Text => {
                MenuGroup::Office
            }
        }
    }

    /// Word-, Excel-, PowerPoint- or text-like (handled by the Office module).
    pub fn is_document(self) -> bool {
        self.menu_group() == MenuGroup::Office
    }

    fn index(self) -> usize {
        self as usize
    }
}

/// Counts of supported files per category in a selection.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SelectionSummary {
    counts: [usize; 8],
    /// Supported documents per [`DocKind`] slot (`Modern`, `Html`, `Markdown`,
    /// `PlainText`, `OpenDocument`, `Csv`).
    doc_kinds: [usize; 6],
    /// Files that are unsupported or whose category is turned off.
    pub skipped: usize,
    /// Files not looked at (huge selections are only sampled for the menu).
    /// Counted in [`SelectionSummary::label_count`] only.
    pub unscanned: usize,
    /// The selection may be only part of what the user selected (Explorer
    /// hands the menu a sample of big selections), so labels show no count.
    pub partial: bool,
}

impl SelectionSummary {
    pub fn from_paths<'a, I>(paths: I, enabled: &EnabledCategories) -> SelectionSummary
    where
        I: IntoIterator<Item = &'a Path>,
    {
        let mut summary = SelectionSummary::default();
        for path in paths {
            summary.add_path(path, enabled);
        }
        summary
    }

    /// Count one file: its category and, for documents, its [`DocKind`].
    pub fn add_path(&mut self, path: &Path, enabled: &EnabledCategories) {
        let category = Category::from_path(path);
        self.add(category, enabled);
        if let Some(cat) = category
            && cat.is_document()
            && enabled.is_enabled(cat)
            && let Some(slot) = DocKind::from_path(path).slot()
        {
            self.doc_kinds[slot] += 1;
        }
    }

    /// Supported documents of this kind (counted by [`SelectionSummary::add_path`]).
    pub fn doc_kind_count(&self, kind: DocKind) -> usize {
        match kind.slot() {
            Some(slot) => self.doc_kinds[slot],
            None => self
                .document_count()
                .saturating_sub(self.doc_kinds.iter().sum::<usize>()),
        }
    }

    /// Supported documents (Word, Excel, PowerPoint and text-like).
    pub fn document_count(&self) -> usize {
        self.group_count(MenuGroup::Office)
    }

    pub fn add(&mut self, category: Option<Category>, enabled: &EnabledCategories) {
        match category {
            Some(cat) if enabled.is_enabled(cat) => self.counts[cat.index()] += 1,
            _ => self.skipped += 1,
        }
    }

    pub fn count(&self, category: Category) -> usize {
        self.counts[category.index()]
    }

    pub fn supported(&self) -> usize {
        self.counts.iter().sum()
    }

    /// File count shown in labels like "Convert N files…".
    pub fn label_count(&self) -> usize {
        self.supported() + self.unscanned
    }

    pub fn is_empty(&self) -> bool {
        self.supported() == 0
    }

    pub fn group_count(&self, group: MenuGroup) -> usize {
        Category::ALL
            .iter()
            .filter(|c| c.menu_group() == group)
            .map(|c| self.count(*c))
            .sum()
    }

    /// Distinct menu groups present, in a fixed order.
    pub fn groups(&self) -> Vec<MenuGroup> {
        [
            MenuGroup::Image,
            MenuGroup::Pdf,
            MenuGroup::Video,
            MenuGroup::Audio,
            MenuGroup::Office,
        ]
        .into_iter()
        .filter(|g| self.group_count(*g) > 0)
        .collect()
    }

    /// Distinct categories present.
    pub fn categories(&self) -> Vec<Category> {
        Category::ALL
            .into_iter()
            .filter(|c| self.count(*c) > 0)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extensions_map_case_insensitively() {
        assert_eq!(Category::from_extension("JPG"), Some(Category::Image));
        assert_eq!(Category::from_extension("Pdf"), Some(Category::Pdf));
        assert_eq!(Category::from_extension("mkv"), Some(Category::Video));
        assert_eq!(Category::from_extension("FLAC"), Some(Category::Audio));
        assert_eq!(Category::from_extension("docx"), Some(Category::Word));
        assert_eq!(Category::from_extension("csv"), Some(Category::Excel));
        assert_eq!(Category::from_extension("odp"), Some(Category::PowerPoint));
        assert_eq!(Category::from_extension("MD"), Some(Category::Text));
        assert_eq!(Category::from_extension("markdown"), Some(Category::Text));
        assert_eq!(Category::from_extension("htm"), Some(Category::Text));
        assert_eq!(Category::from_extension("txt"), Some(Category::Text));
        assert_eq!(Category::from_extension("exe"), None);
        assert_eq!(Category::from_extension(""), None);
        assert_eq!(Category::from_extension("waytoolongext"), None);
    }

    #[test]
    fn paths_without_extension_are_unsupported() {
        assert_eq!(Category::from_path(Path::new(r"C:\folder")), None);
        assert_eq!(
            Category::from_path(Path::new(r"C:\Fotos\Férias 夏.HEIC")),
            Some(Category::Image)
        );
    }

    #[test]
    fn summary_counts_and_skips() {
        let enabled = EnabledCategories::default();
        let paths = ["a.jpg", "b.png", "c.pdf", "d.exe", "folder"].map(Path::new);
        let s = SelectionSummary::from_paths(paths, &enabled);
        assert_eq!(s.count(Category::Image), 2);
        assert_eq!(s.count(Category::Pdf), 1);
        assert_eq!(s.skipped, 2);
        assert_eq!(s.groups(), vec![MenuGroup::Image, MenuGroup::Pdf]);
    }

    #[test]
    fn doc_kinds_are_counted() {
        let enabled = EnabledCategories::default();
        let paths = [
            "a.docx", "b.DOC", "c.html", "d.md", "e.txt", "f.jpg", "g.ODS", "h.csv",
        ]
        .map(Path::new);
        let s = SelectionSummary::from_paths(paths, &enabled);
        assert_eq!(s.doc_kind_count(DocKind::Modern), 1);
        assert_eq!(s.doc_kind_count(DocKind::Html), 1);
        assert_eq!(s.doc_kind_count(DocKind::Markdown), 1);
        assert_eq!(s.doc_kind_count(DocKind::PlainText), 1);
        assert_eq!(s.doc_kind_count(DocKind::OpenDocument), 1);
        assert_eq!(s.doc_kind_count(DocKind::Csv), 1);
        assert_eq!(s.doc_kind_count(DocKind::Other), 1, "b.DOC");
        assert_eq!(s.count(Category::Text), 3);
        assert_eq!(s.groups(), vec![MenuGroup::Image, MenuGroup::Office]);
    }

    #[test]
    fn disabled_categories_are_skipped() {
        let enabled = EnabledCategories {
            images: false,
            ..EnabledCategories::default()
        };
        let s = SelectionSummary::from_paths([Path::new("a.jpg")], &enabled);
        assert!(s.is_empty());
        assert_eq!(s.skipped, 1);
    }
}
