// SPDX-License-Identifier: GPL-3.0-or-later
//! HTML pre-clean before HTML → Markdown (SPEC §4.6).
//!
//! Called by [`super::html::to_markdown`] / [`super::html::to_text`] before `htmd`:
//! - drops `script`, `style`, `noscript`, `template`, `nav`, `iframe`, form
//!   controls (`input`, `select`, `textarea`, `button`; a `form` wrapper keeps
//!   its content, as ASP.NET pages wrap the whole body in one), and page chrome
//!   (`header`, `footer`, `aside`) outside `main`/`article` when the page has one;
//! - `data:` images are saved through [`ImageFolder`] (decoded, size-capped)
//!   and linked; with pictures off (or undecodable) only the alt text stays;
//! - `javascript:`, `file:` and other unsafe links lose the link (the text stays);
//! - the `<title>` becomes the `# Title` when the page has no `<h1>`.
//!
//! Never fetches anything; relative images and other links are kept as they are.

use super::html::{Attr, TagAction, attr_text, build_tag, decode_entities, rewrite_tags};
use crate::error::ConvertError;
use crate::markdown::{ImageFolder, is_safe_link};
use base64::Engine as _;

/// Larger `data:` URIs are not decoded (alt text only).
const MAX_DATA_URI: usize = 32 * 1024 * 1024;

pub fn preclean(html: &str, images: &mut ImageFolder) -> Result<String, ConvertError> {
    let lower = html.to_ascii_lowercase();
    let has_main = has_tag(&lower, "main") || has_tag(&lower, "article");
    let title = (!has_tag(&lower, "h1"))
        .then(|| page_title(html, &lower))
        .flatten();
    let mut content_depth = 0usize;
    let mut saw_body = false;
    let mut error = None;
    let cleaned = rewrite_tags(html, |name, attrs| match name {
        "script" | "style" | "noscript" | "template" | "nav" | "iframe" | "select" | "textarea"
        | "button" | "object" | "embed" => TagAction::DropElement,
        "input" => TagAction::Drop,
        "header" | "footer" | "aside" if has_main && content_depth == 0 => TagAction::DropElement,
        "main" | "article" => {
            content_depth += 1;
            TagAction::Keep
        }
        "/main" | "/article" => {
            content_depth = content_depth.saturating_sub(1);
            TagAction::Keep
        }
        "body" => {
            saw_body = true;
            match &title {
                Some(t) => TagAction::Replace(format!("{}<h1>{t}</h1>", rebuild("body", attrs))),
                None => TagAction::Keep,
            }
        }
        "a" if attrs
            .iter()
            .any(|a| a.name.eq_ignore_ascii_case("href") && a.value.is_some_and(is_script_url)) =>
        {
            TagAction::Replace("<a>".to_owned())
        }
        "img" => match data_image(attrs, images) {
            Ok(action) => action,
            Err(e) => {
                error.get_or_insert(e);
                TagAction::Keep
            }
        },
        _ => TagAction::Keep,
    });
    if let Some(e) = error {
        return Err(e);
    }
    Ok(match title {
        Some(t) if !saw_body => format!("<h1>{t}</h1>{cleaned}"),
        _ => cleaned,
    })
}

/// `<name` followed by `>`, whitespace or `/` somewhere in lower-cased HTML.
fn has_tag(lower: &str, name: &str) -> bool {
    let open = format!("<{name}");
    lower.match_indices(&open).any(|(i, _)| {
        lower[i + open.len()..]
            .bytes()
            .next()
            .is_some_and(|b| b == b'>' || b == b'/' || b.is_ascii_whitespace())
    })
}

/// The `<title>` text (still HTML: entities kept, tags removed), if not empty.
fn page_title(html: &str, lower: &str) -> Option<String> {
    let start = lower.find("<title")?;
    let start = start + lower[start..].find('>')? + 1;
    let end = start + lower[start..].find("</title")?;
    let text = rewrite_tags(&html[start..end], |_, _| TagAction::Drop);
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    (!text.is_empty()).then_some(text)
}

