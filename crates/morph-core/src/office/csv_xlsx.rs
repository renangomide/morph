// SPDX-License-Identifier: GPL-3.0-or-later
//! Native CSV ↔ XLSX, no Office needed (SPEC §4.5).
//!
//! - [`csv_to_xlsx`]: decode (UTF-8 with or without BOM, UTF-16 with BOM, else
//!   Windows-1252 via `encoding_rs`), read with `csv` (delimiter `opts.delimiter`,
//!   `Auto` = [`detect_delimiter`]), write one sheet named after the file with
//!   `rust_xlsxwriter`. Cell typing ([`parse_number`], ISO dates): numbers become
//!   numbers, but IDs with leading zeros (`00123`), `+` phone numbers, numbers
//!   with more than 15 digits and anything ambiguous stay text. In `;`/tab files
//!   a decimal comma (`1234,5`) and `.` thousands separators are understood.
//!   `yyyy-mm-dd[ hh:mm[:ss]]` become real dates. Text is never read as a formula.
//! - [`to_csv`]: read xlsx/xls/ods with `calamine` (formulas as their cached
//!   values); one sheet → `name.csv`, several → `name - Sheet1.csv`, …
//!   Delimiter `opts.delimiter.write_byte()` (`;` files write a decimal comma,
//!   like Excel in Europe), UTF-8, BOM when `opts.csv_bom`, CRLF. Dates are
//!   written as ISO `yyyy-mm-dd[ hh:mm:ss]`, numbers with up to 15 significant
//!   digits (what Excel shows). All files are written first and committed at the
//!   end, so a failure leaves nothing behind.

use super::options::{CsvDelimiter, OfficeConvertOpts};
use crate::converter::{TaskCtx, TaskOutput};
use crate::error::{ConvertError, ErrorKind};
use crate::job::Task;
use crate::naming::{self, OutputFile, Suffix};
use crate::text::plain::decode_text;
use calamine::{Data, Reader, open_workbook_auto};
use rust_xlsxwriter::{ExcelDateTime, Format, Workbook, XlsxError};
use std::path::Path;

/// Excel's limits.
const MAX_ROWS: usize = 1_048_576;
const MAX_COLS: usize = 16_384;

const UTF8_BOM: &[u8] = b"\xEF\xBB\xBF";

// ---------------------------------------------------------------- CSV → XLSX

pub fn csv_to_xlsx(
    task: &Task,
    input: &Path,
    opts: &OfficeConvertOpts,
    ctx: &TaskCtx<'_>,
) -> Result<TaskOutput, ConvertError> {
    let buffer = csv_to_xlsx_bytes(input, opts.delimiter, ctx)?;
    let folder = task.output.folder_for(input);
    let name = naming::output_name(input, "xlsx", Suffix::Converted);
    let out = naming::write_output(&folder, &name, &buffer)?;
    ctx.progress(1.0, None);
    Ok(TaskOutput::single(out))
}

/// The CSV as an XLSX in a fresh folder of the task's temp dir
/// (`<temp>\csv-…\name.xlsx`). Office engines get this instead of the CSV, so
/// they see exactly Morph's typing: text stays text, nothing is evaluated as a
/// formula (`=1+41`), `007` keeps its zeros, `1/2` is no date, and any
/// encoding is already decoded.
pub(crate) fn csv_as_temp_xlsx(
    input: &Path,
    delimiter: CsvDelimiter,
    ctx: &TaskCtx<'_>,
) -> Result<std::path::PathBuf, ConvertError> {
    let buffer = csv_to_xlsx_bytes(input, delimiter, ctx)?;
    let dir = ctx
        .temp_dir()?
        .join(format!("csv-{}", uuid::Uuid::new_v4().simple()));
    std::fs::create_dir_all(&dir)
        .map_err(|e| ConvertError::other("Couldn't create a temporary folder.", e))?;
    let stem = input
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "sheet".to_owned());
    let path = dir.join(format!("{stem}.xlsx"));
    std::fs::write(&path, &buffer)?;
    Ok(path)
}

