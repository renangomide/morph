// SPDX-License-Identifier: GPL-3.0-or-later
//! Office / text-document option panels (SPEC §3.2, §4.5) as plain Rust, apart from Slint.
//!
//! Same shape as [`crate::options`]: [`OfficeForm`] keeps one options struct per
//! document action, [`OfficeValues`] is the flat widget view the window binds to,
//! and [`OfficeValues::apply`] turns it back into real options.
//!
//! The target format list depends on the files ([`targets`]): only formats every
//! selected family can produce, never a format all files already are. Files that
//! already are the chosen format, or that need an Office engine that isn't
//! installed, are skipped with a reason ([`plan_files`]); everything else still
//! converts (SPEC §3.2: "Convert is disabled for those files only").

use crate::pdf_options::{DPI_CUSTOM, MAX_DPI, MIN_DPI};
use crate::strings;
use morph_core::Action;
use morph_core::office::options::{
    CsvDelimiter, DocFormat, DocImageFormat, Family, OfficeConvertOpts, OfficeToImagesOpts,
};
use morph_core::office::{self, EnginePreference, Engines};
use morph_core::pdf::options::DPI_PRESETS;
use morph_core::state::State;
use morph_formats::{Category, actions};
use serde_json::{Map, Value};
use std::path::{Path, PathBuf};

/// The document actions of the action selector, in display order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OfficeKind {
    #[default]
    Convert,
    ToImages,
}

impl OfficeKind {
    pub const ALL: [OfficeKind; 2] = [OfficeKind::Convert, OfficeKind::ToImages];

    pub fn id(self) -> &'static str {
        match self {
            OfficeKind::Convert => actions::OFFICE_CONVERT,
            OfficeKind::ToImages => actions::OFFICE_TO_IMAGES,
        }
    }

    pub fn from_id(id: &str) -> Option<OfficeKind> {
        OfficeKind::ALL.into_iter().find(|k| k.id() == id)
    }

    pub fn index(self) -> i32 {
        match self {
            OfficeKind::Convert => 0,
            OfficeKind::ToImages => 1,
        }
    }

    /// Out-of-range indexes mean Convert.
    pub fn from_index(index: i32) -> OfficeKind {
        usize::try_from(index)
            .ok()
            .and_then(|i| OfficeKind::ALL.get(i).copied())
            .unwrap_or(OfficeKind::Convert)
    }

    pub fn label(self) -> &'static str {
        match self {
            OfficeKind::Convert => strings::OFFICE_ACTION_CONVERT,
            OfficeKind::ToImages => strings::OFFICE_ACTION_TO_IMAGES,
        }
    }
}

/// Options of every document action, plus the selected one.
#[derive(Debug, Clone, PartialEq)]
pub struct OfficeForm {
    pub kind: OfficeKind,
    pub convert: OfficeConvertOpts,
    pub to_images: OfficeToImagesOpts,
}

impl Default for OfficeForm {
    fn default() -> Self {
        OfficeForm {
            kind: OfficeKind::Convert,
            convert: OfficeConvertOpts::default(),
            to_images: OfficeToImagesOpts::default(),
        }
    }
}

/// The user (remembered settings) or the menu (preset) chose `key` of action `id`.
fn chosen(state: &State, id: &str, preset: &Map<String, Value>, key: &str) -> bool {
    preset.contains_key(key)
        || (state.remember && state.actions.get(id).is_some_and(|m| m.contains_key(key)))
}

impl OfficeForm {
    /// Pre-fill the window: `kind` is selected; every action gets defaults →
    /// remembered options (`state`) → and, for `kind` only, the menu preset on top.
    /// A target format that doesn't fit `files` (or nobody chose) becomes the
    /// best default for them ([`default_target`]).
    pub fn prefill(
        state: &State,
        kind: OfficeKind,
        preset: &Map<String, Value>,
        files: &[PathBuf],
        engines: &Engines,
        preference: EnginePreference,
    ) -> OfficeForm {
        let mut form = OfficeForm {
            kind,
            ..OfficeForm::default()
        };
        let empty = Map::new();
        let preset_for = |k: OfficeKind| if k == kind { preset } else { &empty };
        for k in OfficeKind::ALL {
            if let Some(action) = state.action_for(k.id(), preset_for(k)) {
                form.set_action(action);
            }
        }
        let convert_preset = preset_for(OfficeKind::Convert);
        let list = targets(files);
        if !chosen(state, actions::OFFICE_CONVERT, convert_preset, "format")
            || !list.contains(&form.convert.format)
        {
            form.convert.format = default_target(files, &list, engines, preference);
        }
        form
    }

