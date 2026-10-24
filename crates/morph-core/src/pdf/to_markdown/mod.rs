// SPDX-License-Identifier: GPL-3.0-or-later
//! PDF → Markdown / HTML (`pdf.to_markdown`, SPEC §4.6): native layout reader on
//! PDFium's characters.
//!
//! 1. [`extract`]: characters → words → rows → segments (wide gaps), per page.
//! 2. [`layout`]: running headers/footers and page numbers dropped; two-column
//!    runs read left side first.
//! 3. [`blocks`]: tables, headings, lists, paragraphs (de-hyphenated) → Markdown
//!    through [`crate::markdown::MarkdownWriter`].
//!
//! Output: `name.md` (or `name.html`: the same Markdown →
//! `crate::text::markdown::markdown_to_html_page`) next to the input; pictures
//! in `name (images)/` with [`crate::markdown::ImageFolder`] when `opts.images`.
//! No text on any page: "No text found. This PDF may be scanned pages (pictures, not text)." and no output.

mod blocks;
mod extract;
mod layout;

use crate::action::Action;
use crate::converter::{Converter, TaskCtx, TaskOutput};
use crate::error::{ConvertError, ErrorKind};
use crate::job::Task;
use crate::markdown::{ImageFolder, write_markdown};
use crate::naming::{self, OutputFile, Suffix};
use crate::pdf::options::PdfMarkupFormat;
use crate::pdf::pdfium;
use blocks::Item;
use pdfium_render::prelude::*;
use std::collections::HashMap;
use std::path::Path;

pub struct PdfToMarkdown;

impl Converter for PdfToMarkdown {
    fn run(&self, task: &Task, ctx: &TaskCtx<'_>) -> Result<TaskOutput, ConvertError> {
        let Action::PdfToMarkdown(opts) = &task.action else {
            return Err(super::wrong_action("PdfToMarkdown"));
        };
        let input = task
            .input()
            .ok_or_else(|| super::wrong_action("PdfToMarkdown"))?;
        let folder = task.output.folder_for(input);
        let mut images = ImageFolder::new(&folder, input, opts.images);
        let md = convert(input, &mut images, ctx)?;
        ctx.check_cancel()?;
        match opts.format {
            PdfMarkupFormat::Md => write_markdown(&folder, input, &md, images),
            PdfMarkupFormat::Html => {
                let stem = input
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let page = crate::text::markdown::markdown_to_html_page(&md, &stem);
                let name = naming::output_name(input, "html", Suffix::Converted);
                let mut out = OutputFile::create(&folder, &name)?;
                out.write_all(page.as_bytes())?;
                let path = out.commit()?;
                images.commit();
                Ok(TaskOutput::single(path))
            }
        }
    }
}

/// The whole PDF as Markdown (pictures saved into `images`).
fn convert(
    input: &Path,
    images: &mut ImageFolder,
    ctx: &TaskCtx<'_>,
) -> Result<String, ConvertError> {
    let doc = pdfium::open(input)?;
    let count = pdfium::page_count(&doc);
    let total = count.max(1) as f32;
    let mut pages = Vec::with_capacity(count as usize);
    for i in 0..count {
        ctx.check_cancel()?;
        let page = pdfium::page(&doc, i)?;
        pages.push(extract::read_page(&page)?);
        ctx.progress(0.8 * (i + 1) as f32 / total, None);
    }
    if pages.iter().all(|p| p.rows.is_empty()) {
        return Err(no_text());
    }
    layout::remove_furniture(&mut pages);
    let outline = outline(&doc);
    let mut items = Vec::new();
    for (i, text) in pages.iter().enumerate() {
        ctx.check_cancel()?;
        let lines = layout::page_lines(i, text);
        let mut pics = Vec::new();
        if images.enabled() {
            let page = pdfium::page(&doc, i as u32)?;
            for pic in extract::read_images(&page) {
                if let Some(link) = images.save(&pic.bytes, None)? {
                    pics.push((pic.top, crate::markdown::image("", &link)));
                }
            }
        }
        // Each picture goes before the first line below its top edge.
        let mut pics = pics.into_iter().peekable();
        for line in lines {
            while let Some((_, md)) = pics.next_if(|(top, _)| line.top() < *top) {
                items.push(Item::Image(md));
            }
            items.push(Item::Text(line));
        }
        items.extend(pics.map(|(_, md)| Item::Image(md)));
        ctx.progress(0.8 + 0.2 * (i + 1) as f32 / total, None);
    }
    Ok(blocks::to_markdown(items, &outline))
}

/// Outline (bookmark) titles → heading level (depth + 1, at most 6).
fn outline(doc: &PdfDocument<'_>) -> HashMap<String, u8> {
    let mut out = HashMap::new();
    for b in doc.bookmarks().iter().take(2000) {
        let Some(title) = b.title() else {
            continue;
        };
        let mut depth = 1u8;
        let mut parent = b.parent();
        while let Some(p) = parent {
            depth = depth.saturating_add(1);
            if depth >= 6 {
                break;
            }
            parent = p.parent();
        }
        let key = blocks::outline_key(&title);
        if !key.is_empty() {
            out.entry(key).or_insert(depth.min(6));
        }
    }
    out
}

/// "No text found. This PDF may be scanned pages (pictures, not text)."
fn no_text() -> ConvertError {
    ConvertError::new(
        ErrorKind::Unsupported,
        "No text found. This PDF may be scanned pages (pictures, not text).",
        "no page has a text layer",
    )
}
