// SPDX-License-Identifier: GPL-3.0-or-later
//! The OOXML package (zip + relationships) shared by the native DOCX and PPTX
//! readers ([`super::docx_markdown`], [`super::pptx_markdown`]).
//!
//! Hostile files: every part read is size-capped (zip bombs), part names are
//! looked up case-insensitively (OPC), pictures are read once per part and
//! all pictures of one document together are capped ([`MAX_PICTURES`]); over
//! a cap a picture keeps its alt text. Readers call [`check_depth`] in their
//! recursive walks so deeply nested XML fails plainly instead of overflowing
//! the stack.

use crate::error::{ConvertError, ErrorKind};
use crate::markdown::{self, ImageFolder};
use roxmltree::{Document, Node};
use std::collections::HashMap;
use std::fs::File;
use std::io::Read;
use std::path::Path;

/// Biggest XML part read.
pub const MAX_XML: u64 = 64 * 1024 * 1024;
/// Biggest single picture copied.
pub const MAX_MEDIA: u64 = 100 * 1024 * 1024;
/// All pictures of one document together (inflated bytes).
pub const MAX_PICTURES: u64 = 1024 * 1024 * 1024;
/// Deepest XML element the readers walk into.
pub const MAX_DEPTH: usize = 256;

pub struct Package {
    zip: zip::ZipArchive<File>,
    /// Lower-cased part name → link (`None`: missing, too big, not saved).
    pictures: HashMap<String, Option<String>>,
    picture_bytes: u64,
}

pub struct Rel {
    /// The full relationship type URI.
    pub kind: String,
    /// Package path for internal targets, the URL for external ones.
    pub target: String,
    pub external: bool,
}

impl Package {
    pub fn open(path: &Path) -> Result<Package, ConvertError> {
        let file = File::open(path)?;
        match zip::ZipArchive::new(file) {
            Ok(zip) => Ok(Package {
                zip,
                pictures: HashMap::new(),
                picture_bytes: 0,
            }),
            Err(e) => {
                // "Encrypt with password" stores the package in an OLE file.
                super::engine_convert::check_not_encrypted(path)?;
                Err(super::damaged_document(format!("{}: {e}", path.display())))
            }
        }
    }

    /// One part's bytes (`None` when missing), at most `cap` bytes.
    pub fn part(&mut self, name: &str, cap: u64) -> Result<Option<Vec<u8>>, ConvertError> {
        // Part names are case-insensitive in OPC.
        let index = match self.zip.index_for_name(name) {
            Some(i) => i,
            None => match self
                .zip
                .file_names()
                .position(|n| n.is_ok_and(|n| n.eq_ignore_ascii_case(name)))
            {
                Some(i) => i,
                None => return Ok(None),
            },
        };
        let mut f = self
            .zip
            .by_index(index)
            .map_err(|e| super::damaged_document(format!("{name}: {e}")))?;
        if f.size() > cap {
            return Err(too_big(name));
        }
        let mut buf = Vec::new();
        (&mut f)
            .take(cap + 1)
            .read_to_end(&mut buf)
            .map_err(|e| super::damaged_document(format!("{name}: {e}")))?;
        if buf.len() as u64 > cap {
            return Err(too_big(name));
        }
        Ok(Some(buf))
    }

    /// An XML part as text (BOM removed), `None` when missing.
    pub fn xml(&mut self, name: &str) -> Result<Option<String>, ConvertError> {
        let Some(bytes) = self.part(name, MAX_XML)? else {
            return Ok(None);
        };
        let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(&bytes);
        String::from_utf8(bytes.to_vec())
            .map(Some)
            .map_err(|e| super::damaged_document(format!("{name}: {e}")))
    }

    /// Relationships of `part` (`word/document.xml` → `word/_rels/document.xml.rels`;
    /// `""` → the package's `_rels/.rels`), keyed by id.
    pub fn rels(&mut self, part: &str) -> Result<HashMap<String, Rel>, ConvertError> {
        let (dir, name) = part.rsplit_once('/').unwrap_or(("", part));
        let rels_part = if dir.is_empty() {
            format!("_rels/{name}.rels")
        } else {
            format!("{dir}/_rels/{name}.rels")
        };
        match self.xml(&rels_part)? {
            Some(xml) => Ok(rels_of(&parse(&xml)?, dir)),
            None => Ok(HashMap::new()),
        }
    }