    /// The selected action with its options.
    pub fn action(&self) -> Action {
        match self.kind {
            OfficeKind::Convert => Action::OfficeConvert(self.convert.clone()),
            OfficeKind::ToImages => Action::OfficeToImages(self.to_images.clone()),
        }
    }

    /// Store `action`'s options (does not change the selected kind).
    pub fn set_action(&mut self, action: Action) {
        match action {
            Action::OfficeConvert(o) => self.convert = o,
            Action::OfficeToImages(o) => self.to_images = o,
            // Not a document action: nothing to store in the document form.
            _ => {}
        }
    }

    /// The selection changed (a file was removed): keep the format if it still
    /// fits, else pick the default for the files left.
    pub fn fit_to(&mut self, files: &[PathBuf], engines: &Engines, preference: EnginePreference) {
        let list = targets(files);
        if !list.contains(&self.convert.format) {
            self.convert.format = default_target(files, &list, engines, preference);
        }
    }
}

fn document_files(files: &[PathBuf]) -> impl Iterator<Item = (&PathBuf, Family)> {
    files
        .iter()
        .filter_map(|f| Family::of_path(f).map(|family| (f, family)))
}

/// Target formats for `files` (SPEC §4.5), in a fixed order (PDF first): the
/// formats every selected family can produce, minus a format that every file
/// already is. Never empty.
pub fn targets(files: &[PathBuf]) -> Vec<DocFormat> {
    let docs: Vec<(&PathBuf, Family)> = document_files(files).collect();
    if docs.is_empty() {
        return vec![DocFormat::Pdf];
    }
    let list: Vec<DocFormat> = DocFormat::ALL
        .into_iter()
        .filter(|format| docs.iter().all(|(_, family)| family.can_export(*format)))
        .filter(|format| {
            !docs
                .iter()
                .all(|(file, _)| DocFormat::of_path(file) == Some(*format))
        })
        .collect();
    if list.is_empty() {
        vec![DocFormat::Pdf]
    } else {
        list
    }
}

/// The best target for non-technical users when nobody chose one: CSV files →
/// XLSX, text files → PDF (or HTML / Markdown natively when no Office engine is
/// installed), everything else → PDF.
pub fn default_target(
    files: &[PathBuf],
    list: &[DocFormat],
    engines: &Engines,
    preference: EnginePreference,
) -> DocFormat {
    let docs: Vec<(&PathBuf, Family)> = document_files(files).collect();
    let all = |pred: &dyn Fn(&PathBuf, Family) -> bool| {
        !docs.is_empty() && docs.iter().all(|(f, fam)| pred(f, *fam))
    };
    let candidates: &[DocFormat] = if all(&|f, _| DocFormat::of_path(f) == Some(DocFormat::Csv)) {
        &[DocFormat::Xlsx]
    } else if all(&|_, fam| fam == Family::Text) {
        if engines.resolve(Family::Text, preference).is_some() {
            &[DocFormat::Pdf]
        } else {
            &[DocFormat::Html, DocFormat::Md]
        }
    } else {
        &[DocFormat::Pdf]
    };
    candidates
        .iter()
        .copied()
        .find(|f| list.contains(f))
        .or_else(|| list.first().copied())
        .unwrap_or_default()
}

/// Any CSV input (the separator option applies).
pub fn has_csv_input(files: &[PathBuf]) -> bool {
    files
        .iter()
        .any(|f| DocFormat::of_path(f) == Some(DocFormat::Csv))
}

/// Which Markdown options apply to `files` (shown only when Markdown is the target).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MarkdownExtras {
    /// "Save pictures": documents, presentations, HTML.
    pub pictures: bool,
    /// "Rows per sheet": spreadsheets.
    pub sheets: bool,
    /// "Include speaker notes": presentations.
    pub slides: bool,
}

