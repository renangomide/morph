// SPDX-License-Identifier: GPL-3.0-or-later
//! Turning events into human lines or JSON lines.

use crate::strings;
use morph_core::{Event, Summary, TaskId};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};

/// Task id -> input file, for readable lines.
pub type Names = HashMap<u64, PathBuf>;

/// Unicode symbols on a console, ASCII when stdout is redirected or piped.
pub fn symbols() -> &'static strings::Symbols {
    if std::io::stdout().is_terminal() {
        &strings::UNICODE
    } else {
        &strings::ASCII
    }
}

pub fn emit(line: &str) {
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{line}");
}

pub fn emit_err(line: &str) {
    let mut err = std::io::stderr().lock();
    let _ = writeln!(err, "{line}");
}

fn file_name(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    )
}

fn name_of(names: &Names, id: TaskId) -> String {
    names
        .get(&id.0)
        .map_or_else(|| format!("task {}", id.0), |p| file_name(p))
}

/// `4.1 MB`, `512 B`, `0.9 KB` (1024-based).
pub fn human_size(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    let b = bytes as f64;
    if bytes < 1024 {
        format!("{bytes} B")
    } else if b < KB * KB {
        format!("{:.1} KB", b / KB)
    } else if b < KB * KB * KB {
        format!("{:.1} MB", b / (KB * KB))
    } else {
        format!("{:.1} GB", b / (KB * KB * KB))
    }
}

/// `−78%` when smaller, `+12%` when bigger, `0%` when equal.
pub fn size_change(bytes_in: u64, bytes_out: u64, sym: &strings::Symbols) -> String {
    if bytes_in == 0 {
        return "0%".to_owned();
    }
    let pct = ((bytes_out as f64 - bytes_in as f64) / bytes_in as f64 * 100.0).round() as i64;
    match pct {
        0 => "0%".to_owned(),
        p if p < 0 => format!("{}{}%", sym.minus, -p),
        p => format!("+{p}%"),
    }
}

/// Lines to print for one event in human mode (empty for progress/start).
pub fn human_lines(event: &Event, names: &Names, sym: &strings::Symbols) -> Vec<String> {
    match event {
        Event::TaskStarted { .. } | Event::TaskProgress { .. } => Vec::new(),
        Event::TaskStatus { text, .. } if text.is_empty() => Vec::new(),
        Event::TaskStatus { id, text } => vec![strings::task_status(&name_of(names, *id), text)],
        Event::TaskDone {
            id,
            outputs,
            bytes_in,
            bytes_out,
            note,
        } => {
            if outputs.is_empty() {
                let text = note.as_deref().unwrap_or(strings::NO_CHANGE_NOTE);
                return vec![strings::no_output(&name_of(names, *id), text)];
            }
            let first = outputs.first().map_or_else(String::new, |p| file_name(p));
            let mut out_name = first;
            if outputs.len() > 1 {
                out_name.push(' ');
                out_name.push_str(&strings::more_outputs(outputs.len() - 1));
            }
            let mut lines = vec![strings::task_done(
                &name_of(names, *id),
                &out_name,
                &strings::sizes(&human_size(*bytes_in), &human_size(*bytes_out), sym),
                &size_change(*bytes_in, *bytes_out, sym),
                sym,
            )];
            if let Some(note) = note {
                lines.push(format!("    {note}"));
            }
            lines
        }
        Event::TaskFailed { id, error, details } => {
            let mut lines = vec![strings::task_failed(&name_of(names, *id), error, sym)];
            lines.extend(
                details
                    .lines()
                    .filter(|l| !l.trim().is_empty())
                    .map(|l| format!("    {l}")),
            );
            lines
        }
        Event::TaskSkipped { id, reason } => {
            vec![strings::task_skipped(&name_of(names, *id), reason, sym)]
        }
        Event::JobFinished { summary } => vec![summary_line(summary, sym)],
    }
}

pub fn summary_line(s: &Summary, sym: &strings::Symbols) -> String {
    strings::summary_line(
        s.done,
        s.failed,
        s.skipped,
        s.cancelled,
        &human_size(s.bytes_in),
        &human_size(s.bytes_out),
        sym,
    )
}

fn path_str(p: &Path) -> String {
    p.display().to_string()
}

