// SPDX-License-Identifier: GPL-3.0-or-later
//! Output naming and safe writing (SPEC §5.4).
//!
//! - New extension: `photo.jpg → photo.webp`; same extension: `photo (compressed).jpg`.
//! - Never overwrite: `photo (1).webp`, `photo (2).webp`, …
//! - Write to a hidden temp file in the target folder, then rename without
//!   replacing. Dropping an uncommitted [`OutputFile`] deletes the temp file.
//! - Input files are never touched.
//! - Temp files a killed job left behind (`.~morph-<uuid>.tmp`, over a day
//!   old) are deleted the first time this process writes into that folder.

use crate::error::ConvertError;
use std::ffi::OsStr;
use std::fs::File;
use std::io::Write;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime};
use windows::Win32::Storage::FileSystem::{
    FILE_ATTRIBUTE_HIDDEN, FILE_ATTRIBUTE_NORMAL, MOVEFILE_WRITE_THROUGH, MoveFileExW,
    SetFileAttributesW,
};
use windows::core::PCWSTR;

/// Suffix for outputs that keep the input's extension.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Suffix {
    /// Convert to the same format (e.g. JPG → JPG with new quality).
    Converted,
    Compressed,
    Resized,
    Clean,
}

impl Suffix {
    pub fn text(self) -> &'static str {
        match self {
            Suffix::Converted => "converted",
            Suffix::Compressed => "compressed",
            Suffix::Resized => "resized",
            Suffix::Clean => "clean",
        }
    }
}

fn stem(input: &Path) -> String {
    input
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "output".to_owned())
}

fn same_extension(input: &Path, ext: &str) -> bool {
    input
        .extension()
        .and_then(OsStr::to_str)
        .is_some_and(|e| e.eq_ignore_ascii_case(ext))
}

/// Desired output file name (before collision handling).
/// The suffix is added only when the extension doesn't change.
pub fn output_name(input: &Path, ext: &str, suffix: Suffix) -> String {
    let stem = stem(input);
    if same_extension(input, ext) {
        format!("{stem} ({}).{ext}", suffix.text())
    } else {
        format!("{stem}.{ext}")
    }
}

/// Folder for actions with many outputs (SPEC §5.4): `name (pages)`, `name (split)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MultiOutput {
    /// PDF → images.
    Pages,
    /// PDF split.
    Split,
    /// PowerPoint → images.
    Slides,
}

impl MultiOutput {
    pub fn text(self) -> &'static str {
        match self {
            MultiOutput::Pages => "pages",
            MultiOutput::Split => "split",
            MultiOutput::Slides => "slides",
        }
    }
}

/// `report.pdf` → `report (pages)`. Create it with [`OutputDir::create`].
pub fn multi_output_dir_name(input: &Path, kind: MultiOutput) -> String {
    format!("{} ({})", stem(input), kind.text())
}

/// One page of a multi-page output: `report-01.png`. As many digits as `total`
/// has, at least 2 (3 pages → `-01`, 150 → `-001`), so names sort in page order.
pub fn page_file_name(input: &Path, page: u32, total: u32, ext: &str) -> String {
    let width = total.max(1).to_string().len().max(2);
    format!("{}-{page:0width$}.{ext}", stem(input))
}

/// One part of a split: `report-p1-3.pdf`, or `report-p7.pdf` for one page.
pub fn split_file_name(input: &Path, first: u32, last: u32) -> String {
    if first == last {
        format!("{}-p{first}.pdf", stem(input))
    } else {
        format!("{}-p{first}-{last}.pdf", stem(input))
    }
}

/// Output of Merge / Combine into one PDF, in the first input's folder
/// (collisions: `Merged (1).pdf`).
pub const MERGED_FILE_NAME: &str = "Merged.pdf";

