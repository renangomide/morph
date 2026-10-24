// SPDX-License-Identifier: GPL-3.0-or-later
//! PDF → images and Extract text over `tests/fixtures/` (SPEC §4.2).
//! Perf: `cargo test --release -p morph-core --test pdf_render -- --ignored --nocapture`.

use morph_core::action::Action;
use morph_core::converter::{Converter, TaskCtx, TaskOutput};
use morph_core::error::{ConvertError, ErrorKind};
use morph_core::pdf::options::{PageImageFormat, PdfToImagesOpts, PdfToTextOpts};
use morph_core::pdf::text::PdfToText;
use morph_core::pdf::to_images::PdfToImages;
use morph_core::{CancelToken, OutputSpec, Task, TaskId};
use std::path::{Path, PathBuf};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures")
        .join(name)
}

fn task(input: &Path, action: Action, out: &Path) -> Task {
    Task {
        id: TaskId(1),
        inputs: vec![input.to_path_buf()],
        action,
        output: OutputSpec {
            folder: Some(out.to_path_buf()),
        },
    }
}

fn run_with(
    conv: &dyn Converter,
    input: &Path,
    action: Action,
    out: &Path,
    cancel: &CancelToken,
    progress: &(dyn Fn(f32, Option<std::time::Duration>) + Sync),
) -> Result<TaskOutput, ConvertError> {
    let before = std::fs::read(input).ok();
    let temp = tempfile::tempdir().unwrap();
    let ctx = TaskCtx::new(cancel, progress, temp.path());
    let r = conv.run(&task(input, action, out), &ctx);
    assert_eq!(std::fs::read(input).ok(), before, "input changed");
    r
}

fn run(
    conv: &dyn Converter,
    input: &Path,
    action: Action,
    out: &Path,
) -> Result<TaskOutput, ConvertError> {
    run_with(conv, input, action, out, &CancelToken::new(), &|_, _| {})
}

fn images(format: PageImageFormat, dpi: u32, pages: &str) -> Action {
    Action::PdfToImages(PdfToImagesOpts {
        format,
        dpi,
        pages: pages.into(),
    })
}

fn entries(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    v.sort();
    v
}

#[test]
fn png_pages_match_page_count_and_dpi() {
    let out = tempfile::tempdir().unwrap();
    let r = run(
        &PdfToImages,
        &fixture("text-3pages.pdf"),
        images(PageImageFormat::Png, 150, ""),
        out.path(),
    )
    .unwrap();
    assert_eq!(r.outputs.len(), 3);
    let dir = out.path().join("text-3pages (pages)");
    assert_eq!(
        entries(&dir),
        [
            "text-3pages-01.png",
            "text-3pages-02.png",
            "text-3pages-03.png"
        ]
    );
    for p in &r.outputs {
        assert_eq!(p.parent(), Some(dir.as_path()));
        let img = image::open(p).unwrap();
        // US Letter (612×792 pt) at 150 DPI.
        assert_eq!((img.width(), img.height()), (1275, 1650), "{}", p.display());
    }
    assert_eq!(entries(out.path()), ["text-3pages (pages)"]);
}

#[test]
fn page_range_and_jpg_webp_decode() {
    let out = tempfile::tempdir().unwrap();
    let input = fixture("text-3pages.pdf");
    let r = run(
        &PdfToImages,
        &input,
        images(PageImageFormat::Jpg, 72, "2-3"),
        out.path(),
    )
    .unwrap();
    assert_eq!(
        entries(&out.path().join("text-3pages (pages)")),
        ["text-3pages-02.jpg", "text-3pages-03.jpg"]
    );
    for p in &r.outputs {
        let img = image::open(p).unwrap();
        assert_eq!((img.width(), img.height()), (612, 792));
        // White background, not black.
        assert!(img.to_rgb8().get_pixel(2, 2).0.iter().all(|&c| c > 240));
    }
    // A second run gets a fresh folder instead of mixing files.
    let r = run(
        &PdfToImages,
        &input,
        images(PageImageFormat::Webp, 72, "1"),
        out.path(),
    )
    .unwrap();
    assert_eq!(r.outputs.len(), 1);
    assert!(r.outputs[0].ends_with("text-3pages (pages) (1)/text-3pages-01.webp"));
    let img = image::open(&r.outputs[0]).unwrap();
    assert_eq!((img.width(), img.height()), (612, 792));
}

#[test]
fn bad_range_and_bad_dpi_leave_nothing() {
    let out = tempfile::tempdir().unwrap();
    let input = fixture("text-3pages.pdf");
    let e = run(
        &PdfToImages,
        &input,
        images(PageImageFormat::Png, 150, "7-9"),
        out.path(),
    )
    .unwrap_err();
    assert!(e.message.contains("past the end"), "{e}");
    let e = run(
        &PdfToImages,
        &input,
        images(PageImageFormat::Png, 0, ""),
        out.path(),
    )
    .unwrap_err();
    assert!(e.message.contains("DPI"), "{e}");
    assert!(entries(out.path()).is_empty());
}

