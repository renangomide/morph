// SPDX-License-Identifier: GPL-3.0-or-later
//! Request file handed from the shell DLL to `Morph.exe` (SPEC §5.2).
//! A file instead of command-line arguments avoids the 32K command-line limit.

use serde::{Deserialize, Serialize};
use std::io;
use std::path::{Path, PathBuf};

pub const REQUEST_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Request {
    pub version: u32,
    pub files: Vec<PathBuf>,
    pub action: String,
    #[serde(default)]
    pub preset: serde_json::Map<String, serde_json::Value>,
}

impl Request {
    pub fn new(files: Vec<PathBuf>, action: &str, preset: &[(&str, &str)]) -> Request {
        Request {
            version: REQUEST_VERSION,
            files,
            action: action.to_owned(),
            preset: preset
                .iter()
                .map(|(k, v)| ((*k).to_owned(), serde_json::Value::from(*v)))
                .collect(),
        }
    }

    /// Write the request so a reader never sees a half-written file:
    /// write `<path>.tmp`, then rename it to `path`.
    pub fn save_atomic(&self, path: &Path) -> io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let json = serde_json::to_vec(self).map_err(io::Error::other)?;
        let mut tmp = path.as_os_str().to_owned();
        tmp.push(".tmp");
        let tmp = PathBuf::from(tmp);
        std::fs::write(&tmp, json)?;
        std::fs::rename(&tmp, path).inspect_err(|_| {
            let _ = std::fs::remove_file(&tmp);
        })
    }

    pub fn load(path: &Path) -> io::Result<Request> {
        let bytes = std::fs::read(path)?;
        let request: Request = serde_json::from_slice(&bytes).map_err(io::Error::other)?;
        if request.version != REQUEST_VERSION {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("unsupported request version {}", request.version),
            ));
        }
        Ok(request)
    }

    /// Read the request, then delete the file (it is single-use), also when it
    /// can't be parsed. Only files directly inside one of `allowed_dirs` (the
    /// requests folders) are read or deleted; anything else is refused, so
    /// `--request <path>` can't make Morph delete an arbitrary file.
    pub fn take(path: &Path, allowed_dirs: &[PathBuf]) -> io::Result<Request> {
        let canonical = |p: &Path| std::fs::canonicalize(p).ok();
        let parent = path.parent().and_then(canonical);
        let allowed = parent.is_some_and(|parent| {
            allowed_dirs
                .iter()
                .filter_map(|d| canonical(d))
                .any(|d| d == parent)
        });
        if !allowed {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!("not in the requests folder: {}", path.display()),
            ));
        }
        let request = Request::load(path);
        let _ = std::fs::remove_file(path);
        request
    }

    /// Delete request files older than `min_age` in `dir` (never read, e.g.
    /// the app failed to start). Best effort; call at startup.
    pub fn sweep(dir: &Path, min_age: std::time::Duration) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        let now = std::time::SystemTime::now();
        for entry in entries.flatten() {
            let old = entry
                .metadata()
                .ok()
                .filter(|m| m.is_file())
                .and_then(|m| m.modified().ok())
                .and_then(|t| now.duration_since(t).ok())
                .is_some_and(|age| age >= min_age);
            if old {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_with_unicode_paths() {
        let dir = std::env::temp_dir().join(format!("morph-req-test-{}", std::process::id()));
        let path = dir.join("req.json");
        let req = Request::new(
            vec![
                PathBuf::from(r"C:\Fotos\Férias 夏.jpg"),
                PathBuf::from(r"D:\b.png"),
            ],
            crate::actions::IMAGE_CONVERT,
            &[("format", "png")],
        );
        req.save_atomic(&path).unwrap();
        let back = Request::take(&path, std::slice::from_ref(&dir)).unwrap();
        assert_eq!(back, req);
        assert!(!path.exists(), "take() deletes the file");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn take_refuses_outside_paths_and_deletes_broken_requests() {
        let dir = std::env::temp_dir().join(format!("morph-req-take-{}", std::process::id()));
        let requests = dir.join("requests");
        std::fs::create_dir_all(&requests).unwrap();
        let outside = dir.join("victim.json");
        Request::new(vec![], crate::actions::IMAGE_CONVERT, &[])
            .save_atomic(&outside)
            .unwrap();
        assert!(Request::take(&outside, std::slice::from_ref(&requests)).is_err());
        assert!(
            outside.exists(),
            "files outside the requests folder are never deleted"
        );

        let broken = requests.join("broken.json");
        std::fs::write(&broken, "{ not json").unwrap();
        assert!(Request::take(&broken, std::slice::from_ref(&requests)).is_err());
        assert!(!broken.exists(), "broken requests are deleted too");

        std::fs::write(requests.join("old.json"), "{}").unwrap();
        Request::sweep(&requests, std::time::Duration::ZERO);
        assert_eq!(std::fs::read_dir(&requests).unwrap().count(), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rejects_unknown_version() {
        let dir = std::env::temp_dir().join(format!("morph-req-ver-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("req.json");
        std::fs::write(&path, r#"{"version":99,"files":[],"action":"x"}"#).unwrap();
        assert!(Request::load(&path).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
