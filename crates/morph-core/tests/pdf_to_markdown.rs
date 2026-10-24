// SPDX-License-Identifier: GPL-3.0-or-later
//! PDF → Markdown / HTML over `tests/fixtures/` (SPEC §4.6).
//! Perf: `cargo test --release -p morph-core --test pdf_to_markdown -- --ignored --nocapture`.

use morph_core::action::Action;
use morph_core::converter::{Converter, TaskCtx, TaskOutput};
use morph_core::error::{ConvertError, ErrorKind};
use morph_core::pdf::options::{PdfMarkupFormat, PdfToMarkdownOpts};
use morph_core::pdf::to_markdown::PdfToMarkdown;
use morph_core::{CancelToken, OutputSpec, Task, TaskId};
use std::path::{Path, PathBuf};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures")
        .join(name)
}

fn opts(format: PdfMarkupFormat, images: bool) -> Action {
    Action::PdfToMarkdown(PdfToMarkdownOpts { format, images })
}

fn run_with(
    input: &Path,
    action: Action,
    out: &Path,
    cancel: &CancelToken,
    progress: &(dyn Fn(f32, Option<std::time::Duration>) + Sync),
) -> Result<TaskOutput, ConvertError> {
    let before = std::fs::read(input).ok();
    let temp = tempfile::tempdir().unwrap();
    let ctx = TaskCtx::new(cancel, progress, temp.path());
    let task = Task {
        id: TaskId(1),
        inputs: vec![input.to_path_buf()],
        action,
        output: OutputSpec {
            folder: Some(out.to_path_buf()),
        },
    };
    let r = PdfToMarkdown.run(&task, &ctx);
    assert_eq!(std::fs::read(input).ok(), before, "input changed");
    r
}

fn run(input: &Path, action: Action, out: &Path) -> Result<TaskOutput, ConvertError> {
    run_with(input, action, out, &CancelToken::new(), &|_, _| {})
}

fn markdown(name: &str) -> String {
    let out = tempfile::tempdir().unwrap();
    let r = run(&fixture(name), opts(PdfMarkupFormat::Md, true), out.path()).unwrap();
    std::fs::read_to_string(&r.outputs[0]).unwrap()
}

fn entries(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    v.sort();
    v
}

fn pos(md: &str, needle: &str) -> usize {
    md.find(needle)
        .unwrap_or_else(|| panic!("missing {needle:?} in:\n{md}"))
}

#[test]
fn structured_headings_in_order() {
    let md = markdown("structured.pdf");
    let headings: Vec<&str> = md.lines().filter(|l| l.starts_with('#')).collect();
    assert_eq!(
        headings,
        [
            "# Annual Field Report",
            "## Introduction",
            "### Background",
            "### Method",
            "### Results",
            "## Discussion",
        ],
        "{md}"
    );
}

#[test]
fn structured_paragraphs_lists_and_table() {
    let md = markdown("structured.pdf");
    let para = md
        .lines()
        .find(|l| l.starts_with("This report describes"))
        .unwrap();
    assert!(para.contains("environmental survey"), "{md}");
    assert!(para.ends_with("in the autumn."), "{md}");
    assert!(!md.contains("environ-"));
    assert!(md.contains("the café where"));
    assert!(md.contains("- Water quality improved at most sites.\n- Two sites need a second visit because the samples from the first visit were lost in transport.\n- Costs stayed within the budget."), "{md}");
    assert!(md.contains("1. Take three samples at the marked points.\n2. Seal and label every sample on site.\n3. Send the samples to the laboratory within a day."), "{md}");
    assert!(md.contains("| Region | Samples | Passed |"), "{md}");
    assert!(md.contains("| North  | 120     | 112    |"), "{md}");
    assert!(md.contains("| East   | 75      | 75     |"), "{md}");
}

#[test]
fn structured_furniture_and_columns() {
    let md = markdown("structured.pdf");
    assert!(!md.contains("Morph Field Report"), "{md}");
    for n in ["1", "2", "3"] {
        assert!(!md.lines().any(|l| l.trim() == n), "page number {n}:\n{md}");
    }
    let last_alpha = md.rfind("Alpha").unwrap();
    assert!(last_alpha < pos(&md, "Beta one"), "{md}");
    assert!(md.contains("Alpha one: the left column starts here and continues"));
    assert!(md.contains("Beta one: the right column is read after the left column"));
}

