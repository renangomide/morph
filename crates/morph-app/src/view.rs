// SPDX-License-Identifier: GPL-3.0-or-later
//! Plain-Rust view helpers for the window, kept apart from Slint so they are easy to test:
//! the selection (files + what the window can do with them), header and sizes,
//! and the tabs of a mixed selection (SPEC §3.1, §3.2).

use crate::media_options::{self, VideoKind};
use crate::office_options::{self, OfficeKind};
use crate::options::ImageKind;
use crate::pdf_options::PdfKind;
use crate::strings;
use morph_core::naming::natural_cmp;
use morph_core::office::{EnginePreference, Engines};
use morph_core::{Action, Job, OutputSpec, group_files};
use morph_formats::{Category, EnabledCategories, MenuGroup, Request, actions};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileView {
    pub name: String,
    pub folder: String,
    pub size: String,
}

/// What the window shows for a request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mode {
    /// Image option panels with this action selected.
    Images(ImageKind),
    /// PDF option panels with this action selected.
    Pdf(PdfKind),
    /// Office / text document panels with this action selected.
    Office(OfficeKind),
    /// Video panels with this action selected (an Audio tab too when audio
    /// files are selected as well).
    Video(VideoKind),
    /// Audio → Convert (a Video tab too when videos are selected as well).
    Audio,
    /// Several types (`mixed.convert`): one tab per type, see [`Opening::tabs`].
    Mixed,
    /// A category a later milestone handles: a plain message.
    Unavailable(String),
}

/// What the window opens with for a request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Opening {
    pub mode: Mode,
    /// The files some panel takes (selection order).
    pub files: Vec<PathBuf>,
    /// Unsupported files and types turned off in Settings, with the reason.
    pub skipped: Vec<(PathBuf, String)>,
    /// [`Mode::Mixed`]: the tabs, in order (images, PDF, video, audio, documents).
    pub tabs: Vec<MenuGroup>,
}

/// Split the request's files by type: a `mixed.convert` of several types gets
/// tabs, anything else opens the panels [`mode_for`] picks, as before.
pub fn open(request: &Request, enabled: &EnabledCategories) -> Opening {
    let mixed = request.action == actions::MIXED_CONVERT;
    // Dropped / browsed folders (even one named "Trip.jpg") aren't files to convert.
    let (folders, files): (Vec<PathBuf>, Vec<PathBuf>) = request
        .files
        .iter()
        .cloned()
        .partition(|f| mixed && f.is_dir());
    let mut grouped = group_files(&files, enabled);
    let out: HashSet<&Path> = grouped.skipped.iter().map(|(p, _)| p.as_path()).collect();
    let files: Vec<PathBuf> = files
        .iter()
        .filter(|f| !out.contains(f.as_path()))
        .cloned()
        .collect();
    let folders = folders
        .into_iter()
        .map(|f| (f, strings::FOLDERS_NOT_SUPPORTED.to_owned()));
    grouped.skipped = folders.chain(grouped.skipped).collect();
    let mode = if mixed && files.is_empty() {
        Mode::Unavailable(strings::NO_SUPPORTED_FILES.to_owned())
    } else if mixed && grouped.is_mixed() {
        Mode::Mixed
    } else {
        mode_for(&Request {
            files: files.clone(),
            ..request.clone()
        })
    };
    let tabs = if mode == Mode::Mixed {
        grouped.groups.iter().map(|(g, _)| *g).collect()
    } else {
        Vec::new()
    };
    Opening {
        mode,
        files,
        skipped: grouped.skipped,
        tabs,
    }
}

/// The type of `file` as a tab, if it has one.
pub fn group_of(file: &Path) -> Option<MenuGroup> {
    Category::from_path(file).map(Category::menu_group)
}

/// The tabs `files` still fill, in tab order.
pub fn tabs_present(files: &[PathBuf]) -> Vec<MenuGroup> {
    group_files(files, &EnabledCategories::default())
        .groups
        .into_iter()
        .map(|(g, _)| g)
        .collect()
}

/// "Images", "PDF", … (no count), for "Include Images" and the skip reason.
pub fn group_name(group: MenuGroup) -> &'static str {
    match group {
        MenuGroup::Image => strings::TYPE_IMAGES,
        MenuGroup::Pdf => strings::TYPE_PDF,
        MenuGroup::Video => strings::TYPE_VIDEO,
        MenuGroup::Audio => strings::TYPE_AUDIO,
        MenuGroup::Office => strings::GROUP_DOCUMENTS,
    }
}

/// One line of what a type tab runs now, for its card: "→ WebP",
/// "Compress", "→ PDF".
pub fn action_summary(action: &Action) -> String {
    use morph_core::pdf::options::PdfMarkupFormat;
    let to = strings::summary_to;
    match action {
        Action::ImageConvert(o) => to(strings::format_label(o.format)),
        Action::ImageToPdf(_) => to(strings::SUMMARY_PDF),
        Action::ImageCompress(_) => strings::ACTION_COMPRESS.to_owned(),
        Action::ImageResize(_) => strings::ACTION_RESIZE.to_owned(),
        Action::ImageClean(_) => strings::ACTION_CLEAN.to_owned(),
        Action::PdfCompress(_) => strings::PDF_ACTION_COMPRESS.to_owned(),
        Action::PdfToImages(o) => to(strings::format_label(o.format.output_format())),
        Action::PdfToWord(_) => to(strings::SUMMARY_WORD),
        Action::PdfToText(_) => to(strings::SUMMARY_TEXT),
        Action::PdfToMarkdown(o) => to(match o.format {
            PdfMarkupFormat::Md => strings::SUMMARY_MARKDOWN,
            PdfMarkupFormat::Html => strings::SUMMARY_HTML,
        }),
        Action::PdfSplit(_) => strings::PDF_ACTION_SPLIT.to_owned(),
        Action::PdfMerge(_) => strings::PDF_ACTION_MERGE.to_owned(),
        Action::OfficeConvert(o) => to(strings::doc_format_short(o.format)),
        Action::OfficeToImages(o) => to(strings::format_label(o.format.output_format())),
        Action::VideoConvert(o) => to(strings::video_container_label(o.format)),
        Action::VideoCompress(_) => strings::VIDEO_ACTION_COMPRESS.to_owned(),
        Action::VideoExtractAudio(o) | Action::AudioConvert(o) => {
            to(strings::audio_format_label(o.format))
        }
        Action::VideoToGif(_) => to(strings::SUMMARY_GIF),
    }
}