#[test]
fn unicode_name_works() {
    let out = tempfile::tempdir().unwrap();
    let input = fixture("Relatório 報告.pdf");
    let r = run(
        &PdfToImages,
        &input,
        images(PageImageFormat::Png, 72, ""),
        out.path(),
    )
    .unwrap();
    assert_eq!(r.outputs.len(), 1);
    assert!(r.outputs[0].ends_with("Relatório 報告 (pages)/Relatório 報告-01.png"));
    let r = run(
        &PdfToText,
        &input,
        Action::PdfToText(PdfToTextOpts::default()),
        out.path(),
    )
    .unwrap();
    assert!(r.outputs[0].ends_with("Relatório 報告.txt"));
}

#[test]
fn password_and_broken_fail_cleanly() {
    let out = tempfile::tempdir().unwrap();
    let cases: [(&dyn Converter, Action); 2] = [
        (&PdfToImages, images(PageImageFormat::Png, 150, "")),
        (&PdfToText, Action::PdfToText(PdfToTextOpts::default())),
    ];
    for (conv, action) in cases {
        let e = run(conv, &fixture("password.pdf"), action.clone(), out.path()).unwrap_err();
        assert_eq!(e.message, "This PDF is password-protected.");
        let e = run(conv, &fixture("broken.pdf"), action, out.path()).unwrap_err();
        assert_eq!(e.kind, ErrorKind::BadInput, "{e}");
    }
    assert!(entries(out.path()).is_empty());
}

#[test]
fn cancel_removes_the_half_written_folder() {
    let out = tempfile::tempdir().unwrap();
    let cancel = CancelToken::new();
    let c = cancel.clone();
    let progress = move |_: f32, _| c.cancel();
    let e = run_with(
        &PdfToImages,
        &fixture("pages-100.pdf"),
        images(PageImageFormat::Png, 72, ""),
        out.path(),
        &cancel,
        &progress,
    )
    .unwrap_err();
    assert!(e.is_cancelled(), "{e}");
    assert!(entries(out.path()).is_empty(), "{:?}", entries(out.path()));
}

#[test]
fn text_contains_expected_words() {
    let out = tempfile::tempdir().unwrap();
    let input = fixture("text-3pages.pdf");
    let r = run(
        &PdfToText,
        &input,
        Action::PdfToText(PdfToTextOpts {
            page_separators: true,
        }),
        out.path(),
    )
    .unwrap();
    assert_eq!(r.outputs, [out.path().join("text-3pages.txt")]);
    let bytes = std::fs::read(&r.outputs[0]).unwrap();
    assert!(!bytes.starts_with(&[0xEF, 0xBB, 0xBF]), "no BOM");
    let text = String::from_utf8(bytes).unwrap();
    assert!(
        text.contains("café") && text.contains("relatório") && text.contains("ação"),
        "{text}"
    );
    for n in 1..=3 {
        assert!(text.contains(&format!("--- Page {n} ---\r\n")), "{text}");
        assert!(text.contains(&format!("Page {n} of 3")), "{text}");
    }
    assert!(!text.replace("\r\n", "").contains('\n'), "CRLF only");

    let r = run(
        &PdfToText,
        &input,
        Action::PdfToText(PdfToTextOpts::default()),
        out.path(),
    )
    .unwrap();
    assert!(r.outputs[0].ends_with("text-3pages (1).txt"));
    let text = std::fs::read_to_string(&r.outputs[0]).unwrap();
    assert!(!text.contains("--- Page") && text.contains("café"));
}

#[test]
fn scanned_pdf_reports_no_text_and_writes_nothing() {
    let out = tempfile::tempdir().unwrap();
    let e = run(
        &PdfToText,
        &fixture("scanned.pdf"),
        Action::PdfToText(PdfToTextOpts::default()),
        out.path(),
    )
    .unwrap_err();
    assert_eq!(
        e.message,
        "No text found. This PDF may be scanned pages (pictures, not text)."
    );
    assert_eq!(e.kind, ErrorKind::Unsupported);
    assert!(entries(out.path()).is_empty());
}

/// SPEC §10: 100 pages → PNG at 150 DPI in < 30 s (release build).
#[test]
#[ignore = "performance; run in release with --ignored"]
fn perf_100_pages_png_150dpi() {
    // Release builds load pdfium.dll only from next to the exe: put it there.
    if !cfg!(debug_assertions) {
        let exe = std::env::current_exe().unwrap();
        let dst = exe.parent().unwrap().join("pdfium.dll");
        if !dst.exists() {
            let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../vendor/pdfium/pdfium.dll");
            std::fs::copy(src, dst).unwrap();
        }
    }
    let out = tempfile::tempdir().unwrap();
    let start = std::time::Instant::now();
    let r = run(
        &PdfToImages,
        &fixture("pages-100.pdf"),
        images(PageImageFormat::Png, 150, ""),
        out.path(),
    )
    .unwrap();
    let secs = start.elapsed().as_secs_f64();
    let bytes: u64 = r
        .outputs
        .iter()
        .map(|p| std::fs::metadata(p).unwrap().len())
        .sum();
    println!(
        "pages-100.pdf → PNG @150 DPI: {secs:.2} s, {} files, {} KB",
        r.outputs.len(),
        bytes / 1024
    );
    assert_eq!(r.outputs.len(), 100);
    assert!(secs < 30.0, "{secs:.2} s");
}
