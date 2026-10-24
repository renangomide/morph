// SPDX-License-Identifier: GPL-3.0-or-later
//! Native DOCX → Markdown, and doc/odt → Markdown through the
//! engine (M5.1). Engine tests print "SKIPPED: …" without
//! LibreOffice.

use morph_core::action::Action;
use morph_core::converter::{Converter, TaskCtx, TaskOutput};
use morph_core::error::ConvertError;
use morph_core::job::{Task, TaskId};
use morph_core::office::detect;
use morph_core::office::options::DocFormat;
use morph_core::office::{OfficeConvert, OfficeConvertOpts};
use morph_core::{CancelToken, OutputSpec};
use std::io::Write;
use std::path::{Path, PathBuf};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures")
        .join(name)
}

/// A fresh folder with `fixture` copied in as `name`.
fn workspace(fixture_name: &str, name: &str) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join(name);
    std::fs::copy(fixture(fixture_name), &input).unwrap();
    (dir, input)
}

fn run(input: &Path, images: bool) -> Result<TaskOutput, ConvertError> {
    let before = std::fs::read(input).unwrap();
    let task = Task {
        id: TaskId(1),
        inputs: vec![input.to_path_buf()],
        action: Action::OfficeConvert(OfficeConvertOpts {
            format: DocFormat::Md,
            images,
            ..Default::default()
        }),
        output: OutputSpec::default(),
    };
    let cancel = CancelToken::new();
    let temp = tempfile::tempdir().unwrap();
    let ctx = TaskCtx::new(&cancel, &|_, _| {}, temp.path());
    let result = OfficeConvert.run(&task, &ctx);
    assert_eq!(std::fs::read(input).unwrap(), before, "input was modified");
    result
}

fn markdown(input: &Path, images: bool) -> String {
    let out = run(input, images).unwrap_or_else(|e| panic!("{}: {e}", input.display()));
    assert_eq!(out.outputs.len(), 1);
    std::fs::read_to_string(&out.outputs[0]).unwrap()
}

fn html_of(md: &str) -> String {
    let opts = pulldown_cmark::Options::ENABLE_TABLES | pulldown_cmark::Options::ENABLE_FOOTNOTES;
    let mut html = String::new();
    pulldown_cmark::html::push_html(&mut html, pulldown_cmark::Parser::new_ext(md, opts));
    html
}

fn names_in(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    v.sort();
    v
}

#[test]
fn structured_docx() {
    let (dir, input) = workspace("structured.docx", "structured.docx");
    let md = markdown(&input, true);
    let lines: Vec<&str> = md.lines().collect();
    let pos = |l: &str| {
        lines
            .iter()
            .position(|x| *x == l)
            .unwrap_or_else(|| panic!("{l:?} missing in:\n{md}"))
    };
    // Headings: Title, custom style based on Heading 1, Heading 2, outline level 2.
    assert_eq!(lines[0], "# Structured Document");
    let order = [
        pos("# Chapter One"),
        pos("## Lists"),
        pos("### Direct outline heading"),
        pos("## Table"),
        pos("## Picture"),
    ];
    assert!(order.windows(2).all(|w| w[0] < w[1]), "{md}");
    assert!(md.contains("**bold**") && md.contains("*italic*"), "{md}");
    assert!(
        md.contains("[Morph docs](https://example.com/docs)"),
        "{md}"
    );
    assert!(md.contains("a footnote.[^1]"), "{md}");
    assert!(md.contains("[^1]: Source: field notes, 2026."), "{md}");
    // Nested lists.
    let fruit = pos("- Fruit");
    assert_eq!(lines[fruit + 1], "  - Apple");
    assert_eq!(lines[fruit + 2], "    - Green apple");
    assert_eq!(lines[fruit + 3], "- Vegetables");
    let step = pos("1. Step one");
    assert_eq!(lines[step + 1], "   1. Sub-step a");
    assert_eq!(lines[step + 2], "2. Step two");
    // Table.
    assert!(md.contains("| Name | Qty | Note "), "{md}");
    assert!(md.contains("a\\|b") && md.contains("two<br>lines"), "{md}");
    let html = html_of(&md);
    assert!(
        html.contains("<table>") && html.contains("<td>a|b</td>"),
        "{html}"
    );
    assert!(html.contains("footnote-definition"), "{html}");
    // Picture.
    assert!(
        md.contains("![A red and blue sample picture](structured%20%28images%29/image-1.png)"),
        "{md}"
    );
    let png = std::fs::read(dir.path().join("structured (images)").join("image-1.png")).unwrap();
    assert!(png.starts_with(b"\x89PNG"));
}

