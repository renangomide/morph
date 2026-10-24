// SPDX-License-Identifier: GPL-3.0-or-later
//! Markdown → HTML / TXT, natively with `pulldown-cmark`.
//!
//! HTML: a complete UTF-8 page (`<meta charset>`, title from the first heading or
//! the file name, a small readable stylesheet), CommonMark + tables,
//! strikethrough and task lists. Relative image paths stay relative (the output
//! is written next to the input by default); nothing is fetched.
//!
//! TXT: the readable text: headings and paragraphs separated by blank lines,
//! list bullets, table cells separated by tabs, code as-is, link targets in
//! brackets, images as their description.

use super::html::{Attr, TagAction, attr_text, build_tag, decode_entities, rewrite_tags};
use super::plain::{crlf, html_page, read_text, stem, write_result};
use crate::converter::{TaskCtx, TaskOutput};
use crate::error::ConvertError;
use crate::job::Task;
use crate::markdown::is_safe_link;
use crate::office::OfficeConvertOpts;
use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};
use std::path::Path;

pub fn to_html(
    task: &Task,
    input: &Path,
    _opts: &OfficeConvertOpts,
    ctx: &TaskCtx<'_>,
) -> Result<TaskOutput, ConvertError> {
    let md = read_text(input)?;
    ctx.check_cancel()?;
    let page = markdown_to_html_page(&md, &stem(input));
    write_result(task, input, "html", page.as_bytes())
}

pub fn to_text(
    task: &Task,
    input: &Path,
    _opts: &OfficeConvertOpts,
    ctx: &TaskCtx<'_>,
) -> Result<TaskOutput, ConvertError> {
    let md = read_text(input)?;
    ctx.check_cancel()?;
    let text = markdown_to_text(&md);
    write_result(task, input, "txt", crlf(&text).as_bytes())
}

fn options() -> Options {
    Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS
}

/// The text of the first heading, if any.
fn first_heading(md: &str) -> Option<String> {
    let mut in_heading = false;
    let mut title = String::new();
    for event in Parser::new_ext(md, options()) {
        match event {
            Event::Start(Tag::Heading { .. }) => in_heading = true,
            Event::End(TagEnd::Heading(_)) => {
                let t = title.trim();
                if !t.is_empty() {
                    return Some(t.to_owned());
                }
                in_heading = false;
            }
            Event::Text(t) | Event::Code(t) if in_heading => title.push_str(&t),
            _ => {}
        }
    }
    None
}

/// A standalone HTML page. Title: the first heading, else `fallback_title`.
pub(crate) fn markdown_to_html_page(md: &str, fallback_title: &str) -> String {
    let mut body = String::with_capacity(md.len() * 3 / 2);
    pulldown_cmark::html::push_html(&mut body, Parser::new_ext(md, options()));
    let body = drop_unsafe_hrefs(&body);
    let title = first_heading(md).unwrap_or_else(|| fallback_title.to_owned());
    html_page(&title, &body)
}

/// Links (Markdown or raw HTML) to unsafe targets (`javascript:`, `file:`,
/// `data:`, … see [`is_safe_link`]) lose their `href`; the text stays.
fn drop_unsafe_hrefs(html: &str) -> String {
    let bad = |a: &Attr<'_>| {
        a.name.eq_ignore_ascii_case("href")
            && a.value.is_some_and(|v| !is_safe_link(&decode_entities(v)))
    };
    rewrite_tags(html, |name, attrs| {
        if name.starts_with('/') || !attrs.iter().any(bad) {
            return TagAction::Keep;
        }
        let kept: Vec<(String, Option<String>)> = attrs
            .iter()
            .filter(|a| !bad(a))
            .map(|a| (a.name.to_owned(), a.value.map(attr_text)))
            .collect();
        TagAction::Replace(build_tag(name, &kept))
    })
}

/// Readable plain text (LF line endings).
pub(crate) fn markdown_to_text(md: &str) -> String {
    let mut w = TextWriter::default();
    for event in Parser::new_ext(md, options()) {
        w.event(event);
    }
    let mut out = w.out.trim_end().to_owned();
    out.push('\n');
    out
}

#[derive(Default)]
struct TextWriter {
    out: String,
    /// One entry per open list: `Some(next number)` for ordered lists.
    lists: Vec<Option<u64>>,
    /// Open links' targets (printed after the link text).
    links: Vec<String>,
    /// Text written since the current link started (to skip "url (url)").
    link_start: Vec<usize>,
    in_cell_row: bool,
    cell_index: usize,
}

impl TextWriter {
    /// End the current block with a blank line.
    fn block_break(&mut self) {
        if self.out.is_empty() {
            return;
        }
        while !self.out.ends_with("\n\n") {
            self.out.push('\n');
        }
    }

    fn line_break(&mut self) {
        if !self.out.is_empty() && !self.out.ends_with('\n') {
            self.out.push('\n');
        }
    }

