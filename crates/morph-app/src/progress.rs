// SPDX-License-Identifier: GPL-3.0-or-later
//! Progress view model (SPEC §3.3) as plain Rust: scheduler events → row states,
//! row text, the summary line, the toast title and the saved results.
//!
//! Rows are in task-ID order: row `i` is task `i`; inputs the action can't take
//! (`Job::skipped`) follow, as the scheduler numbers them.

use crate::results::{RowStatus, SavedResults, SavedRow};
use crate::strings;
use crate::view::format_size;
use morph_core::action::Queue;
use morph_core::{Event, Job, Summary};
use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Debug, Clone, PartialEq)]
pub enum RowState {
    Waiting,
    Running {
        fraction: f32,
        /// Time left, when the converter can tell (long videos).
        eta: Option<Duration>,
    },
    Done {
        outputs: Vec<PathBuf>,
        bytes_in: u64,
        bytes_out: u64,
        note: Option<String>,
    },
    Failed {
        error: String,
        details: String,
    },
    /// The action can't take this file.
    Skipped {
        reason: String,
    },
    /// Not run because of Cancel (or stopped by it).
    Cancelled,
}

impl RowState {
    fn is_final(&self) -> bool {
        !matches!(self, RowState::Waiting | RowState::Running { .. })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    /// The (first) input.
    pub input: PathBuf,
    /// Number of inputs: several for Merge (one row for all files).
    pub inputs: usize,
    /// A slow engine conversion (Office queue): its progress is shown as
    /// "Working…" with a moving bar, not a percentage that stands still.
    pub slow: bool,
    pub state: RowState,
    /// Status text from the core while running (`Event::TaskStatus`), shown
    /// instead of the percentage; empty = none.
    pub status: String,
    /// What the action makes, until the real output is known: "→ WebP",
    /// "Compress" (empty for skipped inputs).
    pub target: String,
}

/// What one row of the list shows. Mirrors the Slint `ProgressRow` struct.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RowView {
    pub status: Status,
    /// The full input name ("IMG_0012.jpg"), or "3 files" for a merge.
    pub name: String,
    /// Target pill: "→ WebP" (same name, new format), "→ Merged.pdf" (the
    /// name changed), "→ a (pages) (12 files)", or the action ("Compress")
    /// before there is an output.
    pub target: String,
    /// "4.1 MB → 0.9 MB".
    pub sizes: String,
    /// Size change pill: "−78%", "+12%", or empty (same size, no output).
    pub change: String,
    /// The output is smaller than the input (green pill).
    pub smaller: bool,
    /// Waiting… / 41% / note / failure message / skip reason.
    pub message: String,
    pub details: String,
    /// 0–1, for the running bar.
    pub fraction: f32,
    /// Has output files (Open / Show in folder).
    pub has_outputs: bool,
    /// Running without a meaningful percentage (moving bar).
    pub indeterminate: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Status {
    #[default]
    Waiting,
    Running,
    Done,
    Failed,
    Skipped,
    Cancelled,
}

impl Status {
    /// Index used by the Slint view.
    pub fn index(self) -> i32 {
        match self {
            Status::Waiting => 0,
            Status::Running => 1,
            Status::Done => 2,
            Status::Failed => 3,
            Status::Skipped => 4,
            Status::Cancelled => 5,
        }
    }
}

/// Live state of a running job.
#[derive(Debug, Clone)]
pub struct Progress {
    rows: Vec<Row>,
    /// Rows `0..tasks` are tasks; the rest are skipped inputs.
    tasks: usize,
    finished_tasks: usize,
    summary: Option<Summary>,
    dirty: Vec<usize>,
    is_dirty: Vec<bool>,
}

impl Progress {
    pub fn new(job: &Job) -> Progress {
        let mut rows: Vec<Row> = job
            .tasks
            .iter()
            .map(|t| Row {
                input: t.inputs.first().cloned().unwrap_or_default(),
                inputs: t.inputs.len(),
                slow: t
                    .inputs
                    .first()
                    .is_some_and(|i| t.action.queue(i) == Queue::Office),
                state: RowState::Waiting,
                status: String::new(),
                target: crate::view::action_summary(&t.action),
            })
            .collect();
        rows.extend(job.skipped.iter().map(|(input, _)| Row {
            input: input.clone(),
            inputs: 1,
            slow: false,
            state: RowState::Waiting,
            status: String::new(),
            target: String::new(),
        }));
        let n = rows.len();
        Progress {
            rows,
            tasks: job.tasks.len(),
            finished_tasks: 0,
            summary: None,
            dirty: Vec::new(),
            is_dirty: vec![false; n],
        }
    }

    pub fn rows(&self) -> &[Row] {
        &self.rows
    }

    /// Number of files that run (skipped inputs excluded).
    pub fn total(&self) -> usize {
        self.tasks
    }

    /// Tasks that are done, failed or cancelled.
    pub fn finished(&self) -> usize {
        self.finished_tasks
    }

    pub fn summary(&self) -> Option<&Summary> {
        self.summary.as_ref()
    }

    pub fn is_finished(&self) -> bool {
        self.summary.is_some()
    }

    /// Overall progress 0–1, counting the running tasks' own progress.
    pub fn fraction(&self) -> f32 {
        if self.is_finished() {
            return 1.0;
        }
        if self.tasks == 0 {
            return 0.0;
        }
        let running: f32 = self.rows[..self.tasks]
            .iter()
            .map(|r| match r.state {
                RowState::Running { fraction, .. } => fraction.clamp(0.0, 1.0),
                _ => 0.0,
            })
            .sum();
        ((self.finished_tasks as f32 + running) / self.tasks as f32).clamp(0.0, 1.0)
    }

