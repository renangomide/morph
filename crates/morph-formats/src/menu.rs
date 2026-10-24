// SPDX-License-Identifier: GPL-3.0-or-later
//! The right-click submenu (SPEC §3.1, §4.6), built only from the selection summary.
//!
//! SPEC §4.6: each file type shows its **4 most common targets** + "More options…";
//! the window lists every valid target. Document menus pick the first 4 entries of a
//! priority list per family, leaving out a target that every selected file already is.

use crate::actions;
use crate::category::{Category, DocKind, MenuGroup, SelectionSummary};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuItem {
    /// Menu text, e.g. "Convert to PNG…".
    pub label: String,
    /// Action ID written to the request file (see [`crate::actions`]).
    pub action: &'static str,
    /// Pre-filled options for the window, e.g. `[("format", "png")]`.
    pub preset: &'static [(&'static str, &'static str)],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MenuEntry {
    Item(MenuItem),
    Separator,
}

impl MenuEntry {
    fn item(
        label: impl Into<String>,
        action: &'static str,
        preset: &'static [(&'static str, &'static str)],
    ) -> MenuEntry {
        MenuEntry::Item(MenuItem {
            label: label.into(),
            action,
            preset,
        })
    }
}

const MORE_OPTIONS: &str = "More options…";

/// How many targets a document menu shows before "More options…" (SPEC §4.6).
pub const TOP_TARGETS: usize = 4;

/// Build the submenu for a selection. Empty means: hide the "Morph" entry.
pub fn build_menu(summary: &SelectionSummary) -> Vec<MenuEntry> {
    let groups = summary.groups();
    match groups.as_slice() {
        [] => Vec::new(),
        [MenuGroup::Image] => image_menu(),
        [MenuGroup::Pdf] => pdf_menu(summary.count(Category::Pdf)),
        [MenuGroup::Video] => video_menu(),
        [MenuGroup::Audio] => audio_menu(),
        [MenuGroup::Office] => office_menu(summary),
        _ => mixed_menu(summary, &groups),
    }
}

fn image_menu() -> Vec<MenuEntry> {
    use MenuEntry as E;
    vec![
        E::item(
            "Convert to JPG…",
            actions::IMAGE_CONVERT,
            &[("format", "jpg")],
        ),
        E::item(
            "Convert to PNG…",
            actions::IMAGE_CONVERT,
            &[("format", "png")],
        ),
        E::item(
            "Convert to WebP…",
            actions::IMAGE_CONVERT,
            &[("format", "webp")],
        ),
        E::item(
            "Convert to AVIF…",
            actions::IMAGE_CONVERT,
            &[("format", "avif")],
        ),
        // One PDF per image (SPEC §4.6); several into one PDF is Combine/Merge.
        E::item("Convert to PDF…", actions::IMAGE_TO_PDF, &[]),
        E::Separator,
        E::item("Compress…", actions::IMAGE_COMPRESS, &[]),
        E::item("Resize…", actions::IMAGE_RESIZE, &[]),
        E::item(
            "Remove metadata…",
            actions::IMAGE_CLEAN,
            &[("strip", "all")],
        ),
        E::Separator,
        E::item(MORE_OPTIONS, actions::IMAGE_CONVERT, &[]),
    ]
}

/// PDF: Word, Markdown, images, Compress (SPEC §4.6). Extract text, Split and
/// HTML are in the window ("More options…").
fn pdf_menu(pdf_count: usize) -> Vec<MenuEntry> {
    use MenuEntry as E;
    let mut menu = vec![
        E::item("Convert to Word…", actions::PDF_TO_WORD, &[]),
        E::item(
            "Convert to Markdown…",
            actions::PDF_TO_MARKDOWN,
            &[("format", "md")],
        ),
        E::item("Convert to images…", actions::PDF_TO_IMAGES, &[]),
        E::item("Compress…", actions::PDF_COMPRESS, &[]),
    ];
    if pdf_count >= 2 {
        menu.push(E::item("Merge into one PDF…", actions::PDF_MERGE, &[]));
    }
    menu.push(E::Separator);
    menu.push(E::item(MORE_OPTIONS, actions::PDF_COMPRESS, &[]));
    menu
}

