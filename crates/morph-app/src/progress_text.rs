// SPDX-License-Identifier: GPL-3.0-or-later
//! Puts the progress / results view's text (`ProgressText` global) into the
//! window, and turns a [`RowView`] into a Slint `ProgressRow`. The text itself
//! lives in `strings/background.rs`.

use crate::progress::RowView;
use crate::{MainWindow, ProgressRow, ProgressText, strings};
use slint::{ComponentHandle, SharedString};

fn shared(text: &str) -> SharedString {
    SharedString::from(text)
}

/// Set every text of the progress view. Called once at startup.
pub fn apply(window: &MainWindow) {
    use strings as s;
    let t = window.global::<ProgressText>();
    t.set_close(shared(s::CLOSE));
    t.set_cancel_all(shared(s::CANCEL_ALL));
    t.set_run_in_background(shared(s::RUN_IN_BACKGROUND));
    t.set_open_folder(shared(s::OPEN_FOLDER));
    t.set_open(shared(s::OPEN));
    t.set_show_in_folder(shared(s::SHOW_IN_FOLDER));
    t.set_details(shared(s::DETAILS));
    t.set_copy(shared(s::COPY));
    t.set_copied(shared(s::COPIED));
    t.set_details_title(shared(s::DETAILS_TITLE));
    t.set_adjust_options(shared(s::ADJUST_OPTIONS));
}

/// One results row as the view shows it.
pub fn row(v: RowView) -> ProgressRow {
    ProgressRow {
        status: v.status.index(),
        name: SharedString::from(v.name),
        target: SharedString::from(v.target),
        sizes: SharedString::from(v.sizes),
        change: SharedString::from(v.change),
        smaller: v.smaller,
        message: SharedString::from(v.message),
        details: SharedString::from(v.details),
        fraction: v.fraction,
        has_outputs: v.has_outputs,
        indeterminate: v.indeterminate,
    }
}
