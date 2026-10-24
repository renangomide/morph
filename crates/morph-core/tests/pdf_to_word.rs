// SPDX-License-Identifier: GPL-3.0-or-later
//! PDF → Word (SPEC §4.2).
//!
//! LibreOffice tests run for real when LibreOffice is detected and print
//! "SKIPPED: …" otherwise. Microsoft Word tests are `#[ignore]`d (run them with
//! `--ignored` on a machine with Word).

use morph_core::action::Action;
use morph_core::converter::TaskCtx;
use morph_core::error::ErrorKind;
use morph_core::event::Event;
use morph_core::office::detect::{self, EngineChoice, EnginePreference};
use morph_core::pdf::options::PdfToWordOpts;
use morph_core::pdf::to_word::{self, SCANNED_NOTE};
use morph_core::scheduler::{JobConfig, run_job_blocking};
use morph_core::{CancelToken, Job, OutputSpec};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures")
        .join(name)
}

fn libreoffice() -> Option<EngineChoice> {
    let lo = detect::engines().libreoffice.clone();
    if lo.is_none() {
        println!("SKIPPED: LibreOffice not installed");
    }
    lo.map(EngineChoice::LibreOffice)
}

/// `word/document.xml` of a DOCX (panics if it isn't one).
fn document_xml(docx: &Path) -> String {
    let mut z = zip::ZipArchive::new(std::fs::File::open(docx).unwrap()).unwrap();
    let mut xml = String::new();
    z.by_name("word/document.xml")
        .unwrap()
        .read_to_string(&mut xml)
        .unwrap();
    xml
}

fn has_media(docx: &Path) -> bool {
    let z = zip::ZipArchive::new(std::fs::File::open(docx).unwrap()).unwrap();
    z.file_names()
        .any(|n| n.is_ok_and(|n| n.starts_with("word/media/")))
}

struct Run {
    events: Vec<Event>,
    done: usize,
    failed: usize,
}

fn run_job(files: &[PathBuf], out: &Path, pref: EnginePreference) -> Run {
    let action = Action::PdfToWord(PdfToWordOpts::default());
    let job = Job::for_files(
        files,
        &action,
        &OutputSpec {
            folder: Some(out.to_path_buf()),
        },
    );
    let (tx, rx) = crossbeam_channel::unbounded();
    let config = JobConfig {
        temp_root: tempfile::tempdir().unwrap().keep(),
        office_engine: pref,
        ..JobConfig::default()
    };
    let summary = run_job_blocking(job, &config, &CancelToken::new(), &tx);
    drop(tx);
    Run {
        events: rx.iter().collect(),
        done: summary.done,
        failed: summary.failed,
    }
}

fn errors(events: &[Event]) -> Vec<String> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::TaskFailed { error, .. } => Some(error.clone()),
            _ => None,
        })
        .collect()
}

fn notes(events: &[Event]) -> Vec<String> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::TaskDone { note, .. } => note.clone(),
            _ => None,
        })
        .collect()
}

// ---------------------------------------------------------------- no engine needed

#[test]
fn password_and_damaged_pdfs_fail_before_any_engine() {
    let cancel = CancelToken::new();
    let temp = tempfile::tempdir().unwrap();
    let ctx = TaskCtx::new(&cancel, &|_, _| {}, temp.path());
    let e = to_word::inspect(&fixture("password.pdf"), &ctx).unwrap_err();
    assert_eq!(e.message, "This PDF is password-protected.");
    assert_eq!(e.kind, ErrorKind::BadInput);
    let e = to_word::inspect(&fixture("broken.pdf"), &ctx).unwrap_err();
    assert!(e.message.contains("damaged"), "{e}");
    assert!(!to_word::inspect(&fixture("text-3pages.pdf"), &ctx).unwrap());
    assert!(to_word::inspect(&fixture("scanned.pdf"), &ctx).unwrap());
}

// ---------------------------------------------------------------- LibreOffice

#[test]
fn libreoffice_text_pdf_becomes_editable_docx() {
    let Some(engine) = libreoffice() else { return };
    let cancel = CancelToken::new();
    let temp = tempfile::tempdir().unwrap();
    let ctx = TaskCtx::new(&cancel, &|_, _| {}, temp.path());
    let input = fixture("text-3pages.pdf");
    let before = std::fs::read(&input).unwrap();
    let start = Instant::now();
    let docx = to_word::export(&input, &engine, Duration::from_secs(180), &ctx).unwrap();
    println!(
        "text-3pages.pdf → DOCX (LibreOffice) in {:?}",
        start.elapsed()
    );
    assert!(docx.starts_with(temp.path()), "engine output stays in temp");
    let xml = document_xml(&docx);
    for words in [
        "Text - Page 1 of 3",
        "Text - Page 3 of 3",
        "café, relatório, ação",
        "The quick brown fox",
    ] {
        assert!(xml.contains(words), "missing {words:?}");
    }
    assert_eq!(std::fs::read(&input).unwrap(), before, "input changed");
}