/// "Images (3)".
pub fn tab_label(group: MenuGroup, files: &[PathBuf]) -> String {
    let count = files.iter().filter(|f| group_of(f) == Some(group)).count();
    match group {
        MenuGroup::Image => strings::tab_images(count),
        MenuGroup::Pdf => strings::tab_pdf(count),
        MenuGroup::Video => strings::tab_video(count),
        MenuGroup::Audio => strings::tab_audio(count),
        MenuGroup::Office => strings::tab_documents(count),
    }
}

/// The job the window would run, plus what the engine banner needs.
#[derive(Debug, Clone, Default)]
pub struct Planned {
    pub job: Job,
    /// Per tab (one entry for a single-type window): engine needs.
    pub groups: Vec<GroupPlan>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupPlan {
    /// `None` in a single-type window.
    pub group: Option<MenuGroup>,
    /// Files skipped because no Office engine is installed.
    pub missing_engine: usize,
    /// Engines that will be used ("Microsoft Word", "LibreOffice").
    pub engines: Vec<&'static str>,
}

impl Planned {
    /// Files that run (each once).
    pub fn runs(&self) -> HashSet<&Path> {
        self.job
            .tasks
            .iter()
            .flat_map(|t| t.inputs.iter().map(PathBuf::as_path))
            .collect()
    }

    /// Files waiting for an engine and the engines used, for one tab (`None` = all).
    pub fn engine(&self, group: Option<MenuGroup>) -> (usize, Vec<&'static str>) {
        let mut missing = 0;
        let mut engines = Vec::new();
        for g in self
            .groups
            .iter()
            .filter(|g| group.is_none() || g.group == group)
        {
            missing += g.missing_engine;
            for name in &g.engines {
                if !engines.contains(name) {
                    engines.push(*name);
                }
            }
        }
        (missing, engines)
    }
}

/// One type's files under `action`: files already in the target format or
/// without an engine are skipped up front with their reason.
fn plan_part(
    files: &[PathBuf],
    action: &Action,
    engines: &Engines,
    preference: EnginePreference,
    output: &OutputSpec,
) -> (Job, GroupPlan) {
    let plan = office_options::plan_files(files, action, engines, preference);
    let mut job = Job::for_files(&plan.run, action, output);
    job.skipped.extend(plan.skipped);
    let group = GroupPlan {
        group: None,
        missing_engine: plan.missing_engine,
        engines: plan.engines,
    };
    (job, group)
}

/// A single-type window: `actions` on all `files` (video + audio: one action
/// per type), then the files skipped up front.
pub fn single_job(
    files: &[PathBuf],
    skipped: &[(PathBuf, String)],
    actions: &[Action],
    engines: &Engines,
    preference: EnginePreference,
    output: &OutputSpec,
) -> Planned {
    let mut planned = Planned::default();
    let job = match actions {
        [action] => {
            let (job, group) = plan_part(files, action, engines, preference, output);
            planned.groups.push(group);
            job
        }
        _ => media_options::build_job(files, actions, output),
    };
    planned.job = Job::combine([
        job,
        Job {
            skipped: skipped.to_vec(),
            ..Job::default()
        },
    ]);
    planned
}

/// A mixed window: each tab's files with that tab's action, in tab order, in
/// one job ([`Job::combine`]). A tab the user unchecked (`excluded`) has its
/// files skipped by choice; a tab without an action (no FFmpeg) has them
/// skipped with `no_action`; `skipped` (unsupported, turned off) last.
#[allow(clippy::too_many_arguments)]
pub fn mixed_job(
    files: &[PathBuf],
    skipped: &[(PathBuf, String)],
    actions: &[(MenuGroup, Action)],
    excluded: &[MenuGroup],
    no_action: &str,
    engines: &Engines,
    preference: EnginePreference,
    output: &OutputSpec,
) -> Planned {
    let grouped = group_files(files, &EnabledCategories::default());
    let mut groups = Vec::new();
    let job = Job::for_groups(&grouped, |group, files| {
        if excluded.contains(&group) {
            return Job::skip_all(files, &strings::skipped_by_choice(group_name(group)));
        }
        match actions.iter().find(|(g, _)| *g == group) {
            Some((_, action)) => {
                let (job, plan) = plan_part(files, action, engines, preference, output);
                groups.push(GroupPlan {
                    group: Some(group),
                    ..plan
                });
                job
            }
            None => Job::skip_all(files, no_action),
        }
    });
    Planned {
        job: Job::combine([
            job,
            Job {
                skipped: skipped.to_vec(),
                ..Job::default()
            },
        ]),
        groups,
    }
}

/// Decide the window mode from the request's action and files.
pub fn mode_for(request: &Request) -> Mode {
    if let Some(kind) = ImageKind::from_id(&request.action) {
        return Mode::Images(kind);
    }
    if let Some(kind) = PdfKind::from_id(&request.action) {
        return Mode::Pdf(kind);
    }
    if let Some(kind) = OfficeKind::from_id(&request.action) {
        return Mode::Office(kind);
    }
    if let Some(kind) = VideoKind::from_id(&request.action) {
        return Mode::Video(kind);
    }
    if request.action == actions::AUDIO_CONVERT {
        return Mode::Audio;
    }
    let has = |pred: &dyn Fn(Category) -> bool| {
        request
            .files
            .iter()
            .any(|f| Category::from_path(f).is_some_and(pred))
    };
    if request.action == actions::MIXED_CONVERT {
        if has(&|c| c == Category::Image) {
            return Mode::Images(ImageKind::Convert);
        }
        if has(&|c| c == Category::Pdf) {
            return Mode::Pdf(PdfKind::Compress);
        }
        if has(&Category::is_document) {
            return Mode::Office(OfficeKind::Convert);
        }
        if has(&|c| c == Category::Video) {
            return Mode::Video(VideoKind::Convert);
        }
        if has(&|c| c == Category::Audio) {
            return Mode::Audio;
        }
        let first = request.files.iter().find_map(|f| Category::from_path(f));
        return Mode::Unavailable(match first {
            Some(category) => strings::not_available(category_what(category)),
            None => strings::NO_SUPPORTED_FILES.to_owned(),
        });
    }
    let what = match request.action.split('.').next() {
        Some("pdf") => strings::WHAT_PDF,
        Some("video") => strings::WHAT_VIDEO,
        Some("audio") => strings::WHAT_AUDIO,
        Some("office") => strings::WHAT_OFFICE,
        _ => strings::WHAT_THESE,
    };
    Mode::Unavailable(strings::not_available(what))
}

fn category_what(category: Category) -> &'static str {
    match category {
        Category::Image => strings::WHAT_THESE,
        Category::Pdf => strings::WHAT_PDF,
        Category::Video => strings::WHAT_VIDEO,
        Category::Audio => strings::WHAT_AUDIO,
        Category::Word | Category::Excel | Category::PowerPoint | Category::Text => {
            strings::WHAT_OFFICE
        }
    }
}

