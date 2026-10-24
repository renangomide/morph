// SPDX-License-Identifier: GPL-3.0-or-later
//! Microsoft Office exports through [`crate::office::com::run_office`].
//!
//! - Word: `Documents.Open(ReadOnly, no conversion prompt, not in recent files,
//!   a dummy password)` → `ExportAsFixedFormat` (PDF) or `SaveAs2` (DOCX, ODT,
//!   RTF, UTF-8 TXT, filtered HTML).
//! - Excel: `Workbooks.Open(ReadOnly, UpdateLinks:=0, a dummy password)` →
//!   `ExportAsFixedFormat(xlTypePDF)` for the whole workbook with its own print
//!   setup, or `SaveAs` (XLSX, ODS, CSV UTF-8).
//! - PowerPoint: `Presentations.Open("path::password::", ReadOnly, WithWindow:=False)`
//!   → `SaveAs` (PDF, PPTX, ODP), or `Slide.Export` per slide for images.
//!
//! The dummy password makes Office fail on a protected file instead of showing
//! a password prompt nobody can see (an unprotected file ignores it). Word and
//! Excel open a copy staged in the temp folder, so no `~$` owner file appears
//! next to the user's file; HTML inputs open in place (relative pictures).

use crate::converter::TaskCtx;
use crate::error::ConvertError;
use crate::office::com::{self, ComError, Dispatch, OfficeSession, Variant};
use crate::office::detect::{AppInfo, OfficeApp};
use crate::office::options::{DocFormat, DocImageFormat, Family};
use std::path::{Path, PathBuf};

/// Given to every Open call: a protected file then fails instead of prompting.
const DUMMY_PASSWORD: &str = "morph-no-password";

// Word WdSaveFormat / WdExportFormat.
const WD_EXPORT_PDF: i32 = 17;
const WD_DOCX: i32 = 16;
const WD_ODT: i32 = 23;
const WD_RTF: i32 = 6;
const WD_TXT_ENCODED: i32 = 7;
const WD_FILTERED_HTML: i32 = 10;
const CP_UTF8: i32 = 65001;
// Excel XlFileFormat / XlFixedFormatType.
const XL_TYPE_PDF: i32 = 0;
const XL_XLSX: i32 = 51;
const XL_ODS: i32 = 60;
const XL_CSV_UTF8: i32 = 62;
// PowerPoint PpSaveAsFileType, MsoTriState.
const PP_PDF: i32 = 32;
const PP_PPTX: i32 = 24;
const PP_ODP: i32 = 35;
const MSO_TRUE: i32 = -1;
const MSO_FALSE: i32 = 0;

/// The Office "SaveAs" code for a target, by app. `None`: the app can't write it.
pub fn save_format(app: OfficeApp, format: DocFormat) -> Option<i32> {
    use DocFormat as F;
    Some(match (app, format) {
        (OfficeApp::Word, F::Pdf) => WD_EXPORT_PDF,
        (OfficeApp::Word, F::Docx) => WD_DOCX,
        (OfficeApp::Word, F::Odt) => WD_ODT,
        (OfficeApp::Word, F::Rtf) => WD_RTF,
        (OfficeApp::Word, F::Txt) => WD_TXT_ENCODED,
        (OfficeApp::Word, F::Html) => WD_FILTERED_HTML,
        (OfficeApp::Excel, F::Pdf) => XL_TYPE_PDF,
        (OfficeApp::Excel, F::Xlsx) => XL_XLSX,
        (OfficeApp::Excel, F::Ods) => XL_ODS,
        (OfficeApp::Excel, F::Csv) => XL_CSV_UTF8,
        (OfficeApp::PowerPoint, F::Pdf) => PP_PDF,
        (OfficeApp::PowerPoint, F::Pptx) => PP_PPTX,
        (OfficeApp::PowerPoint, F::Odp) => PP_ODP,
        _ => return None,
    })
}

/// A fresh work folder in the task's temp dir: `(in\, out\)`.
fn work_dirs(ctx: &TaskCtx<'_>) -> Result<(PathBuf, PathBuf), ConvertError> {
    let work = ctx
        .temp_dir()?
        .join(format!("ms-{}", uuid::Uuid::new_v4().simple()));
    let (i, o) = (work.join("in"), work.join("out"));
    std::fs::create_dir_all(&i)
        .and_then(|()| std::fs::create_dir_all(&o))
        .map_err(|e| ConvertError::other("Couldn't create a temporary folder.", e))?;
    Ok((i, o))
}

