// SPDX-License-Identifier: GPL-3.0-or-later
//! Native CSV ↔ XLSX, text-like conversions, text → DOCX/PDF/ODT through
//! the engine and Word → Markdown.
//!
//! Engine tests run for real when LibreOffice is detected and print
//! "SKIPPED: …" otherwise.

use calamine::{Data, Reader, open_workbook_auto};
use morph_core::action::Action;
use morph_core::converter::{Converter, TaskCtx, TaskOutput};
use morph_core::error::ConvertError;
use morph_core::job::{Task, TaskId};
use morph_core::office::detect;
use morph_core::office::options::{CsvDelimiter, DocFormat};
use morph_core::office::{OfficeConvert, OfficeConvertOpts};
use morph_core::{CancelToken, OutputSpec};
use std::io::Read;
use std::path::{Path, PathBuf};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures")
        .join(name)
}

fn has_libreoffice() -> bool {
    let ok = detect::engines().libreoffice.is_some();
    if !ok {
        println!("SKIPPED: LibreOffice not installed");
    }
    ok
}

/// Copy fixtures into a fresh folder (outputs go next to them).
fn workspace(names: &[&str]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    for n in names {
        std::fs::copy(fixture(n), dir.path().join(n)).unwrap();
    }
    dir
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

fn convert(input: &Path, format: DocFormat) -> TaskOutput {
    run(
        input,
        OfficeConvertOpts {
            format,
            ..Default::default()
        },
    )
    .unwrap_or_else(|e| panic!("{} → {format:?}: {e}", input.display()))
}

fn read_utf8(path: &Path) -> String {
    String::from_utf8(std::fs::read(path).unwrap()).unwrap()
}

fn zip_text(path: &Path, part: &str) -> String {
    let mut z = zip::ZipArchive::new(std::fs::File::open(path).unwrap()).unwrap();
    let mut s = String::new();
    z.by_name(part).unwrap().read_to_string(&mut s).unwrap();
    s
}

fn zip_names(path: &Path) -> Vec<String> {
    let mut z = zip::ZipArchive::new(std::fs::File::open(path).unwrap()).unwrap();
    (0..z.len())
        .map(|i| z.by_index(i).unwrap().name().unwrap().into_owned())
        .collect()
}

fn sheet_cells(path: &Path) -> Vec<(String, Vec<Vec<Data>>)> {
    let mut wb = open_workbook_auto(path).unwrap();
    wb.sheet_names()
        .into_iter()
        .map(|name| {
            let range = wb.worksheet_range(&name).unwrap();
            let rows = range.rows().map(<[Data]>::to_vec).collect();
            (name, rows)
        })
        .collect()
}

fn s(text: &str) -> Data {
    Data::String(text.to_owned())
}

// ---------------------------------------------------------------- CSV ↔ XLSX

#[test]
fn both_csv_fixtures_become_the_same_typed_sheet() {
    let dir = workspace(&["sample.csv", "semicolon-bom.csv"]);
    for name in ["sample.csv", "semicolon-bom.csv"] {
        let out = convert(&dir.path().join(name), DocFormat::Xlsx);
        assert_eq!(out.outputs.len(), 1);
        let xlsx = &out.outputs[0];
        assert_eq!(xlsx.extension().unwrap(), "xlsx");
        let sheets = sheet_cells(xlsx);
        assert_eq!(sheets.len(), 1);
        let (sheet, rows) = &sheets[0];
        assert_eq!(
            sheet,
            Path::new(name).file_stem().unwrap().to_str().unwrap()
        );
        assert_eq!(rows[0], [s("Name"), s("City"), s("Amount"), s("Note")]);
        assert_eq!(
            rows[1][..3],
            [s("Ana"), s("São Paulo"), Data::Float(1234.5)]
        );
        assert_eq!(rows[1][3], s("Says \"hi\""));
        assert_eq!(rows[2][1], s("Zürich"));
        assert_eq!(rows[2][2], Data::Float(99.0));
        assert_eq!(rows[3][..3], [s("陳"), s("東京"), Data::Float(7.0)]);
    }
}

#[test]
fn semicolon_bom_round_trip_keeps_values_and_accents() {
    let dir = workspace(&["semicolon-bom.csv"]);
    let input = dir.path().join("semicolon-bom.csv");
    let xlsx = convert(&input, DocFormat::Xlsx).outputs.remove(0);
    let back = run(
        &xlsx,
        OfficeConvertOpts {
            format: DocFormat::Csv,
            delimiter: CsvDelimiter::Semicolon,
            csv_bom: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(back.outputs.len(), 1);
    let out = &back.outputs[0];
    assert_eq!(out.file_name().unwrap(), "semicolon-bom (1).csv");
    assert_ne!(out, &input, "never overwrites the original");
    // Same bytes as the original (BOM, `;`, decimal comma, quoting, CRLF).
    let original = std::fs::read(&input).unwrap();
    let result = std::fs::read(out).unwrap();
    assert!(result.starts_with(b"\xEF\xBB\xBF"));
    assert_eq!(
        String::from_utf8_lossy(&result).replace("\r\n", "\n"),
        String::from_utf8_lossy(&original).replace("\r\n", "\n")
    );
}

#[test]
fn leading_zero_ids_and_types_survive() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("ids.csv");
    std::fs::write(
        &input,
        "id\tphone\tqty\tdate\tcard\r\n00123\t+351 912 345 678\t3\t2024-01-31\t4111111111111111\r\n0\t=1+1\t-2.5\t2024-01-31 13:45\t\r\n",
    )
    .unwrap();
    let xlsx = convert(&input, DocFormat::Xlsx).outputs.remove(0);
    let rows = sheet_cells(&xlsx).remove(0).1;
    assert_eq!(rows[1][0], s("00123"));
    assert_eq!(rows[1][1], s("+351 912 345 678"));
    assert_eq!(rows[1][2], Data::Float(3.0));
    assert!(matches!(rows[1][3], Data::DateTime(_)), "{:?}", rows[1][3]);
    assert_eq!(rows[1][4], s("4111111111111111"));
    assert_eq!(rows[2][0], Data::Float(0.0));
    assert_eq!(rows[2][1], s("=1+1"), "text is never a formula");
    assert_eq!(rows[2][2], Data::Float(-2.5));

    // And back: IDs, dates and numbers read the same.
    let csv = convert(&xlsx, DocFormat::Csv).outputs.remove(0);
    let text = read_utf8(&csv);
    assert!(
        text.contains("00123,+351 912 345 678,3,2024-01-31,4111111111111111\r\n"),
        "{text}"
    );
    assert!(text.contains("0,=1+1,-2.5,2024-01-31 13:45:00,"), "{text}");
}

#[test]
fn windows_1252_csv_is_decoded() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("legacy.csv");
    // "Café;São" in Windows-1252, not valid UTF-8.
    std::fs::write(&input, b"Caf\xE9;S\xE3o\r\n1,5;2\r\n").unwrap();
    let xlsx = convert(&input, DocFormat::Xlsx).outputs.remove(0);
    let rows = sheet_cells(&xlsx).remove(0).1;
    assert_eq!(rows[0], [s("Café"), s("São")]);
    assert_eq!(rows[1], [Data::Float(1.5), Data::Float(2.0)]);
}

#[test]
fn workbook_with_two_sheets_gives_two_csvs() {
    let dir = workspace(&["sample.xlsx"]);
    let out = convert(&dir.path().join("sample.xlsx"), DocFormat::Csv);
    let names: Vec<_> = out
        .outputs
        .iter()
        .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, ["sample - Sales.csv", "sample - Notes.csv"]);
    let sales = read_utf8(&out.outputs[0]);
    assert!(sales.starts_with('\u{feff}'), "BOM by default, for Excel");
    assert!(
        sales.contains("São Paulo") && sales.contains("東京"),
        "{sales}"
    );
    // Formulas come out as their values: no '=' left.
    assert!(!sales.contains('='), "{sales}");
    let notes = read_utf8(&out.outputs[1]);
    assert!(notes.contains('"'), "{notes}");
}

