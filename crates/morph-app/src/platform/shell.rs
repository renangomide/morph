// SPDX-License-Identifier: GPL-3.0-or-later
//! Open files and folders in Explorer (result rows, "Open folder").
//! All functions are fire-and-forget and never panic.
//!
//! The shell does not understand verbatim (`\\?\`) paths, which `std::fs::canonicalize`
//! returns, so they are turned back into normal form first. Unicode is passed as UTF-16
//! all the way through.

use std::ffi::OsString;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use windows::Win32::Foundation::{HWND, RPC_E_CHANGED_MODE};
use windows::Win32::System::Com::{
    COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoInitializeEx, CoUninitialize,
};
use windows::Win32::UI::Shell::Common::ITEMIDLIST;
use windows::Win32::UI::Shell::{
    ILFindLastID, ILFree, SE_ERR_NOASSOC, SHOpenFolderAndSelectItems, SHParseDisplayName,
    ShellExecuteW,
};
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
use windows::core::{HSTRING, PCWSTR, w};

/// Open a file with its default app. Files without an associated app get the
/// "Open with" dialog.
pub fn open_file(path: &Path) -> anyhow::Result<()> {
    let path = shell_path(path);
    let _com = ComGuard::new();
    match shell_execute(w!("open"), &path) {
        Ok(()) => Ok(()),
        Err(code) if code == SE_ERR_NOASSOC as usize => shell_execute(w!("openas"), &path)
            .map_err(|c| anyhow::anyhow!("ShellExecute(openas) failed with code {c}")),
        Err(code) => bail!("ShellExecute(open) failed with code {code}"),
    }
    .with_context(|| format!("could not open {}", path.display()))
}

/// Open a folder in Explorer.
///
/// The path may come from the command line (toast button), so anything that is
/// not an existing folder is refused: "open" on a file would run it.
pub fn open_folder(path: &Path) -> anyhow::Result<()> {
    let path = shell_path(path);
    if !path.is_dir() {
        bail!("not a folder: {}", path.display());
    }
    let _com = ComGuard::new();
    shell_execute(w!("open"), &path)
        .map_err(|code| anyhow::anyhow!("ShellExecute(open) failed with code {code}"))
        .with_context(|| format!("could not open folder {}", path.display()))
}

/// Open an `https://` link in the default browser ("Download LibreOffice").
/// Anything else is refused: "open" on a path would run it.
pub fn open_url(url: &str) -> anyhow::Result<()> {
    if !url.starts_with("https://") || url.chars().any(char::is_whitespace) {
        bail!("not an https link: {url}");
    }
    let _com = ComGuard::new();
    shell_execute(w!("open"), Path::new(url))
        .map_err(|code| anyhow::anyhow!("ShellExecute(open) failed with code {code}"))
        .with_context(|| format!("could not open {url}"))
}

/// Open the file's folder in Explorer with the file selected.
/// With several files in the same folder, all of them are selected.
/// Files in other folders than the first file's are ignored.
pub fn show_in_folder(paths: &[&Path]) -> anyhow::Result<()> {
    let Some(group) = select_group(paths) else {
        bail!("no file to show");
    };
    let _com = ComGuard::new();
    match open_and_select(&group) {
        Ok(()) => Ok(()),
        Err(api_err) => {
            // Fallback: Explorer can select one item from its command line.
            explorer_select(&group.files[0]).with_context(|| {
                format!("SHOpenFolderAndSelectItems failed ({api_err}), explorer.exe /select too")
            })
        }
    }
}

/// The folder to open and the files in it to select.
#[derive(Debug, PartialEq, Eq)]
struct SelectGroup {
    folder: PathBuf,
    files: Vec<PathBuf>,
}

/// Group for [`show_in_folder`]: the first file's folder and every given file in it
/// (compared case-insensitively, like NTFS), without duplicates, in input order.
fn select_group(paths: &[&Path]) -> Option<SelectGroup> {
    let first = shell_path(paths.first()?);
    let folder = first.parent().filter(|p| !p.as_os_str().is_empty())?;
    let key = fold(folder);
    let mut seen = Vec::new();
    let mut files = Vec::new();
    for p in paths {
        let p = shell_path(p);
        let Some(parent) = p.parent() else { continue };
        if fold(parent) != key || p.file_name().is_none() {
            continue;
        }
        let k = fold(&p);
        if !seen.contains(&k) {
            seen.push(k);
            files.push(p);
        }
    }
    Some(SelectGroup {
        folder: folder.to_path_buf(),
        files,
    })
}

