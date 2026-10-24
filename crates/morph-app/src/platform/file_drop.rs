// SPDX-License-Identifier: GPL-3.0-or-later
//! Files dropped from Explorer onto the window (SPEC §3.4 drop zone).
//!
//! Slint's `DropArea` doesn't receive drops from other apps on the winit
//! backend, so this listens to winit's `DroppedFile` (one event per file) and
//! hands the whole batch over once the drop is complete. `HoveredFile` /
//! `HoveredFileCancelled` drive the drop zone's drag-over highlight.

use slint::winit_030::winit::event::WindowEvent;
use slint::winit_030::{EventResult, WinitWindowAccessor};
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

/// Call `on_drop` with all files of each drop onto `window`. `accept` is asked
/// first (e.g. "is the drop zone showing?"); refused drops are ignored.
/// `on_hover(true)` while accepted files are dragged over the window,
/// `on_hover(false)` when they leave or are dropped.
pub fn on_files_dropped(
    window: &slint::Window,
    accept: impl Fn() -> bool + 'static,
    on_hover: impl Fn(bool) + 'static,
    on_drop: impl Fn(Vec<PathBuf>) + 'static,
) {
    let pending: Rc<RefCell<Vec<PathBuf>>> = Rc::default();
    let on_drop = Rc::new(on_drop);
    window.on_winit_window_event(move |_, event| {
        let path = match event {
            WindowEvent::HoveredFile(_) => {
                on_hover(accept());
                return EventResult::Propagate;
            }
            WindowEvent::HoveredFileCancelled => {
                on_hover(false);
                return EventResult::Propagate;
            }
            WindowEvent::DroppedFile(path) => path,
            _ => return EventResult::Propagate,
        };
        on_hover(false);
        if !accept() {
            return EventResult::PreventDefault;
        }
        let first = pending.borrow().is_empty();
        pending.borrow_mut().push(path.clone());
        if first {
            // The other files of this drop arrive before the next loop turn.
            let (pending, on_drop) = (pending.clone(), on_drop.clone());
            slint::Timer::single_shot(Duration::ZERO, move || {
                let files = std::mem::take(&mut *pending.borrow_mut());
                on_drop(files);
            });
        }
        EventResult::PreventDefault
    });
}
