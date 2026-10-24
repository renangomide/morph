// SPDX-License-Identifier: GPL-3.0-or-later
//! Text-like inputs: Markdown, plain text, HTML (SPEC §4.5, approved 2026-10-19).
//!
//! Reached through `office.convert` ([`crate::office::Plan::Text`]). This file
//! (foundation) only routes by input format × target format; each conversion is
//! in its own file:
//!
//! | From → to | File | How |
//! |---|---|---|
//! | Markdown → HTML / TXT | [`markdown`] | `pulldown-cmark` (CommonMark + tables) |
//! | TXT → HTML / Markdown | [`plain`] | escaped, paragraphs kept |
//! | HTML → Markdown / TXT | [`html`] | `htmd` |
//! | any → DOCX / PDF / ODT | [`via_engine`] | native → HTML in the temp folder, then the Office engine |
//!
//! Local images referenced by relative paths are kept; remote images are never fetched.

pub mod html;
pub mod html_clean;
pub mod markdown;
pub mod plain;
pub mod via_engine;

use crate::converter::{TaskCtx, TaskOutput};
use crate::error::ConvertError;
use crate::job::Task;
use crate::office::{DocFormat, Family, OfficeConvertOpts, engine_for};
use std::path::Path;

/// Convert one text-like file to `opts.format`.
pub fn convert(
    task: &Task,
    input: &Path,
    opts: &OfficeConvertOpts,
    ctx: &TaskCtx<'_>,
) -> Result<TaskOutput, ConvertError> {
    use DocFormat as F;
    match (DocFormat::of_path(input), opts.format) {
        (Some(F::Md), F::Html) => markdown::to_html(task, input, opts, ctx),
        (Some(F::Md), F::Txt) => markdown::to_text(task, input, opts, ctx),
        (Some(F::Txt), F::Html) => plain::to_html(task, input, opts, ctx),
        (Some(F::Txt), F::Md) => plain::to_markdown(task, input, opts, ctx),
        (Some(F::Html), F::Md) => html::to_markdown(task, input, opts, ctx),
        (Some(F::Html), F::Txt) => html::to_text(task, input, opts, ctx),
        (Some(F::Md | F::Txt | F::Html), F::Docx | F::Pdf | F::Odt) => {
            let engine = engine_for(Family::Text, ctx)?;
            via_engine::convert(task, input, opts, &engine, ctx)
        }
        (from, to) => Err(ConvertError::unsupported(
            format!("A text file can't be converted to {}.", to.label()),
            format!("{from:?} → {to:?}"),
        )),
    }
}
