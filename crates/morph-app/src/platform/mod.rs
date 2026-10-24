// SPDX-License-Identifier: GPL-3.0-or-later
//! Windows integration used by the window: toast notifications, the tray icon
//! shown during background jobs, and opening files/folders in Explorer.
//!
//! The signatures here are the contract between the window code and the
//! platform code; change them only together.

pub mod shell;
pub mod toast;
pub mod tray;

/// Files dropped from Explorer onto the window (main window drop zone).
pub mod file_drop;
/// Window placement on the cursor's monitor (SPEC §3.2).
pub mod placement;
