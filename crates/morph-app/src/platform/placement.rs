// SPDX-License-Identifier: GPL-3.0-or-later
//! Window placement (SPEC §3.2): centered on the work area of the monitor
//! under the mouse cursor, on top only while it opens.
//!
//! Two steps, both best effort:
//! 1. Before `show()`: a position from the expected size, so the window is
//!    created on the right monitor (and so at that monitor's DPI) roughly centered.
//! 2. Once the native window exists: centered exactly from its real outer size,
//!    shrunk if it is bigger than the work area, raised on top for a moment.

use std::time::Duration;

use slint::ComponentHandle;
use slint::winit_030::WinitWindowAccessor;
use slint::winit_030::winit::dpi::{PhysicalPosition, PhysicalSize};
use slint::winit_030::winit::window::WindowLevel;
use windows::Win32::Foundation::POINT;
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint,
};
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::WindowsAndMessaging::GetCursorPos;

/// The window's preferred sizes in logical px (`MainWindow` in `ui/main.slint`):
/// the conversion window, and the main window (Start menu launch).
/// Only for the first guess; step 2 uses the real size.
pub const CONVERSION_SIZE: (f64, f64) = (600.0, 640.0);
pub const MAIN_SIZE: (f64, f64) = (780.0, 580.0);
/// How long the window stays always-on-top after it appears.
const ON_TOP_FOR: Duration = Duration::from_millis(600);

/// A rectangle in physical (screen) pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

/// Where a window of outer size `outer` goes in `work`: centered; a dimension
/// bigger than the work area is shrunk to it and starts at its edge (so the
/// title bar stays reachable). Returns the outer rectangle.
pub fn center_in(work: Rect, outer: (i32, i32)) -> Rect {
    let w = outer.0.clamp(1, work.w.max(1));
    let h = outer.1.clamp(1, work.h.max(1));
    Rect {
        x: work.x + (work.w - w) / 2,
        y: work.y + (work.h - h) / 2,
        w,
        h,
    }
}

/// Work area and scale factor of the monitor under the cursor.
fn cursor_monitor() -> Option<(Rect, f64)> {
    // SAFETY: plain Win32 queries; every pointer is to a local we own.
    unsafe {
        let mut pt = POINT::default();
        GetCursorPos(&mut pt).ok()?;
        let monitor = MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST);
        if monitor.is_invalid() {
            return None;
        }
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if !GetMonitorInfoW(monitor, &mut info).as_bool() {
            return None;
        }
        let (mut dpi_x, mut dpi_y) = (96u32, 96u32);
        let scale = match GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y) {
            Ok(()) if dpi_x > 0 => f64::from(dpi_x) / 96.0,
            _ => 1.0,
        };
        let r = info.rcWork;
        Some((
            Rect {
                x: r.left,
                y: r.top,
                w: r.right - r.left,
                h: r.bottom - r.top,
            },
            scale,
        ))
    }
}

/// Center `component`'s window (about `expected` logical px, see
/// [`CONVERSION_SIZE`]) on the cursor's monitor. Call before `show()`.
/// Best effort.
pub fn place_on_cursor_monitor<C: ComponentHandle + 'static>(component: &C, expected: (f64, f64)) {
    let Some((work, scale)) = cursor_monitor() else {
        return;
    };
    let window = component.window();
    // Step 1: first guess, before the native window exists.
    let guess = (
        (expected.0 * scale).round() as i32,
        (expected.1 * scale).round() as i32,
    );
    let at = center_in(work, guess);
    window.set_position(slint::PhysicalPosition::new(at.x, at.y));

    // Step 2: exact, once the event loop has created the window.
    let weak = component.as_weak();
    let spawned = slint::spawn_local(async move {
        let Some(component) = weak.upgrade() else {
            return;
        };
        let Ok(win) = component.window().winit_window().await else {
            return;
        };
        drop(component);
        let outer = win.outer_size();
        let inner = win.inner_size();
        let frame = (
            outer.width.saturating_sub(inner.width) as i32,
            outer.height.saturating_sub(inner.height) as i32,
        );
        let (ow, oh) = (outer.width as i32, outer.height as i32);
        let to = center_in(work, (ow, oh));
        if to.w < ow || to.h < oh {
            let _ = win.request_inner_size(PhysicalSize::new(
                (to.w - frame.0).max(1) as u32,
                (to.h - frame.1).max(1) as u32,
            ));
        }
        win.set_outer_position(PhysicalPosition::new(to.x, to.y));
        win.set_window_level(WindowLevel::AlwaysOnTop);
        win.focus_window();
        slint::Timer::single_shot(ON_TOP_FOR, move || {
            win.set_window_level(WindowLevel::Normal);
        });
    });
    if let Err(err) = spawned {
        crate::applog::error("could not place the window", &err);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WORK: Rect = Rect {
        x: 0,
        y: 0,
        w: 1920,
        h: 1040,
    };

    #[test]
    fn centers_in_the_work_area() {
        assert_eq!(
            center_in(WORK, (620, 640)),
            Rect {
                x: 650,
                y: 200,
                w: 620,
                h: 640
            }
        );
    }

    #[test]
    fn second_monitor_left_of_primary_with_negative_coordinates() {
        let left = Rect {
            x: -2560,
            y: -200,
            w: 2560,
            h: 1400,
        };
        let r = center_in(left, (1240, 1280));
        assert_eq!((r.x, r.y), (-2560 + 660, -200 + 60));
    }

    #[test]
    fn work_area_offset_by_a_top_or_left_taskbar() {
        let work = Rect {
            x: 48,
            y: 0,
            w: 1872,
            h: 1080,
        };
        assert_eq!(center_in(work, (600, 600)).x, 48 + 636);
    }

    #[test]
    fn bigger_than_the_work_area_is_shrunk_and_kept_on_screen() {
        // 620×640 at 200% on a 1366×728 work area.
        let small = Rect {
            x: 0,
            y: 0,
            w: 1366,
            h: 728,
        };
        assert_eq!(
            center_in(small, (1240, 1280)),
            Rect {
                x: 63,
                y: 0,
                w: 1240,
                h: 728
            }
        );
    }
}
