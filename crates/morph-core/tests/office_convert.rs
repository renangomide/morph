// SPDX-License-Identifier: GPL-3.0-or-later
//! Acceptance (SPEC §12 M5): engine conversions and
//! Word/PowerPoint → images through the scheduler, for real with LibreOffice.
//!
//! LibreOffice tests print "SKIPPED: …" when it isn't installed. Microsoft Office
//! tests live next to the COM code (`office::engine_convert::ms_office`, `#[ignore]`d).

use morph_core::Summary as JobSummary;
use morph_core::action::Action;
use morph_core::event::Event;
use morph_core::office::detect::{self, EnginePreference};
use morph_core::office::options::{DocFormat, DocImageFormat};
use morph_core::office::{OfficeConvertOpts, OfficeToImagesOpts};
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

fn have_libreoffice() -> bool {
    let ok = detect::engines().libreoffice.is_some();
    if !ok {
        println!("SKIPPED: LibreOffice not installed");
    }
    ok
}

fn page_count(pdf: &Path) -> u32 {
    let doc = morph_core::pdf::pdfium::open(pdf).unwrap();
    morph_core::pdf::pdfium::page_count(&doc)
}

fn zip_text(path: &Path, part: &str) -> String {
    let mut z = zip::ZipArchive::new(std::fs::File::open(path).unwrap()).unwrap();
    let mut s = String::new();
    z.by_name(part).unwrap().read_to_string(&mut s).unwrap();
    s
}

struct Run {
    summary: JobSummary,
    events: Vec<Event>,
}

impl Run {
    fn outputs(&self) -> Vec<PathBuf> {
        self.events
            .iter()
            .filter_map(|e| match e {
                Event::TaskDone { outputs, .. } => Some(outputs.clone()),
                _ => None,
            })
            .flatten()
            .collect()
    }

    fn notes(&self) -> Vec<String> {
        self.events
            .iter()
            .filter_map(|e| match e {
                Event::TaskDone { note, .. } => note.clone(),
                _ => None,
            })
            .collect()
    }

    fn errors(&self) -> Vec<(String, String)> {
        self.events
            .iter()
            .filter_map(|e| match e {
                Event::TaskFailed { error, details, .. } => Some((error.clone(), details.clone())),
                _ => None,
            })
            .collect()
    }
}

fn run_with(
    files: &[PathBuf],
    action: Action,
    out: &Path,
    engine: EnginePreference,
    cancel: &CancelToken,
) -> Run {
    let job = Job::for_files(
        files,
        &action,
        &OutputSpec {
            folder: Some(out.to_path_buf()),
        },
    );
    let (tx, rx) = crossbeam_channel::unbounded();
    let config = JobConfig {
        cpu_threads: 2,
        temp_root: tempfile::tempdir().unwrap().keep(),
        office_engine: engine,
        ..JobConfig::default()
    };
    let summary = run_job_blocking(job, &config, cancel, &tx);
    drop(tx);
    Run {
        summary,
        events: rx.iter().collect(),
    }
}

fn convert(files: &[PathBuf], format: DocFormat, out: &Path) -> Run {
    let opts = OfficeConvertOpts {
        format,
        ..Default::default()
    };
    run_with(
        files,
        Action::OfficeConvert(opts),
        out,
        EnginePreference::Auto,
        &CancelToken::new(),
    )
}

fn timed<T>(what: &str, f: impl FnOnce() -> T) -> T {
    let start = Instant::now();
    let r = f();
    println!("TIMING {what}: {:.2}s", start.elapsed().as_secs_f64());
    r
}

fn unchanged(files: &[PathBuf], before: &[Vec<u8>]) {
    for (f, b) in files.iter().zip(before) {
        assert_eq!(&std::fs::read(f).unwrap(), b, "{} changed", f.display());
    }
}

// ---------------------------------------------------------------- → PDF