    /// The overall bar has nothing meaningful to show yet: no task has
    /// finished and only slow engine conversions are running (one Office file
    /// would otherwise sit at 0% for its whole conversion).
    pub fn indeterminate(&self) -> bool {
        let mut running = self.rows[..self.tasks]
            .iter()
            .filter(|r| matches!(r.state, RowState::Running { .. }))
            .peekable();
        !self.is_finished()
            && self.finished_tasks == 0
            && running.peek().is_some()
            && running.all(|r| r.slow)
    }

    /// The core's status text ("Checking the graphics card…") when exactly
    /// one task is running and it has one; for the tray tooltip.
    pub fn lone_status(&self) -> Option<&str> {
        let mut running = self.rows[..self.tasks]
            .iter()
            .filter(|r| matches!(r.state, RowState::Running { .. }));
        match (running.next(), running.next()) {
            (Some(row), None) if !row.status.is_empty() => Some(&row.status),
            _ => None,
        }
    }

    /// Apply one scheduler event.
    pub fn apply(&mut self, event: Event) {
        let (id, state) = match event {
            Event::TaskStatus { id, text } => {
                if let Some(i) = usize::try_from(id.0).ok().filter(|i| *i < self.rows.len())
                    && !self.rows[i].state.is_final()
                {
                    self.rows[i].status = text;
                    self.mark(i);
                }
                return;
            }
            Event::TaskStarted { id } => (
                id,
                RowState::Running {
                    fraction: 0.0,
                    eta: None,
                },
            ),
            Event::TaskProgress { id, fraction, eta } => (id, RowState::Running { fraction, eta }),
            Event::TaskDone {
                id,
                outputs,
                bytes_in,
                bytes_out,
                note,
            } => (
                id,
                RowState::Done {
                    outputs,
                    bytes_in,
                    bytes_out,
                    note,
                },
            ),
            Event::TaskFailed { id, error, details } => (id, RowState::Failed { error, details }),
            Event::TaskSkipped { id, reason } => {
                let is_task = usize::try_from(id.0).is_ok_and(|i| i < self.tasks);
                (
                    id,
                    if is_task {
                        RowState::Cancelled
                    } else {
                        RowState::Skipped { reason }
                    },
                )
            }
            Event::JobFinished { summary } => {
                self.summary = Some(summary);
                // Anything still open (shouldn't happen) counts as cancelled.
                for i in 0..self.rows.len() {
                    if !self.rows[i].state.is_final() {
                        self.rows[i].state = RowState::Cancelled;
                        if i < self.tasks {
                            self.finished_tasks += 1;
                        }
                        self.mark(i);
                    }
                }
                return;
            }
        };
        let Some(i) = usize::try_from(id.0).ok().filter(|i| *i < self.rows.len()) else {
            return;
        };
        let row = &mut self.rows[i];
        if row.state.is_final() {
            return; // late progress after the end
        }
        if state.is_final() && i < self.tasks {
            self.finished_tasks += 1;
        }
        row.state = state;
        self.mark(i);
    }

    fn mark(&mut self, i: usize) {
        if let Some(flag) = self.is_dirty.get_mut(i)
            && !*flag
        {
            *flag = true;
            self.dirty.push(i);
        }
    }

    /// Rows changed since the last call.
    pub fn take_dirty(&mut self) -> Vec<usize> {
        for &i in &self.dirty {
            if let Some(flag) = self.is_dirty.get_mut(i) {
                *flag = false;
            }
        }
        std::mem::take(&mut self.dirty)
    }

    pub fn row_view(&self, i: usize) -> RowView {
        self.rows.get(i).map(row_view).unwrap_or_default()
    }

    /// Every row's view, in order. Used to rebuild the whole list once the job
    /// ends, so each row shows its final state whatever happened to the
    /// per-row updates before.
    pub fn row_views(&self) -> Vec<RowView> {
        self.rows.iter().map(row_view).collect()
    }

    /// A summary counted from the rows, for when the job thread is gone
    /// without sending `JobFinished`.
    pub fn summary_from_rows(&self) -> Summary {
        let mut s = Summary::default();
        for row in &self.rows {
            match &row.state {
                RowState::Done {
                    bytes_in,
                    bytes_out,
                    ..
                } => {
                    s.done += 1;
                    s.bytes_in += bytes_in;
                    s.bytes_out += bytes_out;
                }
                RowState::Failed { .. } => s.failed += 1,
                RowState::Skipped { .. } => s.skipped += 1,
                RowState::Waiting | RowState::Running { .. } | RowState::Cancelled => {
                    s.cancelled += 1
                }
            }
        }
        s
    }

    /// Where "Open folder" goes: the chosen folder, else the first output's folder.
    pub fn open_folder(&self, chosen: Option<&Path>) -> Option<PathBuf> {
        if let Some(folder) = chosen {
            return Some(folder.to_path_buf());
        }
        self.rows.iter().find_map(|r| match &r.state {
            // Pages / split parts: the folder that holds their new folder.
            RowState::Done { outputs, .. } => shared_folder(outputs)
                .or_else(|| outputs.first().map(PathBuf::as_path))
                .and_then(Path::parent)
                .map(Path::to_path_buf),
            _ => None,
        })
    }

