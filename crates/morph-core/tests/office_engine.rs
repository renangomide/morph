// SPDX-License-Identifier: GPL-3.0-or-later
//! Office foundation: fixtures, engine detection, the LibreOffice runner end to end
//! and the `office.convert` router.
//!
//! LibreOffice tests run for real when LibreOffice is detected and print
//! "SKIPPED: …" otherwise. Microsoft Office tests are `#[ignore]`d (run them
//! with `--ignored` on a machine with Office).

use morph_core::action::Action;
use morph_core::converter::TaskCtx;
use morph_core::error::ErrorKind;
use morph_core::event::Event;
use morph_core::office::detect::{self, EngineChoice, LibreOfficeInfo};
use morph_core::office::options::{DocFormat, Family};
use morph_core::office::soffice::{self, SofficeJob};
use morph_core::office::{OfficeConvertOpts, engine_convert};
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

fn libreoffice() -> Option<LibreOfficeInfo> {
    let lo = detect::engines().libreoffice.clone();
    if lo.is_none() {
        println!("SKIPPED: LibreOffice not installed");
    }
    lo
}

fn page_count(pdf: &Path) -> u32 {
    let doc = morph_core::pdf::pdfium::open(pdf).unwrap();
    morph_core::pdf::pdfium::page_count(&doc)
}

fn zip_names(path: &Path) -> Vec<String> {
    let file = std::fs::File::open(path).unwrap();
    let mut z = zip::ZipArchive::new(file).unwrap();
    (0..z.len())
        .map(|i| z.by_index(i).unwrap().name().unwrap().into_owned())
        .collect()
}

// ---------------------------------------------------------------- fixtures (no engine)

#[test]
fn office_fixtures_are_well_formed() {
    let docx = zip_names(&fixture("sample.docx"));
    for part in [
        "word/document.xml",
        "word/styles.xml",
        "word/numbering.xml",
        "word/media/image1.png",
    ] {
        assert!(docx.iter().any(|n| n == part), "docx misses {part}");
    }
    let pptx = zip_names(&fixture("sample.pptx"));
    assert_eq!(
        pptx.iter()
            .filter(|n| n.starts_with("ppt/slides/slide"))
            .count(),
        3
    );
    let odt = zip_names(&fixture("sample.odt"));
    assert_eq!(odt.first().map(String::as_str), Some("mimetype"));
    assert_eq!(
        std::fs::read(fixture("sample.docx")).unwrap(),
        std::fs::read(fixture("Relatório 報告.docx")).unwrap()
    );

    use calamine::{Reader, open_workbook_auto};
    let mut wb = open_workbook_auto(fixture("sample.xlsx")).unwrap();
    assert_eq!(
        wb.sheet_names(),
        vec!["Sales".to_owned(), "Notes".to_owned()]
    );
    let sales = wb.worksheet_range("Sales").unwrap();
    assert_eq!(sales.get_size(), (5, 4));

    let bom = std::fs::read(fixture("semicolon-bom.csv")).unwrap();
    assert!(bom.starts_with(&[0xEF, 0xBB, 0xBF]));
    assert!(String::from_utf8_lossy(&bom).contains("1234,5"));
    let csv = std::fs::read_to_string(fixture("sample.csv")).unwrap();
    assert!(csv.starts_with("Name,City,Amount"));
    let md = std::fs::read_to_string(fixture("sample.md")).unwrap();
    assert!(md.contains("| Item |") && md.contains("```rust") && md.contains("(sample.png)"));
    let html = std::fs::read_to_string(fixture("sample.html")).unwrap();
    assert!(html.contains("<table") && html.contains("src=\"sample.png\""));
    let txt = std::fs::read(fixture("sample.txt")).unwrap();
    assert!(txt.windows(2).any(|w| w == b"\r\n"), "CRLF text");
    assert!(fixture("sample.png").is_file(), "md/html reference it");
    let mut broken = Vec::new();
    std::fs::File::open(fixture("broken.docx"))
        .unwrap()
        .read_to_end(&mut broken)
        .unwrap();
    assert!(zip::ZipArchive::new(std::io::Cursor::new(broken)).is_err());
}

// ---------------------------------------------------------------- detection