fn csv_to_xlsx_bytes(
    input: &Path,
    delimiter: CsvDelimiter,
    ctx: &TaskCtx<'_>,
) -> Result<Vec<u8>, ConvertError> {
    let bytes = std::fs::read(input)?;
    let text = decode_text(&bytes);
    let delimiter = delimiter.byte().unwrap_or_else(|| detect_delimiter(&text));
    let decimal_comma = delimiter != b',';
    ctx.check_cancel()?;

    let mut workbook = Workbook::new();
    let date_format = Format::new().set_num_format("yyyy-mm-dd");
    let datetime_format = Format::new().set_num_format("yyyy-mm-dd hh:mm:ss");
    let sheet = workbook.add_worksheet();
    sheet.set_name(sheet_name(input)).map_err(xlsx_failed)?;

    let mut reader = csv::ReaderBuilder::new()
        .delimiter(delimiter)
        .has_headers(false)
        .flexible(true)
        .from_reader(text.as_bytes());
    let total = text.len().max(1) as f32;
    let mut rows = 0usize;
    for (r, record) in reader.records().enumerate() {
        let record = record.map_err(|e| {
            ConvertError::new(
                ErrorKind::BadInput,
                "This CSV file couldn't be read.",
                format!("line {}: {e}", r + 1),
            )
        })?;
        if r % 2000 == 0 {
            ctx.check_cancel()?;
            let at = record.position().map_or(0, |p| p.byte()) as f32;
            ctx.progress(0.9 * at / total, None);
        }
        if r >= MAX_ROWS {
            return Err(too_big("rows", MAX_ROWS));
        }
        if record.len() > MAX_COLS {
            return Err(too_big("columns", MAX_COLS));
        }
        let row = r as u32;
        for (c, field) in record.iter().enumerate() {
            let col = c as u16;
            match parse_cell(field, decimal_comma) {
                Cell::Empty => {}
                Cell::Number(n) => {
                    sheet.write_number(row, col, n).map_err(xlsx_failed)?;
                }
                Cell::Date(d, has_time) => {
                    let format = if has_time {
                        &datetime_format
                    } else {
                        &date_format
                    };
                    sheet
                        .write_datetime_with_format(row, col, &d, format)
                        .map_err(xlsx_failed)?;
                }
                Cell::Text => {
                    sheet.write_string(row, col, field).map_err(xlsx_failed)?;
                }
            }
        }
        rows = r + 1;
    }
    // Autofit reads every cell: only for sheets of a sensible size.
    if rows <= 50_000 {
        sheet.autofit();
    }
    ctx.check_cancel()?;
    workbook.save_to_buffer().map_err(xlsx_failed)
}

fn too_big(what: &str, limit: usize) -> ConvertError {
    ConvertError::unsupported(
        format!("This CSV file has more {what} than Excel allows."),
        format!("limit {limit} {what}"),
    )
}

fn xlsx_failed(e: XlsxError) -> ConvertError {
    match e {
        XlsxError::MaxStringLengthExceeded => {
            ConvertError::unsupported("A cell is longer than Excel allows (32,767 characters).", e)
        }
        _ => ConvertError::other("Couldn't create the spreadsheet.", e),
    }
}

/// A valid Excel sheet name from the file name (no `[]:*?/\`, max 31 chars).
fn sheet_name(input: &Path) -> String {
    let stem = input
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let cleaned: String = stem
        .chars()
        .map(|c| {
            if matches!(c, '[' | ']' | ':' | '*' | '?' | '/' | '\\') || c.is_control() {
                '_'
            } else {
                c
            }
        })
        .collect();
    let cleaned = cleaned.trim().trim_matches('\'');
    let name: String = cleaned.chars().take(31).collect();
    let name = name.trim_end_matches('\'').trim().to_owned();
    if name.is_empty() || name.eq_ignore_ascii_case("history") {
        "Sheet1".to_owned()
    } else {
        name
    }
}

/// Guess the separator (`,` `;` tab) from the first records, quote-aware:
/// the candidate that appears the same number of times on every line wins,
/// then the most frequent one; comma when nothing matches.
pub fn detect_delimiter(text: &str) -> u8 {
    const CANDIDATES: [u8; 3] = *b",;\t";
    let mut lines: Vec<[usize; 3]> = Vec::new();
    let mut counts = [0usize; 3];
    let mut in_quotes = false;
    let mut any = false;
    for (i, b) in text.bytes().enumerate() {
        if i > 64 * 1024 {
            break;
        }
        match b {
            b'"' => in_quotes = !in_quotes,
            b'\n' | b'\r' if !in_quotes => {
                if any {
                    lines.push(counts);
                    if lines.len() >= 50 {
                        break;
                    }
                }
                counts = [0; 3];
                any = false;
            }
            _ => {
                if !in_quotes && let Some(k) = CANDIDATES.iter().position(|&c| c == b) {
                    counts[k] += 1;
                }
                any = true;
            }
        }
    }
    if any && lines.len() < 50 {
        lines.push(counts);
    }
    if lines.is_empty() {
        return b',';
    }
    let consistent = |k: usize| {
        let first = lines[0][k];
        first > 0 && lines.iter().all(|l| l[k] == first)
    };
    let best = |pick: &dyn Fn(usize) -> usize| {
        (0..3)
            .map(|k| (pick(k), k))
            .filter(|&(score, _)| score > 0)
            // Ties: earlier candidate (comma, then semicolon, then tab).
            .max_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(&a.1)))
            .map(|(_, k)| CANDIDATES[k])
    };
    best(&|k| if consistent(k) { lines[0][k] } else { 0 })
        .or_else(|| best(&|k| lines.iter().map(|l| l[k]).min().unwrap_or(0)))
        .or_else(|| best(&|k| lines.iter().map(|l| l[k]).sum()))
        .unwrap_or(b',')
}

