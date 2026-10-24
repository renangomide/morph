// SPDX-License-Identifier: GPL-3.0-or-later
//! Text-like → DOCX / PDF / ODT through the Office engine.
//!
//! Markdown / TXT are converted to an HTML page natively; HTML inputs are used
//! as they are. The page is then prepared for the engine ([`prepare_for_engine`])
//! and written to the task's temp folder:
//!
//! - pictures with relative paths inside the input's folder
//!   ([`super::html::local_file_in`]; the engine only sees a copy in a temp
//!   folder) are embedded as `data:` URLs, so DOCX/ODT really contain them;
//!   every other picture (remote, `file:`, `\\server`, absolute, outside the
//!   folder, missing) is replaced by its description,
//! - the only URLs the engine gets are `data:` ones (and plain `<a href>`
//!   links, which nobody fetches): `<link>`, `<script>`, frames, objects,
//!   `<base>`, meta refresh, `background=` and every other URL attribute are
//!   removed, and so are `style=""` / `<style>` blocks that could load
//!   something (`url(`, `@import`, CSS escapes). So the engine never reads
//!   another local file or contacts a server (no NTLM leak),
//! - the page is re-declared as UTF-8 (it was decoded to Unicode).
//!
//! Then `crate::office::engine_convert::export(html, Family::Text, format,
//! engine, ctx)` and `naming::publish_file`.

use super::html::{Attr, TagAction, attr_escape, decode_entities, local_file_in, rewrite_tags};
use super::markdown::markdown_to_html_page;
use super::plain::{escape_html, read_text, stem, text_to_html_page};
use crate::converter::{TaskCtx, TaskOutput};
use crate::error::ConvertError;
use crate::job::Task;
use crate::naming::{self, Suffix};
use crate::office::{DocFormat, EngineChoice, Family, OfficeConvertOpts, engine_convert};
use std::path::Path;

pub fn convert(
    task: &Task,
    input: &Path,
    opts: &OfficeConvertOpts,
    engine: &EngineChoice,
    ctx: &TaskCtx<'_>,
) -> Result<TaskOutput, ConvertError> {
    let title = stem(input);
    let source = read_text(input)?;
    let page = match DocFormat::of_path(input) {
        Some(DocFormat::Md) => markdown_to_html_page(&source, &title),
        Some(DocFormat::Txt) => text_to_html_page(&source, &title),
        Some(DocFormat::Html) => source,
        other => {
            return Err(ConvertError::unsupported(
                "This file type isn't supported.",
                format!("{other:?} as text input"),
            ));
        }
    };
    let input_abs = std::path::absolute(input)?;
    let base = input_abs.parent().unwrap_or(Path::new("."));
    let page = prepare_for_engine(&page, base);
    ctx.check_cancel()?;

    let temp = ctx.temp_dir()?.join(format!("{title}.html"));
    std::fs::write(&temp, page.as_bytes())?;
    ctx.progress(0.1, None);

    let result = engine_convert::export(&temp, Family::Text, opts.format, engine, ctx)?;
    ctx.check_cancel()?;
    let folder = task.output.folder_for(input);
    let name = naming::output_name(input, opts.format.extension(), Suffix::Converted);
    let out = naming::publish_file(&result, &folder, &name)?;
    ctx.progress(1.0, None);
    Ok(TaskOutput::single(out))
}