/// Natural file-name order (`2.pdf` before `10.pdf`, case-insensitive), the
/// starting order of the Merge list (SPEC §3.2).
pub fn natural_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    let (mut a, mut b) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (a.peek().copied(), b.peek().copied()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let take = |it: &mut std::iter::Peekable<std::str::Chars<'_>>| {
                    let mut digits = String::new();
                    while let Some(c) = it.peek().copied().filter(char::is_ascii_digit) {
                        digits.push(c);
                        it.next();
                    }
                    digits
                };
                let (da, db) = (take(&mut a), take(&mut b));
                let (ta, tb) = (da.trim_start_matches('0'), db.trim_start_matches('0'));
                let ord = ta
                    .len()
                    .cmp(&tb.len())
                    .then_with(|| ta.cmp(tb))
                    .then_with(|| da.len().cmp(&db.len()));
                if ord != Ordering::Equal {
                    return ord;
                }
            }
            (Some(x), Some(y)) => {
                let ord = x.to_lowercase().cmp(y.to_lowercase());
                if ord != Ordering::Equal {
                    return ord;
                }
                a.next();
                b.next();
            }
        }
    }
}

/// Sort paths by file name in natural order (see [`natural_cmp`]).
pub fn sort_natural(paths: &mut [PathBuf]) {
    let name = |p: &PathBuf| {
        p.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    };
    paths.sort_by(|a, b| natural_cmp(&name(a), &name(b)));
}

/// A new output folder (`name`, `name (1)`, …) for multi-output actions.
/// Write files into it with [`OutputFile`]; call [`OutputDir::commit`] when
/// every file is written. Dropped without commit (error, cancel), the folder
/// and everything in it is deleted: it was created by us, so no user file is lost.
pub struct OutputDir {
    path: PathBuf,
    committed: bool,
}

impl OutputDir {
    pub fn create(folder: &Path, name: &str) -> Result<OutputDir, ConvertError> {
        Ok(OutputDir {
            path: create_unique_dir(folder, name)?,
            committed: false,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Keep the folder. Returns its path.
    pub fn commit(mut self) -> PathBuf {
        self.committed = true;
        std::mem::take(&mut self.path)
    }
}

impl Drop for OutputDir {
    fn drop(&mut self) {
        if !self.committed {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }
}

/// `folder/name`, or `folder/name (1)`, `(2)`… if taken. `name` may have an extension.
pub fn unique_path(folder: &Path, name: &str) -> PathBuf {
    let candidate = folder.join(name);
    if !candidate.exists() {
        return candidate;
    }
    let (base, ext) = match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i..]),
        _ => (name, ""),
    };
    (1u32..)
        .map(|n| folder.join(format!("{base} ({n}){ext}")))
        .find(|p| !p.exists())
        .unwrap_or(candidate)
}

/// Create a fresh folder for multi-output actions: `folder/name`, or `name (1)`…
pub fn create_unique_dir(folder: &Path, name: &str) -> Result<PathBuf, ConvertError> {
    for n in 0u32..10_000 {
        let path = if n == 0 {
            folder.join(name)
        } else {
            folder.join(format!("{name} ({n})"))
        };
        match std::fs::create_dir(&path) {
            Ok(()) => return Ok(path),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e.into()),
        }
    }
    Err(ConvertError::other(
        "Couldn't create the output folder.",
        "too many existing folders",
    ))
}

fn wide(path: &Path) -> Vec<u16> {
    path.as_os_str().encode_wide().chain([0]).collect()
}

const TEMP_PREFIX: &str = ".~morph-";
const TEMP_SUFFIX: &str = ".tmp";
/// Our temp files older than this are leftovers of a killed job.
const STALE_TEMP_AGE: Duration = Duration::from_secs(24 * 60 * 60);

/// `.~morph-<hyphenated uuid>.tmp`, exactly.
fn is_temp_name(name: &str) -> bool {
    name.strip_prefix(TEMP_PREFIX)
        .and_then(|rest| rest.strip_suffix(TEMP_SUFFIX))
        .is_some_and(|id| id.len() == 36 && uuid::Uuid::try_parse(id).is_ok())
}

