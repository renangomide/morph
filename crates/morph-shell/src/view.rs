// SPDX-License-Identifier: GPL-3.0-or-later
//! The selection as Explorer's folder view sees it.
//!
//! The Windows 11 menu hands an `IExplorerCommand` at most 16 of the selected
//! items, and may hand a subcommand's `Invoke` no array at all. When the user
//! clicks, we ask the view itself: first through the site Explorer gave us
//! (`IObjectWithSite`), else through the open Explorer windows (foreground first).
//! While the menu is being built for a big (maybe sampled) selection, only the
//! site's view is asked ([`from_site`]); the window list is for `Invoke` only.

use std::mem::ManuallyDrop;
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{CLSCTX_ALL, CoCreateInstance, IServiceProvider};
use windows::Win32::System::Variant::{VARIANT, VARIANT_0, VARIANT_0_0, VARIANT_0_0_0, VT_I4};
use windows::Win32::UI::Shell::{
    IFolderView2, IShellBrowser, IShellItemArray, IShellWindows, SID_STopLevelBrowser, ShellWindows,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GA_ROOT, GA_ROOTOWNER, GetAncestor, GetForegroundWindow,
};
use windows_core::{IUnknown, Interface};

/// Explorer windows (tabs) looked at, at most.
const MAX_WINDOWS: i32 = 64;

/// Selection of the view that owns `site` (the site Explorer set on a command).
pub(crate) fn from_site(site: &IUnknown) -> Option<IShellItemArray> {
    let provider: IServiceProvider = site.cast().ok()?;
    // SAFETY: plain COM call on a valid interface.
    let browser: IShellBrowser = unsafe { provider.QueryService(&SID_STopLevelBrowser) }.ok()?;
    selection_of(&browser)
}

fn selection_of(browser: &IShellBrowser) -> Option<IShellItemArray> {
    // SAFETY: plain COM calls on valid interfaces.
    let view = unsafe { browser.QueryActiveShellView() }.ok()?;
    let folder_view: IFolderView2 = view.cast().ok()?;
    // SAFETY: plain COM call; `false`: an empty selection is not the folder.
    unsafe { folder_view.GetSelection(false) }.ok()
}

fn variant_i4(value: i32) -> VARIANT {
    VARIANT {
        Anonymous: VARIANT_0 {
            Anonymous: ManuallyDrop::new(VARIANT_0_0 {
                vt: VT_I4,
                wReserved1: 0,
                wReserved2: 0,
                wReserved3: 0,
                Anonymous: VARIANT_0_0_0 { lVal: value },
            }),
        },
    }
}

/// Selections of the open Explorer windows (one per tab), tabs of the
/// foreground window first. Tabs of one window can't be told apart reliably
/// (all report visible), so the caller keeps the selection that holds the
/// clicked item ([`crate::selection::prefer_view`]).
pub(crate) fn window_selections() -> Vec<IShellItemArray> {
    // SAFETY: plain Win32 calls; invalid handles just yield null results.
    let frames = unsafe {
        let foreground = GetForegroundWindow();
        [
            GetAncestor(foreground, GA_ROOTOWNER),
            GetAncestor(foreground, GA_ROOT),
        ]
    };
    let in_foreground = |hwnd: HWND| !hwnd.is_invalid() && frames.contains(&hwnd);
    let (mut front, mut back) = (Vec::new(), Vec::new());
    // SAFETY: standard COM activation of the shell's window list.
    let Ok(windows) =
        (unsafe { CoCreateInstance::<_, IShellWindows>(&ShellWindows, None, CLSCTX_ALL) })
    else {
        return front;
    };
    // SAFETY: plain COM call.
    let count = unsafe { windows.Count() }.unwrap_or(0).min(MAX_WINDOWS);
    for i in 0..count {
        // SAFETY: plain COM call with a VT_I4 index (nothing to free).
        let Ok(window) = (unsafe { windows.Item(&variant_i4(i)) }) else {
            continue;
        };
        let Ok(provider) = window.cast::<IServiceProvider>() else {
            continue;
        };
        // SAFETY: plain COM calls on valid interfaces.
        let Ok(browser) =
            (unsafe { provider.QueryService::<IShellBrowser>(&SID_STopLevelBrowser) })
        else {
            continue;
        };
        let Some(selection) = selection_of(&browser) else {
            continue; // nothing selected there
        };
        // SAFETY: plain COM call (IOleWindow::GetWindow) and Win32 call on its result.
        let frame = unsafe { browser.GetWindow() }.map(|tab| unsafe { GetAncestor(tab, GA_ROOT) });
        if frame.is_ok_and(in_foreground) {
            front.push(selection);
        } else {
            back.push(selection);
        }
    }
    front.extend(back);
    front
}