fn attr<'a>(attrs: &'a [Attr<'a>], name: &str) -> Option<&'a str> {
    attrs
        .iter()
        .find(|a| a.name.eq_ignore_ascii_case(name))
        .and_then(|a| a.value)
}

/// Attributes that make an engine load something. Any `…href` too (except a
/// plain `<a href>` / `<area href>` link, which is never fetched).
const URL_ATTRS: &[&str] = &[
    "src",
    "srcset",
    "lowsrc",
    "dynsrc",
    "background",
    "poster",
    "data",
    "codebase",
    "classid",
    "archive",
    "action",
    "formaction",
    "cite",
    "longdesc",
    "usemap",
    "profile",
    "manifest",
    "ping",
    "srcdoc",
];

fn loads_something(tag: &str, attr: &Attr<'_>) -> bool {
    let name = attr.name.to_ascii_lowercase();
    if name.ends_with("href") {
        return !(name == "href" && matches!(tag, "a" | "area"));
    }
    if name.starts_with("on") {
        return true; // event handlers: nothing runs them, but nothing needs them
    }
    if name == "style" {
        return attr
            .value
            .is_some_and(|v| css_may_load(&decode_entities(v)));
    }
    URL_ATTRS.contains(&name.as_str())
}

/// CSS that could reference a file or a server (`url(`, `@import`,
/// `image-set(`, `src(`, old `expression(`), or hide one behind CSS escapes.
fn css_may_load(css: &str) -> bool {
    let l = css.to_ascii_lowercase();
    [
        "url(",
        "@import",
        "image",
        "src",
        "expression",
        "\\",
        "behavior",
    ]
    .iter()
    .any(|p| l.contains(p))
}

/// Rebuild a start tag with only the attributes `keep` accepts, values as
/// they were (entities still encoded).
fn rebuild(
    tag: &str,
    attrs: &[Attr<'_>],
    extra: &[(&str, &str)],
    keep: impl Fn(&Attr<'_>) -> bool,
) -> String {
    let mut out = format!("<{tag}");
    for (name, value) in extra {
        out.push_str(&format!(" {name}=\"{}\"", attr_escape(value)));
    }
    for a in attrs.iter().filter(|a| keep(a)) {
        out.push(' ');
        out.push_str(a.name);
        match a.value {
            None => {}
            Some(v) if !v.contains('"') => out.push_str(&format!("=\"{v}\"")),
            Some(v) if !v.contains('\'') => out.push_str(&format!("='{v}'")),
            Some(v) => out.push_str(&format!("=\"{}\"", attr_escape(&decode_entities(v)))),
        }
    }
    out.push('>');
    out
}

/// `data:` pictures the engine may decode. SVG is left out: an SVG can point
/// at other files itself.
fn is_safe_data_picture(src: &str) -> bool {
    let s = src.trim().to_ascii_lowercase();
    [
        "data:image/png",
        "data:image/jpeg",
        "data:image/jpg",
        "data:image/gif",
        "data:image/bmp",
        "data:image/webp",
        "data:image/tiff",
    ]
    .iter()
    .any(|p| s.starts_with(p))
}

/// Make an HTML page safe and self-contained for the engine (see module docs).
pub(crate) fn prepare_for_engine(html: &str, base: &Path) -> String {
    let html = drop_loading_style_blocks(html);
    let mut saw_head = false;
    let out = rewrite_tags(&html, |name, attrs| match name {
        "img" => {
            let src = attr(attrs, "src").map(decode_entities).unwrap_or_default();
            let new_src = if is_safe_data_picture(&src) {
                Some(src.trim().to_owned())
            } else {
                local_file_in(&src, base).and_then(|file| embed(&file))
            };
            match new_src {
                Some(src) => TagAction::Replace(rebuild("img", attrs, &[("src", &src)], |a| {
                    !loads_something("img", a)
                })),
                // Keep the description in its place.
                None => {
                    let alt = attr(attrs, "alt").map(decode_entities).unwrap_or_default();
                    TagAction::Replace(escape_html(&alt))
                }
            }
        }
        "head" => {
            saw_head = true;
            TagAction::Replace("<head><meta charset=\"utf-8\">".to_owned())
        }
        // charset, content-type, refresh, set-cookie, link headers…
        "meta" if attr(attrs, "charset").is_some() || attr(attrs, "http-equiv").is_some() => {
            TagAction::Drop
        }
        "link" | "iframe" | "frame" | "frameset" | "embed" | "base" | "source" | "track"
        | "param" | "portal" => TagAction::Drop,
        "script" | "object" | "applet" | "video" | "audio" | "noscript" => TagAction::DropElement,
        _ if name.starts_with('/') => TagAction::Keep,
        _ if attrs.iter().any(|a| loads_something(name, a)) => {
            TagAction::Replace(rebuild(name, attrs, &[], |a| !loads_something(name, a)))
        }
        _ => TagAction::Keep,
    });
    if saw_head {
        out
    } else {
        format!("<meta charset=\"utf-8\">\n{out}")
    }
}

/// Remove `<style>…</style>` blocks whose CSS could load something
/// ([`css_may_load`]); harmless ones (Morph's own page style) stay.
fn drop_loading_style_blocks(html: &str) -> String {
    let lower = html.to_ascii_lowercase();
    let mut out = String::with_capacity(html.len());
    let mut at = 0;
    while let Some(off) = lower[at..].find("<style") {
        let start = at + off;
        let after = lower.as_bytes().get(start + 6).copied().unwrap_or(b'>');
        if !(after == b'>' || after == b'/' || after.is_ascii_whitespace()) {
            out.push_str(&html[at..start + 6]);
            at = start + 6;
            continue;
        }
        let end = lower[start..]
            .find("</style")
            .map(|c| start + c)
            .and_then(|c| lower[c..].find('>').map(|g| c + g + 1))
            .unwrap_or(html.len());
        out.push_str(&html[at..start]);
        let block = &html[start..end];
        let css_start = block.find('>').map_or(block.len(), |g| g + 1);
        if !css_may_load(&block[css_start..]) {
            out.push_str(block);
        }
        at = end;
    }
    out.push_str(&html[at..]);
    out
}

/// Pictures above this size are left out (the engine would struggle anyway).
const MAX_EMBED_BYTES: u64 = 64 * 1024 * 1024;

/// A local picture as a `data:` URL, so the engine embeds it in the document
/// (with a `file:///` link LibreOffice keeps a link in DOCX instead). `None`
/// when the file is missing, too big or not a picture type.
fn embed(path: &Path) -> Option<String> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    let mime = match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" | "jfif" => "image/jpeg",
        "gif" => "image/gif",
        "bmp" => "image/bmp",
        "webp" => "image/webp",
        // SVG only when it can't point at other files or servers.
        "svg" if svg_is_self_contained(path) => "image/svg+xml",
        "tif" | "tiff" => "image/tiff",
        _ => return None,
    };
    if std::fs::metadata(path).ok()?.len() > MAX_EMBED_BYTES {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    Some(format!("data:{mime};base64,{}", base64(&bytes)))
}

/// An SVG without links to other resources (`href`, `url(`, `@import`,
/// entities, XInclude).
fn svg_is_self_contained(path: &Path) -> bool {
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    if meta.len() > MAX_EMBED_BYTES {
        return false;
    }
    let Ok(text) = std::fs::read(path) else {
        return false;
    };
    let l = String::from_utf8_lossy(&text).to_ascii_lowercase();
    ![
        "href", "url(", "@import", "<!entity", "xinclude", "src=", "&#",
    ]
    .iter()
    .any(|p| l.contains(p))
}

fn base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for (i, shift) in [18u32, 12, 6, 0].into_iter().enumerate() {
            if i <= chunk.len() {
                out.push(TABLE[((n >> shift) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn base64_matches_rfc4648() {
        for (input, expected) in [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(super::base64(input.as_bytes()), expected);
        }
    }

    use super::*;

    #[test]
    fn images_become_absolute_and_remote_content_goes() {
        let html = "<!DOCTYPE html><html><head><meta charset=\"windows-1252\">\
<link rel=\"stylesheet\" href=\"https://cdn/x.css\"></head><body>\
<img src=\"img/a%20b.png\" alt=\"A &amp; B\" width=\"10\">\
<img src=\"https://example.com/r.png\" alt=\"remote\">\
<img src=\"data:image/png;base64,AAAA\"><img src=\"gone.png\" alt=\"Gone\"><script>x()</script></body></html>";
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("img")).unwrap();
        std::fs::write(dir.path().join("img").join("a b.png"), b"foo").unwrap();
        let out = prepare_for_engine(html, dir.path());
        assert!(out.contains("<head><meta charset=\"utf-8\">"), "{out}");
        assert!(!out.contains("windows-1252") && !out.contains("cdn/x.css"));
        assert!(
            out.contains("<img src=\"data:image/png;base64,Zm9v\" alt=\"A &amp; B\" width=\"10\">"),
            "{out}"
        );
        assert!(!out.contains("example.com") && out.contains("remote"));
        assert!(out.contains("data:image/png;base64,AAAA"));
        assert!(!out.contains("x()"));
        assert!(!out.contains("gone.png") && out.contains("Gone"));
    }

    /// Crafted pages: only local `file:///` / UNC-looking targets, never real hosts.
    #[test]
    fn engine_never_gets_a_path_or_server_to_load() {
        let root = tempfile::tempdir().unwrap();
        let base = root.path().join("doc");
        std::fs::create_dir(&base).unwrap();
        std::fs::write(base.join("ok.png"), b"foo").unwrap();
        std::fs::write(root.path().join("secret.png"), b"SECRET").unwrap();
        std::fs::write(
            base.join("evil.svg"),
            b"<svg><image href=\"file:///C:/x.png\"/></svg>",
        )
        .unwrap();
        std::fs::write(base.join("plain.svg"), b"<svg><rect width=\"1\"/></svg>").unwrap();
        let secret = root
            .path()
            .join("secret.png")
            .to_string_lossy()
            .replace('\\', "/");
        let html = format!(
            "<html><head><base href=\"file:///C:/\"><meta http-equiv=\"refresh\" content=\"0;url=file:///C:/\">\
<style>body{{color:red}}</style><style>body{{background:url(file:///C:/x.png)}}</style>\
<style>@import 'file:///C:/a.css';</style></head>\
<body background=\"file:///C:/bg.png\" style=\"color:blue\">\
<img src=\"%5C%5Clocalhost%5Cc$%5Cx.png\" alt=\"unc-escaped\">\
<img src=\"C%3A/Windows/win.ini\" alt=\"drive-escaped\">\
<img src=\"..%2Fsecret.png\" alt=\"dotdot\">\
<img src=\"file:///{secret}\" alt=\"fileurl\">\
<img src=\"file://localhost/c$/x.png\" alt=\"fileserver\">\
<img src=\"\\\\localhost\\c$\\x.png\" alt=\"unc\">\
<img src=\"file&#58;///{secret}\" alt=\"entity\">\
<img src=\"data:image/svg+xml;base64,AAAA\" alt=\"datasvg\">\
<img src=\"evil.svg\" alt=\"evilsvg\"><img src=\"plain.svg\" alt=\"plainsvg\">\
<img src=\"ok.png\" srcset=\"file:///C:/x.png 2x\" alt=\"good\">\
<div style=\"background-image:url('file:///C:/x.png')\">d</div>\
<p style=\"background:u\\72l(file:///C:/x.png)\">p</p>\
<table background=\"\\\\localhost\\s\\t.png\"><tr><td>t</td></tr></table>\
<svg><image href=\"file:///C:/x.png\"/><use xlink:href=\"file:///C:/u.svg#a\"/></svg>\
<input type=\"image\" src=\"file:///C:/i.png\"><video poster=\"file:///C:/v.png\"></video>\
<iframe src=\"file:///C:/f.html\"></iframe><object data=\"file:///C:/o.swf\">o</object>\
<link rel=\"stylesheet\" href=\"file:///C:/l.css\"><a href=\"https://example.invalid/\">link</a>\
</body></html>"
        );
        let out = prepare_for_engine(&html, &base);
        for gone in [
            "file:",
            "localhost",
            "C%3A",
            "win.ini",
            "secret",
            "SECRET",
            "U0VDUkVU",
            "url(",
            "@import",
            "background=",
            "<base",
            "refresh",
            "srcset",
            "u\\72l",
            "<iframe",
            "<object",
            "<link",
            "xlink",
            "evil.svg",
            "poster",
            "datasvg\"",
        ] {
            assert!(!out.contains(gone), "{gone} left in:\n{out}");
        }
        assert!(out.contains("<style>body{color:red}</style>"), "{out}");
        assert!(out.contains("style=\"color:blue\""), "{out}");
        assert!(
            out.contains("<img src=\"data:image/png;base64,Zm9v\" alt=\"good\">"),
            "{out}"
        );
        assert!(
            out.contains("data:image/svg+xml;base64,"),
            "plain SVG embedded: {out}"
        );
        for alt in [
            "unc-escaped",
            "drive-escaped",
            "dotdot",
            "fileurl",
            "unc",
            "evilsvg",
        ] {
            assert!(out.contains(alt), "{alt} kept as text");
        }
        assert!(
            out.contains("<a href=\"https://example.invalid/\">link</a>"),
            "{out}"
        );
    }
}