/// The selected files with their sizes (`None` = missing).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Selection {
    pub files: Vec<PathBuf>,
    pub sizes: Vec<Option<u64>>,
}

impl Selection {
    /// Read each file's size from disk.
    pub fn from_files(files: Vec<PathBuf>) -> Selection {
        let sizes = files
            .iter()
            .map(|p| {
                std::fs::metadata(p)
                    .ok()
                    .filter(|m| m.is_file())
                    .map(|m| m.len())
            })
            .collect();
        Selection { files, sizes }
    }

    pub fn remove(&mut self, index: usize) {
        if index < self.files.len() {
            self.files.remove(index);
        }
        if index < self.sizes.len() {
            self.sizes.remove(index);
        }
    }

    /// Total size in bytes of the files at `indexes` (missing files count 0).
    pub fn size_of(&self, indexes: &[usize]) -> u64 {
        indexes
            .iter()
            .filter_map(|i| self.sizes.get(*i).copied().flatten())
            .sum()
    }

    /// Natural file-name order (`2.pdf` before `10.pdf`), the starting order of Merge.
    pub fn sort_natural(&mut self) {
        let sizes = std::mem::take(&mut self.sizes);
        let mut pairs: Vec<(PathBuf, Option<u64>)> = std::mem::take(&mut self.files)
            .into_iter()
            .zip(sizes.into_iter().chain(std::iter::repeat(None)))
            .collect();
        let name = |p: &Path| {
            p.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default()
        };
        // Stable: equal names keep their order.
        pairs.sort_by(|a, b| natural_cmp(&name(&a.0), &name(&b.0)));
        (self.files, self.sizes) = pairs.into_iter().unzip();
    }

    /// Move the file at `from` so it ends up at index `to` (drag-to-reorder).
    /// Returns whether anything moved.
    pub fn move_file(&mut self, from: usize, to: usize) -> bool {
        let n = self.files.len();
        if from >= n || to >= n || from == to {
            return false;
        }
        let file = self.files.remove(from);
        self.files.insert(to, file);
        if from < self.sizes.len() && to < self.sizes.len() {
            let size = self.sizes.remove(from);
            self.sizes.insert(to, size);
        }
        true
    }

    pub fn has_svg(&self) -> bool {
        self.files
            .iter()
            .any(|f| f.extension().is_some_and(|e| e.eq_ignore_ascii_case("svg")))
    }

    pub fn file_views(&self) -> Vec<FileView> {
        self.files
            .iter()
            .enumerate()
            .map(|(i, f)| file_view(f, self.sizes.get(i).copied().flatten()))
            .collect()
    }
}

fn file_view(path: &Path, size: Option<u64>) -> FileView {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned());
    let folder = path
        .parent()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default();
    let size = match size {
        Some(bytes) => format_size(bytes),
        None => strings::SIZE_MISSING.to_owned(),
    };
    FileView { name, folder, size }
}