#[test]
fn detection_reports_what_it_found() {
    let e = detect::refresh();
    println!(
        "Word: {:?}\nExcel: {:?}\nPowerPoint: {:?}\nLibreOffice: {:?}",
        e.word, e.excel, e.powerpoint, e.libreoffice
    );
    for family in [
        Family::Word,
        Family::Excel,
        Family::PowerPoint,
        Family::Text,
    ] {
        let choice = e.resolve(family, Default::default());
        if e.libreoffice.is_some() {
            assert!(choice.is_some(), "LibreOffice handles every family");
        }
        println!(
            "{family:?} → {:?}",
            choice.as_ref().map(EngineChoice::display_name)
        );
    }
}

// ---------------------------------------------------------------- LibreOffice runner

fn ctx_dir() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

#[test]
fn libreoffice_exports_each_family_to_pdf() {
    let Some(lo) = libreoffice() else { return };
    let engine = EngineChoice::LibreOffice(lo);
    let cancel = CancelToken::new();
    for (name, family, pages) in [
        ("sample.docx", Family::Word, 1),
        ("sample.odt", Family::Word, 1),
        ("sample.xlsx", Family::Excel, 2),
        ("sample.pptx", Family::PowerPoint, 3),
        ("sample.html", Family::Text, 1),
    ] {
        let temp = ctx_dir();
        let ctx = TaskCtx::new(&cancel, &|_, _| {}, temp.path());
        let input = fixture(name);
        let before = std::fs::read(&input).unwrap();
        let start = Instant::now();
        let pdf = engine_convert::export(&input, family, DocFormat::Pdf, &engine, &ctx)
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        println!("{name} → PDF in {:?}", start.elapsed());
        assert!(pdf.starts_with(temp.path()), "engine output stays in temp");
        assert_eq!(page_count(&pdf), pages, "{name}");
        assert_eq!(std::fs::read(&input).unwrap(), before, "input changed");
    }
}

#[test]
fn libreoffice_csv_cells_stay_as_written() {
    use calamine::{Data, Reader, open_workbook_auto};
    let Some(lo) = libreoffice() else { return };
    let engine = EngineChoice::LibreOffice(lo);
    let cancel = CancelToken::new();
    let temp = ctx_dir();
    let ctx = TaskCtx::new(&cancel, &|_, _| {}, temp.path());
    // Windows-1252, `;` separated: a formula-looking text, a zero-padded ID,
    // a fraction-looking text and an accent.
    let csv = temp.path().join("cells.csv");
    std::fs::write(&csv, b"A;B;C;D\r\n=1+41;007;1/2;Caf\xE9\r\n").unwrap();
    let ods = engine_convert::export(&csv, Family::Excel, DocFormat::Ods, &engine, &ctx).unwrap();
    let mut book = open_workbook_auto(&ods).unwrap();
    let name = book.sheet_names()[0].clone();
    let range = book.worksheet_range(&name).unwrap();
    let row: Vec<Data> = range.rows().nth(1).unwrap().to_vec();
    let s = |v: &str| Data::String(v.to_owned());
    assert_eq!(row, [s("=1+41"), s("007"), s("1/2"), s("Café")]);
}

#[test]
fn libreoffice_exports_between_office_formats() {
    let Some(lo) = libreoffice() else { return };
    let engine = EngineChoice::LibreOffice(lo);
    let cancel = CancelToken::new();
    let temp = ctx_dir();
    let ctx = TaskCtx::new(&cancel, &|_, _| {}, temp.path());
    let docx = engine_convert::export(
        &fixture("sample.odt"),
        Family::Word,
        DocFormat::Docx,
        &engine,
        &ctx,
    )
    .unwrap();
    assert!(zip_names(&docx).iter().any(|n| n == "word/document.xml"));
    let odp = engine_convert::export(
        &fixture("sample.pptx"),
        Family::PowerPoint,
        DocFormat::Odp,
        &engine,
        &ctx,
    )
    .unwrap();
    assert!(zip_names(&odp).iter().any(|n| n == "content.xml"));
    let xlsx = engine_convert::export(
        &fixture("sample.csv"),
        Family::Excel,
        DocFormat::Xlsx,
        &engine,
        &ctx,
    )
    .unwrap();
    assert!(
        zip_names(&xlsx)
            .iter()
            .any(|n| n.starts_with("xl/worksheets/"))
    );
}