pub fn markdown_extras(files: &[PathBuf]) -> MarkdownExtras {
    let mut extras = MarkdownExtras::default();
    for (_, family) in document_files(files) {
        match family {
            Family::Excel => extras.sheets = true,
            Family::PowerPoint => {
                extras.slides = true;
                extras.pictures = true;
            }
            Family::Word | Family::Text => extras.pictures = true,
        }
    }
    extras
}

/// Any file that has pages/slides to export as images.
pub fn has_image_input(files: &[PathBuf]) -> bool {
    document_files(files).any(|(_, family)| family.has_images())
}

// ---------------------------------------------------------------- engines

/// What happens to each selected file under an action.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FilePlan {
    /// Files to hand to the job (inputs the action doesn't take at all are
    /// still in here: the job skips them as usual).
    pub run: Vec<PathBuf>,
    /// Files Morph skips up front, with the reason shown in the results.
    pub skipped: Vec<(PathBuf, String)>,
    /// How many of `skipped` need an Office engine that isn't installed (banner).
    pub missing_engine: usize,
    /// Engines that will be used ("Microsoft Word", "LibreOffice"), without repeats.
    pub engines: Vec<&'static str>,
}

/// The engine family `file` needs under `action`, if it needs one.
fn engine_family(file: &Path, action: &Action) -> Option<Family> {
    match action {
        Action::OfficeConvert(o) => match office::plan(file, o.format) {
            Ok(plan) if plan.needs_engine() => Family::of_path(file),
            _ => None,
        },
        Action::OfficeToImages(_) => Family::of_path(file).filter(|f| f.has_images()),
        Action::PdfToWord(_) => {
            (Category::from_path(file) == Some(Category::Pdf)).then_some(Family::Word)
        }
        _ => None,
    }
}

/// Split `files` into what runs and what is skipped up front (already the
/// target format, or no engine for it), and name the engines that will run.
pub fn plan_files(
    files: &[PathBuf],
    action: &Action,
    engines: &Engines,
    preference: EnginePreference,
) -> FilePlan {
    let mut plan = FilePlan::default();
    for file in files {
        if let Action::OfficeConvert(o) = action
            && Family::of_path(file).is_some()
            && DocFormat::of_path(file) == Some(o.format)
        {
            plan.skipped.push((
                file.clone(),
                strings::already_format(strings::doc_format_short(o.format)),
            ));
            continue;
        }
        if let Some(family) = engine_family(file, action) {
            let choice = match action {
                Action::PdfToWord(_) => engines.resolve_pdf_to_word(preference),
                _ => engines.resolve(family, preference),
            };
            match choice {
                Some(choice) => {
                    let name = choice.display_name();
                    if !plan.engines.contains(&name) {
                        plan.engines.push(name);
                    }
                }
                None => {
                    plan.skipped
                        .push((file.clone(), strings::NEEDS_ENGINE.to_owned()));
                    plan.missing_engine += 1;
                    continue;
                }
            }
        }
        plan.run.push(file.clone());
    }
    plan
}

// ---------------------------------------------------------------- widgets

/// Flat values of the document option widgets.
#[derive(Debug, Clone, PartialEq)]
pub struct OfficeValues {
    pub kind: i32,
    /// Index in the target list ([`targets`]).
    pub format: i32,
    /// Index in `CsvDelimiter::ALL`.
    pub delimiter: i32,
    pub csv_bom: bool,
    /// Markdown target: save pictures, rows per sheet (0 = all), speaker notes.
    pub images: bool,
    pub max_rows: i32,
    pub slide_notes: bool,
    /// Index in `DocImageFormat::ALL`.
    pub image_format: i32,
    /// Index in `DPI_PRESETS`, or [`DPI_CUSTOM`].
    pub dpi_choice: i32,
    pub dpi_custom: i32,
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

impl OfficeValues {
    /// `list`: the target formats shown in the format box.
    pub fn from_form(form: &OfficeForm, list: &[DocFormat]) -> OfficeValues {
        let i = &form.to_images;
        let dpi_choice = DPI_PRESETS
            .iter()
            .position(|d| *d == i.dpi)
            .and_then(|p| i32::try_from(p).ok())
            .unwrap_or(DPI_CUSTOM);
        OfficeValues {
            kind: form.kind.index(),
            format: index_of(list, &form.convert.format),
            delimiter: index_of(&CsvDelimiter::ALL, &form.convert.delimiter),
            csv_bom: form.convert.csv_bom,
            images: form.convert.images,
            max_rows: to_i32(form.convert.max_rows),
            slide_notes: form.convert.slide_notes,
            image_format: index_of(&DocImageFormat::ALL, &i.format),
            dpi_choice,
            dpi_custom: to_i32(i.dpi),
        }
    }

