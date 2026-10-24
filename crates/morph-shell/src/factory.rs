// SPDX-License-Identifier: GPL-3.0-or-later
//! Class factory for the root command.

use crate::command::RootCommand;
use crate::{ObjectToken, guard, lock_server};
use std::ffi::c_void;
use windows::Win32::Foundation::{CLASS_E_NOAGGREGATION, E_POINTER};
use windows::Win32::System::Com::{IClassFactory, IClassFactory_Impl};
use windows::Win32::UI::Shell::IExplorerCommand;
use windows_core::{BOOL, GUID, IUnknown, Interface, Ref, implement};

#[implement(IClassFactory)]
pub(crate) struct ClassFactory {
    _token: ObjectToken,
}

impl ClassFactory {
    pub(crate) fn new() -> ClassFactory {
        ClassFactory {
            _token: ObjectToken::new(),
        }
    }
}

impl IClassFactory_Impl for ClassFactory_Impl {
    fn CreateInstance(
        &self,
        outer: Ref<'_, IUnknown>,
        riid: *const GUID,
        ppv: *mut *mut c_void,
    ) -> windows_core::Result<()> {
        guard("CreateInstance", || {
            if ppv.is_null() || riid.is_null() {
                return Err(E_POINTER.into());
            }
            // SAFETY: checked non-null.
            unsafe { *ppv = std::ptr::null_mut() };
            if outer.is_some() {
                return Err(CLASS_E_NOAGGREGATION.into());
            }
            let command: IExplorerCommand = RootCommand::new().into();
            // SAFETY: riid/ppv checked non-null above.
            unsafe { command.query(riid, ppv) }.ok()
        })
    }

    fn LockServer(&self, lock: BOOL) -> windows_core::Result<()> {
        lock_server(lock.as_bool());
        Ok(())
    }
}
