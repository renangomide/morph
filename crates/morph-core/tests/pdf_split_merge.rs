// SPDX-License-Identifier: GPL-3.0-or-later
//! Acceptance tests: Split and Merge (SPEC §4.2).

use morph_core::cancel::CancelToken;
use morph_core::converter::{Converter, TaskCtx, TaskOutput};
use morph_core::error::ConvertError;
use morph_core::image::io::{self, DecodeOptions};
use morph_core::pdf::merge::PdfMerge;
use morph_core::pdf::options::{PageSize, PdfMergeOpts, PdfSplitOpts, SplitMode};
use morph_core::pdf::pdfium;
use morph_core::pdf::split::PdfSplit;
use morph_core::{Action, Job, OutputSpec};
use std::path::{Path, PathBuf};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures")
        .join(name)
}

/// Copy fixtures into a fresh folder (outputs land next to the inputs).
fn setup(names: &[&str]) -> (tempfile::TempDir, Vec<PathBuf>) {
    let dir = tempfile::tempdir().unwrap();
    let files = names
        .iter()
        .map(|n| {
            let to = dir.path().join(n);
            std::fs::copy(fixture(n), &to).unwrap();
            to
        })
        .collect();
    (dir, files)
}

fn run_with(
    action: Action,
    files: &[PathBuf],
    cancel: &CancelToken,
) -> Result<TaskOutput, ConvertError> {
    let job = Job::for_files(files, &action, &OutputSpec::default());
    assert_eq!(job.tasks.len(), 1);
    let progress = |_: f32, _| {};
    let temp = tempfile::tempdir().unwrap();
    let ctx = TaskCtx::new(cancel, &progress, temp.path());
    match action {
        Action::PdfSplit(_) => PdfSplit.run(&job.tasks[0], &ctx),
        Action::PdfMerge(_) => PdfMerge.run(&job.tasks[0], &ctx),
        _ => unreachable!(),
    }
}

fn run(action: Action, files: &[PathBuf]) -> Result<TaskOutput, ConvertError> {
    run_with(action, files, &CancelToken::new())
}

fn split(mode: SplitMode, every: u32, ranges: &str) -> Action {
    Action::PdfSplit(PdfSplitOpts {
        mode,
        every,
        ranges: ranges.to_owned(),
    })
}

fn merge(page_size: PageSize, margin_mm: u32, bookmarks: bool) -> Action {
    Action::PdfMerge(PdfMergeOpts {
        page_size,
        margin_mm,
        bookmarks,
    })
}

/// File names in `dir`, sorted.
fn names(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    v.sort();
    v
}

fn pages_of(path: &Path) -> u32 {
    pdfium::page_count(&pdfium::open(path).unwrap())
}

/// Page sizes in points, rounded.
fn sizes(path: &Path) -> Vec<(i32, i32)> {
    let doc = pdfium::open(path).unwrap();
    (0..pdfium::page_count(&doc))
        .map(|i| {
            let p = pdfium::page(&doc, i).unwrap();
            (
                p.width().value.round() as i32,
                p.height().value.round() as i32,
            )
        })
        .collect()
}

/// (title, 1-based page) of each outline entry.
fn toc(path: &Path) -> Vec<(String, usize)> {
    let doc = lopdf::Document::load(path).unwrap();
    let toc = doc.get_toc().unwrap();
    assert!(toc.errors.is_empty(), "{:?}", toc.errors);
    toc.toc.into_iter().map(|t| (t.title, t.page)).collect()
}

fn bytes(files: &[PathBuf]) -> Vec<Vec<u8>> {
    files.iter().map(|f| std::fs::read(f).unwrap()).collect()
}

// ------------------------------------------------------------------ Split