#[test]
fn single_sheet_and_unicode_names() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("Relatório 報告.csv");
    std::fs::copy(fixture("sample.csv"), &input).unwrap();
    let xlsx = convert(&input, DocFormat::Xlsx).outputs.remove(0);
    assert_eq!(xlsx.file_name().unwrap(), "Relatório 報告.xlsx");
    let csv = convert(&xlsx, DocFormat::Csv).outputs.remove(0);
    assert_eq!(csv.file_name().unwrap(), "Relatório 報告 (1).csv");
    // Same text; Morph adds a BOM by default (the input has none).
    assert_eq!(
        read_utf8(&csv)
            .trim_start_matches('\u{feff}')
            .replace("\r\n", "\n"),
        read_utf8(&input).replace("\r\n", "\n")
    );
}

#[test]
fn ods_to_csv_through_calamine() {
    if !has_libreoffice() {
        return;
    }
    let dir = workspace(&["sample.xlsx"]);
    let ods = convert(&dir.path().join("sample.xlsx"), DocFormat::Ods)
        .outputs
        .remove(0);
    let out = convert(&ods, DocFormat::Csv);
    assert_eq!(out.outputs.len(), 2);
    assert!(read_utf8(&out.outputs[0]).contains("São Paulo"));
}

#[test]
fn damaged_spreadsheet_fails_plainly() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("broken.xlsx");
    std::fs::write(
        &input,
        &std::fs::read(fixture("sample.xlsx")).unwrap()[..300],
    )
    .unwrap();
    let err = run(
        &input,
        OfficeConvertOpts {
            format: DocFormat::Csv,
            ..Default::default()
        },
    )
    .unwrap_err();
    assert_eq!(
        err.message,
        "This file seems to be damaged or isn't a supported document."
    );
    assert_eq!(
        std::fs::read_dir(dir.path()).unwrap().count(),
        1,
        "nothing left behind"
    );
}

