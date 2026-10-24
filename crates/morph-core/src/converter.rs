// SPDX-License-Identifier: GPL-3.0-or-later
//! The converter interface every action implements.

use crate::cancel::CancelToken;
use crate::error::ConvertError;
use crate::job::Task;
use crate::office::EnginePreference;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// What a finished task produced.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TaskOutput {
    pub outputs: Vec<PathBuf>,
    /// Shown in the results row, e.g. "Animated GIF: only the first frame was converted."
    pub note: Option<String>,
}

impl TaskOutput {
    pub fn single(path: PathBuf) -> Self {
        TaskOutput {
            outputs: vec![path],
            note: None,
        }
    }

    pub fn with_note(mut self, note: impl Into<String>) -> Self {
        self.note = Some(note.into());
        self
    }
}

/// Per-task context handed to a converter.
pub struct TaskCtx<'a> {
    pub cancel: &'a CancelToken,
    progress: &'a (dyn Fn(f32, Option<Duration>) + Sync),
    temp_dir: &'a Path,
    office_engine: EnginePreference,
    status: &'a (dyn Fn(&str) + Sync),
}

impl<'a> TaskCtx<'a> {
    pub fn new(
        cancel: &'a CancelToken,
        progress: &'a (dyn Fn(f32, Option<Duration>) + Sync),
        temp_dir: &'a Path,
    ) -> Self {
        TaskCtx {
            cancel,
            progress,
            temp_dir,
            office_engine: EnginePreference::Auto,
            status: &|_| {},
        }
    }

    /// Send status texts ([`TaskCtx::status`]) here (the scheduler does).
    pub fn with_status(mut self, status: &'a (dyn Fn(&str) + Sync)) -> Self {
        self.status = status;
        self
    }

    /// Show a short status on this task's row instead of the percentage, e.g.
    /// "Checking the graphics card…" (English, like notes). `""` clears it.
    pub fn status(&self, text: &str) {
        (self.status)(text);
    }

    /// Use this Office engine preference (Settings → Office engine).
    pub fn with_office_engine(mut self, preference: EnginePreference) -> Self {
        self.office_engine = preference;
        self
    }

    /// Settings → Office engine for this task (default Auto). Resolve the engine
    /// with `crate::office::engine_for(family, ctx)`.
    pub fn office_engine(&self) -> EnginePreference {
        self.office_engine
    }

    /// Report progress for this task, 0.0–1.0.
    pub fn progress(&self, fraction: f32, eta: Option<Duration>) {
        (self.progress)(fraction.clamp(0.0, 1.0), eta);
    }

    /// Private scratch folder for this task, created on first use and deleted
    /// after the task. Never put outputs here; use [`crate::naming::OutputFile`].
    pub fn temp_dir(&self) -> Result<&Path, ConvertError> {
        std::fs::create_dir_all(self.temp_dir)
            .map_err(|e| ConvertError::other("Couldn't create a temporary folder.", e))?;
        Ok(self.temp_dir)
    }

    /// Shortcut for `self.cancel.check()`.
    pub fn check_cancel(&self) -> Result<(), ConvertError> {
        self.cancel.check()
    }
}

pub trait Converter: Send + Sync {
    /// Run one task. Must never modify the inputs; write outputs through
    /// [`crate::naming::OutputFile`] so partial files are cleaned up.
    fn run(&self, task: &Task, ctx: &TaskCtx<'_>) -> Result<TaskOutput, ConvertError>;
}
