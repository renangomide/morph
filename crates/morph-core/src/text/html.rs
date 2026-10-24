// SPDX-License-Identifier: GPL-3.0-or-later
//! HTML → Markdown / TXT, natively with `htmd`. Also used by
//! Word → Markdown (`crate::office::to_markdown`). Scripts and styles are dropped;
//! links and relative image paths are kept; nothing is fetched from the network.
//!
//! Before `htmd`, [`super::html_clean::preclean`] drops page chrome and
//! handles `data:` images.
//!
//! TXT goes HTML → Markdown → readable text ([`super::markdown::markdown_to_text`]),
//! so both outputs agree on structure.
//!
//! Also: a small tag scanner ([`rewrite_tags`]) used to fix image paths and drop
//! remote content before HTML is handed to an Office engine.

use super::markdown::markdown_to_text;
use super::plain::{crlf, read_text, unreadable, write_result};
use crate::converter::{TaskCtx, TaskOutput};
use crate::error::ConvertError;
use crate::job::Task;
use crate::markdown::ImageFolder;
use crate::office::OfficeConvertOpts;
use std::path::{Path, PathBuf};

pub fn to_markdown(
    task: &Task,
    input: &Path,
    opts: &OfficeConvertOpts,
    ctx: &TaskCtx<'_>,
) -> Result<TaskOutput, ConvertError> {
    let html = read_text(input)?;
    ctx.check_cancel()?;
    let folder = task.output.folder_for(input);
    let mut images = ImageFolder::new(&folder, input, opts.images);
    let html = super::html_clean::preclean(&html, &mut images)?;
    let md = html_to_markdown(&html)?;
    ctx.check_cancel()?;
    crate::markdown::write_markdown(&folder, input, &md, images)
}

pub fn to_text(
    task: &Task,
    input: &Path,
    _opts: &OfficeConvertOpts,
    ctx: &TaskCtx<'_>,
) -> Result<TaskOutput, ConvertError> {
    let html = read_text(input)?;
    ctx.check_cancel()?;
    // Text has no pictures: nothing is saved.
    let mut no_images = ImageFolder::new(&task.output.folder_for(input), input, false);
    let html = super::html_clean::preclean(&html, &mut no_images)?;
    let md = html_to_markdown(&html)?;
    let text = markdown_to_text(&md);
    write_result(task, input, "txt", crlf(&text).as_bytes())
}

/// Deepest element nesting converted (deeper pages would overflow the stack).
const MAX_DEPTH: usize = 256;

