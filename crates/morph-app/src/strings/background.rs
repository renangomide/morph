// SPDX-License-Identifier: GPL-3.0-or-later
//! Progress / results view text and background job text: tray, toast,
//! summary, results (SPEC §3.3).

/// Tray tooltip with the running file's status on a second line:
/// "Morph: converting 0 / 1 (0%)\nChecking the graphics card…".
pub fn tray_tooltip_with_status(tooltip: &str, status: &str) -> String {
    format!("{tooltip}\n{status}")
}

// ── Progress view ────────────────────────────────────────────────────────────

/// "Converting 12 files…".
pub fn converting(count: usize) -> String {
    if count == 1 {
        "Converting 1 file…".to_owned()
    } else {
        format!("Converting {count} files…")
    }
}
pub const CANCELLING: &str = "Cancelling…";
pub const CANCEL_ALL: &str = "Cancel all";
pub const RUN_IN_BACKGROUND: &str = "Run in background";
pub const OPEN_FOLDER: &str = "Open folder";
pub const OPEN: &str = "Open";
pub const SHOW_IN_FOLDER: &str = "Show in folder";
pub const DETAILS: &str = "Details…";
pub const COPY: &str = "Copy details";
pub const COPIED: &str = "Copied";
pub const DETAILS_TITLE: &str = "Error details";

pub const ROW_WAITING: &str = "Waiting…";
/// Running row of a slow engine conversion (no meaningful percentage).
pub const ROW_WORKING: &str = "Working…";
pub const ROW_CANCELLED: &str = "Cancelled";
pub const ROW_SKIPPED: &str = "Skipped";
pub const ROW_FAILED_PREFIX: &str = "Failed: ";
pub const ROW_NOTHING_WRITTEN: &str = "No new file was needed.";
pub const ARROW: &str = " → ";
/// " (+2 more)" after the first output name.
pub fn more_outputs(n: usize) -> String {
    format!(" (+{n} more)")
}
/// Size change pill next to the sizes: "−33%" / "+12%" / "" (same size).
pub fn size_change(percent: i64) -> String {
    match percent {
        p if p < 0 => format!("\u{2212}{}%", -p),
        p if p > 0 => format!("+{p}%"),
        _ => String::new(),
    }
}

/// Summary pieces: "11 converted", "1 failed", …
pub fn n_converted(n: usize) -> String {
    format!("{n} converted")
}
pub fn n_failed(n: usize) -> String {
    format!("{n} failed")
}
pub fn n_skipped(n: usize) -> String {
    format!("{n} skipped")
}
pub fn n_cancelled(n: usize) -> String {
    format!("{n} cancelled")
}
pub const LIST_SEPARATOR: &str = ", ";
/// "saved 38.2 MB".
pub fn saved(size: &str) -> String {
    format!("saved {size}")
}
/// "1.2 MB larger".
pub fn grew(size: &str) -> String {
    format!("{size} larger")
}
/// Toast title when nothing failed: "Converted 11 files · saved 38.2 MB".
pub fn toast_converted(count: usize) -> String {
    if count == 1 {
        "Converted 1 file".to_owned()
    } else {
        format!("Converted {count} files")
    }
}

/// Tray tooltip: "Morph: converting 7 / 12 (58%)".
pub fn tray_tooltip(app: &str, done: usize, total: usize, percent: u32) -> String {
    format!("{app}: converting {done} / {total} ({percent}%)")
}
/// Tray tooltip after Cancel.
pub fn tray_cancelling(app: &str) -> String {
    format!("{app}: cancelling…")
}

// Tray icon menu and toast button (platform layer).
pub const TRAY_SHOW_PROGRESS: &str = "Show progress";
pub const TRAY_CANCEL: &str = "Cancel";
pub const TOAST_OPEN_FOLDER: &str = "Open folder";
/// Toast button: shows the results window (like a click on the toast).
pub const TOAST_VIEW_RESULTS: &str = "View results";

// ── M9: finished view ────────────────────────────────────────────────────────

/// Finished view button: back to the options page with the same files.
pub const ADJUST_OPTIONS: &str = "Adjust options";
/// Summary subtitle: where the files went.
pub const SAVED_NEXT_TO: &str = "Saved next to the original files";
/// Summary subtitle with a chosen folder: "Saved to C:\Users\…\Pictures".
pub fn saved_to(folder: &str) -> String {
    format!("Saved to {folder}")
}