    /// Markdown for one picture: `![alt](link)` when the internal part
    /// `target` was saved, else the escaped alt text. A missing, oversized or
    /// over-budget picture keeps its alt text.
    pub fn picture_md(
        &mut self,
        images: &mut ImageFolder,
        target: Option<&str>,
        alt: &str,
    ) -> Result<String, ConvertError> {
        let link = match target {
            Some(t) if images.enabled() => self.picture_link(images, t)?,
            _ => None,
        };
        Ok(match link {
            Some(link) => markdown::image(alt, &link),
            None => markdown::escape_text(&markdown::one_line(alt)),
        })
    }

    fn picture_link(
        &mut self,
        images: &mut ImageFolder,
        part: &str,
    ) -> Result<Option<String>, ConvertError> {
        let key = part.to_ascii_lowercase();
        if let Some(link) = self.pictures.get(&key) {
            return Ok(link.clone());
        }
        let cap = MAX_MEDIA.min(MAX_PICTURES.saturating_sub(self.picture_bytes));
        let link = match self.part(part, cap) {
            Ok(Some(bytes)) => {
                self.picture_bytes += bytes.len() as u64;
                images.save(&bytes, Some(part))?
            }
            Ok(None) => None,
            Err(_) => {
                // Inflating it was work too: count it, so many bombs can't add up.
                self.picture_bytes += cap;
                None
            }
        };
        self.pictures.insert(key, link.clone());
        Ok(link)
    }
}

fn too_big(part: &str) -> ConvertError {
    ConvertError::new(
        ErrorKind::BadInput,
        "This document is too big to read.",
        format!("{part} is larger than the limit"),
    )
}

pub fn too_deep() -> ConvertError {
    ConvertError::new(
        ErrorKind::BadInput,
        "This document is nested too deeply to convert.",
        format!("XML nesting deeper than {MAX_DEPTH} levels"),
    )
}

/// Fails when `node` is deeper than [`MAX_DEPTH`] in its document.
pub fn check_depth(node: Node<'_, '_>) -> Result<(), ConvertError> {
    match node.ancestors().nth(MAX_DEPTH) {
        Some(_) => Err(too_deep()),
        None => Ok(()),
    }
}

/// Parse an XML part. Too deeply nested XML is refused first: the parser
/// itself recurses per level.
pub fn parse(xml: &str) -> Result<Document<'_>, ConvertError> {
    if xml_depth(xml) > MAX_DEPTH {
        return Err(too_deep());
    }
    Document::parse(xml).map_err(super::damaged_document)
}

/// Deepest element nesting of `xml` (a quick scan: comments, CDATA,
/// processing instructions and quoted attribute values are skipped).
pub fn xml_depth(xml: &str) -> usize {
    let b = xml.as_bytes();
    let find = |from: usize, pat: &[u8]| {
        b.get(from..)
            .and_then(|rest| rest.windows(pat.len()).position(|w| w == pat))
            .map_or(b.len(), |p| from + p + pat.len())
    };
    let (mut i, mut depth, mut max) = (0usize, 0usize, 0usize);
    while i < b.len() {
        if b[i] != b'<' {
            i += 1;
            continue;
        }
        let rest = &b[i..];
        if rest.starts_with(b"<!--") {
            i = find(i + 4, b"-->");
        } else if rest.starts_with(b"<![CDATA[") {
            i = find(i + 9, b"]]>");
        } else if rest.starts_with(b"<?") || rest.starts_with(b"<!") {
            i = find(i + 2, b">");
        } else if rest.starts_with(b"</") {
            depth = depth.saturating_sub(1);
            i = find(i + 2, b">");
        } else {
            // A start tag: find its `>` outside quotes; `/>` closes it.
            let mut j = i + 1;
            let mut quote = None;
            while j < b.len() {
                match (quote, b[j]) {
                    (None, b'"' | b'\'') => quote = Some(b[j]),
                    (Some(q), c) if c == q => quote = None,
                    (None, b'>') => break,
                    _ => {}
                }
                j += 1;
            }
            if b.get(j.wrapping_sub(1)) != Some(&b'/') {
                depth += 1;
                max = max.max(depth);
            }
            i = j + 1;
        }
    }
    max
}

