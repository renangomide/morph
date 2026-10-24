// SPDX-License-Identifier: GPL-3.0-or-later
//! PDF → Word (SPEC §4.2): `name.docx` next to the input (or in the
//! chosen folder), published with [`naming::publish_file`].
//!
//! 1. **Check with PDFium first**: a password-protected PDF fails with
//!    "This PDF is password-protected.", a damaged one with the plain "damaged"
//!    message, before any engine starts. PDFium also tells us whether the PDF has
//!    a text layer: when no page has text, the conversion still runs and the
//!    result row gets [`SCANNED_NOTE`].
//! 2. **Microsoft Word** when installed (and allowed by Settings → Office engine):
//!    Word's own PDF reflow, through [`com::run_office`]: `Documents.Open` on a
//!    staged copy of the PDF with `ConfirmConversions := False`, `ReadOnly := True`,
//!    `AddToRecentFiles := False`, `Visible := False` (alerts are off, so Word's
//!    "will now convert your PDF" prompt doesn't appear), then
//!    `SaveAs2(FileFormat := wdFormatDocumentDefault)` into the temp folder.
//! 3. Otherwise **LibreOffice**: `--infilter=writer_pdf_import --convert-to
//!    "docx:MS Word 2007 XML"` through [`soffice::convert`].
//!
//! Why `writer_pdf_import`: LibreOffice's default PDF import opens the file in
//! **Draw**, and Draw has no Word export, so `--convert-to docx` fails with an I/O
//! write error (checked with LibreOffice 26.8 on `text-3pages.pdf`; it only
//! produces ODG/PDF/images). `writer_pdf_import` opens the PDF in **Writer**:
//! every text run becomes an editable text box positioned like on the page, with
//! its font, size and colour; pictures stay pictures. Not flowing paragraphs
//! like Word's reflow, but the text is real, editable and searchable, which is
//! what users expect from "To Word". It is also fast (~1.5 s for 3 pages).
//!
//! The window shows [`LAYOUT_WARNING`] next to the "Word" choice.

use crate::action::Action;
use crate::converter::{Converter, TaskCtx, TaskOutput};
use crate::error::ConvertError;
use crate::job::Task;
use crate::naming::{self, Suffix};
use crate::office::com::{self, ComError, Variant};
use crate::office::detect::{self, AppInfo, EngineChoice, LibreOfficeInfo};
use crate::office::soffice::{self, SofficeJob};
use crate::office::{self, ENGINE_TIMEOUT};
use crate::pdf::pdfium;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// SPEC §4.2 warning shown in the window when "Word" is chosen.
pub const LAYOUT_WARNING: &str = "Layout may change; works best on text-based PDFs.";

/// Result note for a PDF without any text layer.
pub const SCANNED_NOTE: &str =
    "This PDF looks scanned; the Word file will contain images, not editable text.";

/// LibreOffice: open the PDF in Writer (see module docs), save as DOCX.
pub const LIBREOFFICE_INFILTER: &str = "writer_pdf_import";
pub const LIBREOFFICE_TARGET: &str = "docx:MS Word 2007 XML";

/// `WdSaveFormat.wdFormatDocumentDefault` (DOCX).
const WD_FORMAT_DOCUMENT_DEFAULT: i32 = 16;

pub struct PdfToWord;

impl Converter for PdfToWord {
    fn run(&self, task: &Task, ctx: &TaskCtx<'_>) -> Result<TaskOutput, ConvertError> {
        let Action::PdfToWord(_opts) = &task.action else {
            return Err(super::wrong_action("PdfToWord"));
        };
        let input = task
            .input()
            .ok_or_else(|| super::wrong_action("PdfToWord"))?;
        ctx.progress(0.0, None);
        let scanned = inspect(input, ctx)?;
        let engine = detect::engines()
            .resolve_pdf_to_word(ctx.office_engine())
            .ok_or_else(office::no_engine)?;
        ctx.progress(0.05, None);
        let docx = export(input, &engine, ENGINE_TIMEOUT, ctx)?;
        ctx.check_cancel()?;
        let name = naming::output_name(input, "docx", Suffix::Converted);
        let path = naming::publish_file(&docx, &task.output.folder_for(input), &name)?;
        ctx.progress(1.0, None);
        let out = TaskOutput::single(path);
        Ok(if scanned {
            out.with_note(SCANNED_NOTE)
        } else {
            out
        })
    }
}