    /// Everything the results view needs after the process is gone.
    pub fn saved(&self, chosen: Option<&Path>) -> SavedResults {
        let summary = self.summary.clone().unwrap_or_default();
        SavedResults {
            summary: summary_line(&summary),
            folder: self.open_folder(chosen),
            rows: self.rows.iter().map(saved_row).collect(),
        }
    }
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

/// The folder holding every output, when there are several (`name (pages)`).
fn shared_folder(outputs: &[PathBuf]) -> Option<&Path> {
    if outputs.len() < 2 {
        return None;
    }
    let parent = outputs.first()?.parent()?;
    outputs
        .iter()
        .all(|o| o.parent() == Some(parent))
        .then_some(parent)
}

/// The row's name: "IMG_0012.jpg", or "3 files" for a merge.
fn row_name(input: &Path, inputs: usize) -> String {
    if inputs > 1 {
        strings::merged_inputs(inputs)
    } else {
        file_name(input)
    }
}

/// The target pill. Before there is an output: `planned` ("→ WebP",
/// "Compress"). Same name, new format: "→ WebP" (or "→ WEBP" when `planned`
/// is not a "→ …" summary). Otherwise the output's name: "→ Merged.pdf",
/// "→ photo (1).webp", "→ a (pages) (12 files)", "→ a.png (+2 more)".
fn target(input: &Path, inputs: usize, outputs: &[PathBuf], planned: &str) -> String {
    let to = strings::summary_to;
    if let Some(folder) = shared_folder(outputs) {
        return to(&format!(
            "{}{}",
            file_name(folder),
            strings::files_in_folder(outputs.len())
        ));
    }
    let Some(first) = outputs.first() else {
        return planned.to_owned();
    };
    if outputs.len() > 1 {
        return to(&format!(
            "{}{}",
            file_name(first),
            strings::more_outputs(outputs.len() - 1)
        ));
    }
    let same_stem = inputs <= 1
        && match (input.file_stem(), first.file_stem()) {
            (Some(a), Some(b)) => {
                a.to_string_lossy().to_lowercase() == b.to_string_lossy().to_lowercase()
            }
            _ => false,
        };
    if same_stem {
        if planned.starts_with(&to("")) {
            return planned.to_owned();
        }
        if let Some(ext) = first.extension() {
            return to(&ext.to_string_lossy().to_uppercase());
        }
    }
    to(&file_name(first))
}

/// What a results row's "Open" opens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpenTarget {
    File(PathBuf),
    Folder(PathBuf),
}

/// "Open": the file; for multi-output actions (pages, split parts) the new
/// folder that holds them. `is_dir` tells whether an output is a folder.
pub fn open_target(outputs: &[PathBuf], is_dir: impl Fn(&Path) -> bool) -> Option<OpenTarget> {
    if let Some(folder) = shared_folder(outputs) {
        return Some(OpenTarget::Folder(folder.to_path_buf()));
    }
    let first = outputs.first()?;
    Some(if is_dir(first) {
        OpenTarget::Folder(first.clone())
    } else {
        OpenTarget::File(first.clone())
    })
}

/// "Show in folder": the outputs selected in their folder; for multi-output
/// actions the new folder selected in its parent.
pub fn show_targets(outputs: &[PathBuf]) -> Vec<PathBuf> {
    match shared_folder(outputs) {
        Some(folder) => vec![folder.to_path_buf()],
        None => outputs.to_vec(),
    }
}

/// "4.1 MB → 0.9 MB".
pub fn sizes_line(bytes_in: u64, bytes_out: u64) -> String {
    format!(
        "{}{}{}",
        format_size(bytes_in),
        strings::ARROW,
        format_size(bytes_out)
    )
}

/// Size change in whole percent (−78 = 78% smaller); `None` without an input size.
pub fn size_change(bytes_in: u64, bytes_out: u64) -> Option<i64> {
    (bytes_in > 0)
        .then(|| ((bytes_out as f64 - bytes_in as f64) / bytes_in as f64 * 100.0).round() as i64)
}

fn row_view(row: &Row) -> RowView {
    let base = |status: Status| RowView {
        status,
        name: row_name(&row.input, row.inputs),
        target: row.target.clone(),
        ..RowView::default()
    };
    match &row.state {
        RowState::Waiting => RowView {
            message: strings::ROW_WAITING.to_owned(),
            ..base(Status::Waiting)
        },
        RowState::Running { fraction, .. } if !row.status.is_empty() => RowView {
            message: row.status.clone(),
            fraction: fraction.clamp(0.0, 1.0),
            indeterminate: *fraction <= 0.0,
            ..base(Status::Running)
        },
        RowState::Running { .. } if row.slow => RowView {
            message: strings::ROW_WORKING.to_owned(),
            indeterminate: true,
            ..base(Status::Running)
        },
        RowState::Running { fraction, eta } => {
            let fraction = fraction.clamp(0.0, 1.0);
            let mut message = format!("{}%", (fraction * 100.0).round() as u32);
            if let Some(eta) = eta {
                message.push_str(strings::HEADER_SEPARATOR);
                message.push_str(&strings::time_left(eta.as_secs()));
            }
            RowView {
                message,
                fraction,
                ..base(Status::Running)
            }
        }
        RowState::Done {
            outputs,
            bytes_in,
            bytes_out,
            note,
        } => RowView {
            target: target(&row.input, row.inputs, outputs, &row.target),
            sizes: if outputs.is_empty() {
                String::new()
            } else {
                sizes_line(*bytes_in, *bytes_out)
            },
            change: change_pill(outputs, *bytes_in, *bytes_out),
            smaller: !outputs.is_empty() && bytes_out < bytes_in,
            message: match note {
                Some(note) => note.clone(),
                None if outputs.is_empty() => strings::ROW_NOTHING_WRITTEN.to_owned(),
                None => String::new(),
            },
            fraction: 1.0,
            has_outputs: !outputs.is_empty(),
            ..base(Status::Done)
        },
        RowState::Failed { error, details } => RowView {
            message: format!("{}{error}", strings::ROW_FAILED_PREFIX),
            details: details_text(&row.input, error, details),
            ..base(Status::Failed)
        },
        RowState::Skipped { reason } => RowView {
            // Some reasons already say it ("Skipped: Images is unchecked.").
            message: if reason.starts_with(strings::ROW_SKIPPED) {
                reason.clone()
            } else {
                format!("{}: {reason}", strings::ROW_SKIPPED)
            },
            ..base(Status::Skipped)
        },
        RowState::Cancelled => RowView {
            message: strings::ROW_CANCELLED.to_owned(),
            ..base(Status::Cancelled)
        },
    }
}

/// The copyable text behind [Details].
fn details_text(input: &Path, error: &str, details: &str) -> String {
    format!("{}\n{error}\n\n{details}", input.display())
}

fn saved_row(row: &Row) -> SavedRow {
    let view = row_view(row);
    let (status, outputs) = match &row.state {
        RowState::Done { outputs, .. } => (RowStatus::Done, outputs.clone()),
        RowState::Failed { .. } => (RowStatus::Failed, Vec::new()),
        RowState::Skipped { .. } => (RowStatus::Skipped, Vec::new()),
        RowState::Waiting | RowState::Running { .. } | RowState::Cancelled => {
            (RowStatus::Cancelled, Vec::new())
        }
    };
    let change = match &row.state {
        RowState::Done {
            outputs,
            bytes_in,
            bytes_out,
            ..
        } if !outputs.is_empty() => size_change(*bytes_in, *bytes_out),
        _ => None,
    };
    SavedRow {
        input: row.input.clone(),
        inputs: row.inputs,
        status,
        outputs,
        sizes: view.sizes,
        message: view.message,
        details: view.details,
        target: view.target,
        change,
    }
}

/// The size change pill of a done row: "−78%", "+12%", "" (same, no output).
fn change_pill(outputs: &[PathBuf], bytes_in: u64, bytes_out: u64) -> String {
    match size_change(bytes_in, bytes_out) {
        Some(pct) if !outputs.is_empty() => strings::size_change(pct),
        _ => String::new(),
    }
}

/// A saved row, as the results view shows it.
pub fn saved_row_view(row: &SavedRow) -> RowView {
    let status = match row.status {
        RowStatus::Done => Status::Done,
        RowStatus::Failed => Status::Failed,
        RowStatus::Skipped => Status::Skipped,
        RowStatus::Cancelled => Status::Cancelled,
    };
    RowView {
        status,
        name: row_name(&row.input, row.inputs),
        // Files from before M9 have no target: work it out from the outputs.
        target: if row.target.is_empty() {
            target(&row.input, row.inputs, &row.outputs, "")
        } else {
            row.target.clone()
        },
        sizes: row.sizes.clone(),
        change: row.change.map(strings::size_change).unwrap_or_default(),
        smaller: row.change.is_some_and(|pct| pct < 0),
        message: row.message.clone(),
        details: row.details.clone(),
        fraction: 1.0,
        has_outputs: !row.outputs.is_empty(),
        indeterminate: false,
    }
}

/// " · saved 38.2 MB" / " · 1.2 MB larger" / "" (nothing converted or no change).
fn size_part(s: &Summary) -> String {
    if s.done == 0 || s.bytes_in == s.bytes_out {
        return String::new();
    }
    let text = if s.bytes_out < s.bytes_in {
        strings::saved(&format_size(s.saved()))
    } else {
        strings::grew(&format_size(s.bytes_out - s.bytes_in))
    };
    format!("{}{text}", strings::HEADER_SEPARATOR)
}

/// "11 converted, 1 failed · saved 38.2 MB".
pub fn summary_line(s: &Summary) -> String {
    let mut parts = vec![strings::n_converted(s.done)];
    if s.failed > 0 {
        parts.push(strings::n_failed(s.failed));
    }
    if s.skipped > 0 {
        parts.push(strings::n_skipped(s.skipped));
    }
    if s.cancelled > 0 {
        parts.push(strings::n_cancelled(s.cancelled));
    }
    format!("{}{}", parts.join(strings::LIST_SEPARATOR), size_part(s))
}

/// Toast title: "Converted 11 files · saved 38.2 MB", or the summary line when
/// something failed or was cancelled ("11 converted, 1 failed").
pub fn toast_title(s: &Summary) -> String {
    if s.failed == 0 && s.cancelled == 0 && s.done > 0 {
        format!("{}{}", strings::toast_converted(s.done), size_part(s))
    } else if s.failed > 0 {
        // SPEC §3.3: "11 converted, 1 failed".
        let mut t = format!(
            "{}{}{}",
            strings::n_converted(s.done),
            strings::LIST_SEPARATOR,
            strings::n_failed(s.failed)
        );
        if s.cancelled > 0 {
            t.push_str(strings::LIST_SEPARATOR);
            t.push_str(&strings::n_cancelled(s.cancelled));
        }
        t
    } else {
        summary_line(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use morph_core::job::OutputSpec;
    use morph_core::{Action, TaskId};
    use morph_formats::actions;

    fn job(names: &[&str]) -> Job {
        let files: Vec<PathBuf> = names
            .iter()
            .map(|n| PathBuf::from(r"C:\in").join(n))
            .collect();
        let action = Action::default_for(actions::IMAGE_CONVERT).unwrap();
        Job::for_files(&files, &action, &OutputSpec::default())
    }

    #[test]
    fn events_reduce_to_row_states() {
        let job = job(&["a.jpg", "b.png", "c.exe", "d.jpg"]);
        assert_eq!(job.tasks.len(), 3);
        let mut p = Progress::new(&job);
        assert_eq!(p.rows().len(), 4);
        assert_eq!(p.rows()[3].input, PathBuf::from(r"C:\in\c.exe"));
        assert_eq!(p.total(), 3);

        p.apply(Event::TaskSkipped {
            id: TaskId(3),
            reason: "Not supported for this action.".into(),
        });
        p.apply(Event::TaskStarted { id: TaskId(0) });
        p.apply(Event::TaskProgress {
            id: TaskId(0),
            fraction: 0.5,
            eta: None,
        });
        assert_eq!(p.row_view(0).status, Status::Running);
        assert_eq!(p.row_view(0).message, "50%");
        assert!((p.fraction() - 0.5 / 3.0).abs() < 1e-6);
        let mut dirty = p.take_dirty();
        dirty.sort_unstable();
        assert_eq!(dirty, vec![0, 3], "each row once");
        assert!(p.take_dirty().is_empty());

        p.apply(Event::TaskDone {
            id: TaskId(0),
            outputs: vec![PathBuf::from(r"C:\in\a.webp")],
            bytes_in: 4_300_000,
            bytes_out: 944_000,
            note: None,
        });
        // Late progress after done is ignored.
        p.apply(Event::TaskProgress {
            id: TaskId(0),
            fraction: 0.9,
            eta: None,
        });
        p.apply(Event::TaskFailed {
            id: TaskId(1),
            error: "This file seems to be damaged.".into(),
            details: "png: CRC mismatch".into(),
        });
        p.apply(Event::TaskSkipped {
            id: TaskId(2),
            reason: "Cancelled.".into(),
        });
        assert_eq!(p.finished(), 3);
        assert!(!p.is_finished());
        let counted = p.summary_from_rows();
        assert_eq!(
            (
                counted.done,
                counted.failed,
                counted.skipped,
                counted.cancelled
            ),
            (1, 1, 1, 1)
        );
        p.apply(Event::JobFinished {
            summary: Summary {
                done: 1,
                failed: 1,
                skipped: 1,
                cancelled: 1,
                bytes_in: 4_300_000,
                bytes_out: 944_000,
            },
        });
        assert!(p.is_finished());
        assert_eq!(p.fraction(), 1.0);

        let done = p.row_view(0);
        assert_eq!(done.status, Status::Done);
        assert_eq!(done.name, "a.jpg");
        assert_eq!(
            done.target,
            crate::view::action_summary(&job.tasks[0].action)
        );
        assert!(done.target.starts_with("→ "), "{}", done.target);
        assert_eq!(done.sizes, "4.1 MB → 921.9 KB");
        assert_eq!((done.change.as_str(), done.smaller), ("\u{2212}78%", true));
        assert!(done.has_outputs);
        let failed = p.row_view(1);
        assert_eq!(failed.status, Status::Failed);
        assert_eq!(failed.message, "Failed: This file seems to be damaged.");
        assert!(failed.details.contains("CRC mismatch"));
        assert_eq!(p.row_view(2).status, Status::Cancelled);
        assert_eq!(p.row_view(3).status, Status::Skipped);
        assert_eq!(p.open_folder(None), Some(PathBuf::from(r"C:\in")));

        let saved = p.saved(None);
        assert_eq!(
            saved.summary,
            "1 converted, 1 failed, 1 skipped, 1 cancelled · saved 3.2 MB"
        );
        assert_eq!(saved.rows.len(), 4);
        assert_eq!(saved.rows[0].status, RowStatus::Done);
        assert_eq!(saved_row_view(&saved.rows[0]), {
            let mut v = done.clone();
            v.fraction = 1.0;
            v
        });
        assert_eq!(saved.rows[2].status, RowStatus::Cancelled);
    }

    fn done(id: u64) -> Event {
        Event::TaskDone {
            id: TaskId(id),
            outputs: vec![PathBuf::from(format!(r"C:\in\{id}.webp"))],
            bytes_in: 1000,
            bytes_out: 400,
            note: None,
        }
    }

    fn progress(id: u64, fraction: f32) -> Event {
        Event::TaskProgress {
            id: TaskId(id),
            fraction,
            eta: None,
        }
    }

    /// Mirror of the UI list: the controller pushes only dirty rows per tick.
    fn push_dirty(p: &mut Progress, ui: &mut [RowView]) {
        for i in p.take_dirty() {
            ui[i] = p.row_view(i);
        }
    }

    #[test]
    fn status_text_replaces_the_percentage_until_cleared() {
        let mut p = Progress::new(&job(&["a.mp4"]));
        p.apply(Event::TaskStarted { id: TaskId(0) });
        p.apply(Event::TaskStatus {
            id: TaskId(0),
            text: "Checking the graphics card…".into(),
        });
        let view = p.row_view(0);
        assert_eq!(view.message, "Checking the graphics card…");
        assert!(view.indeterminate);
        p.apply(Event::TaskStatus {
            id: TaskId(0),
            text: String::new(),
        });
        p.apply(progress(0, 0.5));
        assert_eq!(p.row_view(0).message, "50%");
        p.apply(done(0));
        p.apply(Event::TaskStatus {
            id: TaskId(0),
            text: "late".into(),
        });
        assert_eq!(p.row_view(0).status, Status::Done);
        assert_ne!(p.row_view(0).message, "late");
    }

    #[test]
    fn progress_after_done_is_ignored() {
        let mut p = Progress::new(&job(&["a.jpg"]));
        let mut ui = p.row_views();
        p.apply(Event::TaskStarted { id: TaskId(0) });
        p.apply(progress(0, 0.68));
        push_dirty(&mut p, &mut ui);
        assert_eq!(ui[0].message, "68%");

        p.apply(done(0));
        p.apply(progress(0, 0.68));
        p.apply(Event::TaskStarted { id: TaskId(0) });
        push_dirty(&mut p, &mut ui);
        assert_eq!(ui[0].status, Status::Done);
        assert_eq!(ui[0].fraction, 1.0);
        assert_eq!(p.finished(), 1);
        // The late events didn't make the row dirty again.
        p.apply(progress(0, 0.9));
        assert!(p.take_dirty().is_empty());
        assert_eq!(p.row_view(0).status, Status::Done);
    }

    #[test]
    fn done_within_one_tick_shows_done() {
        let mut p = Progress::new(&job(&["a.jpg", "b.jpg", "c.jpg"]));
        let mut ui = p.row_views();
        // Everything for row 0 arrives between two ticks.
        p.apply(Event::TaskStarted { id: TaskId(0) });
        p.apply(progress(0, 0.3));
        p.apply(progress(0, 0.68));
        p.apply(done(0));
        // Row 1 is mid-way at this tick, finishes in the next one.
        p.apply(Event::TaskStarted { id: TaskId(1) });
        p.apply(progress(1, 0.68));
        push_dirty(&mut p, &mut ui);
        assert_eq!(ui[0].status, Status::Done);
        assert_eq!(ui[0].sizes, "1000 B → 400 B");
        assert_eq!(ui[0].change, "\u{2212}60%");
        assert_eq!(ui[1].message, "68%");

        p.apply(done(1));
        p.apply(Event::TaskStarted { id: TaskId(2) });
        p.apply(Event::TaskFailed {
            id: TaskId(2),
            error: "This file seems to be damaged.".into(),
            details: "x".into(),
        });
        p.apply(progress(2, 0.5));
        push_dirty(&mut p, &mut ui);
        assert_eq!(ui[1].status, Status::Done);
        assert_eq!(ui[2].status, Status::Failed);
        assert_eq!(ui, p.row_views());
    }

    #[test]
    fn overall_reaches_one_hundred_percent() {
        let mut p = Progress::new(&job(&["a.jpg", "b.jpg"]));
        p.apply(Event::TaskStarted { id: TaskId(0) });
        p.apply(progress(0, 0.5));
        assert!((p.fraction() - 0.25).abs() < 1e-6);
        p.apply(done(0));
        p.apply(done(1));
        assert_eq!(p.fraction(), 1.0);
        p.apply(Event::JobFinished {
            summary: Summary::default(),
        });
        assert_eq!(p.fraction(), 1.0);

        // A row left open when the job ends is closed and counted.
        let mut p = Progress::new(&job(&["a.jpg", "b.jpg"]));
        p.apply(done(0));
        p.apply(Event::TaskStarted { id: TaskId(1) });
        p.apply(progress(1, 0.68));
        p.apply(Event::JobFinished {
            summary: Summary::default(),
        });
        assert_eq!(p.finished(), p.total());
        assert_eq!(p.fraction(), 1.0);
        assert_eq!(p.row_view(1).status, Status::Cancelled);
    }

    #[test]
    fn done_without_output_shows_note() {
        let mut p = Progress::new(&job(&["a.png"]));
        p.apply(Event::TaskDone {
            id: TaskId(0),
            outputs: vec![],
            bytes_in: 10,
            bytes_out: 10,
            note: None,
        });
        let v = p.row_view(0);
        assert_eq!((v.sizes.as_str(), v.has_outputs), ("", false));
        assert_eq!(v.message, strings::ROW_NOTHING_WRITTEN);
    }

    #[test]
    fn video_rows_show_percent_time_left_and_notes() {
        let files = [PathBuf::from(r"C:\in\clip.mp4")];
        let action = Action::default_for(actions::VIDEO_COMPRESS).unwrap();
        let job = Job::for_files(&files, &action, &OutputSpec::default());
        let mut p = Progress::new(&job);
        assert!(!p.rows()[0].slow, "FFmpeg reports real progress");
        p.apply(Event::TaskProgress {
            id: TaskId(0),
            fraction: 0.41,
            eta: Some(Duration::from_secs(95)),
        });
        let v = p.row_view(0);
        assert_eq!(v.message, "41% · about 2 min left");
        assert!(!v.indeterminate && !p.indeterminate());
        assert!((p.fraction() - 0.41).abs() < 1e-6);
        p.apply(Event::TaskProgress {
            id: TaskId(0),
            fraction: 0.9,
            eta: Some(Duration::from_secs(20)),
        });
        assert_eq!(p.row_view(0).message, "90% · less than a minute left");
        // "Best" found nothing to gain: no file, the core's note.
        p.apply(Event::TaskDone {
            id: TaskId(0),
            outputs: vec![],
            bytes_in: 10,
            bytes_out: 10,
            note: Some("Already well compressed".into()),
        });
        let v = p.row_view(0);
        assert_eq!(v.message, "Already well compressed");
        assert!(!v.has_outputs);
        // A note next to a written file (stepped-down resolution).
        let mut p = Progress::new(&job);
        p.apply(Event::TaskDone {
            id: TaskId(0),
            outputs: vec![PathBuf::from(r"C:\in\clip (compressed).mp4")],
            bytes_in: 100 << 20,
            bytes_out: 9 << 20,
            note: Some("Resolution lowered to 720p to fit 10 MB.".into()),
        });
        let v = p.row_view(0);
        assert_eq!(v.message, "Resolution lowered to 720p to fit 10 MB.");
        assert!(v.has_outputs && v.change == "\u{2212}91%" && v.smaller);
        assert_eq!(v.target, "→ clip (compressed).mp4");
    }

    #[test]
    fn merge_is_one_row_and_handles_a_failure() {
        let files: Vec<PathBuf> = ["2.pdf", "10.pdf", "c.jpg", "d.mp4"]
            .iter()
            .map(|n| PathBuf::from(r"C:\in").join(n))
            .collect();
        let action = Action::default_for(actions::PDF_MERGE).unwrap();
        let job = Job::for_files(&files, &action, &OutputSpec::default());
        let mut p = Progress::new(&job);
        assert_eq!(p.total(), 1, "one task for every merged file");
        assert_eq!(p.rows().len(), 2, "the merge row + the skipped video");
        assert_eq!(p.row_view(0).name, "3 files");

        // The converter stub fails: the row shows it, with copyable details.
        p.apply(Event::TaskStarted { id: TaskId(0) });
        p.apply(Event::TaskFailed {
            id: TaskId(0),
            error: "This PDF action isn't available yet.".into(),
            details: "pdf.merge: not implemented".into(),
        });
        p.apply(Event::TaskSkipped {
            id: TaskId(1),
            reason: "Not supported for this action.".into(),
        });
        p.apply(Event::JobFinished {
            summary: Summary {
                failed: 1,
                skipped: 1,
                ..Summary::default()
            },
        });
        let v = p.row_view(0);
        assert_eq!(v.status, Status::Failed);
        assert_eq!(v.message, "Failed: This PDF action isn't available yet.");
        assert!(v.details.contains("not implemented"));
        assert!(!v.has_outputs);
        assert_eq!(p.open_folder(None), None);
        assert_eq!(p.saved(None).summary, "0 converted, 1 failed, 1 skipped");

        let mut p = Progress::new(&job);
        p.apply(Event::TaskDone {
            id: TaskId(0),
            outputs: vec![PathBuf::from(r"C:\in\Merged.pdf")],
            bytes_in: 3000,
            bytes_out: 2000,
            note: None,
        });
        let v = p.row_view(0);
        assert_eq!(
            (v.name.as_str(), v.target.as_str()),
            ("3 files", "→ Merged.pdf")
        );
        let saved = p.saved(None);
        assert_eq!(saved.rows[0].inputs, 3);
        assert_eq!(saved_row_view(&saved.rows[0]), v);
    }

    #[test]
    fn multi_output_rows_open_their_folder() {
        let pages: Vec<PathBuf> = (1..=3)
            .map(|i| PathBuf::from(format!(r"C:\in\a (pages)\a-00{i}.png")))
            .collect();
        let mut p = Progress::new(&job(&["a.jpg"]));
        p.apply(Event::TaskDone {
            id: TaskId(0),
            outputs: pages.clone(),
            bytes_in: 10,
            bytes_out: 30,
            note: None,
        });
        assert_eq!(p.row_view(0).target, "→ a (pages) (3 files)");
        assert_eq!(p.open_folder(None), Some(PathBuf::from(r"C:\in")));
        let folder = PathBuf::from(r"C:\in\a (pages)");
        assert_eq!(
            open_target(&pages, |_| false),
            Some(OpenTarget::Folder(folder.clone()))
        );
        assert_eq!(show_targets(&pages), vec![folder.clone()]);
        // One output that is a folder, or a plain file.
        assert_eq!(
            open_target(std::slice::from_ref(&folder), |_| true),
            Some(OpenTarget::Folder(folder.clone()))
        );
        let file = PathBuf::from(r"C:\in\a.webp");
        assert_eq!(
            open_target(std::slice::from_ref(&file), |_| false),
            Some(OpenTarget::File(file.clone()))
        );
        assert_eq!(show_targets(std::slice::from_ref(&file)), vec![file]);
        assert_eq!(open_target(&[], |_| false), None);
    }

    #[test]
    fn mixed_job_open_folder_and_lone_status() {
        // Progress only sees task IDs and outputs; three image tasks stand in
        // for image / PDF / video groups.
        let mut p = Progress::new(&job(&["a.jpg", "b.jpg", "c.jpg"]));
        p.apply(Event::TaskStarted { id: TaskId(0) });
        p.apply(Event::TaskStarted { id: TaskId(2) });
        p.apply(Event::TaskStatus {
            id: TaskId(2),
            text: "Checking the graphics card…".into(),
        });
        assert_eq!(p.lone_status(), None, "two files running");
        let out = |id: u64, path: &str| Event::TaskDone {
            id: TaskId(id),
            outputs: vec![PathBuf::from(path)],
            bytes_in: 10,
            bytes_out: 5,
            note: None,
        };
        p.apply(out(0, r"C:\in\a.webp"));
        assert_eq!(p.lone_status(), Some("Checking the graphics card…"));
        // The PDF's pages go in their own folder; the common folder is C:\in.
        p.apply(Event::TaskDone {
            id: TaskId(1),
            outputs: vec![
                PathBuf::from(r"C:\in\b (pages)\b-1.png"),
                PathBuf::from(r"C:\in\b (pages)\b-2.png"),
            ],
            bytes_in: 10,
            bytes_out: 5,
            note: None,
        });
        p.apply(out(2, r"C:\in\c (compressed).mp4"));
        assert_eq!(p.open_folder(None), Some(PathBuf::from(r"C:\in")));
        let chosen = PathBuf::from(r"D:\out");
        assert_eq!(p.open_folder(Some(&chosen)), Some(chosen.clone()));
    }

    #[test]
    fn pdf_split_rows_match_pdf_to_images() {
        // Split returns its part files (like PDF → images), so sizes add up and
        // Open goes to the shared `name (split)` folder.
        let parts: Vec<PathBuf> = ["a-p1-2.pdf", "a-p3.pdf"]
            .iter()
            .map(|n| PathBuf::from(r"C:\in\a (split)").join(n))
            .collect();
        let mut p = Progress::new(&job(&["a.pdf"]));
        p.apply(Event::TaskDone {
            id: TaskId(0),
            outputs: parts.clone(),
            bytes_in: 1000,
            bytes_out: 1100,
            note: None,
        });
        let v = p.row_view(0);
        assert_eq!(v.target, "→ a (split) (2 files)");
        assert_eq!(v.sizes, "1000 B → 1.1 KB");
        assert_eq!((v.change.as_str(), v.smaller), ("+10%", false));
        let folder = PathBuf::from(r"C:\in\a (split)");
        assert_eq!(
            open_target(&parts, |_| false),
            Some(OpenTarget::Folder(folder.clone()))
        );
        assert_eq!(show_targets(&parts), vec![folder]);
        assert_eq!(p.open_folder(None), Some(PathBuf::from(r"C:\in")));

        // A one-page PDF → images: Open opens the image itself.
        let one = PathBuf::from(r"C:\in\b (pages)\b-1.png");
        assert_eq!(
            open_target(std::slice::from_ref(&one), |_| false),
            Some(OpenTarget::File(one))
        );
    }

    #[test]
    fn office_rows_show_working_not_a_stuck_percentage() {
        let files: Vec<PathBuf> = ["a.docx", "b.csv", "c.pptx"]
            .iter()
            .map(|n| PathBuf::from(r"C:\in").join(n))
            .collect();
        let pdf = Action::default_for(actions::OFFICE_CONVERT).unwrap();
        let job = Job::for_files(&files, &pdf, &OutputSpec::default());
        let mut p = Progress::new(&job);
        assert!(
            p.rows()[0].slow && p.rows()[1].slow,
            "PDF output needs the engine"
        );
        assert!(!p.indeterminate(), "nothing running yet");
        p.apply(Event::TaskStarted { id: TaskId(0) });
        p.apply(progress(0, 0.1));
        let v = p.row_view(0);
        assert_eq!(v.status, Status::Running);
        assert!(v.indeterminate);
        assert_eq!(v.message, strings::ROW_WORKING);
        assert!(p.indeterminate());
        // A stub converter fails: a normal failure row.
        p.apply(Event::TaskFailed {
            id: TaskId(0),
            error: "This conversion isn't available yet.".into(),
            details: "office.convert: not implemented yet".into(),
        });
        let v = p.row_view(0);
        assert_eq!(v.status, Status::Failed);
        assert!(!v.indeterminate);
        assert_eq!(v.message, "Failed: This conversion isn't available yet.");
        p.apply(Event::TaskStarted { id: TaskId(1) });
        assert!(
            !p.indeterminate(),
            "one task finished: the bar means something now"
        );

        // CSV → XLSX is native (CPU queue): a normal percentage.
        let opts = morph_core::office::OfficeConvertOpts {
            format: morph_core::office::DocFormat::Xlsx,
            ..Default::default()
        };
        let job = Job::for_files(
            &files[1..2],
            &Action::OfficeConvert(opts),
            &OutputSpec::default(),
        );
        let mut p = Progress::new(&job);
        p.apply(Event::TaskStarted { id: TaskId(0) });
        p.apply(progress(0, 0.4));
        assert!(!p.row_view(0).indeterminate);
        assert_eq!(p.row_view(0).message, "40%");
        assert!(!p.indeterminate());
    }

    #[test]
    fn size_change_formatting() {
        assert_eq!(sizes_line(1000, 1120), "1000 B → 1.1 KB");
        let pill = |a, b| change_pill(&[PathBuf::from("x")], a, b);
        assert_eq!(pill(1024, 1024), "");
        assert_eq!(pill(1000, 1120), "+12%");
        assert_eq!(pill(1000, 667), "\u{2212}33%");
        assert_eq!(pill(1000, 1004), "");
        assert_eq!(pill(0, 50), "");
        assert_eq!(change_pill(&[], 1000, 10), "", "no output, no pill");
    }

    #[test]
    fn target_pill_names_the_format_or_the_new_name() {
        let input = Path::new(r"C:\in\photo.png");
        let one = |o: &str| vec![PathBuf::from(o)];
        // Before an output: what the action makes.
        assert_eq!(target(input, 1, &[], "→ WebP"), "→ WebP");
        assert_eq!(target(input, 1, &[], "Compress"), "Compress");
        // Same name, new format.
        assert_eq!(
            target(input, 1, &one(r"C:\in\photo.webp"), "→ WebP"),
            "→ WebP"
        );
        assert_eq!(
            target(input, 1, &one(r"C:\in\Photo.JPG"), "Compress"),
            "→ JPG"
        );
        // The name changed: collision, merge.
        assert_eq!(
            target(input, 1, &one(r"C:\in\photo (1).webp"), "→ WebP"),
            "→ photo (1).webp"
        );
        assert_eq!(
            target(input, 3, &one(r"C:\in\photo.pdf"), "Merge"),
            "→ photo.pdf"
        );
        let two = vec![PathBuf::from(r"C:\a\x.png"), PathBuf::from(r"C:\b\y.png")];
        assert_eq!(target(input, 1, &two, ""), "→ x.png (+1 more)");
        // Saved rows from before M9 (no stored target) still get one.
        let old = SavedRow {
            input: input.to_path_buf(),
            inputs: 1,
            status: RowStatus::Done,
            outputs: one(r"C:\in\photo.webp"),
            sizes: "4.1 MB → 0.9 MB · 78% smaller".into(),
            message: String::new(),
            details: String::new(),
            target: String::new(),
            change: None,
        };
        let v = saved_row_view(&old);
        assert_eq!((v.target.as_str(), v.change.as_str()), ("→ WEBP", ""));
    }

    #[test]
    fn skip_reasons_are_not_prefixed_twice() {
        let mut p = Progress::new(&job(&["a.jpg", "b.exe", "c.exe"]));
        p.apply(Event::TaskSkipped {
            id: TaskId(1),
            reason: strings::skipped_by_choice("Images"),
        });
        p.apply(Event::TaskSkipped {
            id: TaskId(2),
            reason: "Not supported for this action.".into(),
        });
        assert_eq!(p.row_view(1).message, "Skipped: Images is unchecked.");
        assert_eq!(
            p.row_view(2).message,
            "Skipped: Not supported for this action."
        );
        assert_eq!(p.row_view(1).target, "", "skipped inputs have no target");
    }

    #[test]
    fn summary_and_toast_lines() {
        let s = Summary {
            done: 11,
            failed: 1,
            bytes_in: 50_000_000,
            bytes_out: 10_000_000,
            ..Summary::default()
        };
        assert_eq!(summary_line(&s), "11 converted, 1 failed · saved 38.1 MB");
        assert_eq!(toast_title(&s), "11 converted, 1 failed");

        let ok = Summary {
            failed: 0,
            ..s.clone()
        };
        assert_eq!(toast_title(&ok), "Converted 11 files · saved 38.1 MB");
        let one = Summary {
            done: 1,
            bytes_in: 100,
            bytes_out: 100,
            ..Summary::default()
        };
        assert_eq!(summary_line(&one), "1 converted");
        assert_eq!(toast_title(&one), "Converted 1 file");
        let grew = Summary {
            done: 2,
            bytes_in: 1024,
            bytes_out: 3072,
            ..Summary::default()
        };
        assert_eq!(summary_line(&grew), "2 converted · 2.0 KB larger");
        let none = Summary {
            failed: 2,
            bytes_in: 0,
            ..Summary::default()
        };
        assert_eq!(summary_line(&none), "0 converted, 2 failed");
        let cancelled = Summary {
            done: 3,
            cancelled: 9,
            bytes_in: 300,
            bytes_out: 100,
            ..Summary::default()
        };
        assert_eq!(
            toast_title(&cancelled),
            "3 converted, 9 cancelled · saved 200 B"
        );
    }
}
