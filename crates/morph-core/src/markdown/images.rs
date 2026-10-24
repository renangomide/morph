// SPDX-License-Identifier: GPL-3.0-or-later
//! Pictures of a "to Markdown" conversion: saved into a `name (images)` folder
//! next to the `.md` file and linked relatively (SPEC §4.6).
//!
//! - The folder is created only when the first picture is saved, with a free
//!   name (`name (images) (1)` if taken), and deleted again unless
//!   [`ImageFolder::commit`] is called (error / cancel leave nothing behind).
//! - Files are named `image-1.png`, `image-2.jpg`, … in the order saved; the
//!   extension comes from the bytes ([`sniff_extension`]), never from names
//!   inside the document. The same bytes saved twice give the same file.
//! - Links are percent-encoded where Markdown needs it ([`link_encode`]):
//!   `Relatório%20報告%20%28images%29/image-1.png`.
//! - With pictures turned off (`enabled = false`), nothing is written and
//!   [`ImageFolder::image_md`] gives the alt text instead.

use super::writer;
use crate::error::ConvertError;
use crate::naming::{self, OutputDir};
use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

pub struct ImageFolder {
    /// Where the `.md` file goes.
    folder: PathBuf,
    /// Wanted folder name (`stem (images)`).
    name: String,
    enabled: bool,
    dir: Option<OutputDir>,
    /// Encoded name of the folder actually created.
    dir_link: String,
    /// (length, hash) of saved bytes → link.
    saved: HashMap<(usize, u64), String>,
    count: usize,
}

impl ImageFolder {
    /// Pictures for the Markdown made from `input`, written to `output_folder`.
    pub fn new(output_folder: &Path, input: &Path, enabled: bool) -> ImageFolder {
        let stem = input
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Document".to_owned());
        ImageFolder {
            folder: output_folder.to_path_buf(),
            name: format!("{stem} (images)"),
            enabled,
            dir: None,
            dir_link: String::new(),
            saved: HashMap::new(),
            count: 0,
        }
    }

    /// Pictures are saved (the user didn't turn them off).
    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// Distinct pictures saved so far.
    pub fn count(&self) -> usize {
        self.count
    }

    /// Save one picture; returns its relative link, or `None` when pictures are
    /// off or `bytes` is empty. `ext_hint` (e.g. from a part name) is used only
    /// when the bytes aren't a known picture format.
    pub fn save(
        &mut self,
        bytes: &[u8],
        ext_hint: Option<&str>,
    ) -> Result<Option<String>, ConvertError> {
        if !self.enabled || bytes.is_empty() {
            return Ok(None);
        }
        let key = (bytes.len(), hash(bytes));
        if let Some(link) = self.saved.get(&key) {
            return Ok(Some(link.clone()));
        }
        if self.dir.is_none() {
            let dir = OutputDir::create(&self.folder, &self.name)?;
            self.dir_link = dir
                .path()
                .file_name()
                .map(|n| link_encode(&n.to_string_lossy()))
                .unwrap_or_default();
            self.dir = Some(dir);
        }
        let Some(dir) = self.dir.as_ref() else {
            return Ok(None);
        };
        let ext = sniff_extension(bytes)
            .map(str::to_owned)
            .or_else(|| ext_hint.and_then(safe_extension))
            .unwrap_or_else(|| "bin".to_owned());
        self.count += 1;
        let file_name = format!("image-{}.{ext}", self.count);
        let written = naming::write_output(dir.path(), &file_name, bytes)?;
        let written_name = written
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or(file_name);
        let link = format!("{}/{}", self.dir_link, link_encode(&written_name));
        self.saved.insert(key, link.clone());
        Ok(Some(link))
    }

    /// Markdown for one picture: `![alt](link)` when saved; with pictures off,
    /// the escaped alt text (empty when there is none).
    pub fn image_md(
        &mut self,
        bytes: &[u8],
        ext_hint: Option<&str>,
        alt: &str,
    ) -> Result<String, ConvertError> {
        Ok(match self.save(bytes, ext_hint)? {
            Some(link) => writer::image(alt, &link),
            None => writer::escape_text(&writer::one_line(alt)),
        })
    }

    /// Keep the folder (call after the `.md` file is committed). Returns its
    /// path, or `None` when no picture was saved.
    pub fn commit(self) -> Option<PathBuf> {
        self.dir.map(OutputDir::commit)
    }
}

fn hash(bytes: &[u8]) -> u64 {
    let mut h = DefaultHasher::new();
    bytes.hash(&mut h);
    h.finish()
}

/// 1–5 ASCII letters/digits, lower-cased (`.PNG` → `png`); else `None`.
fn safe_extension(hint: &str) -> Option<String> {
    let ext = hint.rsplit('.').next().unwrap_or(hint).trim();
    (!ext.is_empty() && ext.len() <= 5 && ext.chars().all(|c| c.is_ascii_alphanumeric()))
        .then(|| ext.to_ascii_lowercase())
}