/// [`remove_stale_temps`] for `folder`, the first time only (per process).
fn sweep_once(folder: &Path) {
    static SWEPT: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());
    let Ok(mut swept) = SWEPT.lock() else {
        return;
    };
    if swept.iter().any(|f| f == folder) {
        return;
    }
    swept.push(folder.to_path_buf());
    drop(swept);
    remove_stale_temps(folder, STALE_TEMP_AGE);
}

/// Delete our temp files (only [`is_temp_name`] names) in `folder` that
/// weren't modified for `age`. Errors are ignored.
fn remove_stale_temps(folder: &Path, age: Duration) {
    let Ok(entries) = std::fs::read_dir(folder) else {
        return;
    };
    let now = SystemTime::now();
    for entry in entries.flatten() {
        if !entry.file_name().to_str().is_some_and(is_temp_name) {
            continue;
        }
        let old = entry
            .metadata()
            .ok()
            .filter(|m| m.is_file())
            .and_then(|m| m.modified().ok())
            .and_then(|t| now.duration_since(t).ok())
            .is_some_and(|d| d >= age);
        if old {
            let path = entry.path();
            // Our temp files are hidden; clear that so the delete can't be refused.
            let w = wide(&path);
            // SAFETY: NUL-terminated path.
            let _ = unsafe { SetFileAttributesW(PCWSTR(w.as_ptr()), FILE_ATTRIBUTE_NORMAL) };
            let _ = std::fs::remove_file(path);
        }
    }
}

/// A file being written. Call [`OutputFile::commit`] to give it its final name.
pub struct OutputFile {
    temp: PathBuf,
    folder: PathBuf,
    name: String,
    file: Option<File>,
    committed: bool,
}

impl OutputFile {
    /// Start writing `name` (e.g. `photo.webp`) in `folder`.
    pub fn create(folder: &Path, name: &str) -> Result<OutputFile, ConvertError> {
        sweep_once(folder);
        let temp = folder.join(format!(
            "{TEMP_PREFIX}{}{TEMP_SUFFIX}",
            uuid::Uuid::new_v4()
        ));
        let file = File::create_new(&temp).map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound
            | std::io::ErrorKind::PermissionDenied
            | std::io::ErrorKind::ReadOnlyFilesystem => ConvertError::not_writable(e),
            _ => e.into(),
        })?;
        let w = wide(&temp);
        // SAFETY: NUL-terminated path. Failure only means the temp file isn't hidden.
        let _ = unsafe { SetFileAttributesW(PCWSTR(w.as_ptr()), FILE_ATTRIBUTE_HIDDEN) };
        Ok(OutputFile {
            temp,
            folder: folder.to_path_buf(),
            name: name.to_owned(),
            file: Some(file),
            committed: false,
        })
    }

    /// Path of the temp file (for engines that write by path; close the handle first
    /// with [`OutputFile::close_handle`]).
    pub fn temp_path(&self) -> &Path {
        &self.temp
    }

    /// Close our handle so another program can write the temp file by path.
    pub fn close_handle(&mut self) {
        self.file = None;
    }

    pub fn write_all(&mut self, bytes: &[u8]) -> Result<(), ConvertError> {
        match self.file.as_mut() {
            Some(f) => f.write_all(bytes).map_err(ConvertError::from),
            None => std::fs::write(&self.temp, bytes).map_err(ConvertError::from),
        }
    }

    /// Flush, then rename to a free name (`name`, `name (1)`, …). Never replaces a file.
    pub fn commit(mut self) -> Result<PathBuf, ConvertError> {
        if let Some(f) = self.file.take() {
            f.sync_all()?;
        }
        let w_temp = wide(&self.temp);
        for _ in 0..100 {
            let target = unique_path(&self.folder, &self.name);
            let w_target = wide(&target);
            // SAFETY: NUL-terminated paths. Without MOVEFILE_REPLACE_EXISTING the
            // call fails if `target` appeared in the meantime; then we retry.
            let moved = unsafe {
                MoveFileExW(
                    PCWSTR(w_temp.as_ptr()),
                    PCWSTR(w_target.as_ptr()),
                    MOVEFILE_WRITE_THROUGH,
                )
            };
            match moved {
                Ok(()) => {
                    // SAFETY: NUL-terminated path; un-hide the final file.
                    let _ = unsafe {
                        SetFileAttributesW(PCWSTR(w_target.as_ptr()), FILE_ATTRIBUTE_NORMAL)
                    };
                    self.committed = true;
                    return Ok(target);
                }
                Err(_) if target.exists() => continue, // lost a race; pick the next name
                Err(e) => return Err(ConvertError::other("Couldn't save the result.", e)),
            }
        }
        Err(ConvertError::other(
            "Couldn't save the result.",
            "no free output name",
        ))
    }
}

