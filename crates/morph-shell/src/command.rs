// SPDX-License-Identifier: GPL-3.0-or-later
//! The root "Morph" command and its subcommands.
//!
//! Explorer asks the root for its state (with the selection), then for its
//! subcommands (without the selection). So the root computes the menu in
//! `GetState` and keeps it for `EnumSubCommands`.
//!
//! Both kinds of command take the site Explorer offers (`IObjectWithSite`):
//! on a click it leads to the folder view, which knows the whole selection
//! when the array Explorer passes is cut to 16 items or missing.

use crate::enumerator::CommandEnum;
use crate::{ObjectToken, ROOT_CLSID, guard, launch, log, selection};
use morph_formats::{
    Category, EnabledCategories, MenuEntry, MenuItem, SelectionSummary, actions, build_menu,
    config, paths,
};
use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use windows::Win32::Foundation::{E_FAIL, E_NOTIMPL, E_POINTER};
use windows::Win32::System::Com::IBindCtx;
use windows::Win32::System::Ole::{IObjectWithSite, IObjectWithSite_Impl};
use windows::Win32::UI::Shell::{
    ECF_DEFAULT, ECF_HASSUBCOMMANDS, ECF_ISSEPARATOR, ECS_ENABLED, ECS_HIDDEN,
    IEnumExplorerCommand, IExplorerCommand, IExplorerCommand_Impl, IShellItemArray, SHStrDupW,
};
use windows_core::{BOOL, GUID, HSTRING, IUnknown, Interface, PWSTR, Ref, implement};

/// Allocate a string the shell will free (CoTaskMem).
fn shell_string(s: &str) -> windows_core::Result<PWSTR> {
    let h = HSTRING::from(s);
    // SAFETY: `h` is a valid NUL-terminated wide string for the duration of the call.
    unsafe { SHStrDupW(&h) }
}

fn enabled_categories() -> EnabledCategories {
    match paths::shell_data_dir() {
        Some(root) => config::cached_menu_config(&paths::config_file(&root)),
        None => EnabledCategories::default(),
    }
}

/// The menu for `selected` (the first [`selection::MENU_SCAN_LIMIT`] of `total`
/// items). `partial`: Explorer may have cut the selection, don't show a count.
fn menu_for_paths(selected: &[PathBuf], total: usize, partial: bool) -> Vec<MenuEntry> {
    let enabled = enabled_categories();
    let scanned = selected.len().min(selection::MENU_SCAN_LIMIT as usize);
    let mut summary =
        SelectionSummary::from_paths(selected[..scanned].iter().map(PathBuf::as_path), &enabled);
    // Items past the scan limit count towards "Convert N files…" only.
    summary.unscanned = total.saturating_sub(scanned);
    summary.partial = partial;
    build_menu(&summary)
}

/// The menu for the selection `items` stands for, and where it was read (for
/// the log). `site` leads to the folder view, asked only for big arrays.
fn menu_for(items: &IShellItemArray, site: Option<&IUnknown>) -> (Vec<MenuEntry>, &'static str) {
    let total = selection::count(items) as usize;
    let array = selection::paths(items, selection::MENU_SCAN_LIMIT);
    menu_for_selection(&array, total, || selection::view_for_menu(site))
}

/// The menu for `array` (the first paths of the `total` items Explorer passed).
///
/// The Windows 11 menu builds from the first 16 items only, so 16 may mean
/// "16 or more". On a click it asks again with every item and looks for the
/// clicked subcommand in that new menu: it must be the same menu, or Explorer
/// runs a stale subcommand on the focused file alone (16 PNGs sampled from 42
/// PNGs + 40 PDFs + 2 MP3s gave the image menu, the full build "Convert
/// files…"). So a big array's menu comes from the whole selection as the
/// folder view sees it (`view`: its first paths and total, read only then),
/// never from a sample, and never shows a count ("Convert 55 files…" would not
/// match "Convert files…"). If the view can't be read, a 16-item array may be
/// a sample: the generic "Convert files…" opens the window, and `Invoke`
/// completes the selection from the view.
fn menu_for_selection(
    array: &[PathBuf],
    total: usize,
    view: impl FnOnce() -> Option<(Vec<PathBuf>, usize)>,
) -> (Vec<MenuEntry>, &'static str) {
    if !is_big(total) {
        return (menu_for_paths(array, total, false), "array");
    }
    if let Some(first) = array.first()
        && let Some((paths, view_total)) = view()
        && selection::same_selection(first, total, &paths, view_total)
    {
        return (menu_for_paths(&paths, view_total, true), "Explorer view");
    }
    if total > selection::EXPLORER_MENU_SAMPLE as usize {
        // More than a sample: this is the whole selection.
        return (menu_for_paths(array, total, true), "array");
    }
    (generic_menu(), "generic (maybe a sample, no view)")
}