#[test]
fn split_every_page() {
    let (dir, files) = setup(&["text-3pages.pdf"]);
    let before = bytes(&files);
    let out = run(split(SplitMode::EveryPage, 0, ""), &files).unwrap();
    let folder = dir.path().join("text-3pages (split)");
    // The part files, in order, all in the new folder (like PDF → images).
    let parts: Vec<PathBuf> = (1..=3)
        .map(|n| folder.join(format!("text-3pages-p{n}.pdf")))
        .collect();
    assert_eq!(out.outputs, parts);
    assert_eq!(
        names(&folder),
        [
            "text-3pages-p1.pdf",
            "text-3pages-p2.pdf",
            "text-3pages-p3.pdf"
        ]
    );
    for n in 1..=3 {
        let part = folder.join(format!("text-3pages-p{n}.pdf"));
        assert_eq!(pages_of(&part), 1);
        let doc = pdfium::open(&part).unwrap();
        let text = pdfium::page_text(&pdfium::page(&doc, 0).unwrap()).unwrap();
        assert!(text.contains(&format!("Page {n}")), "{text}");
    }
    assert_eq!(bytes(&files), before, "input unchanged");
}

#[test]
fn split_every_two_and_ranges() {
    let (dir, files) = setup(&["text-3pages.pdf"]);
    run(split(SplitMode::EveryN, 2, ""), &files).unwrap();
    let folder = dir.path().join("text-3pages (split)");
    assert_eq!(
        names(&folder),
        ["text-3pages-p1-2.pdf", "text-3pages-p3.pdf"]
    );
    assert_eq!(pages_of(&folder.join("text-3pages-p1-2.pdf")), 2);

    // Second split: new folder with " (1)".
    run(split(SplitMode::Ranges, 0, " 1-2 , 3- "), &files).unwrap();
    let folder = dir.path().join("text-3pages (split) (1)");
    assert_eq!(
        names(&folder),
        ["text-3pages-p1-2.pdf", "text-3pages-p3.pdf"]
    );

    // Overlapping ranges are allowed.
    let out = run(split(SplitMode::Ranges, 0, "1-3,2-3"), &files).unwrap();
    let folder = out.outputs[0].parent().unwrap();
    assert_eq!(folder, dir.path().join("text-3pages (split) (2)"));
    assert_eq!(pages_of(&folder.join("text-3pages-p1-3.pdf")), 3);
    assert_eq!(pages_of(&folder.join("text-3pages-p2-3.pdf")), 2);
    assert_eq!(out.outputs.len(), 2);

    let out = run(
        split(SplitMode::EveryPage, 0, ""),
        &[fixture_copy(&dir, "pages-100.pdf")],
    )
    .unwrap();
    assert_eq!(out.outputs.len(), 100);
    assert_eq!(names(out.outputs[0].parent().unwrap()).len(), 100);
}

fn fixture_copy(dir: &tempfile::TempDir, name: &str) -> PathBuf {
    let to = dir.path().join(name);
    std::fs::copy(fixture(name), &to).unwrap();
    to
}

#[test]
fn split_bad_ranges_leave_nothing() {
    let (dir, files) = setup(&["text-3pages.pdf"]);
    let err = run(split(SplitMode::Ranges, 0, "2-3,4-"), &files).unwrap_err();
    assert_eq!(
        err.message,
        "This PDF has only 3 pages, so \"4-\" is past the end."
    );
    let err = run(split(SplitMode::Ranges, 0, "1-x"), &files).unwrap_err();
    assert!(err.message.contains("isn't valid"), "{err}");
    let err = run(split(SplitMode::Ranges, 0, "3-1"), &files).unwrap_err();
    assert!(err.message.contains("isn't valid"), "{err}");
    assert_eq!(names(dir.path()), ["text-3pages.pdf"], "no folder left");
}

#[test]
fn split_password_and_broken_fail_cleanly() {
    let (dir, files) = setup(&["password.pdf", "broken.pdf"]);
    let err = run(split(SplitMode::EveryPage, 0, ""), &files[..1]).unwrap_err();
    assert_eq!(err.message, "This PDF is password-protected.");
    assert!(run(split(SplitMode::EveryPage, 0, ""), &files[1..]).is_err());
    assert_eq!(names(dir.path()), ["broken.pdf", "password.pdf"]);
}

#[test]
fn split_unicode_name() {
    let (dir, files) = setup(&["Relatório 報告.pdf"]);
    let out = run(split(SplitMode::EveryPage, 0, ""), &files).unwrap();
    assert_eq!(
        out.outputs,
        [dir.path()
            .join("Relatório 報告 (split)")
            .join("Relatório 報告-p1.pdf")]
    );
}