#[test]
fn libreoffice_damaged_file_fails_plainly() {
    let Some(lo) = libreoffice() else { return };
    let cancel = CancelToken::new();
    let temp = ctx_dir();
    let ctx = TaskCtx::new(&cancel, &|_, _| {}, temp.path());
    let e =
        soffice::convert(&lo, &SofficeJob::new(&fixture("broken.docx"), "pdf"), &ctx).unwrap_err();
    assert_eq!(e.kind, ErrorKind::BadInput, "{e}");
    assert!(e.message.contains("damaged"), "{e}");
}

#[test]
fn libreoffice_profiles_keep_the_safe_settings() {
    let Some(lo) = libreoffice() else { return };
    let cancel = CancelToken::new();
    let temp = ctx_dir();
    let ctx = TaskCtx::new(&cancel, &|_, _| {}, temp.path());
    soffice::convert(&lo, &SofficeJob::new(&fixture("sample.odt"), "pdf"), &ctx).unwrap();
    // After LibreOffice rewrote its settings on exit, every profile still has ours.
    let mut seen = 0;
    for slot in 0..3 {
        let xcu = soffice::profiles_root()
            .join(format!("profile-{slot}"))
            .join("user")
            .join("registrymodifications.xcu");
        let Ok(text) = std::fs::read_to_string(&xcu) else {
            continue;
        };
        seen += 1;
        for (p, n, v) in soffice::PROFILE_SETTINGS {
            assert!(
                text.contains(&soffice::profile_item(p, n, v)),
                "{n} missing in {}",
                xcu.display()
            );
        }
    }
    assert!(seen >= 1);
}

#[test]
fn libreoffice_encrypted_xls_says_password() {
    let Some(lo) = libreoffice() else { return };
    let cancel = CancelToken::new();
    let temp = ctx_dir();
    let ctx = TaskCtx::new(&cancel, &|_, _| {}, temp.path());
    // No hidden prompt: a stuck LibreOffice would hit this timeout instead.
    // (A timeout, not a wall clock: the wait for a free profile isn't counted.)
    let input = fixture("password.xls");
    let mut job = SofficeJob::new(&input, "pdf");
    job.timeout = Duration::from_secs(60);
    let e = soffice::convert(&lo, &job, &ctx).unwrap_err();
    assert_eq!(e.message, "This file is password-protected.", "{e}");
    // The unencrypted twin converts.
    let ok = soffice::convert(&lo, &SofficeJob::new(&fixture("sample.xls"), "pdf"), &ctx).unwrap();
    assert!(page_count(ok.file().unwrap()) >= 1);
}

#[test]
fn libreoffice_timeout_kills_and_frees_the_profile() {
    let Some(lo) = libreoffice() else { return };
    let cancel = CancelToken::new();
    let temp = ctx_dir();
    let ctx = TaskCtx::new(&cancel, &|_, _| {}, temp.path());
    let input = fixture("sample.pptx");
    let mut job = SofficeJob::new(&input, "pdf");
    job.timeout = Duration::from_millis(300);
    let e = soffice::convert(&lo, &job, &ctx).unwrap_err();
    assert_eq!(e.message, "LibreOffice took too long and was stopped.");
    // Measured by the runner from LibreOffice's start (kill included), not
    // from here: waiting for a free profile slot (shared by every test and
    // Morph process on the machine) can take long under load.
    let ran: f64 = e
        .details
        .split_once("stopped after ")
        .and_then(|(_, rest)| rest.split_once('s'))
        .and_then(|(secs, _)| secs.parse().ok())
        .unwrap_or_else(|| panic!("no run time in details: {e}"));
    assert!(ran < 10.0, "killed promptly: {}", e.details);
    // The profile lease is released: a normal run right after works.
    let ok = soffice::convert(&lo, &SofficeJob::new(&input, "pdf"), &ctx).unwrap();
    assert_eq!(page_count(ok.file().unwrap()), 3);
}

#[test]
fn libreoffice_cancel_stops_it() {
    let Some(lo) = libreoffice() else { return };
    let cancel = CancelToken::new();
    let temp = ctx_dir();
    let ctx = TaskCtx::new(&cancel, &|_, _| {}, temp.path());
    let input = fixture("sample.docx");
    let canceller = cancel.clone();
    let t = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(400));
        canceller.cancel();
    });
    let start = Instant::now();
    let r = soffice::convert(&lo, &SofficeJob::new(&input, "pdf"), &ctx);
    t.join().unwrap();
    match r {
        Err(e) => assert!(e.is_cancelled(), "{e}"),
        Ok(_) => println!("finished before the cancel arrived"),
    }
    assert!(start.elapsed() < Duration::from_secs(20));
}

