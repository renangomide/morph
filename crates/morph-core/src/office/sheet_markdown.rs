// SPDX-License-Identifier: GPL-3.0-or-later
//! Spreadsheet (XLSX/XLS/XLSB/ODS/CSV) → Markdown, natively (SPEC §4.6).
//! Reached through `office::plan` → `Plan::SheetToMarkdown`
//! (CPU queue, no engine).
//!
//! calamine reads the workbook (a CSV is first typed into a temp XLSX by
//! [`super::csv_xlsx::csv_as_temp_xlsx`], same rules as CSV → XLSX). Per sheet:
//! `## Sheet` + one pipe table, first non-empty row = header. Cells as values,
//! not as formatted: whole numbers without `.0`, other numbers with up to 15
//! significant digits (`2.5`, not the `0.00` format's `2.50`), dates ISO
//! (`2026-03-14`, `2026-03-14 08:30:00`, `08:30:00`), `TRUE`/`FALSE`, errors as
//! text (`#DIV/0!`), formulas as their cached result. Empty edge rows/columns are
//! trimmed, empty sheets skipped, `opts.max_rows` (0 = all) cuts with a note.

use super::csv_xlsx::{csv_as_temp_xlsx, format_number};
use super::options::{DocFormat, OfficeConvertOpts};
use crate::converter::{TaskCtx, TaskOutput};
use crate::error::{ConvertError, ErrorKind};
use crate::job::Task;
use crate::markdown::{ImageFolder, MarkdownWriter, escape_text, write_markdown};
use calamine::{Data, Reader, open_workbook_auto};
use std::path::Path;

pub fn convert(
    task: &Task,
    input: &Path,
    opts: &OfficeConvertOpts,
    ctx: &TaskCtx<'_>,
) -> Result<TaskOutput, ConvertError> {
    let staged;
    let source = if DocFormat::of_path(input) == Some(DocFormat::Csv) {
        staged = csv_as_temp_xlsx(input, opts.delimiter, ctx)?;
        staged.as_path()
    } else {
        input
    };
    let mut workbook = open_workbook_auto(source).map_err(open_failed)?;
    let names = workbook.sheet_names();
    let mut w = MarkdownWriter::new();
    for (i, name) in names.iter().enumerate() {
        ctx.check_cancel()?;
        let range = workbook.worksheet_range(name).map_err(open_failed)?;
        // Only the rows shown become text: leading blank rows, the header and
        // `max_rows` data rows. The rest is only counted (up to its last non-blank row).
        let max_rows = opts.max_rows as usize;
        let mut source = range.rows().skip_while(|r| r.iter().all(cell_blank));
        let take = if max_rows > 0 {
            max_rows + 1
        } else {
            usize::MAX
        };
        let rows: Vec<Vec<String>> = source
            .by_ref()
            .take(take)
            .map(|r| r.iter().map(cell_text).collect())
            .collect();
        let (mut hidden, mut pending) = (0usize, 0usize);
        for r in source {
            pending += 1;
            if !r.iter().all(cell_blank) {
                hidden += pending;
                pending = 0;
            }
        }
        write_sheet(&mut w, name, rows, max_rows, hidden);
        ctx.progress((i + 1) as f32 / names.len() as f32 * 0.95, None);
    }
    if w.is_empty() {
        return Err(ConvertError::new(
            ErrorKind::BadInput,
            "This spreadsheet has no data to convert.",
            "all sheets are empty",
        ));
    }
    let folder = task.output.folder_for(input);
    let images = ImageFolder::new(&folder, input, false);
    let out = write_markdown(&folder, input, &w.finish(), images)?;
    ctx.progress(1.0, None);
    Ok(out)
}