/// Copy the input into `dir` (same file name), except HTML (relative pictures).
fn stage(input: &Path, dir: &Path) -> Result<PathBuf, ConvertError> {
    if DocFormat::of_path(input) == Some(DocFormat::Html) {
        return Ok(input.to_path_buf());
    }
    let name = input
        .file_name()
        .map_or_else(|| "input".into(), |n| n.to_owned());
    let staged = dir.join(name);
    std::fs::copy(input, &staged).map_err(ConvertError::from)?;
    Ok(staged)
}

/// Export `input` with the Office app in `app`; returns the file in the temp folder.
pub fn export(
    app: &AppInfo,
    input: &Path,
    family: Family,
    format: DocFormat,
    ctx: &TaskCtx<'_>,
) -> Result<PathBuf, ConvertError> {
    let code = save_format(app.app, format).ok_or_else(|| {
        ConvertError::unsupported(
            format!(
                "A {} can't be converted to {}.",
                family.noun(),
                format.label()
            ),
            format!("{} → {format:?}", app.app.display_name()),
        )
    })?;
    let (in_dir, out_dir) = work_dirs(ctx)?;
    let source = stage(input, &in_dir)?;
    let out = out_dir.join(format!("output.{}", format.extension()));
    let target = out.clone();
    let which = app.app;
    com::run_office(app, ctx, move |s| {
        match which {
            OfficeApp::Word => word_save(s, &source, &target, format, code),
            OfficeApp::Excel => excel_save(s, &source, &target, format, code),
            OfficeApp::PowerPoint => powerpoint_save(s, &source, &target, code),
        }?;
        Ok(())
    })?;
    if !out.is_file() {
        return Err(ConvertError::other(
            format!("{} couldn't convert this file.", app.app.display_name()),
            format!("no output written: {}", out.display()),
        ));
    }
    Ok(out)
}

/// Slides → one picture each (`Slide.Export`), `dpi` relative to the slide's
/// size in points. Returns the files in slide order (in the temp folder).
pub fn export_slides(
    app: &AppInfo,
    input: &Path,
    format: DocImageFormat,
    dpi: u32,
    ctx: &TaskCtx<'_>,
) -> Result<Vec<PathBuf>, ConvertError> {
    let (in_dir, out_dir) = work_dirs(ctx)?;
    let source = stage(input, &in_dir)?;
    com::run_office(app, ctx, move |s| {
        let pres = open_presentation(s, &source)?;
        let setup = pres.object("PageSetup")?;
        let points = |name: &str| -> Result<f64, ComError> {
            Ok(f64::from(setup.get(name)?.to_i32().unwrap_or(0)))
        };
        let scale = f64::from(dpi) / 72.0;
        let width = (points("SlideWidth")? * scale).round().max(1.0) as i32;
        let height = (points("SlideHeight")? * scale).round().max(1.0) as i32;
        let slides = pres.object("Slides")?;
        let count = slides.get("Count")?.to_i32().unwrap_or(0).max(0);
        let filter = match format {
            DocImageFormat::Png => "PNG",
            DocImageFormat::Jpg => "JPG",
        };
        let mut files = Vec::with_capacity(count as usize);
        for i in 1..=count {
            let slide = slides
                .get_with("Item", vec![Variant::i32(i)])?
                .as_dispatch()
                .ok_or_else(|| com_missing("Slides.Item"))?;
            let file = out_dir.join(format!("slide-{i:04}.{}", format.extension()));
            slide.call(
                "Export",
                vec![
                    Variant::path(&file),
                    Variant::str(filter),
                    Variant::i32(width),
                    Variant::i32(height),
                ],
            )?;
            files.push(file);
        }
        Ok(files)
    })
}

fn com_missing(member: &str) -> ComError {
    ComError {
        member: member.to_owned(),
        code: 0,
        description: "no object returned".into(),
    }
}

