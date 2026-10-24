// SPDX-License-Identifier: GPL-3.0-or-later
//! Regenerates the M5.1 "two-way conversions" fixtures in `tests/fixtures/`
//! (`structured.*`, see the README there).
//!
//!     cargo run -p morph-core --example make_twoway_fixtures
//!
//! No Office or LibreOffice needed: the PDF is built with lopdf (Helvetica /
//! Helvetica-Bold, not embedded), DOCX/PPTX are hand-written zips, XLSX comes
//! from rust_xlsxwriter, HTML is text. Dates are fixed, so the bytes are the
//! same on every run.

use lopdf::content::{Content, Operation};
use lopdf::{Document, Object, Stream, dictionary};
use rust_xlsxwriter::{DocProperties, ExcelDateTime, Format, Formula, Workbook};
use std::error::Error;
use std::io::Write;
use std::path::{Path, PathBuf};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, DateTime, ZipWriter};

type Res<T> = Result<T, Box<dyn Error>>;

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures")
}

fn save(name: &str, bytes: &[u8]) -> Res<()> {
    let path = fixtures_dir().join(name);
    std::fs::write(&path, bytes)?;
    println!("{:>9} bytes  {}", bytes.len(), path.display());
    Ok(())
}

// ---------------------------------------------------------------- PDF

const LETTER: (f32, f32) = (612.0, 792.0);
const HEADER: &str = "Morph Field Report \u{2014} 2026";

/// One piece of text at an absolute position (baseline).
struct Txt {
    bold: bool,
    size: f32,
    x: f32,
    y: f32,
    text: String,
}

/// Text in WinAnsiEncoding (Latin-1 plus the bullet and dashes).
fn win_ansi(s: &str) -> Vec<u8> {
    s.chars()
        .map(|c| match c {
            '\u{2022}' => 0x95,
            '\u{2013}' => 0x96,
            '\u{2014}' => 0x97,
            c => u8::try_from(u32::from(c)).unwrap_or(b'?'),
        })
        .collect()
}

/// A page being laid out from the top.
struct Page {
    items: Vec<Txt>,
    y: f32,
}

impl Page {
    fn new(n: u32) -> Page {
        let mut p = Page {
            items: Vec::new(),
            y: 700.0,
        };
        // Running header and a bare page number in the footer.
        p.at(false, 9.0, 72.0, 750.0, HEADER);
        p.at(false, 9.0, 303.0, 40.0, &n.to_string());
        p
    }

    fn at(&mut self, bold: bool, size: f32, x: f32, y: f32, text: &str) {
        self.items.push(Txt {
            bold,
            size,
            x,
            y,
            text: text.to_owned(),
        });
    }

    /// Lines at `x`, moving down; `gap_before` extra space first.
    fn lines(&mut self, bold: bool, size: f32, x: f32, gap_before: f32, lines: &[&str]) {
        self.y -= gap_before;
        for line in lines {
            self.at(bold, size, x, self.y, line);
            self.y -= size * 1.3;
        }
    }

    fn heading(&mut self, size: f32, text: &str) {
        self.lines(true, size, 72.0, size * 0.8, &[text]);
        self.y -= 2.0;
    }

    fn body(&mut self, lines: &[&str]) {
        self.lines(false, 11.0, 72.0, 6.0, lines);
    }

    /// A list item: marker at 80, text (and wrapped lines) at 96.
    fn item(&mut self, marker: &str, lines: &[&str]) {
        self.at(false, 11.0, 80.0, self.y, marker);
        self.lines(false, 11.0, 96.0, 0.0, lines);
    }

    fn table(&mut self, rows: &[[&str; 3]]) {
        self.y -= 8.0;
        for (i, row) in rows.iter().enumerate() {
            for (c, cell) in row.iter().enumerate() {
                let x = [72.0, 230.0, 390.0][c];
                self.at(i == 0, 11.0, x, self.y, cell);
            }
            self.y -= 16.0;
        }
    }
}