/// `## name` + the table (rows of raw text). Nothing for an empty sheet.
/// `hidden`: rows after `rows` that were not read (counted in the note).
fn write_sheet(
    w: &mut MarkdownWriter,
    name: &str,
    mut rows: Vec<Vec<String>>,
    max_rows: usize,
    hidden: usize,
) {
    let blank = |s: &String| s.trim().is_empty();
    // Trim empty rows at both ends, then empty columns at both ends.
    while rows.last().is_some_and(|r| r.iter().all(blank)) {
        rows.pop();
    }
    let first = rows
        .iter()
        .position(|r| !r.iter().all(blank))
        .unwrap_or(rows.len());
    rows.drain(..first);
    let used = |c: usize| rows.iter().any(|r| r.get(c).is_some_and(|s| !blank(s)));
    let width = rows.iter().map(Vec::len).max().unwrap_or(0);
    let Some(last_col) = (0..width).rev().find(|&c| used(c)) else {
        return;
    };
    let first_col = (0..=last_col).find(|&c| used(c)).unwrap_or(0);
    let mut table: Vec<Vec<String>> = rows
        .iter()
        .map(|r| {
            (first_col..=last_col)
                .map(|c| r.get(c).map(|s| escape_text(s)).unwrap_or_default())
                .collect()
        })
        .collect();
    let data_rows = table.len() - 1;
    let cut = hidden
        + if max_rows > 0 && data_rows > max_rows {
            table.truncate(max_rows + 1);
            data_rows - max_rows
        } else {
            0
        };
    w.heading(2, &escape_text(name));
    w.table(&table);
    if cut > 0 {
        let s = if cut == 1 { "" } else { "s" };
        w.paragraph(&format!("… {cut} more row{s} not shown."));
    }
}

fn cell_blank(cell: &Data) -> bool {
    match cell {
        Data::Empty => true,
        Data::String(s) => s.trim().is_empty(),
        _ => false,
    }
}

fn cell_text(cell: &Data) -> String {
    match cell {
        Data::Empty => String::new(),
        Data::String(s) | Data::DateTimeIso(s) | Data::DurationIso(s) => s.clone(),
        Data::Int(i) => i.to_string(),
        Data::Float(f) => format_number(*f, false),
        Data::Bool(b) => if *b { "TRUE" } else { "FALSE" }.to_owned(),
        Data::Error(e) => e.to_string(),
        Data::DateTime(dt) => {
            let value = dt.as_f64();
            if dt.is_duration() {
                let secs = (value * 86_400.0).round() as i64;
                return format!("{}:{:02}:{:02}", secs / 3600, secs / 60 % 60, secs % 60);
            }
            let (y, mo, d, h, mi, s, _) = dt.to_ymd_hms_milli();
            if value.abs() < 1.0 {
                format!("{h:02}:{mi:02}:{s:02}")
            } else if value.fract().abs() > 1e-9 {
                format!("{y:04}-{mo:02}-{d:02} {h:02}:{mi:02}:{s:02}")
            } else {
                format!("{y:04}-{mo:02}-{d:02}")
            }
        }
    }
}

fn open_failed(e: calamine::Error) -> ConvertError {
    let text = e.to_string();
    if text.to_ascii_lowercase().contains("password") {
        super::password_protected()
    } else {
        super::damaged_document(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(v: &[&[&str]]) -> Vec<Vec<String>> {
        v.iter()
            .map(|r| r.iter().map(|s| s.to_string()).collect())
            .collect()
    }

    #[test]
    fn trims_edges_and_cuts_rows() {
        let mut w = MarkdownWriter::new();
        let data = rows(&[
            &["", "", ""],
            &["", "A", "B", ""],
            &["", "1", "x|y", ""],
            &["", "2", "", ""],
            &["", "3", "", ""],
            &["", "", "", ""],
        ]);
        write_sheet(&mut w, "S", data, 2, 0);
        write_sheet(&mut w, "Empty", rows(&[&["", ""], &[" "]]), 0, 0);
        assert_eq!(
            w.finish(),
            "## S\n\n| A   | B    |\n| --- | ---- |\n| 1   | x\\|y |\n| 2   |      |\n\n… 1 more row not shown.\n"
        );
    }
}
