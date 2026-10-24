// SPDX-License-Identifier: GPL-3.0-or-later
//! Shared Markdown output for every "to Markdown" reader (SPEC §4.6): PDF
//! (`crate::pdf::to_markdown`), DOCX (`crate::office::docx_markdown`), PPTX
//! (`crate::office::pptx_markdown`), spreadsheets (`crate::office::sheet_markdown`)
//! and HTML (`crate::text::html`).
//!
//! - [`writer`]: escaping, inline emitters, the [`Inline`] run builder and the
//!   [`MarkdownWriter`] block builder (headings, nested lists, pipe tables, …).
//! - [`images`]: the `name (images)/` folder ([`ImageFolder`]).
//! - [`write_markdown`]: write the `.md` file safely, then keep the pictures folder.

pub mod images;
pub mod writer;

pub use images::{ImageFolder, link_encode, sniff_extension};
pub use writer::{
    Align, HARD_BREAK, Inline, ListKind, MarkdownWriter, Style, code_span, emphasis, escape_text,
    footnote_ref, image, is_safe_link, link, one_line, strikethrough, strong, table_cell,
};

use crate::converter::TaskOutput;
use crate::error::ConvertError;
use crate::naming::{self, OutputFile, Suffix};
use std::path::Path;

/// Write `markdown` as `name.md` in `folder` (temp file + rename, never
/// replacing a file), then commit the pictures folder. The task output is the
/// `.md` file. Use the same `folder` the [`ImageFolder`] was made with.
pub fn write_markdown(
    folder: &Path,
    input: &Path,
    markdown: &str,
    images: ImageFolder,
) -> Result<TaskOutput, ConvertError> {
    let mut out = OutputFile::create(folder, &naming::output_name(input, "md", Suffix::Converted))?;
    out.write_all(markdown.as_bytes())?;
    let path = out.commit()?;
    images.commit();
    Ok(TaskOutput::single(path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_md_and_keeps_pictures() {
        let root = tempfile::tempdir().unwrap();
        let input = root.path().join("deck.pptx");
        let mut images = ImageFolder::new(root.path(), &input, true);
        let pic = images
            .image_md(b"\x89PNG\r\n\x1a\nxx", None, "Logo")
            .unwrap();
        let mut w = MarkdownWriter::new();
        w.heading(2, "Slide 1: Hello");
        w.paragraph(&pic);
        let out = write_markdown(root.path(), &input, &w.finish(), images).unwrap();
        assert_eq!(out.outputs, vec![root.path().join("deck.md")]);
        let md = std::fs::read_to_string(&out.outputs[0]).unwrap();
        assert_eq!(
            md,
            "## Slide 1: Hello\n\n![Logo](deck%20%28images%29/image-1.png)\n"
        );
        assert!(
            root.path()
                .join("deck (images)")
                .join("image-1.png")
                .is_file()
        );
    }
}
