// SPDX-License-Identifier: GPL-3.0-or-later
//! Writing the request file and starting `Morph.exe` (SPEC §5.2).

use crate::{log, module_handle};
use morph_formats::{ARG_REQUEST, Request, paths};
use std::ffi::OsString;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::PathBuf;
use windows::Win32::Foundation::{CloseHandle, E_FAIL};
use windows::Win32::System::Com::{CLSCTX_LOCAL_SERVER, CoCreateInstance};
use windows::Win32::System::LibraryLoader::GetModuleFileNameW;
use windows::Win32::System::Threading::{
    CREATE_UNICODE_ENVIRONMENT, CreateProcessW, PROCESS_INFORMATION, STARTUPINFOW,
};
use windows::Win32::UI::Shell::{
    AO_NONE, ApplicationActivationManager, IApplicationActivationManager,
};
use windows_core::{HSTRING, PCWSTR, PWSTR};

/// Full path of a file that sits next to this DLL (e.g. `morph.ico`, `Morph.exe`).
pub(crate) fn file_next_to_dll(name: &str) -> Option<PathBuf> {
    let module = module_handle()?;
    let mut buf = vec![0u16; 1024];
    loop {
        // SAFETY: the buffer is valid for its length; `module` is this DLL's handle.
        let len = unsafe { GetModuleFileNameW(Some(module), &mut buf) } as usize;
        if len == 0 {
            return None;
        }
        if len < buf.len() {
            let dll = PathBuf::from(OsString::from_wide(&buf[..len]));
            return Some(dll.parent()?.join(name));
        }
        if buf.len() >= 32 * 1024 {
            return None;
        }
        buf.resize(buf.len() * 2, 0);
    }
}

fn fail(msg: impl Into<String>) -> windows_core::Error {
    windows_core::Error::new(E_FAIL, msg.into())
}

/// Write the request and open the conversion window.
pub(crate) fn open_window(
    files: Vec<PathBuf>,
    action: &str,
    preset: &[(&str, &str)],
) -> windows_core::Result<()> {
    let request_path = match write_request(files, action, preset) {
        Ok(path) => Some(path),
        Err(e) => {
            // Still open Morph (its home window) so the click isn't lost silently.
            log::error(&format!("{e}; opening Morph without the selection"));
            None
        }
    };
    let args = match &request_path {
        Some(path) => format!("{ARG_REQUEST} \"{}\"", path.display()),
        None => String::new(),
    };
    let result = activate_packaged_app(&args).or_else(|e| {
        // Fallback (SPEC §13): start Morph.exe from the package folder directly.
        log::error(&format!(
            "ActivateApplication failed ({e}); falling back to Morph.exe"
        ));
        spawn_exe_next_to_dll(&args)
    });
    if result.is_err()
        && let Some(path) = &request_path
    {
        // Nobody will read it.
        let _ = std::fs::remove_file(path);
    }
    result
}

fn write_request(
    files: Vec<PathBuf>,
    action: &str,
    preset: &[(&str, &str)],
) -> windows_core::Result<PathBuf> {
    let root = paths::shell_data_dir().ok_or_else(|| fail("no data folder"))?;
    let request_path = paths::requests_dir(&root).join(format!("{}.json", uuid::Uuid::new_v4()));
    Request::new(files, action, preset)
        .save_atomic(&request_path)
        .map_err(|e| fail(format!("writing request: {e}")))?;
    Ok(request_path)
}

fn activate_packaged_app(args: &str) -> windows_core::Result<()> {
    let aumid = paths::app_user_model_id().ok_or_else(|| fail("no package family name"))?;
    // SAFETY: standard COM activation; COM is initialized on the calling shell thread.
    let manager: IApplicationActivationManager =
        unsafe { CoCreateInstance(&ApplicationActivationManager, None, CLSCTX_LOCAL_SERVER) }?;
    // SAFETY: valid HSTRING arguments for the duration of the call.
    unsafe { manager.ActivateApplication(&HSTRING::from(aumid), &HSTRING::from(args), AO_NONE) }?;
    Ok(())
}

fn to_wide_nul(s: &std::ffi::OsStr) -> Vec<u16> {
    s.encode_wide().chain([0]).collect()
}

/// Start `Morph.exe` without inheriting any handles from the host (Explorer).
fn spawn_exe_next_to_dll(args: &str) -> windows_core::Result<()> {
    let exe = file_next_to_dll("Morph.exe").ok_or_else(|| fail("can't locate Morph.exe"))?;
    let exe_w = to_wide_nul(exe.as_os_str());
    let mut cmdline = to_wide_nul(format!("\"{}\" {args}", exe.display()).as_ref());
    let startup = STARTUPINFOW {
        cb: std::mem::size_of::<STARTUPINFOW>() as u32,
        ..Default::default()
    };
    let mut info = PROCESS_INFORMATION::default();
    // SAFETY: all strings are NUL-terminated and outlive the call; `cmdline` is
    // mutable as CreateProcessW requires; handles in `info` are closed below.
    unsafe {
        CreateProcessW(
            PCWSTR(exe_w.as_ptr()),
            Some(PWSTR(cmdline.as_mut_ptr())),
            None,
            None,
            false, // bInheritHandles
            CREATE_UNICODE_ENVIRONMENT,
            None,
            None,
            &startup,
            &mut info,
        )
    }?;
    // SAFETY: handles returned by CreateProcessW, closed exactly once.
    unsafe {
        let _ = CloseHandle(info.hThread);
        let _ = CloseHandle(info.hProcess);
    }
    Ok(())
}