/// Open the PDF with PDFium (plain errors for password-protected and damaged
/// files) and tell whether it looks scanned (no page has any text).
pub fn inspect(input: &Path, ctx: &TaskCtx<'_>) -> Result<bool, ConvertError> {
    let doc = pdfium::open(input)?;
    for i in 0..pdfium::page_count(&doc) {
        ctx.check_cancel()?;
        let page = pdfium::page(&doc, i)?;
        if has_text(&pdfium::page_text(&page)?) {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Any visible character (PDFium may return only whitespace or U+FFFE markers).
fn has_text(text: &str) -> bool {
    text.chars()
        .any(|c| !c.is_whitespace() && !c.is_control() && c != '\u{FFFE}' && c != '\u{FFFF}')
}

/// Convert `input` to DOCX with `engine`, within `timeout`. Returns the file in
/// the task's temp folder; the caller publishes it. Never writes next to the input.
pub fn export(
    input: &Path,
    engine: &EngineChoice,
    timeout: Duration,
    ctx: &TaskCtx<'_>,
) -> Result<PathBuf, ConvertError> {
    match engine {
        EngineChoice::MsOffice(app) => export_word(app, input, timeout, ctx),
        EngineChoice::LibreOffice(lo) => export_libreoffice(lo, input, timeout, ctx),
    }
}

fn export_libreoffice(
    lo: &LibreOfficeInfo,
    input: &Path,
    timeout: Duration,
    ctx: &TaskCtx<'_>,
) -> Result<PathBuf, ConvertError> {
    let mut job = SofficeJob::new(input, LIBREOFFICE_TARGET);
    job.infilter = Some(LIBREOFFICE_INFILTER);
    job.timeout = timeout;
    let out = soffice::convert(lo, &job, ctx)?;
    out.files
        .iter()
        .find(|f| {
            f.extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("docx"))
        })
        .cloned()
        .ok_or_else(|| soffice::convert_failed(&out.log))
}

fn export_word(
    app: &AppInfo,
    input: &Path,
    timeout: Duration,
    ctx: &TaskCtx<'_>,
) -> Result<PathBuf, ConvertError> {
    // Word only reads a private copy: no lock files or recent-file traces next
    // to the user's PDF, and odd names / long paths can't trip it.
    let work = ctx
        .temp_dir()?
        .join(format!("word-{}", uuid::Uuid::new_v4().simple()));
    std::fs::create_dir_all(&work)
        .map_err(|e| ConvertError::other("Couldn't create a temporary folder.", e))?;
    let staged = work.join("input.pdf");
    std::fs::copy(input, &staged).map_err(ConvertError::from)?;
    let target = work.join("output.docx");

    let save_to = target.clone();
    com::run_office_with_timeout(app, ctx, timeout, move |session| {
        let docs = session.app.object("Documents").map_err(word_failed)?;
        let opened = docs
            .call_named("Open", word_open_args(&staged))
            .map_err(word_failed)?;
        let doc = opened.as_dispatch().ok_or_else(|| {
            ConvertError::other(
                "Microsoft Word couldn't open this PDF.",
                "Documents.Open returned no document",
            )
        })?;
        session.track(&doc);
        doc.call_named(
            "SaveAs2",
            vec![
                ("FileName", Variant::path(&save_to)),
                ("FileFormat", Variant::i32(WD_FORMAT_DOCUMENT_DEFAULT)),
                ("AddToRecentFiles", Variant::bool(false)),
            ],
        )
        .map_err(word_failed)?;
        Ok(())
    })?;
    if target.is_file() {
        Ok(target)
    } else {
        Err(ConvertError::other(
            "Microsoft Word couldn't convert this PDF.",
            "SaveAs2 wrote no file",
        ))
    }
}

/// Named arguments for `Documents.Open` on a PDF (Word's PDF reflow).
fn word_open_args(pdf: &Path) -> Vec<(&'static str, Variant)> {
    vec![
        ("FileName", Variant::path(pdf)),
        ("ConfirmConversions", Variant::bool(false)),
        ("ReadOnly", Variant::bool(true)),
        ("AddToRecentFiles", Variant::bool(false)),
        ("Visible", Variant::bool(false)),
    ]
}

fn word_failed(e: ComError) -> ConvertError {
    ConvertError::other("Microsoft Word couldn't convert this PDF.", e)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_detection_ignores_blanks_and_markers() {
        assert!(!has_text(""));
        assert!(!has_text(" \r\n\t\u{FFFE}\u{0}"));
        assert!(has_text("\r\nc"));
        assert!(has_text("報"));
    }

    #[test]
    fn word_opens_pdf_without_prompts_read_only() {
        let names: Vec<&str> = word_open_args(Path::new(r"C:\t\input.pdf"))
            .iter()
            .map(|(n, _)| *n)
            .collect();
        assert_eq!(
            names,
            [
                "FileName",
                "ConfirmConversions",
                "ReadOnly",
                "AddToRecentFiles",
                "Visible"
            ]
        );
        let args = word_open_args(Path::new("x.pdf"));
        assert_eq!(args[1].1.to_bool(), Some(false));
        assert_eq!(args[2].1.to_bool(), Some(true));
        assert_eq!(WD_FORMAT_DOCUMENT_DEFAULT, 16);
    }

    #[test]
    fn strings_follow_the_spec() {
        assert_eq!(
            LAYOUT_WARNING,
            "Layout may change; works best on text-based PDFs."
        );
        assert!(SCANNED_NOTE.starts_with("This PDF looks scanned"));
    }
}
