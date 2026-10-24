// SPDX-License-Identifier: GPL-3.0-or-later
//! Every user-facing string of the CLI lives here.

pub const ABOUT: &str = "Convert files from the command line with the same engine as the app.";
pub const ABOUT_CONVERT: &str = "Run an action on one or more files.";
pub const ABOUT_ACTIONS: &str = "List the action IDs with their default options as JSON.";
pub const ABOUT_OPTIONS: &str = "Print the default options JSON of one action.";
pub const ABOUT_ENGINES: &str =
    "Show which Office engines (Microsoft Office, LibreOffice) are installed, as JSON.";

pub const ABOUT_FFMPEG: &str =
    "Show where the video engine (FFmpeg) was found, its version and the GPU encoders, as JSON.";

pub const HELP_FILES: &str = "Input files.";
pub const HELP_ACTION: &str = "Action ID, e.g. image.convert (see `morph-cli actions`).";
pub const HELP_FORMAT: &str = "Shortcut for --set format=<fmt>.";
pub const HELP_QUALITY: &str = "Shortcut for --set quality=<1-100>.";
pub const HELP_SET: &str = "Set one option (repeatable). The value is JSON when valid, else a string. \
                            Use dotted keys for nested objects; this is safe in Windows PowerShell. \
                            Example: --set mode.type=target_size --set mode.bytes=500000. \
                            image.compress modes: best (default: smallest file that still looks \
                            the same), quality (with mode.quality=1-100), target_size. \
                            Video and audio: --action video.compress --set preset=discord \
                            (best, balanced, smaller, discord, email, custom with target_mb=8; \
                            resolution=720p; encoder=cpu), video.convert --format webm \
                            (fast=false re-encodes), video.to_gif --set start_s=90 --set \
                            length_s=5 --set width=320, video.extract_audio / audio.convert \
                            --format m4a --set bitrate_kbps=256 (mp3_mode=v0, channels=mono, sample_rate=48000). \
                            Whole JSON values lose their quotes in PowerShell 5.1; use dotted keys \
                            or --options with a here-string instead.";
pub const HELP_TARGET_SIZE: &str =
    "Shortcut for mode target_size, e.g. --target-size 500KB, 2MB, 1.5MB or 300000 (bytes).";
pub const HELP_WIDTH: &str = "Shortcut for mode max_width (with --height: exact size).";
pub const HELP_HEIGHT: &str = "Shortcut for mode max_height (with --width: exact size).";
pub const HELP_PERCENT: &str = "Shortcut for mode percent, e.g. --percent 50.";
pub const HELP_OPTIONS: &str = "Options as a JSON object, merged before --format/--quality/--set.";
pub const HELP_OUT: &str = "Output folder (default: next to each input).";
pub const HELP_THREADS: &str = "Number of files processed in parallel.";
pub const HELP_JSON: &str = "Print one JSON object per event, one per line.";
pub const HELP_ENGINE: &str = "Office engine: auto (Microsoft Office, else LibreOffice), microsoft_office or libreoffice. Default: the app's setting.";
pub const HELP_ACTION_ID: &str = "Action ID.";

pub fn err_prefix() -> &'static str {
    "error: "
}

pub fn bad_set(arg: &str) -> String {
    format!("invalid --set '{arg}': expected key=value")
}

pub fn bad_size(text: &str) -> String {
    format!("invalid size '{text}': use a number of bytes or a size like 500KB, 2MB, 1.5MB")
}

pub fn bad_percent(text: &str) -> String {
    format!("invalid percent '{text}': expected a number above 0")
}

pub fn cancelling() -> &'static str {
    "Cancelling... press Ctrl+C again to stop immediately."
}

/// Used when a task made no file and gave no note.
pub const NO_CHANGE_NOTE: &str = "Nothing to change, no file made";

/// A task that finished without writing anything (plain ASCII on purpose).
pub fn no_output(name: &str, note: &str) -> String {
    format!("= {name}: {note}")
}

pub fn shortcut_not_applicable(flag: &str, action: &str, applies_to: &str) -> String {
    format!("{flag} does not apply to {action}; it only works with {applies_to}")
}

/// Status symbols: Unicode on a console, ASCII when output is redirected.
#[derive(Debug, Clone, Copy)]
pub struct Symbols {
    pub ok: &'static str,
    pub arrow: &'static str,
    pub minus: &'static str,
    pub fail: &'static str,
    pub skip: &'static str,
}

pub const UNICODE: Symbols = Symbols {
    ok: "✓",
    arrow: "→",
    minus: "−",
    fail: "✕",
    skip: "·",
};

pub const ASCII: Symbols = Symbols {
    ok: "OK",
    arrow: "->",
    minus: "-",
    fail: "FAIL",
    skip: "SKIP",
};

pub fn bad_options_json(err: &str) -> String {
    format!("invalid --options JSON: {err}")
}

pub fn options_not_object() -> String {
    "invalid --options: expected a JSON object".to_owned()
}

pub fn unknown_action(id: &str, known: &[&str]) -> String {
    format!("unknown action '{id}'. Known actions: {}", known.join(", "))
}

pub fn task_skipped(name: &str, reason: &str, sym: &Symbols) -> String {
    format!("{} {name}  skipped: {reason}", sym.skip)
}

/// A running task's status from the core, e.g. "Checking the graphics card…".
pub fn task_status(name: &str, text: &str) -> String {
    format!("  {name}  {text}")
}

pub fn task_failed(name: &str, error: &str, sym: &Symbols) -> String {
    format!("{} {name}  {error}", sym.fail)
}

pub fn task_done(input: &str, output: &str, sizes: &str, change: &str, sym: &Symbols) -> String {
    format!(
        "{} {input} {} {output}  {sizes}  {change}",
        sym.ok, sym.arrow
    )
}

pub fn more_outputs(count: usize) -> String {
    format!("(+{count} more)")
}

pub fn sizes(bytes_in: &str, bytes_out: &str, sym: &Symbols) -> String {
    format!("{bytes_in} {} {bytes_out}", sym.arrow)
}

pub fn summary_line(
    done: usize,
    failed: usize,
    skipped: usize,
    cancelled: usize,
    bytes_in: &str,
    bytes_out: &str,
    sym: &Symbols,
) -> String {
    let mut line = format!("{done} done, {failed} failed, {skipped} skipped");
    if cancelled > 0 {
        line.push_str(&format!(", {cancelled} cancelled"));
    }
    line.push_str(&format!(". {bytes_in} {} {bytes_out}", sym.arrow));
    line
}

pub fn no_summary() -> &'static str {
    "The job ended without a summary."
}

pub fn ctrlc_handler_failed(err: &str) -> String {
    format!("warning: Ctrl+C will not cancel the job ({err})")
}