fn rebuild(name: &str, attrs: &[Attr<'_>]) -> String {
    let attrs: Vec<(String, Option<String>)> = attrs
        .iter()
        .map(|a| (a.name.to_owned(), a.value.map(attr_text)))
        .collect();
    build_tag(name, &attrs)
}

/// `javascript:`, `vbscript:`, `file:`, … (also hidden with entities, case or
/// whitespace): see [`is_safe_link`].
fn is_script_url(raw: &str) -> bool {
    !is_safe_link(&decode_entities(raw))
}

/// An `<img>` with a `data:` source: saved and relinked, or its alt text.
fn data_image(attrs: &[Attr<'_>], images: &mut ImageFolder) -> Result<TagAction, ConvertError> {
    let get = |n: &str| {
        attrs
            .iter()
            .find(|a| a.name.eq_ignore_ascii_case(n))
            .and_then(|a| a.value)
    };
    let Some(src) = get("src") else {
        return Ok(TagAction::Keep);
    };
    if !src
        .trim_start()
        .get(..5)
        .is_some_and(|s| s.eq_ignore_ascii_case("data:"))
    {
        return Ok(TagAction::Keep);
    }
    let alt = get("alt").map(decode_entities).unwrap_or_default();
    let link = match decode_data_uri(src) {
        Some(bytes) => images.save(&bytes, None)?,
        None => None,
    };
    Ok(match link {
        Some(link) => TagAction::Replace(build_tag(
            "img",
            &[("src".into(), Some(link)), ("alt".into(), Some(alt))],
        )),
        None => TagAction::Replace(html_escape(&alt)),
    })
}

/// Bytes of a base64 `data:` URI (`data:image/png;base64,…`); `None` for
/// non-base64, broken or oversized ones.
fn decode_data_uri(src: &str) -> Option<Vec<u8>> {
    if src.len() > MAX_DATA_URI {
        return None;
    }
    let (meta, payload) = src.trim().split_once(',')?;
    if !meta.to_ascii_lowercase().ends_with(";base64") {
        return None;
    }
    let clean: String = payload
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '=')
        .collect();
    base64::engine::general_purpose::STANDARD_NO_PAD
        .decode(clean)
        .ok()
}

fn html_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOT: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAIAAACQd1PeAAAADElEQVR4nGP4z8AAAAMBAQDJ/pLvAAAAAElFTkSuQmCC";

    fn clean(html: &str, pictures: bool) -> (String, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let mut f = ImageFolder::new(dir.path(), std::path::Path::new("page.html"), pictures);
        let out = preclean(html, &mut f).unwrap();
        f.commit();
        (out, dir)
    }

    #[test]
    fn drops_chrome_only_with_main() {
        let page = "<header>Top</header><main><article><header>Kept</header></article></main><footer>F</footer>";
        let (out, _d) = clean(page, true);
        assert_eq!(out, "<main><article><header>Kept</header></article></main>");
        let (out, _d) = clean("<header>Site</header><p>x</p><nav>menu</nav>", true);
        assert_eq!(out, "<header>Site</header><p>x</p>");
    }

    #[test]
    fn title_becomes_h1_without_one() {
        let (out, _d) = clean(
            "<html><head><title> A &amp; B </title></head><body class=x><p>t</p></body></html>",
            true,
        );
        assert!(
            out.contains("<body class=\"x\"><h1>A &amp; B</h1><p>t</p>"),
            "{out}"
        );
        let (out, _d) = clean("<title>T</title><h1>Real</h1>", true);
        assert!(!out.contains("<h1>T</h1>"));
    }

    #[test]
    fn data_images_and_script_links() {
        let html = format!(
            "<img src=\"data:image/png;base64,{DOT}\" alt=\"Red &amp; dot\"><a href=\" Java&#83;cript:alert(1)\">x</a><img src=\"a.png\">"
        );
        let (out, d) = clean(&html, true);
        assert_eq!(
            out,
            "<img src=\"page%20%28images%29/image-1.png\" alt=\"Red &amp; dot\"><a>x</a><img src=\"a.png\">"
        );
        assert!(d.path().join("page (images)/image-1.png").is_file());
        let (out, d) = clean(&html, false);
        assert!(out.starts_with("Red &amp; dot<a>"), "{out}");
        assert_eq!(std::fs::read_dir(d.path()).unwrap().count(), 0);
        assert_eq!(decode_data_uri("data:image/png,abc"), None);
    }
}
