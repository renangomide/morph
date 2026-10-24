// SPDX-License-Identifier: GPL-3.0-or-later
//! SPEC §12 M7 acceptance: "Mixed selection of 4 types converts in one job".
//! Real fixtures (image, PDF, CSV) + a lavfi clip + a broken image + an
//! unsupported file → one `Job::for_groups` job, run by the scheduler.
//! The video part prints `SKIPPED:` and is left out when FFmpeg is missing.
//! CSV → XLSX is native, so no Office engine is needed.

use morph_core::action::Action;
use morph_core::cancel::CancelToken;
use morph_core::converter::TaskCtx;
use morph_core::event::Event;
use morph_core::job::{SKIP_UNSUPPORTED, TaskId, group_files};
use morph_core::media::{self, ffmpeg, probe};
use morph_core::office::OfficeConvertOpts;
use morph_core::office::options::DocFormat;
use morph_core::scheduler::{JobConfig, run_job_blocking};
use morph_core::{Job, OutputSpec};
use morph_formats::category::MenuGroup;
use morph_formats::{EnabledCategories, actions};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures")
        .join(name)
}

/// A 2 s H.264/AAC clip in `folder`, made with lavfi.
fn make_clip(folder: &Path) -> PathBuf {
    let cancel = CancelToken::new();
    let work = tempfile::tempdir().unwrap();
    let ctx = TaskCtx::new(&cancel, &|_, _| {}, work.path());
    let out = media::output_file(folder, "clip.mkv").unwrap();
    let mut args: Vec<OsString> = [
        "-f",
        "lavfi",
        "-i",
        "testsrc2=duration=2:size=320x240:rate=25",
        "-f",
        "lavfi",
        "-i",
        "sine=frequency=440:duration=2",
        "-shortest",
        "-c:v",
        "libx264",
        "-preset",
        "ultrafast",
        "-pix_fmt",
        "yuv420p",
        "-c:a",
        "aac",
        "-f",
        "matroska",
    ]
    .iter()
    .map(OsString::from)
    .collect();
    args.push(ffmpeg::path_arg(out.temp_path()));
    ffmpeg::run(&args, None, &ctx, &mut |_| {}).unwrap();
    out.commit().unwrap()
}

fn mtime(path: &Path) -> SystemTime {
    std::fs::metadata(path).unwrap().modified().unwrap()
}

