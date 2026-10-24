// SPDX-License-Identifier: GPL-3.0-or-later
//! Plain text → HTML / Markdown, natively.
//!
//! Decode UTF-8 (BOM optional; UTF-16 with a BOM too), else Windows-1252
//! (`encoding_rs`). HTML: escaped, blank lines split paragraphs, single line
//! breaks kept (`<br>`). Markdown: the text with Markdown special characters
//! escaped so it reads the same; single line breaks become hard breaks.
//!
//! Also holds the small helpers the other text converters share: decoding,
//! HTML escaping, the standalone page wrapper and output writing.

use crate::converter::{TaskCtx, TaskOutput};
use crate::error::{ConvertError, ErrorKind};
use crate::job::Task;
use crate::naming::{self, Suffix};
use crate::office::OfficeConvertOpts;
use std::path::Path;

pub fn to_html(
    task: &Task,
    input: &Path,
    _opts: &OfficeConvertOpts,
    ctx: &TaskCtx<'_>,
) -> Result<TaskOutput, ConvertError> {
    let text = read_text(input)?;
    ctx.check_cancel()?;
    let page = text_to_html_page(&text, &stem(input));
    write_result(task, input, "html", page.as_bytes())
}

pub fn to_markdown(
    task: &Task,
    input: &Path,
    _opts: &OfficeConvertOpts,
    ctx: &TaskCtx<'_>,
) -> Result<TaskOutput, ConvertError> {
    let text = read_text(input)?;
    ctx.check_cancel()?;
    let md = text_to_markdown(&text);
    write_result(task, input, "md", md.as_bytes())
}

// ---------------------------------------------------------------- conversions

/// Paragraphs (split on blank lines) as `<p>`, single line breaks as `<br>`.
pub(crate) fn text_to_html_body(text: &str) -> String {
    let mut body = String::new();
    for para in paragraphs(text) {
        body.push_str("<p>");
        let lines: Vec<String> = para.iter().map(|l| escape_html(l)).collect();
        body.push_str(&lines.join("<br>\n"));
        body.push_str("</p>\n");
    }
    body
}

pub(crate) fn text_to_html_page(text: &str, title: &str) -> String {
    html_page(title, &text_to_html_body(text))
}

/// Escape Markdown so the text reads the same once rendered.
pub(crate) fn text_to_markdown(text: &str) -> String {
    let mut out = String::new();
    for (i, para) in paragraphs(text).iter().enumerate() {
        if i > 0 {
            out.push_str("\n\n");
        }
        let lines: Vec<String> = para.iter().map(|l| escape_markdown_line(l)).collect();
        // Two trailing spaces: a hard line break that stays invisible in the source.
        out.push_str(&lines.join("  \n"));
    }
    out.push('\n');
    out
}