/// "Convert files…" alone: the menu of a big selection Morph can't see whole.
/// The same entry (title, action, canonical name) as a big mixed selection's.
fn generic_menu() -> Vec<MenuEntry> {
    let all = EnabledCategories::default();
    let mut mixed = SelectionSummary::default();
    mixed.add(Some(Category::Image), &all);
    mixed.add(Some(Category::Audio), &all);
    mixed.partial = true;
    build_menu(&mixed)
        .into_iter()
        .filter(|e| matches!(e, MenuEntry::Item(i) if i.action == actions::MIXED_CONVERT))
        .collect()
}

/// A selection the Windows 11 menu may cut to 16 items.
fn is_big(count: usize) -> bool {
    count >= selection::EXPLORER_MENU_SAMPLE as usize
}

/// A fixed name per subcommand (position + action + preset, never the
/// label), so Explorer can match a subcommand across two menu builds.
fn canonical_name(index: usize, item: &MenuItem) -> GUID {
    // FNV-1a, 64 bits.
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    let mut eat = |bytes: &[u8]| {
        for b in bytes {
            hash ^= u64::from(*b);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        hash ^= 0xff; // field separator
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    };
    eat(&index.to_le_bytes());
    eat(item.action.as_bytes());
    for (key, value) in item.preset {
        eat(key.as_bytes());
        eat(value.as_bytes());
    }
    GUID::from_values(0x6d6f_7270, 0x6873, 0x4d75, hash.to_le_bytes())
}

/// The menu's only item, if it has exactly one (separators aside).
fn lone_item(menu: &[MenuEntry]) -> Option<&MenuItem> {
    let mut items = menu.iter().filter_map(|entry| match entry {
        MenuEntry::Item(item) => Some(item),
        MenuEntry::Separator => None,
    });
    match (items.next(), items.next()) {
        (Some(item), None) => Some(item),
        _ => None,
    }
}

/// Open the window for `action` with `files`. With no files at all, Morph
/// still opens (as for a mixed selection) so a click is never lost silently.
fn open(
    label: &str,
    action: &str,
    preset: &[(&str, &str)],
    files: Vec<PathBuf>,
) -> windows_core::Result<()> {
    let result = if files.is_empty() {
        log::error(&format!(
            "'{label}': no files to pass; opening Morph anyway"
        ));
        launch::open_window(files, actions::MIXED_CONVERT, &[])
    } else {
        launch::open_window(files, action, preset)
    };
    result.inspect_err(|e| log::error(&format!("Invoke '{label}' failed: {e}")))
}

/// The item the root runs when clicked, or `None` when it is the root of a
/// real submenu (Explorer just opens it then). The Windows 11 menu runs the
/// root instead of a lone subcommand (a mixed selection's "Convert N
/// files…"). For big selections Explorer may invoke an object that never got
/// `GetState`, or pass another array than `GetState` saw, so `menu` is
/// rebuilt from the clicked files and the `GetState` menu is the fallback.
fn root_item(cached: &[MenuEntry], menu: &[MenuEntry]) -> Option<MenuItem> {
    if let Some(item) = lone_item(menu).or_else(|| lone_item(cached)) {
        return Some(item.clone());
    }
    if !menu.is_empty() {
        return None;
    }
    // Nothing Morph handles: still open the window, it lists the skipped files.
    Some(MenuItem {
        label: "(no menu)".to_owned(),
        action: actions::MIXED_CONVERT,
        preset: &[],
    })
}

/// What a subcommand click runs. A menu built for 16 or more files (`big`)
/// that ends up with fewer (Explorer invoked a stale subcommand with only the
/// focused item, and the view couldn't complete it) must not quietly convert
/// just those: the window opens instead, showing what it got.
fn sub_action(
    item: &MenuItem,
    big: bool,
    files: usize,
) -> (&'static str, &'static [(&'static str, &'static str)]) {
    if big && files < selection::EXPLORER_MENU_SAMPLE as usize {
        (actions::MIXED_CONVERT, &[])
    } else {
        (item.action, item.preset)
    }
}

/// The menu's titles, for the log: `"Convert files…" | "Combine into one PDF…"`.
fn titles(menu: &[MenuEntry]) -> String {
    let titles: Vec<String> = menu
        .iter()
        .filter_map(|entry| match entry {
            MenuEntry::Item(item) => Some(format!("\"{}\"", item.label)),
            MenuEntry::Separator => None,
        })
        .collect();
    titles.join(" | ")
}

fn yes_no(b: bool) -> &'static str {
    if b { "yes" } else { "no" }
}