enum Cell {
    Empty,
    Number(f64),
    /// Date (+ whether it has a time part).
    Date(ExcelDateTime, bool),
    Text,
}

fn parse_cell(field: &str, decimal_comma: bool) -> Cell {
    if field.is_empty() {
        return Cell::Empty;
    }
    if let Some(n) = parse_number(field, decimal_comma) {
        return Cell::Number(n);
    }
    if let Some((d, t)) = parse_iso_date(field) {
        return Cell::Date(d, t);
    }
    Cell::Text
}

/// A plain decimal number, or `None` to keep the text. `decimal_comma`: the
/// file uses `1.234,5` (`;`/tab files; `1234.5` is accepted too); otherwise
/// `1,234.5`. Leading zeros (`007`), a `+` sign, exponents, more than 15
/// significant digits, currency and percent signs all stay text.
pub fn parse_number(field: &str, decimal_comma: bool) -> Option<f64> {
    let s = field.trim();
    let (negative, body) = match s.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, s),
    };
    if body.is_empty()
        || !body
            .bytes()
            .all(|b| b.is_ascii_digit() || b == b'.' || b == b',')
    {
        return None;
    }
    let (thousands, decimal) = if decimal_comma {
        ('.', ',')
    } else {
        (',', '.')
    };
    let (int_part, frac_part) = match body.rfind(decimal) {
        Some(i) => (&body[..i], Some(&body[i + 1..])),
        None => (body, None),
    };
    // In decimal-comma files a lone `.` with a fraction is also a decimal point
    // (`1234.5`), unless it looks like thousands grouping (`1.234`).
    let (int_part, frac_part) = if decimal_comma && frac_part.is_none() {
        match int_part.find('.') {
            Some(i) if int_part.matches('.').count() == 1 && !is_grouped(int_part, '.') => {
                (&int_part[..i], Some(&int_part[i + 1..]))
            }
            _ => (int_part, None),
        }
    } else {
        (int_part, frac_part)
    };
    if int_part.is_empty() {
        return None; // ".5" stays text
    }
    let digits: String = if int_part.contains(thousands) {
        if !is_grouped(int_part, thousands) {
            return None;
        }
        int_part.chars().filter(|&c| c != thousands).collect()
    } else {
        int_part.to_owned()
    };
    if !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    if digits.len() > 1 && digits.starts_with('0') {
        return None; // IDs like 00123
    }
    let frac = match frac_part {
        Some(f) if f.is_empty() || !f.bytes().all(|b| b.is_ascii_digit()) => return None,
        Some(f) => f,
        None => "",
    };
    let significant = digits.trim_start_matches('0').len() + frac.len();
    if significant > 15 {
        return None; // would lose digits (card numbers, long IDs)
    }
    let text = if frac.is_empty() {
        digits
    } else {
        format!("{digits}.{frac}")
    };
    let value: f64 = text.parse().ok()?;
    Some(if negative { -value } else { value })
}

/// `1.234.567` / `1,234`: groups of exactly three digits after the first.
fn is_grouped(int_part: &str, sep: char) -> bool {
    let mut groups = int_part.split(sep);
    let first = groups.next().unwrap_or_default();
    (1..=3).contains(&first.len())
        && !first.starts_with('0')
        && groups.all(|g| g.len() == 3 && g.bytes().all(|b| b.is_ascii_digit()))
}

