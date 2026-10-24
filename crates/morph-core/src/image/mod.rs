// SPDX-License-Identifier: GPL-3.0-or-later
//! Image actions (SPEC §4.1).
//!
//! - [`io`]: decode any input format / encode any output format, metadata helpers.
//! - [`convert`], [`compress`], [`resize`], [`clean`]: the four actions, built on `io`.
//! - [`best`]: the perceptual search behind Compress "Best (automatic)".

pub mod best;
pub mod clean;
pub(crate) mod common;
pub mod compress;
pub mod convert;
pub mod io;
pub mod options;
pub mod resize;
pub mod to_pdf;