#[test]
fn libreoffice_job_publishes_docx_with_notes_and_errors() {
    let Some(_engine) = libreoffice() else { return };
    let out = tempfile::tempdir().unwrap();
    let files = vec![
        fixture("text-3pages.pdf"),
        fixture("Relatório 報告.pdf"),
        fixture("scanned.pdf"),
        fixture("password.pdf"),
        fixture("broken.pdf"),
        fixture("text-3pages.pdf"), // twice: "text-3pages (1).docx"
    ];
    let befores: Vec<Vec<u8>> = files.iter().map(|f| std::fs::read(f).unwrap()).collect();
    let start = Instant::now();
    let run = run_job(&files, out.path(), EnginePreference::LibreOffice);
    println!("6 PDFs → DOCX job in {:?}", start.elapsed());
    assert_eq!(run.done, 4, "{:#?}", run.events);
    assert_eq!(run.failed, 2);

    for name in ["text-3pages.docx", "text-3pages (1).docx"] {
        assert!(
            document_xml(&out.path().join(name)).contains("café"),
            "{name}"
        );
    }
    document_xml(&out.path().join("Relatório 報告.docx"));
    let scanned = out.path().join("scanned.docx");
    document_xml(&scanned);
    assert!(has_media(&scanned), "scanned pages stay pictures");
    assert_eq!(notes(&run.events), vec![SCANNED_NOTE.to_owned()]);

    let errs = errors(&run.events);
    assert!(
        errs.contains(&"This PDF is password-protected.".to_owned()),
        "{errs:?}"
    );
    assert!(errs.iter().any(|e| e.contains("damaged")), "{errs:?}");

    let names: Vec<String> = std::fs::read_dir(out.path())
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names.len(), 4, "{names:?}");
    assert!(!names.iter().any(|n| n.starts_with(".~morph")), "temp left");
    for (f, before) in files.iter().zip(&befores) {
        assert_eq!(&std::fs::read(f).unwrap(), before, "input changed");
    }
}

#[test]
fn libreoffice_timeout_is_reported_plainly() {
    let Some(engine) = libreoffice() else { return };
    let cancel = CancelToken::new();
    let temp = tempfile::tempdir().unwrap();
    let ctx = TaskCtx::new(&cancel, &|_, _| {}, temp.path());
    let e = to_word::export(
        &fixture("pages-100.pdf"),
        &engine,
        Duration::from_millis(300),
        &ctx,
    )
    .unwrap_err();
    assert_eq!(e.message, "LibreOffice took too long and was stopped.");
    // From LibreOffice's start, kill included; not the wait for a free profile slot.
    let ran: f64 = e
        .details
        .split_once("stopped after ")
        .and_then(|(_, rest)| rest.split_once('s'))
        .and_then(|(secs, _)| secs.parse().ok())
        .unwrap_or_else(|| panic!("no run time in details: {e}"));
    assert!(ran < 10.0, "killed promptly: {}", e.details);
}

// ---------------------------------------------------------------- Microsoft Word

fn word() -> Option<EngineChoice> {
    let w = detect::engines().word.clone();
    if w.is_none() {
        println!("SKIPPED: Microsoft Word not installed");
    }
    w.map(EngineChoice::MsOffice)
}

#[test]
#[ignore = "needs Microsoft Word (PDF reflow through COM)"]
fn word_text_pdf_becomes_docx() {
    let Some(engine) = word() else { return };
    let cancel = CancelToken::new();
    let temp = tempfile::tempdir().unwrap();
    let ctx = TaskCtx::new(&cancel, &|_, _| {}, temp.path());
    let input = fixture("text-3pages.pdf");
    let before = std::fs::read(&input).unwrap();
    let start = Instant::now();
    let docx = to_word::export(&input, &engine, Duration::from_secs(180), &ctx).unwrap();
    println!("PDF → DOCX (Word) in {:?}", start.elapsed());
    assert!(document_xml(&docx).contains("café"));
    assert_eq!(std::fs::read(&input).unwrap(), before, "input changed");

    let out = tempfile::tempdir().unwrap();
    let run = run_job(
        &[fixture("Relatório 報告.pdf"), fixture("password.pdf")],
        out.path(),
        EnginePreference::MicrosoftOffice,
    );
    assert_eq!((run.done, run.failed), (1, 1), "{:#?}", run.events);
    document_xml(&out.path().join("Relatório 報告.docx"));
}

#[test]
#[ignore = "needs Microsoft Word (COM watchdog kills our instance)"]
fn word_timeout_is_reported_plainly() {
    let Some(engine) = word() else { return };
    let cancel = CancelToken::new();
    let temp = tempfile::tempdir().unwrap();
    let ctx = TaskCtx::new(&cancel, &|_, _| {}, temp.path());
    let e = to_word::export(
        &fixture("pages-100.pdf"),
        &engine,
        Duration::from_millis(500),
        &ctx,
    )
    .unwrap_err();
    assert_eq!(e.message, "Microsoft Word took too long and was stopped.");
}