// ---------------------------------------------------------------- text-like, native

#[test]
fn markdown_to_html_is_a_full_page() {
    let dir = workspace(&["sample.md"]);
    let out = convert(&dir.path().join("sample.md"), DocFormat::Html);
    let html = read_utf8(&out.outputs[0]);
    assert!(html.contains("<meta charset=\"utf-8\">"));
    assert!(html.contains("<title>Project notes</title>"));
    assert!(html.contains("<h1>Project notes</h1>"));
    assert!(html.contains("<table>") && html.contains("<td>Coffee</td>"));
    assert!(html.contains("<code class=\"language-rust\">"));
    assert!(html.contains("src=\"sample.png\""), "relative image kept");
    assert!(html.contains("café") && html.contains("報告"));
    assert!(html.contains("<style>"));
}

#[test]
fn markdown_to_text() {
    let dir = workspace(&["sample.md"]);
    let out = convert(&dir.path().join("sample.md"), DocFormat::Txt);
    let text = read_utf8(&out.outputs[0]);
    assert!(text.starts_with("Project notes\r\n\r\n"), "{text}");
    assert!(text.contains("Coffee\t2\t3.50"), "{text}");
    assert!(text.contains("println!(\"Hello, Morph\");"));
    assert!(text.contains("- A link (https://example.com)"), "{text}");
    assert!(!text.contains("**") && !text.contains("##"));
}

#[test]
fn text_to_html_and_markdown() {
    let dir = workspace(&["sample.txt"]);
    let input = dir.path().join("sample.txt");
    let source = read_utf8(&input);
    let html = read_utf8(&convert(&input, DocFormat::Html).outputs[0]);
    assert!(html.contains("<p>"));
    assert!(html.contains("&lt;") && html.contains("&amp;"));
    assert!(!source.contains("<p>"));
    let md = read_utf8(&convert(&input, DocFormat::Md).outputs[0]);
    // Rendering the Markdown gives back the same words.
    assert!(md.contains("\\<") || !source.contains('<'), "{md}");
    for word in ["café", "報告"] {
        if source.contains(word) {
            assert!(html.contains(word) && md.contains(word));
        }
    }
}

#[test]
fn html_to_markdown_and_text() {
    let dir = workspace(&["sample.html"]);
    let input = dir.path().join("sample.html");
    let md = read_utf8(&convert(&input, DocFormat::Md).outputs[0]);
    assert!(md.starts_with("# Sample page"), "{md}");
    assert!(
        md.contains("## List") && md.contains("- First point"),
        "{md}"
    );
    assert!(
        md.contains("| Item ") && md.contains("| Coffee | 2 "),
        "{md}"
    );
    assert!(md.contains("![Sample picture](sample.png)"), "{md}");
    assert!(md.contains("[link](https://example.com)"), "{md}");
    assert!(
        !md.contains("console.log") && !md.contains("font-family"),
        "{md}"
    );
    assert!(md.contains("café") && md.contains("報告"));

    let text = read_utf8(&convert(&input, DocFormat::Txt).outputs[0]);
    assert!(text.starts_with("Sample page\r\n"), "{text}");
    assert!(text.contains("Coffee\t2"), "{text}");
    assert!(
        !text.contains("console.log") && !text.contains('#'),
        "{text}"
    );
}

#[test]
fn same_format_is_refused() {
    let dir = workspace(&["sample.md"]);
    let err = run(
        &dir.path().join("sample.md"),
        OfficeConvertOpts {
            format: DocFormat::Md,
            ..Default::default()
        },
    )
    .unwrap_err();
    assert_eq!(err.message, "This file already is Markdown.");
}

#[test]
fn unicode_text_names() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("Notas 報告.md");
    std::fs::copy(fixture("sample.md"), &input).unwrap();
    let out = convert(&input, DocFormat::Html);
    assert_eq!(out.outputs[0].file_name().unwrap(), "Notas 報告.html");
}