/// One JSON object per event.
pub fn json_value(event: &Event, names: &Names) -> Value {
    let input = |id: &TaskId| names.get(&id.0).map(|p| path_str(p));
    match event {
        Event::TaskStarted { id } => {
            json!({"event": "task_started", "id": id.0, "input": input(id)})
        }
        Event::TaskProgress { id, fraction, eta } => json!({
            "event": "task_progress", "id": id.0, "input": input(id),
            "fraction": fraction, "eta_secs": eta.map(|d| d.as_secs_f64()),
        }),
        Event::TaskStatus { id, text } => {
            json!({"event": "task_status", "id": id.0, "input": input(id), "text": text})
        }
        Event::TaskDone {
            id,
            outputs,
            bytes_in,
            bytes_out,
            note,
        } => json!({
            "event": "task_done", "id": id.0, "input": input(id),
            "outputs": outputs.iter().map(|p| path_str(p)).collect::<Vec<_>>(),
            "bytes_in": bytes_in, "bytes_out": bytes_out, "note": note,
        }),
        Event::TaskFailed { id, error, details } => json!({
            "event": "task_failed", "id": id.0, "input": input(id),
            "error": error, "details": details,
        }),
        Event::TaskSkipped { id, reason } => {
            json!({"event": "task_skipped", "id": id.0, "input": input(id), "reason": reason})
        }
        Event::JobFinished { summary } => json!({
            "event": "job_finished",
            "summary": {
                "done": summary.done, "failed": summary.failed,
                "skipped": summary.skipped, "cancelled": summary.cancelled,
                "bytes_in": summary.bytes_in, "bytes_out": summary.bytes_out,
            },
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names() -> Names {
        let mut n = Names::new();
        n.insert(0, PathBuf::from("dir").join("photo.png"));
        n
    }

    #[test]
    fn status_lines() {
        let ev = Event::TaskStatus {
            id: TaskId(0),
            text: "Checking the graphics card…".into(),
        };
        let lines = human_lines(&ev, &names(), &strings::ASCII);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("photo.png") && lines[0].contains("graphics card"));
        let clear = Event::TaskStatus {
            id: TaskId(0),
            text: String::new(),
        };
        assert!(human_lines(&clear, &names(), &strings::ASCII).is_empty());
        assert_eq!(json_value(&ev, &names())["event"], "task_status");
    }

    #[test]
    fn sizes_are_readable() {
        assert_eq!(human_size(512), "512 B");
        assert_eq!(human_size(2048), "2.0 KB");
        assert_eq!(human_size(4_300_000), "4.1 MB");
    }

    #[test]
    fn change_is_signed() {
        assert_eq!(size_change(1000, 220, &strings::UNICODE), "−78%");
        assert_eq!(size_change(1000, 1120, &strings::UNICODE), "+12%");
        assert_eq!(size_change(1000, 1000, &strings::UNICODE), "0%");
        assert_eq!(size_change(0, 10, &strings::UNICODE), "0%");
    }

    #[test]
    fn done_line() {
        let ev = Event::TaskDone {
            id: TaskId(0),
            outputs: vec![PathBuf::from("out").join("photo.webp")],
            bytes_in: 4_300_000,
            bytes_out: 946_000,
            note: None,
        };
        assert_eq!(
            human_lines(&ev, &names(), &strings::UNICODE),
            vec!["✓ photo.png → photo.webp  4.1 MB → 923.8 KB  −78%".to_owned()]
        );
    }

    #[test]
    fn ascii_lines_have_no_unicode() {
        let done = Event::TaskDone {
            id: TaskId(0),
            outputs: vec![PathBuf::from("photo.webp")],
            bytes_in: 4_300_000,
            bytes_out: 946_000,
            note: None,
        };
        let a = &strings::ASCII;
        assert_eq!(
            human_lines(&done, &names(), a),
            vec!["OK photo.png -> photo.webp  4.1 MB -> 923.8 KB  -78%".to_owned()]
        );
        let fail = Event::TaskFailed {
            id: TaskId(0),
            error: "bad".into(),
            details: String::new(),
        };
        let skip = Event::TaskSkipped {
            id: TaskId(0),
            reason: "r".into(),
        };
        assert_eq!(human_lines(&fail, &names(), a), vec!["FAIL photo.png  bad"]);
        assert_eq!(
            human_lines(&skip, &names(), a),
            vec!["SKIP photo.png  skipped: r"]
        );
        let fin = Event::JobFinished {
            summary: Summary {
                done: 1,
                failed: 0,
                skipped: 0,
                cancelled: 0,
                bytes_in: 2048,
                bytes_out: 1024,
            },
        };
        let lines = human_lines(&fin, &names(), a);
        assert!(lines.iter().all(|l| l.is_ascii()), "{lines:?}");
    }

    #[test]
    fn already_optimized_line() {
        let ev = Event::TaskDone {
            id: TaskId(0),
            outputs: Vec::new(),
            bytes_in: 100,
            bytes_out: 100,
            note: Some("Already optimized".into()),
        };
        assert_eq!(
            human_lines(&ev, &names(), &strings::UNICODE),
            vec!["= photo.png: Already optimized".to_owned()]
        );
    }

    #[test]
    fn failed_line_has_indented_details() {
        let ev = Event::TaskFailed {
            id: TaskId(0),
            error: "This image is damaged.".into(),
            details: "bad header\nat byte 8".into(),
        };
        assert_eq!(
            human_lines(&ev, &names(), &strings::UNICODE),
            vec![
                "✕ photo.png  This image is damaged.".to_owned(),
                "    bad header".to_owned(),
                "    at byte 8".to_owned(),
            ]
        );
    }

    #[test]
    fn skipped_line_and_json() {
        let ev = Event::TaskSkipped {
            id: TaskId(0),
            reason: "Not supported for this action.".into(),
        };
        assert_eq!(
            human_lines(&ev, &names(), &strings::UNICODE),
            vec!["· photo.png  skipped: Not supported for this action.".to_owned()]
        );
        let v = json_value(&ev, &names());
        assert_eq!(v["event"], "task_skipped");
        assert_eq!(v["reason"], "Not supported for this action.");
    }

    #[test]
    fn finished_summary() {
        let ev = Event::JobFinished {
            summary: Summary {
                done: 2,
                failed: 1,
                skipped: 0,
                cancelled: 0,
                bytes_in: 2048,
                bytes_out: 1024,
            },
        };
        assert_eq!(
            human_lines(&ev, &names(), &strings::UNICODE),
            vec!["2 done, 1 failed, 0 skipped. 2.0 KB → 1.0 KB".to_owned()]
        );
    }
}
