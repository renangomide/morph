// SPDX-License-Identifier: GPL-3.0-or-later
//! Progress events sent from the scheduler to the UI / CLI.

use crate::job::TaskId;
use std::path::PathBuf;
use std::time::Duration;

#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    TaskStarted {
        id: TaskId,
    },
    TaskProgress {
        id: TaskId,
        fraction: f32,
        eta: Option<Duration>,
    },
    /// A short status for a running task, shown instead of the percentage
    /// (e.g. "Checking the graphics card…"). Empty text clears it. English text
    /// from the core, like notes.
    TaskStatus {
        id: TaskId,
        text: String,
    },
    TaskDone {
        id: TaskId,
        outputs: Vec<PathBuf>,
        bytes_in: u64,
        bytes_out: u64,
        note: Option<String>,
    },
    TaskFailed {
        id: TaskId,
        error: String,
        details: String,
    },
    TaskSkipped {
        id: TaskId,
        reason: String,
    },
    JobFinished {
        summary: Summary,
    },
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Summary {
    pub done: usize,
    pub failed: usize,
    pub skipped: usize,
    pub cancelled: usize,
    pub bytes_in: u64,
    pub bytes_out: u64,
}

impl Summary {
    /// Bytes saved (0 if outputs are bigger).
    pub fn saved(&self) -> u64 {
        self.bytes_in.saturating_sub(self.bytes_out)
    }
}
