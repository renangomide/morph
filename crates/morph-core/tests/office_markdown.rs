// SPDX-License-Identifier: GPL-3.0-or-later
//! PPTX, spreadsheets and HTML → Markdown.
//! Engine tests (PPT/ODP) run for real when LibreOffice is detected and print
//! "SKIPPED: …" otherwise.

use morph_core::action::Action;
use morph_core::converter::{Converter, TaskCtx, TaskOutput};
use morph_core::error::ConvertError;
use morph_core::job::{Task, TaskId};
use morph_core::office::detect;
use morph_core::office::options::DocFormat;
use morph_core::office::{OfficeConvert, OfficeConvertOpts};
use morph_core::{CancelToken, OutputSpec};
use std::path::{Path, PathBuf};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures")
        .join(name)
}

/// Copy a fixture into a fresh folder under `as_name` (outputs go next to it).
fn workspace(name: &str, as_name: &str) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(as_name);
    std::fs::copy(fixture(name), &path).unwrap();
    (dir, path)
}

fn run(input: &Path, opts: OfficeConvertOpts) -> Result<TaskOutput, ConvertError> {
    let before = std::fs::read(input).unwrap();
    let task = Task {
        id: TaskId(1),
        inputs: vec![input.to_path_buf()],
        action: Action::OfficeConvert(opts),
        output: OutputSpec::default(),
    };
    let cancel = CancelToken::new();
    let temp = tempfile::tempdir().unwrap();
    let ctx = TaskCtx::new(&cancel, &|_, _| {}, temp.path());
    let result = OfficeConvert.run(&task, &ctx);
    assert_eq!(std::fs::read(input).unwrap(), before, "input was modified");
    result
}

fn md_opts() -> OfficeConvertOpts {
    OfficeConvertOpts {
        format: DocFormat::Md,
        ..Default::default()
    }
}

fn to_md(input: &Path, opts: OfficeConvertOpts) -> String {
    let out = run(input, opts).unwrap_or_else(|e| panic!("{}: {e}", input.display()));
    assert_eq!(out.outputs.len(), 1);
    assert_eq!(out.outputs[0].extension().unwrap(), "md");
    std::fs::read_to_string(&out.outputs[0]).unwrap()
}

fn before(md: &str, a: &str, b: &str) -> bool {
    match (md.find(a), md.find(b)) {
        (Some(x), Some(y)) => x < y,
        _ => false,
    }
}

// ---------------------------------------------------------------- PPTX

#[test]
fn structured_pptx() {
    let (dir, input) = workspace("structured.pptx", "Apresentação 報告.pptx");
    let md = to_md(&input, md_opts());
    println!("{md}");
    assert!(md.starts_with("## Slide 1: Project Overview\n"), "{md}");
    assert!(md.contains(
        "- Goals\n  - Faster conversions\n  - Fewer clicks\n    - Details matter\n- Timeline\n"
    ));
    assert!(md.contains("### Notes:\n\nWelcome everyone. Mention the café meeting."));
    assert!(md.contains("## Slide 2: Budget"));
    assert!(md.contains("| Item   | Q1   | Q2  |"), "{md}");
    assert!(md.contains("12.5"));
    assert!(before(&md, "| Item", "\nNumbers in EUR \\| estimates\n"));
    assert!(md.contains("## Slide 3: Team photo"));
    assert!(
        md.contains("![Team photo at the office](Apresentação%20報告%20%28images%29/image-1.png)")
    );
    assert!(md.contains("Visit [the Morph site](https://example.com/morph)"));
    assert!(
        dir.path()
            .join("Apresentação 報告 (images)/image-1.png")
            .is_file()
    );
    // Titles are not repeated as body text.
    assert_eq!(md.matches("Budget").count(), 1);
}

#[test]
fn pptx_without_pictures_or_notes() {
    let (dir, input) = workspace("structured.pptx", "deck.pptx");
    let md = to_md(
        &input,
        OfficeConvertOpts {
            images: false,
            slide_notes: false,
            ..md_opts()
        },
    );
    assert!(md.contains("\nTeam photo at the office\n"), "{md}");
    assert!(!md.contains("Notes"));
    assert!(!dir.path().join("deck (images)").exists());
}

#[test]
fn sample_pptx_without_placeholders() {
    let (_dir, input) = workspace("sample.pptx", "sample.pptx");
    let md = to_md(&input, md_opts());
    for n in 1..=3 {
        assert!(md.contains(&format!("## Slide {n}\n")), "{md}");
    }
    assert!(md.contains("Apresentação café ação 報告"));
}

#[test]
fn broken_pptx_fails_plainly() {
    let (dir, input) = workspace("broken.docx", "broken.pptx");
    let err = run(&input, md_opts()).unwrap_err();
    assert!(err.message.contains("damaged"), "{}", err.message);
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
}

