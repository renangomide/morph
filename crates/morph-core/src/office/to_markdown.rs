// SPDX-License-Identifier: GPL-3.0-or-later
//! Word-like → Markdown (SPEC §4.5): the engine converts to HTML in the
//! temp folder ([`super::engine_convert::export`] with `DocFormat::Html`), then
//! HTML → Markdown natively (`htmd`, shared with `crate::text`), after the same
//! pre-clean as HTML input ([`crate::text::html_clean::preclean`]).
//!
//! Pictures: the engine writes them as files next to its HTML. They are saved
//! through the shared [`ImageFolder`] (`name (images)/image-1.png`, created only
//! when the document has pictures) and linked relatively, so the Markdown shows
//! them in any viewer. `data:` pictures are saved by the pre-clean; remote
//! pictures are never fetched: their description is kept as text.

use super::detect::EngineChoice;
use super::options::{DocFormat, Family, OfficeConvertOpts};
use crate::converter::{TaskCtx, TaskOutput};
use crate::error::ConvertError;
use crate::job::Task;
use crate::markdown::{ImageFolder, write_markdown};
use crate::text::html::{
    Attr, TagAction, attr_text, build_tag, html_to_markdown, local_file_in, rewrite_tags,
};
use crate::text::html_clean::preclean;
use crate::text::plain::{escape_html, read_text};
use std::path::Path;

pub fn convert(
    task: &Task,
    input: &Path,
    opts: &OfficeConvertOpts,
    engine: &EngineChoice,
    ctx: &TaskCtx<'_>,
) -> Result<TaskOutput, ConvertError> {
    let html_path =
        super::engine_convert::export(input, Family::Word, DocFormat::Html, engine, ctx)?;
    ctx.check_cancel()?;
    ctx.progress(0.8, None);
    let html = read_text(&html_path)?;
    let html_dir = html_path.parent().unwrap_or(Path::new("."));
    let folder = task.output.folder_for(input);
    let mut images = ImageFolder::new(&folder, input, opts.images);

    // Pictures the engine wrote next to its HTML → saved and relinked; other
    // non-`data:` pictures → their description (pictures off: always).
    let mut error = None;
    let html = rewrite_tags(&html, |name, attrs| {
        if name != "img" {
            return TagAction::Keep;
        }
        let src = get(attrs, "src").unwrap_or_default();
        if src.trim_start().to_ascii_lowercase().starts_with("data:") {
            return TagAction::Keep;
        }
        let bytes = match local_file_in(&src, html_dir) {
            Some(file) if images.enabled() => std::fs::read(&file).unwrap_or_default(),
            _ => Vec::new(),
        };
        let alt = get(attrs, "alt").unwrap_or_default();
        match images.save(&bytes, Some(&src)) {
            Ok(Some(link)) => TagAction::Replace(build_tag(
                "img",
                &[("src".into(), Some(link)), ("alt".into(), Some(alt))],
            )),
            Ok(None) => TagAction::Replace(escape_html(&alt)),
            Err(e) => {
                error.get_or_insert(e);
                TagAction::Keep
            }
        }
    });
    if let Some(e) = error {
        return Err(e);
    }
    ctx.check_cancel()?;
    let html = preclean(&html, &mut images)?;
    let md = html_to_markdown(&html)?;
    ctx.check_cancel()?;
    let out = write_markdown(&folder, input, &md, images)?;
    ctx.progress(1.0, None);
    Ok(out)
}

fn get(attrs: &[Attr<'_>], name: &str) -> Option<String> {
    attrs
        .iter()
        .find(|a| a.name.eq_ignore_ascii_case(name))
        .and_then(|a| a.value)
        .map(attr_text)
}