    /// Write the widget values into `form`. Only the selected action's values
    /// are checked; an error is a plain-English message for the window.
    pub fn apply(&self, form: &mut OfficeForm, list: &[DocFormat]) -> Result<(), String> {
        form.kind = OfficeKind::from_index(self.kind);
        let max_rows = u32::try_from(self.max_rows).map_err(|_| strings::BAD_MAX_ROWS.to_owned());
        form.convert = OfficeConvertOpts {
            format: pick(list, self.format).unwrap_or(form.convert.format),
            delimiter: pick(&CsvDelimiter::ALL, self.delimiter).unwrap_or(form.convert.delimiter),
            csv_bom: self.csv_bom,
            images: self.images,
            max_rows: *max_rows.as_ref().unwrap_or(&form.convert.max_rows),
            slide_notes: self.slide_notes,
        };
        if form.kind == OfficeKind::Convert
            && form.convert.format == DocFormat::Md
            && let Err(e) = max_rows
        {
            return Err(e);
        }
        let dpi = match pick(&DPI_PRESETS, self.dpi_choice) {
            Some(dpi) => Ok(dpi),
            None => match u32::try_from(self.dpi_custom) {
                Ok(v) if (MIN_DPI..=MAX_DPI).contains(&v) => Ok(v),
                _ => Err(strings::BAD_DPI.to_owned()),
            },
        };
        let format = pick(&DocImageFormat::ALL, self.image_format).unwrap_or(form.to_images.format);
        let check = match (&dpi, form.kind) {
            (Err(e), OfficeKind::ToImages) => Some(e.clone()),
            _ => None,
        };
        form.to_images = OfficeToImagesOpts {
            format,
            dpi: dpi.unwrap_or(form.to_images.dpi),
        };
        check.map_or(Ok(()), Err)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use morph_core::office::detect::{AppInfo, LibreOfficeInfo, OfficeApp};

    fn files(names: &[&str]) -> Vec<PathBuf> {
        names
            .iter()
            .map(|n| PathBuf::from(r"C:\in").join(n))
            .collect()
    }

    fn none() -> Engines {
        Engines::default()
    }

    fn libre() -> Engines {
        Engines {
            libreoffice: Some(LibreOfficeInfo {
                soffice: PathBuf::from(r"C:\LO\program\soffice.com"),
                version: Some("26.8.0.1".into()),
            }),
            ..Engines::default()
        }
    }

    fn word_only() -> Engines {
        Engines {
            word: Some(AppInfo {
                app: OfficeApp::Word,
                exe: PathBuf::from(r"C:\Office\WINWORD.EXE"),
                version: Some("16.0.1".into()),
            }),
            ..Engines::default()
        }
    }

    const AUTO: EnginePreference = EnginePreference::Auto;

    #[test]
    fn target_lists_per_family_never_offer_the_inputs_own_format() {
        use DocFormat as F;
        assert_eq!(
            targets(&files(&["a.docx"])),
            [F::Pdf, F::Odt, F::Rtf, F::Txt, F::Html, F::Md]
        );
        assert_eq!(
            targets(&files(&["a.doc"])),
            [F::Pdf, F::Docx, F::Odt, F::Rtf, F::Txt, F::Html, F::Md]
        );
        assert_eq!(
            targets(&files(&["a.xlsx"])),
            [F::Pdf, F::Md, F::Ods, F::Csv]
        );
        assert_eq!(
            targets(&files(&["a.csv"])),
            [F::Pdf, F::Md, F::Xlsx, F::Ods]
        );
        assert_eq!(targets(&files(&["a.pptx"])), [F::Pdf, F::Md, F::Odp]);
        assert_eq!(
            targets(&files(&["a.ppt"])),
            [F::Pdf, F::Md, F::Pptx, F::Odp]
        );
        assert_eq!(
            targets(&files(&["a.md"])),
            [F::Pdf, F::Docx, F::Odt, F::Txt, F::Html]
        );
        assert_eq!(
            targets(&files(&["a.markdown"])),
            [F::Pdf, F::Docx, F::Odt, F::Txt, F::Html]
        );
        assert_eq!(
            targets(&files(&["a.htm", "b.html"])),
            [F::Pdf, F::Docx, F::Odt, F::Txt, F::Md]
        );
        assert_eq!(
            targets(&files(&["a.txt"])),
            [F::Pdf, F::Docx, F::Odt, F::Html, F::Md]
        );
        // Mixed: only what every family can produce; a format some files
        // already are stays (those files are skipped).
        assert_eq!(
            targets(&files(&["a.docx", "b.doc"])),
            [F::Pdf, F::Docx, F::Odt, F::Rtf, F::Txt, F::Html, F::Md]
        );
        assert_eq!(targets(&files(&["a.docx", "b.xlsx"])), [F::Pdf, F::Md]);
        assert_eq!(targets(&files(&["a.pptx", "b.md"])), [F::Pdf, F::Md]);
        assert_eq!(
            targets(&files(&["a.docx", "b.md"])),
            [F::Pdf, F::Docx, F::Odt, F::Txt, F::Html, F::Md]
        );
        // Non-documents don't narrow the list.
        assert_eq!(
            targets(&files(&["a.csv", "b.jpg"])),
            [F::Pdf, F::Md, F::Xlsx, F::Ods]
        );
        assert_eq!(targets(&files(&["b.jpg"])), [F::Pdf]);
        for list in [targets(&files(&["a.pdf"])), targets(&[])] {
            assert!(!list.is_empty());
        }
    }

    #[test]
    fn defaults_are_the_easy_choice() {
        use DocFormat as F;
        let pick = |names: &[&str], engines: &Engines| {
            let f = files(names);
            default_target(&f, &targets(&f), engines, AUTO)
        };
        assert_eq!(pick(&["a.docx"], &libre()), F::Pdf);
        assert_eq!(pick(&["a.docx"], &none()), F::Pdf);
        assert_eq!(pick(&["a.csv"], &none()), F::Xlsx);
        assert_eq!(pick(&["a.csv", "b.CSV"], &libre()), F::Xlsx);
        assert_eq!(pick(&["a.csv", "b.xlsx"], &libre()), F::Pdf);
        assert_eq!(pick(&["a.pptx"], &libre()), F::Pdf);
        assert_eq!(pick(&["a.md"], &libre()), F::Pdf);
        assert_eq!(pick(&["a.md"], &word_only()), F::Pdf);
        // No engine: text files still get a native target.
        assert_eq!(pick(&["a.md", "b.txt"], &none()), F::Html);
        assert_eq!(pick(&["a.html"], &none()), F::Md);
        assert!(OfficeForm::default().convert.csv_bom, "BOM on by default");
    }

    fn state_with(id: &str, options: &[(&str, Value)]) -> State {
        let mut state = State::default();
        let map: Map<String, Value> = options
            .iter()
            .map(|(k, v)| ((*k).to_owned(), v.clone()))
            .collect();
        state.actions.insert(id.into(), map);
        state
    }

    fn preset(k: &str, v: &str) -> Map<String, Value> {
        let mut m = Map::new();
        m.insert(k.into(), Value::from(v));
        m
    }

    #[test]
    fn menu_preset_selects_action_and_format() {
        let state = State::default();
        let docx = files(&["a.doc"]);
        let form = OfficeForm::prefill(
            &state,
            OfficeKind::Convert,
            &preset("format", "docx"),
            &docx,
            &libre(),
            AUTO,
        );
        assert_eq!(form.kind, OfficeKind::Convert);
        assert_eq!(form.convert.format, DocFormat::Docx);
        let form = OfficeForm::prefill(
            &state,
            OfficeKind::Convert,
            &preset("format", "txt"),
            &files(&["a.docx"]),
            &libre(),
            AUTO,
        );
        assert_eq!(form.convert.format, DocFormat::Txt, "Export as text…");
        let form = OfficeForm::prefill(
            &state,
            OfficeKind::ToImages,
            &Map::new(),
            &files(&["a.pptx"]),
            &libre(),
            AUTO,
        );
        assert_eq!(form.kind, OfficeKind::ToImages);
        assert_eq!(form.to_images, OfficeToImagesOpts::default());
        // "More options…" on a CSV: XLSX, BOM on.
        let form = OfficeForm::prefill(
            &state,
            OfficeKind::Convert,
            &Map::new(),
            &files(&["a.csv"]),
            &none(),
            AUTO,
        );
        assert_eq!(form.convert.format, DocFormat::Xlsx);
        assert!(form.convert.csv_bom);
    }

    #[test]
    fn remembered_options_win_when_they_fit() {
        let state = state_with(
            actions::OFFICE_CONVERT,
            &[
                ("format", Value::from("odt")),
                ("delimiter", Value::from("semicolon")),
                ("csv_bom", Value::from(false)),
            ],
        );
        let doc = files(&["a.docx"]);
        let form = OfficeForm::prefill(
            &state,
            OfficeKind::Convert,
            &Map::new(),
            &doc,
            &libre(),
            AUTO,
        );
        assert_eq!(form.convert.format, DocFormat::Odt);
        assert_eq!(form.convert.delimiter, CsvDelimiter::Semicolon);
        assert!(!form.convert.csv_bom, "the user turned the BOM off");
        // ODT doesn't fit a spreadsheet: back to the default for it.
        let form = OfficeForm::prefill(
            &state,
            OfficeKind::Convert,
            &Map::new(),
            &files(&["a.csv"]),
            &libre(),
            AUTO,
        );
        assert_eq!(form.convert.format, DocFormat::Xlsx);
        // Remember off → defaults.
        let mut off = state.clone();
        off.remember = false;
        let form =
            OfficeForm::prefill(&off, OfficeKind::Convert, &Map::new(), &doc, &libre(), AUTO);
        assert_eq!(form.convert, OfficeForm::default().convert);
    }

    #[test]
    fn remembered_settings_round_trip_through_state_toml() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.toml");
        let form = OfficeForm {
            kind: OfficeKind::Convert,
            convert: OfficeConvertOpts {
                format: DocFormat::Csv,
                delimiter: CsvDelimiter::Tab,
                csv_bom: false,
                images: false,
                max_rows: 50,
                slide_notes: false,
            },
            to_images: OfficeToImagesOpts {
                format: DocImageFormat::Jpg,
                dpi: 300,
            },
        };
        let mut state = State::default();
        for kind in OfficeKind::ALL {
            state.remember_action(
                &OfficeForm {
                    kind,
                    ..form.clone()
                }
                .action(),
            );
        }
        state.save(&path).unwrap();
        let loaded = State::load(&path);
        let sheet = files(&["a.xlsx"]);
        for kind in OfficeKind::ALL {
            let back = OfficeForm::prefill(&loaded, kind, &Map::new(), &sheet, &libre(), AUTO);
            assert_eq!(
                back,
                OfficeForm {
                    kind,
                    ..form.clone()
                },
                "{kind:?}"
            );
        }
    }