/// Human-readable size: `512 B`, `1.5 KB`, `48.3 MB`.
pub fn format_size(bytes: u64) -> String {
    let units = strings::SIZE_UNITS;
    if bytes < 1024 {
        return format!("{bytes} {}", units[0]);
    }
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < units.len() {
        value /= 1024.0;
        unit += 1;
    }
    format!("{value:.1} {}", units[unit])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes() {
        assert_eq!(format_size(0), "0 B");
        assert_eq!(format_size(1023), "1023 B");
        assert_eq!(format_size(1536), "1.5 KB");
        assert_eq!(format_size(50_646_630), "48.3 MB");
        assert_eq!(format_size(3 * 1024 * 1024 * 1024), "3.0 GB");
    }

    #[test]
    fn selection_counts_and_removes() {
        let mut sel = Selection {
            files: vec![
                PathBuf::from(r"C:\Fotos\a.jpg"),
                PathBuf::from(r"C:\Fotos\logo.SVG"),
                PathBuf::from(r"C:\gone.pdf"),
            ],
            sizes: vec![Some(1536), Some(512), None],
        };
        assert_eq!(sel.size_of(&[0, 1, 2]), 2048);
        assert_eq!(sel.size_of(&[1, 2]), 512);
        assert!(sel.has_svg());
        let views = sel.file_views();
        assert_eq!(
            views[0],
            FileView {
                name: "a.jpg".into(),
                folder: r"C:\Fotos".into(),
                size: "1.5 KB".into(),
            }
        );
        assert_eq!(views[2].size, "missing");
        sel.remove(1);
        sel.remove(9);
        assert_eq!(sel.files.len(), 2);
        assert!(!sel.has_svg());
        assert_eq!(sel.size_of(&[0, 1]), 1536);
    }

    #[test]
    fn modes() {
        let req = |action: &str, files: &[&str]| {
            Request::new(files.iter().map(PathBuf::from).collect(), action, &[])
        };
        assert_eq!(
            mode_for(&req(actions::IMAGE_RESIZE, &["a.jpg"])),
            Mode::Images(ImageKind::Resize)
        );
        assert_eq!(
            mode_for(&req(actions::MIXED_CONVERT, &["a.pdf", "b.jpg"])),
            Mode::Images(ImageKind::Convert)
        );
        assert_eq!(
            mode_for(&req(actions::MIXED_CONVERT, &["a.pdf", "b.mp4"])),
            Mode::Pdf(PdfKind::Compress)
        );
        assert_eq!(
            mode_for(&req(actions::MIXED_CONVERT, &["b.mp4"])),
            Mode::Video(VideoKind::Convert)
        );
        assert_eq!(
            mode_for(&req(actions::MIXED_CONVERT, &["b.mp3", "c.mkv"])),
            Mode::Video(VideoKind::Convert)
        );
        assert_eq!(
            mode_for(&req(actions::MIXED_CONVERT, &["b.mp3"])),
            Mode::Audio
        );
        assert_eq!(
            mode_for(&req(actions::MIXED_CONVERT, &["a.exe"])),
            Mode::Unavailable(strings::NO_SUPPORTED_FILES.into())
        );
        assert_eq!(
            mode_for(&req(actions::VIDEO_COMPRESS, &["a.mp4"])),
            Mode::Video(VideoKind::Compress)
        );
        assert_eq!(
            mode_for(&req("video.unknown", &["a.mp4"])),
            Mode::Unavailable(strings::not_available(strings::WHAT_VIDEO))
        );
    }

    #[test]
    fn pdf_menu_entries_preselect_their_action() {
        let req = |action: &str, files: &[&str]| {
            Request::new(files.iter().map(PathBuf::from).collect(), action, &[])
        };
        for (id, kind) in [
            (actions::PDF_COMPRESS, PdfKind::Compress),
            (actions::PDF_TO_IMAGES, PdfKind::ToImages),
            (actions::PDF_TO_TEXT, PdfKind::ToText),
            (actions::PDF_SPLIT, PdfKind::Split),
            (actions::PDF_MERGE, PdfKind::Merge),
        ] {
            assert_eq!(
                mode_for(&req(id, &["a.pdf", "b.pdf"])),
                Mode::Pdf(kind),
                "{id}"
            );
        }
        // "Combine into one PDF…" on images + PDFs.
        assert_eq!(
            mode_for(&req(actions::PDF_MERGE, &["a.jpg", "b.pdf"])),
            Mode::Pdf(PdfKind::Merge)
        );
        assert_eq!(
            mode_for(&req(actions::PDF_TO_WORD, &["a.pdf"])),
            Mode::Pdf(PdfKind::ToWord)
        );
    }

    #[test]
    fn office_and_text_menu_entries_open_the_document_panels() {
        use crate::office_options::OfficeForm;
        use morph_core::office::options::DocFormat;
        use morph_core::office::{EnginePreference, Engines};
        use morph_core::state::State;
        use morph_formats::{EnabledCategories, MenuEntry, SelectionSummary, build_menu};
        let menu_for_paths = |files: &[PathBuf], enabled: &EnabledCategories| {
            build_menu(&SelectionSummary::from_paths(
                files.iter().map(PathBuf::as_path),
                enabled,
            ))
        };
        // Every leaf of every document menu opens the document panels with its
        // action (and the format preset) selected.
        let selections: [&[&str]; 9] = [
            &["a.doc"],
            &["a.docx"],
            &["a.xlsx"],
            &["a.csv"],
            &["a.ppt"],
            &["a.pptx"],
            &["a.md"],
            &["a.html"],
            &["a.docx", "b.xlsx", "c.txt"],
        ];
        for names in selections {
            let files: Vec<PathBuf> = names.iter().map(PathBuf::from).collect();
            let menu = menu_for_paths(&files, &Default::default());
            assert!(!menu.is_empty(), "{names:?}");
            for entry in menu {
                let MenuEntry::Item(item) = entry else {
                    continue;
                };
                let request = Request::new(files.clone(), item.action, item.preset);
                let expected = OfficeKind::from_id(item.action).map(Mode::Office);
                assert_eq!(
                    Some(mode_for(&request)),
                    expected,
                    "{names:?} {}",
                    item.label
                );
                let Some(Mode::Office(kind)) = expected else {
                    continue;
                };
                let form = OfficeForm::prefill(
                    &State::default(),
                    kind,
                    &request.preset,
                    &files,
                    &Engines::default(),
                    EnginePreference::Auto,
                );
                assert_eq!(form.kind, kind);
                if let Some((_, format)) = item.preset.iter().find(|(k, _)| *k == "format") {
                    let got = DocFormat::ALL
                        .into_iter()
                        .find(|f| f.extension() == *format);
                    assert_eq!(Some(form.convert.format), got, "{}", item.label);
                }
            }
        }
        let req = |action: &str, files: &[&str]| {
            Request::new(files.iter().map(PathBuf::from).collect(), action, &[])
        };
        assert_eq!(
            mode_for(&req(actions::MIXED_CONVERT, &["a.docx", "b.mp4"])),
            Mode::Office(OfficeKind::Convert)
        );
        assert_eq!(
            mode_for(&req(actions::OFFICE_TO_IMAGES, &["a.pptx"])),
            Mode::Office(OfficeKind::ToImages)
        );
    }

    #[test]
    fn image_and_pdf_menu_entries_preselect_their_target() {
        use crate::options::{FormValues, ImageForm, PDF_FORMAT};
        use crate::pdf_options::{PdfForm, PdfValues};
        use morph_core::state::State;
        use morph_formats::{MenuEntry, SelectionSummary, build_menu};
        let mut seen_to_pdf = false;
        let mut seen_to_markdown = false;
        let selections: [&[&str]; 3] = [&["a.jpg"], &["a.pdf"], &["a.pdf", "b.png"]];
        for names in selections {
            let files: Vec<PathBuf> = names.iter().map(PathBuf::from).collect();
            let menu = build_menu(&SelectionSummary::from_paths(
                files.iter().map(PathBuf::as_path),
                &Default::default(),
            ));
            for entry in menu {
                let MenuEntry::Item(item) = entry else {
                    continue;
                };
                let request = Request::new(files.clone(), item.action, item.preset);
                let mode = mode_for(&request);
                assert!(
                    !matches!(mode, Mode::Unavailable(_)),
                    "{names:?} {}",
                    item.label
                );
                match mode {
                    Mode::Images(kind) => {
                        let form = ImageForm::prefill(&State::default(), kind, &request.preset);
                        if item.action != actions::MIXED_CONVERT {
                            assert_eq!(form.action().id(), item.action, "{}", item.label);
                        }
                        if item.action == actions::IMAGE_TO_PDF {
                            seen_to_pdf = true;
                            let v = FormValues::from_form(&form);
                            assert_eq!((v.kind, v.format), (0, PDF_FORMAT), "PDF pre-selected");
                        }
                    }
                    Mode::Pdf(kind) => {
                        let form = PdfForm::prefill(&State::default(), kind, &request.preset);
                        if item.action != actions::MIXED_CONVERT {
                            assert_eq!(form.action().id(), item.action, "{}", item.label);
                        }
                        if item.action == actions::PDF_TO_MARKDOWN {
                            seen_to_markdown = true;
                            assert_eq!(
                                PdfValues::from_form(&form).kind,
                                PdfKind::ToMarkdown.index()
                            );
                        }
                    }
                    _ => {}
                }
            }
        }
        assert!(seen_to_pdf && seen_to_markdown, "the menus offer both");
    }

    #[test]
    fn video_and_audio_menu_entries_open_their_panel_preselected() {
        use crate::media_options::{MediaForm, MediaTab};
        use morph_core::media::options::{AudioFormat, VideoContainer};
        use morph_core::state::State;
        use morph_formats::{MenuEntry, SelectionSummary, build_menu};
        let selections: [&[&str]; 4] = [
            &["a.mp4"],
            &["a.mkv", "b.webm"],
            &["a.mp3"],
            &["a.flac", "b.wav"],
        ];
        let mut seen = Vec::new();
        for names in selections {
            let files: Vec<PathBuf> = names.iter().map(PathBuf::from).collect();
            let menu = build_menu(&SelectionSummary::from_paths(
                files.iter().map(PathBuf::as_path),
                &Default::default(),
            ));
            assert!(!menu.is_empty(), "{names:?}");
            for entry in menu {
                let MenuEntry::Item(item) = entry else {
                    continue;
                };
                let request = Request::new(files.clone(), item.action, item.preset);
                let mode = mode_for(&request);
                let form =
                    MediaForm::prefill(&State::default(), item.action, &request.preset, &files);
                match mode {
                    Mode::Video(kind) => {
                        assert_eq!(kind.id(), item.action, "{}", item.label);
                        assert_eq!((form.tab, form.video_kind), (MediaTab::Video, kind));
                        assert_eq!(form.video_action().id(), item.action);
                    }
                    Mode::Audio => {
                        assert_eq!(item.action, actions::AUDIO_CONVERT, "{}", item.label);
                        assert_eq!(form.tab, MediaTab::Audio);
                    }
                    other => panic!("{names:?} {}: {other:?}", item.label),
                }
                if let Some((_, format)) = item.preset.iter().find(|(k, _)| *k == "format") {
                    let got = if item.action == actions::AUDIO_CONVERT {
                        form.audio.format.extension()
                    } else {
                        form.convert.format.extension()
                    };
                    assert_eq!(got, *format, "{}", item.label);
                }
                seen.push(item.action);
            }
        }
        for id in [
            actions::VIDEO_CONVERT,
            actions::VIDEO_COMPRESS,
            actions::VIDEO_EXTRACT_AUDIO,
            actions::VIDEO_TO_GIF,
            actions::AUDIO_CONVERT,
        ] {
            assert!(seen.contains(&id), "{id} is in a menu");
        }
        // "Convert to WebM…" / "Convert to FLAC…".
        let webm = Request::new(
            vec![PathBuf::from("a.mp4")],
            actions::VIDEO_CONVERT,
            &[("format", "webm")],
        );
        let form = MediaForm::prefill(&State::default(), &webm.action, &webm.preset, &webm.files);
        assert_eq!(form.convert.format, VideoContainer::Webm);
        let flac = Request::new(
            vec![PathBuf::from("a.mp3")],
            actions::AUDIO_CONVERT,
            &[("format", "flac")],
        );
        let form = MediaForm::prefill(&State::default(), &flac.action, &flac.preset, &flac.files);
        assert_eq!(form.audio.format, AudioFormat::Flac);
    }

    #[test]
    fn natural_sort_and_reorder_keep_sizes_aligned() {
        let mut sel = Selection {
            files: ["10.pdf", "2.pdf", "B.jpg", "a.pdf", "1.PDF"]
                .iter()
                .map(|n| PathBuf::from(r"C:\x").join(n))
                .collect(),
            sizes: vec![Some(10), Some(2), Some(66), Some(65), None],
        };
        sel.sort_natural();
        let names: Vec<String> = sel.file_views().into_iter().map(|f| f.name).collect();
        assert_eq!(names, ["1.PDF", "2.pdf", "10.pdf", "a.pdf", "B.jpg"]);
        assert_eq!(sel.sizes, [None, Some(2), Some(10), Some(65), Some(66)]);

        assert!(sel.move_file(4, 0));
        assert!(sel.move_file(1, 3));
        let names: Vec<String> = sel.file_views().into_iter().map(|f| f.name).collect();
        assert_eq!(names, ["B.jpg", "2.pdf", "10.pdf", "1.PDF", "a.pdf"]);
        assert_eq!(sel.sizes, [Some(66), Some(2), Some(10), None, Some(65)]);
        assert!(!sel.move_file(0, 0));
        assert!(!sel.move_file(0, 9));

        // Merge takes images too, in this order; Split only the PDFs.
        let plan = |id: &str| {
            let action = Action::default_for(id).unwrap();
            single_job(
                &sel.files,
                &[],
                &[action],
                &Engines::default(),
                EnginePreference::Auto,
                &OutputSpec::default(),
            )
            .job
        };
        let merge = plan(actions::PDF_MERGE);
        assert_eq!(merge.tasks.len(), 1);
        assert_eq!(merge.tasks[0].inputs, sel.files);
        let split = plan(actions::PDF_SPLIT);
        assert_eq!((split.tasks.len(), split.skipped.len()), (4, 1));
    }

    fn paths(names: &[&str]) -> Vec<PathBuf> {
        names.iter().map(PathBuf::from).collect()
    }

    fn mixed(names: &[&str]) -> Request {
        Request::new(paths(names), actions::MIXED_CONVERT, &[])
    }

    #[test]
    fn mixed_selection_gets_one_tab_per_type_and_skips_the_rest() {
        let all_on = EnabledCategories::default();
        let o = open(
            &mixed(&["a.jpg", "b.pdf", "c.mp4", "d.docx", "x.exe", "e.txt"]),
            &all_on,
        );
        assert_eq!(o.mode, Mode::Mixed);
        use MenuGroup as G;
        assert_eq!(o.tabs, [G::Image, G::Pdf, G::Video, G::Office]);
        assert_eq!(
            o.files,
            paths(&["a.jpg", "b.pdf", "c.mp4", "d.docx", "e.txt"])
        );
        assert_eq!(
            o.skipped,
            [(
                PathBuf::from("x.exe"),
                morph_core::job::SKIP_UNSUPPORTED.to_owned()
            )]
        );
        let labels: Vec<String> = o.tabs.iter().map(|g| tab_label(*g, &o.files)).collect();
        assert_eq!(
            labels,
            ["Images (1)", "PDF (1)", "Video (1)", "Documents (2)"]
        );

        // Video turned off in Settings: no tab, the file is skipped with why.
        let no_video = EnabledCategories {
            video: false,
            ..all_on
        };
        let o = open(&mixed(&["a.jpg", "c.mp4", "b.pdf"]), &no_video);
        assert_eq!(o.tabs, [G::Image, G::Pdf]);
        assert_eq!(o.skipped[0].1, morph_core::job::SKIP_TURNED_OFF);

        // Video + audio: two top-level tabs.
        let o = open(&mixed(&["a.mp3", "b.mkv"]), &all_on);
        assert_eq!((o.mode, o.tabs), (Mode::Mixed, vec![G::Video, G::Audio]));

        // One type left: the single-type window, the rest skipped.
        let o = open(&mixed(&["a.jpg", "x.exe"]), &all_on);
        assert_eq!(o.mode, Mode::Images(ImageKind::Convert));
        assert!(o.tabs.is_empty());
        assert_eq!((o.files.len(), o.skipped.len()), (1, 1));
        let o = open(&mixed(&["a.mp4", "b.jpg"]), &no_video);
        assert_eq!(o.mode, Mode::Images(ImageKind::Convert));

        // Nothing supported (or everything turned off).
        let o = open(&mixed(&["x.exe"]), &all_on);
        assert_eq!(
            o.mode,
            Mode::Unavailable(strings::NO_SUPPORTED_FILES.into())
        );
        let o = open(&mixed(&["a.mp4"]), &no_video);
        assert!(matches!(o.mode, Mode::Unavailable(_)));

        // "Combine into one PDF…" on images + PDFs: Merge with both, no tabs.
        let merge = Request::new(paths(&["b.pdf", "a.jpg"]), actions::PDF_MERGE, &[]);
        let o = open(&merge, &all_on);
        assert_eq!(o.mode, Mode::Pdf(PdfKind::Merge));
        assert!(o.tabs.is_empty());
        assert_eq!(o.files.len(), 2);
    }

    #[test]
    fn dropped_folders_are_skipped_even_with_a_file_extension() {
        let dir = tempfile::tempdir().unwrap();
        let trip = dir.path().join("Trip.jpg");
        let plain = dir.path().join("Photos");
        std::fs::create_dir(&trip).unwrap();
        std::fs::create_dir(&plain).unwrap();
        let photo = dir.path().join("a.jpg");
        let request = Request::new(
            vec![trip.clone(), photo.clone(), plain.clone()],
            actions::MIXED_CONVERT,
            &[],
        );
        let o = open(&request, &EnabledCategories::default());
        assert_eq!(o.mode, Mode::Images(ImageKind::Convert));
        assert_eq!(o.files, [photo]);
        let why = strings::FOLDERS_NOT_SUPPORTED.to_owned();
        assert_eq!(o.skipped, [(trip, why.clone()), (plain, why)]);
    }

    #[test]
    fn removing_the_last_file_of_a_tab_removes_the_tab() {
        let mut files = paths(&["a.jpg", "b.pdf", "c.png"]);
        assert_eq!(tabs_present(&files), [MenuGroup::Image, MenuGroup::Pdf]);
        files.remove(1);
        assert_eq!(tabs_present(&files), [MenuGroup::Image]);
        assert_eq!(tab_label(MenuGroup::Image, &files), "Images (2)");
    }

    #[test]
    fn mixed_job_runs_each_tabs_action_in_tab_order() {
        let files = paths(&["c.mp4", "a.jpg", "t.xlsx", "b.pdf", "a2.jpg", "t.csv"]);
        let skipped = vec![(
            PathBuf::from("x.exe"),
            morph_core::job::SKIP_UNSUPPORTED.to_owned(),
        )];
        let action = |id: &str| Action::default_for(id).unwrap();
        let mut office = action(actions::OFFICE_CONVERT);
        if let Action::OfficeConvert(o) = &mut office {
            o.format = morph_core::office::options::DocFormat::Xlsx;
        }
        let tab_actions = vec![
            (MenuGroup::Image, action(actions::IMAGE_COMPRESS)),
            (MenuGroup::Pdf, action(actions::PDF_TO_TEXT)),
            (MenuGroup::Video, action(actions::VIDEO_TO_GIF)),
            (MenuGroup::Office, office),
        ];
        let output = OutputSpec {
            folder: Some(PathBuf::from(r"C:\out")),
        };
        let planned = mixed_job(
            &files,
            &skipped,
            &tab_actions,
            &[],
            "no ffmpeg",
            &Engines::default(),
            EnginePreference::Auto,
            &output,
        );
        let tasks: Vec<(u64, &str, &str)> = planned
            .job
            .tasks
            .iter()
            .map(|t| {
                (
                    t.id.0,
                    t.action.id(),
                    t.inputs[0].to_str().unwrap_or_default(),
                )
            })
            .collect();
        assert_eq!(
            tasks,
            [
                (0, actions::IMAGE_COMPRESS, "a.jpg"),
                (1, actions::IMAGE_COMPRESS, "a2.jpg"),
                (2, actions::PDF_TO_TEXT, "b.pdf"),
                (3, actions::VIDEO_TO_GIF, "c.mp4"),
                // CSV → XLSX is native: no engine needed.
                (4, actions::OFFICE_CONVERT, "t.csv"),
            ]
        );
        assert!(planned.job.tasks.iter().all(|t| t.output == output));
        // Already XLSX: skipped with the reason; then x.exe.
        let skipped_names: Vec<&str> = planned
            .job
            .skipped
            .iter()
            .map(|(p, _)| p.to_str().unwrap_or_default())
            .collect();
        assert_eq!(skipped_names, ["t.xlsx", "x.exe"]);
        assert_eq!(planned.runs().len(), 5);

        // DOCX → PDF without Office or LibreOffice: the Documents tab's banner.
        let to_pdf = vec![(MenuGroup::Office, action(actions::OFFICE_CONVERT))];
        let files = paths(&["c.mp4", "a.jpg", "d.docx"]);
        let planned = mixed_job(
            &files,
            &[],
            &to_pdf,
            &[],
            "no ffmpeg",
            &Engines::default(),
            EnginePreference::Auto,
            &OutputSpec::default(),
        );
        assert_eq!(planned.engine(Some(MenuGroup::Office)).0, 1);
        assert_eq!(planned.engine(Some(MenuGroup::Image)).0, 0);
        // Tabs without an action (no FFmpeg): their files skipped with why.
        let reasons: Vec<(&str, &str)> = planned
            .job
            .skipped
            .iter()
            .map(|(p, r)| (p.to_str().unwrap_or_default(), r.as_str()))
            .collect();
        assert!(reasons.contains(&("c.mp4", "no ffmpeg")));
        assert!(reasons.contains(&("d.docx", strings::NEEDS_ENGINE)));
    }

    #[test]
    fn unchecked_types_are_skipped_by_choice() {
        let files = paths(&["a.jpg", "b.pdf", "c.png", "d.mp4"]);
        let action = |id: &str| Action::default_for(id).unwrap();
        let tab_actions = vec![
            (MenuGroup::Image, action(actions::IMAGE_CONVERT)),
            (MenuGroup::Pdf, action(actions::PDF_COMPRESS)),
            (MenuGroup::Video, action(actions::VIDEO_CONVERT)),
        ];
        let plan = |excluded: &[MenuGroup]| {
            mixed_job(
                &files,
                &[],
                &tab_actions,
                excluded,
                "no ffmpeg",
                &Engines::default(),
                EnginePreference::Auto,
                &OutputSpec::default(),
            )
        };
        let all = plan(&[]);
        assert_eq!(all.runs().len(), 4);

        let planned = plan(&[MenuGroup::Image, MenuGroup::Video]);
        let ran: Vec<&str> = planned
            .job
            .tasks
            .iter()
            .map(|t| t.inputs[0].to_str().unwrap_or_default())
            .collect();
        assert_eq!(ran, ["b.pdf"]);
        // Skipped in tab order, each with the plain reason.
        let skipped: Vec<(&str, &str)> = planned
            .job
            .skipped
            .iter()
            .map(|(p, r)| (p.to_str().unwrap_or_default(), r.as_str()))
            .collect();
        let images = strings::skipped_by_choice(strings::TYPE_IMAGES);
        let video = strings::skipped_by_choice(strings::TYPE_VIDEO);
        assert_eq!(
            skipped,
            [
                ("a.jpg", images.as_str()),
                ("c.png", images.as_str()),
                ("d.mp4", video.as_str()),
            ]
        );
        // Unchecked wins over "no action" (no FFmpeg).
        let no_video_action = mixed_job(
            &files,
            &[],
            &tab_actions[..2],
            &[MenuGroup::Video],
            "no ffmpeg",
            &Engines::default(),
            EnginePreference::Auto,
            &OutputSpec::default(),
        );
        assert!(
            no_video_action
                .job
                .skipped
                .contains(&(PathBuf::from("d.mp4"), video))
        );
        // Everything unchecked: nothing runs.
        let none = plan(&[MenuGroup::Image, MenuGroup::Pdf, MenuGroup::Video]);
        assert!(none.job.tasks.is_empty());
        assert_eq!(none.job.skipped.len(), 4);
    }

    #[test]
    fn category_cards_summarize_what_runs() {
        use morph_core::image::options::OutputFormat;
        use morph_core::office::options::DocFormat;
        let action = |id: &str| Action::default_for(id).unwrap();
        let mut webp = action(actions::IMAGE_CONVERT);
        if let Action::ImageConvert(o) = &mut webp {
            o.format = OutputFormat::Webp;
        }
        assert_eq!(action_summary(&webp), "→ WebP");
        assert_eq!(action_summary(&action(actions::IMAGE_COMPRESS)), "Compress");
        assert_eq!(action_summary(&action(actions::IMAGE_TO_PDF)), "→ PDF");
        assert_eq!(action_summary(&action(actions::PDF_TO_WORD)), "→ Word");
        assert_eq!(action_summary(&action(actions::PDF_SPLIT)), "Split");
        let mut docx = action(actions::OFFICE_CONVERT);
        if let Action::OfficeConvert(o) = &mut docx {
            o.format = DocFormat::Docx;
        }
        assert_eq!(action_summary(&docx), "→ DOCX");
        assert_eq!(action_summary(&action(actions::VIDEO_CONVERT)), "→ MP4");
        assert_eq!(action_summary(&action(actions::VIDEO_TO_GIF)), "→ GIF");
        assert_eq!(action_summary(&action(actions::AUDIO_CONVERT)), "→ MP3");
        // Every action has a non-empty one-liner.
        use actions as a;
        for id in [
            a::IMAGE_CONVERT,
            a::IMAGE_COMPRESS,
            a::IMAGE_RESIZE,
            a::IMAGE_CLEAN,
            a::IMAGE_TO_PDF,
            a::PDF_COMPRESS,
            a::PDF_TO_IMAGES,
            a::PDF_TO_WORD,
            a::PDF_TO_TEXT,
            a::PDF_TO_MARKDOWN,
            a::PDF_SPLIT,
            a::PDF_MERGE,
            a::VIDEO_CONVERT,
            a::VIDEO_COMPRESS,
            a::VIDEO_EXTRACT_AUDIO,
            a::VIDEO_TO_GIF,
            a::AUDIO_CONVERT,
            a::OFFICE_CONVERT,
            a::OFFICE_TO_IMAGES,
        ] {
            let action = Action::default_for(id).unwrap();
            assert!(!action_summary(&action).is_empty(), "{id}");
        }
        assert_eq!(group_name(MenuGroup::Office), "Documents");
        assert_eq!(strings::file_count(1), "1 file");
        assert_eq!(strings::file_count(12), "12 files");
    }

    #[test]
    fn selection_size_of_counts_only_the_given_files() {
        let sel = Selection {
            files: paths(&["a.jpg", "b.jpg", "c.jpg"]),
            sizes: vec![Some(1024), None, Some(512)],
        };
        assert_eq!(sel.size_of(&[0, 1, 2]), 1536);
        assert_eq!(sel.size_of(&[1]), 0);
        assert_eq!(sel.size_of(&[]), 0);
    }

    #[test]
    fn every_tab_starts_from_its_remembered_options() {
        use crate::media_options::MediaForm;
        use crate::options::ImageForm;
        use crate::pdf_options::PdfForm;
        use morph_core::state::State;
        let mut state = State::default();
        state.remember = true;
        let mut webp = Action::default_for(actions::IMAGE_CONVERT).unwrap();
        if let Action::ImageConvert(o) = &mut webp {
            o.format = morph_core::image::options::OutputFormat::Webp;
        }
        let mut flac = Action::default_for(actions::AUDIO_CONVERT).unwrap();
        if let Action::AudioConvert(o) = &mut flac {
            o.format = morph_core::media::options::AudioFormat::Flac;
        }
        let mut pdf = Action::default_for(actions::PDF_COMPRESS).unwrap();
        if let Action::PdfCompress(o) = &mut pdf {
            o.preset = morph_core::pdf::options::CompressPreset::High;
        }
        for a in [&webp, &flac, &pdf] {
            state.remember_action(a);
        }
        let none = Default::default();
        let files = paths(&["a.jpg", "b.pdf", "c.mp3", "d.mp4"]);
        assert_eq!(
            ImageForm::prefill(&state, ImageKind::Convert, &none).action(),
            webp
        );
        assert_eq!(
            PdfForm::prefill(&state, PdfKind::Compress, &none).action(),
            pdf
        );
        let media = MediaForm::prefill(&state, actions::VIDEO_CONVERT, &none, &files);
        assert_eq!(media.audio_action(), flac);
    }
}