#[test]
fn html_output() {
    let out = tempfile::tempdir().unwrap();
    let r = run(
        &fixture("structured.pdf"),
        opts(PdfMarkupFormat::Html, true),
        out.path(),
    )
    .unwrap();
    assert!(r.outputs[0].ends_with("structured.html"));
    let html = std::fs::read_to_string(&r.outputs[0]).unwrap();
    for tag in ["<h1>", "<h2>", "<h3>", "<table>", "<ul>", "<ol>"] {
        assert!(html.contains(tag), "{tag}");
    }
    assert_eq!(entries(out.path()), ["structured.html"]);
}

#[test]
fn plain_and_unicode_fixtures() {
    let md = markdown("text-3pages.pdf");
    assert!(md.contains("café, relatório, ação"), "{md}");
    assert!(md.contains("lazy dog (3)."), "{md}");
    let out = tempfile::tempdir().unwrap();
    let r = run(
        &fixture("Relatório 報告.pdf"),
        opts(PdfMarkupFormat::Md, true),
        out.path(),
    )
    .unwrap();
    assert!(r.outputs[0].ends_with("Relatório 報告.md"));
    let md = std::fs::read_to_string(&r.outputs[0]).unwrap();
    assert!(md.contains("ação"), "{md}");
}

#[test]
fn pictures_saved_or_left_out() {
    let out = tempfile::tempdir().unwrap();
    let r = run(
        &fixture("images.pdf"),
        opts(PdfMarkupFormat::Md, true),
        out.path(),
    )
    .unwrap();
    let md = std::fs::read_to_string(&r.outputs[0]).unwrap();
    assert!(md.contains("](images%20%28images%29/image-1."), "{md}");
    let pics = entries(&out.path().join("images (images)"));
    assert!(!pics.is_empty());

    let out = tempfile::tempdir().unwrap();
    run(
        &fixture("images.pdf"),
        opts(PdfMarkupFormat::Md, false),
        out.path(),
    )
    .unwrap();
    assert_eq!(entries(out.path()), ["images.md"]);
}

#[test]
fn scanned_password_broken_fail_cleanly() {
    let out = tempfile::tempdir().unwrap();
    let action = opts(PdfMarkupFormat::Md, true);
    let e = run(&fixture("scanned.pdf"), action.clone(), out.path()).unwrap_err();
    assert_eq!(
        e.message,
        "No text found. This PDF may be scanned pages (pictures, not text)."
    );
    let e = run(&fixture("password.pdf"), action.clone(), out.path()).unwrap_err();
    assert_eq!(e.message, "This PDF is password-protected.");
    let e = run(&fixture("broken.pdf"), action, out.path()).unwrap_err();
    assert_eq!(e.kind, ErrorKind::BadInput, "{e}");
    assert!(entries(out.path()).is_empty());
}

#[test]
fn cancel_leaves_nothing() {
    let out = tempfile::tempdir().unwrap();
    let cancel = CancelToken::new();
    let c = cancel.clone();
    let progress = move |_: f32, _| c.cancel();
    let e = run_with(
        &fixture("images.pdf"),
        opts(PdfMarkupFormat::Md, true),
        out.path(),
        &cancel,
        &progress,
    )
    .unwrap_err();
    assert!(e.is_cancelled(), "{e}");
    assert!(entries(out.path()).is_empty());
}

