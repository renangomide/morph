// SPDX-License-Identifier: GPL-3.0-or-later
//! The three sub-commands.

use crate::cli::ConvertArgs;
use crate::output::{self, Names};
use crate::strings;
use morph_core::scheduler::{self, JobConfig};
use morph_core::{Action, Event, Job, OutputSpec};
use morph_formats::actions;
use serde_json::{Map, Value};
use std::process::ExitCode;
use std::sync::atomic::{AtomicUsize, Ordering};

const EXIT_FAILED: u8 = 1;
const EXIT_USAGE: u8 = 2;
const EXIT_CANCELLED: u8 = 130;

/// Every action ID the CLI could know about; only those the core implements are listed.
const CANDIDATE_IDS: &[&str] = &[
    actions::IMAGE_CONVERT,
    actions::IMAGE_COMPRESS,
    actions::IMAGE_RESIZE,
    actions::IMAGE_CLEAN,
    actions::IMAGE_TO_PDF,
    actions::PDF_COMPRESS,
    actions::PDF_TO_IMAGES,
    actions::PDF_TO_WORD,
    actions::PDF_TO_TEXT,
    actions::PDF_TO_MARKDOWN,
    actions::PDF_SPLIT,
    actions::PDF_MERGE,
    actions::VIDEO_CONVERT,
    actions::VIDEO_COMPRESS,
    actions::VIDEO_EXTRACT_AUDIO,
    actions::VIDEO_TO_GIF,
    actions::AUDIO_CONVERT,
    actions::OFFICE_CONVERT,
    actions::OFFICE_TO_IMAGES,
];

fn known_ids() -> Vec<&'static str> {
    CANDIDATE_IDS
        .iter()
        .copied()
        .filter(|id| Action::default_for(id).is_some())
        .collect()
}

fn usage_error(message: &str) -> ExitCode {
    output::emit_err(&format!("{}{message}", strings::err_prefix()));
    ExitCode::from(EXIT_USAGE)
}

pub fn list_actions() -> ExitCode {
    let mut all = Map::new();
    for id in known_ids() {
        if let Some(action) = Action::default_for(id) {
            all.insert(id.to_owned(), Value::Object(action.options_json()));
        }
    }
    match serde_json::to_string_pretty(&Value::Object(all)) {
        Ok(text) => {
            output::emit(&text);
            ExitCode::SUCCESS
        }
        Err(e) => usage_error(&e.to_string()),
    }
}

pub fn show_options(id: &str) -> ExitCode {
    let Some(action) = Action::default_for(id) else {
        return usage_error(&strings::unknown_action(id, &known_ids()));
    };
    match serde_json::to_string_pretty(&Value::Object(action.options_json())) {
        Ok(text) => {
            output::emit(&text);
            ExitCode::SUCCESS
        }
        Err(e) => usage_error(&e.to_string()),
    }
}

/// Installed Office engines and which one each family uses, as JSON.
pub fn show_engines() -> ExitCode {
    use morph_core::office::{Family, detect};
    let engines = detect::refresh();
    let preference = configured_engine();
    let app = |info: Option<&detect::AppInfo>| match info {
        Some(i) => serde_json::json!({ "exe": i.exe, "version": i.version }),
        None => Value::Null,
    };
    let mut uses = Map::new();
    for (name, family) in [
        ("word", Family::Word),
        ("excel", Family::Excel),
        ("powerpoint", Family::PowerPoint),
        ("text", Family::Text),
    ] {
        let engine = engines
            .resolve(family, preference)
            .map_or(Value::Null, |c| Value::from(c.display_name()));
        uses.insert(name.to_owned(), engine);
    }
    let report = serde_json::json!({
        "preference": preference,
        "microsoft_word": app(engines.word.as_ref()),
        "microsoft_excel": app(engines.excel.as_ref()),
        "microsoft_powerpoint": app(engines.powerpoint.as_ref()),
        "libreoffice": engines.libreoffice.as_ref().map(|lo| serde_json::json!({
            "soffice": lo.soffice, "version": lo.version,
        })),
        "uses": uses,
    });
    match serde_json::to_string_pretty(&report) {
        Ok(text) => {
            output::emit(&text);
            ExitCode::SUCCESS
        }
        Err(e) => usage_error(&e.to_string()),
    }
}