/// An `Open` that failed: password → "This file is password-protected.",
/// anything else → "This file seems to be damaged…", with Office's message.
pub fn open_error(e: ComError) -> ConvertError {
    // 0x800A1520: Word "The password is incorrect".
    let password =
        e.description.to_lowercase().contains("password") || e.code as u32 == 0x800A_1520;
    if password {
        let mut err = crate::office::password_protected();
        err.details = e.to_string();
        err
    } else {
        crate::office::damaged_document(e)
    }
}

fn opened(v: Result<Variant, ComError>, member: &str) -> Result<Dispatch, ConvertError> {
    v.map_err(open_error)?
        .as_dispatch()
        .ok_or_else(|| open_error(com_missing(member)))
}

fn word_save(
    s: &OfficeSession,
    source: &Path,
    out: &Path,
    format: DocFormat,
    code: i32,
) -> Result<(), ConvertError> {
    let docs = s.app.object("Documents")?;
    let doc = opened(
        docs.call_named(
            "Open",
            vec![
                ("FileName", Variant::path(source)),
                ("ConfirmConversions", Variant::bool(false)),
                ("ReadOnly", Variant::bool(true)),
                ("AddToRecentFiles", Variant::bool(false)),
                ("PasswordDocument", Variant::str(DUMMY_PASSWORD)),
                ("Revert", Variant::bool(false)),
                ("Visible", Variant::bool(false)),
                ("NoEncodingDialog", Variant::bool(true)),
            ],
        ),
        "Documents.Open",
    )?;
    s.track(&doc);
    if format == DocFormat::Pdf {
        doc.call_named(
            "ExportAsFixedFormat",
            vec![
                ("OutputFileName", Variant::path(out)),
                ("ExportFormat", Variant::i32(code)),
                ("OpenAfterExport", Variant::bool(false)),
            ],
        )?;
    } else {
        let mut args = vec![
            ("FileName", Variant::path(out)),
            ("FileFormat", Variant::i32(code)),
            ("AddToRecentFiles", Variant::bool(false)),
        ];
        if matches!(format, DocFormat::Txt | DocFormat::Html) {
            args.push(("Encoding", Variant::i32(CP_UTF8)));
        }
        doc.call_named("SaveAs2", args)?;
    }
    Ok(())
}

fn excel_save(
    s: &OfficeSession,
    source: &Path,
    out: &Path,
    format: DocFormat,
    code: i32,
) -> Result<(), ConvertError> {
    let books = s.app.object("Workbooks")?;
    let csv = DocFormat::of_path(source) == Some(DocFormat::Csv);
    let mut args = vec![
        ("Filename", Variant::path(source)),
        ("UpdateLinks", Variant::i32(0)),
        ("ReadOnly", Variant::bool(true)),
        ("Password", Variant::str(DUMMY_PASSWORD)),
        ("IgnoreReadOnlyRecommended", Variant::bool(true)),
        ("Notify", Variant::bool(false)),
        ("AddToMru", Variant::bool(false)),
    ];
    if csv {
        // Read the CSV with the user's list separator and number format.
        args.push(("Local", Variant::bool(true)));
    }
    let book = opened(books.call_named("Open", args), "Workbooks.Open")?;
    s.track(&book);
    if format == DocFormat::Pdf {
        // The whole workbook with its own print setup (SPEC §4.5).
        book.call_named(
            "ExportAsFixedFormat",
            vec![
                ("Type", Variant::i32(code)),
                ("Filename", Variant::path(out)),
                ("IgnorePrintAreas", Variant::bool(false)),
                ("OpenAfterPublish", Variant::bool(false)),
            ],
        )?;
    } else {
        book.call_named(
            "SaveAs",
            vec![
                ("Filename", Variant::path(out)),
                ("FileFormat", Variant::i32(code)),
                ("AddToMru", Variant::bool(false)),
            ],
        )?;
    }
    Ok(())
}

fn open_presentation(s: &OfficeSession, source: &Path) -> Result<Dispatch, ConvertError> {
    let pres = s.app.object("Presentations")?;
    // `FileName::password::` opens a protected file (or fails); PowerPoint has no
    // password argument, and without one it would show a prompt.
    let name = format!("{}::{DUMMY_PASSWORD}::", source.display());
    let doc = opened(
        pres.call(
            "Open",
            vec![
                Variant::str(&name),
                Variant::i32(MSO_TRUE),  // ReadOnly
                Variant::i32(MSO_FALSE), // Untitled
                Variant::i32(MSO_FALSE), // WithWindow
            ],
        ),
        "Presentations.Open",
    )?;
    s.track(&doc);
    Ok(doc)
}

