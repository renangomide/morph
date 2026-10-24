// SPDX-License-Identifier: GPL-3.0-or-later
//! Runs a [`Job`] and reports [`Event`]s (SPEC §5.2, §10).
//!
//! - CPU tasks run on a rayon pool (`cpu_threads`, default cores − 1), started in
//!   input order. A task starts only when its memory estimate fits the budget
//!   ([`crate::memory`]), so a batch of huge images can't run out of RAM.
//! - Office queue ([`Queue::Office`]: anything that needs Microsoft Office or
//!   LibreOffice) runs one task at a time on its own thread, next to the CPU pool,
//!   so slow engine conversions never block image work (SPEC §5.2).
//! - FFmpeg queue ([`Queue::Ffmpeg`]: video and audio) runs `ffmpeg_jobs` tasks
//!   at a time (default 1, at most 2) on its own threads, next to the others.
//! - Each task fails on its own: errors and panics become `TaskFailed`, the batch goes on.
//! - After cancel, tasks that haven't started are reported as `TaskSkipped`.
//!
//! Task IDs: tasks keep their own IDs; skipped inputs (`Job::skipped`) are reported
//! with IDs `tasks.len()`, `tasks.len() + 1`, … in order.

use crate::action::{Action, Queue};
use crate::cancel::CancelToken;
use crate::converter::{Converter, TaskCtx, TaskOutput};
use crate::error::ConvertError;
use crate::event::{Event, Summary};
use crate::job::{Job, Task, TaskId};
use crate::memory::{self, Budget};
use crate::office::EnginePreference;
use crossbeam_channel::{Receiver, Sender, unbounded};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub struct JobConfig {
    /// Parallel CPU tasks.
    pub cpu_threads: usize,
    /// Parent of per-job temp folders (normally `<LocalData>\tmp`).
    pub temp_root: PathBuf,
    /// Memory budget for running tasks, in bytes. `None`: 60% of free RAM.
    pub memory_limit: Option<u64>,
    /// Settings → Office engine (`[office] engine` in `config.toml`).
    pub office_engine: EnginePreference,
    /// ffmpeg.exe children at once (SPEC §5.2: 1, configurable 1–2).
    pub ffmpeg_jobs: usize,
}

/// Most ffmpeg children at once (each already uses every core).
pub const MAX_FFMPEG_JOBS: usize = 2;

impl Default for JobConfig {
    fn default() -> Self {
        let cores = std::thread::available_parallelism().map_or(2, |n| n.get());
        let temp_root = morph_formats::paths::app_data_dir()
            .map(|root| morph_formats::paths::tmp_dir(&root))
            .unwrap_or_else(|| std::env::temp_dir().join("morph"));
        JobConfig {
            cpu_threads: cores.saturating_sub(1).max(1),
            temp_root,
            memory_limit: None,
            office_engine: EnginePreference::Auto,
            ffmpeg_jobs: 1,
        }
    }
}