/// The video engine: where FFmpeg was found, its version and the cached GPU
/// encoders that passed their test encode, as JSON.
pub fn show_ffmpeg() -> ExitCode {
    use morph_core::media::ffmpeg;
    let tools = ffmpeg::find();
    let gpu = morph_formats::paths::app_data_dir()
        .and_then(|root| {
            morph_core::state::State::load(&morph_formats::paths::state_file(&root)).gpu
        })
        .map(|g| Value::from(g.working));
    let report = serde_json::json!({
        "ffmpeg": tools.as_ref().map(|t| &t.ffmpeg),
        "ffprobe": tools.as_ref().map(|t| &t.ffprobe),
        "version": tools.as_ref().and_then(|_| ffmpeg::version()),
        "searched": ffmpeg::candidate_dirs(),
        "gpu_encoders": gpu,
    });
    match serde_json::to_string_pretty(&report) {
        Ok(text) => {
            output::emit(&text);
            if tools.is_some() {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(EXIT_FAILED)
            }
        }
        Err(e) => usage_error(&e.to_string()),
    }
}

/// Settings → Office engine from the app's `config.toml` (Auto if unset).
fn configured_engine() -> morph_core::office::EnginePreference {
    morph_formats::paths::app_data_dir()
        .map(|root| {
            morph_core::office::EnginePreference::from_config_file(
                &morph_formats::paths::config_file(&root),
            )
        })
        .unwrap_or_default()
}

fn job_config(args: &ConvertArgs) -> JobConfig {
    let mut config = JobConfig::default();
    if let Some(n) = args.threads {
        config.cpu_threads = n.max(1);
    }
    if let Some(root) = morph_formats::paths::app_data_dir() {
        config.temp_root = morph_formats::paths::tmp_dir(&root);
    }
    config.office_engine = match args.engine {
        Some(engine) => engine.preference(),
        None => configured_engine(),
    };
    config
}

pub fn convert(args: &ConvertArgs) -> ExitCode {
    let Some(defaults) = Action::default_for(&args.action) else {
        return usage_error(&strings::unknown_action(&args.action, &known_ids()));
    };
    let mut overrides = match args.overrides() {
        Ok(map) => map,
        Err(e) => return usage_error(&e),
    };
    crate::cli::match_default_types(&mut overrides, &defaults.options_json());
    let action = match Action::from_id(&args.action, &overrides) {
        Ok(a) => a,
        Err(e) => return usage_error(&e),
    };

    if let Some(folder) = &args.out {
        // Best effort: the core reports a clear error per file if this fails.
        let _ = std::fs::create_dir_all(folder);
    }
    let spec = OutputSpec {
        folder: args.out.clone(),
    };
    let job = Job::for_files(&args.files, &action, &spec);

    let mut names = Names::new();
    for task in &job.tasks {
        if let Some(input) = task.input() {
            names.insert(task.id.0, input.to_path_buf());
        }
    }
    let base = job.tasks.len() as u64;
    for (i, (path, _)) in job.skipped.iter().enumerate() {
        names.insert(base + i as u64, path.clone());
    }

    let handle = scheduler::run_job(job, job_config(args));
    let cancel = handle.cancel.clone();
    let presses = AtomicUsize::new(0);
    let handler = move || {
        if presses.fetch_add(1, Ordering::SeqCst) == 0 {
            cancel.cancel();
            output::emit_err(strings::cancelling());
        } else {
            // Safe: outputs only appear after encoding finishes and temp files are hidden.
            std::process::exit(i32::from(EXIT_CANCELLED));
        }
    };
    if let Err(e) = ctrlc::set_handler(handler) {
        output::emit_err(&strings::ctrlc_handler_failed(&e.to_string()));
    }

    let sym = output::symbols();
    let mut summary = None;
    while let Ok(event) = handle.events.recv() {
        print_event(&event, &names, args.json, sym);
        if let Event::JobFinished { summary: s } = event {
            summary = Some(s);
            break;
        }
    }
    let was_cancelled = handle.cancel.is_cancelled();
    handle.join();

    match summary {
        Some(s) if s.cancelled > 0 || was_cancelled => ExitCode::from(EXIT_CANCELLED),
        Some(s) if s.failed == 0 => ExitCode::SUCCESS,
        Some(_) => ExitCode::from(EXIT_FAILED),
        None => {
            output::emit_err(strings::no_summary());
            ExitCode::from(EXIT_FAILED)
        }
    }
}

fn print_event(event: &Event, names: &Names, json: bool, sym: &strings::Symbols) {
    if json {
        output::emit(&output::json_value(event, names).to_string());
    } else {
        for line in output::human_lines(event, names, sym) {
            output::emit(&line);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_ids_are_implemented_actions() {
        let ids = known_ids();
        assert!(ids.contains(&actions::IMAGE_CONVERT));
        assert!(ids.iter().all(|id| Action::default_for(id).is_some()));
    }

    #[test]
    fn unknown_action_is_a_usage_error() {
        assert_eq!(
            format!("{:?}", show_options("nope")),
            format!("{:?}", ExitCode::from(EXIT_USAGE))
        );
    }
}