/// Case-insensitive comparison key, ignoring trailing separators.
fn fold(p: &Path) -> String {
    p.to_string_lossy()
        .trim_end_matches(['\\', '/'])
        .replace('/', "\\")
        .to_lowercase()
}

/// `\\?\C:\x` → `C:\x`, `\\?\UNC\srv\share\x` → `\\srv\share\x`; other paths unchanged.
fn shell_path(p: &Path) -> PathBuf {
    let wide: Vec<u16> = p.as_os_str().encode_wide().collect();
    let prefix: Vec<u16> = r"\\?\".encode_utf16().collect();
    let unc: Vec<u16> = r"\\?\UNC\".encode_utf16().collect();
    let starts_ci = |w: &[u16], pre: &[u16]| {
        w.len() >= pre.len()
            && w[..pre.len()]
                .iter()
                .zip(pre)
                .all(|(a, b)| ascii_lower(*a) == ascii_lower(*b))
    };
    if starts_ci(&wide, &unc) {
        let mut out: Vec<u16> = r"\\".encode_utf16().collect();
        out.extend_from_slice(&wide[unc.len()..]);
        PathBuf::from(OsString::from_wide(&out))
    } else if starts_ci(&wide, &prefix) {
        PathBuf::from(OsString::from_wide(&wide[prefix.len()..]))
    } else {
        p.to_path_buf()
    }
}

fn ascii_lower(c: u16) -> u16 {
    if (u16::from(b'A')..=u16::from(b'Z')).contains(&c) {
        c + 32
    } else {
        c
    }
}

/// ShellExecuteW; `Err` holds the error code (≤ 32).
fn shell_execute(verb: PCWSTR, path: &Path) -> Result<(), usize> {
    let file = HSTRING::from(path.as_os_str());
    // SAFETY: all string pointers are valid NUL-terminated UTF-16 for the duration of the call.
    let rc = unsafe {
        ShellExecuteW(
            None::<HWND>,
            verb,
            &file,
            PCWSTR::null(),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        )
    };
    let code = rc.0 as usize;
    if code > 32 { Ok(()) } else { Err(code) }
}

/// Owned absolute PIDL, freed on drop.
struct Pidl(*mut ITEMIDLIST);

impl Pidl {
    fn parse(path: &Path) -> windows::core::Result<Pidl> {
        let name = HSTRING::from(path.as_os_str());
        let mut pidl: *mut ITEMIDLIST = std::ptr::null_mut();
        // SAFETY: `name` is a valid string; `pidl` receives a PIDL we free in Drop.
        unsafe { SHParseDisplayName(&name, None, &mut pidl, 0, None)? };
        if pidl.is_null() {
            return Err(windows::core::Error::from_hresult(
                windows::Win32::Foundation::E_FAIL,
            ));
        }
        Ok(Pidl(pidl))
    }
}

impl Drop for Pidl {
    fn drop(&mut self) {
        // SAFETY: the PIDL was allocated by SHParseDisplayName and is freed once.
        unsafe { ILFree(Some(self.0)) };
    }
}

fn open_and_select(group: &SelectGroup) -> windows::core::Result<()> {
    let folder = Pidl::parse(&group.folder)?;
    let items: Vec<Pidl> = group
        .files
        .iter()
        .filter_map(|f| Pidl::parse(f).ok())
        .collect();
    if items.is_empty() {
        return Err(windows::core::Error::from_hresult(
            windows::Win32::Foundation::E_FAIL,
        ));
    }
    // Children relative to the folder: the last ID of each absolute PIDL
    // (valid as long as `items` lives).
    // SAFETY: each PIDL is a valid absolute ITEMIDLIST.
    let children: Vec<*const ITEMIDLIST> = items
        .iter()
        .map(|p| unsafe { ILFindLastID(p.0) } as *const ITEMIDLIST)
        .collect();
    // SAFETY: `folder` and `children` are valid PIDLs for the duration of the call.
    unsafe { SHOpenFolderAndSelectItems(folder.0, Some(&children), 0) }
}

