// SPDX-License-Identifier: GPL-3.0-or-later
//! Lane D acceptance tests: Image → PDF, one PDF per image (SPEC §4.6).

use morph_core::cancel::CancelToken;
use morph_core::converter::{Converter, TaskCtx, TaskOutput};
use morph_core::error::ConvertError;
use morph_core::image::options::ImageToPdfOpts;
use morph_core::image::to_pdf::ImageToPdf;
use morph_core::pdf::options::PageSize;
use morph_core::pdf::pdfium;
use morph_core::{Action, Job, OutputSpec};
use std::path::{Path, PathBuf};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures")
        .join(name)
}

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

/// Run every task of the job; one result per input.
fn run(
    page_size: PageSize,
    margin_mm: u32,
    files: &[PathBuf],
) -> Vec<Result<TaskOutput, ConvertError>> {
    let action = Action::ImageToPdf(ImageToPdfOpts {
        page_size,
        margin_mm,
    });
    let job = Job::for_files(files, &action, &OutputSpec::default());
    assert_eq!(job.tasks.len(), files.len(), "one task per image");
    let cancel = CancelToken::new();
    let progress = |_: f32, _| {};
    let temp = tempfile::tempdir().unwrap();
    let ctx = TaskCtx::new(&cancel, &progress, temp.path());
    job.tasks.iter().map(|t| ImageToPdf.run(t, &ctx)).collect()
}

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

#[test]
fn one_pdf_per_image_next_to_it() {
    let (dir, files) = setup(&["sample.jpg", "orientation6.jpg", "sample.png"]);
    let before: Vec<Vec<u8>> = files.iter().map(|f| std::fs::read(f).unwrap()).collect();
    let outs = run(PageSize::FitToImage, 0, &files);
    for out in outs {
        let out = out.unwrap();
        assert_eq!(out.outputs.len(), 1);
        assert_eq!(out.outputs[0].parent().unwrap(), dir.path());
        assert_eq!(sizes(&out.outputs[0]).len(), 1, "one page");
    }
    assert!(dir.path().join("sample.pdf").exists());
    assert!(
        dir.path().join("sample (1).pdf").exists(),
        "collision gets (1)"
    );
    // 64×32 stored, orientation 6: an upright 32×64 page (24×48 pt at 96 DPI).
    assert_eq!(sizes(&dir.path().join("orientation6.pdf")), [(24, 48)]);
    let after: Vec<Vec<u8>> = files.iter().map(|f| std::fs::read(f).unwrap()).collect();
    assert_eq!(before, after, "inputs untouched");
}

#[test]
fn a4_and_letter_pages() {
    let (_dir, files) = setup(&["sample.png"]);
    let out = run(PageSize::A4, 10, &files).remove(0).unwrap();
    let s = sizes(&out.outputs[0]);
    assert!(s == [(595, 842)] || s == [(842, 595)], "{s:?}");
    let (_dir, files) = setup(&["sample.jpg"]);
    let out = run(PageSize::Letter, 0, &files).remove(0).unwrap();
    let s = sizes(&out.outputs[0]);
    assert!(s == [(612, 792)] || s == [(792, 612)], "{s:?}");
}

#[test]
fn heic_avif_svg_and_unicode_names() {
    let names = [
        "sample.heic",
        "sample.avif",
        "sample.svg",
        "Férias 夏.jpg",
        "café.png",
    ];
    let (dir, files) = setup(&names);
    for out in run(PageSize::FitToImage, 0, &files) {
        let out = out.unwrap();
        assert_eq!(sizes(&out.outputs[0]).len(), 1);
    }
    assert!(dir.path().join("Férias 夏.pdf").exists());
    assert!(dir.path().join("café.pdf").exists());
}

#[test]
fn broken_image_fails_cleanly() {
    let (dir, files) = setup(&["broken.png"]);
    let err = run(PageSize::FitToImage, 0, &files).remove(0).unwrap_err();
    assert!(!err.message.is_empty());
    let left: Vec<_> = std::fs::read_dir(dir.path()).unwrap().collect();
    assert_eq!(left.len(), 1, "nothing left behind");
}
