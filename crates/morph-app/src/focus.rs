// SPDX-License-Identifier: GPL-3.0-or-later
//! Whether one of this process's windows is the foreground window
//! (the toast is shown only when the user isn't looking at the window, SPEC §3.3).

use windows::Win32::System::Threading::GetCurrentProcessId;
use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};

pub fn app_is_foreground() -> bool {
    // SAFETY: plain Win32 queries; the only pointer is the out-param we own.
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.is_invalid() {
            return false;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        pid != 0 && pid == GetCurrentProcessId()
    }
}
