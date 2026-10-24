// SPDX-License-Identifier: GPL-3.0-or-later
//! Log for `Morph.exe` (SPEC §5.5): `<LocalData>\logs\morph-YYYY-MM-DD.log`,
//! one file per **local** day, local timestamps, the 7 newest files kept.
//! `tracing` events go through `tracing_appender::non_blocking` (a background
//! thread does the file writes) into [`DailyFile`].
//! Our crates log at INFO (job start/finish, failures with details, panics);
//! other crates only at WARN. Paths are logged, file contents never.
//! Best effort: there is no console, and logging must never stop the app.

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::filter::{LevelFilter, Targets};
use tracing_subscriber::fmt::format::Writer;
use tracing_subscriber::fmt::time::FormatTime;
use tracing_subscriber::prelude::*;

/// Log files kept (one per day).
const KEEP_FILES: usize = 7;
const PREFIX: &str = "morph-";
const SUFFIX: &str = ".log";

/// Flushes the background writer when dropped; taken by [`flush`].
static GUARD: Mutex<Option<WorkerGuard>> = Mutex::new(None);

/// Start file logging and log panics. Call once, first thing in `main`.
pub fn init() {
    let Some(root) = morph_formats::paths::app_data_dir() else {
        return;
    };
    let dir = morph_formats::paths::logs_dir(&root);
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    remove_legacy(&dir);
    let (writer, guard) = tracing_appender::non_blocking(DailyFile::new(dir, local_date));
    let filter = Targets::new()
        .with_default(LevelFilter::WARN)
        .with_target("Morph", LevelFilter::INFO)
        .with_target("morph_core", LevelFilter::INFO);
    let layer = tracing_subscriber::fmt::layer()
        .with_writer(writer)
        .with_timer(LocalTime)
        .with_ansi(false)
        .with_thread_names(true);
    if tracing_subscriber::registry()
        .with(layer.with_filter(filter))
        .try_init()
        .is_err()
    {
        return;
    }
    if let Ok(mut slot) = GUARD.lock() {
        *slot = Some(guard);
    }

    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        // Panics inside converters are caught by the scheduler (that file fails
        // alone); this line keeps the location for the bug report.
        let location = info
            .location()
            .map(|l| format!("{}:{}", l.file(), l.line()))
            .unwrap_or_default();
        let payload = info.payload();
        let message = payload
            .downcast_ref::<&str>()
            .map(|s| (*s).to_owned())
            .or_else(|| payload.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "unknown panic".into());
        tracing::error!(%location, "panic: {message}");
        // A main-thread panic ends the process without reaching `flush`.
        if std::thread::current().name() == Some("main") {
            flush();
        }
        previous(info);
    }));
    tracing::info!(version = env!("CARGO_PKG_VERSION"), "Morph started");
}

/// Write out buffered lines (waits up to a second). Call before the process
/// is terminated: `hard_exit` skips destructors.
pub fn flush() {
    let guard = GUARD.lock().ok().and_then(|mut slot| slot.take());
    drop(guard);
}

/// A non-fatal problem (state file not saved, toast failed, …).
pub fn error(context: &str, err: &dyn std::fmt::Display) {
    tracing::error!("{context}: {err:#}");
}

/// The app could not start or its event loop failed.
pub fn fatal(err: &anyhow::Error) {
    tracing::error!("fatal: {err:#}");
}

/// Today's local date as `YYYY-MM-DD`.
fn local_date() -> String {
    let t = local_now();
    format!("{:04}-{:02}-{:02}", t.wYear, t.wMonth, t.wDay)
}

fn local_now() -> windows::Win32::Foundation::SYSTEMTIME {
    // SAFETY: GetLocalTime only fills in the returned struct.
    unsafe { windows::Win32::System::SystemInformation::GetLocalTime() }
}

/// Line timestamps in local time: `2026-10-21 14:03:05.123`.
struct LocalTime;

impl FormatTime for LocalTime {
    fn format_time(&self, w: &mut Writer<'_>) -> std::fmt::Result {
        let t = local_now();
        write!(
            w,
            "{:04}-{:02}-{:02} {:02}:{:02}:{:02}.{:03}",
            t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond, t.wMilliseconds
        )
    }
}

fn file_name(date: &str) -> String {
    format!("{PREFIX}{date}{SUFFIX}")
}

/// `YYYY-MM-DD` (digits and dashes only).
fn is_date(s: &str) -> bool {
    s.len() == 10
        && s.char_indices().all(|(i, c)| match i {
            4 | 7 => c == '-',
            _ => c.is_ascii_digit(),
        })
}

