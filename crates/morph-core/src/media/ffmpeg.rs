// SPDX-License-Identifier: GPL-3.0-or-later
//! Running the bundled `ffmpeg.exe` / `ffprobe.exe` (SPEC §7, §10).
//!
//! Where the two exes are looked for, in order (both must be in the same folder):
//! 1. `%MORPH_FFMPEG_DIR%` (dev/testing override);
//! 2. next to the running exe (installed package, `layout\`);
//! 3. debug builds only: `vendor\ffmpeg\` in the nearest ancestor folder of the
//!    exe that has a `Cargo.toml` (dev builds and `cargo test`; run
//!    `scripts\fetch-deps.ps1`).
//!
//! [`run`] starts ffmpeg with `CREATE_NO_WINDOW`, suspended, puts it in a
//! kill-on-close job object (so it dies with Morph), resumes it, reads
//! `-progress pipe:1` from stdout (`out_time_us` → 0–1) and keeps the last 30
//! lines of stderr for "Details". Cancel kills it at once.
//!
//! Paths: pass every file through [`path_arg`] (`file:` prefix, so a name that
//! starts with `-` or looks like a protocol is still a file). Outputs: write to an
//! [`crate::naming::OutputFile`] temp path, which has no media extension, so
//! always give the muxer with `-f` ([`muxer`]). `run` adds `-y` (the temp file
//! already exists).

use crate::cancel::CancelToken;
use crate::converter::TaskCtx;
use crate::error::{ConvertError, ErrorKind};
use crate::office::win::{self, KillJob};
use std::collections::VecDeque;
use std::ffi::{OsStr, OsString};
use std::io::{BufRead, BufReader, Read};
use std::os::windows::io::AsRawHandle;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use windows::Win32::System::Threading::{CREATE_NO_WINDOW, CREATE_SUSPENDED};

/// Environment variable: folder with `ffmpeg.exe` and `ffprobe.exe` (overrides the bundled ones).
pub const ENV_DIR: &str = "MORPH_FFMPEG_DIR";
/// Lines of stderr kept for "Details" (SPEC §10: last 30 lines).
const LOG_LINES: usize = 30;
/// How often we look at the process and the cancel button.
const POLL: Duration = Duration::from_millis(50);
/// Most stdout [`capture`] keeps (ffprobe JSON is a few KB).
const MAX_CAPTURE: u64 = 16 * 1024 * 1024;
/// Time limit for small helper runs (`ffprobe`, `-version`).
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(60);
/// A [`run`] whose progress doesn't move for this long is stuck (GPU driver
/// deadlock, a demuxer waiting forever): killed.
pub const STALL_TIMEOUT: Duration = Duration::from_secs(120);
/// Message for a stuck run.
pub const STALLED_MESSAGE: &str = "FFmpeg stopped responding.";

/// The two executables, found together.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tools {
    pub ffmpeg: PathBuf,
    pub ffprobe: PathBuf,
}

/// Folders searched, in order (see the module docs).
pub fn candidate_dirs() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(dir) = std::env::var_os(ENV_DIR).filter(|d| !d.is_empty()) {
        out.push(PathBuf::from(dir));
    }
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf));
    if let Some(exe_dir) = exe_dir {
        out.push(exe_dir.clone());
        // Dev fallback only: a release build never runs an exe from outside its folder.
        #[cfg(debug_assertions)]
        if let Some(repo) = exe_dir
            .ancestors()
            .find(|dir| dir.join("Cargo.toml").is_file())
        {
            out.push(repo.join("vendor").join("ffmpeg"));
        }
    }
    out
}

/// Look for the exes now (not cached).
pub fn find() -> Option<Tools> {
    candidate_dirs().into_iter().find_map(|dir| {
        let tools = Tools {
            ffmpeg: dir.join("ffmpeg.exe"),
            ffprobe: dir.join("ffprobe.exe"),
        };
        (tools.ffmpeg.is_file() && tools.ffprobe.is_file()).then_some(tools)
    })
}

static TOOLS: OnceLock<Option<Tools>> = OnceLock::new();

/// The exes (looked up once per process), or the "FFmpeg is missing" error.
pub fn tools() -> Result<&'static Tools, ConvertError> {
    TOOLS.get_or_init(find).as_ref().ok_or_else(missing_ffmpeg)
}

