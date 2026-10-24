// SPDX-License-Identifier: GPL-3.0-or-later
//! Extract text (SPEC §4.2): UTF-8 `name.txt`, optional `--- Page N ---` lines.
//! No text on any page → fail with "No text found. This PDF may be scanned pages (pictures, not text).".
//!
//! Format: UTF-8 without BOM (Notepad detects UTF-8 on its own), Windows line
//! endings (CRLF) everywhere, a blank line between pages. Each page's text is
//! trimmed at the end; control characters PDFium emits (NUL, U+FFFE markers)
//! are dropped. The file always ends with a line break.

use crate::action::Action;
use crate::converter::{Converter, TaskCtx, TaskOutput};
use crate::error::{ConvertError, ErrorKind};
use crate::job::Task;
use crate::naming::{Suffix, output_name, write_output};
use crate::pdf::pdfium;

const CRLF: &str = "\r\n";

pub struct PdfToText;

impl Converter for PdfToText {
    fn run(&self, task: &Task, ctx: &TaskCtx<'_>) -> Result<TaskOutput, ConvertError> {
        let Action::PdfToText(opts) = &task.action else {
            return Err(super::wrong_action("PdfToText"));
        };
        let input = task
            .input()
            .ok_or_else(|| super::wrong_action("PdfToText"))?;
        let doc = pdfium::open(input)?;
        let count = pdfium::page_count(&doc);
        let mut texts = Vec::with_capacity(count as usize);
        for i in 0..count {
            ctx.check_cancel()?;
            let page = pdfium::page(&doc, i)?;
            texts.push(clean(&pdfium::page_text(&page)?));
            ctx.progress((i + 1) as f32 / count.max(1) as f32, None);
        }
        drop(doc);
        let out = build_text(&texts, opts.page_separators).ok_or_else(no_text)?;
        ctx.check_cancel()?;
        let name = output_name(input, "txt", Suffix::Converted);
        let path = write_output(&task.output.folder_for(input), &name, out.as_bytes())?;
        Ok(TaskOutput::single(path))
    }
}

/// "No text found. This PDF may be scanned pages (pictures, not text)."
fn no_text() -> ConvertError {
    ConvertError::new(
        ErrorKind::Unsupported,
        "No text found. This PDF may be scanned pages (pictures, not text).",
        "every page's text layer is empty",
    )
}

/// One page's text with CRLF line endings, no stray control characters and no
/// trailing whitespace.
fn clean(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len() + raw.len() / 32);
    let mut chars = raw.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                out.push_str(CRLF);
            }
            '\n' => out.push_str(CRLF),
            '\t' => out.push('\t'),
            '\u{FFFE}' | '\u{FFFF}' => {}
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out.truncate(out.trim_end().len());
    out
}

/// The whole file, or `None` when every page is blank.
fn build_text(pages: &[String], separators: bool) -> Option<String> {
    if pages.iter().all(|t| t.trim().is_empty()) {
        return None;
    }
    let mut out = String::new();
    for (i, text) in pages.iter().enumerate() {
        if i > 0 {
            out.push_str(CRLF);
        }
        if separators {
            out.push_str(&format!("--- Page {} ---{CRLF}", i + 1));
        }
        if !text.is_empty() {
            out.push_str(text);
            out.push_str(CRLF);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_endings_become_crlf() {
        assert_eq!(clean("a\nb\r\nc\rd \u{0}\u{FFFE}\n\n"), "a\r\nb\r\nc\r\nd");
    }

    #[test]
    fn pages_are_joined_with_optional_separators() {
        let pages = vec!["one".to_owned(), String::new(), "three".to_owned()];
        assert_eq!(
            build_text(&pages, false).unwrap(),
            "one\r\n\r\n\r\nthree\r\n"
        );
        assert_eq!(
            build_text(&pages, true).unwrap(),
            "--- Page 1 ---\r\none\r\n\r\n--- Page 2 ---\r\n\r\n--- Page 3 ---\r\nthree\r\n"
        );
        assert!(build_text(&[" ".into(), String::new()], true).is_none());
    }
}