/// HTML → Markdown (ATX headings, fenced code, `-` bullets, pipe tables).
pub(crate) fn html_to_markdown(html: &str) -> Result<String, ConvertError> {
    if nesting_depth(html) > MAX_DEPTH {
        return Err(ConvertError::new(
            crate::error::ErrorKind::BadInput,
            "This document is nested too deeply to convert.",
            format!("HTML elements nested deeper than {MAX_DEPTH} levels"),
        ));
    }
    let options = htmd::options::Options {
        bullet_list_marker: htmd::options::BulletListMarker::Dash,
        ul_bullet_spacing: 1,
        ol_number_spacing: 1,
        ..Default::default()
    };
    let converter = htmd::HtmlToMarkdown::builder()
        .options(options)
        .skip_tags(vec![
            "head", "script", "style", "noscript", "template", "iframe", "object", "embed",
            "canvas", "svg",
        ])
        .build();
    let mut md = converter
        .convert(&flatten_cells_and_items(html))
        .map_err(unreadable)?;
    md.truncate(md.trim_end().len());
    md.push('\n');
    // htmd writes `|` in table cells as `&#124;`; `\|` reads better.
    if md.contains("&#124;") {
        md = md
            .lines()
            .map(|l| {
                if l.trim_start().starts_with('|') {
                    l.replace("&#124;", "\\|")
                } else {
                    l.to_owned()
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        md.push('\n');
    }
    Ok(md)
}

/// How deeply elements nest, roughly as a browser would build the tree: void
/// elements, `<x/>` and elements whose end tag is optional (`p`, `li`, `td`,
/// …: they can't hold themselves) don't count. May over-count sloppy HTML.
fn nesting_depth(html: &str) -> usize {
    const SKIP: &[&str] = &[
        "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "source",
        "track", "wbr", "param", "keygen", "p", "li", "dt", "dd", "tr", "td", "th", "thead",
        "tbody", "tfoot", "option", "optgroup", "colgroup", "rb", "rt", "rp", "rtc", "html",
        "head", "body",
    ];
    let (mut depth, mut max) = (0usize, 0usize);
    let mut rest = html;
    while let Some(lt) = rest.find('<') {
        rest = &rest[lt + 1..];
        if let Some(after) = rest.strip_prefix("!--") {
            rest = after.find("-->").map_or("", |e| &after[e + 3..]);
            continue;
        }
        let closing = rest.starts_with('/');
        let name_part = &rest[usize::from(closing)..];
        let len = name_part
            .bytes()
            .take_while(|b| b.is_ascii_alphanumeric())
            .count();
        if len == 0 || !name_part.as_bytes()[0].is_ascii_alphabetic() {
            continue;
        }
        let name = name_part[..len].to_ascii_lowercase();
        let gt = rest.find('>').unwrap_or(rest.len());
        let self_closing = rest[..gt].ends_with('/');
        rest = &rest[gt..];
        if self_closing || SKIP.contains(&name.as_str()) {
            continue;
        }
        if closing {
            depth = depth.saturating_sub(1);
        } else {
            depth += 1;
            max = max.max(depth);
        }
    }
    max
}

/// Prepare HTML for Markdown tables and lists:
/// - Office engines wrap table cells and list items in `<p>`, which makes them
///   block content: Markdown tables can't hold that, and lists come out loose.
///   Unwrap paragraphs (and line breaks in cells) there.
/// - Markdown tables need a header row; tables without `<th>` (all Word
///   tables) would become loose paragraphs, so their first row becomes the header.
fn flatten_cells_and_items(html: &str) -> String {
    let mut cells = 0usize;
    let mut items = 0usize;
    // Per open table: still in its first row.
    let mut first_row: Vec<bool> = Vec::new();
    rewrite_tags(html, |name, attrs| {
        let in_first_row = first_row.last().copied().unwrap_or(false);
        match name {
            "table" => first_row.push(true),
            "/table" => {
                first_row.pop();
            }
            "/tr" => {
                if let Some(f) = first_row.last_mut() {
                    *f = false;
                }
            }
            "td" if in_first_row => {
                cells += 1;
                let attrs: Vec<(String, Option<String>)> = attrs
                    .iter()
                    .map(|a| (a.name.to_owned(), a.value.map(attr_text)))
                    .collect();
                return TagAction::Replace(build_tag("th", &attrs));
            }
            "/td" if in_first_row => {
                cells = cells.saturating_sub(1);
                return TagAction::Replace("</th>".to_owned());
            }
            "td" | "th" => cells += 1,
            "/td" | "/th" => cells = cells.saturating_sub(1),
            "li" => items += 1,
            "/li" => items = items.saturating_sub(1),
            "p" | "div" if cells > 0 || items > 0 => return TagAction::Drop,
            "/p" | "/div" if cells > 0 || items > 0 => return TagAction::Replace(" ".to_owned()),
            "br" if cells > 0 => return TagAction::Replace(" ".to_owned()),
            _ => {}
        }
        TagAction::Keep
    })
}

// ---------------------------------------------------------------- tag scanner

/// What to do with one start tag found by [`rewrite_tags`].
pub(crate) enum TagAction {
    Keep,
    /// Replace the tag text (`<img …>`) with this.
    Replace(String),
    /// Remove the tag (only the start tag).
    Drop,
    /// Remove the whole element up to its end tag (`<script>…</script>`).
    DropElement,
}

/// One attribute of a start tag.
pub(crate) struct Attr<'a> {
    pub name: &'a str,
    /// Raw value (entities not decoded), `None` for `<input disabled>`.
    pub value: Option<&'a str>,
}

/// Value with the common entities decoded (`&amp;` in URLs).
pub(crate) fn attr_text(raw: &str) -> String {
    raw.replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

pub(crate) fn attr_escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('"', "&quot;")
}

/// Rebuild a start tag from its name and attributes.
pub(crate) fn build_tag(name: &str, attrs: &[(String, Option<String>)]) -> String {
    let mut tag = format!("<{name}");
    for (n, v) in attrs {
        tag.push(' ');
        tag.push_str(n);
        if let Some(v) = v {
            tag.push_str("=\"");
            tag.push_str(&attr_escape(v));
            tag.push('"');
        }
    }
    tag.push('>');
    tag
}

/// Walk the tags of `html`; `f(lowercase name, attributes)` decides what
/// happens to each. End tags are reported as `/name` (no attributes; don't
/// answer `DropElement` for them). Comments and text are copied unchanged. Not a full
/// HTML parser: enough for `<img src>`/`<link>`/`<script>` in real pages.
pub(crate) fn rewrite_tags(
    html: &str,
    mut f: impl FnMut(&str, &[Attr<'_>]) -> TagAction,
) -> String {
    let bytes = html.as_bytes();
    let mut out = String::with_capacity(html.len());
    let mut copied = 0; // html[..copied] is already in `out`
    let mut i = 0;
    while let Some(off) = html[i..].find('<') {
        let lt = i + off;
        // Comments: skip whole.
        if html[lt..].starts_with("<!--") {
            i = html[lt..].find("-->").map_or(html.len(), |e| lt + e + 3);
            continue;
        }
        let closing = html[lt + 1..].starts_with('/');
        let name_start = lt + 1 + usize::from(closing);
        let name_len = html[name_start..]
            .bytes()
            .take_while(|b| b.is_ascii_alphanumeric())
            .count();
        if name_len == 0 || !bytes[name_start].is_ascii_alphabetic() {
            i = lt + 1;
            continue;
        }
        let mut name = html[name_start..name_start + name_len].to_ascii_lowercase();
        if closing {
            name.insert(0, '/');
        }
        let (attrs, end) = parse_attrs(html, name_start + name_len);
        let action = f(&name, &attrs);
        let tag_end = end; // index just past '>'
        match action {
            TagAction::Keep => {}
            TagAction::Replace(text) => {
                out.push_str(&html[copied..lt]);
                out.push_str(&text);
                copied = tag_end;
            }
            TagAction::Drop => {
                out.push_str(&html[copied..lt]);
                copied = tag_end;
            }
            TagAction::DropElement if closing => {
                out.push_str(&html[copied..lt]);
                copied = tag_end;
            }
            TagAction::DropElement => {
                out.push_str(&html[copied..lt]);
                let close = format!("</{name}");
                let lower_rest = html[tag_end..].to_ascii_lowercase();
                copied = match lower_rest.find(&close) {
                    Some(c) => {
                        let c = tag_end + c;
                        html[c..].find('>').map_or(html.len(), |g| c + g + 1)
                    }
                    None => html.len(),
                };
                i = copied;
                continue;
            }
        }
        i = tag_end;
    }
    out.push_str(&html[copied.min(html.len())..]);
    out
}

/// Parse attributes from `pos` (just after the tag name) to the closing `>`.
/// Returns the attributes and the index just past `>` (or the end of input).
fn parse_attrs(html: &str, mut pos: usize) -> (Vec<Attr<'_>>, usize) {
    let bytes = html.as_bytes();
    let mut attrs = Vec::new();
    let n = bytes.len();
    loop {
        while pos < n && (bytes[pos].is_ascii_whitespace() || bytes[pos] == b'/') {
            pos += 1;
        }
        if pos >= n {
            return (attrs, n);
        }
        if bytes[pos] == b'>' {
            return (attrs, pos + 1);
        }
        let name_start = pos;
        while pos < n
            && !matches!(bytes[pos], b'=' | b'>' | b'/')
            && !bytes[pos].is_ascii_whitespace()
        {
            pos += 1;
        }
        let name = &html[name_start..pos];
        while pos < n && bytes[pos].is_ascii_whitespace() {
            pos += 1;
        }
        let mut value = None;
        if pos < n && bytes[pos] == b'=' {
            pos += 1;
            while pos < n && bytes[pos].is_ascii_whitespace() {
                pos += 1;
            }
            if pos < n && (bytes[pos] == b'"' || bytes[pos] == b'\'') {
                let q = bytes[pos];
                let start = pos + 1;
                let end = html[start..]
                    .bytes()
                    .position(|b| b == q)
                    .map_or(n, |e| start + e);
                value = Some(&html[start..end]);
                pos = (end + 1).min(n);
            } else {
                let start = pos;
                while pos < n && !bytes[pos].is_ascii_whitespace() && bytes[pos] != b'>' {
                    pos += 1;
                }
                value = Some(&html[start..pos]);
            }
        }
        if !name.is_empty() {
            attrs.push(Attr { name, value });
        }
    }
}

/// The local file a relative URL (`img/a%20b.png`) points to, only if it is
/// really inside `base`. The value is decoded first (entities, then percent
/// escapes) and only then checked, so `%5C%5Chost%5Cx.png`, `C%3A/x.png`,
/// `file:`, `\\?\`, `/x`, `..` and anything resolving outside `base` (also
/// through links) give `None`.
pub(crate) fn local_file_in(url: &str, base: &Path) -> Option<PathBuf> {
    let decoded = url_path_to_relative(&decode_entities(url));
    let rel = decoded.trim();
    if rel.is_empty()
        || rel.starts_with('\\')
        || rel.contains(':') // drive letters, schemes, alternate data streams
        || rel.chars().any(char::is_control)
        || rel.split('\\').any(|part| part == "..")
    {
        return None;
    }
    let path = Path::new(rel);
    if path.is_absolute() || path.has_root() {
        return None;
    }
    let base = std::fs::canonicalize(base).ok()?;
    let file = std::fs::canonicalize(base.join(path)).ok()?;
    (file.starts_with(&base) && file.is_file()).then_some(file)
}

/// All character references decoded (`&#58;`, `&#x3A;`, `&colon;` and the
/// usual named ones), as an HTML parser would read the attribute.
pub(crate) fn decode_entities(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut rest = raw;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        rest = &rest[amp..];
        let end = rest
            .char_indices()
            .skip(1)
            .find(|&(_, c)| !(c.is_ascii_alphanumeric() || c == '#'))
            .map_or(rest.len(), |(i, _)| i);
        let body = &rest[1..end];
        let decoded = if let Some(num) = body.strip_prefix('#') {
            let n = match num.strip_prefix(['x', 'X']) {
                Some(hex) => u32::from_str_radix(hex, 16).ok(),
                None => num.parse::<u32>().ok(),
            };
            n.map(|n| char::from_u32(n).unwrap_or('\u{FFFD}'))
        } else {
            match body.to_ascii_lowercase().as_str() {
                "amp" => Some('&'),
                "quot" => Some('"'),
                "apos" => Some('\''),
                "lt" => Some('<'),
                "gt" => Some('>'),
                "colon" => Some(':'),
                "sol" => Some('/'),
                "bsol" => Some('\\'),
                "period" => Some('.'),
                "percnt" => Some('%'),
                "tab" => Some('\t'),
                "newline" => Some('\n'),
                "nbsp" => Some('\u{A0}'),
                _ => None,
            }
        };
        match decoded {
            Some(c) => {
                out.push(c);
                let semi = rest[end..].starts_with(';');
                rest = &rest[end + usize::from(semi)..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// `a%20b.png?x#y` → `a b.png` (percent-decoded UTF-8, query/fragment removed).
fn url_path_to_relative(url: &str) -> String {
    let path = url.trim().split(['?', '#']).next().unwrap_or_default();
    let bytes = path.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = |b: u8| (b as char).to_digit(16);
        if bytes[i] == b'%'
            && i + 3 <= bytes.len() // a trailing "%41" too
            && let (Some(h), Some(l)) = (hex(bytes[i + 1]), hex(bytes[i + 2]))
        {
            out.push((h * 16 + l) as u8);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).replace('/', "\\")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markdown_drops_scripts_and_keeps_structure() {
        let html = "<html><head><title>T</title><style>p{}</style><script>alert(1)</script></head>\
<body><h1>Hi</h1><ul><li>a</li><li><b>b</b></li></ul>\
<table><tr><th>X</th><th>Y</th></tr><tr><td>1</td><td>2</td></tr></table>\
<p><img src=\"pic.png\" alt=\"Pic\"> <a href=\"https://e.com\">e</a></p>\
<script>alert(2)</script></body></html>";
        let md = html_to_markdown(html).unwrap();
        assert!(md.starts_with("# Hi"), "{md}");
        assert!(md.contains("- a\n- **b**"), "{md}");
        assert!(md.contains("| X | Y |"), "{md}");
        assert!(md.contains("![Pic](pic.png)"), "{md}");
        assert!(md.contains("[e](https://e.com)"), "{md}");
        assert!(!md.contains("alert") && !md.contains("p{}"), "{md}");
        assert!(!md.lines().any(|l| l.trim() == "T"), "{md}");
    }

    #[test]
    fn deep_nesting_fails_plainly_and_quickly() {
        let deep = format!("{}x{}", "<div>".repeat(2_000), "</div>".repeat(2_000));
        let err = html_to_markdown(&deep).unwrap_err();
        assert!(err.message.contains("nested too deeply"), "{err}");
        let start = std::time::Instant::now();
        let huge = "<div>".repeat(50_000);
        assert!(html_to_markdown(&huge).is_err());
        assert!(start.elapsed().as_secs() < 5);
        // Many unclosed paragraphs, list items, void and self-closed tags are flat.
        let flat = format!(
            "{}{}{}",
            "<p>a<br><img src=x.png>".repeat(1_000),
            "<ul><li>i".repeat(10),
            "<svg><path d='M0'/></svg>".repeat(1_000)
        );
        assert!(nesting_depth(&flat) < 20);
        let ok = format!("{}x{}", "<div>".repeat(200), "</div>".repeat(200));
        assert!(html_to_markdown(&ok).unwrap().contains('x'));
    }

    #[test]
    fn pipes_in_cells_are_escaped() {
        let md =
            html_to_markdown("<table><tr><th>A</th></tr><tr><td>x|y</td></tr></table>").unwrap();
        assert!(md.contains("x\\|y") && !md.contains("&#124;"), "{md}");
    }

    #[test]
    fn office_style_tables_and_lists_become_markdown() {
        let html = "<table><col width=1><tr><td><p>Region</p></td><td><p>Q1</p></td></tr>\
<tr><td><p><font>North</font></p></td><td><p>120</p></td></tr></table>\
<ul><li><p>one</p></li><li><p>two</p></li></ul>";
        let md = html_to_markdown(html).unwrap();
        assert!(md.contains("| Region | Q1  |"), "{md}");
        assert!(md.contains("| North  | 120 |"), "{md}");
        assert!(md.contains("- one\n- two"), "{md}");
    }

    #[test]
    fn tag_scanner_rewrites_and_drops() {
        let html = "<p>x</p><IMG alt='a b' SRC=pic.png><!-- <img src=c.png> -->\
<script src=x.js>var a = '<img src=no.png>';</script><link rel=stylesheet href=\"http://e/s.css\">end";
        let out = rewrite_tags(html, |name, attrs| match name {
            "img" => {
                let src = attrs
                    .iter()
                    .find(|a| a.name.eq_ignore_ascii_case("src"))
                    .and_then(|a| a.value)
                    .unwrap_or_default();
                TagAction::Replace(format!("<img src=\"NEW-{src}\">"))
            }
            "script" => TagAction::DropElement,
            "link" => TagAction::Drop,
            _ => TagAction::Keep,
        });
        assert_eq!(
            out,
            "<p>x</p><img src=\"NEW-pic.png\"><!-- <img src=c.png> -->end"
        );
    }

    #[test]
    fn url_helpers() {
        assert_eq!(
            url_path_to_relative("img/a%20b%C3%A9.png?v=1"),
            "img\\a bé.png"
        );
        assert_eq!(url_path_to_relative("a%41"), "aA", "trailing escape");
        assert_eq!(url_path_to_relative("a%4"), "a%4");
        assert_eq!(
            decode_entities("a&#58;b&#x2F;c&colon;&amp;&bogus;&"),
            "a:b/c:&&bogus;&"
        );
    }

    #[test]
    fn only_files_inside_the_base_are_local() {
        let root = tempfile::tempdir().unwrap();
        let base = root.path().join("doc");
        std::fs::create_dir_all(base.join("img")).unwrap();
        std::fs::write(base.join("img").join("a b.png"), b"x").unwrap();
        std::fs::write(root.path().join("secret.png"), b"s").unwrap();
        let secret = root.path().join("secret.png");
        let secret_url = secret.to_string_lossy().replace('\\', "/");
        let ok = local_file_in("img/a%20b.png", &base).unwrap();
        assert!(ok.ends_with("a b.png"));
        assert!(local_file_in("img&#47;a%20b.png?v=2#x", &base).is_some());
        for bad in [
            "../secret.png".to_owned(),
            "img/../../secret.png".to_owned(),
            "%2E%2E/secret.png".to_owned(),
            "..%5Csecret.png".to_owned(),
            "%5C%5Chost%5Cs%5Cx.png".to_owned(),
            "//host/s/x.png".to_owned(),
            "https://example.invalid/x.png".to_owned(),
            r"\\host\s\x.png".to_owned(),
            r"\\?\C:\x.png".to_owned(),
            "/secret.png".to_owned(),
            secret.to_string_lossy().into_owned(),
            secret_url.replace(':', "%3A"),
            format!("file:///{secret_url}"),
            format!("file&#58;///{secret_url}"),
            "file://localhost/c$/x.png".to_owned(),
            "data:image/png;base64,AAAA".to_owned(),
            "img/a b.png:stream".to_owned(),
            "gone.png".to_owned(),
            "img".to_owned(),
            String::new(),
        ] {
            assert_eq!(local_file_in(&bad, &base), None, "{bad}");
        }
    }
}