    #[test]
    fn widget_values_round_trip() {
        let list = targets(&files(&["a.xlsx"]));
        let forms = [
            OfficeForm::default(),
            OfficeForm {
                kind: OfficeKind::ToImages,
                convert: OfficeConvertOpts {
                    format: DocFormat::Csv,
                    delimiter: CsvDelimiter::Semicolon,
                    csv_bom: false,
                    ..Default::default()
                },
                to_images: OfficeToImagesOpts {
                    format: DocImageFormat::Jpg,
                    dpi: 450,
                },
            },
        ];
        for form in forms {
            let v = OfficeValues::from_form(&form, &list);
            let mut back = OfficeForm::default();
            v.apply(&mut back, &list).unwrap();
            assert_eq!(back, form);
        }
        // Widget indexes → options.
        let mut form = OfficeForm::default();
        let mut v = OfficeValues::from_form(&form, &list);
        v.format = index_of(&list, &DocFormat::Ods);
        v.delimiter = index_of(&CsvDelimiter::ALL, &CsvDelimiter::Comma);
        v.apply(&mut form, &list).unwrap();
        assert_eq!(
            form.action(),
            Action::OfficeConvert(OfficeConvertOpts {
                format: DocFormat::Ods,
                delimiter: CsvDelimiter::Comma,
                csv_bom: true,
                ..Default::default()
            })
        );
        v.kind = OfficeKind::ToImages.index();
        v.dpi_choice = DPI_CUSTOM;
        v.dpi_custom = 5;
        assert_eq!(v.apply(&mut form, &list), Err(strings::BAD_DPI.to_owned()));
        v.kind = OfficeKind::Convert.index();
        assert_eq!(
            v.apply(&mut form, &list),
            Ok(()),
            "DPI only matters for images"
        );
        v.kind = OfficeKind::ToImages.index();
        v.dpi_choice = 2;
        v.image_format = 1;
        v.apply(&mut form, &list).unwrap();
        assert_eq!(form.action().id(), actions::OFFICE_TO_IMAGES);
        assert_eq!(
            form.to_images,
            OfficeToImagesOpts {
                format: DocImageFormat::Jpg,
                dpi: 300
            }
        );
    }