/// `yyyy-mm-dd`, optionally followed by ` hh:mm[:ss]` or `Thh:mm[:ss]`.
fn parse_iso_date(field: &str) -> Option<(ExcelDateTime, bool)> {
    let s = field.trim();
    let b = s.as_bytes();
    if b.len() < 10 || b[4] != b'-' || b[7] != b'-' {
        return None;
    }
    let num = |r: std::ops::Range<usize>| -> Option<u32> {
        let t = s.get(r)?;
        t.bytes()
            .all(|c| c.is_ascii_digit())
            .then(|| t.parse().ok())?
    };
    let (y, m, d) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let date = ExcelDateTime::from_ymd(y as u16, m as u8, d as u8).ok()?;
    if b.len() == 10 {
        return Some((date, false));
    }
    if !(b[10] == b' ' || b[10] == b'T') || !(b.len() == 16 || b.len() == 19) || b[13] != b':' {
        return None;
    }
    let (hh, mm) = (num(11..13)?, num(14..16)?);
    let ss = if b.len() == 19 {
        if b[16] != b':' {
            return None;
        }
        num(17..19)?
    } else {
        0
    };
    if hh > 23 {
        return None;
    }
    let dt = date.and_hms(hh as u16, mm as u8, ss as f64).ok()?;
    Some((dt, true))
}

// ---------------------------------------------------------------- sheets → CSV

pub fn to_csv(
    task: &Task,
    input: &Path,
    opts: &OfficeConvertOpts,
    ctx: &TaskCtx<'_>,
) -> Result<TaskOutput, ConvertError> {
    let mut workbook = open_workbook_auto(input).map_err(open_failed)?;
    let names = workbook.sheet_names();
    if names.is_empty() {
        return Err(super::damaged_document("no worksheets"));
    }
    let delimiter = opts.delimiter.write_byte();
    let decimal_comma = delimiter == b';';
    let folder = task.output.folder_for(input);
    let stem = input
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Sheet".to_owned());

    let mut pending: Vec<OutputFile> = Vec::with_capacity(names.len());
    for (i, sheet) in names.iter().enumerate() {
        ctx.check_cancel()?;
        let range = workbook.worksheet_range(sheet).map_err(open_failed)?;
        let bytes = sheet_to_csv(&range, delimiter, decimal_comma, opts.csv_bom)?;
        let name = if names.len() == 1 {
            format!("{stem}.csv")
        } else {
            format!("{stem} - {}.csv", file_safe(sheet))
        };
        let mut out = OutputFile::create(&folder, &name)?;
        out.write_all(&bytes)?;
        pending.push(out);
        ctx.progress((i + 1) as f32 / names.len() as f32 * 0.95, None);
    }
    ctx.check_cancel()?;
    let mut outputs = Vec::with_capacity(pending.len());
    for out in pending {
        outputs.push(out.commit()?);
    }
    ctx.progress(1.0, None);
    Ok(TaskOutput {
        outputs,
        note: None,
    })
}

fn open_failed(e: calamine::Error) -> ConvertError {
    let text = e.to_string();
    if text.to_ascii_lowercase().contains("password") {
        super::password_protected()
    } else {
        super::damaged_document(text)
    }
}

/// A sheet name usable in a Windows file name.
fn file_safe(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') || c.is_control() {
                '_'
            } else {
                c
            }
        })
        .collect();
    let cleaned = cleaned.trim().trim_end_matches('.').trim();
    if cleaned.is_empty() {
        "Sheet".to_owned()
    } else {
        cleaned.to_owned()
    }
}

fn sheet_to_csv(
    range: &calamine::Range<Data>,
    delimiter: u8,
    decimal_comma: bool,
    bom: bool,
) -> Result<Vec<u8>, ConvertError> {
    let buffer = if bom { UTF8_BOM.to_vec() } else { Vec::new() };
    let mut writer = csv::WriterBuilder::new()
        .delimiter(delimiter)
        .flexible(true)
        .terminator(csv::Terminator::CRLF)
        .from_writer(buffer);
    let failed = |e: csv::Error| ConvertError::other("Couldn't write the CSV file.", e);
    // calamine trims leading empty rows/columns: put them back so cells keep
    // their positions (data starting at B3 stays at B3).
    let (first_row, first_col) = range.start().unwrap_or((0, 0));
    let width = range.width() + first_col as usize;
    for _ in 0..first_row {
        writer
            .write_record(std::iter::repeat_n("", width.max(1)))
            .map_err(failed)?;
    }
    for row in range.rows() {
        let cells = std::iter::repeat_n(String::new(), first_col as usize)
            .chain(row.iter().map(|c| cell_text(c, decimal_comma)));
        writer.write_record(cells).map_err(failed)?;
    }
    writer
        .into_inner()
        .map_err(|e| ConvertError::other("Couldn't write the CSV file.", e))
}