#[test]
fn mixed_selection_of_four_types_converts_in_one_job() {
    let dir = tempfile::tempdir().unwrap();
    let copy = |name: &str| {
        let to = dir.path().join(name);
        std::fs::copy(fixture(name), &to).unwrap();
        to
    };
    let jpg = copy("sample.jpg");
    let broken = copy("broken.png");
    let pdf = copy("text-3pages.pdf");
    let csv = copy("sample.csv");
    let exe = dir.path().join("x.exe");
    std::fs::write(&exe, b"MZ").unwrap();
    let clip = if ffmpeg::find().is_some() {
        Some(make_clip(dir.path()))
    } else {
        println!("SKIPPED: FFmpeg not found, the video part is left out");
        None
    };

    // Old dates on every input, to check "Keep original file dates".
    let old = SystemTime::UNIX_EPOCH + Duration::from_secs(1_500_000_000);
    let mut files = vec![jpg.clone(), broken.clone(), pdf.clone()];
    files.extend(clip.clone());
    files.extend([csv.clone(), exe.clone()]);
    for f in &files {
        let file = std::fs::File::options().write(true).open(f).unwrap();
        file.set_modified(old).unwrap();
    }
    let before: Vec<Vec<u8>> = files.iter().map(|f| std::fs::read(f).unwrap()).collect();

    let grouped = group_files(&files, &EnabledCategories::default());
    let mut want_groups = vec![MenuGroup::Image, MenuGroup::Pdf];
    if clip.is_some() {
        want_groups.push(MenuGroup::Video);
    }
    want_groups.push(MenuGroup::Office);
    let groups: Vec<MenuGroup> = grouped.groups.iter().map(|(g, _)| *g).collect();
    assert_eq!(groups, want_groups);
    assert_eq!(
        grouped.skipped,
        [(exe.clone(), SKIP_UNSUPPORTED.to_owned())]
    );

    let def = |id| Action::default_for(id).unwrap();
    let per_group = [
        (MenuGroup::Image, def(actions::IMAGE_CONVERT)), // → PNG
        (MenuGroup::Pdf, def(actions::PDF_COMPRESS)),
        (MenuGroup::Video, def(actions::VIDEO_CONVERT)), // → MP4
        (
            MenuGroup::Office,
            Action::OfficeConvert(OfficeConvertOpts {
                format: DocFormat::Xlsx,
                ..OfficeConvertOpts::default()
            }),
        ),
    ];
    // The window's path (`view::mixed_job`): `Job::for_groups` + a per-tab planner.
    let mut job = Job::for_groups(&grouped, |group, files| {
        match per_group.iter().find(|(g, _)| *g == group) {
            Some((_, action)) => Job::for_files(files, action, &OutputSpec::default()),
            None => Job::skip_all(files, "no action"),
        }
    });
    job.keep_dates = true;
    let tasks = job.tasks.len();
    assert_eq!(tasks, files.len() - 1, "one task per supported file");

    let work = tempfile::tempdir().unwrap();
    let config = JobConfig {
        temp_root: work.path().to_path_buf(),
        ..JobConfig::default()
    };
    let (tx, rx) = crossbeam_channel::unbounded();
    let summary = run_job_blocking(job, &config, &CancelToken::new(), &tx);
    drop(tx);
    let events: Vec<Event> = rx.into_iter().collect();

    // Inputs untouched.
    for (f, bytes) in files.iter().zip(&before) {
        assert_eq!(&std::fs::read(f).unwrap(), bytes, "{} changed", f.display());
    }

    // Summary: everything done but the broken file; the .exe skipped.
    assert_eq!(summary.done, tasks - 1, "{events:#?}");
    assert_eq!(summary.failed, 1);
    assert_eq!(summary.skipped, 1);
    assert_eq!(summary.cancelled, 0);

    // Per-task event order: Started → (Status…) → Done/Failed, exactly one end.
    for id in 0..tasks as u64 {
        let mine: Vec<&Event> = events
            .iter()
            .filter(|e| match e {
                Event::TaskStarted { id: i }
                | Event::TaskStatus { id: i, .. }
                | Event::TaskDone { id: i, .. }
                | Event::TaskFailed { id: i, .. } => *i == TaskId(id),
                _ => false,
            })
            .collect();
        assert!(
            matches!(mine.first(), Some(Event::TaskStarted { .. })),
            "task {id}: {mine:?}"
        );
        let ends = mine
            .iter()
            .filter(|e| matches!(e, Event::TaskDone { .. } | Event::TaskFailed { .. }))
            .count();
        assert_eq!(ends, 1, "task {id}: {mine:?}");
        assert!(
            matches!(
                mine.last(),
                Some(Event::TaskDone { .. } | Event::TaskFailed { .. })
            ),
            "task {id}: nothing after the end: {mine:?}"
        );
    }
    assert!(matches!(events.last(), Some(Event::JobFinished { .. })));
    let skipped: Vec<&Event> = events
        .iter()
        .filter(|e| matches!(e, Event::TaskSkipped { .. }))
        .collect();
    assert_eq!(skipped.len(), 1);
    assert!(matches!(
        skipped[0],
        Event::TaskSkipped { id, reason } if *id == TaskId(tasks as u64) && reason == SKIP_UNSUPPORTED
    ));

    // The broken file failed alone, with a message and details.
    let failed: Vec<&Event> = events
        .iter()
        .filter(|e| matches!(e, Event::TaskFailed { .. }))
        .collect();
    assert_eq!(failed.len(), 1);
    if let Event::TaskFailed { error, details, .. } = failed[0] {
        assert!(!error.is_empty() && !details.is_empty());
    }

    // Every output exists, opens, and kept the input's date.
    let outputs: Vec<PathBuf> = events
        .iter()
        .filter_map(|e| match e {
            Event::TaskDone { outputs, .. } => Some(outputs.clone()),
            _ => None,
        })
        .flatten()
        .collect();
    let by_ext = |ext: &str| {
        outputs
            .iter()
            .find(|o| o.extension().is_some_and(|e| e.eq_ignore_ascii_case(ext)))
            .unwrap_or_else(|| panic!("no .{ext} output in {outputs:?}"))
            .clone()
    };
    let png = by_ext("png");
    assert_eq!(png.parent(), Some(dir.path()));
    let img = image::open(&png).unwrap();
    assert!(img.width() > 0 && img.height() > 0);

    let out_pdf = outputs
        .iter()
        .find(|o| o.extension().is_some_and(|e| e == "pdf"))
        .unwrap();
    assert_ne!(out_pdf, &pdf);
    let doc = morph_core::pdf::pdfium::open(out_pdf).unwrap();
    assert_eq!(morph_core::pdf::pdfium::page_count(&doc), 3);
    drop(doc);

    let xlsx = by_ext("xlsx");
    use calamine::Reader;
    let mut book = calamine::open_workbook_auto(&xlsx).unwrap();
    let sheet = book.sheet_names()[0].clone();
    let range = book.worksheet_range(&sheet).unwrap();
    assert!(range.height() >= 2, "rows: {}", range.height());

    if clip.is_some() {
        let mp4 = by_ext("mp4");
        let info = probe::probe(&mp4, &CancelToken::new()).unwrap();
        assert!(info.has_video() && info.has_audio());
    }

    for out in &outputs {
        assert_eq!(mtime(out), old, "{} kept the input's date", out.display());
    }
}