/// Delete leftover `job-*` folders in `temp_root` older than `min_age`
/// (from jobs that were killed). Call once at startup (SPEC §10). Best effort.
pub fn sweep_stale_temp(temp_root: &Path, min_age: std::time::Duration) {
    let Ok(entries) = std::fs::read_dir(temp_root) else {
        return;
    };
    let now = std::time::SystemTime::now();
    for entry in entries.flatten() {
        let is_job_dir = entry.file_name().to_string_lossy().starts_with("job-")
            && entry.file_type().is_ok_and(|t| t.is_dir());
        let old_enough = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| now.duration_since(t).ok())
            .is_some_and(|age| age >= min_age);
        if is_job_dir && old_enough {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

/// The converter for an action.
pub fn converter_for(action: &Action) -> &'static dyn Converter {
    match action {
        Action::ImageConvert(_) => &crate::image::convert::ImageConvert,
        Action::ImageCompress(_) => &crate::image::compress::ImageCompress,
        Action::ImageResize(_) => &crate::image::resize::ImageResize,
        Action::ImageClean(_) => &crate::image::clean::ImageClean,
        Action::ImageToPdf(_) => &crate::image::to_pdf::ImageToPdf,
        Action::PdfCompress(_) => &crate::pdf::compress::PdfCompress,
        Action::PdfToImages(_) => &crate::pdf::to_images::PdfToImages,
        Action::PdfToWord(_) => &crate::pdf::to_word::PdfToWord,
        Action::PdfToText(_) => &crate::pdf::text::PdfToText,
        Action::PdfToMarkdown(_) => &crate::pdf::to_markdown::PdfToMarkdown,
        Action::PdfSplit(_) => &crate::pdf::split::PdfSplit,
        Action::PdfMerge(_) => &crate::pdf::merge::PdfMerge,
        Action::OfficeConvert(_) => &crate::office::OfficeConvert,
        Action::OfficeToImages(_) => &crate::office::OfficeToImages,
        Action::VideoConvert(_) => &crate::media::VideoConvert,
        Action::VideoCompress(_) => &crate::media::VideoCompress,
        Action::VideoExtractAudio(_) => &crate::media::ExtractAudio,
        Action::VideoToGif(_) => &crate::media::VideoToGif,
        Action::AudioConvert(_) => &crate::media::AudioConvert,
    }
}

/// A job running on a background thread.
pub struct JobHandle {
    pub events: Receiver<Event>,
    pub cancel: CancelToken,
    thread: Option<JoinHandle<()>>,
}

impl JobHandle {
    /// Wait for the job thread to end (after `JobFinished`).
    pub fn join(mut self) {
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// Stack of the threads that run tasks: a backstop for deeply nested input
/// (readers also limit nesting themselves; a stack overflow aborts the process).
const TASK_STACK: usize = 16 * 1024 * 1024;

/// Start a job in the background.
pub fn run_job(job: Job, config: JobConfig) -> JobHandle {
    let (tx, rx) = unbounded();
    let cancel = CancelToken::new();
    let thread_cancel = cancel.clone();
    let thread = std::thread::Builder::new()
        .name("morph-job".into())
        .spawn(move || {
            run_job_blocking(job, &config, &thread_cancel, &tx);
        });
    JobHandle {
        events: rx,
        cancel,
        thread: thread.ok(),
    }
}

/// Run a job on the current thread, sending events to `tx`. Returns the summary.
pub fn run_job_blocking(
    job: Job,
    config: &JobConfig,
    cancel: &CancelToken,
    tx: &Sender<Event>,
) -> Summary {
    let summary = Mutex::new(Summary::default());
    let job_temp = config
        .temp_root
        .join(format!("job-{}", uuid::Uuid::new_v4()));

    let job_started = Instant::now();
    tracing::info!(
        tasks = job.tasks.len(),
        skipped = job.skipped.len(),
        "job started"
    );
    let base = job.tasks.len() as u64;
    for (i, (path, reason)) in job.skipped.iter().enumerate() {
        tracing::info!(input = %path.display(), "skipped: {reason}");
        let _ = tx.send(Event::TaskSkipped {
            id: TaskId(base + i as u64),
            reason: reason.clone(),
        });
        if let Ok(mut s) = summary.lock() {
            s.skipped += 1;
        }
    }

    let office = RunOpts {
        office_engine: config.office_engine,
        keep_dates: job.keep_dates,
    };
    let on = |queue: Queue| -> Vec<&Task> {
        job.tasks
            .iter()
            .filter(|t| task_queue(t) == queue)
            .collect()
    };
    let cpu_tasks = on(Queue::Cpu);
    // Serial queues next to the CPU pool: (thread name, tasks, workers, next task).
    let queues = [
        ("morph-office", on(Queue::Office), 1, AtomicUsize::new(0)),
        (
            "morph-ffmpeg",
            on(Queue::Ffmpeg),
            config.ffmpeg_jobs.clamp(1, MAX_FFMPEG_JOBS),
            AtomicUsize::new(0),
        ),
    ];

    std::thread::scope(|s| {
        for (name, tasks, workers, next) in &queues {
            for _ in 0..(*workers).min(tasks.len()) {
                let (job_temp, summary) = (&job_temp, &summary);
                // Each worker takes the next task in input order.
                let worker = move || {
                    while let Some(&task) = tasks.get(next.fetch_add(1, Ordering::SeqCst)) {
                        let ran = catch_unwind(AssertUnwindSafe(|| {
                            run_one(task, job_temp, cancel, tx, summary, None, office);
                        }));
                        if ran.is_err() {
                            report_crash(task.id, tx, summary);
                        }
                    }
                };
                let spawned = std::thread::Builder::new()
                    .name((*name).into())
                    .stack_size(TASK_STACK)
                    .spawn_scoped(s, worker);
                if let Err(e) = spawned {
                    // No thread: whatever no other worker takes fails here.
                    while let Some(&task) = tasks.get(next.fetch_add(1, Ordering::SeqCst)) {
                        report_failure(task.id, tx, summary, &e.to_string());
                    }
                }
            }
        }
        run_cpu_tasks(&cpu_tasks, config, office, &job_temp, cancel, tx, &summary);
    });

    let _ = std::fs::remove_dir_all(&job_temp);
    let summary = summary.into_inner().unwrap_or_default();
    tracing::info!(
        done = summary.done,
        failed = summary.failed,
        skipped = summary.skipped,
        cancelled = summary.cancelled,
        bytes_in = summary.bytes_in,
        bytes_out = summary.bytes_out,
        secs = job_started.elapsed().as_secs_f32(),
        "job finished"
    );
    let _ = tx.send(Event::JobFinished {
        summary: summary.clone(),
    });
    summary
}

/// Which queue a task runs on (by its first input).
fn task_queue(task: &Task) -> Queue {
    task.input()
        .map_or(Queue::Cpu, |input| task.action.queue(input))
}

fn report_failure(id: TaskId, tx: &Sender<Event>, summary: &Mutex<Summary>, details: &str) {
    tracing::error!(task = id.0, "task failed: {CRASH_MESSAGE} ({details})");
    if let Ok(mut s) = summary.lock() {
        s.failed += 1;
    }
    let _ = tx.send(Event::TaskFailed {
        id,
        error: CRASH_MESSAGE.into(),
        details: details.to_owned(),
    });
}

/// CPU tasks on the rayon pool, admitted in order under the memory budget.
fn run_cpu_tasks(
    tasks: &[&Task],
    config: &JobConfig,
    office: RunOpts,
    job_temp: &Path,
    cancel: &CancelToken,
    tx: &Sender<Event>,
    summary: &Mutex<Summary>,
) {
    if tasks.is_empty() {
        return;
    }
    let threads = config.cpu_threads.max(1);
    let budget = match config.memory_limit {
        Some(limit) => Budget::new(limit, threads),
        None => Budget::from_system(threads),
    };
    match rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .thread_name(|i| format!("morph-cpu-{i}"))
        .stack_size(TASK_STACK)
        .build()
    {
        // This thread admits tasks one by one and hands them to the pool. Waiting
        // here (not on pool threads) avoids a deadlock: a pool thread waiting in
        // nested rayon work may steal another task, which must never wait for
        // memory held further down its own stack.
        Ok(pool) => pool.in_place_scope(|scope| {
            for &task in tasks {
                let dims = input_dimensions(task);
                let warning = large_input_note(&dims);
                let estimate =
                    estimate_memory(&dims) + extra_memory(&task.action, &dims) + pdf_memory(task);
                match budget.acquire(estimate, cancel) {
                    Some(permit) => {
                        scope.spawn(move |_| {
                            // Backstop: a panic escaping here would skip JobFinished.
                            let ran = catch_unwind(AssertUnwindSafe(|| {
                                run_one(task, job_temp, cancel, tx, summary, warning, office);
                            }));
                            drop(permit);
                            if ran.is_err() {
                                report_crash(task.id, tx, summary);
                            }
                        });
                    }
                    // Cancelled while waiting: reported as skipped.
                    None => run_one(task, job_temp, cancel, tx, summary, None, office),
                }
            }
        }),
        Err(_) => tasks.iter().for_each(|task| {
            let warning = large_input_note(&input_dimensions(task));
            run_one(task, job_temp, cancel, tx, summary, warning, office);
        }),
    }
}

/// Peak memory a task is expected to need, from its image inputs' sizes.
/// PDF inputs have no entry here; see [`pdf_memory`].
fn estimate_memory(dims: &[Option<(u32, u32)>]) -> u64 {
    dims.iter()
        .map(|d| match d {
            Some((w, h)) => u64::from(*w) * u64::from(*h) * memory::IMAGE_BYTES_PER_PIXEL,
            None => memory::UNKNOWN_ESTIMATE,
        })
        .sum()
}

/// One flat share for a task with PDF inputs (pages are processed one at a time).
fn pdf_memory(task: &Task) -> u64 {
    let has_pdf = task
        .inputs
        .iter()
        .any(|p| morph_formats::Category::from_path(p) == Some(morph_formats::Category::Pdf));
    if has_pdf { memory::PDF_ESTIMATE } else { 0 }
}

/// Memory an action needs on top of [`estimate_memory`]: the perceptual
/// metric of Compress "Best (automatic)".
fn extra_memory(action: &Action, dims: &[Option<(u32, u32)>]) -> u64 {
    use crate::image::best;
    use crate::image::options::CompressMode;
    match action {
        Action::ImageCompress(o) if o.mode == CompressMode::Best => {
            dims.iter().map(|d| best::metric_memory(*d)).sum()
        }
        _ => 0,
    }
}

/// Warning for inputs over 100 MP, added to the task's note (SPEC §10).
fn large_input_note(dims: &[Option<(u32, u32)>]) -> Option<String> {
    use crate::image::common::{LARGE_IMAGE_PIXELS, large_image_note};
    dims.iter()
        .flatten()
        .copied()
        .filter(|(w, h)| u64::from(*w) * u64::from(*h) > LARGE_IMAGE_PIXELS)
        .max_by_key(|(w, h)| u64::from(*w) * u64::from(*h))
        .map(|(w, h)| large_image_note(w, h))
}

/// Pixel size of each image input (header read only); `None` if unreadable.
/// Non-image inputs (PDFs) are left out.
fn input_dimensions(task: &Task) -> Vec<Option<(u32, u32)>> {
    use crate::image::io::{DecodeOptions, probe_dimensions_with};
    let decode = match &task.action {
        Action::ImageConvert(o) => DecodeOptions {
            svg_width: o.svg_width.max(1),
        },
        _ => DecodeOptions::default(),
    };
    task.inputs
        .iter()
        .filter(|p| morph_formats::Category::from_path(p) == Some(morph_formats::Category::Image))
        .map(|p| probe_dimensions_with(p, &decode).ok())
        .collect()
}

const CRASH_MESSAGE: &str = "Something went wrong while converting this file.";

fn report_crash(id: TaskId, tx: &Sender<Event>, summary: &Mutex<Summary>) {
    tracing::error!(
        task = id.0,
        "task failed: {CRASH_MESSAGE} (panic outside the converter)"
    );
    if let Ok(mut s) = summary.lock() {
        s.failed += 1;
    }
    let _ = tx.send(Event::TaskFailed {
        id,
        error: CRASH_MESSAGE.into(),
        details: "panic outside the converter".into(),
    });
}

/// Time left at `fraction` done after `elapsed`: `None` until there is enough to go on.
fn estimate_eta(elapsed: Duration, fraction: f32) -> Option<Duration> {
    if elapsed < Duration::from_secs(2) || !(0.02..1.0).contains(&fraction) {
        return None;
    }
    Some(elapsed.mul_f32((1.0 - fraction) / fraction))
}

fn file_size(path: &Path) -> u64 {
    std::fs::metadata(path).map(|m| m.len()).unwrap_or(0)
}

/// Job-wide settings every task runs with.
#[derive(Debug, Clone, Copy)]
struct RunOpts {
    office_engine: EnginePreference,
    /// Settings → "Keep original file dates".
    keep_dates: bool,
}

fn run_one(
    task: &Task,
    job_temp: &Path,
    cancel: &CancelToken,
    tx: &Sender<Event>,
    summary: &Mutex<Summary>,
    warning: Option<String>,
    opts: RunOpts,
) {
    let id = task.id;
    if cancel.is_cancelled() {
        let _ = tx.send(Event::TaskSkipped {
            id,
            reason: "Cancelled.".into(),
        });
        if let Ok(mut s) = summary.lock() {
            s.cancelled += 1;
        }
        return;
    }
    let _ = tx.send(Event::TaskStarted { id });

    let temp_dir = job_temp.join(format!("task-{}", id.0));
    let progress_tx = tx.clone();
    // Video/audio rows show "time left" (SPEC §3.3); converters report `None`,
    // so extrapolate from the elapsed time.
    let estimate = task_queue(task) == Queue::Ffmpeg;
    let started = Instant::now();
    let progress = move |fraction: f32, eta: Option<Duration>| {
        let eta = eta.or_else(|| {
            estimate
                .then(|| estimate_eta(started.elapsed(), fraction))
                .flatten()
        });
        let _ = progress_tx.send(Event::TaskProgress { id, fraction, eta });
    };

    let status_tx = tx.clone();
    let status = move |text: &str| {
        let _ = status_tx.send(Event::TaskStatus {
            id,
            text: text.to_owned(),
        });
    };

    // The task's temp folder is created lazily by `TaskCtx::temp_dir()`.
    let ctx = TaskCtx::new(cancel, &progress, &temp_dir)
        .with_office_engine(opts.office_engine)
        .with_status(&status);
    let result: Result<TaskOutput, ConvertError> = catch_unwind(AssertUnwindSafe(|| {
        converter_for(&task.action).run(task, &ctx)
    }))
    .unwrap_or_else(|payload| {
        let msg = payload
            .downcast_ref::<&str>()
            .map(|s| (*s).to_owned())
            .or_else(|| payload.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "unknown panic".into());
        Err(ConvertError::other(CRASH_MESSAGE, format!("panic: {msg}")))
    });
    let _ = std::fs::remove_dir_all(&temp_dir);

    let bytes_in: u64 = task.inputs.iter().map(|p| file_size(p)).sum();
    let input = task
        .input()
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    let (action, secs) = (task.action.id(), started.elapsed().as_secs_f32());
    match result {
        Ok(out) => {
            if opts.keep_dates
                && let Some(input) = task.input()
            {
                keep_dates(input, &out.outputs);
            }
            // No output (e.g. "Already optimized"): counts as 0% saved, not −100%.
            let bytes_out: u64 = if out.outputs.is_empty() {
                bytes_in
            } else {
                out.outputs.iter().map(|p| file_size(p)).sum()
            };
            if let Ok(mut s) = summary.lock() {
                s.done += 1;
                s.bytes_in += bytes_in;
                s.bytes_out += bytes_out;
            }
            let note = match (warning, out.note) {
                (Some(warn), Some(note)) => Some(format!("{note} {warn}")),
                (warn, note) => note.or(warn),
            };
            tracing::info!(task = id.0, action, %input, outputs = ?out.outputs, bytes_in, bytes_out, secs, "task done");
            let _ = tx.send(Event::TaskDone {
                id,
                outputs: out.outputs,
                bytes_in,
                bytes_out,
                note,
            });
        }
        Err(e) if e.is_cancelled() => {
            tracing::info!(task = id.0, action, %input, "task cancelled");
            if let Ok(mut s) = summary.lock() {
                s.cancelled += 1;
            }
            let _ = tx.send(Event::TaskSkipped {
                id,
                reason: e.message,
            });
        }
        Err(e) => {
            tracing::error!(task = id.0, action, %input, kind = ?e.kind, details = %e.details, secs, "task failed: {}", e.message);
            if let Ok(mut s) = summary.lock() {
                s.failed += 1;
            }
            let _ = tx.send(Event::TaskFailed {
                id,
                error: e.message,
                details: e.details,
            });
        }
    }
}

/// Give `outputs` (files, and the files directly inside output folders) the
/// modified and created times of `input`. Best effort: a date that can't be
/// set never fails the task.
fn keep_dates(input: &Path, outputs: &[PathBuf]) {
    use std::fs::FileTimes;
    use std::os::windows::fs::FileTimesExt;
    let Ok(meta) = std::fs::metadata(input) else {
        return;
    };
    let mut times = FileTimes::new();
    if let Ok(modified) = meta.modified() {
        times = times.set_modified(modified);
    }
    if let Ok(created) = meta.created() {
        times = times.set_created(created);
    }
    let set = |path: &Path| {
        if let Ok(file) = std::fs::OpenOptions::new().write(true).open(path) {
            let _ = file.set_times(times);
        }
    };
    for output in outputs {
        if output.is_dir() {
            for entry in std::fs::read_dir(output).into_iter().flatten().flatten() {
                if entry.file_type().is_ok_and(|t| t.is_file()) {
                    set(&entry.path());
                }
            }
        } else {
            set(output);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::job::OutputSpec;

    #[test]
    fn eta_extrapolates_once_there_is_enough_to_go_on() {
        let s = Duration::from_secs;
        assert_eq!(estimate_eta(s(1), 0.5), None);
        assert_eq!(estimate_eta(s(10), 0.01), None);
        assert_eq!(estimate_eta(s(10), 1.0), None);
        assert_eq!(estimate_eta(s(10), 0.25), Some(s(30)));
    }

    fn collect(job: Job, cancel: &CancelToken) -> (Summary, Vec<Event>) {
        let (tx, rx) = unbounded();
        let config = JobConfig {
            cpu_threads: 2,
            temp_root: std::env::temp_dir().join("morph-test"),
            memory_limit: None,
            office_engine: EnginePreference::Auto,
            ffmpeg_jobs: 2,
        };
        let summary = run_job_blocking(job, &config, cancel, &tx);
        drop(tx);
        (summary, rx.iter().collect())
    }

    #[test]
    fn unsupported_inputs_are_skipped_and_failures_dont_stop_the_batch() {
        let files = [
            PathBuf::from(r"C:\nope\a.jpg"),
            PathBuf::from(r"C:\nope\b.png"),
            PathBuf::from(r"C:\nope\c.exe"),
        ];
        let action = Action::default_for(morph_formats::actions::IMAGE_CONVERT).unwrap();
        let job = Job::for_files(&files, &action, &OutputSpec::default());
        assert_eq!(job.tasks.len(), 2);
        assert_eq!(job.skipped.len(), 1);

        let (summary, events) = collect(job, &CancelToken::new());
        assert_eq!(summary.skipped, 1);
        assert_eq!(summary.failed + summary.done, 2, "every task reports");
        assert!(matches!(events.last(), Some(Event::JobFinished { .. })));
        assert!(
            events
                .iter()
                .any(|e| matches!(e, Event::TaskSkipped { id: TaskId(2), .. }))
        );
    }

    #[test]
    fn tiny_memory_budget_still_finishes_every_task() {
        let files: Vec<PathBuf> = (0..6).map(|i| PathBuf::from(format!("{i}.jpg"))).collect();
        let action = Action::default_for(morph_formats::actions::IMAGE_CONVERT).unwrap();
        let job = Job::for_files(&files, &action, &OutputSpec::default());
        let (tx, rx) = unbounded();
        let config = JobConfig {
            cpu_threads: 3,
            temp_root: std::env::temp_dir().join("morph-test"),
            memory_limit: Some(1),
            ..JobConfig::default()
        };
        let summary = run_job_blocking(job, &config, &CancelToken::new(), &tx);
        drop(tx);
        assert_eq!(summary.done + summary.failed, 6);
        assert!(matches!(rx.iter().last(), Some(Event::JobFinished { .. })));
    }

    #[test]
    fn sweep_removes_only_old_job_dirs() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("job-old")).unwrap();
        std::fs::create_dir(root.path().join("keep-me")).unwrap();
        sweep_stale_temp(root.path(), std::time::Duration::ZERO);
        assert!(!root.path().join("job-old").exists());
        assert!(root.path().join("keep-me").exists());
        std::fs::create_dir(root.path().join("job-new")).unwrap();
        sweep_stale_temp(root.path(), std::time::Duration::from_secs(3600));
        assert!(root.path().join("job-new").exists(), "recent job dir kept");
    }

    #[test]
    fn ffmpeg_queue_tasks_all_report() {
        let files: Vec<PathBuf> = (0..5)
            .map(|i| PathBuf::from(format!(r"C:\nope\{i}.mp4")))
            .collect();
        let action = Action::default_for(morph_formats::actions::VIDEO_TO_GIF).unwrap();
        let job = Job::for_files(&files, &action, &OutputSpec::default());
        assert!(job.tasks.iter().all(|t| task_queue(t) == Queue::Ffmpeg));
        let (summary, events) = collect(job, &CancelToken::new());
        assert_eq!(summary.done + summary.failed, 5);
        let mut ids: Vec<u64> = events
            .iter()
            .filter_map(|e| match e {
                Event::TaskFailed { id, .. } | Event::TaskDone { id, .. } => Some(id.0),
                _ => None,
            })
            .collect();
        ids.sort_unstable();
        assert_eq!(ids, [0, 1, 2, 3, 4], "each task exactly once");
        assert!(matches!(events.last(), Some(Event::JobFinished { .. })));
    }

    #[test]
    fn office_and_cpu_tasks_all_report() {
        // Missing files: every task fails fast, on its own queue, and the job ends.
        let files = [
            PathBuf::from(r"C:\nope\a.docx"),
            PathBuf::from(r"C:\nope\b.xlsx"),
            PathBuf::from(r"C:\nope\c.md"),
            PathBuf::from(r"C:\nope\d.pptx"),
        ];
        let action = Action::default_for(morph_formats::actions::OFFICE_CONVERT).unwrap();
        let job = Job::for_files(&files, &action, &OutputSpec::default());
        assert_eq!(job.tasks.len(), 4);
        let queues: Vec<Queue> = job.tasks.iter().map(task_queue).collect();
        assert_eq!(queues.iter().filter(|q| **q == Queue::Office).count(), 4);
        let (summary, events) = collect(job, &CancelToken::new());
        assert_eq!(summary.done + summary.failed, 4);
        assert!(matches!(events.last(), Some(Event::JobFinished { .. })));
    }

    #[test]
    fn keep_dates_copies_the_input_times() {
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("in.txt");
        let output = dir.path().join("out.txt");
        std::fs::write(&input, "a").unwrap();
        let old = std::time::SystemTime::UNIX_EPOCH + Duration::from_secs(1_600_000_000);
        std::fs::File::options()
            .write(true)
            .open(&input)
            .unwrap()
            .set_modified(old)
            .unwrap();
        std::fs::write(&output, "b").unwrap();
        keep_dates(&input, std::slice::from_ref(&output));
        let modified = std::fs::metadata(&output).unwrap().modified().unwrap();
        assert_eq!(modified, old);
    }

    #[test]
    fn cancelled_job_runs_nothing() {
        let files = [PathBuf::from("a.jpg"), PathBuf::from("b.jpg")];
        let action = Action::default_for(morph_formats::actions::IMAGE_CONVERT).unwrap();
        let job = Job::for_files(&files, &action, &OutputSpec::default());
        let cancel = CancelToken::new();
        cancel.cancel();
        let (summary, events) = collect(job, &cancel);
        assert_eq!(summary.cancelled, 2);
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, Event::TaskStarted { .. }))
        );
    }
}
