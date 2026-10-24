// SPDX-License-Identifier: GPL-3.0-or-later
//! Reading the selected paths out of an `IShellItemArray`. Names only, no file I/O.
//!
//! The Windows 11 menu passes at most 16 items (seen: 55 selected, the menu
//! said "Convert 16 files…"); Adobe users see the same with "Combine files in
//! Acrobat": <https://community.adobe.com/t5/acrobat-discussions/right-click-quot-combine-files-in-acrobat-quot-file-limit-in-windows-11-environment/m-p/15434410>.

use crate::view;
use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use std::path::{Path, PathBuf};
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::UI::Shell::{IShellItemArray, SIGDN_FILESYSPATH};
use windows_core::IUnknown;

/// The menu shape is decided from at most this many items, so a huge
/// selection (Ctrl+A on 50,000 files) can't slow down the right-click.
pub(crate) const MENU_SCAN_LIMIT: u32 = 1000;

/// The Windows 11 menu builds an `IExplorerCommand` menu from at most this many
/// of the selected items (seen: 55 selected, the menu said "Convert 16
/// files…"). `Invoke` may get the same cut array, or none: see [`files_for_invoke`].
pub(crate) const EXPLORER_MENU_SAMPLE: u32 = 16;

/// Number of items in the selection.
pub(crate) fn count(items: &IShellItemArray) -> u32 {
    // SAFETY: plain COM call on a valid interface.
    unsafe { items.GetCount() }.unwrap_or(0)
}

/// File-system paths of the first `limit` selected items. Items without a
/// file-system path (virtual folders, some library items) are left out.
pub(crate) fn paths(items: &IShellItemArray, limit: u32) -> Vec<PathBuf> {
    let n = count(items).min(limit);
    let mut out = Vec::with_capacity(n as usize);
    for i in 0..n {
        // SAFETY: `i` < GetCount(); plain COM call on a valid interface.
        let Ok(item) = (unsafe { items.GetItemAt(i) }) else {
            continue;
        };
        // SAFETY: plain COM call; the returned string is freed below.
        let Ok(name) = (unsafe { item.GetDisplayName(SIGDN_FILESYSPATH) }) else {
            continue;
        };
        if name.is_null() {
            continue;
        }
        // SAFETY: `name` is a NUL-terminated string allocated by the shell.
        let wide = unsafe { name.as_wide() };
        out.push(PathBuf::from(OsString::from_wide(wide)));
        // SAFETY: the shell allocated `name` with CoTaskMemAlloc; we own it.
        unsafe { CoTaskMemFree(Some(name.0 as *const _)) };
    }
    out
}

/// All selected paths (used when the user clicks an item).
pub(crate) fn all_paths(items: &IShellItemArray) -> Vec<PathBuf> {
    paths(items, u32::MAX)
}

/// The files a click applies to, and where they came from (for the log).
///
/// `invoked` is the array `Invoke` got. It is used as is when it is clearly
/// complete (fewer than [`EXPLORER_MENU_SAMPLE`] items, and the clicked menu
/// was not built for a big selection: `menu_was_big`). Otherwise Explorer's
/// view is asked for the whole selection (`site` first, then the open Explorer
/// windows), and that answer is kept only if it is bigger and contains the
/// clicked items (`anchor`: the first item `GetState` saw).
pub(crate) fn files_for_invoke(
    invoked: Option<&IShellItemArray>,
    site: Option<&IUnknown>,
    anchor: Option<&Path>,
    menu_was_big: bool,
) -> (Vec<PathBuf>, &'static str) {
    let invoked_paths = invoked.map(all_paths).unwrap_or_default();
    if invoked.is_some() && !menu_was_big && invoked_paths.len() < EXPLORER_MENU_SAMPLE as usize {
        return (invoked_paths, "invoked array");
    }
    let from_site = site
        .and_then(view::from_site)
        .map(|a| (a, "Explorer view (site)"));
    let from_windows = std::iter::once_with(|| {
        view::window_selections()
            .into_iter()
            .map(|a| (a, "Explorer window list"))
    })
    .flatten();
    for (array, source) in from_site.into_iter().chain(from_windows) {
        if count(&array) as usize <= invoked_paths.len() {
            continue; // not bigger: don't read every path for nothing
        }
        let view_paths = all_paths(&array);
        if prefer_view(&invoked_paths, anchor, &view_paths) {
            return (view_paths, source);
        }
    }
    let source = if invoked.is_some() {
        "invoked array"
    } else {
        "nothing (no array, no view)"
    };
    (invoked_paths, source)
}

/// Whether the view's selection should replace the invoked items: it must be
/// bigger and include the first invoked item (or, with no array, `anchor`), so
/// a selection from some other window is never used.
pub(crate) fn prefer_view(invoked: &[PathBuf], anchor: Option<&Path>, view: &[PathBuf]) -> bool {
    let Some(check) = invoked.first().map(PathBuf::as_path).or(anchor) else {
        return false;
    };
    view.len() > invoked.len() && view.iter().any(|p| p == check)
}

/// The selection of the view that owns `site`, for building the menu: its
/// first [`MENU_SCAN_LIMIT`] paths and its item count. Names only, no file I/O.
pub(crate) fn view_for_menu(site: Option<&IUnknown>) -> Option<(Vec<PathBuf>, usize)> {
    let view = view::from_site(site?)?;
    Some((paths(&view, MENU_SCAN_LIMIT), count(&view) as usize))
}

/// Whether a view's selection (`view_scanned`: its first paths, `view_total`
/// items) is the one Explorer passed (maybe as a sample): at least as big as
/// the array, and holding the array's first item.
pub(crate) fn same_selection(
    first: &Path,
    array_total: usize,
    view_scanned: &[PathBuf],
    view_total: usize,
) -> bool {
    view_total >= array_total && view_scanned.iter().any(|p| p == first)
}