    #[test]
    fn markdown_options_map_and_show_where_relevant() {
        assert_eq!(
            markdown_extras(&files(&["a.xlsx"])),
            MarkdownExtras {
                pictures: false,
                sheets: true,
                slides: false
            }
        );
        assert_eq!(
            markdown_extras(&files(&["a.pptx"])),
            MarkdownExtras {
                pictures: true,
                sheets: false,
                slides: true
            }
        );
        assert_eq!(
            markdown_extras(&files(&["a.docx", "b.html"])),
            MarkdownExtras {
                pictures: true,
                ..Default::default()
            }
        );
        assert_eq!(
            markdown_extras(&files(&["a.jpg"])),
            MarkdownExtras::default()
        );

        let list = targets(&files(&["a.xlsx"]));
        assert!(list.contains(&DocFormat::Csv) && list.contains(&DocFormat::Md));
        let mut form = OfficeForm::default();
        let mut v = OfficeValues::from_form(&form, &list);
        assert!(v.images && v.slide_notes && v.max_rows == 0, "defaults");
        v.format = index_of(&list, &DocFormat::Md);
        v.images = false;
        v.max_rows = 100;
        v.slide_notes = false;
        v.apply(&mut form, &list).unwrap();
        assert_eq!(
            form.action(),
            Action::OfficeConvert(OfficeConvertOpts {
                format: DocFormat::Md,
                images: false,
                max_rows: 100,
                slide_notes: false,
                ..Default::default()
            })
        );
        v.max_rows = -1;
        assert_eq!(
            v.apply(&mut form, &list),
            Err(strings::BAD_MAX_ROWS.to_owned())
        );
        assert_eq!(form.convert.max_rows, 100, "bad value keeps the old one");
        v.format = index_of(&list, &DocFormat::Pdf);
        assert_eq!(v.apply(&mut form, &list), Ok(()), "only for Markdown");
    }

