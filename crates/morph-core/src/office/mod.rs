// SPDX-License-Identifier: GPL-3.0-or-later
//! Office documents (SPEC §4.5): Word-, Excel-, PowerPoint- and text-like files.
//!
//! Shared:
//! - [`options`]: `OfficeConvertOpts`, `OfficeToImagesOpts`, [`DocFormat`], [`Family`].
//! - [`detect`]: which engines are installed, which one handles a family
//!   ([`detect::engines`], [`detect::Engines::resolve`]).
//! - [`com`]: Microsoft Office automation runner (STA thread, watchdog, cleanup).
//! - [`soffice`]: LibreOffice headless runner (private profile, kill job).
//! - [`win`]: registry / process helpers.
//! - This file: the `office.convert` router ([`plan`]), engine lookup
//!   ([`engine_for`]) and the shared plain-English errors.
//!
//! One file per conversion:
//! [`engine_convert`] (engine conversions), [`to_images`],
//! [`csv_xlsx`] (native CSV ↔ XLSX), [`to_markdown`] (Word → Markdown
//! through the engine's HTML). Text-like inputs go to [`crate::text`].
//!
//! Native "to Markdown" readers (M5.1, SPEC §4.6):
//! [`docx_markdown`] (DOCX), [`pptx_markdown`] (PPTX; PPT/ODP through
//! the engine → PPTX first), [`sheet_markdown`] (XLSX/XLS/ODS/CSV).
//! They all write with [`crate::markdown`].

pub mod com;
pub mod csv_xlsx;
pub mod detect;
pub mod docx_markdown;
pub mod engine_convert;
pub(crate) mod ole;
pub(crate) mod ooxml;
pub mod options;
pub mod pptx_markdown;
pub mod sheet_markdown;
pub mod soffice;
pub mod to_images;
pub mod to_markdown;
pub mod win;

pub use detect::{EngineChoice, EnginePreference, Engines};
pub use options::{DocFormat, Family, OfficeConvertOpts, OfficeToImagesOpts};

use crate::action::Action;
use crate::converter::{Converter, TaskCtx, TaskOutput};
use crate::error::{ConvertError, ErrorKind};
use crate::job::Task;
use std::path::Path;
use std::time::Duration;

/// Hard limit for one engine conversion (SPEC §7: 180 s, then kill).
pub const ENGINE_TIMEOUT: Duration = Duration::from_secs(180);

/// Where SPEC §3.2's "Download LibreOffice" button goes.
pub const LIBREOFFICE_DOWNLOAD_URL: &str = "https://www.libreoffice.org/download/download/";

// ---------------------------------------------------------------- errors

/// No Office engine is installed (the window shows the banner instead).
pub fn no_engine() -> ConvertError {
    ConvertError::new(
        ErrorKind::MissingEngine,
        "To convert documents, install Microsoft Office or LibreOffice (free).",
        "no Office engine detected",
    )
}

pub fn password_protected() -> ConvertError {
    ConvertError::new(ErrorKind::BadInput, "This file is password-protected.", "")
}

pub fn damaged_document(details: impl ToString) -> ConvertError {
    ConvertError::new(
        ErrorKind::BadInput,
        "This file seems to be damaged or isn't a supported document.",
        details,
    )
}

/// "Microsoft Word took too long and was stopped." / "LibreOffice took too long…"
pub fn engine_timeout(engine: &str) -> ConvertError {
    ConvertError::other(
        format!("{engine} took too long and was stopped."),
        format!("timeout after {}s", ENGINE_TIMEOUT.as_secs()),
    )
}

/// A converter got a task for another action (a dispatcher bug).
pub(crate) fn wrong_action(converter: &str) -> ConvertError {
    crate::pdf::wrong_action(converter)
}

fn no_input() -> ConvertError {
    ConvertError::other(
        "Something went wrong while converting this file.",
        "task without input",
    )
}

// ---------------------------------------------------------------- engines

/// The engine for `family` under the user's preference, or [`no_engine`].
pub fn engine_for(family: Family, ctx: &TaskCtx<'_>) -> Result<EngineChoice, ConvertError> {
    detect::engines()
        .resolve(family, ctx.office_engine())
        .ok_or_else(no_engine)
}

// ---------------------------------------------------------------- router