#[test]
fn split_cancelled_removes_folder() {
    let (dir, files) = setup(&["text-3pages.pdf"]);
    let cancel = CancelToken::new();
    cancel.cancel();
    let err = run_with(split(SplitMode::EveryPage, 0, ""), &files, &cancel).unwrap_err();
    assert!(err.is_cancelled());
    assert_eq!(names(dir.path()), ["text-3pages.pdf"]);
}

// ------------------------------------------------------------------ Merge

#[test]
fn merge_pdfs_and_images_in_order_with_bookmarks() {
    let (dir, files) = setup(&["text-3pages.pdf", "sample.jpg", "images.pdf", "sample.png"]);
    let before = bytes(&files);
    let out = run(merge(PageSize::FitToImage, 0, true), &files).unwrap();
    let merged = dir.path().join("Merged.pdf");
    assert_eq!(out.outputs, std::slice::from_ref(&merged));
    assert_eq!(out.note, None);
    assert_eq!(pages_of(&merged), 3 + 1 + 3 + 1);

    // Order: text pages first, then the 96×64 JPEG at 96 DPI (72×48 pt), the
    // image PDF (Letter), then the PNG.
    let s = sizes(&merged);
    assert_eq!(s[0], (612, 792));
    assert_eq!(s[3], (72, 48));
    assert_eq!(s[4], (612, 792));
    assert_eq!(s[7], (72, 48));
    let doc = pdfium::open(&merged).unwrap();
    let text = pdfium::page_text(&pdfium::page(&doc, 0).unwrap()).unwrap();
    assert!(text.contains("Page 1"), "{text}");
    drop(doc);

    assert_eq!(
        toc(&merged),
        [
            ("text-3pages".to_owned(), 1),
            ("sample".to_owned(), 4),
            ("images".to_owned(), 5),
            ("sample".to_owned(), 8),
        ]
    );
    // PDFium (what most viewers use) sees the same outline.
    let doc = pdfium::open(&merged).unwrap();
    let titles: Vec<String> = doc.bookmarks().iter().filter_map(|b| b.title()).collect();
    assert_eq!(titles, ["text-3pages", "sample", "images", "sample"]);
    drop(doc);
    assert_eq!(bytes(&files), before, "inputs unchanged");
}

#[test]
fn merge_jpeg_is_embedded_without_reencoding() {
    let (dir, files) = setup(&["sample.jpg"]);
    run(merge(PageSize::FitToImage, 0, false), &files).unwrap();
    let pdf = std::fs::read(dir.path().join("Merged.pdf")).unwrap();
    let jpg = std::fs::read(&files[0]).unwrap();
    assert!(
        pdf.windows(jpg.len()).any(|w| w == jpg.as_slice()),
        "the JPEG bytes are in the PDF as they are"
    );
    // No bookmarks requested: no outline.
    let doc = lopdf::Document::load(dir.path().join("Merged.pdf")).unwrap();
    assert!(doc.get_toc().is_err());
}

#[test]
fn merge_respects_exif_orientation() {
    // 64×32 stored, orientation 6: shows as 32×64.
    let (dir, files) = setup(&["orientation6.jpg", "sample.png"]);
    run(merge(PageSize::FitToImage, 0, true), &files).unwrap();
    let merged = dir.path().join("Merged.pdf");
    assert_eq!(sizes(&merged)[0], (24, 48));

    let decoded = io::decode(&files[0], &DecodeOptions::default()).unwrap();
    let upright = io::apply_orientation(decoded.image, decoded.orientation).to_rgb8();
    let doc = pdfium::open(&merged).unwrap();
    let rendered = pdfium::render_rgba(&pdfium::page(&doc, 0).unwrap(), 96).unwrap();
    assert_eq!(rendered.dimensions(), upright.dimensions());
    let diff: f64 = rendered
        .pixels()
        .zip(upright.pixels())
        .map(|(a, b)| {
            (0..3)
                .map(|c| (a[c] as f64 - b[c] as f64).abs())
                .sum::<f64>()
        })
        .sum::<f64>()
        / (rendered.width() * rendered.height() * 3) as f64;
    assert!(
        diff < 12.0,
        "rendered page matches the upright photo (diff {diff})"
    );
}