/// The date of a `morph-YYYY-MM-DD.log` file name.
fn log_date(name: &str) -> Option<&str> {
    let date = name.strip_prefix(PREFIX)?.strip_suffix(SUFFIX)?;
    is_date(date).then_some(date)
}

/// Delete all but the `keep` newest `morph-YYYY-MM-DD.log` files in `dir`.
fn prune(dir: &Path, keep: usize) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut logs: Vec<String> = entries
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .filter(|n| log_date(n).is_some())
        .collect();
    // Same prefix and fixed-width dates: name order is date order.
    logs.sort_unstable_by(|a, b| b.cmp(a));
    for name in logs.into_iter().skip(keep) {
        let _ = std::fs::remove_file(dir.join(name));
    }
}

/// Logs from earlier builds: `morph-app.log` and `morph.YYYY-MM-DD.log`.
fn remove_legacy(dir: &Path) {
    let _ = std::fs::remove_file(dir.join("morph-app.log"));
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for name in entries.filter_map(|e| e.ok()?.file_name().into_string().ok()) {
        let old = name
            .strip_prefix("morph.")
            .and_then(|n| n.strip_suffix(SUFFIX))
            .is_some_and(is_date);
        if old {
            let _ = std::fs::remove_file(dir.join(name));
        }
    }
}

/// Appends to `morph-<today>.log`, switching file (and pruning) when the
/// local date changes. Write errors are swallowed; the next line retries.
struct DailyFile {
    dir: PathBuf,
    today: fn() -> String,
    date: String,
    file: Option<File>,
}

impl DailyFile {
    fn new(dir: PathBuf, today: fn() -> String) -> DailyFile {
        DailyFile {
            dir,
            today,
            date: String::new(),
            file: None,
        }
    }
}

impl Write for DailyFile {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let today = (self.today)();
        if self.file.is_none() || today != self.date {
            self.file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(self.dir.join(file_name(&today)))
                .ok();
            self.date = today;
            prune(&self.dir, KEEP_FILES);
        }
        if let Some(file) = &mut self.file {
            // One write per line, so a line never straddles two files.
            let _ = file.write_all(buf);
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        if let Some(file) = &mut self.file {
            let _ = file.flush();
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn names(dir: &Path) -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        v.sort();
        v
    }

    #[test]
    fn naming() {
        assert_eq!(file_name("2026-10-21"), "morph-2026-10-21.log");
        assert_eq!(log_date("morph-2026-10-21.log"), Some("2026-10-21"));
        for other in [
            "morph-app.log",
            "morph.2026-10-21.log",
            "shell.log",
            "morph-2026-10-21.log.bak",
            "morph-2026-1-021.log",
        ] {
            assert_eq!(log_date(other), None, "{other}");
        }
        let d = local_date();
        assert!(is_date(&d), "{d}");
    }

    #[test]
    fn retention_keeps_newest_and_removes_legacy() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        for day in 1..=9 {
            std::fs::write(dir.join(format!("morph-2026-09-{day:02}.log")), "x").unwrap();
        }
        for other in [
            "shell.log",
            "shell.old.log",
            "morph-app.log",
            "morph.2026-09-30.log",
        ] {
            std::fs::write(dir.join(other), "x").unwrap();
        }
        remove_legacy(dir);
        prune(dir, KEEP_FILES);
        let mut expected: Vec<String> = (3..=9)
            .map(|day| format!("morph-2026-09-{day:02}.log"))
            .collect();
        expected.extend(["shell.log".to_owned(), "shell.old.log".to_owned()]);
        expected.sort();
        assert_eq!(names(dir), expected);
    }

    static DAY: AtomicUsize = AtomicUsize::new(1);

    fn fake_today() -> String {
        format!("2026-10-{:02}", DAY.load(Ordering::SeqCst))
    }

    #[test]
    fn rotates_when_the_date_changes() {
        let tmp = tempfile::tempdir().unwrap();
        let mut w = DailyFile::new(tmp.path().to_path_buf(), fake_today);
        for day in 1..=9 {
            DAY.store(day, Ordering::SeqCst);
            w.write_all(b"a\n").unwrap();
            w.write_all(b"b\n").unwrap();
        }
        let files = names(tmp.path());
        assert_eq!(files.len(), KEEP_FILES, "{files:?}");
        assert_eq!(
            files.first().map(String::as_str),
            Some("morph-2026-10-03.log")
        );
        let last = std::fs::read_to_string(tmp.path().join("morph-2026-10-09.log")).unwrap();
        assert_eq!(last, "a\nb\n");
    }
}
