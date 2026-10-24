// SPDX-License-Identifier: GPL-3.0-or-later
//! Small log: `<LocalData>\logs\shell.log`, i.e.
//! `%LOCALAPPDATA%\Packages\OpenMorph.Morph_<hash>\LocalState\logs\shell.log`.
//! Errors only (SPEC §5.5). The INFO breadcrumbs (one line per menu call and
//! click, to trace a click that does nothing) are off unless the user
//! environment variable `MORPH_SHELL_LOG=1` is set (then restart Explorer).
//! Logging must never fail loudly.

use std::io::Write;
use std::sync::OnceLock;

/// Past this size the log is moved to `shell.old.log` (one old copy kept).
const MAX_BYTES: u64 = 256 * 1024;

pub(crate) fn error(message: &str) {
    write("ERROR", message);
}

/// Breadcrumb: written only when `MORPH_SHELL_LOG=1` (read once per process,
/// so the menu path does no file I/O for it by default).
/// `message` is only built when breadcrumbs are on.
pub(crate) fn info(message: impl FnOnce() -> String) {
    if breadcrumbs_on() {
        write("INFO", &message());
    }
}

fn breadcrumbs_on() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("MORPH_SHELL_LOG").is_some_and(|v| v == "1"))
}

fn write(level: &str, message: &str) {
    let Some(root) = morph_formats::paths::shell_data_dir() else {
        return;
    };
    let dir = morph_formats::paths::logs_dir(&root);
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    let path = dir.join("shell.log");
    if std::fs::metadata(&path).is_ok_and(|m| m.len() > MAX_BYTES) {
        let _ = std::fs::rename(&path, dir.join("shell.old.log"));
    }
    let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    else {
        return;
    };
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let pid = std::process::id();
    let _ = writeln!(file, "[{secs}] [pid {pid}] {level} {message}");
}
