// SPDX-License-Identifier: GPL-3.0-or-later
//! PDF option panels (SPEC §3.2, §4.2) as plain Rust, apart from Slint.
//!
//! Same shape as [`crate::options`]: [`PdfForm`] keeps one options struct per
//! PDF action, [`PdfValues`] is the flat widget view the window binds to, and
//! [`PdfValues::apply`] turns it back into real options.

use crate::strings;
use morph_core::Action;
use morph_core::pdf::options::{
    CompressPreset, DPI_PRESETS, PageImageFormat, PageSize, PdfCompressOpts, PdfMarkupFormat,
    PdfMergeOpts, PdfSplitOpts, PdfToImagesOpts, PdfToMarkdownOpts, PdfToTextOpts, PdfToWordOpts,
    SplitMode,
};
use morph_core::pdf::pages;
use morph_core::state::State;
use morph_formats::actions;
use serde_json::{Map, Value};

/// The PDF actions of the action selector, in display order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PdfKind {
    #[default]
    Compress,
    ToImages,
    ToWord,
    /// Markdown or HTML (`pdf.to_markdown`).
    ToMarkdown,
    ToText,
    Split,
    Merge,
}

impl PdfKind {
    pub const ALL: [PdfKind; 7] = [
        PdfKind::Compress,
        PdfKind::ToImages,
        PdfKind::ToWord,
        PdfKind::ToMarkdown,
        PdfKind::ToText,
        PdfKind::Split,
        PdfKind::Merge,
    ];

    pub fn id(self) -> &'static str {
        match self {
            PdfKind::Compress => actions::PDF_COMPRESS,
            PdfKind::ToImages => actions::PDF_TO_IMAGES,
            PdfKind::ToWord => actions::PDF_TO_WORD,
            PdfKind::ToMarkdown => actions::PDF_TO_MARKDOWN,
            PdfKind::ToText => actions::PDF_TO_TEXT,
            PdfKind::Split => actions::PDF_SPLIT,
            PdfKind::Merge => actions::PDF_MERGE,
        }
    }

    pub fn from_id(id: &str) -> Option<PdfKind> {
        PdfKind::ALL.into_iter().find(|k| k.id() == id)
    }

    pub fn index(self) -> i32 {
        match self {
            PdfKind::Compress => 0,
            PdfKind::ToImages => 1,
            PdfKind::ToWord => 2,
            PdfKind::ToMarkdown => 3,
            PdfKind::ToText => 4,
            PdfKind::Split => 5,
            PdfKind::Merge => 6,
        }
    }

    /// Out-of-range indexes mean Compress.
    pub fn from_index(index: i32) -> PdfKind {
        usize::try_from(index)
            .ok()
            .and_then(|i| PdfKind::ALL.get(i).copied())
            .unwrap_or(PdfKind::Compress)
    }
}

/// Options of every PDF action, plus the selected one.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PdfForm {
    pub kind: PdfKind,
    pub compress: PdfCompressOpts,
    pub to_images: PdfToImagesOpts,
    pub to_word: PdfToWordOpts,
    pub to_markdown: PdfToMarkdownOpts,
    pub to_text: PdfToTextOpts,
    pub split: PdfSplitOpts,
    pub merge: PdfMergeOpts,
}

impl PdfForm {
    /// Pre-fill the window: `kind` is selected; every action gets defaults →
    /// remembered options (`state`) → and, for `kind` only, the menu preset on top.
    pub fn prefill(state: &State, kind: PdfKind, preset: &Map<String, Value>) -> PdfForm {
        let mut form = PdfForm {
            kind,
            ..PdfForm::default()
        };
        let empty = Map::new();
        for k in PdfKind::ALL {
            let preset = if k == kind { preset } else { &empty };
            if let Some(action) = state.action_for(k.id(), preset) {
                form.set_action(action);
            }
        }
        form
    }