fn structured_pdf() -> Res<Vec<u8>> {
    // Page 1: title, H1, paragraphs (wrapped, one hyphenated word), H2, bullets.
    let mut p1 = Page::new(1);
    p1.lines(true, 24.0, 72.0, 0.0, &["Annual Field Report"]);
    p1.heading(18.0, "Introduction");
    p1.body(&[
        "This report describes the work of the field team during the year. It covers",
        "the planning, the visits to every site and the results of the environ-",
        "mental survey that was carried out in the spring and in the autumn.",
    ]);
    p1.body(&["The second paragraph is shorter and mentions the caf\u{e9} where the team met."]);
    p1.heading(14.0, "Background");
    p1.body(&["The main findings were:"]);
    p1.y -= 4.0;
    p1.item("\u{2022}", &["Water quality improved at most sites."]);
    p1.item(
        "\u{2022}",
        &[
            "Two sites need a second visit because the samples from the first",
            "visit were lost in transport.",
        ],
    );
    p1.item("\u{2022}", &["Costs stayed within the budget."]);

    // Page 2: H2, numbered list, H2, paragraph, 3x4 table.
    let mut p2 = Page::new(2);
    p2.heading(14.0, "Method");
    p2.body(&["Each visit followed the same steps:"]);
    p2.y -= 4.0;
    p2.item("1.", &["Take three samples at the marked points."]);
    p2.item("2.", &["Seal and label every sample on site."]);
    p2.item("3.", &["Send the samples to the laboratory within a day."]);
    p2.heading(14.0, "Results");
    p2.body(&["The table below lists the number of samples per region."]);
    p2.table(&[
        ["Region", "Samples", "Passed"],
        ["North", "120", "112"],
        ["South", "98", "90"],
        ["East", "75", "75"],
    ]);
    p2.body(&["All regions met the target of ninety percent."]);

    // Page 3: H1 across the page, then two columns (gutter 288-324).
    let mut p3 = Page::new(3);
    p3.heading(18.0, "Discussion");
    let top = p3.y - 10.0;
    let left = [
        "Alpha one: the left column starts",
        "here and continues for a few",
        "lines of text in a narrow block.",
        "Alpha two: it ends before the",
        "right column begins.",
    ];
    let right = [
        "Beta one: the right column is",
        "read after the left column has",
        "ended, from top to bottom.",
        "Beta two: this is the last line",
        "of the page body.",
    ];
    for (i, (l, r)) in left.iter().zip(right.iter()).enumerate() {
        let y = top - i as f32 * 14.3;
        p3.at(false, 11.0, 72.0, y, l);
        p3.at(false, 11.0, 324.0, y, r);
    }

    let mut doc = Document::with_version("1.5");
    let pages_id = doc.new_object_id();
    let regular = doc.add_object(dictionary! {
        "Type" => "Font", "Subtype" => "Type1",
        "BaseFont" => "Helvetica", "Encoding" => "WinAnsiEncoding",
    });
    let bold = doc.add_object(dictionary! {
        "Type" => "Font", "Subtype" => "Type1",
        "BaseFont" => "Helvetica-Bold", "Encoding" => "WinAnsiEncoding",
    });
    let mut kids = Vec::new();
    for page in [p1, p2, p3] {
        let mut ops = Vec::new();
        for t in page.items {
            ops.push(Operation::new("BT", vec![]));
            let font = if t.bold { "F2" } else { "F1" };
            ops.push(Operation::new("Tf", vec![font.into(), t.size.into()]));
            ops.push(Operation::new("Td", vec![t.x.into(), t.y.into()]));
            ops.push(Operation::new(
                "Tj",
                vec![Object::string_literal(win_ansi(&t.text))],
            ));
            ops.push(Operation::new("ET", vec![]));
        }
        let content = Content { operations: ops }.encode()?;
        let content_id = doc.add_object(Stream::new(dictionary! {}, content));
        let page_id = doc.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "MediaBox" => vec![0.into(), 0.into(), LETTER.0.into(), LETTER.1.into()],
            "Contents" => content_id,
            "Resources" => dictionary! {
                "Font" => dictionary! { "F1" => regular, "F2" => bold },
            },
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
    let id = Object::String(
        b"morph-twoway-pdf".to_vec(),
        lopdf::StringFormat::Hexadecimal,
    );
    doc.trailer.set("ID", vec![id.clone(), id]);
    doc.compress();
    let mut bytes = Vec::new();
    doc.save_to(&mut bytes)?;
    Ok(bytes)
}

// ---------------------------------------------------------------- zip / OOXML helpers

fn zip(entries: &[(&str, &[u8])]) -> Res<Vec<u8>> {
    let mut w = ZipWriter::new(std::io::Cursor::new(Vec::new()));
    for (name, bytes) in entries {
        let method = if name.ends_with(".png") {
            CompressionMethod::Stored
        } else {
            CompressionMethod::Deflated
        };
        let options = SimpleFileOptions::default()
            .compression_method(method)
            .last_modified_time(DateTime::default());
        w.start_file(*name, options)?;
        w.write_all(bytes)?;
    }
    Ok(w.finish()?.into_inner())
}

const XML: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>"#;
const REL: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";

/// Relationships part from `(id, type suffix, target, external)`.
fn rels(items: &[(&str, &str, &str, bool)]) -> String {
    let mut s = format!(
        r#"{XML}<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">"#
    );
    for (id, kind, target, external) in items {
        let mode = if *external {
            r#" TargetMode="External""#
        } else {
            ""
        };
        s.push_str(&format!(
            r#"<Relationship Id="{id}" Type="{REL}/{kind}" Target="{target}"{mode}/>"#
        ));
    }
    s.push_str("</Relationships>");
    s
}

fn content_types(overrides: &[(&str, &str)]) -> String {
    let mut s = format!(
        r#"{XML}<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Default Extension="png" ContentType="image/png"/>"#
    );
    for (part, ct) in overrides {
        s.push_str(&format!(
            r#"<Override PartName="{part}" ContentType="{ct}"/>"#
        ));
    }
    s.push_str("</Types>");
    s
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

// ---------------------------------------------------------------- DOCX

const W_NS: &str = r#"xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:pic="http://schemas.openxmlformats.org/drawingml/2006/picture""#;
const WML: &str = "application/vnd.openxmlformats-officedocument.wordprocessingml";

fn run(text: &str, rpr: &str) -> String {
    let rpr = if rpr.is_empty() {
        String::new()
    } else {
        format!("<w:rPr>{rpr}</w:rPr>")
    };
    format!(
        r#"<w:r>{rpr}<w:t xml:space="preserve">{}</w:t></w:r>"#,
        esc(text)
    )
}

fn para(ppr: &str, runs: &str) -> String {
    let ppr = if ppr.is_empty() {
        String::new()
    } else {
        format!("<w:pPr>{ppr}</w:pPr>")
    };
    format!("<w:p>{ppr}{runs}</w:p>")
}

fn styled(style: &str, text: &str) -> String {
    para(&format!(r#"<w:pStyle w:val="{style}"/>"#), &run(text, ""))
}

fn list_para(num_id: u32, level: u32, text: &str) -> String {
    para(
        &format!(
            r#"<w:pStyle w:val="ListParagraph"/><w:numPr><w:ilvl w:val="{level}"/><w:numId w:val="{num_id}"/></w:numPr>"#
        ),
        &run(text, ""),
    )
}

fn docx_styles() -> String {
    let heading = |id: &str, name: &str, level: u32, size: u32| {
        format!(
            r#"<w:style w:type="paragraph" w:styleId="{id}"><w:name w:val="{name}"/><w:basedOn w:val="Normal"/><w:next w:val="Normal"/><w:qFormat/><w:pPr><w:keepNext/><w:spacing w:before="240" w:after="120"/><w:outlineLvl w:val="{level}"/></w:pPr><w:rPr><w:b/><w:sz w:val="{size}"/></w:rPr></w:style>"#
        )
    };
    let borders = r#"<w:tblBorders><w:top w:val="single" w:sz="4" w:space="0" w:color="000000"/><w:left w:val="single" w:sz="4" w:space="0" w:color="000000"/><w:bottom w:val="single" w:sz="4" w:space="0" w:color="000000"/><w:right w:val="single" w:sz="4" w:space="0" w:color="000000"/><w:insideH w:val="single" w:sz="4" w:space="0" w:color="000000"/><w:insideV w:val="single" w:sz="4" w:space="0" w:color="000000"/></w:tblBorders>"#;
    format!(
        concat!(
            r#"{xml}<w:styles {ns}><w:docDefaults><w:rPrDefault><w:rPr><w:rFonts w:ascii="Calibri" w:hAnsi="Calibri" w:cs="Calibri"/><w:sz w:val="22"/><w:lang w:val="en-US"/></w:rPr></w:rPrDefault><w:pPrDefault><w:pPr><w:spacing w:after="120"/></w:pPr></w:pPrDefault></w:docDefaults>"#,
            r#"<w:style w:type="paragraph" w:default="1" w:styleId="Normal"><w:name w:val="Normal"/><w:qFormat/></w:style>"#,
            r#"<w:style w:type="paragraph" w:styleId="Title"><w:name w:val="Title"/><w:basedOn w:val="Normal"/><w:next w:val="Normal"/><w:qFormat/><w:rPr><w:sz w:val="56"/></w:rPr></w:style>"#,
            "{h1}{h2}",
            // Custom style based on Heading 1, no outline level of its own: must read as level 1.
            r#"<w:style w:type="paragraph" w:customStyle="1" w:styleId="MyChapter"><w:name w:val="My Chapter"/><w:basedOn w:val="Heading1"/><w:next w:val="Normal"/><w:qFormat/><w:rPr><w:color w:val="2E74B5"/></w:rPr></w:style>"#,
            r#"<w:style w:type="paragraph" w:styleId="ListParagraph"><w:name w:val="List Paragraph"/><w:basedOn w:val="Normal"/><w:pPr><w:ind w:left="720"/></w:pPr></w:style>"#,
            r#"<w:style w:type="paragraph" w:styleId="FootnoteText"><w:name w:val="footnote text"/><w:basedOn w:val="Normal"/><w:rPr><w:sz w:val="20"/></w:rPr></w:style>"#,
            r#"<w:style w:type="character" w:styleId="FootnoteReference"><w:name w:val="footnote reference"/><w:rPr><w:vertAlign w:val="superscript"/></w:rPr></w:style>"#,
            r#"<w:style w:type="character" w:styleId="Hyperlink"><w:name w:val="Hyperlink"/><w:rPr><w:color w:val="0563C1"/><w:u w:val="single"/></w:rPr></w:style>"#,
            r#"<w:style w:type="table" w:styleId="TableGrid"><w:name w:val="Table Grid"/><w:tblPr>{borders}</w:tblPr></w:style></w:styles>"#
        ),
        xml = XML,
        ns = W_NS,
        h1 = heading("Heading1", "heading 1", 0, 32),
        h2 = heading("Heading2", "heading 2", 1, 26),
        borders = borders,
    )
}

fn docx_numbering() -> String {
    let level = |ilvl: u32, fmt: &str, text: &str| {
        format!(
            r#"<w:lvl w:ilvl="{ilvl}"><w:start w:val="1"/><w:numFmt w:val="{fmt}"/><w:lvlText w:val="{text}"/><w:lvlJc w:val="left"/><w:pPr><w:ind w:left="{}" w:hanging="360"/></w:pPr></w:lvl>"#,
            720 * (ilvl + 1)
        )
    };
    format!(
        r#"{XML}<w:numbering {W_NS}><w:abstractNum w:abstractNumId="0"><w:multiLevelType w:val="hybridMultilevel"/>{}{}{}</w:abstractNum><w:abstractNum w:abstractNumId="1"><w:multiLevelType w:val="hybridMultilevel"/>{}{}{}</w:abstractNum><w:num w:numId="1"><w:abstractNumId w:val="0"/></w:num><w:num w:numId="2"><w:abstractNumId w:val="1"/></w:num></w:numbering>"#,
        level(0, "bullet", "\u{2022}"),
        level(1, "bullet", "o"),
        level(2, "bullet", "\u{25AA}"),
        level(0, "decimal", "%1."),
        level(1, "lowerLetter", "%2."),
        level(2, "lowerRoman", "%3."),
    )
}

fn structured_docx(png: &[u8]) -> Res<Vec<u8>> {
    let mut body = String::new();
    body.push_str(&styled("Title", "Structured Document"));
    body.push_str(&styled("MyChapter", "Chapter One"));
    // Bold, italic, a hyperlink and a footnote reference.
    body.push_str(&para(
        "",
        &format!(
            r#"{}{}{}{}{}<w:hyperlink r:id="rId10" w:history="1">{}</w:hyperlink>{}<w:r><w:rPr><w:rStyle w:val="FootnoteReference"/></w:rPr><w:footnoteReference w:id="1"/></w:r>"#,
            run("This chapter has ", ""),
            run("bold", "<w:b/>"),
            run(" and ", ""),
            run("italic", "<w:i/>"),
            run(" text, a link to the ", ""),
            run("Morph docs", r#"<w:rStyle w:val="Hyperlink"/>"#),
            run(" and a footnote.", ""),
        ),
    ));
    body.push_str(&styled("Heading2", "Lists"));
    body.push_str(&list_para(1, 0, "Fruit"));
    body.push_str(&list_para(1, 1, "Apple"));
    body.push_str(&list_para(1, 2, "Green apple"));
    body.push_str(&list_para(1, 0, "Vegetables"));
    body.push_str(&para("", &run("Steps:", "")));
    body.push_str(&list_para(2, 0, "Step one"));
    body.push_str(&list_para(2, 1, "Sub-step a"));
    body.push_str(&list_para(2, 0, "Step two"));
    // Outline level set on the paragraph itself (Normal style): heading level 3.
    body.push_str(&para(
        r#"<w:outlineLvl w:val="2"/><w:rPr><w:b/></w:rPr>"#,
        &run("Direct outline heading", "<w:b/>"),
    ));
    body.push_str(&para(
        "",
        &run("Text under the direct outline heading.", ""),
    ));
    body.push_str(&styled("Heading2", "Table"));
    let rows = [
        ["Name", "Qty", "Note"],
        ["Caf\u{e9}", "2", "a|b"],
        ["Tea", "10", "two\nlines"],
    ];
    body.push_str(r#"<w:tbl><w:tblPr><w:tblStyle w:val="TableGrid"/><w:tblW w:w="0" w:type="auto"/></w:tblPr><w:tblGrid><w:gridCol w:w="3000"/><w:gridCol w:w="3000"/><w:gridCol w:w="3000"/></w:tblGrid>"#);
    for row in rows {
        body.push_str("<w:tr>");
        for cell in row {
            let mut runs = String::new();
            for (i, part) in cell.split('\n').enumerate() {
                if i > 0 {
                    runs.push_str("<w:r><w:br/></w:r>");
                }
                runs.push_str(&run(part, ""));
            }
            body.push_str(&format!(
                r#"<w:tc><w:tcPr><w:tcW w:w="3000" w:type="dxa"/></w:tcPr>{}</w:tc>"#,
                para("", &runs)
            ));
        }
        body.push_str("</w:tr>");
    }
    body.push_str("</w:tbl>");
    body.push_str(&styled("Heading2", "Picture"));
    body.push_str(r#"<w:p><w:r><w:drawing><wp:inline distT="0" distB="0" distL="0" distR="0"><wp:extent cx="914400" cy="609600"/><wp:docPr id="1" name="Picture 1" descr="A red and blue sample picture"/><a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/picture"><pic:pic><pic:nvPicPr><pic:cNvPr id="1" name="image1.png" descr="A red and blue sample picture"/><pic:cNvPicPr/></pic:nvPicPr><pic:blipFill><a:blip r:embed="rId4"/><a:stretch><a:fillRect/></a:stretch></pic:blipFill><pic:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="914400" cy="609600"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></pic:spPr></pic:pic></a:graphicData></a:graphic></wp:inline></w:drawing></w:r></w:p>"#);
    body.push_str(r#"<w:sectPr><w:pgSz w:w="12240" w:h="15840"/><w:pgMar w:top="1440" w:right="1440" w:bottom="1440" w:left="1440" w:header="720" w:footer="720" w:gutter="0"/></w:sectPr>"#);
    let document = format!(r#"{XML}<w:document {W_NS}><w:body>{body}</w:body></w:document>"#);
    let footnotes = format!(
        r#"{XML}<w:footnotes {W_NS}><w:footnote w:type="separator" w:id="-1"><w:p><w:r><w:separator/></w:r></w:p></w:footnote><w:footnote w:type="continuationSeparator" w:id="0"><w:p><w:r><w:continuationSeparator/></w:r></w:p></w:footnote><w:footnote w:id="1"><w:p><w:pPr><w:pStyle w:val="FootnoteText"/></w:pPr><w:r><w:rPr><w:rStyle w:val="FootnoteReference"/></w:rPr><w:footnoteRef/></w:r>{}</w:p></w:footnote></w:footnotes>"#,
        run(" Source: field notes, 2026.", "")
    );
    let ct = content_types(&[
        ("/word/document.xml", &format!("{WML}.document.main+xml")),
        ("/word/styles.xml", &format!("{WML}.styles+xml")),
        ("/word/numbering.xml", &format!("{WML}.numbering+xml")),
        ("/word/footnotes.xml", &format!("{WML}.footnotes+xml")),
    ]);
    let root_rels = rels(&[("rId1", "officeDocument", "word/document.xml", false)]);
    let doc_rels = rels(&[
        ("rId1", "styles", "styles.xml", false),
        ("rId2", "numbering", "numbering.xml", false),
        ("rId3", "footnotes", "footnotes.xml", false),
        ("rId4", "image", "media/image1.png", false),
        ("rId10", "hyperlink", "https://example.com/docs", true),
    ]);
    zip(&[
        ("[Content_Types].xml", ct.as_bytes()),
        ("_rels/.rels", root_rels.as_bytes()),
        ("word/document.xml", document.as_bytes()),
        ("word/_rels/document.xml.rels", doc_rels.as_bytes()),
        ("word/styles.xml", docx_styles().as_bytes()),
        ("word/numbering.xml", docx_numbering().as_bytes()),
        ("word/footnotes.xml", footnotes.as_bytes()),
        ("word/media/image1.png", png),
    ])
}

// ---------------------------------------------------------------- PPTX

const P_NS: &str = r#"xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main""#;
const PML: &str = "application/vnd.openxmlformats-officedocument";
const GROUP: &str = r#"<p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="0" cy="0"/><a:chOff x="0" y="0"/><a:chExt cx="0" cy="0"/></a:xfrm></p:grpSpPr>"#;

fn theme() -> String {
    let colors = [
        ("dk1", "000000"),
        ("lt1", "FFFFFF"),
        ("dk2", "1F497D"),
        ("lt2", "EEECE1"),
        ("accent1", "4F81BD"),
        ("accent2", "C0504D"),
        ("accent3", "9BBB59"),
        ("accent4", "8064A2"),
        ("accent5", "4BACC6"),
        ("accent6", "F79646"),
        ("hlink", "0000FF"),
        ("folHlink", "800080"),
    ];
    let clr: String = colors
        .iter()
        .map(|(n, c)| format!(r#"<a:{n}><a:srgbClr val="{c}"/></a:{n}>"#))
        .collect();
    let fill = r#"<a:solidFill><a:schemeClr val="phClr"/></a:solidFill>"#;
    let line = r#"<a:ln w="9525"><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:ln>"#;
    let effect = r#"<a:effectStyle><a:effectLst/></a:effectStyle>"#;
    format!(
        r#"{XML}<a:theme xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" name="Morph"><a:themeElements><a:clrScheme name="Morph">{clr}</a:clrScheme><a:fontScheme name="Morph"><a:majorFont><a:latin typeface="Calibri"/><a:ea typeface=""/><a:cs typeface=""/></a:majorFont><a:minorFont><a:latin typeface="Calibri"/><a:ea typeface=""/><a:cs typeface=""/></a:minorFont></a:fontScheme><a:fmtScheme name="Morph"><a:fillStyleLst>{fill}{fill}{fill}</a:fillStyleLst><a:lnStyleLst>{line}{line}{line}</a:lnStyleLst><a:effectStyleLst>{effect}{effect}{effect}</a:effectStyleLst><a:bgFillStyleLst>{fill}{fill}{fill}</a:bgFillStyleLst></a:fmtScheme></a:themeElements></a:theme>"#
    )
}

fn xfrm(x: u32, y: u32, cx: u32, cy: u32) -> String {
    format!(r#"<a:xfrm><a:off x="{x}" y="{y}"/><a:ext cx="{cx}" cy="{cy}"/></a:xfrm>"#)
}

/// A shape with a placeholder (`ph`, e.g. `type="title"`) and paragraphs `(level, text)`.
fn shape(id: u32, name: &str, ph: &str, pos: &str, paras: &[(u32, &str)]) -> String {
    let nv_pr = if ph.is_empty() {
        "<p:nvPr/>".to_owned()
    } else {
        format!("<p:nvPr><p:ph {ph}/></p:nvPr>")
    };
    let tx_box = if ph.is_empty() { r#" txBox="1""# } else { "" };
    let mut body = String::new();
    for (lvl, text) in paras {
        let ppr = if *lvl > 0 {
            format!(r#"<a:pPr lvl="{lvl}"/>"#)
        } else {
            String::new()
        };
        body.push_str(&format!(
            r#"<a:p>{ppr}<a:r><a:rPr lang="en-US"/><a:t>{}</a:t></a:r></a:p>"#,
            esc(text)
        ));
    }
    format!(
        r#"<p:sp><p:nvSpPr><p:cNvPr id="{id}" name="{name}"/><p:cNvSpPr{tx_box}/>{nv_pr}</p:nvSpPr><p:spPr>{pos}</p:spPr><p:txBody><a:bodyPr/><a:lstStyle/>{body}</p:txBody></p:sp>"#
    )
}

fn slide_xml(tree: &str) -> String {
    format!(
        r#"{XML}<p:sld {P_NS}><p:cSld><p:spTree>{GROUP}{tree}</p:spTree></p:cSld><p:clrMapOvr><a:masterClrMapping/></p:clrMapOvr></p:sld>"#
    )
}

fn notes_xml(slide_img_id: u32, text: &str) -> String {
    format!(
        r#"{XML}<p:notes {P_NS}><p:cSld><p:spTree>{GROUP}<p:sp><p:nvSpPr><p:cNvPr id="2" name="Slide Image {slide_img_id}"/><p:cNvSpPr><a:spLocks noGrp="1" noRot="1" noChangeAspect="1"/></p:cNvSpPr><p:nvPr><p:ph type="sldImg"/></p:nvPr></p:nvSpPr><p:spPr/></p:sp><p:sp><p:nvSpPr><p:cNvPr id="3" name="Notes Placeholder"/><p:cNvSpPr><a:spLocks noGrp="1"/></p:cNvSpPr><p:nvPr><p:ph type="body" idx="1"/></p:nvPr></p:nvSpPr><p:spPr/><p:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:rPr lang="en-US"/><a:t>{}</a:t></a:r></a:p></p:txBody></p:sp></p:spTree></p:cSld><p:clrMapOvr><a:masterClrMapping/></p:clrMapOvr></p:notes>"#,
        esc(text)
    )
}

fn table_frame(rows: &[[&str; 3]]) -> String {
    let mut grid = String::new();
    for _ in 0..3 {
        grid.push_str(r#"<a:gridCol w="2438400"/>"#);
    }
    let mut trs = String::new();
    for row in rows {
        trs.push_str(r#"<a:tr h="370840">"#);
        for cell in row {
            trs.push_str(&format!(
                r#"<a:tc><a:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:rPr lang="en-US"/><a:t>{}</a:t></a:r></a:p></a:txBody><a:tcPr/></a:tc>"#,
                esc(cell)
            ));
        }
        trs.push_str("</a:tr>");
    }
    format!(
        r#"<p:graphicFrame><p:nvGraphicFramePr><p:cNvPr id="4" name="Table 1"/><p:cNvGraphicFramePr><a:graphicFrameLocks noGrp="1"/></p:cNvGraphicFramePr><p:nvPr/></p:nvGraphicFramePr><p:xfrm><a:off x="914400" y="1828800"/><a:ext cx="7315200" cy="1112520"/></p:xfrm><a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/table"><a:tbl><a:tblPr firstRow="1" bandRow="1"/><a:tblGrid>{grid}</a:tblGrid>{trs}</a:tbl></a:graphicData></a:graphic></p:graphicFrame>"#
    )
}

fn structured_pptx(png: &[u8]) -> Res<Vec<u8>> {
    let title_pos = xfrm(457200, 274638, 8229600, 1143000);
    let body_pos = xfrm(457200, 1600200, 8229600, 4525963);
    // Slide 1: title + nested bullets.
    let s1 = slide_xml(&format!(
        "{}{}",
        shape(
            2,
            "Title 1",
            r#"type="title""#,
            "",
            &[(0, "Project Overview")]
        ),
        shape(
            3,
            "Content 2",
            r#"idx="1""#,
            "",
            &[
                (0, "Goals"),
                (1, "Faster conversions"),
                (1, "Fewer clicks"),
                (2, "Details matter"),
                (0, "Timeline"),
            ],
        ),
    ));
    // Slide 2: the note box comes first in the XML but sits below the table:
    // reading order by position puts it last.
    let s2 = slide_xml(&format!(
        "{}{}{}",
        shape(
            5,
            "Note box",
            "",
            &xfrm(914400, 5486400, 7315200, 457200),
            &[(0, "Numbers in EUR | estimates")],
        ),
        shape(2, "Title 1", r#"type="title""#, "", &[(0, "Budget")]),
        table_frame(&[
            ["Item", "Q1", "Q2"],
            ["Coffee", "12.5", "14"],
            ["Paper", "3", "4"],
        ]),
    ));
    // Slide 3: picture with alt text + a text box with a link.
    let link_box = format!(
        r#"<p:sp><p:nvSpPr><p:cNvPr id="6" name="Link box"/><p:cNvSpPr txBox="1"/><p:nvPr/></p:nvSpPr><p:spPr>{}</p:spPr><p:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:rPr lang="en-US"/><a:t>Visit </a:t></a:r><a:r><a:rPr lang="en-US"><a:hlinkClick r:id="rId3"/></a:rPr><a:t>the Morph site</a:t></a:r></a:p></p:txBody></p:sp>"#,
        xfrm(914400, 5486400, 7315200, 457200)
    );
    let picture = format!(
        r#"<p:pic><p:nvPicPr><p:cNvPr id="4" name="Picture 3" descr="Team photo at the office"/><p:cNvPicPr><a:picLocks noChangeAspect="1"/></p:cNvPicPr><p:nvPr/></p:nvPicPr><p:blipFill><a:blip r:embed="rId4"/><a:stretch><a:fillRect/></a:stretch></p:blipFill><p:spPr>{}<a:prstGeom prst="rect"><a:avLst/></a:prstGeom></p:spPr></p:pic>"#,
        xfrm(2743200, 1828800, 2743200, 1828800)
    );
    let s3 = slide_xml(&format!(
        "{}{picture}{link_box}",
        shape(2, "Title 1", r#"type="title""#, "", &[(0, "Team photo")]),
    ));
    let slides = [s1, s2, s3];
    let notes = [
        notes_xml(1, "Welcome everyone. Mention the caf\u{e9} meeting."),
        notes_xml(2, "Numbers are estimates."),
        notes_xml(3, "Thank you."),
    ];

    let master = format!(
        r#"{XML}<p:sldMaster {P_NS}><p:cSld><p:bg><p:bgRef idx="1001"><a:schemeClr val="bg1"/></p:bgRef></p:bg><p:spTree>{GROUP}{}{}</p:spTree></p:cSld><p:clrMap bg1="lt1" tx1="dk1" bg2="lt2" tx2="dk2" accent1="accent1" accent2="accent2" accent3="accent3" accent4="accent4" accent5="accent5" accent6="accent6" hlink="hlink" folHlink="folHlink"/><p:sldLayoutIdLst><p:sldLayoutId id="2147483649" r:id="rId1"/></p:sldLayoutIdLst><p:txStyles><p:titleStyle><a:lvl1pPr><a:defRPr sz="4400"/></a:lvl1pPr></p:titleStyle><p:bodyStyle><a:lvl1pPr marL="342900" indent="-342900"><a:buChar char="&#8226;"/><a:defRPr sz="3200"/></a:lvl1pPr><a:lvl2pPr marL="742950" indent="-285750"><a:buChar char="&#8211;"/><a:defRPr sz="2800"/></a:lvl2pPr><a:lvl3pPr marL="1143000" indent="-228600"><a:buChar char="&#8226;"/><a:defRPr sz="2400"/></a:lvl3pPr></p:bodyStyle><p:otherStyle><a:lvl1pPr><a:defRPr sz="1800"/></a:lvl1pPr></p:otherStyle></p:txStyles></p:sldMaster>"#,
        shape(2, "Title Placeholder", r#"type="title""#, &title_pos, &[]),
        shape(
            3,
            "Text Placeholder",
            r#"type="body" idx="1""#,
            &body_pos,
            &[]
        ),
    );
    let layout = format!(
        r#"{XML}<p:sldLayout {P_NS} type="obj" preserve="1"><p:cSld name="Title and Content"><p:spTree>{GROUP}{}{}</p:spTree></p:cSld><p:clrMapOvr><a:masterClrMapping/></p:clrMapOvr></p:sldLayout>"#,
        shape(2, "Title 1", r#"type="title""#, &title_pos, &[]),
        shape(3, "Content Placeholder 2", r#"idx="1""#, &body_pos, &[]),
    );
    let notes_master = format!(
        r#"{XML}<p:notesMaster {P_NS}><p:cSld><p:spTree>{GROUP}</p:spTree></p:cSld><p:clrMap bg1="lt1" tx1="dk1" bg2="lt2" tx2="dk2" accent1="accent1" accent2="accent2" accent3="accent3" accent4="accent4" accent5="accent5" accent6="accent6" hlink="hlink" folHlink="folHlink"/></p:notesMaster>"#
    );
    let presentation = format!(
        r#"{XML}<p:presentation {P_NS}><p:sldMasterIdLst><p:sldMasterId id="2147483648" r:id="rId1"/></p:sldMasterIdLst><p:notesMasterIdLst><p:notesMasterId r:id="rId5"/></p:notesMasterIdLst><p:sldIdLst><p:sldId id="256" r:id="rId2"/><p:sldId id="257" r:id="rId3"/><p:sldId id="258" r:id="rId4"/></p:sldIdLst><p:sldSz cx="9144000" cy="6858000" type="screen4x3"/><p:notesSz cx="6858000" cy="9144000"/></p:presentation>"#
    );
    let pres_rels = rels(&[
        (
            "rId1",
            "slideMaster",
            "slideMasters/slideMaster1.xml",
            false,
        ),
        ("rId2", "slide", "slides/slide1.xml", false),
        ("rId3", "slide", "slides/slide2.xml", false),
        ("rId4", "slide", "slides/slide3.xml", false),
        (
            "rId5",
            "notesMaster",
            "notesMasters/notesMaster1.xml",
            false,
        ),
        ("rId6", "theme", "theme/theme1.xml", false),
    ]);
    let mut overrides: Vec<(String, String)> = vec![
        (
            "/ppt/presentation.xml".into(),
            format!("{PML}.presentationml.presentation.main+xml"),
        ),
        (
            "/ppt/slideMasters/slideMaster1.xml".into(),
            format!("{PML}.presentationml.slideMaster+xml"),
        ),
        (
            "/ppt/slideLayouts/slideLayout1.xml".into(),
            format!("{PML}.presentationml.slideLayout+xml"),
        ),
        (
            "/ppt/notesMasters/notesMaster1.xml".into(),
            format!("{PML}.presentationml.notesMaster+xml"),
        ),
        ("/ppt/theme/theme1.xml".into(), format!("{PML}.theme+xml")),
        ("/ppt/theme/theme2.xml".into(), format!("{PML}.theme+xml")),
    ];
    for n in 1..=3 {
        overrides.push((
            format!("/ppt/slides/slide{n}.xml"),
            format!("{PML}.presentationml.slide+xml"),
        ));
        overrides.push((
            format!("/ppt/notesSlides/notesSlide{n}.xml"),
            format!("{PML}.presentationml.notesSlide+xml"),
        ));
    }
    let overrides_ref: Vec<(&str, &str)> = overrides
        .iter()
        .map(|(a, b)| (a.as_str(), b.as_str()))
        .collect();
    let ct = content_types(&overrides_ref);
    let root_rels = rels(&[("rId1", "officeDocument", "ppt/presentation.xml", false)]);
    let master_rels = rels(&[
        (
            "rId1",
            "slideLayout",
            "../slideLayouts/slideLayout1.xml",
            false,
        ),
        ("rId2", "theme", "../theme/theme1.xml", false),
    ]);
    let layout_rels = rels(&[(
        "rId1",
        "slideMaster",
        "../slideMasters/slideMaster1.xml",
        false,
    )]);
    let notes_master_rels = rels(&[("rId1", "theme", "../theme/theme2.xml", false)]);
    let slide_rels: Vec<String> = (1..=3)
        .map(|n| {
            let notes = format!("../notesSlides/notesSlide{n}.xml");
            let mut items = vec![
                (
                    "rId1",
                    "slideLayout",
                    "../slideLayouts/slideLayout1.xml",
                    false,
                ),
                ("rId2", "notesSlide", notes.as_str(), false),
            ];
            if n == 3 {
                items.push(("rId3", "hyperlink", "https://example.com/morph", true));
                items.push(("rId4", "image", "../media/image1.png", false));
            }
            rels(&items)
        })
        .collect();
    let notes_rels: Vec<String> = (1..=3)
        .map(|n| {
            let slide = format!("../slides/slide{n}.xml");
            rels(&[
                (
                    "rId1",
                    "notesMaster",
                    "../notesMasters/notesMaster1.xml",
                    false,
                ),
                ("rId2", "slide", slide.as_str(), false),
            ])
        })
        .collect();
    let theme = theme();
    let names: Vec<[String; 4]> = (1..=3)
        .map(|n| {
            [
                format!("ppt/slides/slide{n}.xml"),
                format!("ppt/slides/_rels/slide{n}.xml.rels"),
                format!("ppt/notesSlides/notesSlide{n}.xml"),
                format!("ppt/notesSlides/_rels/notesSlide{n}.xml.rels"),
            ]
        })
        .collect();
    let mut entries: Vec<(&str, &[u8])> = vec![
        ("[Content_Types].xml", ct.as_bytes()),
        ("_rels/.rels", root_rels.as_bytes()),
        ("ppt/presentation.xml", presentation.as_bytes()),
        ("ppt/_rels/presentation.xml.rels", pres_rels.as_bytes()),
        ("ppt/slideMasters/slideMaster1.xml", master.as_bytes()),
        (
            "ppt/slideMasters/_rels/slideMaster1.xml.rels",
            master_rels.as_bytes(),
        ),
        ("ppt/slideLayouts/slideLayout1.xml", layout.as_bytes()),
        (
            "ppt/slideLayouts/_rels/slideLayout1.xml.rels",
            layout_rels.as_bytes(),
        ),
        ("ppt/notesMasters/notesMaster1.xml", notes_master.as_bytes()),
        (
            "ppt/notesMasters/_rels/notesMaster1.xml.rels",
            notes_master_rels.as_bytes(),
        ),
        ("ppt/theme/theme1.xml", theme.as_bytes()),
        ("ppt/theme/theme2.xml", theme.as_bytes()),
        ("ppt/media/image1.png", png),
    ];
    for i in 0..3 {
        entries.push((names[i][0].as_str(), slides[i].as_bytes()));
        entries.push((names[i][1].as_str(), slide_rels[i].as_bytes()));
        entries.push((names[i][2].as_str(), notes[i].as_bytes()));
        entries.push((names[i][3].as_str(), notes_rels[i].as_bytes()));
    }
    zip(&entries)
}

// ---------------------------------------------------------------- XLSX

fn structured_xlsx(path: &Path) -> Res<()> {
    let mut wb = Workbook::new();
    let created = ExcelDateTime::from_ymd(2026, 1, 1)?.and_hms(0, 0, 0)?;
    wb.set_properties(
        &DocProperties::new()
            .set_title("Morph two-way workbook")
            .set_creation_datetime(&created),
    );
    let bold = Format::new().set_bold();
    let date = Format::new().set_num_format("yyyy-mm-dd");
    let money = Format::new().set_num_format("0.00");
    let fill = Format::new().set_background_color("FFFF00");

    let data = wb.add_worksheet().set_name("Data")?;
    data.write_row_with_format(0, 0, ["Item", "Count", "Price", "Date", "Note"], &bold)?;
    type Row<'a> = (&'a str, f64, f64, (u16, u8, u8), &'a str);
    let rows: [Row; 4] = [
        ("Apples", 3.0, 2.5, (2026, 3, 14), "fresh"),
        ("Pears", 12.0, 0.75, (2026, 4, 1), "a|b pipe"),
        (
            "Caf\u{e9}",
            1.0,
            1234.5,
            (2025, 12, 31),
            "line one\nline two",
        ),
        ("\u{6771}\u{4EAC}", 0.0, -3.125, (2026, 1, 2), ""),
    ];
    for (i, (item, count, price, (y, m, d), note)) in rows.iter().enumerate() {
        let r = i as u32 + 1;
        data.write_string(r, 0, *item)?;
        data.write_number(r, 1, *count)?;
        data.write_number_with_format(r, 2, *price, &money)?;
        data.write_datetime_with_format(r, 3, ExcelDateTime::from_ymd(*y, *m, *d)?, &date)?;
        if !note.is_empty() {
            data.write_string(r, 4, *note)?;
        }
    }
    // Formatted but empty cells below and right of the data: the used range
    // grows, the Markdown must not (empty edges trimmed).
    for r in 5..9 {
        data.write_blank(r, 0, &fill)?;
    }
    data.write_blank(2, 6, &fill)?;

    let summary = wb.add_worksheet().set_name("Summary")?;
    summary.write_string(0, 0, "Total count")?;
    summary.write_formula(0, 1, Formula::new("=SUM(Data!B2:B5)").set_result("16"))?;
    summary.write_string(1, 0, "Average price")?;
    summary.write_number(1, 1, 308.4)?;
    summary.write_string(2, 0, "Checked")?;
    summary.write_boolean(2, 1, true)?;

    // An empty sheet: skipped in the Markdown.
    wb.add_worksheet().set_name("Empty")?;
    wb.save(path)?;
    Ok(())
}

// ---------------------------------------------------------------- HTML

/// 1x1 red PNG.
const DATA_PNG: &str =
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAIAAACQd1PeAAAADElEQVR4nGP4z8AAAAMBAQDJ/pLvAAAAAElFTkSuQmCC";

fn structured_html() -> String {
    format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<title>Morph structured page</title>
<style>body {{ font-family: sans-serif; }} nav a {{ color: red; }}</style>
<script>window.tracking = "should never appear";</script>
</head>
<body>
<header><nav><a href="/">Home</a> | <a href="/about">About</a> | <a href="/contact">Contact</a></nav></header>
<noscript>Please enable JavaScript (should never appear).</noscript>
<main>
<article>
<h1>Structured page</h1>
<p>This page has <strong>bold</strong>, <em>italic</em> and a <a href="https://example.com/page">link</a>. Caf&eacute; &amp; 報告.</p>
<h2>List</h2>
<ul><li>One</li><li>Two<ul><li>Two point one</li></ul></li></ul>
<h2>Table</h2>
<table><tr><th>Name</th><th>Value</th></tr><tr><td>a|b</td><td>1</td></tr><tr><td>c</td><td>2</td></tr></table>
<h2>Picture</h2>
<p><img src="data:image/png;base64,{DATA_PNG}" alt="Red dot"></p>
</article>
</main>
<footer><nav><a href="/privacy">Privacy</a></nav><p>Footer text (page chrome)</p></footer>
<script>console.log("bye");</script>
</body>
</html>
"#
    )
}

fn main() -> Res<()> {
    let dir = fixtures_dir();
    let png = std::fs::read(dir.join("sample.png"))?;
    save("structured.pdf", &structured_pdf()?)?;
    save("structured.docx", &structured_docx(&png)?)?;
    save("structured.pptx", &structured_pptx(&png)?)?;
    let xlsx = dir.join("structured.xlsx");
    structured_xlsx(&xlsx)?;
    println!(
        "{:>9} bytes  {}",
        std::fs::metadata(&xlsx)?.len(),
        xlsx.display()
    );
    save("structured.html", structured_html().as_bytes())?;
    Ok(())
}