/// `explorer.exe /select,"<path>"`. Explorer does its own command-line parsing,
/// so the argument is passed raw with the path quoted.
fn explorer_select(file: &Path) -> anyhow::Result<()> {
    let mut arg = OsString::from("/select,\"");
    arg.push(file.as_os_str());
    arg.push("\"");
    std::process::Command::new(explorer_exe())
        .raw_arg(arg)
        .spawn()
        .context("could not start explorer.exe")?;
    Ok(())
}

fn explorer_exe() -> PathBuf {
    std::env::var_os("SystemRoot")
        .or_else(|| std::env::var_os("windir"))
        .map(|root| PathBuf::from(root).join("explorer.exe"))
        .unwrap_or_else(|| PathBuf::from("explorer.exe"))
}

/// Initializes COM (STA) on this thread for the guard's lifetime, if needed.
///
/// `S_OK`/`S_FALSE` both add a reference that must be released; `RPC_E_CHANGED_MODE`
/// means the thread is already MTA, which the shell calls also accept, and adds none.
struct ComGuard {
    uninit: bool,
}

impl ComGuard {
    fn new() -> ComGuard {
        // SAFETY: plain COM initialization; balanced in Drop when it succeeded.
        let hr = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) };
        ComGuard {
            uninit: hr.is_ok() && hr != RPC_E_CHANGED_MODE,
        }
    }
}

impl Drop for ComGuard {
    fn drop(&mut self) {
        if self.uninit {
            // SAFETY: balances the successful CoInitializeEx in `new` on the same thread.
            unsafe { CoUninitialize() };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn group(paths: &[&str]) -> Option<SelectGroup> {
        let paths: Vec<&Path> = paths.iter().map(Path::new).collect();
        select_group(&paths)
    }

    #[test]
    fn same_folder_selects_all() {
        let g = group(&[r"C:\Fotos\a.jpg", r"C:\fotos\B.webp", r"C:\Fotos\a.jpg"]).unwrap();
        assert_eq!(g.folder, PathBuf::from(r"C:\Fotos"));
        assert_eq!(
            g.files,
            vec![
                PathBuf::from(r"C:\Fotos\a.jpg"),
                PathBuf::from(r"C:\fotos\B.webp")
            ]
        );
    }

    #[test]
    fn other_folders_are_ignored() {
        let g = group(&[
            r"D:\Férias 夏\x.png",
            r"D:\Other\y.png",
            r"D:\Férias 夏\z.png",
        ])
        .unwrap();
        assert_eq!(g.folder, PathBuf::from(r"D:\Férias 夏"));
        assert_eq!(
            g.files,
            vec![
                PathBuf::from(r"D:\Férias 夏\x.png"),
                PathBuf::from(r"D:\Férias 夏\z.png")
            ]
        );
    }

    #[test]
    fn empty_or_rootless_is_none() {
        assert_eq!(group(&[]), None);
        assert_eq!(group(&["a.jpg"]), None);
    }

    #[test]
    fn verbatim_paths_are_normalized() {
        assert_eq!(
            shell_path(Path::new(r"\\?\C:\Long\path.jpg")),
            PathBuf::from(r"C:\Long\path.jpg")
        );
        assert_eq!(
            shell_path(Path::new(r"\\?\UNC\srv\share\a.jpg")),
            PathBuf::from(r"\\srv\share\a.jpg")
        );
        assert_eq!(
            shell_path(Path::new(r"C:\plain.jpg")),
            PathBuf::from(r"C:\plain.jpg")
        );
        let g = group(&[r"\\?\C:\X\a.jpg", r"C:\x\b.jpg"]).unwrap();
        assert_eq!(g.files.len(), 2);
    }

    #[test]
    fn open_folder_refuses_files_and_missing() {
        let exe = std::env::current_exe().unwrap();
        assert!(open_folder(&exe).is_err());
        assert!(open_folder(Path::new(r"C:\definitely\not\here\morph")).is_err());
    }

    #[test]
    fn open_url_refuses_non_https() {
        assert!(open_url(r"C:\Windows\notepad.exe").is_err());
        assert!(open_url("http://example.com").is_err());
        assert!(open_url("https://x y").is_err());
    }

    #[test]
    #[ignore = "opens Explorer windows; run on a desktop"]
    fn desktop_show_in_folder() {
        let exe = std::env::current_exe().unwrap();
        show_in_folder(&[&exe]).unwrap();
        open_folder(exe.parent().unwrap()).unwrap();
    }
}