impl Drop for OutputFile {
    fn drop(&mut self) {
        if !self.committed {
            self.file = None;
            let _ = std::fs::remove_file(&self.temp);
        }
    }
}

/// Copy a finished file (e.g. an engine's output in the task's temp folder) to
/// `folder/name` safely: temp file in `folder`, then rename to a free name
/// (`name (1)`, …). Works across drives. Returns the final path.
pub fn publish_file(source: &Path, folder: &Path, name: &str) -> Result<PathBuf, ConvertError> {
    let mut reader = File::open(source).map_err(|e| {
        ConvertError::other(
            "Couldn't save the result.",
            format!("{}: {e}", source.display()),
        )
    })?;
    let mut out = OutputFile::create(folder, name)?;
    // Stream through our open handle (CopyFile can't overwrite the hidden temp file).
    match out.file.as_mut() {
        Some(f) => std::io::copy(&mut reader, f).map(|_| ())?,
        None => {
            return Err(ConvertError::other(
                "Couldn't save the result.",
                "no temp file",
            ));
        }
    }
    out.commit()
}

/// Write `bytes` as `folder/name` safely. Returns the final path.
pub fn write_output(folder: &Path, name: &str, bytes: &[u8]) -> Result<PathBuf, ConvertError> {
    let mut out = OutputFile::create(folder, name)?;
    out.write_all(bytes)?;
    out.commit()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_follow_spec() {
        let p = Path::new(r"C:\x\photo.jpg");
        assert_eq!(output_name(p, "webp", Suffix::Compressed), "photo.webp");
        assert_eq!(
            output_name(p, "jpg", Suffix::Compressed),
            "photo (compressed).jpg"
        );
        assert_eq!(
            output_name(Path::new("PHOTO.JPG"), "jpg", Suffix::Resized),
            "PHOTO (resized).jpg"
        );
        assert_eq!(
            output_name(Path::new("v.mp4"), "mp4", Suffix::Clean),
            "v (clean).mp4"
        );
    }

    #[test]
    fn collisions_get_numbers_and_nothing_is_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let first = write_output(dir.path(), "photo.webp", b"one").unwrap();
        let second = write_output(dir.path(), "photo.webp", b"two").unwrap();
        let third = write_output(dir.path(), "photo.webp", b"three").unwrap();
        assert_eq!(first.file_name().unwrap(), "photo.webp");
        assert_eq!(second.file_name().unwrap(), "photo (1).webp");
        assert_eq!(third.file_name().unwrap(), "photo (2).webp");
        assert_eq!(std::fs::read(&first).unwrap(), b"one");
    }

    #[test]
    fn dropped_output_leaves_no_temp_file() {
        let dir = tempfile::tempdir().unwrap();
        {
            let mut out = OutputFile::create(dir.path(), "a.png").unwrap();
            out.write_all(b"partial").unwrap();
        }
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[test]
    fn unicode_names_work() {
        let dir = tempfile::tempdir().unwrap();
        let p = write_output(dir.path(), "Férias 夏.png", b"x").unwrap();
        assert!(p.exists());
    }

    #[test]
    fn unique_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let a = create_unique_dir(dir.path(), "doc (pages)").unwrap();
        let b = create_unique_dir(dir.path(), "doc (pages)").unwrap();
        assert!(a.ends_with("doc (pages)"));
        assert!(b.ends_with("doc (pages) (1)"));
    }

    #[test]
    fn multi_output_names() {
        let p = Path::new(r"C:\x\Relatório.pdf");
        assert_eq!(
            multi_output_dir_name(p, MultiOutput::Pages),
            "Relatório (pages)"
        );
        assert_eq!(
            multi_output_dir_name(p, MultiOutput::Split),
            "Relatório (split)"
        );
        assert_eq!(page_file_name(p, 1, 1, "png"), "Relatório-01.png");
        assert_eq!(page_file_name(p, 1, 3, "png"), "Relatório-01.png");
        assert_eq!(page_file_name(p, 7, 12, "png"), "Relatório-07.png");
        assert_eq!(page_file_name(p, 7, 99, "png"), "Relatório-07.png");
        assert_eq!(page_file_name(p, 7, 150, "png"), "Relatório-007.png");
        assert_eq!(page_file_name(p, 7, 1200, "png"), "Relatório-0007.png");
        assert_eq!(split_file_name(p, 1, 3), "Relatório-p1-3.pdf");
        assert_eq!(split_file_name(p, 4, 4), "Relatório-p4.pdf");
    }

    #[test]
    fn natural_order() {
        let mut v: Vec<PathBuf> = ["10.pdf", "2.pdf", "b.pdf", "A.pdf", "1.pdf", "02.pdf"]
            .iter()
            .map(PathBuf::from)
            .collect();
        sort_natural(&mut v);
        let names: Vec<_> = v.iter().map(|p| p.to_string_lossy().into_owned()).collect();
        assert_eq!(
            names,
            ["1.pdf", "2.pdf", "02.pdf", "10.pdf", "A.pdf", "b.pdf"]
        );
        assert_eq!(
            natural_cmp("scan 9 x.pdf", "scan 10 a.pdf"),
            std::cmp::Ordering::Less
        );
    }

    #[test]
    fn output_dir_is_removed_unless_committed() {
        let dir = tempfile::tempdir().unwrap();
        {
            let out = OutputDir::create(dir.path(), "doc (split)").unwrap();
            write_output(out.path(), "doc-p1.pdf", b"x").unwrap();
        }
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
        let out = OutputDir::create(dir.path(), "doc (split)").unwrap();
        write_output(out.path(), "doc-p1.pdf", b"x").unwrap();
        let kept = out.commit();
        assert!(kept.join("doc-p1.pdf").exists());
    }

    #[test]
    fn stale_temps_are_removed_and_nothing_else() {
        let dir = tempfile::tempdir().unwrap();
        let file = |name: &str, age_h: u64| {
            let path = dir.path().join(name);
            let f = File::create(&path).unwrap();
            f.set_modified(SystemTime::now() - Duration::from_secs(age_h * 3600))
                .unwrap();
            path
        };
        let id = "0b7c3a2e-5f3d-4c4e-9a51-2d1f0e6b7a88";
        let stale = file(&format!(".~morph-{id}.tmp"), 30);
        let fresh = file(".~morph-1b7c3a2e-5f3d-4c4e-9a51-2d1f0e6b7a88.tmp", 1);
        let others = [
            file(".~morph-notauuid.tmp", 30),
            file(&format!(".~morph-{id}.tmp.bak"), 30),
            file(&format!("~morph-{id}.tmp"), 30),
            file("photo.tmp", 30),
            file("photo.jpg", 30),
        ];
        remove_stale_temps(dir.path(), STALE_TEMP_AGE);
        assert!(!stale.exists());
        assert!(fresh.exists());
        for p in others {
            assert!(p.exists(), "{}", p.display());
        }
        assert!(is_temp_name(&format!(
            ".~morph-{}.tmp",
            uuid::Uuid::new_v4()
        )));
    }

    #[test]
    fn missing_folder_is_not_writable_error() {
        let err = OutputFile::create(Path::new(r"C:\definitely\missing\dir"), "a.png")
            .err()
            .unwrap();
        assert_eq!(err.kind, crate::error::ErrorKind::OutputNotWritable);
    }
}