    /// The selected action with its options.
    pub fn action(&self) -> Action {
        match self.kind {
            PdfKind::Compress => Action::PdfCompress(self.compress.clone()),
            PdfKind::ToImages => Action::PdfToImages(self.to_images.clone()),
            PdfKind::ToWord => Action::PdfToWord(self.to_word.clone()),
            PdfKind::ToMarkdown => Action::PdfToMarkdown(self.to_markdown.clone()),
            PdfKind::ToText => Action::PdfToText(self.to_text.clone()),
            PdfKind::Split => Action::PdfSplit(self.split.clone()),
            PdfKind::Merge => Action::PdfMerge(self.merge.clone()),
        }
    }

    /// Store `action`'s options (does not change the selected kind).
    pub fn set_action(&mut self, action: Action) {
        match action {
            Action::PdfCompress(o) => self.compress = o,
            Action::PdfToImages(o) => self.to_images = o,
            Action::PdfToWord(o) => self.to_word = o,
            Action::PdfToMarkdown(o) => self.to_markdown = o,
            Action::PdfToText(o) => self.to_text = o,
            Action::PdfSplit(o) => self.split = o,
            Action::PdfMerge(o) => self.merge = o,
            // Not a PDF action: nothing to store in the PDF form.
            _ => {}
        }
    }
}

// Widget indexes. Kept here so the Slint lists and this mapping can't drift.
pub const PRESETS: [CompressPreset; 4] = [
    CompressPreset::Low,
    CompressPreset::Medium,
    CompressPreset::High,
    CompressPreset::Custom,
];
pub const SPLIT_MODES: [SplitMode; 3] =
    [SplitMode::EveryPage, SplitMode::EveryN, SplitMode::Ranges];
pub const PAGE_SIZES: [PageSize; 3] = [PageSize::FitToImage, PageSize::A4, PageSize::Letter];
/// DPI combo box index for "Custom" (after the `DPI_PRESETS`).
pub const DPI_CUSTOM: i32 = DPI_PRESETS.len() as i32;
/// Limits of the number boxes.
pub const MIN_DPI: u32 = 36;
pub const MAX_DPI: u32 = 1200;
pub const MAX_EVERY: u32 = 10_000;
pub const MAX_MARGIN_MM: u32 = 100;
/// Page ranges are checked while typing, before the PDF is opened: with this
/// many pages, only the syntax can be wrong.
const ANY_PAGE_COUNT: u32 = 1_000_000;

/// Flat values of the PDF option widgets.
#[derive(Debug, Clone, PartialEq)]
pub struct PdfValues {
    pub kind: i32,

    // Compress
    /// Index in [`PRESETS`].
    pub preset: i32,
    pub compress_dpi: i32,
    pub compress_quality: i32,

    // To images
    /// Index in `PageImageFormat::ALL`.
    pub image_format: i32,
    /// Index in `DPI_PRESETS`, or [`DPI_CUSTOM`].
    pub dpi_choice: i32,
    pub dpi_custom: i32,
    pub pages: String,

    // To Markdown / HTML
    /// Index in `PdfMarkupFormat::ALL`.
    pub markup_format: i32,
    pub markup_images: bool,

    // Extract text
    pub page_separators: bool,

    // Split
    /// Index in [`SPLIT_MODES`].
    pub split_mode: i32,
    pub split_every: i32,
    pub split_ranges: String,

    // Merge
    /// Index in [`PAGE_SIZES`].
    pub page_size: i32,
    pub margin_mm: i32,
    pub bookmarks: bool,
}

fn index_of<T: PartialEq>(list: &[T], value: &T) -> i32 {
    list.iter()
        .position(|v| v == value)
        .and_then(|i| i32::try_from(i).ok())
        .unwrap_or(0)
}

fn pick<T: Copy>(list: &[T], index: i32) -> Option<T> {
    usize::try_from(index)
        .ok()
        .and_then(|i| list.get(i).copied())
}

fn to_i32(v: u32) -> i32 {
    i32::try_from(v).unwrap_or(i32::MAX)
}