#[test]
fn office_documents_to_pdf_with_libreoffice() {
    if !have_libreoffice() {
        return;
    }
    // An RTF input made by LibreOffice itself (no RTF fixture).
    let rtf_dir = tempfile::tempdir().unwrap();
    let made = convert(&[fixture("sample.docx")], DocFormat::Rtf, rtf_dir.path());
    assert_eq!(made.summary.done, 1, "{:?}", made.errors());
    let rtf = made.outputs()[0].clone();

    for (file, pages) in [
        (fixture("sample.docx"), 1),
        (fixture("sample.odt"), 1),
        (fixture("sample.xlsx"), 2),
        (fixture("sample.pptx"), 3),
        (rtf.clone(), 1),
    ] {
        let out = tempfile::tempdir().unwrap();
        let before = std::fs::read(&file).unwrap();
        let name = file.file_name().unwrap().to_string_lossy().into_owned();
        let run = timed(&format!("{name} → PDF"), || {
            convert(std::slice::from_ref(&file), DocFormat::Pdf, out.path())
        });
        assert_eq!(run.summary.done, 1, "{name}: {:?}", run.errors());
        let pdf = &run.outputs()[0];
        let stem = file.file_stem().unwrap().to_string_lossy().into_owned();
        // Morph's naming, not LibreOffice's "input.pdf".
        assert_eq!(pdf, &out.path().join(format!("{stem}.pdf")));
        assert_eq!(page_count(pdf), pages, "{name}");
        assert_eq!(std::fs::read(&file).unwrap(), before, "{name} changed");
    }
}

#[test]
fn unicode_name_converts() {
    if !have_libreoffice() {
        return;
    }
    let out = tempfile::tempdir().unwrap();
    let run = convert(
        &[fixture("Relatório 報告.docx")],
        DocFormat::Odt,
        out.path(),
    );
    assert_eq!(run.summary.done, 1, "{:?}", run.errors());
    let odt = out.path().join("Relatório 報告.odt");
    assert!(zip_text(&odt, "content.xml").contains("報告"));
}

// ---------------------------------------------------------------- Word → other formats

#[test]
fn docx_to_word_formats_contain_the_text() {
    if !have_libreoffice() {
        return;
    }
    let out = tempfile::tempdir().unwrap();
    let input = fixture("sample.docx");
    let before = vec![std::fs::read(&input).unwrap()];
    let text = "relatório, ação";

    let odt = timed("docx → odt", || {
        convert(std::slice::from_ref(&input), DocFormat::Odt, out.path())
    });
    assert_eq!(odt.summary.done, 1, "{:?}", odt.errors());
    let content = zip_text(&out.path().join("sample.odt"), "content.xml");
    assert!(content.contains(text) && content.contains("報告"));

    let rtf = timed("docx → rtf", || {
        convert(std::slice::from_ref(&input), DocFormat::Rtf, out.path())
    });
    assert_eq!(rtf.summary.done, 1, "{:?}", rtf.errors());
    let rtf = std::fs::read(out.path().join("sample.rtf")).unwrap();
    assert!(rtf.starts_with(b"{\\rtf1"));

    let txt = timed("docx → txt", || {
        convert(std::slice::from_ref(&input), DocFormat::Txt, out.path())
    });
    assert_eq!(txt.summary.done, 1, "{:?}", txt.errors());
    let txt = std::fs::read(out.path().join("sample.txt")).unwrap();
    let txt = String::from_utf8(txt).expect("UTF-8 text");
    assert!(txt.contains(text) && txt.contains("報告"), "{txt}");

    let html = timed("docx → html", || {
        convert(std::slice::from_ref(&input), DocFormat::Html, out.path())
    });
    assert_eq!(html.summary.done, 1, "{:?}", html.errors());
    let html = std::fs::read_to_string(out.path().join("sample.html")).unwrap();
    assert!(html.contains("relatório") && html.contains("報告"));
    assert!(
        html.contains("src=\"data:image/png;base64,"),
        "picture is embedded"
    );
    // Only the published files (no LibreOffice picture files, no temp files).
    let names: Vec<String> = std::fs::read_dir(out.path())
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names.len(), 4, "{names:?}");

    // ODT → DOCX opens as a Word document.
    let docx = convert(&[fixture("sample.odt")], DocFormat::Docx, out.path());
    assert_eq!(docx.summary.done, 1, "{:?}", docx.errors());
    assert!(zip_text(&docx.outputs()[0], "word/document.xml").contains("Meeting notes"));
    unchanged(&[input], &before);
}