fn describe(items: Option<&IShellItemArray>) -> String {
    match items {
        Some(items) => format!("{} items", selection::count(items)),
        None => "no array".to_owned(),
    }
}

/// What `GetState` saw, kept for `EnumSubCommands` and `Invoke`.
#[derive(Default)]
struct Seen {
    menu: Vec<MenuEntry>,
    /// First selected path: checks that a view's selection is this one.
    anchor: Option<PathBuf>,
    /// The array had 16 or more items: a click must apply to as many.
    big: bool,
    /// Big or missing selection: every later call leaves a log line, so a
    /// click that does nothing shows where Explorer stopped.
    trace: bool,
}

/// The site Explorer set on a command (`IObjectWithSite`).
#[derive(Default)]
struct Site(Mutex<Option<IUnknown>>);

impl Site {
    fn get(&self) -> Option<IUnknown> {
        self.0.lock().ok().and_then(|s| s.clone())
    }

    fn set(&self, site: Option<IUnknown>) {
        if let Ok(mut s) = self.0.lock() {
            *s = site;
        }
    }

    fn query(&self, riid: *const GUID, ppv: *mut *mut c_void) -> windows_core::Result<()> {
        if riid.is_null() || ppv.is_null() {
            return Err(E_POINTER.into());
        }
        // SAFETY: checked non-null.
        unsafe { *ppv = std::ptr::null_mut() };
        match self.get() {
            // SAFETY: riid/ppv checked non-null above.
            Some(site) => unsafe { site.query(riid, ppv) }.ok(),
            None => Err(E_FAIL.into()),
        }
    }
}

// ---------------------------------------------------------------- root

#[implement(IExplorerCommand, IObjectWithSite)]
pub(crate) struct RootCommand {
    seen: Mutex<Seen>,
    site: Site,
    _token: ObjectToken,
}

impl RootCommand {
    pub(crate) fn new() -> RootCommand {
        RootCommand {
            seen: Mutex::new(Seen::default()),
            site: Site::default(),
            _token: ObjectToken::new(),
        }
    }
}

fn root_invoke(
    cached: &[MenuEntry],
    anchor: Option<&Path>,
    big: bool,
    items: Option<&IShellItemArray>,
    site: Option<&IUnknown>,
) -> windows_core::Result<()> {
    // Logged before asking Explorer's view, in case that call never returns.
    log::info(|| {
        format!(
            "Root::Invoke: {}, site {}, GetState menu {} entries",
            describe(items),
            yes_no(site.is_some()),
            cached.len(),
        )
    });
    let (files, source) = selection::files_for_invoke(items, site, anchor, big);
    log::info(|| format!("Root::Invoke: using {} files from {source}", files.len()));
    let menu = menu_for_paths(&files, files.len(), false);
    let Some(item) = root_item(cached, &menu) else {
        return Ok(());
    };
    open(&item.label, item.action, item.preset, files)
}

impl IExplorerCommand_Impl for RootCommand_Impl {
    fn GetTitle(&self, _items: Ref<'_, IShellItemArray>) -> windows_core::Result<PWSTR> {
        guard("Root::GetTitle", || {
            shell_string(morph_formats::APP_DISPLAY_NAME)
        })
    }

    fn GetIcon(&self, _items: Ref<'_, IShellItemArray>) -> windows_core::Result<PWSTR> {
        guard("Root::GetIcon", || {
            match launch::file_next_to_dll("morph.ico") {
                Some(path) => shell_string(&path.to_string_lossy()),
                None => Err(E_NOTIMPL.into()),
            }
        })
    }

    fn GetToolTip(&self, _items: Ref<'_, IShellItemArray>) -> windows_core::Result<PWSTR> {
        Err(E_NOTIMPL.into())
    }

    fn GetCanonicalName(&self) -> windows_core::Result<GUID> {
        Ok(ROOT_CLSID)
    }

