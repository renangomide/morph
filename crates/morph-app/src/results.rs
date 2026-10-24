// SPDX-License-Identifier: GPL-3.0-or-later
//! Saved results of a finished job (`<LocalData>\results\<id>.json`), so a
//! toast click can show them after the process has exited (SPEC §3.3).

use serde::{Deserialize, Serialize};
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// How long saved results are kept.
const KEEP_FOR: Duration = Duration::from_secs(7 * 24 * 3600);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedResults {
    /// e.g. "11 converted, 1 failed · saved 38.2 MB".
    pub summary: String,
    /// Folder for the "Open folder" button.
    pub folder: Option<PathBuf>,
    pub rows: Vec<SavedRow>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedRow {
    /// The (first) input.
    pub input: PathBuf,
    /// Number of inputs (several for Merge; 0 in files from older versions = 1).
    #[serde(default)]
    pub inputs: usize,
    pub status: RowStatus,
    /// Files written (empty when failed, skipped or nothing to do).
    pub outputs: Vec<PathBuf>,
    /// Sizes, e.g. "4.1 MB → 0.9 MB" (older files: "… · 78% smaller").
    pub sizes: String,
    /// Note, skip reason, or the failure message.
    pub message: String,
    /// Copyable technical details of a failure.
    pub details: String,
    /// Target pill, e.g. "→ WebP", "→ Merged.pdf" (empty in older files).
    #[serde(default)]
    pub target: String,
    /// Size change in percent (−78 = smaller), for the change pill.
    #[serde(default)]
    pub change: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RowStatus {
    Done,
    Failed,
    Skipped,
    Cancelled,
}

/// A new id for a results file.
pub fn new_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

fn path_for(dir: &Path, id: &str) -> Option<PathBuf> {
    // Ids come from the command line: accept only what `new_id` makes.
    let ok =
        !id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_ascii_hexdigit() || c == '-');
    ok.then(|| dir.join(format!("{id}.json")))
}

impl SavedResults {
    /// Save under `dir` (normally `paths::results_dir`) and delete files older than 7 days.
    pub fn save(&self, dir: &Path, id: &str) -> io::Result<()> {
        let path = path_for(dir, id).ok_or_else(|| io::Error::other("bad results id"))?;
        std::fs::create_dir_all(dir)?;
        sweep(dir);
        let json = serde_json::to_vec_pretty(self).map_err(io::Error::other)?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, json)?;
        std::fs::rename(&tmp, &path).inspect_err(|_| {
            let _ = std::fs::remove_file(&tmp);
        })
    }

    pub fn load(dir: &Path, id: &str) -> io::Result<SavedResults> {
        let path = path_for(dir, id).ok_or_else(|| io::Error::other("bad results id"))?;
        let bytes = std::fs::read(path)?;
        serde_json::from_slice(&bytes).map_err(io::Error::other)
    }
}

fn sweep(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let now = SystemTime::now();
    for entry in entries.flatten() {
        let old = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| now.duration_since(t).ok())
            .is_some_and(|age| age > KEEP_FOR);
        if old {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_rejects_bad_ids() {
        let dir = tempfile::tempdir().unwrap();
        let results = SavedResults {
            summary: "1 converted, 1 failed".into(),
            folder: Some(PathBuf::from(r"C:\Fotos")),
            rows: vec![SavedRow {
                input: PathBuf::from(r"C:\Fotos\a.jpg"),
                inputs: 1,
                status: RowStatus::Failed,
                outputs: vec![],
                sizes: String::new(),
                message: "This file seems to be damaged.".into(),
                details: "unexpected end of file".into(),
                target: "→ WebP".into(),
                change: None,
            }],
        };
        let id = new_id();
        results.save(dir.path(), &id).unwrap();
        assert_eq!(SavedResults::load(dir.path(), &id).unwrap(), results);
        assert!(SavedResults::load(dir.path(), r"..\..\x").is_err());
        assert!(results.save(dir.path(), "a/b").is_err());
    }
}