/// FFmpeg isn't installed next to Morph.
pub fn missing_ffmpeg() -> ConvertError {
    let tried: Vec<String> = candidate_dirs()
        .iter()
        .map(|d| d.display().to_string())
        .collect();
    ConvertError::other(
        "The video engine (FFmpeg) is missing. Reinstall the app to fix this.",
        format!(
            "ffmpeg.exe + ffprobe.exe not found in: {}",
            tried.join("; ")
        ),
    )
}

static VERSION: OnceLock<Option<String>> = OnceLock::new();

/// FFmpeg's version string (e.g. `n9.0.2-morph-1`), once per
/// process. Part of the GPU detection cache key.
pub fn version() -> Option<&'static str> {
    VERSION
        .get_or_init(|| {
            let tools = tools().ok()?;
            let out = capture(
                &tools.ffmpeg,
                &[OsString::from("-hide_banner"), OsString::from("-version")],
                &CancelToken::new(),
                PROBE_TIMEOUT,
            )
            .ok()?;
            parse_version(&String::from_utf8_lossy(&out.stdout))
        })
        .as_deref()
}

/// `ffmpeg version 9.0.2-full_build-www.gyan.dev Copyright …` → `9.0.2-full_build-www.gyan.dev`.
pub fn parse_version(text: &str) -> Option<String> {
    let rest = text.lines().next()?.strip_prefix("ffmpeg version ")?;
    rest.split_whitespace().next().map(str::to_owned)
}

/// A file path as an FFmpeg argument: `file:<path>`.
pub fn path_arg(path: &Path) -> OsString {
    let mut arg = OsString::from("file:");
    arg.push(path.as_os_str());
    arg
}

/// The `-f` muxer for an output extension (no dot), `None` if we don't write it.
pub fn muxer(ext: &str) -> Option<&'static str> {
    Some(match ext.to_ascii_lowercase().as_str() {
        "mp4" | "m4v" => "mp4",
        "m4a" => "ipod",
        "mov" => "mov",
        "mkv" => "matroska",
        "webm" => "webm",
        "avi" => "avi",
        "mp3" => "mp3",
        "wav" => "wav",
        "flac" => "flac",
        "ogg" => "ogg",
        "opus" => "opus",
        "gif" => "gif",
        _ => return None,
    })
}

/// What a finished [`run`] printed.
#[derive(Debug, Clone, Default)]
pub struct RunOutput {
    /// The last stderr lines (with `-loglevel error`, only problems).
    pub log: String,
}

/// Run `ffmpeg` with `args` (after `-hide_banner -nostdin -y -nostats
/// -loglevel error -progress pipe:1`; a later `-loglevel info` in `args` wins).
///
/// `duration_s`: seconds of media this run produces (the clip, or the `-t`
/// length), for `on_progress(0.0–1.0)`; `None` = no progress calls. The working
/// folder is the task's temp folder (two-pass logs land there unless
/// `-passlogfile` says otherwise). Cancel kills ffmpeg and returns `Cancelled`.
/// A non-zero exit → [`failed`] with the stderr tail. No progress for
/// [`STALL_TIMEOUT`] → killed, [`STALLED_MESSAGE`].
pub fn run(
    args: &[OsString],
    duration_s: Option<f64>,
    ctx: &TaskCtx<'_>,
    on_progress: &mut dyn FnMut(f32),
) -> Result<RunOutput, ConvertError> {
    ctx.check_cancel()?;
    let tools = tools()?;
    let mut full: Vec<OsString> = [
        "-hide_banner",
        "-nostdin",
        "-y",
        "-nostats",
        "-loglevel",
        "error",
        "-progress",
        "pipe:1",
    ]
    .iter()
    .map(OsString::from)
    .collect();
    full.extend(args.iter().cloned());
    let work = ctx.temp_dir()?;
    let mut progress = |line: &str| {
        if let Some(f) = duration_s.and_then(|d| parse_progress(line, d)) {
            on_progress(f);
        }
    };
    let out = exec(
        &tools.ffmpeg,
        &full,
        Some(work),
        ctx.cancel,
        Limit::Stall(STALL_TIMEOUT),
        StdoutMode::Lines(&mut progress),
    )?;
    if out.success {
        Ok(RunOutput { log: out.log })
    } else {
        Err(failed(&out.log))
    }
}

/// Output of [`capture`].
#[derive(Debug, Clone, Default)]
pub struct Captured {
    pub success: bool,
    pub stdout: Vec<u8>,
    /// Last stderr lines.
    pub log: String,
}