#[test]
fn libreoffice_runs_in_parallel_with_separate_profiles() {
    let Some(lo) = libreoffice() else { return };
    let threads: Vec<_> = ["sample.docx", "sample.odt"]
        .into_iter()
        .map(|name| {
            let lo = lo.clone();
            std::thread::spawn(move || {
                let cancel = CancelToken::new();
                let temp = ctx_dir();
                let ctx = TaskCtx::new(&cancel, &|_, _| {}, temp.path());
                let out = soffice::convert(&lo, &SofficeJob::new(&fixture(name), "pdf"), &ctx)
                    .unwrap_or_else(|e| panic!("{name}: {e}"));
                page_count(out.file().unwrap())
            })
        })
        .collect();
    for t in threads {
        assert_eq!(t.join().unwrap(), 1);
    }
}

// ---------------------------------------------------------------- office.convert via the scheduler

#[test]
fn office_convert_job_writes_next_to_the_chosen_folder() {
    let Some(_lo) = libreoffice() else { return };
    let out = tempfile::tempdir().unwrap();
    let files = vec![
        fixture("sample.docx"),
        fixture("Relatório 報告.docx"),
        fixture("broken.docx"),
        fixture("sample.docx"), // twice: the second gets "sample (1).pdf"
    ];
    let action = Action::OfficeConvert(OfficeConvertOpts::default());
    let job = Job::for_files(
        &files,
        &action,
        &OutputSpec {
            folder: Some(out.path().to_path_buf()),
        },
    );
    let (tx, rx) = crossbeam_channel::unbounded();
    let config = JobConfig {
        cpu_threads: 2,
        temp_root: tempfile::tempdir().unwrap().keep(),
        ..JobConfig::default()
    };
    let summary = run_job_blocking(job, &config, &CancelToken::new(), &tx);
    drop(tx);
    let events: Vec<Event> = rx.iter().collect();
    assert_eq!(summary.done, 3, "{events:#?}");
    assert_eq!(summary.failed, 1);
    for name in ["sample.pdf", "sample (1).pdf", "Relatório 報告.pdf"] {
        assert_eq!(page_count(&out.path().join(name)), 1, "{name}");
    }
    let leftovers: Vec<_> = std::fs::read_dir(out.path())
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with(".~morph"))
        .collect();
    assert!(leftovers.is_empty(), "temp files left");
}

#[test]
fn refused_conversions_have_plain_messages() {
    // No engine needed: the router refuses before any engine runs.
    let out = tempfile::tempdir().unwrap();
    let opts = OfficeConvertOpts {
        format: DocFormat::Docx,
        ..Default::default()
    };
    let action = Action::OfficeConvert(opts);
    let job = Job::for_files(
        &[fixture("sample.xlsx"), fixture("sample.docx")],
        &action,
        &OutputSpec {
            folder: Some(out.path().to_path_buf()),
        },
    );
    let (tx, rx) = crossbeam_channel::unbounded();
    let config = JobConfig {
        temp_root: tempfile::tempdir().unwrap().keep(),
        ..JobConfig::default()
    };
    let summary = run_job_blocking(job, &config, &CancelToken::new(), &tx);
    drop(tx);
    assert_eq!(summary.failed, 2);
    let errors: Vec<String> = rx
        .iter()
        .filter_map(|e| match e {
            Event::TaskFailed { error, .. } => Some(error),
            _ => None,
        })
        .collect();
    assert!(errors.contains(&"A spreadsheet can't be converted to DOCX.".to_owned()));
    assert!(errors.contains(&"This file already is DOCX.".to_owned()));
}

#[test]
fn missing_engine_is_reported_plainly() {
    let none = detect::Engines::default();
    assert!(none.resolve(Family::Word, Default::default()).is_none());
    let e = morph_core::office::no_engine();
    assert_eq!(e.kind, ErrorKind::MissingEngine);
    assert_eq!(
        e.message,
        "To convert documents, install Microsoft Office or LibreOffice (free)."
    );
}