    fn event(&mut self, event: Event<'_>) {
        match event {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),
            Event::Text(t) | Event::Code(t) => self.out.push_str(&t),
            Event::SoftBreak | Event::HardBreak => self.out.push('\n'),
            Event::Rule => {
                self.block_break();
                self.out.push_str("----------\n\n");
            }
            Event::TaskListMarker(done) => self.out.push_str(if done { "[x] " } else { "[ ] " }),
            // Raw HTML, math, footnotes: not part of the readable text.
            _ => {}
        }
    }

    fn start(&mut self, tag: Tag<'_>) {
        match tag {
            Tag::Paragraph
            | Tag::Heading { .. }
            | Tag::BlockQuote(_)
            | Tag::Table(_)
            | Tag::CodeBlock(_) => {
                if self.lists.is_empty() {
                    self.block_break();
                } else {
                    self.line_break();
                }
            }
            Tag::List(start) => {
                if self.lists.is_empty() {
                    self.block_break();
                } else {
                    self.line_break();
                }
                self.lists.push(start);
            }
            Tag::Item => {
                self.line_break();
                let depth = self.lists.len().saturating_sub(1);
                self.out.push_str(&"    ".repeat(depth));
                match self.lists.last_mut() {
                    Some(Some(n)) => {
                        self.out.push_str(&format!("{n}. "));
                        *n += 1;
                    }
                    _ => self.out.push_str("- "),
                }
            }
            Tag::TableHead | Tag::TableRow => {
                self.line_break();
                self.in_cell_row = true;
                self.cell_index = 0;
            }
            Tag::TableCell => {
                if self.in_cell_row && self.cell_index > 0 {
                    self.out.push('\t');
                }
                self.cell_index += 1;
            }
            Tag::Link { dest_url, .. } => {
                self.links.push(dest_url.into_string());
                self.link_start.push(self.out.len());
            }
            _ => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph
            | TagEnd::Heading(_)
            | TagEnd::BlockQuote(_)
            | TagEnd::Table
            | TagEnd::CodeBlock => {
                if self.lists.is_empty() {
                    self.block_break();
                } else {
                    self.line_break();
                }
            }
            TagEnd::List(_) => {
                self.lists.pop();
                if self.lists.is_empty() {
                    self.block_break();
                } else {
                    self.line_break();
                }
            }
            TagEnd::TableHead | TagEnd::TableRow => {
                self.in_cell_row = false;
                self.out.push('\n');
            }
            TagEnd::Link => {
                let url = self.links.pop().unwrap_or_default();
                let start = self.link_start.pop().unwrap_or(self.out.len());
                let text = self.out.get(start..).unwrap_or_default();
                let mailto = url.strip_prefix("mailto:").unwrap_or(&url);
                if !url.is_empty() && !url.starts_with('#') && text.trim() != mailto {
                    self.out.push_str(&format!(" ({url})"));
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "# Notes\n\nSome *text* and a [link](https://example.com).\n\n\
| A | B |\n|---|--:|\n| 1 | 2 |\n\n```rust\nfn main() {}\n```\n\n- one\n- [x] done\n\n\
1. first\n2. second\n\n~~gone~~ ![pic](img/a%20b.png)\n";

    #[test]
    fn html_page_has_title_table_code_and_relative_image() {
        let page = markdown_to_html_page(SAMPLE, "fallback");
        assert!(page.starts_with("<!DOCTYPE html>"));
        assert!(page.contains("<meta charset=\"utf-8\">"));
        assert!(page.contains("<title>Notes</title>"));
        assert!(page.contains("<h1>Notes</h1>"));
        assert!(page.contains("<table>"));
        assert!(page.contains("<code class=\"language-rust\">"));
        assert!(page.contains("<del>gone</del>"));
        assert!(page.contains("type=\"checkbox\""));
        assert!(page.contains("src=\"img/a%20b.png\""));
        assert!(markdown_to_html_page("just text", "file").contains("<title>file</title>"));
    }

    #[test]
    fn html_never_links_to_scripts_or_local_files() {
        let md = "[a](javascript:alert(1)) [b](<file:///C:/x.txt>) [c](vbscript:x) \
[d](data:text/html,x) [ok](https://e.com) [rel](notes/b.md)\n\n\
<a href=\"java&#115;cript:alert(2)\">raw</a> <a href='https://e.com/2'>fine</a>\n";
        let page = markdown_to_html_page(md, "f");
        for bad in [
            "javascript",
            "java&#115;cript",
            "file:",
            "vbscript",
            "data:",
        ] {
            assert!(!page.contains(bad), "{bad}: {page}");
        }
        assert!(
            page.contains("<a>a</a>") && page.contains("<a>raw</a>"),
            "{page}"
        );
        assert!(page.contains("href=\"https://e.com\"") && page.contains("href=\"notes/b.md\""));
        assert!(page.contains("href='https://e.com/2'>fine"), "{page}");
    }

    #[test]
    fn text_is_readable() {
        let text = markdown_to_text(SAMPLE);
        assert!(text.starts_with("Notes\n\nSome text and a link (https://example.com).\n\n"));
        assert!(text.contains("A\tB\n1\t2\n"));
        assert!(text.contains("fn main() {}\n"));
        assert!(text.contains("- one\n- [x] done\n"));
        assert!(text.contains("1. first\n2. second\n"));
        assert!(text.contains("gone pic"));
        assert!(!text.contains('|') && !text.contains('#') && !text.contains('*'));
    }
}