/// Run `exe` (ffmpeg or ffprobe) with exactly `args`, collecting stdout. Same
/// window/job/cancel handling as [`run`]; killed after `timeout`. A non-zero
/// exit is not an error here: look at `success`.
pub fn capture(
    exe: &Path,
    args: &[OsString],
    cancel: &CancelToken,
    timeout: Duration,
) -> Result<Captured, ConvertError> {
    let mut stdout = Vec::new();
    let out = exec(
        exe,
        args,
        None,
        cancel,
        Limit::Total(timeout),
        StdoutMode::Collect(&mut stdout),
    )?;
    Ok(Captured {
        success: out.success,
        stdout,
        log: out.log,
    })
}

/// `out_time_us=1500000` with a 3 s clip → `Some(0.5)`; `progress=end` → `Some(1.0)`.
pub fn parse_progress(line: &str, duration_s: f64) -> Option<f32> {
    let (key, value) = line.trim().split_once('=')?;
    match key {
        // (`out_time_ms` is also microseconds; one key is enough.)
        "out_time_us" => {
            let us: i64 = value.trim().parse().ok()?;
            if us < 0 || duration_s <= 0.0 {
                return None;
            }
            Some(((us as f64 / 1e6) / duration_s).clamp(0.0, 1.0) as f32)
        }
        "progress" if value.trim() == "end" => Some(1.0),
        _ => None,
    }
}

/// Plain-English error for a failed ffmpeg run, from its stderr tail.
/// Message for a full disk (also checked by the GPU → CPU retry rule).
pub const DISK_FULL: &str = "The disk is full. Free up some space and try again.";

pub fn failed(log: &str) -> ConvertError {
    let lower = log.to_ascii_lowercase();
    let has = |needles: &[&str]| needles.iter().any(|n| lower.contains(n));
    if has(&["no such file or directory"]) {
        ConvertError::new(ErrorKind::BadInput, "The file was moved or deleted.", log)
    } else if has(&["permission denied"]) {
        ConvertError::not_writable(log)
    } else if has(&["no space left on device"]) {
        ConvertError::other(DISK_FULL, log)
    } else if has(&[
        "invalid data found when processing input",
        "moov atom not found",
        "could not find codec parameters",
        "ebml header parsing failed",
        "error while decoding",
        "invalid frame dimensions",
        "end of file",
    ]) {
        super::damaged_media(log)
    } else if has(&[
        "nvenc",
        "nvcuda.dll",
        "cuda",
        "amf",
        "qsv",
        "mfx",
        "vpl",
        "hardware",
    ]) && has(&["error", "failed", "cannot", "no capable", "not supported"])
    {
        ConvertError::other("The graphics card couldn't encode this video.", log)
    } else if has(&["unknown encoder", "encoder not found"]) {
        ConvertError::other("This encoder isn't available in the included FFmpeg.", log)
    } else if has(&[
        "could not write header",
        "incorrect codec parameters",
        "not currently supported in container",
        "codec not currently supported",
        "could not find tag for codec",
    ]) {
        ConvertError::unsupported("This combination of formats isn't supported.", log)
    } else {
        ConvertError::other("FFmpeg couldn't convert this file.", log)
    }
}

// ---------------------------------------------------------------- process

