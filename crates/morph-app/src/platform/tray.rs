// SPDX-License-Identifier: GPL-3.0-or-later
//! Tray icon shown only while a job runs in the background (SPEC §3.3, §14).
//! Tooltip shows progress; the menu has "Show progress" and "Cancel".
//!
//! Create, update and drop it on the UI (Slint event loop) thread. Dropping it
//! removes the icon.
//!
//! tray-icon creates a hidden message window on the creating thread, so that
//! thread must pump Win32 messages: the Slint (winit) event loop does. Events are
//! queued on tray-icon's / muda's global channels and drained by [`Tray::try_recv`].

use std::io::Cursor;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::Context;
use tray_icon::menu::{Menu, MenuEvent, MenuId, MenuItem};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXSMICON};

use crate::strings::{TRAY_CANCEL, TRAY_SHOW_PROGRESS};

/// The app icon, embedded so unpackaged runs have it too.
const ICON_ICO: &[u8] = include_bytes!("../../../../packaging/Assets/morph.ico");

/// Windows limits tray tooltips to 128 UTF-16 units including the NUL.
const TOOLTIP_MAX_UNITS: usize = 127;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayEvent {
    /// Menu "Show progress" or a left click on the icon.
    ShowProgress,
    /// Menu "Cancel".
    Cancel,
}

pub struct Tray {
    icon: TrayIcon,
    show_id: MenuId,
    cancel_id: MenuId,
}

impl Tray {
    /// Add the icon with this tooltip.
    pub fn new(tooltip: &str) -> anyhow::Result<Tray> {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let show_id = MenuId::new(format!("morph-tray-{n}-show"));
        let cancel_id = MenuId::new(format!("morph-tray-{n}-cancel"));

        let menu = Menu::new();
        let show = MenuItem::with_id(show_id.clone(), TRAY_SHOW_PROGRESS, true, None);
        let cancel = MenuItem::with_id(cancel_id.clone(), TRAY_CANCEL, true, None);
        menu.append(&show).context("tray menu")?;
        menu.append(&cancel).context("tray menu")?;

        let icon = TrayIconBuilder::new()
            .with_id(format!("morph-tray-{n}"))
            .with_icon(app_icon()?)
            .with_tooltip(clip_tooltip(tooltip))
            .with_menu(Box::new(menu))
            .with_menu_on_left_click(false)
            .with_menu_on_right_click(true)
            .build()
            .context("could not create the tray icon")?;
        Ok(Tray {
            icon,
            show_id,
            cancel_id,
        })
    }

    /// Update the tooltip, e.g. "Morph: converting 7 / 12 (58%)".
    pub fn set_tooltip(&self, text: &str) {
        let _ = self.icon.set_tooltip(Some(clip_tooltip(text)));
    }

    /// Next pending click/menu event, if any. Poll it from a Slint timer.
    ///
    /// Drains the global queues; events for other tray icons/menus are dropped.
    pub fn try_recv(&self) -> Option<TrayEvent> {
        while let Ok(ev) = MenuEvent::receiver().try_recv() {
            if ev.id == self.show_id {
                return Some(TrayEvent::ShowProgress);
            }
            if ev.id == self.cancel_id {
                return Some(TrayEvent::Cancel);
            }
        }
        while let Ok(ev) = TrayIconEvent::receiver().try_recv() {
            if let TrayIconEvent::Click {
                id,
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = &ev
                && id == self.icon.id()
            {
                return Some(TrayEvent::ShowProgress);
            }
        }
        None
    }
}

/// Clip to what the notification area shows, on a char boundary.
fn clip_tooltip(text: &str) -> String {
    let mut units = 0;
    let mut out = String::new();
    for c in text.chars() {
        units += c.len_utf16();
        if units > TOOLTIP_MAX_UNITS {
            break;
        }
        out.push(c);
    }
    out
}

/// The embedded icon at the small-icon size (16 px at 100%, 32 px at 200%).
fn app_icon() -> anyhow::Result<Icon> {
    // SAFETY: GetSystemMetrics has no preconditions.
    let want = u32::try_from(unsafe { GetSystemMetrics(SM_CXSMICON) })
        .ok()
        .filter(|&w| w > 0)
        .unwrap_or(16);
    let (rgba, w, h) = decode_icon(ICON_ICO, want)?;
    Icon::from_rgba(rgba, w, h).context("bad tray icon")
}

/// Decode the ICO entry best suited to `want` px: the smallest at least that
/// big, else the biggest. Returns RGBA, width, height.
fn decode_icon(ico: &[u8], want: u32) -> anyhow::Result<(Vec<u8>, u32, u32)> {
    let dir = ico::IconDir::read(Cursor::new(ico)).context("bad icon file")?;
    let mut entries: Vec<_> = dir.entries().iter().collect();
    entries.sort_by_key(|e| e.width());
    let entry = entries
        .iter()
        .find(|e| e.width() >= want)
        .or(entries.last())
        .context("empty icon file")?;
    let img = entry.decode().context("bad icon image")?;
    let (w, h) = (img.width(), img.height());
    Ok((img.into_rgba_data(), w, h))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_icon_decodes_at_tray_sizes() {
        for want in [16, 20, 24, 32, 48] {
            let (rgba, w, h) = decode_icon(ICON_ICO, want).unwrap();
            assert!(w >= want && w == h, "want {want}, got {w}x{h}");
            assert_eq!(rgba.len(), (w * h * 4) as usize);
        }
        let (_, w, _) = decode_icon(ICON_ICO, 1000).unwrap();
        assert_eq!(w, 256);
    }

    #[test]
    fn tooltip_is_clipped_on_char_boundary() {
        assert_eq!(clip_tooltip("Morph: 7 / 12"), "Morph: 7 / 12");
        let long = "é".repeat(200);
        assert_eq!(clip_tooltip(&long).chars().count(), TOOLTIP_MAX_UNITS);
        let emoji = "😀".repeat(100); // 2 UTF-16 units each
        assert_eq!(clip_tooltip(&emoji).chars().count(), TOOLTIP_MAX_UNITS / 2);
    }

    #[test]
    #[ignore = "adds a tray icon; run on a desktop"]
    fn desktop_tray_create_update_drop() {
        let tray = Tray::new("Morph test").unwrap();
        tray.set_tooltip("Morph: converting 7 / 12 (58%)");
        assert_eq!(tray.try_recv(), None);
        drop(tray);
    }
}