#[test]
fn pictures_off_keeps_alt_text_only() {
    let (dir, input) = workspace("structured.docx", "structured.docx");
    let md = markdown(&input, false);
    assert!(md.contains("\nA red and blue sample picture\n"), "{md}");
    assert!(!md.contains("!["), "{md}");
    assert_eq!(names_in(dir.path()), ["structured.docx", "structured.md"]);
}

#[test]
fn sample_and_unicode_name_without_engine() {
    // Native reader: no Office engine is involved (CPU queue).
    let (dir, input) = workspace("Relatório 報告.docx", "Relatório 報告.docx");
    let md = markdown(&input, true);
    assert!(md.starts_with("# Quarterly report\n"), "{md}");
    assert!(md.contains("café, relatório, ação. CJK: 報告."), "{md}");
    assert!(
        md.contains("- Second point") && md.contains("| North "),
        "{md}"
    );
    assert!(
        md.contains("](Relatório%20報告%20%28images%29/image-1.png)"),
        "{md}"
    );
    assert!(dir.path().join("Relatório 報告 (images)").is_dir());

    let (_dir, input) = workspace("sample.docx", "sample.docx");
    let md = markdown(&input, true);
    assert!(
        md.contains("## Highlights") && md.contains("![Sample picture]"),
        "{md}"
    );
}

#[test]
fn broken_and_password_docx_fail_plainly() {
    let (dir, input) = workspace("broken.docx", "broken.docx");
    let err = run(&input, true).unwrap_err();
    assert!(err.message.contains("damaged"), "{err}");
    assert_eq!(names_in(dir.path()), ["broken.docx"]);

    // "Encrypt with password": an OLE file with an EncryptedPackage stream.
    let input = dir.path().join("locked.docx");
    let mut bytes = vec![0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1];
    bytes.resize(512, 0);
    bytes.extend("EncryptedPackage".encode_utf16().flat_map(u16::to_le_bytes));
    std::fs::write(&input, bytes).unwrap();
    let err = run(&input, true).unwrap_err();
    assert!(err.message.contains("password"), "{err}");
    assert_eq!(names_in(dir.path()), ["broken.docx", "locked.docx"]);
}

// ---------------------------------------------------------------- hand-built docx

const NS: &str = r#"xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships""#;