    fn GetState(
        &self,
        items: Ref<'_, IShellItemArray>,
        _ok_to_be_slow: BOOL,
    ) -> windows_core::Result<u32> {
        guard("Root::GetState", || {
            let items = items.as_ref();
            let site = self.site.get();
            let (menu, source) = items
                .map(|i| menu_for(i, site.as_ref()))
                .unwrap_or((Vec::new(), "no array"));
            let anchor = items.and_then(|i| selection::paths(i, 1).into_iter().next());
            let count = items.map(selection::count).unwrap_or(0);
            let big = is_big(count as usize);
            // Big (maybe cut) selections only: a trace for "click did nothing".
            let trace = items.is_none() || big;
            if trace {
                log::info(|| {
                    format!(
                        "Root::GetState: {}, site {}, menu from {source}, {} entries: {}",
                        describe(items),
                        yes_no(site.is_some()),
                        menu.len(),
                        titles(&menu)
                    )
                });
            }
            let hidden = menu.is_empty();
            if let Ok(mut seen) = self.seen.lock() {
                *seen = Seen {
                    menu,
                    anchor,
                    big,
                    trace,
                };
            }
            Ok(if hidden {
                ECS_HIDDEN.0 as u32
            } else {
                ECS_ENABLED.0 as u32
            })
        })
    }

    fn Invoke(
        &self,
        items: Ref<'_, IShellItemArray>,
        _bind: Ref<'_, IBindCtx>,
    ) -> windows_core::Result<()> {
        guard("Root::Invoke", || {
            let (cached, anchor, big) = self
                .seen
                .lock()
                .map(|s| (s.menu.clone(), s.anchor.clone(), s.big))
                .unwrap_or_default();
            root_invoke(
                &cached,
                anchor.as_deref(),
                big,
                items.as_ref(),
                self.site.get().as_ref(),
            )
        })
    }

    fn GetFlags(&self) -> windows_core::Result<u32> {
        guard("Root::GetFlags", || {
            if self.seen.lock().map(|s| s.trace).unwrap_or(false) {
                log::info(|| "Root::GetFlags: has subcommands".into());
            }
            Ok(ECF_HASSUBCOMMANDS.0 as u32)
        })
    }

    fn EnumSubCommands(&self) -> windows_core::Result<IEnumExplorerCommand> {
        guard("Root::EnumSubCommands", || {
            let (menu, anchor, big, trace) = self
                .seen
                .lock()
                .map(|s| (s.menu.clone(), s.anchor.clone(), s.big, s.trace))
                .unwrap_or_default();
            if menu.is_empty() {
                // Would mean Explorer didn't call GetState on this object first.
                log::error("EnumSubCommands called with no cached menu");
            } else if trace {
                log::info(|| {
                    format!(
                        "Root::EnumSubCommands: {} entries: {}",
                        menu.len(),
                        titles(&menu)
                    )
                });
            }
            let site = self.site.get();
            let commands: Vec<IExplorerCommand> = menu
                .into_iter()
                .enumerate()
                .map(|(index, entry)| {
                    SubCommand {
                        entry,
                        index,
                        anchor: anchor.clone(),
                        big,
                        site: Site(Mutex::new(site.clone())),
                        trace,
                        _token: ObjectToken::new(),
                    }
                    .into()
                })
                .collect();
            Ok(CommandEnum::new(commands).into())
        })
    }
}

impl IObjectWithSite_Impl for RootCommand_Impl {
    fn SetSite(&self, site: Ref<'_, IUnknown>) -> windows_core::Result<()> {
        self.site.set(site.as_ref().cloned());
        Ok(())
    }

    fn GetSite(&self, riid: *const GUID, ppv: *mut *mut c_void) -> windows_core::Result<()> {
        guard("Root::GetSite", || self.site.query(riid, ppv))
    }
}

// ---------------------------------------------------------------- subcommands

#[implement(IExplorerCommand, IObjectWithSite)]
pub(crate) struct SubCommand {
    entry: MenuEntry,
    /// Position in the menu (part of the canonical name).
    index: usize,
    /// The root's first selected path (see [`Seen::anchor`]).
    anchor: Option<PathBuf>,
    /// See [`Seen::big`].
    big: bool,
    /// Starts as the root's site; Explorer may set its own.
    site: Site,
    /// See [`Seen::trace`].
    trace: bool,
    _token: ObjectToken,
}

impl SubCommand {
    fn trace(&self, call: &str) {
        if self.trace {
            let label = self.item().map_or("(separator)", |i| i.label.as_str());
            log::info(|| format!("Sub::{call} '{label}'"));
        }
    }

    fn item(&self) -> Option<&MenuItem> {
        match &self.entry {
            MenuEntry::Item(item) => Some(item),
            MenuEntry::Separator => None,
        }
    }
}

impl IExplorerCommand_Impl for SubCommand_Impl {
    fn GetTitle(&self, _items: Ref<'_, IShellItemArray>) -> windows_core::Result<PWSTR> {
        guard("Sub::GetTitle", || {
            self.trace("GetTitle");
            match self.item() {
                Some(item) => shell_string(&item.label),
                None => shell_string(""), // separator
            }
        })
    }

    fn GetIcon(&self, _items: Ref<'_, IShellItemArray>) -> windows_core::Result<PWSTR> {
        Err(E_NOTIMPL.into())
    }