/// One-page-per-entry PDF of Helvetica lines `(size, x, y, text)` in `dir`.
fn build_pdf(dir: &Path, media: (f32, f32), pages: &[Vec<(f32, f32, f32, &str)>]) -> PathBuf {
    use lopdf::content::{Content, Operation};
    use lopdf::{Document, Object, Stream, dictionary};
    let mut doc = Document::with_version("1.5");
    let pages_id = doc.new_object_id();
    let font = doc.add_object(dictionary! {
        "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica",
    });
    let mut kids = Vec::new();
    for lines in pages {
        let mut ops = Vec::new();
        for &(size, x, y, text) in lines {
            ops.push(Operation::new("BT", vec![]));
            ops.push(Operation::new("Tf", vec!["F1".into(), size.into()]));
            ops.push(Operation::new("Td", vec![x.into(), y.into()]));
            ops.push(Operation::new("Tj", vec![Object::string_literal(text)]));
            ops.push(Operation::new("ET", vec![]));
        }
        let content = Content { operations: ops }.encode().unwrap();
        let content_id = doc.add_object(Stream::new(dictionary! {}, content));
        let page_id = doc.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "MediaBox" => vec![0.into(), 0.into(), media.0.into(), media.1.into()],
            "Contents" => content_id,
            "Resources" => dictionary! { "Font" => dictionary! { "F1" => font } },
        });
        kids.push(Object::Reference(page_id));
    }
    let count = kids.len() as i64;
    doc.objects.insert(
        pages_id,
        Object::Dictionary(dictionary! { "Type" => "Pages", "Kids" => kids, "Count" => count }),
    );
    let catalog = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
    doc.trailer.set("Root", catalog);
    let path = dir.join("built.pdf");
    doc.save(&path).unwrap();
    path
}

fn built_markdown(media: (f32, f32), pages: &[Vec<(f32, f32, f32, &str)>]) -> String {
    let dir = tempfile::tempdir().unwrap();
    let input = build_pdf(dir.path(), media, pages);
    let out = tempfile::tempdir().unwrap();
    let r = run(&input, opts(PdfMarkupFormat::Md, false), out.path()).unwrap();
    std::fs::read_to_string(&r.outputs[0]).unwrap()
}

#[test]
fn huge_page_finishes() {
    let start = std::time::Instant::now();
    let lines: Vec<(f32, f32, f32, &str)> = (0..6)
        .map(|i| {
            (
                11.0,
                72.0,
                600.0 - 14.0 * i as f32,
                "Some words on a very wide page",
            )
        })
        .collect();
    let md = built_markdown((50_000_000.0, 800.0), &[lines]);
    assert!(md.contains("Some words on a very wide page"), "{md}");
    assert!(start.elapsed().as_secs() < 10, "{:?}", start.elapsed());
}

#[test]
fn roman_and_letter_lists() {
    let body = "Body text of the document sits here in a normal paragraph line.";
    let items = [
        "i. First roman item",
        "ii. Second roman item",
        "iii. Third roman item",
    ];
    let letters = [
        "g. Seventh letter item",
        "h. Eighth letter item",
        "i. Ninth letter item",
    ];
    let mut lines = vec![(11.0, 72.0, 700.0, body)];
    for (k, t) in items.iter().enumerate() {
        lines.push((11.0, 72.0, 670.0 - 14.0 * k as f32, t));
    }
    lines.push((11.0, 72.0, 600.0, body));
    for (k, t) in letters.iter().enumerate() {
        lines.push((11.0, 72.0, 570.0 - 14.0 * k as f32, t));
    }
    let md = built_markdown((612.0, 792.0), &[lines]);
    assert!(
        md.contains("1. First roman item\n2. Second roman item\n3. Third roman item"),
        "{md}"
    );
    assert!(
        md.contains("7. Seventh letter item\n8. Eighth letter item\n9. Ninth letter item"),
        "{md}"
    );
}

#[test]
fn large_repeated_title_kept_once() {
    let page = |n: &'static str| {
        vec![
            (20.0, 72.0, 740.0, "Quarterly Summary"),
            (
                11.0,
                72.0,
                600.0,
                "Body text of the page goes on for a while here.",
            ),
            (11.0, 72.0, 586.0, n),
        ]
    };
    let md = built_markdown(
        (612.0, 792.0),
        &[
            page("Page one ends."),
            page("Page two ends."),
            page("Page three ends."),
        ],
    );
    assert_eq!(md.matches("Quarterly Summary").count(), 1, "{md}");
    assert!(md.starts_with("# Quarterly Summary"), "{md}");
}

#[test]
#[ignore = "performance; run in release with --ignored"]
fn hundred_pages_under_ten_seconds() {
    let out = tempfile::tempdir().unwrap();
    let start = std::time::Instant::now();
    run(
        &fixture("pages-100.pdf"),
        opts(PdfMarkupFormat::Md, true),
        out.path(),
    )
    .unwrap();
    let took = start.elapsed();
    println!("pages-100.pdf → Markdown: {took:?}");
    assert!(took.as_secs_f32() < 10.0, "{took:?}");
}
