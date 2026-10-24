// SPDX-License-Identifier: GPL-3.0-or-later
//! Regenerates the Office / text-like test fixtures in `tests/fixtures/`
//! (see the README there).
//!
//!     cargo run -p morph-core --example make_office_fixtures
//!
//! No Office or LibreOffice needed: DOCX/PPTX/ODT are minimal hand-built zips,
//! XLSX comes from rust_xlsxwriter, the rest is plain text. Zip timestamps and
//! document dates are fixed, so the bytes are the same on every run.

use rust_xlsxwriter::{DocProperties, ExcelDateTime, Format, Workbook};
use std::error::Error;
use std::io::Write;
use std::path::{Path, PathBuf};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, DateTime, ZipWriter};

type Res<T> = Result<T, Box<dyn Error>>;

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures")
}

/// Build a zip from `(name, bytes, stored)` entries, in order, with fixed dates.
fn zip(entries: &[(&str, &[u8], bool)]) -> Res<Vec<u8>> {
    let mut w = ZipWriter::new(std::io::Cursor::new(Vec::new()));
    for (name, bytes, stored) in entries {
        let method = if *stored {
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
const RELS_CT: &str = "application/vnd.openxmlformats-package.relationships+xml";

fn rels(items: &[(&str, &str, &str)]) -> String {
    let mut s = format!(
        r#"{XML}<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">"#
    );
    for (id, kind, target) in items {
        s.push_str(&format!(
            r#"<Relationship Id="{id}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/{kind}" Target="{target}"/>"#
        ));
    }
    s.push_str("</Relationships>");
    s
}

// ---------------------------------------------------------------- DOCX

const W_NS: &str = r#"xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:pic="http://schemas.openxmlformats.org/drawingml/2006/picture""#;

fn w_para(style: Option<&str>, num: bool, text: &str) -> String {
    let mut ppr = String::new();
    if let Some(s) = style {
        ppr.push_str(&format!(r#"<w:pStyle w:val="{s}"/>"#));
    }
    if num {
        ppr.push_str(r#"<w:numPr><w:ilvl w:val="0"/><w:numId w:val="1"/></w:numPr>"#);
    }
    let ppr = if ppr.is_empty() {
        String::new()
    } else {
        format!("<w:pPr>{ppr}</w:pPr>")
    };
    format!(r#"<w:p>{ppr}<w:r><w:t xml:space="preserve">{text}</w:t></w:r></w:p>"#)
}

fn w_table(rows: &[[&str; 3]]) -> String {
    let mut s = String::from(
        r#"<w:tbl><w:tblPr><w:tblStyle w:val="TableGrid"/><w:tblW w:w="0" w:type="auto"/></w:tblPr><w:tblGrid><w:gridCol w:w="3000"/><w:gridCol w:w="3000"/><w:gridCol w:w="3000"/></w:tblGrid>"#,
    );
    for row in rows {
        s.push_str("<w:tr>");
        for cell in row {
            s.push_str(&format!(
                r#"<w:tc><w:tcPr><w:tcW w:w="3000" w:type="dxa"/></w:tcPr>{}</w:tc>"#,
                w_para(None, false, cell)
            ));
        }
        s.push_str("</w:tr>");
    }
    s.push_str("</w:tbl>");
    s
}

fn docx(png: &[u8]) -> Res<Vec<u8>> {
    let content_types = format!(
        r#"{XML}<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="{RELS_CT}"/><Default Extension="xml" ContentType="application/xml"/><Default Extension="png" ContentType="image/png"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/><Override PartName="/word/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml"/><Override PartName="/word/numbering.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.numbering+xml"/></Types>"#
    );
    let root_rels = rels(&[("rId1", "officeDocument", "word/document.xml")]);
    let doc_rels = rels(&[
        ("rId1", "styles", "styles.xml"),
        ("rId2", "numbering", "numbering.xml"),
        ("rId3", "image", "media/image1.png"),
    ]);
    let styles = format!(
        r#"{XML}<w:styles {W_NS}><w:docDefaults><w:rPrDefault><w:rPr><w:rFonts w:ascii="Calibri" w:hAnsi="Calibri" w:eastAsia="MS Mincho" w:cs="Calibri"/><w:sz w:val="22"/><w:lang w:val="en-US"/></w:rPr></w:rPrDefault><w:pPrDefault><w:pPr><w:spacing w:after="120"/></w:pPr></w:pPrDefault></w:docDefaults><w:style w:type="paragraph" w:default="1" w:styleId="Normal"><w:name w:val="Normal"/><w:qFormat/></w:style><w:style w:type="paragraph" w:styleId="Heading1"><w:name w:val="heading 1"/><w:basedOn w:val="Normal"/><w:next w:val="Normal"/><w:qFormat/><w:pPr><w:keepNext/><w:spacing w:before="240" w:after="120"/><w:outlineLvl w:val="0"/></w:pPr><w:rPr><w:b/><w:sz w:val="32"/></w:rPr></w:style><w:style w:type="paragraph" w:styleId="Heading2"><w:name w:val="heading 2"/><w:basedOn w:val="Normal"/><w:next w:val="Normal"/><w:qFormat/><w:pPr><w:keepNext/><w:outlineLvl w:val="1"/></w:pPr><w:rPr><w:b/><w:sz w:val="26"/></w:rPr></w:style><w:style w:type="paragraph" w:styleId="ListParagraph"><w:name w:val="List Paragraph"/><w:basedOn w:val="Normal"/><w:pPr><w:ind w:left="720"/></w:pPr></w:style><w:style w:type="table" w:styleId="TableGrid"><w:name w:val="Table Grid"/><w:tblPr><w:tblBorders><w:top w:val="single" w:sz="4" w:space="0" w:color="000000"/><w:left w:val="single" w:sz="4" w:space="0" w:color="000000"/><w:bottom w:val="single" w:sz="4" w:space="0" w:color="000000"/><w:right w:val="single" w:sz="4" w:space="0" w:color="000000"/><w:insideH w:val="single" w:sz="4" w:space="0" w:color="000000"/><w:insideV w:val="single" w:sz="4" w:space="0" w:color="000000"/></w:tblBorders></w:tblPr></w:style></w:styles>"#
    );
    let numbering = format!(
        r#"{XML}<w:numbering {W_NS}><w:abstractNum w:abstractNumId="0"><w:multiLevelType w:val="singleLevel"/><w:lvl w:ilvl="0"><w:start w:val="1"/><w:numFmt w:val="bullet"/><w:lvlText w:val="•"/><w:lvlJc w:val="left"/><w:pPr><w:ind w:left="720" w:hanging="360"/></w:pPr></w:lvl></w:abstractNum><w:num w:numId="1"><w:abstractNumId w:val="0"/></w:num></w:numbering>"#
    );
    // 96x64 px at 96 DPI = 1 x 0.667 inch (914400 EMU per inch).
    let image = r#"<w:p><w:r><w:drawing><wp:inline distT="0" distB="0" distL="0" distR="0"><wp:extent cx="914400" cy="609600"/><wp:docPr id="1" name="Picture 1" descr="Sample picture"/><a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/picture"><pic:pic><pic:nvPicPr><pic:cNvPr id="1" name="image1.png"/><pic:cNvPicPr/></pic:nvPicPr><pic:blipFill><a:blip r:embed="rId3"/><a:stretch><a:fillRect/></a:stretch></pic:blipFill><pic:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="914400" cy="609600"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></pic:spPr></pic:pic></a:graphicData></a:graphic></wp:inline></w:drawing></w:r></w:p>"#;
    let mut body = String::new();
    body.push_str(&w_para(Some("Heading1"), false, "Quarterly report"));
    body.push_str(&w_para(
        None,
        false,
        "Accented words: café, relatório, ação. CJK: 報告.",
    ));
    body.push_str(&w_para(Some("Heading2"), false, "Highlights"));
    for item in ["First point", "Second point", "Third point"] {
        body.push_str(&w_para(Some("ListParagraph"), true, item));
    }
    body.push_str(&w_para(Some("Heading2"), false, "Numbers"));
    body.push_str(&w_table(&[
        ["Region", "Q1", "Q2"],
        ["North", "120", "135"],
        ["South", "98", "110"],
    ]));
    body.push_str(&w_para(Some("Heading2"), false, "Picture"));
    body.push_str(image);
    // US Letter, 1-inch margins.
    body.push_str(r#"<w:sectPr><w:pgSz w:w="12240" w:h="15840"/><w:pgMar w:top="1440" w:right="1440" w:bottom="1440" w:left="1440" w:header="720" w:footer="720" w:gutter="0"/></w:sectPr>"#);
    let document = format!(r#"{XML}<w:document {W_NS}><w:body>{body}</w:body></w:document>"#);
    zip(&[
        ("[Content_Types].xml", content_types.as_bytes(), false),
        ("_rels/.rels", root_rels.as_bytes(), false),
        ("word/document.xml", document.as_bytes(), false),
        ("word/_rels/document.xml.rels", doc_rels.as_bytes(), false),
        ("word/styles.xml", styles.as_bytes(), false),
        ("word/numbering.xml", numbering.as_bytes(), false),
        ("word/media/image1.png", png, true),
    ])
}

// ---------------------------------------------------------------- PPTX

const P_NS: &str = r#"xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main""#;
const EMPTY_TREE: &str = r#"<p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="0" cy="0"/><a:chOff x="0" y="0"/><a:chExt cx="0" cy="0"/></a:xfrm></p:grpSpPr>"#;

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

fn slide(n: usize, total: usize, color: &str) -> String {
    format!(
        r#"{XML}<p:sld {P_NS}><p:cSld><p:spTree>{EMPTY_TREE}<p:sp><p:nvSpPr><p:cNvPr id="2" name="Title {n}"/><p:cNvSpPr txBox="1"/><p:nvPr/></p:nvSpPr><p:spPr><a:xfrm><a:off x="457200" y="457200"/><a:ext cx="8229600" cy="1143000"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></p:spPr><p:txBody><a:bodyPr wrap="square"/><a:lstStyle/><a:p><a:r><a:rPr lang="en-US" sz="4000" b="1"/><a:t>Slide {n} of {total}</a:t></a:r></a:p><a:p><a:r><a:rPr lang="pt-BR" sz="2000"/><a:t>Apresentação café ação 報告</a:t></a:r></a:p></p:txBody></p:sp><p:sp><p:nvSpPr><p:cNvPr id="3" name="Box {n}"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr><p:spPr><a:xfrm><a:off x="914400" y="2286000"/><a:ext cx="7315200" cy="3657600"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom><a:solidFill><a:srgbClr val="{color}"/></a:solidFill></p:spPr></p:sp></p:spTree></p:cSld><p:clrMapOvr><a:masterClrMapping/></p:clrMapOvr></p:sld>"#
    )
}

fn pptx() -> Res<Vec<u8>> {
    const SLIDES: usize = 3;
    let mut ct = format!(
        r#"{XML}<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="{RELS_CT}"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/ppt/presentation.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.presentation.main+xml"/><Override PartName="/ppt/slideMasters/slideMaster1.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.slideMaster+xml"/><Override PartName="/ppt/slideLayouts/slideLayout1.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.slideLayout+xml"/><Override PartName="/ppt/theme/theme1.xml" ContentType="application/vnd.openxmlformats-officedocument.theme+xml"/>"#
    );
    for n in 1..=SLIDES {
        ct.push_str(&format!(
            r#"<Override PartName="/ppt/slides/slide{n}.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.slide+xml"/>"#
        ));
    }
    ct.push_str("</Types>");
    let root_rels = rels(&[("rId1", "officeDocument", "ppt/presentation.xml")]);
    let mut pres_rels: Vec<(String, &str, String)> = vec![(
        "rId1".into(),
        "slideMaster",
        "slideMasters/slideMaster1.xml".into(),
    )];
    let mut sld_ids = String::new();
    for n in 1..=SLIDES {
        pres_rels.push((
            format!("rId{}", n + 1),
            "slide",
            format!("slides/slide{n}.xml"),
        ));
        sld_ids.push_str(&format!(
            r#"<p:sldId id="{}" r:id="rId{}"/>"#,
            255 + n,
            n + 1
        ));
    }
    pres_rels.push((
        format!("rId{}", SLIDES + 2),
        "theme",
        "theme/theme1.xml".into(),
    ));
    let pres_rels: Vec<(&str, &str, &str)> = pres_rels
        .iter()
        .map(|(a, b, c)| (a.as_str(), *b, c.as_str()))
        .collect();
    let presentation = format!(
        r#"{XML}<p:presentation {P_NS}><p:sldMasterIdLst><p:sldMasterId id="2147483648" r:id="rId1"/></p:sldMasterIdLst><p:sldIdLst>{sld_ids}</p:sldIdLst><p:sldSz cx="9144000" cy="6858000" type="screen4x3"/><p:notesSz cx="6858000" cy="9144000"/></p:presentation>"#
    );
    let master = format!(
        r#"{XML}<p:sldMaster {P_NS}><p:cSld><p:bg><p:bgRef idx="1001"><a:schemeClr val="bg1"/></p:bgRef></p:bg><p:spTree>{EMPTY_TREE}</p:spTree></p:cSld><p:clrMap bg1="lt1" tx1="dk1" bg2="lt2" tx2="dk2" accent1="accent1" accent2="accent2" accent3="accent3" accent4="accent4" accent5="accent5" accent6="accent6" hlink="hlink" folHlink="folHlink"/><p:sldLayoutIdLst><p:sldLayoutId id="2147483649" r:id="rId1"/></p:sldLayoutIdLst><p:txStyles><p:titleStyle><a:lvl1pPr><a:defRPr sz="4400"/></a:lvl1pPr></p:titleStyle><p:bodyStyle><a:lvl1pPr><a:defRPr sz="2000"/></a:lvl1pPr></p:bodyStyle><p:otherStyle><a:lvl1pPr><a:defRPr sz="1800"/></a:lvl1pPr></p:otherStyle></p:txStyles></p:sldMaster>"#
    );
    let master_rels = rels(&[
        ("rId1", "slideLayout", "../slideLayouts/slideLayout1.xml"),
        ("rId2", "theme", "../theme/theme1.xml"),
    ]);
    let layout = format!(
        r#"{XML}<p:sldLayout {P_NS} type="blank" preserve="1"><p:cSld name="Blank"><p:spTree>{EMPTY_TREE}</p:spTree></p:cSld><p:clrMapOvr><a:masterClrMapping/></p:clrMapOvr></p:sldLayout>"#
    );
    let layout_rels = rels(&[("rId1", "slideMaster", "../slideMasters/slideMaster1.xml")]);
    let slide_rels = rels(&[("rId1", "slideLayout", "../slideLayouts/slideLayout1.xml")]);
    let colors = ["C0504D", "9BBB59", "4F81BD"];
    let slides: Vec<String> = (1..=SLIDES)
        .map(|n| slide(n, SLIDES, colors[(n - 1) % colors.len()]))
        .collect();
    let theme = theme();
    let pres_rels = rels(&pres_rels);

    let slide_names: Vec<(String, String)> = (1..=SLIDES)
        .map(|n| {
            (
                format!("ppt/slides/slide{n}.xml"),
                format!("ppt/slides/_rels/slide{n}.xml.rels"),
            )
        })
        .collect();
    let mut entries: Vec<(&str, &[u8], bool)> = vec![
        ("[Content_Types].xml", ct.as_bytes(), false),
        ("_rels/.rels", root_rels.as_bytes(), false),
        ("ppt/presentation.xml", presentation.as_bytes(), false),
        (
            "ppt/_rels/presentation.xml.rels",
            pres_rels.as_bytes(),
            false,
        ),
        (
            "ppt/slideMasters/slideMaster1.xml",
            master.as_bytes(),
            false,
        ),
        (
            "ppt/slideMasters/_rels/slideMaster1.xml.rels",
            master_rels.as_bytes(),
            false,
        ),
        (
            "ppt/slideLayouts/slideLayout1.xml",
            layout.as_bytes(),
            false,
        ),
        (
            "ppt/slideLayouts/_rels/slideLayout1.xml.rels",
            layout_rels.as_bytes(),
            false,
        ),
        ("ppt/theme/theme1.xml", theme.as_bytes(), false),
    ];
    for (i, (name, rel_name)) in slide_names.iter().enumerate() {
        entries.push((name.as_str(), slides[i].as_bytes(), false));
        entries.push((rel_name.as_str(), slide_rels.as_bytes(), false));
    }
    zip(&entries)
}

// ---------------------------------------------------------------- ODT

fn odt() -> Res<Vec<u8>> {
    let manifest = format!(
        r#"{XML}<manifest:manifest xmlns:manifest="urn:oasis:names:tc:opendocument:xmlns:manifest:1.0" manifest:version="1.2"><manifest:file-entry manifest:full-path="/" manifest:version="1.2" manifest:media-type="application/vnd.oasis.opendocument.text"/><manifest:file-entry manifest:full-path="content.xml" manifest:media-type="text/xml"/></manifest:manifest>"#
    );
    let content = format!(
        r#"{XML}<office:document-content xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0" xmlns:table="urn:oasis:names:tc:opendocument:xmlns:table:1.0" office:version="1.2"><office:body><office:text><text:h text:outline-level="1">Meeting notes</text:h><text:p>Accented words: café, relatório, ação. CJK: 報告.</text:p><text:list><text:list-item><text:p>Agenda item one</text:p></text:list-item><text:list-item><text:p>Agenda item two</text:p></text:list-item></text:list><table:table table:name="Budget"><table:table-column table:number-columns-repeated="2"/><table:table-row><table:table-cell office:value-type="string"><text:p>Item</text:p></table:table-cell><table:table-cell office:value-type="string"><text:p>Cost</text:p></table:table-cell></table:table-row><table:table-row><table:table-cell office:value-type="string"><text:p>Coffee</text:p></table:table-cell><table:table-cell office:value-type="float" office:value="12.5"><text:p>12.5</text:p></table:table-cell></table:table-row></table:table></office:text></office:body></office:document-content>"#
    );
    zip(&[
        ("mimetype", b"application/vnd.oasis.opendocument.text", true),
        ("META-INF/manifest.xml", manifest.as_bytes(), false),
        ("content.xml", content.as_bytes(), false),
    ])
}

// ---------------------------------------------------------------- XLSX

fn xlsx(path: &Path) -> Res<()> {
    let mut wb = Workbook::new();
    let date = ExcelDateTime::from_ymd(2026, 1, 1)?.and_hms(0, 0, 0)?;
    wb.set_properties(
        &DocProperties::new()
            .set_title("Morph test workbook")
            .set_creation_datetime(&date),
    );
    let bold = Format::new().set_bold();
    let sales = wb.add_worksheet().set_name("Sales")?;
    sales.write_row_with_format(0, 0, ["Region", "Q1", "Q2", "Total"], &bold)?;
    let rows: [(&str, f64, f64); 4] = [
        ("North", 120.0, 135.5),
        ("South", 98.0, 110.0),
        ("São Paulo", 210.25, 199.0),
        ("東京", 75.0, 80.0),
    ];
    for (i, (region, q1, q2)) in rows.iter().enumerate() {
        let r = i as u32 + 1;
        sales.write_string(r, 0, *region)?;
        sales.write_number(r, 1, *q1)?;
        sales.write_number(r, 2, *q2)?;
        sales.write_formula(r, 3, format!("=B{0}+C{0}", r + 1).as_str())?;
    }
    let notes = wb.add_worksheet().set_name("Notes")?;
    notes.write_string(0, 0, "Comments")?;
    notes.write_string(1, 0, "Café; \"quoted\", with comma")?;
    notes.write_string(2, 0, "Second line")?;
    wb.save(path)?;
    Ok(())
}

// ---------------------------------------------------------------- text

const MD: &str = "# Project notes

Accented words: café, relatório, ação. CJK: 報告.

## Table

| Item | Qty | Price |
|------|----:|------:|
| Coffee | 2 | 3.50 |
| Tea | 1 | 2.00 |

## Code

```rust
fn main() {
    println!(\"Hello, Morph\");
}
```

## Picture

![Sample picture](sample.png)

- First *point*
- Second **point**
- A [link](https://example.com)
";

const HTML: &str = "<!DOCTYPE html>
<html lang=\"en\">
<head>
<meta charset=\"utf-8\">
<title>Sample page</title>
<style>body { font-family: sans-serif; }</style>
<script>console.log('scripts are dropped');</script>
</head>
<body>
<h1>Sample page</h1>
<p>Accented words: café, relatório, ação. CJK: 報告.</p>
<h2>List</h2>
<ul><li>First point</li><li>Second <strong>point</strong></li></ul>
<h2>Table</h2>
<table border=\"1\">
<tr><th>Item</th><th>Qty</th></tr>
<tr><td>Coffee</td><td>2</td></tr>
<tr><td>Tea</td><td>1</td></tr>
</table>
<h2>Picture</h2>
<p><img src=\"sample.png\" alt=\"Sample picture\" width=\"96\" height=\"64\"></p>
<p>A <a href=\"https://example.com\">link</a> and a remote image that is never fetched:
<img src=\"https://example.com/remote.png\" alt=\"remote\"></p>
</body>
</html>
";

/// Windows line endings, as Notepad writes them.
const TXT: &str = "Plain text sample\r\n\r\nAccented words: café, relatório, ação. CJK: 報告.\r\nSecond line of the first paragraph.\r\n\r\nA line with <angle brackets> & ampersands * stars # hashes.\r\n";

fn csv_comma() -> String {
    "Name,City,Amount,Note\r\nAna,São Paulo,1234.5,\"Says \"\"hi\"\"\"\r\nBob,Zürich,99,\"comma, inside\"\r\n陳,東京,7,\r\n".to_owned()
}

/// Excel-style European CSV: UTF-8 BOM, semicolons, decimal commas.
fn csv_semicolon_bom() -> Vec<u8> {
    let mut out = vec![0xEF, 0xBB, 0xBF];
    out.extend_from_slice(
        "Name;City;Amount;Note\r\nAna;São Paulo;1234,5;\"Says \"\"hi\"\"\"\r\nBob;Zürich;99;\"semi; inside\"\r\n陳;東京;7;\r\n"
            .as_bytes(),
    );
    out
}

fn main() -> Res<()> {
    let dir = fixtures_dir();
    let png = std::fs::read(dir.join("sample.png"))?;
    let write = |name: &str, bytes: &[u8]| -> Res<()> {
        std::fs::write(dir.join(name), bytes)?;
        println!("wrote {name} ({} bytes)", bytes.len());
        Ok(())
    };

    let docx = docx(&png)?;
    write("sample.docx", &docx)?;
    write("Relatório 報告.docx", &docx)?;
    // Truncated zip: must fail with a plain-English "damaged" error.
    write("broken.docx", &docx[..docx.len().min(300)])?;
    write("sample.pptx", &pptx()?)?;
    write("sample.odt", &odt()?)?;
    xlsx(&dir.join("sample.xlsx"))?;
    println!("wrote sample.xlsx");
    write("sample.csv", csv_comma().as_bytes())?;
    write("semicolon-bom.csv", &csv_semicolon_bom())?;
    write("sample.md", MD.as_bytes())?;
    write("sample.html", HTML.as_bytes())?;
    write("sample.txt", TXT.as_bytes())?;
    Ok(())
}