fn docx(path: &Path, body: &str, extra: &[(&str, String)]) {
    let mut w = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
    let opts = zip::write::SimpleFileOptions::default();
    let mut parts = vec![
        (
            "word/document.xml",
            format!(r#"<?xml version="1.0" encoding="UTF-8"?><w:document {NS}><w:body>{body}</w:body></w:document>"#),
        ),
        (
            "word/_rels/document.xml.rels",
            r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/numbering" Target="numbering.xml"/><Relationship Id="rId3" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/endnotes" Target="endnotes.xml"/><Relationship Id="rId4" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="file:///C:/secret.png" TargetMode="External"/></Relationships>"#.to_owned(),
        ),
    ];
    // extra may replace a default part.
    parts.retain(|(name, _)| !extra.iter().any(|(n, _)| n == name));
    for (name, xml) in extra {
        parts.push((name, xml.clone()));
    }
    for (name, xml) in parts {
        w.start_file(name, opts).unwrap();
        w.write_all(xml.as_bytes()).unwrap();
    }
    w.finish().unwrap();
}

fn p(ppr: &str, runs: &str) -> String {
    format!("<w:p><w:pPr>{ppr}</w:pPr>{runs}</w:p>")
}

fn r(rpr: &str, text: &str) -> String {
    format!(r#"<w:r><w:rPr>{rpr}</w:rPr><w:t xml:space="preserve">{text}</w:t></w:r>"#)
}

#[test]
fn revisions_fields_merged_cells_code_and_notes() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("edge.docx");
    let styles = format!(
        r#"<w:styles {NS}><w:style w:type="paragraph" w:default="1" w:styleId="Normal"><w:name w:val="Normal"/></w:style><w:style w:type="paragraph" w:styleId="Kop2"><w:name w:val="heading 2"/></w:style><w:style w:type="paragraph" w:styleId="Mine"><w:name w:val="Mine"/><w:basedOn w:val="Kop2"/></w:style><w:style w:type="paragraph" w:styleId="Code"><w:name w:val="Code"/><w:rPr><w:rFonts w:ascii="Consolas" w:hAnsi="Consolas"/></w:rPr></w:style></w:styles>"#
    );
    let numbering = format!(
        r#"<w:numbering {NS}><w:abstractNum w:abstractNumId="0"><w:lvl w:ilvl="0"><w:start w:val="1"/><w:numFmt w:val="decimal"/></w:lvl></w:abstractNum><w:abstractNum w:abstractNumId="1"><w:lvl w:ilvl="0"><w:numFmt w:val="none"/></w:lvl></w:abstractNum><w:num w:numId="5"><w:abstractNumId w:val="0"/><w:lvlOverride w:ilvl="0"><w:startOverride w:val="4"/></w:lvlOverride></w:num><w:num w:numId="6"><w:abstractNumId w:val="1"/></w:num></w:numbering>"#
    );
    let endnotes = format!(
        r#"<w:endnotes {NS}><w:endnote w:id="2"><w:p><w:r><w:endnoteRef/></w:r>{}</w:p></w:endnote></w:endnotes>"#,
        r("", " An endnote.")
    );
    let num = |id: u32| format!(r#"<w:numPr><w:ilvl w:val="0"/><w:numId w:val="{id}"/></w:numPr>"#);
    let tc =
        |pr: &str, text: &str| format!("<w:tc><w:tcPr>{pr}</w:tcPr>{}</w:tc>", p("", &r("", text)));
    let nested = format!("<w:tbl><w:tr>{}{}</w:tr></w:tbl>", tc("", "x"), tc("", "y"));
    let body = [
        p(r#"<w:pStyle w:val="Mine"/>"#, &r("<w:b/>", "Named heading")),
        p(
            "",
            &format!(
                r#"{}<w:del w:id="1"><w:r><w:delText>gone </w:delText></w:r></w:del><w:ins w:id="2">{}</w:ins>{}"#,
                r("", "Kept "),
                r("", "added "),
                r("<w:vanish/>", "hidden"),
            ),
        ),
        p(
            "",
            &format!(
                r#"<w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText> HYPERLINK "https://example.org/x" </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r>{}<w:r><w:fldChar w:fldCharType="end"/></w:r>{}<w:r><w:endnoteReference w:id="2"/></w:r><w:r><w:tab/></w:r>{}<w:r><w:br/></w:r>{}"#,
                r("", "field link"),
                r("<w:strike/>", " old"),
                r(r#"<w:rFonts w:ascii="Courier New"/>"#, "x = 1"),
                r("", "next line"),
            ),
        ),
        p(&num(5), &r("", "Fourth")),
        p(&num(5), &r("", "Fifth")),
        p(&num(6), &r("", "Not a list")),
        p(r#"<w:pStyle w:val="Code"/>"#, &r("", "fn main() {")),
        p(r#"<w:pStyle w:val="Code"/>"#, &r("", "    run();")),
        p(r#"<w:pStyle w:val="Code"/>"#, &r("", "}")),
        format!(
            "<w:tbl><w:tr>{}{}</w:tr><w:tr>{}{}</w:tr><w:tr>{}<w:tc>{}</w:tc></w:tr></w:tbl>",
            tc(r#"<w:gridSpan w:val="2"/>"#, "Wide"),
            tc("", "C"),
            tc(r#"<w:vMerge w:val="restart"/>"#, "Tall"),
            tc("", "b"),
            tc("<w:vMerge/>", "ignored"),
            nested,
        ),
        p("", r#"<w:r><w:drawing><wp:inline xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing"><wp:docPr id="1" name="p" descr="Remote chart"/><a:graphic xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><a:graphicData><a:blip r:link="rId4"/></a:graphicData></a:graphic></wp:inline></w:drawing></w:r>"#),
    ]
    .concat();
    docx(
        &input,
        &body,
        &[
            ("word/styles.xml", styles),
            ("word/numbering.xml", numbering),
            ("word/endnotes.xml", endnotes),
        ],
    );
    let md = markdown(&input, true);
    // Style named "heading 2" (localized id), reached through basedOn; no bold markers.
    assert!(md.starts_with("## Named heading\n"), "{md}");
    assert!(md.contains("\nKept added\n"), "{md}");
    assert!(!md.contains("gone") && !md.contains("hidden"), "{md}");
    assert!(md.contains("[field link](https://example.org/x)"), "{md}");
    assert!(md.contains("~~old~~"), "{md}");
    assert!(
        md.contains("[^1]") && md.contains("[^1]: An endnote."),
        "{md}"
    );
    assert!(md.contains("`x = 1`\\\nnext line"), "{md}");
    assert!(md.contains("4. Fourth\n5. Fifth"), "{md}");
    assert!(md.contains("\nNot a list\n"), "{md}");
    assert!(md.contains("```\nfn main() {\n    run();\n}\n```"), "{md}");
    // Spanned and vertically continued cells are empty; nested table as text.
    assert!(md.contains("| Wide |      | C   |"), "{md}");
    assert!(md.contains("| Tall | b    |"), "{md}");
    assert!(md.contains("|      | x; y |"), "{md}");
    assert!(!md.contains("ignored"), "{md}");
    // External picture: never fetched, alt text kept, no folder.
    assert!(md.contains("\n\nRemote chart\n\n"), "{md}");
    assert!(!dir.path().join("edge (images)").exists());
}

// ---------------------------------------------------------------- hostile files

/// Run  on a 16 MB stack, like the scheduler's task threads (a 2 MB debug
/// test thread can't parse the 256 levels the readers allow).
fn on_task_stack(f: impl FnOnce() + Send + 'static) {
    std::thread::Builder::new()
        .stack_size(16 << 20)
        .spawn(f)
        .unwrap()
        .join()
        .unwrap();
}

#[test]
fn deep_nesting_fails_plainly() {
    on_task_stack(deep_nesting_cases);
}

fn deep_nesting_cases() {
    let dir = tempfile::tempdir().unwrap();
    let cases = [
        // Content controls around blocks (the review's 22 KB repro).
        (
            "sdt.docx",
            format!(
                "{}{}{}",
                "<w:sdt><w:sdtContent>".repeat(200_000),
                p("", &r("", "deep")),
                "</w:sdtContent></w:sdt>".repeat(200_000)
            ),
        ),
        // Unknown inline wrappers inside a paragraph.
        (
            "inline.docx",
            format!(
                "<w:p>{}{}{}</w:p>",
                "<w:x>".repeat(100_000),
                r("", "deep"),
                "</w:x>".repeat(100_000)
            ),
        ),
        // Wrappers inside a table cell.
        (
            "cell.docx",
            format!(
                "<w:tbl><w:tr><w:tc>{}{}{}</w:tc></w:tr></w:tbl>",
                "<w:y>".repeat(100_000),
                p("", &r("", "deep")),
                "</w:y>".repeat(100_000)
            ),
        ),
    ];
    for (name, body) in cases {
        let input = dir.path().join(name);
        docx(&input, &body, &[]);
        let err = run(&input, true).unwrap_err();
        assert!(err.message.contains("nested too deeply"), "{name}: {err}");
    }
    // Reasonable nesting still converts.
    let input = dir.path().join("ok.docx");
    let body = format!(
        "{}{}{}",
        "<w:sdt><w:sdtContent>".repeat(60),
        p("", &r("", "fine")),
        "</w:sdtContent></w:sdt>".repeat(60)
    );
    docx(&input, &body, &[]);
    assert!(markdown(&input, true).contains("fine"));
}

#[test]
fn script_and_file_links_keep_only_their_text() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("links.docx");
    let rels = r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rJs" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink" Target="javascript:alert(1)" TargetMode="External"/><Relationship Id="rFile" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink" Target="file:///C:/Windows/win.ini" TargetMode="External"/><Relationship Id="rOk" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink" Target="https://example.com/" TargetMode="External"/></Relationships>"#;
    let body = [
        p("", &format!(r#"<w:hyperlink r:id="rJs">{}</w:hyperlink>"#, r("", "js"))),
        p("", &format!(r#"<w:hyperlink r:id="rFile">{}</w:hyperlink>"#, r("", "file"))),
        p("", &format!(r#"<w:hyperlink r:id="rOk">{}</w:hyperlink>"#, r("", "web"))),
        p("", &format!(r#"<w:fldSimple w:instr=" HYPERLINK &quot;vbscript:x&quot; ">{}</w:fldSimple>"#, r("", "simple"))),
        p(
            "",
            &format!(
                r#"<w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText> HYPERLINK "\\host\share\x.exe" </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r>{}<w:r><w:fldChar w:fldCharType="end"/></w:r>"#,
                r("", "unc")
            ),
        ),
    ]
    .concat();
    docx(
        &input,
        &body,
        &[("word/_rels/document.xml.rels", rels.to_owned())],
    );
    let md = markdown(&input, true);
    assert!(md.contains("[web](https://example.com/)"), "{md}");
    for text in ["js", "file", "simple", "unc"] {
        assert!(md.lines().any(|l| l == text), "{text}: {md}");
    }
    for bad in ["javascript", "file:", "vbscript", "host"] {
        assert!(!md.contains(bad), "{bad}: {md}");
    }
}

#[test]
fn one_picture_used_many_times_is_read_once() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("many.docx");
    // 32 MB that zips to a few KB, referenced 400 times (12 GB if re-read each time).
    let mut big = b"\x89PNG\r\n\x1a\n".to_vec();
    big.resize(32 * 1024 * 1024, 0);
    let rels = r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rP" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="media/big.png"/></Relationships>"#;
    let pic = p(
        "",
        r#"<w:r><w:drawing><wp:inline xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing"><wp:docPr id="1" name="p" descr="Big"/><a:graphic xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><a:graphicData><a:blip r:embed="rP"/></a:graphicData></a:graphic></wp:inline></w:drawing></w:r>"#,
    );
    docx(
        &input,
        &pic.repeat(400),
        &[("word/_rels/document.xml.rels", rels.to_owned())],
    );
    {
        let mut zip = zip::ZipWriter::new_append(
            std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(&input)
                .unwrap(),
        )
        .unwrap();
        zip.start_file(
            "word/media/big.png",
            zip::write::SimpleFileOptions::default(),
        )
        .unwrap();
        zip.write_all(&big).unwrap();
        zip.finish().unwrap();
    }
    let start = std::time::Instant::now();
    let md = markdown(&input, true);
    assert!(start.elapsed().as_secs() < 30, "{:?}", start.elapsed());
    assert_eq!(
        md.matches("![Big](many%20%28images%29/image-1.png)")
            .count(),
        400
    );
    assert_eq!(names_in(&dir.path().join("many (images)")), ["image-1.png"]);
}
// ---------------------------------------------------------------- engine path

#[test]
fn odt_and_doc_to_markdown_with_libreoffice() {
    if detect::engines().libreoffice.is_none() {
        println!("SKIPPED: LibreOffice not installed");
        return;
    }
    for name in ["sample.odt", "sample.doc"] {
        let (_dir, input) = workspace(name, name);
        let md = markdown(&input, true);
        assert!(md.lines().any(|l| l.starts_with("# ")), "{name}: {md}");
    }
    // Pictures off: no folder.
    let (dir, input) = workspace("sample.doc", "sample.doc");
    markdown(&input, false);
    assert_eq!(names_in(dir.path()), ["sample.doc", "sample.md"]);
}