fn powerpoint_save(
    s: &OfficeSession,
    source: &Path,
    out: &Path,
    code: i32,
) -> Result<(), ConvertError> {
    let doc = open_presentation(s, source)?;
    doc.call("SaveAs", vec![Variant::path(out), Variant::i32(code)])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CancelToken;
    use crate::error::ErrorKind;
    use crate::office::detect;

    #[test]
    fn office_format_codes() {
        for family in [Family::Word, Family::Excel, Family::PowerPoint] {
            let app = OfficeApp::for_family(family);
            for &f in family.outputs() {
                // Markdown is HTML + htmd; CSV is native, but Excel can do it.
                if f != DocFormat::Md {
                    assert!(save_format(app, f).is_some(), "{app:?} {f:?}");
                }
            }
        }
        assert_eq!(save_format(OfficeApp::Word, DocFormat::Pdf), Some(17));
        assert_eq!(save_format(OfficeApp::Excel, DocFormat::Xlsx), Some(51));
        assert_eq!(save_format(OfficeApp::PowerPoint, DocFormat::Pdf), Some(32));
        assert_eq!(save_format(OfficeApp::Excel, DocFormat::Docx), None);
    }

    #[test]
    fn open_errors_are_plain() {
        let e = open_error(ComError {
            member: "Documents.Open".into(),
            code: 0x800A1520u32 as i32,
            description: "The password is incorrect. Word cannot open the document.".into(),
        });
        assert_eq!(e.message, "This file is password-protected.");
        assert!(e.details.contains("Documents.Open"));
        let e = open_error(ComError {
            member: "Workbooks.Open".into(),
            code: 0x800A03ECu32 as i32,
            description: "The file format or file extension is not valid.".into(),
        });
        assert_eq!(e.kind, ErrorKind::BadInput);
        assert!(e.message.contains("damaged"));
    }

    fn fixture(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures")
            .join(name)
    }

    fn run(app: Option<AppInfo>, name: &str, family: Family, format: DocFormat) {
        let Some(app) = app else {
            println!("SKIPPED: Microsoft Office app not detected");
            return;
        };
        let cancel = CancelToken::new();
        let temp = tempfile::tempdir().unwrap();
        let ctx = TaskCtx::new(&cancel, &|_, _| {}, temp.path());
        let input = fixture(name);
        let before = std::fs::read(&input).unwrap();
        let out = export(&app, &input, family, format, &ctx).unwrap();
        assert!(out.starts_with(temp.path()) && out.is_file());
        assert_eq!(std::fs::read(&input).unwrap(), before);
    }

    #[test]
    #[ignore = "needs Microsoft Word (not installed on the dev machine); run with --ignored"]
    fn word_exports() {
        for f in [
            DocFormat::Pdf,
            DocFormat::Odt,
            DocFormat::Rtf,
            DocFormat::Txt,
            DocFormat::Html,
        ] {
            run(
                detect::engines().word.clone(),
                "sample.docx",
                Family::Word,
                f,
            );
        }
    }

    #[test]
    #[ignore = "needs Microsoft Excel (not installed on the dev machine); run with --ignored"]
    fn excel_exports() {
        for f in [DocFormat::Pdf, DocFormat::Ods] {
            run(
                detect::engines().excel.clone(),
                "sample.xlsx",
                Family::Excel,
                f,
            );
        }
    }

    #[test]
    #[ignore = "needs Microsoft PowerPoint (not installed on the dev machine); run with --ignored"]
    fn powerpoint_exports() {
        let engines = detect::engines();
        for f in [DocFormat::Pdf, DocFormat::Odp] {
            run(
                engines.powerpoint.clone(),
                "sample.pptx",
                Family::PowerPoint,
                f,
            );
        }
        let Some(app) = engines.powerpoint.clone() else {
            return;
        };
        let cancel = CancelToken::new();
        let temp = tempfile::tempdir().unwrap();
        let ctx = TaskCtx::new(&cancel, &|_, _| {}, temp.path());
        let files =
            export_slides(&app, &fixture("sample.pptx"), DocImageFormat::Png, 96, &ctx).unwrap();
        assert_eq!(files.len(), 3);
    }
}