fn video_menu() -> Vec<MenuEntry> {
    use MenuEntry as E;
    vec![
        E::item(
            "Convert to MP4…",
            actions::VIDEO_CONVERT,
            &[("format", "mp4")],
        ),
        E::item(
            "Convert to MKV…",
            actions::VIDEO_CONVERT,
            &[("format", "mkv")],
        ),
        E::item(
            "Convert to WebM…",
            actions::VIDEO_CONVERT,
            &[("format", "webm")],
        ),
        E::Separator,
        E::item("Compress…", actions::VIDEO_COMPRESS, &[]),
        E::item("Extract audio…", actions::VIDEO_EXTRACT_AUDIO, &[]),
        E::item("Convert to GIF…", actions::VIDEO_TO_GIF, &[]),
        E::Separator,
        E::item(MORE_OPTIONS, actions::VIDEO_CONVERT, &[]),
    ]
}

fn audio_menu() -> Vec<MenuEntry> {
    use MenuEntry as E;
    vec![
        E::item(
            "Convert to MP3…",
            actions::AUDIO_CONVERT,
            &[("format", "mp3")],
        ),
        E::item(
            "Convert to M4A…",
            actions::AUDIO_CONVERT,
            &[("format", "m4a")],
        ),
        E::item(
            "Convert to WAV…",
            actions::AUDIO_CONVERT,
            &[("format", "wav")],
        ),
        E::item(
            "Convert to FLAC…",
            actions::AUDIO_CONVERT,
            &[("format", "flac")],
        ),
        E::Separator,
        E::item(MORE_OPTIONS, actions::AUDIO_CONVERT, &[]),
    ]
}

/// A document menu candidate.
struct Target {
    label: &'static str,
    action: &'static str,
    preset: &'static [(&'static str, &'static str)],
    /// Hidden when every selected file already is this kind (never convert a
    /// file to its own format).
    own: Option<DocKind>,
}

const fn to(
    label: &'static str,
    preset: &'static [(&'static str, &'static str)],
    own: Option<DocKind>,
) -> Target {
    Target {
        label,
        action: actions::OFFICE_CONVERT,
        preset,
        own,
    }
}

const TO_PDF: Target = to("Convert to PDF…", &[("format", "pdf")], None);
const TO_MARKDOWN: Target = to(
    "Convert to Markdown…",
    &[("format", "md")],
    Some(DocKind::Markdown),
);
const TO_TXT: Target = to(
    "Convert to TXT…",
    &[("format", "txt")],
    Some(DocKind::PlainText),
);

/// Priority order per family; the first [`TOP_TARGETS`] that apply are shown.
/// DOCX → PDF, Markdown, ODT, TXT; DOC/ODT/RTF → PDF, DOCX, Markdown, …
const WORD_TARGETS: &[Target] = &[
    TO_PDF,
    to(
        "Convert to DOCX…",
        &[("format", "docx")],
        Some(DocKind::Modern),
    ),
    TO_MARKDOWN,
    to(
        "Convert to ODT…",
        &[("format", "odt")],
        Some(DocKind::OpenDocument),
    ),
    TO_TXT,
];

/// XLSX → PDF, CSV, Markdown, ODS; XLS/ODS → PDF, XLSX, CSV, Markdown;
/// CSV → PDF, XLSX, Markdown, ODS.
const EXCEL_TARGETS: &[Target] = &[
    TO_PDF,
    to(
        "Convert to XLSX…",
        &[("format", "xlsx")],
        Some(DocKind::Modern),
    ),
    to("Convert to CSV…", &[("format", "csv")], Some(DocKind::Csv)),
    TO_MARKDOWN,
    to(
        "Convert to ODS…",
        &[("format", "ods")],
        Some(DocKind::OpenDocument),
    ),
];

/// PPTX → PDF, images, Markdown, ODP; PPT/ODP → PDF, PPTX, images, Markdown.
const POWERPOINT_TARGETS: &[Target] = &[
    TO_PDF,
    to(
        "Convert to PPTX…",
        &[("format", "pptx")],
        Some(DocKind::Modern),
    ),
    Target {
        label: "Convert to images…",
        action: actions::OFFICE_TO_IMAGES,
        preset: &[],
        own: None,
    },
    TO_MARKDOWN,
    to(
        "Convert to ODP…",
        &[("format", "odp")],
        Some(DocKind::OpenDocument),
    ),
];