#[test]
fn sheets_and_slides_between_formats() {
    if !have_libreoffice() {
        return;
    }
    let out = tempfile::tempdir().unwrap();
    let ods = timed("xlsx → ods", || {
        convert(&[fixture("sample.xlsx")], DocFormat::Ods, out.path())
    });
    assert_eq!(ods.summary.done, 1, "{:?}", ods.errors());
    let content = zip_text(&out.path().join("sample.ods"), "content.xml");
    assert!(content.contains("Sales") && content.contains("東京"));

    let xlsx = convert(&ods.outputs(), DocFormat::Xlsx, out.path());
    assert_eq!(xlsx.summary.done, 1, "{:?}", xlsx.errors());
    {
        use calamine::{Reader, open_workbook_auto};
        let wb = open_workbook_auto(&xlsx.outputs()[0]).unwrap();
        assert_eq!(
            wb.sheet_names(),
            vec!["Sales".to_owned(), "Notes".to_owned()]
        );
    }

    let odp = timed("pptx → odp", || {
        convert(&[fixture("sample.pptx")], DocFormat::Odp, out.path())
    });
    assert_eq!(odp.summary.done, 1, "{:?}", odp.errors());
    let pptx = convert(&odp.outputs(), DocFormat::Pptx, out.path());
    assert_eq!(pptx.summary.done, 1, "{:?}", pptx.errors());
    let mut z = zip::ZipArchive::new(std::fs::File::open(&pptx.outputs()[0]).unwrap()).unwrap();
    let slides = (0..z.len())
        .filter(|&i| {
            let f = z.by_index(i).unwrap();
            let n = f.name().unwrap();
            n.starts_with("ppt/slides/slide") && n.ends_with(".xml")
        })
        .count();
    assert_eq!(slides, 3);

    // CSV input → PDF through the engine (sniffed delimiter).
    let pdf = convert(&[fixture("semicolon-bom.csv")], DocFormat::Pdf, out.path());
    assert_eq!(pdf.summary.done, 1, "{:?}", pdf.errors());
    let text = {
        let doc = morph_core::pdf::pdfium::open(&pdf.outputs()[0]).unwrap();
        let page = morph_core::pdf::pdfium::page(&doc, 0).unwrap();
        morph_core::pdf::pdfium::page_text(&page).unwrap()
    };
    assert!(
        text.contains("Zürich") && !text.contains("Zürich;"),
        "{text}"
    );
}

// ---------------------------------------------------------------- → images

fn to_images(file: &Path, out: &Path) -> Run {
    run_with(
        &[file.to_path_buf()],
        Action::OfficeToImages(OfficeToImagesOpts {
            format: DocImageFormat::Png,
            dpi: 96,
        }),
        out,
        EnginePreference::Auto,
        &CancelToken::new(),
    )
}

#[test]
fn slides_and_pages_to_images() {
    if !have_libreoffice() {
        return;
    }
    let out = tempfile::tempdir().unwrap();
    let run = timed("pptx → 3 PNG", || {
        to_images(&fixture("sample.pptx"), out.path())
    });
    assert_eq!(run.summary.done, 1, "{:?}", run.errors());
    let slides = out.path().join("sample (slides)");
    let names: Vec<PathBuf> = run.outputs();
    assert_eq!(names.len(), 3);
    for (i, p) in names.iter().enumerate() {
        assert_eq!(p, &slides.join(format!("sample-0{}.png", i + 1)));
        let img = image::open(p).unwrap();
        // 4:3 slide (10 x 7.5 in) at 96 DPI.
        assert_eq!((img.width(), img.height()), (960, 720), "{}", p.display());
    }

    let run = timed("docx → PNG pages", || {
        to_images(&fixture("sample.docx"), out.path())
    });
    assert_eq!(run.summary.done, 1, "{:?}", run.errors());
    assert_eq!(
        run.outputs(),
        vec![out.path().join("sample (pages)").join("sample-01.png")]
    );
    let img = image::open(&run.outputs()[0]).unwrap();
    assert_eq!((img.width(), img.height()), (816, 1056), "Letter at 96 DPI");

    // Spreadsheets have no images action.
    let run = to_images(&fixture("sample.xlsx"), out.path());
    assert_eq!(run.summary.done, 0);
}