fn cell_text(cell: &Data, decimal_comma: bool) -> String {
    match cell {
        Data::Empty => String::new(),
        Data::String(s) | Data::DateTimeIso(s) | Data::DurationIso(s) => s.clone(),
        Data::Int(i) => i.to_string(),
        Data::Float(f) => format_number(*f, decimal_comma),
        Data::Bool(b) => if *b { "TRUE" } else { "FALSE" }.to_owned(),
        Data::Error(e) => e.to_string(),
        Data::DateTime(dt) => {
            let value = dt.as_f64();
            if dt.is_duration() {
                let secs = (value * 86_400.0).round() as i64;
                return format!("{}:{:02}:{:02}", secs / 3600, secs / 60 % 60, secs % 60);
            }
            let (y, mo, d, h, mi, s, _) = dt.to_ymd_hms_milli();
            let has_time = value.fract().abs() > 1e-9;
            if value.abs() < 1.0 {
                format!("{h:02}:{mi:02}:{s:02}")
            } else if has_time {
                format!("{y:04}-{mo:02}-{d:02} {h:02}:{mi:02}:{s:02}")
            } else {
                format!("{y:04}-{mo:02}-{d:02}")
            }
        }
    }
}

/// Up to 15 significant digits, no exponent (`0.1 + 0.2` → `0.3`, `7.0` → `7`).
pub fn format_number(value: f64, decimal_comma: bool) -> String {
    if !value.is_finite() {
        return value.to_string();
    }
    let rounded: f64 = format!("{value:.14e}").parse().unwrap_or(value);
    let text = format!("{rounded}");
    if decimal_comma {
        text.replace('.', ",")
    } else {
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_delimiters() {
        assert_eq!(detect_delimiter("a,b,c\n1,2,3\n"), b',');
        assert_eq!(detect_delimiter("a;b;c\r\n1,5;2;3\r\n"), b';');
        assert_eq!(detect_delimiter("a\tb\n1,2\t3\n"), b'\t');
        // Separators inside quotes don't count, also across lines.
        assert_eq!(
            detect_delimiter("a;\"x,y,z\"\n1;\"multi\nline, here\"\n"),
            b';'
        );
        assert_eq!(detect_delimiter("just one column\n"), b',');
        assert_eq!(detect_delimiter(""), b',');
        // Single line: the one that is there.
        assert_eq!(detect_delimiter("x;y"), b';');
    }

    #[test]
    fn numbers_are_typed_carefully() {
        assert_eq!(parse_number("1234.5", false), Some(1234.5));
        assert_eq!(parse_number("1234,5", true), Some(1234.5));
        assert_eq!(parse_number("1.234,5", true), Some(1234.5));
        assert_eq!(parse_number("1,234.5", false), Some(1234.5));
        assert_eq!(parse_number("1234.5", true), Some(1234.5));
        assert_eq!(parse_number("1.234", true), Some(1234.0));
        assert_eq!(parse_number("-7", false), Some(-7.0));
        assert_eq!(parse_number("0", false), Some(0.0));
        assert_eq!(parse_number("0.25", false), Some(0.25));
        assert_eq!(parse_number("0,25", true), Some(0.25));
        for text in [
            "00123",
            "007",
            "+351 912",
            "+5",
            "1e5",
            "12%",
            "$5",
            "1,23",
            "1234,5",
            ".5",
            "5.",
            "1.2.3",
            "1234567890123456",
            "abc",
            "",
        ] {
            assert_eq!(parse_number(text, false), None, "{text}");
        }
        assert_eq!(parse_number("0123,5", true), None);
    }

    #[test]
    fn iso_dates_only() {
        assert!(matches!(
            parse_cell("2024-01-31", false),
            Cell::Date(_, false)
        ));
        assert!(matches!(
            parse_cell("2024-01-31 13:45", false),
            Cell::Date(_, true)
        ));
        assert!(matches!(
            parse_cell("2024-01-31T13:45:10", false),
            Cell::Date(_, true)
        ));
        assert!(matches!(parse_cell("2024-02-30", false), Cell::Text));
        assert!(matches!(parse_cell("31/01/2024", false), Cell::Text));
        assert!(matches!(parse_cell("=SUM(A1)", false), Cell::Text));
    }

    #[test]
    fn numbers_are_written_like_excel_shows_them() {
        assert_eq!(format_number(0.1 + 0.2, false), "0.3");
        assert_eq!(format_number(7.0, false), "7");
        assert_eq!(format_number(1234.5, true), "1234,5");
        assert_eq!(format_number(-0.000125, false), "-0.000125");
    }

    #[test]
    fn names_are_cleaned() {
        assert_eq!(sheet_name(Path::new("a[1]: b?.csv")), "a_1__ b_");
        assert_eq!(
            sheet_name(Path::new("a very long file name that goes on and on.csv")).len(),
            31
        );
        assert_eq!(sheet_name(Path::new("History.csv")), "Sheet1");
        assert_eq!(file_safe("Q1/Q2: \"x\"."), "Q1_Q2_ _x_");
    }
}
