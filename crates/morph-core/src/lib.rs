// SPDX-License-Identifier: GPL-3.0-or-later
//! Morph core: job model, scheduler, converters, output naming. No UI.
//!
//! Flow: build a [`Job`] (one [`Task`] per file, or per group for merge),
//! hand it to [`scheduler::run_job`], read [`Event`]s from the channel.

pub mod action;
pub mod cancel;
pub mod converter;
pub mod error;
pub mod event;
pub mod image;
pub mod job;
pub mod markdown;
pub mod media;
pub mod memory;
pub mod naming;
pub mod office;
pub mod pdf;
pub mod scheduler;
pub mod settings;
pub mod state;
pub mod text;

pub use action::Action;
pub use cancel::CancelToken;
pub use converter::{Converter, TaskCtx, TaskOutput};
pub use error::ConvertError;
pub use event::{Event, Summary};
pub use job::{GroupedFiles, Job, OutputSpec, Task, TaskId, group_files};
pub use morph_formats as formats;
pub use settings::Settings;