    fn GetToolTip(&self, _items: Ref<'_, IShellItemArray>) -> windows_core::Result<PWSTR> {
        Err(E_NOTIMPL.into())
    }

    fn GetCanonicalName(&self) -> windows_core::Result<GUID> {
        match self.item() {
            Some(item) => Ok(canonical_name(self.index, item)),
            None => Err(E_NOTIMPL.into()),
        }
    }

    fn GetState(
        &self,
        items: Ref<'_, IShellItemArray>,
        _ok_to_be_slow: BOOL,
    ) -> windows_core::Result<u32> {
        guard("Sub::GetState", || {
            if self.trace {
                self.trace(&format!("GetState ({}): enabled", describe(items.as_ref())));
            }
            Ok(ECS_ENABLED.0 as u32)
        })
    }

    fn Invoke(
        &self,
        items: Ref<'_, IShellItemArray>,
        _bind: Ref<'_, IBindCtx>,
    ) -> windows_core::Result<()> {
        guard("Sub::Invoke", || {
            let Some(item) = self.item() else {
                log::info(|| "Sub::Invoke on a separator: ignored".into());
                return Ok(());
            };
            let items = items.as_ref();
            let site = self.site.get();
            // Logged before asking Explorer's view, in case that call never returns.
            log::info(|| {
                format!(
                    "Sub::Invoke '{}': {}, site {}",
                    item.label,
                    describe(items),
                    yes_no(site.is_some()),
                )
            });
            let (files, source) =
                selection::files_for_invoke(items, site.as_ref(), self.anchor.as_deref(), self.big);
            log::info(|| format!("Sub::Invoke: using {} files from {source}", files.len()));
            let (action, preset) = sub_action(item, self.big, files.len());
            if action != item.action {
                log::info(|| {
                    "Sub::Invoke: menu was for 16+ files, fewer found: opening the window".into()
                });
            }
            open(&item.label, action, preset, files)
        })
    }

    fn GetFlags(&self) -> windows_core::Result<u32> {
        guard("Sub::GetFlags", || {
            self.trace("GetFlags");
            Ok(match self.entry {
                MenuEntry::Separator => ECF_ISSEPARATOR.0 as u32,
                MenuEntry::Item(_) => ECF_DEFAULT.0 as u32,
            })
        })
    }

    fn EnumSubCommands(&self) -> windows_core::Result<IEnumExplorerCommand> {
        Err(E_NOTIMPL.into())
    }
}

impl IObjectWithSite_Impl for SubCommand_Impl {
    fn SetSite(&self, site: Ref<'_, IUnknown>) -> windows_core::Result<()> {
        self.site.set(site.as_ref().cloned());
        Ok(())
    }

    fn GetSite(&self, riid: *const GUID, ppv: *mut *mut c_void) -> windows_core::Result<()> {
        guard("Sub::GetSite", || self.site.query(riid, ppv))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use morph_formats::actions;
    use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx};
    use windows::Win32::UI::Shell::Common::ITEMIDLIST;
    use windows::Win32::UI::Shell::{
        ILFree, SHCreateShellItemArrayFromIDLists, SHParseDisplayName,
    };

    /// The user's selection: five types and a folder.
    const MIXED: &[&str] = &[
        "sample.jpg",
        "text-3pages.pdf",
        "structured.docx",
        "sample.csv",
        "song.mp3",
        "A folder",
    ];