// ---------------------------------------------------------------- hand-built pptx

const P_NS: &str = r#"xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships""#;
const RELS_NS: &str = "http://schemas.openxmlformats.org/package/2006/relationships";
const REL: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";

/// A one-slide PPTX: `shapes` inside the slide's `p:spTree`, `slide_rels` as
/// `<Relationship …/>` elements, `extra` parts as (name, bytes).
fn pptx(path: &Path, shapes: &str, slide_rels: &str, extra: &[(&str, Vec<u8>)]) {
    use std::io::Write;
    let mut w = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
    let opts = zip::write::SimpleFileOptions::default();
    let mut parts: Vec<(&str, Vec<u8>)> = vec![
        (
            "ppt/presentation.xml",
            format!(r#"<p:presentation {P_NS}><p:sldIdLst><p:sldId id="256" r:id="rId1"/></p:sldIdLst></p:presentation>"#).into_bytes(),
        ),
        (
            "ppt/_rels/presentation.xml.rels",
            format!(r#"<Relationships xmlns="{RELS_NS}"><Relationship Id="rId1" Type="{REL}/slide" Target="slides/slide1.xml"/></Relationships>"#).into_bytes(),
        ),
        (
            "ppt/slides/slide1.xml",
            format!(r#"<p:sld {P_NS}><p:cSld><p:spTree>{shapes}</p:spTree></p:cSld></p:sld>"#).into_bytes(),
        ),
        (
            "ppt/slides/_rels/slide1.xml.rels",
            format!(r#"<Relationships xmlns="{RELS_NS}">{slide_rels}</Relationships>"#).into_bytes(),
        ),
    ];
    parts.extend(extra.iter().map(|(n, b)| (*n, b.clone())));
    for (name, bytes) in parts {
        w.start_file(name, opts).unwrap();
        w.write_all(&bytes).unwrap();
    }
    w.finish().unwrap();
}

fn text_shape(text: &str) -> String {
    format!(r#"<p:sp><p:txBody><a:p><a:r><a:t>{text}</a:t></a:r></a:p></p:txBody></p:sp>"#)
}

#[test]
fn deeply_nested_pptx_groups_fail_plainly() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("deep.pptx");
    let shapes = format!(
        "{}{}{}",
        "<p:grpSp>".repeat(100_000),
        text_shape("deep"),
        "</p:grpSp>".repeat(100_000)
    );
    pptx(&input, &shapes, "", &[]);
    let err = run(&input, md_opts()).unwrap_err();
    assert!(err.message.contains("nested too deeply"), "{err}");
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
}

#[test]
fn huge_chart_tables_are_cut() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("chart.pptx");
    // 2,000 series, each claiming 100,000 points: 200 M cells uncapped.
    let series: String = (0..2_000)
        .map(|i| format!(r#"<c:ser><c:tx><c:v>S{i}</c:v></c:tx><c:cat><c:strRef><c:strCache><c:pt idx="99999"><c:v>last</c:v></c:pt></c:strCache></c:strRef></c:cat><c:val><c:numRef><c:numCache><c:pt idx="0"><c:v>1</c:v></c:pt><c:pt idx="99998"><c:v>2</c:v></c:pt></c:numCache></c:numRef></c:val></c:ser>"#))
        .collect();
    let chart = format!(
        r#"<c:chartSpace xmlns:c="http://schemas.openxmlformats.org/drawingml/2006/chart"><c:chart><c:plotArea><c:barChart>{series}</c:barChart></c:plotArea></c:chart></c:chartSpace>"#
    );
    let frame = r#"<p:graphicFrame><a:graphic><a:graphicData><c:chart xmlns:c="http://schemas.openxmlformats.org/drawingml/2006/chart" r:id="rC"/></a:graphicData></a:graphic></p:graphicFrame>"#;
    pptx(
        &input,
        frame,
        &format!(r#"<Relationship Id="rC" Type="{REL}/chart" Target="../charts/chart1.xml"/>"#),
        &[("ppt/charts/chart1.xml", chart.into_bytes())],
    );
    let start = std::time::Instant::now();
    let md = to_md(&input, md_opts());
    assert!(start.elapsed().as_secs() < 60, "{:?}", start.elapsed());
    assert!(md.contains("| S0 "), "{}", &md[..md.len().min(500)]);
    assert!(!md.contains("S300 "));
    assert!(md.trim_end().ends_with("… more chart values not shown."));
}

#[test]
fn oversized_pptx_picture_keeps_alt_text() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("pic.pptx");
    let mut big = b"\x89PNG\r\n\x1a\n".to_vec();
    big.resize(101 * 1024 * 1024, 0);
    let pic = r#"<p:pic><p:nvPicPr><p:cNvPr id="2" name="P" descr="Huge picture"/></p:nvPicPr><p:blipFill><a:blip r:embed="rP"/></p:blipFill></p:pic>"#;
    pptx(
        &input,
        &format!("{pic}{}", text_shape("after")),
        &format!(r#"<Relationship Id="rP" Type="{REL}/image" Target="../media/big.png"/>"#),
        &[("ppt/media/big.png", big)],
    );
    let md = to_md(&input, md_opts());
    assert!(
        md.contains("\nHuge picture\n") && md.contains("after"),
        "{md}"
    );
    assert!(!dir.path().join("pic (images)").exists());
}

#[test]
fn odp_through_libreoffice() {
    if detect::engines().libreoffice.is_none() {
        println!("SKIPPED: LibreOffice not installed");
        return;
    }
    // No .ppt/.odp fixture: make an ODP with the engine, then read it back
    // (ODP → PPTX by the engine → the native reader).
    let (_dir, input) = workspace("structured.pptx", "deck.pptx");
    let out = run(
        &input,
        OfficeConvertOpts {
            format: DocFormat::Odp,
            ..Default::default()
        },
    )
    .unwrap();
    let md = to_md(&out.outputs[0], md_opts());
    assert!(md.contains("## Slide 1: Project Overview"), "{md}");
    assert!(md.contains("Details matter"));
    assert!(md.contains("## Slide 3"));
}

// ---------------------------------------------------------------- sheets

#[test]
fn structured_xlsx() {
    let (_dir, input) = workspace("structured.xlsx", "Planilha 報告.xlsx");
    let md = to_md(&input, md_opts());
    println!("{md}");
    assert!(md.starts_with("## Data\n"), "{md}");
    assert!(md.contains("| 3 "), "{md}");
    assert!(!md.contains("3.0"));
    assert!(md.contains("2026-03-14"));
    assert!(md.contains("a\\|b pipe"));
    assert!(md.contains("line one<br>line two"));
    assert!(md.contains("## Summary"));
    assert!(md.contains("16"));
    assert!(md.contains("TRUE"));
    assert!(!md.contains("## Empty"));
    // No trailing empty rows/columns: every table line ends with a filled cell.
    assert!(!md.contains("|     |\n"), "{md}");
    assert!(!md.contains("| |"));
}

#[test]
fn xlsx_max_rows() {
    let (_dir, input) = workspace("structured.xlsx", "s.xlsx");
    let md = to_md(
        &input,
        OfficeConvertOpts {
            max_rows: 2,
            ..md_opts()
        },
    );
    let data = md.split("## Summary").next().unwrap();
    assert_eq!(
        data.lines().filter(|l| l.starts_with('|')).count(),
        4,
        "{md}"
    );
    assert!(data.contains("… 2 more rows not shown."), "{md}");
}

#[test]
fn csv_and_xls_to_markdown() {
    for name in [
        "sample.csv",
        "semicolon-bom.csv",
        "sample.xls",
        "sample.xlsx",
    ] {
        let (_dir, input) = workspace(name, name);
        let md = to_md(&input, md_opts());
        assert!(md.starts_with("## "), "{name}: {md}");
        assert!(md.contains("| ---"), "{name}: {md}");
        if name.ends_with(".csv") {
            assert_eq!(md.matches("## ").count(), 1, "{name}");
        }
    }
}

// ---------------------------------------------------------------- HTML

#[test]
fn structured_html() {
    let (dir, input) = workspace("structured.html", "Página 報告.html");
    let md = to_md(&input, md_opts());
    println!("{md}");
    for gone in [
        "Home",
        "About",
        "Privacy",
        "Footer text",
        "tracking",
        "bye",
        "enable JavaScript",
        "base64",
    ] {
        assert!(!md.contains(gone), "{gone} in {md}");
    }
    assert!(md.starts_with("# Structured page\n"), "{md}");
    assert!(md.contains("- Two\n  - Two point one"), "{md}");
    // A pipe in a cell is escaped (htmd's `&#124;` is rewritten).
    assert!(md.contains("| a\\|b | 1 "), "{md}");
    assert!(
        md.contains("![Red dot](Página%20報告%20%28images%29/image-1.png)"),
        "{md}"
    );
    assert!(
        dir.path()
            .join("Página 報告 (images)/image-1.png")
            .is_file()
    );

    let (dir, input) = workspace("structured.html", "off.html");
    let md = to_md(
        &input,
        OfficeConvertOpts {
            images: false,
            ..md_opts()
        },
    );
    assert!(md.contains("Red dot") && !md.contains("!["), "{md}");
    assert!(!dir.path().join("off (images)").exists());
}

#[test]
fn sample_html_still_converts() {
    let (_dir, input) = workspace("sample.html", "sample.html");
    let md = to_md(&input, md_opts());
    assert!(!md.trim().is_empty());
}
