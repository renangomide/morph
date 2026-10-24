// SPDX-License-Identifier: GPL-3.0-or-later
//! PDF actions (SPEC §4.2).
//!
//! - [`options`]: option structs for every PDF action (shared).
//! - [`pdfium`]: PDFium access (load once, open with password detection, render,
//!   text, new document, save). Use it instead of calling `Pdfium` directly.
//! - [`pages`]: page-range parsing (`1-3,7,10-`) and chunking.
//! - One file per action: [`compress`] (lopdf), [`to_images`], [`text`],
//!   [`split`], [`merge`] (PDFium; merge bookmarks via lopdf), [`to_word`]
//!   (Word or LibreOffice, see `crate::office`).
//!
//! Output names are in [`crate::naming`]: `name (pages)/name-01.png`,
//! `name (split)/name-p1-3.pdf`, `Merged.pdf`.

pub mod compress;
pub mod merge;
pub mod options;
pub mod pages;
pub mod pdfium;
pub mod split;
pub mod text;
pub mod to_images;
pub mod to_markdown;
pub mod to_word;

use crate::error::ConvertError;

/// A converter got a task for another action (a dispatcher bug).
pub(crate) fn wrong_action(converter: &str) -> ConvertError {
    ConvertError::other(
        "Something went wrong while converting this file.",
        format!("wrong action for {converter}"),
    )
}