/// How `office.convert` handles one input (SPEC §4.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Plan {
    /// CSV → XLSX natively ([`csv_xlsx::csv_to_xlsx`]).
    CsvToXlsx,
    /// Spreadsheet (xlsx/xls/ods) → CSV natively, one file per sheet ([`csv_xlsx::to_csv`]).
    SheetToCsv,
    /// Spreadsheet (xlsx/xls/ods/csv) → Markdown natively: `## Sheet` + one
    /// pipe table per sheet ([`sheet_markdown::convert`]).
    SheetToMarkdown,
    /// Text-like input ([`crate::text::convert`]); `engine` = the target needs
    /// the Office engine (DOCX/PDF/ODT).
    Text { engine: bool },
    /// DOCX → Markdown natively, OOXML reader ([`docx_markdown::convert`]).
    DocxToMarkdown,
    /// Other Word-like (doc/odt/rtf) → Markdown: engine → HTML, then HTML →
    /// Markdown natively ([`to_markdown::convert`]).
    WordToMarkdown,
    /// Presentation → Markdown, OOXML reader ([`pptx_markdown::convert`]);
    /// `engine` = ppt/odp, converted to PPTX in the temp folder first.
    SlidesToMarkdown { engine: bool },
    /// Everything else: the Office engine ([`engine_convert::convert`]).
    Engine(Family),
}

impl Plan {
    /// Needs Microsoft Office or LibreOffice (runs on the Office queue).
    pub fn needs_engine(self) -> bool {
        match self {
            Plan::CsvToXlsx | Plan::SheetToCsv | Plan::SheetToMarkdown | Plan::DocxToMarkdown => {
                false
            }
            Plan::Text { engine } | Plan::SlidesToMarkdown { engine } => engine,
            Plan::WordToMarkdown | Plan::Engine(_) => true,
        }
    }
}

/// Decide how to convert `input` to `format`. Errors (plain English) for a
/// format the family can't produce and for converting a file to its own format.
pub fn plan(input: &Path, format: DocFormat) -> Result<Plan, ConvertError> {
    let family = Family::of_path(input).ok_or_else(|| {
        ConvertError::unsupported("This file type isn't supported.", input.display())
    })?;
    if !family.can_export(format) {
        return Err(ConvertError::unsupported(
            format!(
                "A {} can't be converted to {}.",
                family.noun(),
                format.label()
            ),
            format!("{family:?} → {format:?}"),
        ));
    }
    let own = DocFormat::of_path(input);
    if own == Some(format) {
        return Err(ConvertError::unsupported(
            format!("This file already is {}.", format.label()),
            format!("{} → {format:?}", input.display()),
        ));
    }
    Ok(match (family, format) {
        (Family::Excel, DocFormat::Csv) => Plan::SheetToCsv,
        (Family::Excel, DocFormat::Md) => Plan::SheetToMarkdown,
        (Family::Excel, DocFormat::Xlsx) if own == Some(DocFormat::Csv) => Plan::CsvToXlsx,
        (Family::Text, f) => Plan::Text {
            engine: matches!(f, DocFormat::Docx | DocFormat::Pdf | DocFormat::Odt),
        },
        (Family::Word, DocFormat::Md) if is_ext(input, "docx") => Plan::DocxToMarkdown,
        (Family::Word, DocFormat::Md) => Plan::WordToMarkdown,
        (Family::PowerPoint, DocFormat::Md) => Plan::SlidesToMarkdown {
            engine: !is_ext(input, "pptx"),
        },
        (family, _) => Plan::Engine(family),
    })
}

fn is_ext(path: &Path, ext: &str) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case(ext))
}

/// `office.convert`: routes each input to the right converter.
pub struct OfficeConvert;

impl Converter for OfficeConvert {
    fn run(&self, task: &Task, ctx: &TaskCtx<'_>) -> Result<TaskOutput, ConvertError> {
        let Action::OfficeConvert(opts) = &task.action else {
            return Err(wrong_action("OfficeConvert"));
        };
        let input = task.input().ok_or_else(no_input)?;
        match plan(input, opts.format)? {
            Plan::CsvToXlsx => csv_xlsx::csv_to_xlsx(task, input, opts, ctx),
            Plan::SheetToCsv => csv_xlsx::to_csv(task, input, opts, ctx),
            Plan::SheetToMarkdown => sheet_markdown::convert(task, input, opts, ctx),
            Plan::Text { .. } => crate::text::convert(task, input, opts, ctx),
            Plan::DocxToMarkdown => docx_markdown::convert(task, input, opts, ctx),
            Plan::WordToMarkdown => {
                let engine = engine_for(Family::Word, ctx)?;
                to_markdown::convert(task, input, opts, &engine, ctx)
            }
            Plan::SlidesToMarkdown { engine } => {
                let engine = if engine {
                    Some(engine_for(Family::PowerPoint, ctx)?)
                } else {
                    None
                };
                pptx_markdown::convert(task, input, opts, engine.as_ref(), ctx)
            }
            Plan::Engine(family) => {
                let engine = engine_for(family, ctx)?;
                engine_convert::convert(task, input, family, opts, &engine, ctx)
            }
        }
    }
}