    /// A real `IShellItemArray` over `names` in a fresh temp folder, as
    /// Explorer passes it (names ending in no extension become folders).
    fn shell_items(test: &str, names: &[&str]) -> (PathBuf, IShellItemArray) {
        // SAFETY: COM init for this test thread; repeated calls are harmless.
        let _ = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
        let dir = std::env::temp_dir().join(format!("morph-shell-{test}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let paths: Vec<PathBuf> = names.iter().map(|name| dir.join(name)).collect();
        for path in &paths {
            if path.extension().is_some() {
                std::fs::write(path, b"x").unwrap();
            } else {
                std::fs::create_dir(path).unwrap();
            }
        }
        (dir, item_array(&paths))
    }

    /// A real `IShellItemArray` over existing `paths`.
    fn item_array(paths: &[PathBuf]) -> IShellItemArray {
        // SAFETY: COM init for this test thread; repeated calls are harmless.
        let _ = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
        let mut pidls: Vec<*mut ITEMIDLIST> = Vec::new();
        for path in paths {
            let mut pidl = std::ptr::null_mut();
            // SAFETY: valid NUL-terminated path; `pidl` is freed below.
            unsafe {
                SHParseDisplayName(&HSTRING::from(path.as_os_str()), None, &mut pidl, 0, None)
            }
            .unwrap();
            pidls.push(pidl);
        }
        let list: Vec<*const ITEMIDLIST> = pidls.iter().map(|p| p.cast_const()).collect();
        // SAFETY: `list` holds valid absolute PIDLs for the duration of the call.
        let items = unsafe { SHCreateShellItemArrayFromIDLists(&list) }.unwrap();
        for pidl in pidls {
            // SAFETY: allocated by SHParseDisplayName, freed once.
            unsafe { ILFree(Some(pidl.cast_const())) };
        }
        items
    }

    /// Every file in `tests/fixtures` (the user's Ctrl+A): Unicode names,
    /// broken and password-protected files, every format.
    fn all_fixtures() -> Vec<PathBuf> {
        let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let dir = manifest
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("tests")
            .join("fixtures");
        let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| p.is_file())
            .collect();
        files.sort();
        assert!(files.len() > 16, "need a big selection");
        files
    }

    #[test]
    fn big_selection_opens_the_window_with_every_file() {
        let files = all_fixtures();
        let all = item_array(&files);
        // Windows 11 builds the menu from only 16 of them ("Convert 16 files…" was
        // shown for 55): the label then has no count, as it may be wrong.
        let sample = item_array(&files[..selection::EXPLORER_MENU_SAMPLE as usize]);
        let shown = menu_for(&sample, None).0;
        let item = lone_item(&shown).expect("mixed menu has one item");
        assert_eq!(
            (item.label.as_str(), item.action),
            ("Convert files…", actions::MIXED_CONVERT)
        );
        // Invoke on the same object, or on a fresh one that never got GetState:
        // both run "Convert files…" with the invoked (full) array.
        for cached in [shown.clone(), Vec::new()] {
            let item = root_item(&cached, &menu_for(&all, None).0).expect("root runs an item");
            assert_eq!(item.action, actions::MIXED_CONVERT);
        }
        // The request lists every selected file, Unicode names included.
        let paths = selection::all_paths(&all);
        assert_eq!(paths, files);
        let request =
            std::env::temp_dir().join(format!("morph-shell-req-{}.json", std::process::id()));
        morph_formats::Request::new(paths, actions::MIXED_CONVERT, &[])
            .save_atomic(&request)
            .unwrap();
        let loaded = morph_formats::Request::load(&request).unwrap();
        let _ = std::fs::remove_file(&request);
        assert_eq!(loaded.files, files);
        assert!(loaded.files.iter().any(|f| f.ends_with("Férias 夏.jpg")));
        // On a click Windows asks again with all 55 and looks for the clicked
        // subcommand: same title, same canonical name as in the 16-item menu.
        let full = menu_for(&all, None).0;
        assert_eq!(titles(&full), titles(&shown));
        let full_item = lone_item(&full).unwrap();
        assert_eq!(canonical_name(0, full_item), canonical_name(0, item));
        // Under 16 the count is exact and shown.
        let small = item_array(&files[..15]);
        assert_eq!(titles(&menu_for(&small, None).0), "\"Convert 15 files…\"");
    }

    #[test]
    fn subcommands_have_distinct_stable_canonical_names() {
        let summary = SelectionSummary::from_paths(
            ["a.docx", "b.docx"].iter().map(std::path::Path::new),
            &EnabledCategories::default(),
        );
        let menu = build_menu(&summary);
        let names: Vec<GUID> = menu
            .iter()
            .enumerate()
            .filter_map(|(i, e)| match e {
                MenuEntry::Item(item) => Some(canonical_name(i, item)),
                MenuEntry::Separator => None,
            })
            .collect();
        assert!(names.len() > 2);
        let unique: std::collections::HashSet<_> = names.iter().map(|g| g.to_u128()).collect();
        assert_eq!(unique.len(), names.len());
        assert!(!names.contains(&ROOT_CLSID));
    }

    #[test]
    fn root_never_ignores_a_click_it_can_handle() {
        let menu = |files: &[&str]| {
            let summary = SelectionSummary::from_paths(
                files.iter().map(std::path::Path::new),
                &EnabledCategories::default(),
            );
            build_menu(&summary)
        };
        let mixed = menu(&["a.jpg", "b.mp4"]);
        let images = menu(&["a.jpg"]);
        // Lone item from the invoked array or, failing that, from GetState.
        assert_eq!(
            root_item(&[], &mixed).unwrap().action,
            actions::MIXED_CONVERT
        );
        assert_eq!(
            root_item(&mixed, &images).unwrap().action,
            actions::MIXED_CONVERT
        );
        // A real submenu: the click only opens it.
        assert_eq!(root_item(&images, &images), None);
        assert_eq!(root_item(&[], &images), None);
        // Nothing usable at all: the window still opens (and lists skipped files).
        assert_eq!(root_item(&[], &[]).unwrap().action, actions::MIXED_CONVERT);
    }

    #[test]
    fn mixed_selection_with_a_folder_runs_from_the_root() {
        for (test, names) in [("folder", MIXED), ("no-folder", &MIXED[..5])] {
            let (dir, items) = shell_items(test, names);
            let menu = menu_for(&items, None).0;
            // One entry, "Convert 5 files…": Windows 11 then invokes the root.
            let item = lone_item(&menu).expect("mixed menu has one item");
            assert_eq!(item.action, actions::MIXED_CONVERT, "{test}");
            assert_eq!(item.label, "Convert 5 files…", "{test}");
            // The request gets every selected path, the folder too (the window skips it).
            let files = selection::all_paths(&items);
            assert_eq!(files.len(), names.len(), "{test}");
            assert!(files.iter().all(|f| f.starts_with(&dir)), "{test}");
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    #[test]
    fn root_does_nothing_for_a_real_submenu() {
        let menu = |files: &[&str]| {
            let summary = SelectionSummary::from_paths(
                files.iter().map(std::path::Path::new),
                &EnabledCategories::default(),
            );
            build_menu(&summary)
        };
        assert_eq!(lone_item(&menu(&["a.jpg"])), None);
        assert_eq!(lone_item(&menu(&["a.jpg", "b.pdf"])), None); // + "Combine into one PDF…"
        assert_eq!(lone_item(&[]), None);
        assert_eq!(lone_item(&[MenuEntry::Separator]), None);
    }

    #[test]
    fn a_view_selection_replaces_a_cut_array_only_if_it_is_the_same_selection() {
        let p = |names: &[&str]| -> Vec<PathBuf> {
            names
                .iter()
                .map(|n| PathBuf::from(format!(r"C:\d\{n}")))
                .collect()
        };
        let cut = p(&["a.jpg", "b.jpg"]);
        let full = p(&["a.jpg", "b.jpg", "c.pdf"]);
        assert!(selection::prefer_view(&cut, None, &full));
        // Another window's selection, or not bigger: keep the array.
        assert!(!selection::prefer_view(
            &cut,
            None,
            &p(&["x.jpg", "y.jpg", "z.jpg"])
        ));
        assert!(!selection::prefer_view(&full, None, &cut));
        // No array (a subcommand's Invoke): the anchor from GetState decides.
        let anchor = PathBuf::from(r"C:\d\a.jpg");
        assert!(selection::prefer_view(&[], Some(&anchor), &full));
        assert!(!selection::prefer_view(
            &[],
            Some(Path::new(r"C:\e\a.jpg")),
            &full
        ));
        assert!(!selection::prefer_view(&[], None, &full));
    }

    /// The user's folder: 42 PNGs, 40 PDFs, 2 MP3s; the first 16 by name are PNGs.
    fn png_pdf_mp3() -> Vec<PathBuf> {
        let mut files: Vec<PathBuf> = (1..=42)
            .map(|i| format!("a{i:02}.png"))
            .chain((1..=40).map(|i| format!("b{i:02}.pdf")))
            .chain((1..=2).map(|i| format!("c{i}.mp3")))
            .map(|n| PathBuf::from(format!(r"C:\d\{n}")))
            .collect();
        files.sort();
        files
    }

    fn names(menu: &[MenuEntry]) -> Vec<u128> {
        menu.iter()
            .enumerate()
            .filter_map(|(i, e)| match e {
                MenuEntry::Item(item) => Some(canonical_name(i, item).to_u128()),
                MenuEntry::Separator => None,
            })
            .collect()
    }

    #[test]
    fn a_one_type_sample_of_a_mixed_selection_gets_the_full_selection_menu() {
        let full = png_pdf_mp3();
        let sample = &full[..selection::EXPLORER_MENU_SAMPLE as usize];
        assert!(sample.iter().all(|p| p.extension().unwrap() == "png"));
        let view = || Some((full.clone(), full.len()));
        // Click-time build: the full array (with or without the view).
        let (at_click, _) = menu_for_selection(&full, full.len(), view);
        let (at_click_no_view, _) = menu_for_selection(&full, full.len(), || None);
        assert_eq!(at_click, at_click_no_view);
        let item = lone_item(&at_click).unwrap();
        assert_eq!(
            (item.label.as_str(), item.action),
            ("Convert files…", actions::MIXED_CONVERT)
        );
        // First build, from the 16 PNGs: same menu, read from the view.
        let (shown, source) = menu_for_selection(sample, sample.len(), view);
        assert_eq!(source, "Explorer view");
        assert_eq!(shown, at_click);
        assert_eq!(names(&shown), names(&at_click));
    }

    #[test]
    fn a_sample_without_its_view_never_gets_a_one_type_menu() {
        let full = png_pdf_mp3();
        let sample = &full[..selection::EXPLORER_MENU_SAMPLE as usize];
        let other_window: Vec<PathBuf> = (0..30)
            .map(|i| PathBuf::from(format!(r"C:\e\x{i}.png")))
            .collect();
        for view in [None, Some((other_window.clone(), other_window.len()))] {
            let (shown, source) = menu_for_selection(sample, sample.len(), || view.clone());
            assert_eq!(source, "generic (maybe a sample, no view)");
            // The generic entry is the full mixed build's entry: Explorer matches it.
            let (at_click, _) = menu_for_selection(&full, full.len(), || None);
            assert_eq!(titles(&shown), "\"Convert files…\"");
            assert_eq!(names(&shown), names(&at_click));
            assert_eq!(lone_item(&shown).unwrap().action, actions::MIXED_CONVERT);
        }
        // Exactly 16 selected, one type: the view says so, the menu is the image menu.
        let (menu, _) = menu_for_selection(sample, sample.len(), || {
            Some((sample.to_vec(), sample.len()))
        });
        assert_eq!(lone_item(&menu), None);
        assert!(titles(&menu).contains("Convert to JPG…"));
        // Small selections never ask the view.
        let (menu, _) = menu_for_selection(&full[..3], 3, || panic!("view asked"));
        assert!(titles(&menu).contains("Convert to JPG…"));
    }

    #[test]
    fn a_stale_big_menu_click_with_few_files_opens_the_window() {
        let jpg = MenuItem {
            label: "Convert to JPG…".to_owned(),
            action: actions::IMAGE_CONVERT,
            preset: &[("format", "jpg")],
        };
        assert_eq!(sub_action(&jpg, true, 1).0, actions::MIXED_CONVERT);
        assert_eq!(sub_action(&jpg, true, 84).0, actions::IMAGE_CONVERT);
        assert_eq!(sub_action(&jpg, false, 1), (jpg.action, jpg.preset));
    }

    /// Needs an open Explorer window with more than 16 files selected
    /// (e.g. Ctrl+A in `tests\fixtures`).
    #[test]
    #[ignore]
    fn a_cut_or_missing_array_is_completed_from_the_explorer_window() {
        // SAFETY: COM init for this test thread; repeated calls are harmless.
        let _ = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
        let full = crate::view::window_selections()
            .iter()
            .map(selection::all_paths)
            .max_by_key(Vec::len)
            .expect("an Explorer window with a selection");
        assert!(full.len() > 16, "select more than 16 files in Explorer");
        let cut = item_array(&full[..16]);
        let (files, source) = selection::files_for_invoke(Some(&cut), None, None, false);
        assert_eq!((files.len(), source), (full.len(), "Explorer window list"));
        let (files, _) = selection::files_for_invoke(None, None, Some(&full[0]), false);
        assert_eq!(files.len(), full.len());
    }

    #[test]
    fn a_small_invoked_array_is_used_as_is() {
        let (dir, items) = shell_items("small-invoke", MIXED);
        let (files, source) = selection::files_for_invoke(Some(&items), None, None, false);
        assert_eq!(files.len(), MIXED.len());
        assert_eq!(source, "invoked array");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn commands_keep_the_site_and_subcommands_inherit_it() {
        let (dir, items) = shell_items("site", &["a.jpg"]);
        let root: IExplorerCommand = RootCommand::new().into();
        let site: IUnknown = RootCommand::new().into();
        let with_site: IObjectWithSite = root.cast().unwrap();
        // SAFETY: plain COM calls on objects created above.
        unsafe {
            with_site.SetSite(&site).unwrap();
            let back: IUnknown = with_site.GetSite().unwrap();
            assert_eq!(back, site);
            root.GetState(&items, false).unwrap();
            let subs = root.EnumSubCommands().unwrap();
            let mut out = [None];
            let mut got = 0;
            let _ = subs.Next(&mut out, Some(&mut got));
            let sub: IObjectWithSite = out[0].take().unwrap().cast().unwrap();
            let inherited: IUnknown = sub.GetSite().unwrap();
            assert_eq!(inherited, site);
            with_site.SetSite(None::<&IUnknown>).unwrap();
            assert!(with_site.GetSite::<IUnknown>().is_err());
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