/// MD → PDF, DOCX, HTML, TXT; HTML → PDF, DOCX, Markdown, TXT;
/// TXT → PDF, DOCX, HTML, Markdown.
const TEXT_TARGETS: &[Target] = &[
    TO_PDF,
    to("Convert to DOCX…", &[("format", "docx")], None),
    to(
        "Convert to HTML…",
        &[("format", "html")],
        Some(DocKind::Html),
    ),
    TO_MARKDOWN,
    TO_TXT,
];

fn office_menu(summary: &SelectionSummary) -> Vec<MenuEntry> {
    use MenuEntry as E;
    // Family-specific targets only when the selection is a single family.
    let targets: &[Target] = match summary.categories().as_slice() {
        [Category::Word] => WORD_TARGETS,
        [Category::Excel] => EXCEL_TARGETS,
        [Category::PowerPoint] => POWERPOINT_TARGETS,
        [Category::Text] => TEXT_TARGETS,
        _ => std::slice::from_ref(&TO_PDF),
    };
    let docs = summary.document_count();
    let mut menu: Vec<MenuEntry> = targets
        .iter()
        .filter(|t| t.own.is_none_or(|kind| summary.doc_kind_count(kind) < docs))
        .take(TOP_TARGETS)
        .map(|t| E::item(t.label, t.action, t.preset))
        .collect();
    menu.push(E::Separator);
    menu.push(E::item(MORE_OPTIONS, actions::OFFICE_CONVERT, &[]));
    menu
}