/// `office.to_images`: Word pages / PowerPoint slides → images.
pub struct OfficeToImages;

impl Converter for OfficeToImages {
    fn run(&self, task: &Task, ctx: &TaskCtx<'_>) -> Result<TaskOutput, ConvertError> {
        let Action::OfficeToImages(opts) = &task.action else {
            return Err(wrong_action("OfficeToImages"));
        };
        let input = task.input().ok_or_else(no_input)?;
        let family = Family::of_path(input)
            .filter(|f| f.has_images())
            .ok_or_else(|| {
                ConvertError::unsupported(
                    "Only documents and presentations can be exported as images.",
                    input.display(),
                )
            })?;
        let engine = engine_for(family, ctx)?;
        to_images::convert(task, input, family, opts, &engine, ctx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(name: &str) -> &Path {
        Path::new(name)
    }

    #[test]
    fn routes_follow_the_spec() {
        use DocFormat as F;
        assert_eq!(plan(p("a.csv"), F::Xlsx).unwrap(), Plan::CsvToXlsx);
        assert_eq!(plan(p("a.xlsx"), F::Csv).unwrap(), Plan::SheetToCsv);
        assert_eq!(plan(p("a.ods"), F::Csv).unwrap(), Plan::SheetToCsv);
        assert_eq!(
            plan(p("a.xls"), F::Xlsx).unwrap(),
            Plan::Engine(Family::Excel)
        );
        assert_eq!(
            plan(p("a.csv"), F::Pdf).unwrap(),
            Plan::Engine(Family::Excel)
        );
        assert_eq!(plan(p("a.docx"), F::Md).unwrap(), Plan::DocxToMarkdown);
        assert_eq!(plan(p("a.DOCX"), F::Md).unwrap(), Plan::DocxToMarkdown);
        assert_eq!(plan(p("a.odt"), F::Md).unwrap(), Plan::WordToMarkdown);
        assert_eq!(plan(p("a.rtf"), F::Md).unwrap(), Plan::WordToMarkdown);
        for sheet in ["a.xlsx", "a.xls", "a.ods", "a.csv"] {
            assert_eq!(plan(p(sheet), F::Md).unwrap(), Plan::SheetToMarkdown);
        }
        assert_eq!(
            plan(p("a.pptx"), F::Md).unwrap(),
            Plan::SlidesToMarkdown { engine: false }
        );
        assert_eq!(
            plan(p("a.odp"), F::Md).unwrap(),
            Plan::SlidesToMarkdown { engine: true }
        );
        assert!(!plan(p("a.docx"), F::Md).unwrap().needs_engine());
        assert!(!plan(p("a.xls"), F::Md).unwrap().needs_engine());
        assert!(plan(p("a.ppt"), F::Md).unwrap().needs_engine());
        assert!(plan(p("a.doc"), F::Md).unwrap().needs_engine());
        assert_eq!(
            plan(p("a.docx"), F::Pdf).unwrap(),
            Plan::Engine(Family::Word)
        );
        assert_eq!(
            plan(p("a.pptx"), F::Odp).unwrap(),
            Plan::Engine(Family::PowerPoint)
        );
        assert_eq!(
            plan(p("a.md"), F::Html).unwrap(),
            Plan::Text { engine: false }
        );
        assert_eq!(
            plan(p("a.html"), F::Md).unwrap(),
            Plan::Text { engine: false }
        );
        assert_eq!(
            plan(p("a.txt"), F::Docx).unwrap(),
            Plan::Text { engine: true }
        );
        assert!(plan(p("a.htm"), F::Pdf).unwrap().needs_engine());
        assert!(!plan(p("a.csv"), F::Xlsx).unwrap().needs_engine());
    }

    #[test]
    fn refuses_impossible_and_same_format() {
        let e = plan(p("a.xlsx"), DocFormat::Docx).unwrap_err();
        assert_eq!(e.message, "A spreadsheet can't be converted to DOCX.");
        let e = plan(p("a.docx"), DocFormat::Docx).unwrap_err();
        assert_eq!(e.message, "This file already is DOCX.");
        assert!(plan(p("a.HTM"), DocFormat::Html).is_err());
        assert!(plan(p("a.markdown"), DocFormat::Md).is_err());
        assert!(plan(p("a.jpg"), DocFormat::Pdf).is_err());
        assert_eq!(no_engine().kind, ErrorKind::MissingEngine);
    }
}