// ---------------------------------------------------------------- through the engine

fn pdf_text(pdf: &Path) -> String {
    use morph_core::pdf::pdfium;
    let doc = pdfium::open(pdf).unwrap();
    let pages = pdfium::page_count(&doc);
    assert!(pages >= 1);
    let mut text = String::new();
    for i in 0..pages {
        text.push_str(&pdfium::page_text(&pdfium::page(&doc, i).unwrap()).unwrap());
    }
    text
}

#[test]
fn markdown_to_docx_pdf_odt_with_libreoffice() {
    if !has_libreoffice() {
        return;
    }
    let dir = workspace(&["sample.md", "sample.png"]);
    let input = dir.path().join("sample.md");

    let docx = convert(&input, DocFormat::Docx).outputs.remove(0);
    assert_eq!(docx.file_name().unwrap(), "sample.docx");
    let xml = zip_text(&docx, "word/document.xml");
    assert!(
        xml.contains("Project notes") && xml.contains("Coffee"),
        "heading/table"
    );
    assert!(xml.contains("<w:tbl>"), "real table");
    assert!(xml.contains("Hello, Morph"), "code");
    assert!(xml.contains("café") && xml.contains("報告"));
    assert!(
        zip_names(&docx)
            .iter()
            .any(|n| n.starts_with("word/media/")),
        "picture embedded: {:?}",
        zip_names(&docx)
    );

    let pdf = convert(&input, DocFormat::Pdf).outputs.remove(0);
    let text = pdf_text(&pdf);
    assert!(
        text.contains("Project notes") && text.contains("Coffee"),
        "{text}"
    );
    assert!(text.contains("Hello, Morph"), "{text}");

    let odt = convert(&input, DocFormat::Odt).outputs.remove(0);
    assert!(zip_text(&odt, "content.xml").contains("Project notes"));
    assert!(
        zip_names(&odt).iter().any(|n| n.starts_with("Pictures/")),
        "{:?}",
        zip_names(&odt)
    );
}

#[test]
fn html_and_text_to_docx_with_libreoffice() {
    if !has_libreoffice() {
        return;
    }
    let dir = workspace(&["sample.html", "sample.txt", "sample.png"]);
    let docx = convert(&dir.path().join("sample.html"), DocFormat::Docx)
        .outputs
        .remove(0);
    let xml = zip_text(&docx, "word/document.xml");
    assert!(xml.contains("Sample page") && xml.contains("Coffee"));
    assert!(!xml.contains("console.log"));
    let media: Vec<_> = zip_names(&docx)
        .into_iter()
        .filter(|n| n.starts_with("word/media/"))
        .collect();
    assert_eq!(
        media.len(),
        1,
        "local picture embedded, remote one not fetched"
    );

    let pdf = convert(&dir.path().join("sample.txt"), DocFormat::Pdf)
        .outputs
        .remove(0);
    let source = read_utf8(&dir.path().join("sample.txt"));
    let first_words: String = source
        .split_whitespace()
        .take(2)
        .collect::<Vec<_>>()
        .join(" ");
    assert!(pdf_text(&pdf).contains(&first_words));
}

#[test]
fn word_to_markdown_with_libreoffice() {
    if !has_libreoffice() {
        return;
    }
    // DOCX has its own native reader (M5.1); the engine path is for doc/odt/rtf.
    let dir = workspace(&["sample.doc"]);
    let input = dir.path().join("Relatório 報告.doc");
    std::fs::rename(dir.path().join("sample.doc"), &input).unwrap();
    let out = convert(&input, DocFormat::Md);
    assert_eq!(out.outputs.len(), 1);
    let md_path = &out.outputs[0];
    assert_eq!(md_path.file_name().unwrap(), "Relatório 報告.md");
    let md = read_utf8(md_path);
    assert!(md.lines().any(|l| l.starts_with("# ")), "heading: {md}");
    assert!(md.lines().any(|l| l.starts_with("- ")), "list: {md}");
    assert!(md.contains("| "), "table: {md}");
    // The picture went to `name (images)` and the link resolves.
    let images = dir.path().join("Relatório 報告 (images)");
    let files: Vec<_> = std::fs::read_dir(&images).unwrap().collect();
    assert_eq!(files.len(), 1);
    let file = files[0]
        .as_ref()
        .unwrap()
        .file_name()
        .to_string_lossy()
        .into_owned();
    assert!(
        md.contains(&format!("Relatório%20報告%20%28images%29/{file}")),
        "{md}"
    );
}
