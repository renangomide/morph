// SPDX-License-Identifier: GPL-3.0-or-later
//! `Morph.exe`: the conversion window opened from the Explorer "Morph" menu,
//! its progress / results view, and the tray icon + toast of background jobs.

#![windows_subsystem = "windows"]
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

mod applog;
mod controller;
mod convert_text;
mod focus;
mod home;
mod launch;
mod media_options;
mod office_options;
mod options;
mod pdf_options;
mod platform;
mod progress;
mod progress_text;
mod results;
mod strings;
mod view;

use anyhow::Result;
use launch::Launch;

/// Code generated from `ui/*.slint` (it uses unwrap internally).
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod ui {
    slint::include_modules!();
}
use ui::*;

fn main() {
    applog::init();
    // Software renderer: the OpenGL one (femtovg) hung at process exit inside
    // the NVIDIA driver, leaving an invisible Morph.exe after every run. The
    // window is simple enough that software rendering is fast.
    if let Err(err) = slint::BackendSelector::new()
        .renderer_name("software".into())
        .select()
    {
        applog::error("could not select the software renderer", &err);
    }
    let code = match run() {
        Ok(()) => 0,
        Err(err) => {
            applog::fatal(&err);
            1
        }
    };
    tracing::info!(code, "Morph exiting");
    applog::flush();
    hard_exit(code);
}

/// End the process now. By this point every job has finished and every file
/// is closed; skipping DLL teardown avoids driver hangs at exit.
fn hard_exit(code: u32) -> ! {
    use windows::Win32::System::Threading::{GetCurrentProcess, TerminateProcess};
    // SAFETY: terminating our own process with a valid pseudo-handle.
    let _ = unsafe { TerminateProcess(GetCurrentProcess(), code) };
    std::process::exit(code as i32)
}

fn run() -> Result<()> {
    let launch = launch::parse(std::env::args_os().skip(1));
    if let Launch::OpenFolder(folder) = &launch {
        // Toast "Open folder" button: no window.
        if let Err(err) = platform::shell::open_folder(folder) {
            applog::error("could not open the folder", &err);
        }
        return Ok(());
    }
    controller::run(launch)
}
