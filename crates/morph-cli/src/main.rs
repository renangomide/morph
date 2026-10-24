// SPDX-License-Identifier: GPL-3.0-or-later
//! morph-cli: runs Morph conversions without the GUI. Mirrors the request/options model.
//!
//! Exit codes: 0 all tasks done or skipped, 1 a task failed, 130 cancelled (Ctrl+C),
//! 2 usage error or unknown action.

mod cli;
mod commands;
mod output;
mod strings;

use clap::Parser;
use cli::{Cli, Command};
use std::process::ExitCode;

fn main() -> ExitCode {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(e) => {
            let code = if e.use_stderr() { 2 } else { 0 };
            let _ = e.print();
            return ExitCode::from(code);
        }
    };
    match cli.command {
        Command::Convert(args) => commands::convert(&args),
        Command::Actions => commands::list_actions(),
        Command::Options { id } => commands::show_options(&id),
        Command::Engines => commands::show_engines(),
        Command::Ffmpeg => commands::show_ffmpeg(),
    }
}