/// Relationships in a `.rels` document for a part in `dir` (`word`), keyed by id.
pub fn rels_of(doc: &Document<'_>, dir: &str) -> HashMap<String, Rel> {
    let mut out = HashMap::new();
    for r in doc.root_element().children().filter(|n| n.is_element()) {
        let (Some(id), Some(target)) = (attr(r, "Id"), attr(r, "Target")) else {
            continue;
        };
        let external = attr(r, "TargetMode").is_some_and(|m| m.eq_ignore_ascii_case("External"));
        let target = if external {
            target.to_owned()
        } else {
            resolve(dir, target)
        };
        let kind = attr(r, "Type").unwrap_or_default().to_owned();
        out.insert(
            id.to_owned(),
            Rel {
                kind,
                target,
                external,
            },
        );
    }
    out
}

/// `dir` + relative `target`, `..` resolved; `/x` is from the package root
/// (`ppt/slides` + `../media/a.png` → `ppt/media/a.png`).
pub fn resolve(dir: &str, target: &str) -> String {
    let mut parts: Vec<&str> = if target.starts_with('/') {
        Vec::new()
    } else {
        dir.split('/').filter(|s| !s.is_empty()).collect()
    };
    for seg in target.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            s => parts.push(s),
        }
    }
    parts.join("/")
}

/// Attribute by local name (`w:val`, `r:id`, …).
pub fn attr<'a>(node: Node<'a, '_>, name: &str) -> Option<&'a str> {
    node.attributes()
        .find(|a| a.name() == name)
        .map(|a| a.value())
}

/// First child element with local name `name`.
pub fn child<'a, 'i>(node: Node<'a, 'i>, name: &str) -> Option<Node<'a, 'i>> {
    node.children()
        .find(|n| n.is_element() && n.tag_name().name() == name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_targets() {
        assert_eq!(resolve("word/", "media/a.png"), "word/media/a.png");
        assert_eq!(resolve("word", "../media/a.png"), "media/a.png");
        assert_eq!(resolve("word", "/word/x.xml"), "word/x.xml");
        assert_eq!(resolve("ppt/slides", "../media/a.png"), "ppt/media/a.png");
        assert_eq!(resolve("ppt", "slides/slide1.xml"), "ppt/slides/slide1.xml");
        assert_eq!(resolve("", "../../x"), "x");
    }

    #[test]
    fn depth_is_checked() {
        // Tasks run on 16 MB stacks (`scheduler`); a 2 MB debug test thread
        // can't parse 256 levels.
        std::thread::Builder::new()
            .stack_size(16 << 20)
            .spawn(depth_checks)
            .unwrap()
            .join()
            .unwrap();
    }

    fn depth_checks() {
        let deep = format!(
            "{}{}",
            "<a>".repeat(MAX_DEPTH + 5),
            "</a>".repeat(MAX_DEPTH + 5)
        );
        let doc = Document::parse(&deep).unwrap();
        let last = doc.descendants().next_back().unwrap();
        assert!(check_depth(doc.root_element()).is_ok());
        let err = check_depth(last).unwrap_err();
        assert!(err.message.contains("nested too deeply"), "{err}");
        // The parser is never reached with very deep XML (it would overflow).
        let bomb = "<a>".repeat(200_000);
        assert!(
            parse(&bomb)
                .unwrap_err()
                .message
                .contains("nested too deeply")
        );
        assert_eq!(
            xml_depth("<?xml v?><!-- <x> --><a k='>'><b/><c x=\"/\"><![CDATA[<d>]]></c></a>"),
            2
        );
    }
}