enum StdoutMode<'a> {
    /// Every stdout line goes to the callback (on the calling thread).
    Lines(&'a mut dyn FnMut(&str)),
    /// Keep stdout (up to [`MAX_CAPTURE`]).
    Collect(&'a mut Vec<u8>),
}

struct Exec {
    success: bool,
    log: String,
}

/// When [`exec`] gives up on a child.
#[derive(Debug, Clone, Copy)]
enum Limit {
    /// The whole run.
    Total(Duration),
    /// Time since the `-progress` output last moved ([`Stall`]).
    Stall(Duration),
}

/// Notices when ffmpeg's `-progress` output stops moving. ffmpeg keeps
/// printing progress blocks while stuck, so only a changed `out_time_us`,
/// `frame` or `total_size` counts as progress.
struct Stall {
    last_move: Instant,
    seen: [Option<String>; 3],
}

impl Stall {
    const KEYS: [&'static str; 3] = ["out_time_us", "frame", "total_size"];

    fn new(now: Instant) -> Stall {
        Stall {
            last_move: now,
            seen: Default::default(),
        }
    }

    /// Look at one stdout line.
    fn line(&mut self, line: &str, now: Instant) {
        let Some((key, value)) = line.trim().split_once('=') else {
            return;
        };
        let Some(i) = Self::KEYS.iter().position(|k| *k == key) else {
            return;
        };
        let value = value.trim();
        if value != "N/A" && self.seen[i].as_deref() != Some(value) {
            self.seen[i] = Some(value.to_owned());
            self.last_move = now;
        }
    }

    fn stalled(&self, now: Instant, limit: Duration) -> bool {
        now.duration_since(self.last_move) >= limit
    }
}

fn exec(
    exe: &Path,
    args: &[OsString],
    cwd: Option<&Path>,
    cancel: &CancelToken,
    limit: Limit,
    mut stdout_mode: StdoutMode<'_>,
) -> Result<Exec, ConvertError> {
    cancel.check()?;
    let name = exe
        .file_name()
        .map_or_else(|| "ffmpeg".into(), OsStr::to_string_lossy)
        .into_owned();
    let mut cmd = Command::new(exe);
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .creation_flags(CREATE_NO_WINDOW.0 | CREATE_SUSPENDED.0);
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    let mut child = cmd.spawn().map_err(|e| {
        ConvertError::other(
            "The video engine (FFmpeg) couldn't be started.",
            format!("{}: {e}", exe.display()),
        )
    })?;
    // Kill-on-close: ffmpeg dies with us even if Morph crashes.
    let job = KillJob::new();
    if let Some(j) = &job {
        j.assign(child.as_raw_handle());
    }
    win::resume_process(child.id());

    let log = Arc::new(Mutex::new(VecDeque::new()));
    let stderr_reader = child.stderr.take().map(|s| {
        let log = Arc::clone(&log);
        std::thread::spawn(move || collect_lines(s, &log))
    });
    let (line_tx, line_rx) = mpsc::channel::<String>();
    let stdout_reader = child.stdout.take().map(|s| {
        let collect = matches!(stdout_mode, StdoutMode::Collect(_));
        std::thread::spawn(move || {
            if collect {
                let mut buf = Vec::new();
                let _ = s.take(MAX_CAPTURE).read_to_end(&mut buf);
                buf
            } else {
                let mut reader = BufReader::new(s);
                let mut line = String::new();
                while matches!(reader.read_line(&mut line), Ok(n) if n > 0) {
                    if line_tx.send(line.trim_end().to_owned()).is_err() {
                        break;
                    }
                    line.clear();
                }
                Vec::new()
            }
        })
    });

    let kill = |child: &mut std::process::Child| {
        let _ = child.kill();
        if let Some(j) = &job {
            j.terminate();
        }
        let _ = child.wait();
    };
    let started = Instant::now();
    let mut stall = Stall::new(started);
    let drain = |mode: &mut StdoutMode<'_>, stall: &mut Stall| {
        if let StdoutMode::Lines(f) = mode {
            for line in line_rx.try_iter() {
                stall.line(&line, Instant::now());
                f(&line);
            }
        }
    };
    let status = loop {
        drain(&mut stdout_mode, &mut stall);
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {}
            Err(e) => {
                kill(&mut child);
                return Err(ConvertError::other(
                    "The video engine (FFmpeg) stopped unexpectedly.",
                    format!("{name}: {e}"),
                ));
            }
        }
        if cancel.is_cancelled() {
            kill(&mut child);
            return Err(ConvertError::cancelled());
        }
        let now = Instant::now();
        match limit {
            Limit::Total(t) if now.duration_since(started) >= t => {
                kill(&mut child);
                return Err(ConvertError::other(
                    "The video engine (FFmpeg) took too long and was stopped.",
                    format!("{name}: timeout\n{}", tail(&log)),
                ));
            }
            Limit::Stall(t) if stall.stalled(now, t) => {
                kill(&mut child);
                return Err(ConvertError::other(
                    STALLED_MESSAGE,
                    format!("{name}: no progress for {} s\n{}", t.as_secs(), tail(&log)),
                ));
            }
            _ => {}
        }
        std::thread::sleep(POLL);
    };
    let collected = stdout_reader
        .and_then(|r| r.join().ok())
        .unwrap_or_default();
    drain(&mut stdout_mode, &mut stall);
    if let StdoutMode::Collect(buf) = stdout_mode {
        *buf = collected;
    }
    if let Some(r) = stderr_reader {
        let _ = r.join();
    }
    Ok(Exec {
        success: status.success(),
        log: tail(&log),
    })
}