fn in_range(v: i32, min: u32, max: u32, err: &'static str) -> Result<u32, &'static str> {
    match u32::try_from(v) {
        Ok(v) if (min..=max).contains(&v) => Ok(v),
        _ => Err(err),
    }
}

/// Live check of the "Pages" box of To images: `None` = fine (empty = all pages).
pub fn pages_error(text: &str) -> Option<String> {
    pages::parse_ranges(text, ANY_PAGE_COUNT)
        .err()
        .map(|_| strings::bad_page_range(text.trim()))
}

/// Live check of the "Ranges" box of Split: `None` = fine. Empty is an error
/// here (it would mean one file with every page, i.e. a copy).
pub fn split_ranges_error(text: &str) -> Option<String> {
    if text.trim().is_empty() {
        return Some(strings::SPLIT_RANGES_EMPTY.to_owned());
    }
    pages_error(text)
}

impl PdfValues {
    pub fn from_form(form: &PdfForm) -> PdfValues {
        let c = &form.compress;
        let i = &form.to_images;
        let s = &form.split;
        let m = &form.merge;
        let (dpi_choice, dpi_custom) = match DPI_PRESETS.iter().position(|d| *d == i.dpi) {
            Some(p) => (to_i32(p as u32), to_i32(i.dpi)),
            None => (DPI_CUSTOM, to_i32(i.dpi)),
        };
        PdfValues {
            kind: form.kind.index(),
            preset: index_of(&PRESETS, &c.preset),
            compress_dpi: to_i32(c.dpi),
            compress_quality: i32::from(c.quality),
            image_format: index_of(&PageImageFormat::ALL, &i.format),
            dpi_choice,
            dpi_custom,
            pages: i.pages.clone(),
            markup_format: index_of(&PdfMarkupFormat::ALL, &form.to_markdown.format),
            markup_images: form.to_markdown.images,
            page_separators: form.to_text.page_separators,
            split_mode: index_of(&SPLIT_MODES, &s.mode),
            split_every: to_i32(s.every),
            split_ranges: s.ranges.clone(),
            page_size: index_of(&PAGE_SIZES, &m.page_size),
            margin_mm: to_i32(m.margin_mm),
            bookmarks: m.bookmarks,
        }
    }

    /// Write the widget values into `form`. Only the selected action's values
    /// are checked; an error is a plain-English message for the window.
    /// The other actions are updated best effort (invalid values keep the old option).
    pub fn apply(&self, form: &mut PdfForm) -> Result<(), String> {
        form.kind = PdfKind::from_index(self.kind);
        let compress = self.compress_opts(&form.compress);
        let to_images = self.to_images_opts(&form.to_images);
        let split = self.split_opts(&form.split);
        let merge = self.merge_opts();
        form.to_markdown = PdfToMarkdownOpts {
            format: pick(&PdfMarkupFormat::ALL, self.markup_format)
                .unwrap_or(form.to_markdown.format),
            images: self.markup_images,
        };
        form.to_text = PdfToTextOpts {
            page_separators: self.page_separators,
        };
        let check = match form.kind {
            PdfKind::Compress => compress.as_ref().err().cloned(),
            PdfKind::ToImages => to_images.as_ref().err().cloned(),
            PdfKind::ToWord | PdfKind::ToMarkdown | PdfKind::ToText => None,
            PdfKind::Split => split.as_ref().err().cloned(),
            PdfKind::Merge => merge.as_ref().err().cloned(),
        };
        if let Ok(o) = compress {
            form.compress = o;
        }
        if let Ok(o) = to_images {
            form.to_images = o;
        }
        if let Ok(o) = split {
            form.split = o;
        }
        if let Ok(o) = merge {
            form.merge = o;
        }
        check.map_or(Ok(()), Err)
    }

    fn compress_opts(&self, base: &PdfCompressOpts) -> Result<PdfCompressOpts, String> {
        let preset = pick(&PRESETS, self.preset).unwrap_or(base.preset);
        let dpi = in_range(self.compress_dpi, MIN_DPI, MAX_DPI, strings::BAD_DPI);
        // The custom values are kept for later even while a preset is chosen;
        // they only have to be valid when Custom is.
        let dpi = match (preset, dpi) {
            (CompressPreset::Custom, dpi) => dpi?,
            (_, dpi) => dpi.unwrap_or(base.dpi),
        };
        Ok(PdfCompressOpts {
            preset,
            dpi,
            quality: u8::try_from(self.compress_quality.clamp(1, 100)).unwrap_or(base.quality),
        })
    }

    fn to_images_opts(&self, base: &PdfToImagesOpts) -> Result<PdfToImagesOpts, String> {
        let dpi = match pick(&DPI_PRESETS, self.dpi_choice) {
            Some(dpi) => dpi,
            None => in_range(self.dpi_custom, MIN_DPI, MAX_DPI, strings::BAD_DPI)?,
        };
        if let Some(err) = pages_error(&self.pages) {
            return Err(err);
        }
        Ok(PdfToImagesOpts {
            format: pick(&PageImageFormat::ALL, self.image_format).unwrap_or(base.format),
            dpi,
            pages: self.pages.trim().to_owned(),
        })
    }

    fn split_opts(&self, base: &PdfSplitOpts) -> Result<PdfSplitOpts, String> {
        let mode = pick(&SPLIT_MODES, self.split_mode).unwrap_or(base.mode);
        let every = match mode {
            SplitMode::EveryN => in_range(self.split_every, 1, MAX_EVERY, strings::BAD_EVERY)?,
            _ => u32::try_from(self.split_every)
                .ok()
                .filter(|v| (1..=MAX_EVERY).contains(v))
                .unwrap_or(base.every),
        };
        if mode == SplitMode::Ranges
            && let Some(err) = split_ranges_error(&self.split_ranges)
        {
            return Err(err);
        }
        Ok(PdfSplitOpts {
            mode,
            every,
            ranges: self.split_ranges.trim().to_owned(),
        })
    }

    fn merge_opts(&self) -> Result<PdfMergeOpts, String> {
        Ok(PdfMergeOpts {
            page_size: pick(&PAGE_SIZES, self.page_size).unwrap_or_default(),
            margin_mm: in_range(self.margin_mm, 0, MAX_MARGIN_MM, strings::BAD_MARGIN)?,
            bookmarks: self.bookmarks,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip(form: &PdfForm) -> PdfForm {
        let values = PdfValues::from_form(form);
        let mut back = PdfForm::default();
        values.apply(&mut back).unwrap();
        back
    }

    #[test]
    fn defaults_round_trip() {
        for kind in PdfKind::ALL {
            let form = PdfForm {
                kind,
                ..PdfForm::default()
            };
            assert_eq!(round_trip(&form), form, "{kind:?}");
        }
    }

    #[test]
    fn defaults_are_the_easy_choice() {
        let v = PdfValues::from_form(&PdfForm::default());
        assert_eq!(pick(&PRESETS, v.preset), Some(CompressPreset::Medium));
        assert_eq!(pick(&DPI_PRESETS, v.dpi_choice), Some(150));
        assert_eq!(pick(&PAGE_SIZES, v.page_size), Some(PageSize::FitToImage));
        assert!(v.bookmarks);
        assert_eq!(v.pages, "", "all pages");
    }

    #[test]
    fn non_default_options_round_trip() {
        let form = PdfForm {
            kind: PdfKind::Split,
            compress: PdfCompressOpts {
                preset: CompressPreset::Custom,
                dpi: 200,
                quality: 55,
            },
            to_images: PdfToImagesOpts {
                format: PageImageFormat::Webp,
                dpi: 96,
                pages: "1-3,7".into(),
            },
            to_word: PdfToWordOpts {},
            to_markdown: PdfToMarkdownOpts {
                format: PdfMarkupFormat::Html,
                images: false,
            },
            to_text: PdfToTextOpts {
                page_separators: true,
            },
            split: PdfSplitOpts {
                mode: SplitMode::Ranges,
                every: 5,
                ranges: "1-3,4-10,11-".into(),
            },
            merge: PdfMergeOpts {
                page_size: PageSize::Letter,
                margin_mm: 10,
                bookmarks: false,
            },
        };
        assert_eq!(round_trip(&form), form);

        let mut other = form.clone();
        other.to_images.dpi = 300;
        other.split.mode = SplitMode::EveryN;
        other.merge.page_size = PageSize::A4;
        assert_eq!(round_trip(&other), other);
        // A preset keeps the custom values for later.
        other.compress.preset = CompressPreset::High;
        assert_eq!(round_trip(&other), other);
    }

    #[test]
    fn widget_values_map_to_options() {
        let mut form = PdfForm::default();
        let mut v = PdfValues::from_form(&form);
        v.kind = PdfKind::Compress.index();
        v.preset = index_of(&PRESETS, &CompressPreset::Custom);
        v.compress_dpi = 120;
        v.compress_quality = 140;
        v.apply(&mut form).unwrap();
        let Action::PdfCompress(o) = form.action() else {
            panic!("wrong action")
        };
        assert_eq!(o.target(), (120, 100), "quality is clamped");

        v.kind = PdfKind::ToImages.index();
        v.image_format = index_of(&PageImageFormat::ALL, &PageImageFormat::Jpg);
        v.dpi_choice = DPI_CUSTOM;
        v.dpi_custom = 450;
        v.pages = " 2-3 ".into();
        v.apply(&mut form).unwrap();
        let Action::PdfToImages(o) = form.action() else {
            panic!("wrong action")
        };
        assert_eq!(
            (o.format, o.dpi, o.pages.as_str()),
            (PageImageFormat::Jpg, 450, "2-3")
        );

        v.kind = PdfKind::Merge.index();
        v.page_size = index_of(&PAGE_SIZES, &PageSize::A4);
        v.margin_mm = 12;
        v.bookmarks = false;
        v.apply(&mut form).unwrap();
        assert_eq!(
            form.action(),
            Action::PdfMerge(PdfMergeOpts {
                page_size: PageSize::A4,
                margin_mm: 12,
                bookmarks: false
            })
        );
        assert_eq!(form.action().id(), actions::PDF_MERGE);
    }

    #[test]
    fn markdown_and_html_targets() {
        let mut form = PdfForm::default();
        let mut v = PdfValues::from_form(&form);
        assert!(v.markup_images, "Save pictures is on by default");
        v.kind = PdfKind::ToMarkdown.index();
        v.apply(&mut form).unwrap();
        assert_eq!(
            form.action(),
            Action::PdfToMarkdown(PdfToMarkdownOpts {
                format: PdfMarkupFormat::Md,
                images: true
            })
        );
        v.markup_format = index_of(&PdfMarkupFormat::ALL, &PdfMarkupFormat::Html);
        v.markup_images = false;
        v.apply(&mut form).unwrap();
        assert_eq!(
            form.action(),
            Action::PdfToMarkdown(PdfToMarkdownOpts {
                format: PdfMarkupFormat::Html,
                images: false
            })
        );
        assert_eq!(form.action().id(), actions::PDF_TO_MARKDOWN);
        // The PDF Convert chips set `pdf-markup-format` 0 = Markdown, 1 = HTML.
        assert_eq!(
            PdfMarkupFormat::ALL,
            [PdfMarkupFormat::Md, PdfMarkupFormat::Html],
            "the Markdown / HTML chips map to these indexes"
        );
        // A menu preset picks the format.
        let mut preset = Map::new();
        preset.insert("format".into(), Value::from("html"));
        let form = PdfForm::prefill(&State::default(), PdfKind::ToMarkdown, &preset);
        assert_eq!(form.to_markdown.format, PdfMarkupFormat::Html);
    }

    #[test]
    fn range_validation_messages() {
        assert_eq!(pages_error(""), None, "empty = all pages");
        assert_eq!(pages_error("all"), None);
        assert_eq!(pages_error("1-3,7,10-"), None);
        assert_eq!(pages_error(" 1 - 3 , 7 "), None);
        assert_eq!(pages_error("-5"), None);
        for bad in ["abc", "0", "3-1", "1--2", "-", "1.5"] {
            assert_eq!(
                pages_error(bad),
                Some(strings::bad_page_range(bad)),
                "{bad}"
            );
        }
        assert_eq!(
            pages_error(" x "),
            Some("\"x\" isn't a valid page range. Use something like 1-3,7,10-.".into())
        );
        assert_eq!(
            split_ranges_error("  "),
            Some(strings::SPLIT_RANGES_EMPTY.into())
        );
        assert_eq!(split_ranges_error("1-3,4-10,11-"), None);
        assert_eq!(
            split_ranges_error("4-2"),
            Some(strings::bad_page_range("4-2"))
        );
    }

    #[test]
    fn invalid_values_only_block_the_selected_action() {
        let mut form = PdfForm::default();
        let mut v = PdfValues::from_form(&form);
        v.pages = "nope".into();
        v.split_mode = index_of(&SPLIT_MODES, &SplitMode::Ranges);
        v.split_ranges = String::new();
        assert_eq!(v.apply(&mut form), Ok(()), "Compress is selected");
        assert_eq!(form.to_images.pages, "", "bad value keeps the old one");
        v.kind = PdfKind::ToImages.index();
        assert_eq!(v.apply(&mut form), Err(strings::bad_page_range("nope")));
        v.kind = PdfKind::Split.index();
        assert_eq!(
            v.apply(&mut form),
            Err(strings::SPLIT_RANGES_EMPTY.to_owned())
        );
        v.split_mode = index_of(&SPLIT_MODES, &SplitMode::EveryN);
        v.split_every = 0;
        assert_eq!(v.apply(&mut form), Err(strings::BAD_EVERY.to_owned()));
        v.kind = PdfKind::Compress.index();
        v.preset = index_of(&PRESETS, &CompressPreset::Custom);
        v.compress_dpi = 5;
        assert_eq!(v.apply(&mut form), Err(strings::BAD_DPI.to_owned()));
        v.kind = PdfKind::Merge.index();
        v.margin_mm = -1;
        assert_eq!(v.apply(&mut form), Err(strings::BAD_MARGIN.to_owned()));
    }

    #[test]
    fn remembered_settings_round_trip_for_each_action() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.toml");
        let form = PdfForm {
            kind: PdfKind::Compress,
            compress: PdfCompressOpts {
                preset: CompressPreset::Custom,
                dpi: 220,
                quality: 66,
            },
            to_images: PdfToImagesOpts {
                format: PageImageFormat::Jpg,
                dpi: 300,
                pages: "2-".into(),
            },
            to_word: PdfToWordOpts {},
            to_markdown: PdfToMarkdownOpts {
                format: PdfMarkupFormat::Html,
                images: false,
            },
            to_text: PdfToTextOpts {
                page_separators: true,
            },
            split: PdfSplitOpts {
                mode: SplitMode::EveryN,
                every: 4,
                ranges: String::new(),
            },
            merge: PdfMergeOpts {
                page_size: PageSize::A4,
                margin_mm: 15,
                bookmarks: false,
            },
        };
        let mut state = State::default();
        for kind in PdfKind::ALL {
            let f = PdfForm {
                kind,
                ..form.clone()
            };
            state.remember_action(&f.action());
        }
        state.save(&path).unwrap();
        let loaded = State::load(&path);
        for kind in PdfKind::ALL {
            let back = PdfForm::prefill(&loaded, kind, &Map::new());
            assert_eq!(
                back,
                PdfForm {
                    kind,
                    ..form.clone()
                },
                "{kind:?}"
            );
        }

        // Remember off → defaults.
        let mut off = loaded.clone();
        off.remember = false;
        let back = PdfForm::prefill(&off, PdfKind::Split, &Map::new());
        assert_eq!(
            back,
            PdfForm {
                kind: PdfKind::Split,
                ..PdfForm::default()
            }
        );
    }

    #[test]
    fn kind_indexes_and_ids() {
        for k in PdfKind::ALL {
            assert_eq!(PdfKind::from_index(k.index()), k);
            assert_eq!(PdfKind::from_id(k.id()), Some(k));
            assert_eq!(Action::default_for(k.id()).map(|a| a.id()), Some(k.id()));
        }
        assert_eq!(PdfKind::from_index(-1), PdfKind::Compress);
        assert_eq!(
            PdfKind::from_id(actions::PDF_TO_WORD),
            Some(PdfKind::ToWord)
        );
        assert_eq!(PdfKind::ToWord.index(), 2, "after To images, like the menu");
        assert_eq!(
            PdfKind::ToMarkdown.index(),
            3,
            "after To Word, like the menu"
        );
        assert_eq!(PdfKind::from_id(actions::IMAGE_CONVERT), None);
    }
}