    fn convert_to(format: DocFormat) -> Action {
        Action::OfficeConvert(OfficeConvertOpts {
            format,
            ..OfficeConvertOpts::default()
        })
    }

    #[test]
    fn banner_logic_engine_present_or_absent() {
        let sel = files(&["a.csv", "b.docx", "c.md"]);
        // No engine: native conversions still run, engine ones are skipped.
        let p = plan_files(&sel, &convert_to(DocFormat::Pdf), &none(), AUTO);
        assert_eq!(p.missing_engine, 3);
        assert!(p.run.is_empty() && p.engines.is_empty());
        assert_eq!(p.skipped[0].1, strings::NEEDS_ENGINE);

        let csv = files(&["a.csv"]);
        let p = plan_files(&csv, &convert_to(DocFormat::Xlsx), &none(), AUTO);
        assert_eq!(
            (p.missing_engine, p.run.len()),
            (0, 1),
            "CSV → XLSX is native"
        );
        let sheet = files(&["a.xlsx"]);
        let p = plan_files(&sheet, &convert_to(DocFormat::Csv), &none(), AUTO);
        assert_eq!(
            (p.missing_engine, p.run.len()),
            (0, 1),
            "XLSX → CSV is native"
        );
        let text = files(&["a.md", "b.txt", "c.html"]);
        let p = plan_files(&text, &convert_to(DocFormat::Html), &none(), AUTO);
        assert_eq!(p.missing_engine, 0, "md/txt → HTML are native");
        assert_eq!(p.run.len(), 2);
        assert_eq!(
            p.skipped,
            vec![(text[2].clone(), strings::already_format("HTML"))]
        );
        let p = plan_files(&text, &convert_to(DocFormat::Docx), &none(), AUTO);
        assert_eq!(p.missing_engine, 3, "text → DOCX needs an engine");

        // With LibreOffice: everything runs, and the window says so.
        let p = plan_files(&sel, &convert_to(DocFormat::Pdf), &libre(), AUTO);
        assert_eq!((p.missing_engine, p.run.len()), (0, 3));
        assert_eq!(p.engines, ["LibreOffice"]);
        // Word only: Word for the document, nothing for the spreadsheet.
        let p = plan_files(&sel, &convert_to(DocFormat::Pdf), &word_only(), AUTO);
        assert_eq!(p.missing_engine, 1);
        assert_eq!(p.engines, ["Microsoft Word"]);
        assert_eq!(p.skipped[0].0, sel[0]);

        // Export as images / PDF → Word.
        let deck = files(&["a.pptx", "b.xlsx"]);
        let images = Action::default_for(actions::OFFICE_TO_IMAGES).unwrap();
        let p = plan_files(&deck, &images, &none(), AUTO);
        assert_eq!(p.missing_engine, 1);
        assert_eq!(
            p.run,
            [deck[1].clone()],
            "the job skips the spreadsheet itself"
        );
        let pdf = files(&["a.pdf"]);
        let to_word = Action::default_for(actions::PDF_TO_WORD).unwrap();
        assert_eq!(plan_files(&pdf, &to_word, &none(), AUTO).missing_engine, 1);
        let p = plan_files(&pdf, &to_word, &libre(), EnginePreference::MicrosoftOffice);
        assert_eq!(
            p.engines,
            ["LibreOffice"],
            "falls back to the installed one"
        );
        // Other actions never need an engine.
        let compress = Action::default_for(actions::PDF_COMPRESS).unwrap();
        assert_eq!(plan_files(&pdf, &compress, &none(), AUTO).run, pdf);
    }