/// The picture format from its first bytes: png, jpg, gif, bmp, webp, tiff,
/// ico, svg, emf, wmf. (EMF/WMF don't show in most Markdown viewers; readers
/// may convert or skip them.)
pub fn sniff_extension(bytes: &[u8]) -> Option<&'static str> {
    let starts = |sig: &[u8]| bytes.starts_with(sig);
    if starts(b"\x89PNG\r\n\x1a\n") {
        Some("png")
    } else if starts(b"\xFF\xD8\xFF") {
        Some("jpg")
    } else if starts(b"GIF87a") || starts(b"GIF89a") {
        Some("gif")
    } else if starts(b"BM") && bytes.len() > 14 {
        Some("bmp")
    } else if starts(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
        Some("webp")
    } else if starts(b"II*\0") || starts(b"MM\0*") {
        Some("tiff")
    } else if starts(b"\0\0\x01\0") {
        Some("ico")
    } else if starts(b"\xD7\xCD\xC6\x9A") || starts(b"\x01\0\x09\0") {
        Some("wmf")
    } else if starts(b"\x01\0\0\0") && bytes.get(40..44) == Some(b" EMF") {
        Some("emf")
    } else {
        let head = String::from_utf8_lossy(&bytes[..bytes.len().min(512)]).to_ascii_lowercase();
        let head = head.trim_start_matches('\u{feff}').trim_start();
        (head.starts_with("<svg") || (head.starts_with("<?xml") && head.contains("<svg")))
            .then_some("svg")
    }
}

/// Percent-encode the ASCII characters that would break a Markdown link
/// (spaces, parentheses, `%`, …); letters with accents and CJK stay readable.
pub fn link_encode(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for c in name.chars() {
        match c {
            'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '_' | '.' | '~' => out.push(c),
            c if !c.is_ascii() && !c.is_whitespace() && !c.is_control() => out.push(c),
            c => {
                let mut buf = [0u8; 4];
                for b in c.encode_utf8(&mut buf).bytes() {
                    out.push_str(&format!("%{b:02X}"));
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const PNG: &[u8] = b"\x89PNG\r\n\x1a\n0000";
    const JPG: &[u8] = b"\xFF\xD8\xFF\xE0jpeg";

    #[test]
    fn links_are_encoded() {
        assert_eq!(
            link_encode("Relatório 報告 (images)"),
            "Relatório%20報告%20%28images%29"
        );
        assert_eq!(link_encode("a_b-1.png"), "a_b-1.png");
    }

    #[test]
    fn sniffs_formats() {
        assert_eq!(sniff_extension(PNG), Some("png"));
        assert_eq!(sniff_extension(JPG), Some("jpg"));
        assert_eq!(sniff_extension(b"GIF89a.."), Some("gif"));
        assert_eq!(sniff_extension(b"RIFF\0\0\0\0WEBPVP8 "), Some("webp"));
        assert_eq!(
            sniff_extension(b"\xEF\xBB\xBF<?xml version='1.0'?><svg/>"),
            Some("svg")
        );
        assert_eq!(sniff_extension(b"  <svg xmlns='x'/>"), Some("svg"));
        assert_eq!(sniff_extension(b"hello"), None);
        assert_eq!(safe_extension("media/image1.JPEG").as_deref(), Some("jpeg"));
        assert_eq!(safe_extension("a.verylong"), None);
        assert_eq!(safe_extension("image1.PNG").as_deref(), Some("png"));
        assert_eq!(safe_extension("x.p/g"), None);
    }

    #[test]
    fn saves_once_and_commits() {
        let root = tempfile::tempdir().unwrap();
        let input = root.path().join("Relatório 報告.docx");
        let mut f = ImageFolder::new(root.path(), &input, true);
        let a = f.save(PNG, None).unwrap().unwrap();
        let b = f.save(JPG, Some("x.jpeg")).unwrap().unwrap();
        let again = f.save(PNG, None).unwrap().unwrap();
        assert_eq!(a, "Relatório%20報告%20%28images%29/image-1.png");
        assert_eq!(b, "Relatório%20報告%20%28images%29/image-2.jpg");
        assert_eq!(again, a);
        assert_eq!(f.count(), 2);
        assert_eq!(f.save(b"", None).unwrap(), None);
        let unknown = f.image_md(b"????", Some("pic.EMZ"), "A chart").unwrap();
        assert_eq!(
            unknown,
            "![A chart](Relatório%20報告%20%28images%29/image-3.emz)"
        );
        let dir = f.commit().unwrap();
        assert!(dir.ends_with("Relatório 報告 (images)"));
        assert_eq!(std::fs::read(dir.join("image-1.png")).unwrap(), PNG);
        assert!(dir.join("image-3.emz").is_file());
    }

    #[test]
    fn taken_folder_name_gets_a_number() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("a (images)")).unwrap();
        let mut f = ImageFolder::new(root.path(), Path::new("a.pptx"), true);
        let link = f.save(PNG, None).unwrap().unwrap();
        assert_eq!(link, "a%20%28images%29%20%281%29/image-1.png");
        assert!(f.commit().unwrap().ends_with("a (images) (1)"));
    }

    #[test]
    fn uncommitted_folder_is_removed_and_disabled_writes_nothing() {
        let root = tempfile::tempdir().unwrap();
        {
            let mut f = ImageFolder::new(root.path(), Path::new("a.docx"), true);
            f.save(PNG, None).unwrap();
            assert!(root.path().join("a (images)").is_dir());
        }
        assert!(!root.path().join("a (images)").exists());
        let mut off = ImageFolder::new(root.path(), Path::new("a.docx"), false);
        assert!(!off.enabled());
        assert_eq!(
            off.image_md(PNG, None, "Logo [v2]").unwrap(),
            "Logo \\[v2\\]"
        );
        assert_eq!(off.image_md(PNG, None, "").unwrap(), "");
        assert_eq!(off.commit(), None);
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
    }
}
