// SPDX-License-Identifier: GPL-3.0-or-later
//! How `Morph.exe` was launched, parsed from the command line (SPEC §5.2).

use std::ffi::OsString;
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Launch {
    /// `Morph.exe --request <path>`: request file written by the shell DLL.
    Request(PathBuf),
    /// `Morph.exe <file1> <file2> …`: plain file list (dev, drag onto shortcut).
    Files(Vec<PathBuf>),
    /// `Morph.exe --open-folder <path>`: toast "Open folder" button.
    OpenFolder(PathBuf),
    /// `Morph.exe --show-results <id>`: toast click; id of a saved results file.
    ShowResults(String),
    /// No arguments: empty window.
    Empty,
}

/// Parse the arguments that follow the program name.
///
/// A flag (`--request`, `--open-folder`, `--show-results`) must be first and
/// followed by its value; anything after that value is ignored. A flag without
/// a value is treated as an empty launch.
pub fn parse<I>(args: I) -> Launch
where
    I: IntoIterator<Item = OsString>,
{
    let args: Vec<OsString> = args.into_iter().collect();
    // Toast activation starts `Morph.exe "morph:…"` (protocol activation).
    // Any app or web page can launch such a URI, so it only ever maps to the
    // two toast actions; anything else is an empty launch, never a file list.
    if let Some(first) = args.first()
        && is_protocol_uri(first)
    {
        let Some(mapped) = crate::platform::toast::args_from_uri(first) else {
            return Launch::Empty;
        };
        return match parse_flags(mapped) {
            // Only plain local paths: a network or device path would make
            // Windows contact that server (and send the user's credentials).
            Launch::OpenFolder(p) if !is_plain_local_path(&p) => Launch::Empty,
            Launch::Request(_) | Launch::Files(_) => Launch::Empty,
            other => other,
        };
    }
    parse_flags(args)
}

/// `X:\folder\…` only: a drive letter, a root, then plain names (no `..`, no
/// `\\server`, `\\?\`, `\??\` or `/`-led forms).
fn is_plain_local_path(path: &std::path::Path) -> bool {
    use std::path::{Component, Prefix};
    let text = path.to_string_lossy();
    if text.contains('/') || text.contains("\\??\\") {
        return false;
    }
    let mut parts = path.components();
    let disk = matches!(
        parts.next(),
        Some(Component::Prefix(p)) if matches!(p.kind(), Prefix::Disk(_))
    );
    disk && matches!(parts.next(), Some(Component::RootDir))
        && parts.all(|c| matches!(c, Component::Normal(_)))
}

fn is_protocol_uri(arg: &std::ffi::OsStr) -> bool {
    let prefix = format!("{}:", crate::platform::toast::PROTOCOL_SCHEME);
    arg.to_str()
        .and_then(|s| s.trim_start().get(..prefix.len()))
        .is_some_and(|p| p.eq_ignore_ascii_case(&prefix))
}

fn parse_flags(args: Vec<OsString>) -> Launch {
    use morph_formats::{ARG_OPEN_FOLDER, ARG_REQUEST, ARG_SHOW_RESULTS};
    let mut args = args.into_iter().peekable();
    let flag = args.peek().and_then(|first| {
        [ARG_REQUEST, ARG_OPEN_FOLDER, ARG_SHOW_RESULTS]
            .into_iter()
            .find(|f| first.as_os_str() == *f)
    });
    if let Some(flag) = flag {
        args.next();
        let Some(value) = args.next().filter(|v| !v.is_empty()) else {
            return Launch::Empty;
        };
        return match flag {
            ARG_OPEN_FOLDER => Launch::OpenFolder(PathBuf::from(value)),
            ARG_SHOW_RESULTS => Launch::ShowResults(value.to_string_lossy().into_owned()),
            _ => Launch::Request(PathBuf::from(value)),
        };
    }
    let files: Vec<PathBuf> = args.filter(|a| !a.is_empty()).map(PathBuf::from).collect();
    if files.is_empty() {
        Launch::Empty
    } else {
        Launch::Files(files)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn os(args: &[&str]) -> Vec<OsString> {
        args.iter().map(OsString::from).collect()
    }

    #[test]
    fn no_args_is_empty() {
        assert_eq!(parse(os(&[])), Launch::Empty);
    }

    #[test]
    fn request_flag_with_path() {
        assert_eq!(
            parse(os(&["--request", r"C:\Users\x\req 1.json"])),
            Launch::Request(PathBuf::from(r"C:\Users\x\req 1.json"))
        );
    }

    #[test]
    fn request_flag_without_path_is_empty() {
        assert_eq!(parse(os(&["--request"])), Launch::Empty);
    }

    #[test]
    fn toast_flags() {
        assert_eq!(
            parse(os(&["--open-folder", r"D:\Fotos\Férias"])),
            Launch::OpenFolder(PathBuf::from(r"D:\Fotos\Férias"))
        );
        assert_eq!(
            parse(os(&["--show-results", "abc-123"])),
            Launch::ShowResults("abc-123".into())
        );
        assert_eq!(parse(os(&["--show-results"])), Launch::Empty);
    }

    #[test]
    fn protocol_uris_map_only_to_toast_actions() {
        assert_eq!(
            parse(os(&[
                "morph:open-folder?path=C%3A%5CUsers%5CPublic%5CF%C3%A9rias"
            ])),
            Launch::OpenFolder(PathBuf::from(r"C:\Users\Public\Férias"))
        );
        assert_eq!(
            parse(os(&["MORPH:show-results?id=abc-123"])),
            Launch::ShowResults("abc-123".into())
        );
        for bad in [
            "morph:nope?x=1",
            "morph:open-folder",
            "morph:open-folder?path=%5C%5Cevil%5Cshare",
            "morph:open-folder?path=%2F%2F127.0.0.1%2Fnoshare%2Fx",
            "morph:open-folder?path=%5C%3F%3F%5CUNC%5C127.0.0.1%5Cnoshare",
            "morph:open-folder?path=%5C%5C%3F%5CUNC%5C127.0.0.1%5Cs",
            "morph:open-folder?path=%5C%5C%3F%5CC%3A%5CUsers",
            "morph:open-folder?path=C%3A%5CUsers%5C..%5C..%5CWindows",
            "morph:open-folder?path=C%3AUsers",
            "morph:open-folder?path=%ZZ",
        ] {
            assert_eq!(parse(os(&[bad])), Launch::Empty, "{bad}");
        }
    }

    #[test]
    fn plain_files() {
        assert_eq!(
            parse(os(&[r"C:\a.jpg", r"D:\Fotos\Férias 夏.png"])),
            Launch::Files(vec![
                PathBuf::from(r"C:\a.jpg"),
                PathBuf::from(r"D:\Fotos\Férias 夏.png"),
            ])
        );
    }

    #[test]
    fn empty_strings_are_skipped() {
        assert_eq!(parse(os(&[""])), Launch::Empty);
        assert_eq!(
            parse(os(&["", r"C:\a.jpg"])),
            Launch::Files(vec![PathBuf::from(r"C:\a.jpg")])
        );
    }
}
