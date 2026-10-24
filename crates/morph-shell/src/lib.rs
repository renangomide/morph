// SPDX-License-Identifier: GPL-3.0-or-later
//! `morph_shell.dll`: the "Morph ▸" entry in the modern Explorer context menu.
//!
//! Loaded by Explorer (in a COM surrogate with package identity). Hard rules:
//! - never crash Explorer: every COM entry point runs inside [`guard`] (catch_unwind);
//! - never slow Explorer: menu decisions use file extensions only;
//! - no `unwrap`/`expect` anywhere in this crate.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod command;
mod enumerator;
mod factory;
mod launch;
mod log;
mod selection;
mod view;

use std::ffi::c_void;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicIsize, AtomicUsize, Ordering};
use windows::Win32::Foundation::{
    CLASS_E_CLASSNOTAVAILABLE, E_FAIL, E_POINTER, HINSTANCE, HMODULE, S_FALSE, S_OK,
};
use windows::Win32::System::Com::IClassFactory;
use windows::Win32::System::LibraryLoader::{
    GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
    GetModuleHandleExW,
};
use windows::Win32::System::SystemServices::DLL_PROCESS_ATTACH;
use windows_core::{BOOL, GUID, HRESULT, Interface, PCWSTR};

/// CLSID of the root command (same value as in the MSIX manifest).
pub(crate) const ROOT_CLSID: GUID = GUID::from_u128(morph_formats::ROOT_CLSID_U128);

/// Handle of this DLL, saved in `DllMain`, used to find files next to it.
static MODULE: AtomicIsize = AtomicIsize::new(0);

/// Live COM objects created by this DLL, for `DllCanUnloadNow`.
static OBJECTS: AtomicUsize = AtomicUsize::new(0);
static LOCKS: AtomicUsize = AtomicUsize::new(0);

/// Handle of this DLL. Saved in `DllMain`; if that never ran, ask Windows which
/// module contains this function (never fall back to the host .exe).
pub(crate) fn module_handle() -> Option<HMODULE> {
    let saved = MODULE.load(Ordering::Relaxed);
    if saved != 0 {
        return Some(HMODULE(saved as *mut c_void));
    }
    let mut module = HMODULE::default();
    let address = module_handle as *const () as *const u16;
    // SAFETY: `address` points into this DLL's code; UNCHANGED_REFCOUNT means no free is needed.
    unsafe {
        GetModuleHandleExW(
            GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
            PCWSTR(address),
            &mut module,
        )
    }
    .ok()?;
    (!module.is_invalid()).then_some(module)
}

/// RAII counter embedded in every COM object we hand out.
pub(crate) struct ObjectToken;

impl ObjectToken {
    pub(crate) fn new() -> ObjectToken {
        OBJECTS.fetch_add(1, Ordering::SeqCst);
        ObjectToken
    }
}

impl Drop for ObjectToken {
    fn drop(&mut self) {
        OBJECTS.fetch_sub(1, Ordering::SeqCst);
    }
}

pub(crate) fn lock_server(lock: bool) {
    if lock {
        LOCKS.fetch_add(1, Ordering::SeqCst);
    } else {
        // Saturating: an unbalanced unlock must not wrap around.
        let _ = LOCKS.try_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1));
    }
}

/// Run a COM method body, turning a panic into `E_FAIL` (and a log line)
/// instead of unwinding into Explorer.
pub(crate) fn guard<T>(
    what: &str,
    f: impl FnOnce() -> windows_core::Result<T>,
) -> windows_core::Result<T> {
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(result) => result,
        Err(payload) => {
            log::error(&format!("panic in {what}: {}", panic_message(&payload)));
            Err(E_FAIL.into())
        }
    }
}

/// Same as [`guard`] for methods that return a raw `HRESULT`.
pub(crate) fn guard_hr(what: &str, f: impl FnOnce() -> HRESULT) -> HRESULT {
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(hr) => hr,
        Err(payload) => {
            log::error(&format!("panic in {what}: {}", panic_message(&payload)));
            E_FAIL
        }
    }
}

fn panic_message(payload: &Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_owned()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "unknown panic".to_owned()
    }
}

/// # Safety
/// Called by the Windows loader.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn DllMain(
    module: HINSTANCE,
    reason: u32,
    _reserved: *mut c_void,
) -> BOOL {
    if reason == DLL_PROCESS_ATTACH {
        MODULE.store(module.0 as isize, Ordering::Relaxed);
    }
    BOOL::from(true)
}

/// # Safety
/// Called by COM with valid pointers (checked anyway).
#[unsafe(no_mangle)]
pub unsafe extern "system" fn DllGetClassObject(
    rclsid: *const GUID,
    riid: *const GUID,
    ppv: *mut *mut c_void,
) -> HRESULT {
    guard_hr("DllGetClassObject", || {
        if ppv.is_null() || rclsid.is_null() || riid.is_null() {
            return E_POINTER;
        }
        // SAFETY: checked non-null above; COM passes a valid out-pointer.
        unsafe { *ppv = std::ptr::null_mut() };
        // SAFETY: checked non-null above.
        if unsafe { *rclsid } != ROOT_CLSID {
            return CLASS_E_CLASSNOTAVAILABLE;
        }
        let factory: IClassFactory = factory::ClassFactory::new().into();
        // SAFETY: riid/ppv are valid per the checks above.
        unsafe { factory.query(riid, ppv) }
    })
}

#[unsafe(no_mangle)]
pub extern "system" fn DllCanUnloadNow() -> HRESULT {
    if OBJECTS.load(Ordering::SeqCst) == 0 && LOCKS.load(Ordering::SeqCst) == 0 {
        S_OK
    } else {
        S_FALSE
    }
}