/// Lines grouped into paragraphs (blank lines separate them). Trailing
/// whitespace is trimmed; leading indentation is kept.
fn paragraphs(text: &str) -> Vec<Vec<&str>> {
    let mut out: Vec<Vec<&str>> = Vec::new();
    let mut current: Vec<&str> = Vec::new();
    for line in text.lines() {
        let line = line.trim_end();
        if line.is_empty() {
            if !current.is_empty() {
                out.push(std::mem::take(&mut current));
            }
        } else {
            current.push(line);
        }
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

fn escape_markdown_line(line: &str) -> String {
    let trimmed = line.trim_start();
    let indent = &line[..line.len() - trimmed.len()];
    let mut out = String::with_capacity(line.len() + 8);
    // Leading indentation would become a code block: keep it as non-breaking spaces.
    for c in indent.chars() {
        out.push_str(if c == '\t' { "&emsp;" } else { "&nbsp;" });
    }
    // Block markers only matter at the start of a line.
    let mut rest = trimmed;
    if let Some(c) = rest.chars().next()
        && matches!(c, '#' | '>' | '-' | '+' | '=')
    {
        out.push('\\');
        out.push(c);
        rest = &rest[c.len_utf8()..];
    } else {
        // "1. item" / "1) item" would become an ordered list.
        let digits = rest.chars().take_while(char::is_ascii_digit).count();
        if digits > 0 && rest[digits..].starts_with(['.', ')']) {
            out.push_str(&rest[..digits]);
            out.push('\\');
            rest = &rest[digits..];
        }
    }
    for c in rest.chars() {
        if matches!(
            c,
            '\\' | '`' | '*' | '_' | '[' | ']' | '<' | '>' | '|' | '~' | '!' | '&'
        ) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

// ---------------------------------------------------------------- shared helpers

/// Read a text file: UTF-8 (BOM optional), UTF-16 with a BOM, else Windows-1252.
pub(crate) fn read_text(input: &Path) -> Result<String, ConvertError> {
    let bytes = std::fs::read(input)?;
    Ok(decode_text(&bytes))
}

/// Decode text bytes: BOM-marked UTF-8/UTF-16, plain UTF-8, else Windows-1252
/// (what older Windows programs write).
pub fn decode_text(bytes: &[u8]) -> String {
    if let Some((encoding, bom_len)) = encoding_rs::Encoding::for_bom(bytes) {
        let (text, _) = encoding.decode_without_bom_handling(&bytes[bom_len..]);
        return text.into_owned();
    }
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_owned(),
        Err(_) => encoding_rs::WINDOWS_1252.decode(bytes).0.into_owned(),
    }
}

pub(crate) fn escape_html(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

/// Small, readable default style for the pages Morph writes.
const PAGE_CSS: &str = "body{font-family:\"Segoe UI\",system-ui,-apple-system,sans-serif;\
line-height:1.6;color:#1f2328;max-width:46em;margin:2em auto;padding:0 1em}\
h1,h2,h3,h4{line-height:1.25;margin-top:1.5em}\
pre,code{font-family:Consolas,\"Cascadia Mono\",monospace;font-size:.92em}\
pre{background:#f6f8fa;padding:.8em 1em;overflow:auto;border-radius:6px}\
code{background:#f6f8fa;padding:.1em .3em;border-radius:4px}pre code{background:none;padding:0}\
table{border-collapse:collapse;margin:1em 0}th,td{border:1px solid #d0d7de;padding:.35em .7em}\
th{background:#f6f8fa}img{max-width:100%}blockquote{margin:1em 0;padding:0 1em;color:#59636e;\
border-left:.25em solid #d0d7de}";

/// A complete UTF-8 HTML document around `body`.
pub(crate) fn html_page(title: &str, body: &str) -> String {
    format!(
        "<!DOCTYPE html>\n<html>\n<head>\n<meta charset=\"utf-8\">\n\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\
         <title>{}</title>\n<style>{PAGE_CSS}</style>\n</head>\n<body>\n{body}</body>\n</html>\n",
        escape_html(title)
    )
}

pub(crate) fn stem(input: &Path) -> String {
    input
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Document".to_owned())
}

/// `\n` → `\r\n` (TXT outputs, so every Windows program shows the lines).
pub(crate) fn crlf(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\n', "\r\n")
}

/// Write `name.ext` next to the input (or in the chosen folder), safely.
pub(crate) fn write_result(
    task: &Task,
    input: &Path,
    ext: &str,
    bytes: &[u8],
) -> Result<TaskOutput, ConvertError> {
    let folder = task.output.folder_for(input);
    let name = naming::output_name(input, ext, Suffix::Converted);
    naming::write_output(&folder, &name, bytes).map(TaskOutput::single)
}

/// "This file couldn't be read as text." for parser failures.
pub(crate) fn unreadable(details: impl ToString) -> ConvertError {
    ConvertError::new(
        ErrorKind::BadInput,
        "This file couldn't be read as text.",
        details,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_utf8_bom_utf16_and_1252() {
        assert_eq!(decode_text("\u{feff}café".as_bytes()), "café");
        assert_eq!(decode_text("café".as_bytes()), "café");
        assert_eq!(decode_text(&[b'c', b'a', b'f', 0xE9]), "café");
        let utf16: Vec<u8> = [0xFF, 0xFE]
            .into_iter()
            .chain("ação".encode_utf16().flat_map(u16::to_le_bytes))
            .collect();
        assert_eq!(decode_text(&utf16), "ação");
    }

    #[test]
    fn html_escapes_and_keeps_paragraphs() {
        let body = text_to_html_body("a < b & c\r\nnext line\r\n\r\nsecond");
        assert_eq!(
            body,
            "<p>a &lt; b &amp; c<br>\nnext line</p>\n<p>second</p>\n"
        );
    }

    #[test]
    fn markdown_escapes_specials() {
        let md = text_to_markdown("# not a heading\n1. not a list\nuse *stars* & <tags>");
        assert_eq!(
            md,
            "\\# not a heading  \n1\\. not a list  \nuse \\*stars\\* \\& \\<tags\\>\n"
        );
    }
}
