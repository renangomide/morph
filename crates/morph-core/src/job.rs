// SPDX-License-Identifier: GPL-3.0-or-later
//! Jobs and tasks.

use crate::action::Action;
use morph_formats::{Category, EnabledCategories, MenuGroup};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct TaskId(pub u64);

/// Where outputs go.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutputSpec {
    /// `None` = next to each input file (default).
    pub folder: Option<PathBuf>,
}

impl OutputSpec {
    /// Output folder for a given input.
    pub fn folder_for(&self, input: &Path) -> PathBuf {
        match &self.folder {
            Some(folder) => folder.clone(),
            None => input
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| PathBuf::from(".")),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Task {
    pub id: TaskId,
    /// Usually one file; several for merge.
    pub inputs: Vec<PathBuf>,
    pub action: Action,
    pub output: OutputSpec,
}

impl Task {
    /// The (first) input file.
    pub fn input(&self) -> Option<&Path> {
        self.inputs.first().map(PathBuf::as_path)
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Job {
    pub tasks: Vec<Task>,
    /// Inputs that won't be processed, with the reason (reported as `TaskSkipped`).
    pub skipped: Vec<(PathBuf, String)>,
    /// Settings → "Keep original file dates": outputs get the (first) input's
    /// modified and created times.
    pub keep_dates: bool,
}

/// Skip reasons for files a mixed selection can't take (core text, like notes).
pub const SKIP_UNSUPPORTED: &str = "This file type isn't supported.";
pub const SKIP_TURNED_OFF: &str = "This file type is turned off in Settings.";
pub const SKIP_NOT_FOR_ACTION: &str = "Not supported for this action.";

/// A selection split by type (SPEC §3.1 mixed selection, §3.4 drop zone): one
/// group per [`MenuGroup`] present (the window's tabs), in a fixed order
/// (images, PDF, video, audio, documents), files in selection order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GroupedFiles {
    pub groups: Vec<(MenuGroup, Vec<PathBuf>)>,
    /// Unsupported files and files of a type turned off in Settings.
    pub skipped: Vec<(PathBuf, String)>,
}

impl GroupedFiles {
    /// More than one type: the window shows tabs.
    pub fn is_mixed(&self) -> bool {
        self.groups.len() > 1
    }
}

/// Split `files` by type (extension only, like the menu).
pub fn group_files(files: &[PathBuf], enabled: &EnabledCategories) -> GroupedFiles {
    const ORDER: [MenuGroup; 5] = [
        MenuGroup::Image,
        MenuGroup::Pdf,
        MenuGroup::Video,
        MenuGroup::Audio,
        MenuGroup::Office,
    ];
    let mut by_group: [Vec<PathBuf>; 5] = Default::default();
    let mut skipped = Vec::new();
    for file in files {
        match Category::from_path(file) {
            Some(cat) if enabled.is_enabled(cat) => {
                if let Some(slot) = ORDER.iter().position(|g| *g == cat.menu_group()) {
                    by_group[slot].push(file.clone());
                }
            }
            Some(_) => skipped.push((file.clone(), SKIP_TURNED_OFF.to_owned())),
            None => skipped.push((file.clone(), SKIP_UNSUPPORTED.to_owned())),
        }
    }
    GroupedFiles {
        groups: ORDER
            .into_iter()
            .zip(by_group)
            .filter(|(_, files)| !files.is_empty())
            .collect(),
        skipped,
    }
}

impl Job {
    /// One task per input whose category the action accepts; others are skipped.
    /// Multi-input actions (merge) get a single task with all accepted inputs.
    pub fn for_files(files: &[PathBuf], action: &Action, output: &OutputSpec) -> Job {
        let mut job = Job::default();
        let mut accepted = Vec::new();
        for file in files {
            match morph_formats::Category::from_path(file) {
                Some(cat) if action.accepts(cat) => accepted.push(file.clone()),
                _ => job
                    .skipped
                    .push((file.clone(), SKIP_NOT_FOR_ACTION.to_owned())),
            }
        }
        if action.is_multi_input() {
            if !accepted.is_empty() {
                job.tasks.push(Task {
                    id: TaskId(0),
                    inputs: accepted,
                    action: action.clone(),
                    output: output.clone(),
                });
            }
        } else {
            for (i, file) in accepted.into_iter().enumerate() {
                job.tasks.push(Task {
                    id: TaskId(i as u64),
                    inputs: vec![file],
                    action: action.clone(),
                    output: output.clone(),
                });
            }
        }
        job
    }

    /// One job from several (a mixed selection: one per tab). Tasks keep
    /// their order and get new IDs `0..n`; skipped files are kept in order.
    /// `keep_dates` is on if any part has it.
    pub fn combine(parts: impl IntoIterator<Item = Job>) -> Job {
        let mut job = Job::default();
        for part in parts {
            job.tasks.extend(part.tasks);
            job.skipped.extend(part.skipped);
            job.keep_dates |= part.keep_dates;
        }
        for (i, task) in job.tasks.iter_mut().enumerate() {
            task.id = TaskId(i as u64);
        }
        job
    }

    /// Mixed selection in one job: `plan` makes each group's part, in group
    /// order (e.g. [`Job::for_files`] with that tab's action, or
    /// [`Job::skip_all`]), then [`Job::combine`]; `grouped.skipped` comes last.
    pub fn for_groups(
        grouped: &GroupedFiles,
        mut plan: impl FnMut(MenuGroup, &[PathBuf]) -> Job,
    ) -> Job {
        let mut parts: Vec<Job> = grouped
            .groups
            .iter()
            .map(|(group, files)| plan(*group, files))
            .collect();
        parts.push(Job::skip_all_with(grouped.skipped.clone()));
        Job::combine(parts)
    }

    /// Nothing to run: every file skipped with `reason`.
    pub fn skip_all(files: &[PathBuf], reason: &str) -> Job {
        Job::skip_all_with(
            files
                .iter()
                .map(|f| (f.clone(), reason.to_owned()))
                .collect(),
        )
    }

    fn skip_all_with(skipped: Vec<(PathBuf, String)>) -> Job {
        Job {
            skipped,
            ..Job::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use morph_formats::actions;

    #[test]
    fn merge_is_one_task_in_given_order() {
        let files: Vec<PathBuf> = [r"C:\a\10.pdf", r"C:\b\photo.jpg", r"C:\a\2.pdf", "x.exe"]
            .iter()
            .map(PathBuf::from)
            .collect();
        let action = Action::default_for(actions::PDF_MERGE).unwrap();
        let job = Job::for_files(&files, &action, &OutputSpec::default());
        assert_eq!(job.tasks.len(), 1);
        assert_eq!(job.tasks[0].inputs, files[..3]);
        assert_eq!(job.skipped.len(), 1);
        // Merged.pdf goes in the first input's folder.
        let first = job.tasks[0].input().unwrap();
        assert_eq!(job.tasks[0].output.folder_for(first), Path::new(r"C:\a"));
    }

    #[test]
    fn mixed_selection_of_four_types_is_one_job() {
        let files: Vec<PathBuf> = [
            r"C:\m\a.jpg",
            r"C:\m\b.pdf",
            r"C:\m\c.mp4",
            r"C:\m\d.docx",
            r"C:\m\e.png",
            r"C:\m\f.exe",
            r"C:\m\g.mp3",
        ]
        .iter()
        .map(PathBuf::from)
        .collect();
        let enabled = EnabledCategories {
            audio: false,
            ..EnabledCategories::default()
        };
        let grouped = group_files(&files, &enabled);
        assert!(grouped.is_mixed());
        let groups: Vec<MenuGroup> = grouped.groups.iter().map(|(g, _)| *g).collect();
        assert_eq!(
            groups,
            [
                MenuGroup::Image,
                MenuGroup::Pdf,
                MenuGroup::Video,
                MenuGroup::Office
            ]
        );
        assert_eq!(grouped.groups[0].1, [files[0].clone(), files[4].clone()]);
        assert_eq!(
            grouped.skipped,
            [
                (files[5].clone(), SKIP_UNSUPPORTED.to_owned()),
                (files[6].clone(), SKIP_TURNED_OFF.to_owned()),
            ]
        );

        let def = |id| Action::default_for(id).unwrap();
        let actions_by_group = [
            (MenuGroup::Image, def(actions::IMAGE_CONVERT)),
            (MenuGroup::Pdf, def(actions::PDF_COMPRESS)),
            (MenuGroup::Video, def(actions::VIDEO_COMPRESS)),
            (MenuGroup::Office, def(actions::OFFICE_CONVERT)),
        ];
        let plan = |actions: &[(MenuGroup, Action)]| {
            Job::for_groups(&grouped, |group, files| {
                match actions.iter().find(|(g, _)| *g == group) {
                    Some((_, action)) => Job::for_files(files, action, &OutputSpec::default()),
                    None => Job::skip_all(files, SKIP_NOT_FOR_ACTION),
                }
            })
        };
        let job = plan(&actions_by_group);
        let ids: Vec<&str> = job.tasks.iter().map(|t| t.action.id()).collect();
        assert_eq!(
            ids,
            [
                actions::IMAGE_CONVERT,
                actions::IMAGE_CONVERT,
                actions::PDF_COMPRESS,
                actions::VIDEO_COMPRESS,
                actions::OFFICE_CONVERT
            ]
        );
        let ordered = job.tasks.iter().enumerate();
        assert!(ordered.into_iter().all(|(i, t)| t.id == TaskId(i as u64)));
        assert_eq!(job.skipped.len(), 2);

        // A group without an action: its files are skipped.
        let job = plan(&actions_by_group[..1]);
        assert_eq!((job.tasks.len(), job.skipped.len()), (2, 5));
    }

    #[test]
    fn pdf_actions_take_one_task_per_pdf() {
        let files = [PathBuf::from("a.pdf"), PathBuf::from("b.jpg")];
        let action = Action::default_for(actions::PDF_SPLIT).unwrap();
        let job = Job::for_files(&files, &action, &OutputSpec::default());
        assert_eq!(job.tasks.len(), 1);
        assert_eq!(job.skipped.len(), 1, "images aren't split");
    }
}