#[test]
fn merge_a4_with_margin() {
    let (dir, files) = setup(&["sample.jpg", "orientation6.jpg"]);
    run(merge(PageSize::A4, 10, false), &files).unwrap();
    let merged = dir.path().join("Merged.pdf");
    // Wide image → landscape A4; tall (after orientation) → portrait.
    assert_eq!(sizes(&merged), [(842, 595), (595, 842)]);
    // The image is inside the 10 mm (28.35 pt) margin: render and check that
    // the border strip is white and the centre isn't.
    let doc = pdfium::open(&merged).unwrap();
    let img = pdfium::render_rgba(&pdfium::page(&doc, 0).unwrap(), 72).unwrap();
    let px = |x: u32, y: u32| img.get_pixel(x, y).0;
    assert_eq!(px(10, img.height() / 2), [255, 255, 255, 255]);
    assert_eq!(px(img.width() / 2, 10), [255, 255, 255, 255]);
    assert_ne!(px(img.width() / 2, img.height() / 2), [255, 255, 255, 255]);
}

#[test]
fn merge_all_image_decoders() {
    let names_in = [
        "sample.heic",
        "sample.avif",
        "sample.webp",
        "sample.svg",
        "sample.gif",
        "sample.bmp",
        "sample.tiff",
        "sample.ico",
        "sample.tga",
        "sample.qoi",
        "icc-profile.jpg",
    ];
    let (dir, files) = setup(&names_in);
    let out = run(merge(PageSize::Letter, 5, true), &files).unwrap();
    let merged = &out.outputs[0];
    assert_eq!(pages_of(merged), names_in.len() as u32);
    assert_eq!(toc(merged).len(), names_in.len());
    assert!(dir.path().join("Merged.pdf").exists());
}

#[test]
fn merge_collision_and_unicode_names() {
    let (dir, files) = setup(&["Relatório 報告.pdf", "Férias 夏.jpg", "café.png"]);
    run(merge(PageSize::FitToImage, 0, true), &files).unwrap();
    let out = run(merge(PageSize::FitToImage, 0, true), &files).unwrap();
    assert_eq!(out.outputs, [dir.path().join("Merged (1).pdf")]);
    assert!(dir.path().join("Merged.pdf").exists());
    assert_eq!(
        toc(&out.outputs[0]),
        [
            ("Relatório 報告".to_owned(), 1),
            ("Férias 夏".to_owned(), 2),
            ("café".to_owned(), 3),
        ]
    );
}

#[test]
fn merge_order_follows_the_list() {
    let (dir, files) = setup(&["sample.jpg", "text-3pages.pdf"]);
    let reversed: Vec<PathBuf> = files.iter().rev().cloned().collect();
    run(merge(PageSize::FitToImage, 0, true), &reversed).unwrap();
    let merged = dir.path().join("Merged.pdf");
    assert_eq!(sizes(&merged)[3], (72, 48));
    assert_eq!(toc(&merged)[1], ("sample".to_owned(), 4));
}

#[test]
fn merge_password_input_fails_without_output() {
    let (dir, files) = setup(&["text-3pages.pdf", "password.pdf", "sample.jpg"]);
    let err = run(merge(PageSize::FitToImage, 0, true), &files).unwrap_err();
    assert_eq!(
        err.message,
        "Couldn't add \"password.pdf\": This PDF is password-protected."
    );
    assert!(err.details.contains("password.pdf"), "{}", err.details);
    assert_eq!(
        names(dir.path()),
        ["password.pdf", "sample.jpg", "text-3pages.pdf"],
        "no partial output"
    );

    let (dir, files) = setup(&["sample.jpg", "broken.png"]);
    let err = run(merge(PageSize::FitToImage, 0, true), &files).unwrap_err();
    assert!(err.message.contains("\"broken.png\""), "{err}");
    assert_eq!(names(dir.path()), ["broken.png", "sample.jpg"]);
}

#[test]
fn merge_cancelled_writes_nothing() {
    let (dir, files) = setup(&["text-3pages.pdf", "sample.jpg"]);
    let cancel = CancelToken::new();
    cancel.cancel();
    let err = run_with(merge(PageSize::FitToImage, 0, true), &files, &cancel).unwrap_err();
    assert!(err.is_cancelled());
    assert_eq!(names(dir.path()), ["sample.jpg", "text-3pages.pdf"]);
}