fn collect_lines(stream: impl Read, log: &Mutex<VecDeque<String>>) {
    let mut reader = BufReader::new(stream);
    let mut buf = Vec::new();
    loop {
        buf.clear();
        match reader.read_until(b'\n', &mut buf) {
            Ok(0) | Err(_) => break,
            Ok(_) => {
                let line = String::from_utf8_lossy(&buf).trim_end().to_owned();
                if line.is_empty() {
                    continue;
                }
                if let Ok(mut l) = log.lock() {
                    if l.len() == LOG_LINES {
                        l.pop_front();
                    }
                    l.push_back(line);
                }
            }
        }
    }
}

fn tail(log: &Mutex<VecDeque<String>>) -> String {
    log.lock()
        .map(|l| l.iter().cloned().collect::<Vec<_>>().join("\n"))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_lines() {
        assert_eq!(parse_progress("out_time_us=1500000", 3.0), Some(0.5));
        assert_eq!(parse_progress("out_time_us=9000000", 3.0), Some(1.0));
        assert_eq!(parse_progress("out_time_us=N/A", 3.0), None);
        assert_eq!(
            parse_progress("out_time_us=-9223372036854775807", 3.0),
            None
        );
        assert_eq!(parse_progress("out_time_ms=1500000", 3.0), None);
        assert_eq!(parse_progress("progress=continue", 3.0), None);
        assert_eq!(parse_progress("progress=end", 3.0), Some(1.0));
        assert_eq!(parse_progress("frame=12", 3.0), None);
        assert_eq!(parse_progress("out_time_us=100", 0.0), None);
    }

    #[test]
    fn stall_watch() {
        let t0 = Instant::now();
        let s = |secs| t0 + Duration::from_secs(secs);
        let limit = Duration::from_secs(120);
        let mut w = Stall::new(t0);
        w.line("out_time_us=1000", s(10));
        w.line("progress=continue", s(100));
        assert!(!w.stalled(s(129), limit));
        // The same values again (ffmpeg stuck, still printing): no progress.
        w.line("out_time_us=1000", s(100));
        w.line("out_time_us=N/A", s(100));
        assert!(w.stalled(s(130), limit));
        w.line("frame=3", s(130));
        assert!(!w.stalled(s(131), limit));
    }

    #[test]
    fn stuck_child_is_killed() {
        // A child that prints no progress (ping waits ~1 s per echo).
        let ping = PathBuf::from(std::env::var_os("SystemRoot").unwrap())
            .join("System32")
            .join("PING.EXE");
        let args = ["-n", "30", "127.0.0.1"].map(OsString::from);
        let started = Instant::now();
        let mut lines = |_: &str| {};
        let err = exec(
            &ping,
            &args,
            None,
            &CancelToken::new(),
            Limit::Stall(Duration::from_secs(1)),
            StdoutMode::Lines(&mut lines),
        )
        .err()
        .unwrap();
        assert_eq!(err.message, STALLED_MESSAGE);
        assert!(started.elapsed() < Duration::from_secs(10));
    }

    #[test]
    fn version_line() {
        assert_eq!(
            parse_version(
                "ffmpeg version 9.0.2-full_build-www.gyan.dev Copyright (c) 2000-2026\nbuilt with gcc"
            )
            .as_deref(),
            Some("9.0.2-full_build-www.gyan.dev")
        );
        assert_eq!(parse_version("nope"), None);
    }

    #[test]
    fn error_mapping() {
        let damaged = failed("[mov,mp4] moov atom not found\nfile:a.mp4: Invalid data found");
        assert_eq!(damaged.kind, ErrorKind::BadInput);
        let gpu = failed("[h264_nvenc] OpenEncodeSessionEx failed: no capable devices found");
        assert!(gpu.message.contains("graphics card"), "{}", gpu.message);
        assert_eq!(
            failed("Could not write header (incorrect codec parameters ?)").kind,
            ErrorKind::Unsupported
        );
        let other = failed("something odd");
        assert_eq!(other.message, "FFmpeg couldn't convert this file.");
        assert_eq!(other.details, "something odd");
    }

    #[test]
    fn args_and_muxers() {
        assert_eq!(
            path_arg(Path::new(r"C:\a\-b.mp4")),
            OsString::from(r"file:C:\a\-b.mp4")
        );
        assert_eq!(muxer("MKV"), Some("matroska"));
        assert_eq!(muxer("m4a"), Some("ipod"));
        assert_eq!(muxer("exe"), None);
    }
}