    #[test]
    fn helpers_and_indexes() {
        assert!(has_csv_input(&files(&["a.CSV", "b.docx"])));
        assert!(!has_csv_input(&files(&["a.xlsx"])));
        assert!(has_image_input(&files(&["a.xlsx", "b.odp"])));
        assert!(!has_image_input(&files(&["a.xlsx", "b.md"])));
        for k in OfficeKind::ALL {
            assert_eq!(OfficeKind::from_index(k.index()), k);
            assert_eq!(OfficeKind::from_id(k.id()), Some(k));
            assert_eq!(Action::default_for(k.id()).map(|a| a.id()), Some(k.id()));
        }
        assert_eq!(OfficeKind::from_index(7), OfficeKind::Convert);
        assert_eq!(OfficeKind::from_id(actions::PDF_TO_WORD), None);
        let mut form = OfficeForm {
            convert: convert_to_opts(DocFormat::Odt),
            ..OfficeForm::default()
        };
        form.fit_to(&files(&["a.xlsx"]), &libre(), AUTO);
        assert_eq!(form.convert.format, DocFormat::Pdf);
        form.convert.format = DocFormat::Csv;
        form.fit_to(&files(&["a.xlsx", "b.csv"]), &libre(), AUTO);
        assert_eq!(form.convert.format, DocFormat::Csv, "still fits");
    }

    fn convert_to_opts(format: DocFormat) -> OfficeConvertOpts {
        OfficeConvertOpts {
            format,
            ..OfficeConvertOpts::default()
        }
    }
}