fn mixed_menu(summary: &SelectionSummary, groups: &[MenuGroup]) -> Vec<MenuEntry> {
    let label = if summary.partial {
        "Convert files…".to_owned()
    } else {
        format!("Convert {} files…", summary.label_count())
    };
    let mut menu = vec![MenuEntry::item(label, actions::MIXED_CONVERT, &[])];
    let only_images_and_pdfs = groups
        .iter()
        .all(|g| matches!(g, MenuGroup::Image | MenuGroup::Pdf));
    if only_images_and_pdfs {
        menu.push(MenuEntry::item(
            "Combine into one PDF…",
            actions::PDF_MERGE,
            &[],
        ));
    }
    menu
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::EnabledCategories;
    use std::path::Path;

    fn menu_for(files: &[&str]) -> Vec<MenuEntry> {
        let enabled = EnabledCategories::default();
        let summary = SelectionSummary::from_paths(files.iter().map(Path::new), &enabled);
        build_menu(&summary)
    }

    fn labels(menu: &[MenuEntry]) -> Vec<String> {
        menu.iter()
            .map(|e| match e {
                MenuEntry::Item(i) => i.label.clone(),
                MenuEntry::Separator => "---".to_owned(),
            })
            .collect()
    }

    fn has(labels: &[String], label: &str) -> bool {
        labels.iter().any(|l| l == label)
    }

    fn item<'a>(menu: &'a [MenuEntry], label: &str) -> &'a MenuItem {
        menu.iter()
            .find_map(|e| match e {
                MenuEntry::Item(i) if i.label == label => Some(i),
                _ => None,
            })
            .unwrap_or_else(|| panic!("no item {label}"))
    }

    /// The labels before the separator, then "More options…".
    fn office(files: &[&str]) -> Vec<String> {
        let l = labels(&menu_for(files));
        assert_eq!(l[l.len() - 2..], ["---", "More options…"], "{l:?}");
        l[..l.len() - 2].to_vec()
    }

    #[test]
    fn unsupported_selection_hides_menu() {
        assert!(menu_for(&["setup.exe", "folder"]).is_empty());
        assert!(menu_for(&[]).is_empty());
    }

    #[test]
    fn image_menu_has_convert_to_pdf() {
        let menu = menu_for(&["a.jpg", "b.jpg", "c.JPG"]);
        assert_eq!(
            labels(&menu),
            [
                "Convert to JPG…",
                "Convert to PNG…",
                "Convert to WebP…",
                "Convert to AVIF…",
                "Convert to PDF…",
                "---",
                "Compress…",
                "Resize…",
                "Remove metadata…",
                "---",
                "More options…"
            ]
        );
        let MenuEntry::Item(png) = &menu[1] else {
            panic!("expected item")
        };
        assert_eq!(png.action, actions::IMAGE_CONVERT);
        assert_eq!(png.preset, &[("format", "png")]);
        let pdf = item(&menu, "Convert to PDF…");
        assert_eq!((pdf.action, pdf.preset), (actions::IMAGE_TO_PDF, &[][..]));
    }

    #[test]
    fn video_and_audio_menus() {
        let menu = menu_for(&["a.mp4", "b.MKV"]);
        assert_eq!(
            labels(&menu),
            [
                "Convert to MP4…",
                "Convert to MKV…",
                "Convert to WebM…",
                "---",
                "Compress…",
                "Extract audio…",
                "Convert to GIF…",
                "---",
                "More options…"
            ]
        );
        let webm = item(&menu, "Convert to WebM…");
        assert_eq!(
            (webm.action, webm.preset),
            (actions::VIDEO_CONVERT, &[("format", "webm")][..])
        );
        assert_eq!(item(&menu, "Compress…").action, actions::VIDEO_COMPRESS);
        assert_eq!(
            item(&menu, "Extract audio…").action,
            actions::VIDEO_EXTRACT_AUDIO
        );
        assert_eq!(item(&menu, "Convert to GIF…").action, actions::VIDEO_TO_GIF);

        let menu = menu_for(&["a.flac", "b.mp3"]);
        assert_eq!(
            labels(&menu),
            [
                "Convert to MP3…",
                "Convert to M4A…",
                "Convert to WAV…",
                "Convert to FLAC…",
                "---",
                "More options…"
            ]
        );
        let m4a = item(&menu, "Convert to M4A…");
        assert_eq!(
            (m4a.action, m4a.preset),
            (actions::AUDIO_CONVERT, &[("format", "m4a")][..])
        );
        // Video + audio is a mixed selection.
        assert_eq!(labels(&menu_for(&["a.mp4", "b.mp3"])), ["Convert 2 files…"]);
        // Only part of the selection seen: no count in the label.
        let mut partial = SelectionSummary::from_paths(
            [Path::new("a.mp4"), Path::new("b.mp3")],
            &EnabledCategories::default(),
        );
        partial.partial = true;
        assert_eq!(labels(&build_menu(&partial)), ["Convert files…"]);
    }

    #[test]
    fn pdf_menu_is_the_top_four() {
        let menu = menu_for(&["a.pdf"]);
        assert_eq!(
            labels(&menu),
            [
                "Convert to Word…",
                "Convert to Markdown…",
                "Convert to images…",
                "Compress…",
                "---",
                "More options…"
            ]
        );
        let md = item(&menu, "Convert to Markdown…");
        assert_eq!(md.action, actions::PDF_TO_MARKDOWN);
        assert_eq!(md.preset, &[("format", "md")]);
        assert_eq!(item(&menu, "Convert to Word…").action, actions::PDF_TO_WORD);
    }

    #[test]
    fn merge_only_with_two_pdfs() {
        assert!(!has(&labels(&menu_for(&["a.pdf"])), "Merge into one PDF…"));
        assert!(has(
            &labels(&menu_for(&["a.pdf", "b.pdf"])),
            "Merge into one PDF…"
        ));
    }

    #[test]
    fn word_top_four() {
        let docx = [
            "Convert to PDF…",
            "Convert to Markdown…",
            "Convert to ODT…",
            "Convert to TXT…",
        ];
        assert_eq!(office(&["a.docx", "b.DOCX"]), docx);
        let older = [
            "Convert to PDF…",
            "Convert to DOCX…",
            "Convert to Markdown…",
            "Convert to ODT…",
        ];
        assert_eq!(office(&["a.doc"]), older);
        assert_eq!(office(&["a.rtf"]), older);
        assert_eq!(office(&["a.docx", "b.odt"]), older, "mixed kinds");
        assert_eq!(
            office(&["a.odt"]),
            [
                "Convert to PDF…",
                "Convert to DOCX…",
                "Convert to Markdown…",
                "Convert to TXT…"
            ]
        );
    }

    #[test]
    fn excel_top_four_has_csv() {
        assert_eq!(
            office(&["a.xlsx"]),
            [
                "Convert to PDF…",
                "Convert to CSV…",
                "Convert to Markdown…",
                "Convert to ODS…"
            ]
        );
        let older = [
            "Convert to PDF…",
            "Convert to XLSX…",
            "Convert to CSV…",
            "Convert to Markdown…",
        ];
        assert_eq!(office(&["a.xls"]), older);
        assert_eq!(office(&["a.ods"]), older);
        assert_eq!(
            office(&["a.csv"]),
            [
                "Convert to PDF…",
                "Convert to XLSX…",
                "Convert to Markdown…",
                "Convert to ODS…"
            ]
        );
        let menu = menu_for(&["a.xlsx"]);
        let csv = item(&menu, "Convert to CSV…");
        assert_eq!(
            (csv.action, csv.preset),
            (actions::OFFICE_CONVERT, &[("format", "csv")][..])
        );
    }

    #[test]
    fn powerpoint_top_four() {
        assert_eq!(
            office(&["a.pptx"]),
            [
                "Convert to PDF…",
                "Convert to images…",
                "Convert to Markdown…",
                "Convert to ODP…"
            ]
        );
        assert_eq!(
            office(&["a.ppt"]),
            [
                "Convert to PDF…",
                "Convert to PPTX…",
                "Convert to images…",
                "Convert to Markdown…"
            ]
        );
        let menu = menu_for(&["a.pptx"]);
        assert_eq!(
            item(&menu, "Convert to images…").action,
            actions::OFFICE_TO_IMAGES
        );
        let pdf = item(&menu, "Convert to PDF…");
        assert_eq!(pdf.action, actions::OFFICE_CONVERT);
        assert_eq!(pdf.preset, &[("format", "pdf")]);
    }

    #[test]
    fn text_like_top_four() {
        assert_eq!(
            office(&["notes.md", "b.markdown"]),
            [
                "Convert to PDF…",
                "Convert to DOCX…",
                "Convert to HTML…",
                "Convert to TXT…"
            ]
        );
        assert_eq!(
            office(&["page.html", "b.HTM"]),
            [
                "Convert to PDF…",
                "Convert to DOCX…",
                "Convert to Markdown…",
                "Convert to TXT…"
            ]
        );
        assert_eq!(
            office(&["a.txt"]),
            [
                "Convert to PDF…",
                "Convert to DOCX…",
                "Convert to HTML…",
                "Convert to Markdown…"
            ]
        );
        // Mixed kinds: a file already in the target format is skipped in the window.
        let both = office(&["page.html", "notes.md"]);
        assert!(has(&both, "Convert to HTML…") && has(&both, "Convert to Markdown…"));
        let md = menu_for(&["a.markdown"]);
        assert_eq!(item(&md, "Convert to HTML…").preset, &[("format", "html")]);
    }

    #[test]
    fn mixed_families_only_offer_pdf() {
        assert_eq!(office(&["a.doc", "b.xlsx"]), ["Convert to PDF…"]);
        assert_eq!(office(&["a.docx", "b.md"]), ["Convert to PDF…"]);
    }

    #[test]
    fn every_document_menu_has_at_most_four_targets() {
        for files in [
            &["a.docx"][..],
            &["a.doc"],
            &["a.xlsx"],
            &["a.csv"],
            &["a.pptx"],
            &["a.odp"],
            &["a.md"],
            &["a.html"],
            &["a.txt"],
        ] {
            assert!(office(files).len() <= TOP_TARGETS, "{files:?}");
        }
    }

    #[test]
    fn text_files_follow_the_office_toggle() {
        let enabled = EnabledCategories {
            office: false,
            ..EnabledCategories::default()
        };
        let summary = SelectionSummary::from_paths([Path::new("a.md")], &enabled);
        assert!(build_menu(&summary).is_empty());
    }

    #[test]
    fn mixed_selection() {
        let m = labels(&menu_for(&["a.jpg", "b.png", "c.pdf"]));
        assert_eq!(m, ["Convert 3 files…", "Combine into one PDF…"]);
        let m = labels(&menu_for(&["a.jpg", "b.mp4", "c.pdf", "skip.exe"]));
        assert_eq!(m, ["Convert 3 files…"]);
    }
}