// ---------------------------------------------------------------- failures

#[test]
fn broken_document_fails_plainly_and_leaves_nothing() {
    if !have_libreoffice() {
        return;
    }
    let out = tempfile::tempdir().unwrap();
    let run = timed("broken.docx", || {
        convert(&[fixture("broken.docx")], DocFormat::Pdf, out.path())
    });
    assert_eq!(run.summary.failed, 1);
    let (msg, details) = &run.errors()[0];
    assert!(msg.contains("damaged"), "{msg}");
    assert!(!details.is_empty());
    let run = to_images(&fixture("broken.docx"), out.path());
    assert_eq!(run.summary.failed, 1);
    assert_eq!(std::fs::read_dir(out.path()).unwrap().count(), 0);
}

#[test]
fn password_protected_document_fails_plainly() {
    // A synthetic encrypted OOXML container (Office stores the package in an
    // OLE file with an EncryptedPackage stream). No engine runs.
    let dir = tempfile::tempdir().unwrap();
    let locked = dir.path().join("locked.docx");
    let mut bytes = vec![0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1];
    bytes.resize(1024, 0);
    bytes.extend("EncryptedPackage".encode_utf16().flat_map(u16::to_le_bytes));
    std::fs::write(&locked, &bytes).unwrap();
    let out = tempfile::tempdir().unwrap();
    let start = Instant::now();
    let run = convert(&[locked], DocFormat::Pdf, out.path());
    if !detect::engines().any() {
        println!("SKIPPED: no Office engine");
        return;
    }
    assert_eq!(run.summary.failed, 1);
    assert_eq!(run.errors()[0].0, "This file is password-protected.");
    assert!(
        start.elapsed() < Duration::from_secs(5),
        "no engine started"
    );
}

#[test]
fn cancel_mid_conversion_leaves_nothing() {
    if !have_libreoffice() {
        return;
    }
    let out = tempfile::tempdir().unwrap();
    let cancel = CancelToken::new();
    let c = cancel.clone();
    let t = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(500));
        c.cancel();
    });
    let start = Instant::now();
    let run = run_with(
        &[fixture("sample.pptx"), fixture("sample.docx")],
        Action::OfficeToImages(OfficeToImagesOpts::default()),
        out.path(),
        EnginePreference::Auto,
        &cancel,
    );
    t.join().unwrap();
    println!(
        "TIMING cancel: {:.2}s, done {}",
        start.elapsed().as_secs_f64(),
        run.summary.done
    );
    assert!(start.elapsed() < Duration::from_secs(20));
    if run.summary.done == 0 {
        assert_eq!(
            std::fs::read_dir(out.path()).unwrap().count(),
            0,
            "nothing left behind"
        );
    }
}

#[test]
fn forced_missing_engine_falls_back_with_a_note() {
    let engines = detect::engines();
    if engines.libreoffice.is_none() || engines.word.is_some() {
        println!("SKIPPED: needs LibreOffice without Microsoft Word");
        return;
    }
    let out = tempfile::tempdir().unwrap();
    let run = run_with(
        &[fixture("sample.docx")],
        Action::OfficeConvert(OfficeConvertOpts::default()),
        out.path(),
        EnginePreference::MicrosoftOffice,
        &CancelToken::new(),
    );
    assert_eq!(run.summary.done, 1, "{:?}", run.errors());
    assert_eq!(
        run.notes(),
        vec!["Microsoft Office isn't installed, so LibreOffice was used.".to_owned()]
    );
}
