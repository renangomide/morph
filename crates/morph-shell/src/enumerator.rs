// SPDX-License-Identifier: GPL-3.0-or-later
//! `IEnumExplorerCommand` over a fixed list of subcommands.

use crate::{ObjectToken, guard, guard_hr};
use std::sync::atomic::{AtomicUsize, Ordering};
use windows::Win32::Foundation::{E_INVALIDARG, E_POINTER, S_FALSE, S_OK};
use windows::Win32::UI::Shell::{
    IEnumExplorerCommand, IEnumExplorerCommand_Impl, IExplorerCommand,
};
use windows_core::{HRESULT, implement};

#[implement(IEnumExplorerCommand)]
pub(crate) struct CommandEnum {
    commands: Vec<IExplorerCommand>,
    next: AtomicUsize,
    _token: ObjectToken,
}

impl CommandEnum {
    pub(crate) fn new(commands: Vec<IExplorerCommand>) -> CommandEnum {
        CommandEnum::starting_at(commands, 0)
    }

    fn starting_at(commands: Vec<IExplorerCommand>, next: usize) -> CommandEnum {
        CommandEnum {
            commands,
            next: AtomicUsize::new(next),
            _token: ObjectToken::new(),
        }
    }
}

impl IEnumExplorerCommand_Impl for CommandEnum_Impl {
    fn Next(&self, celt: u32, out: *mut Option<IExplorerCommand>, fetched: *mut u32) -> HRESULT {
        guard_hr("Enum::Next", || {
            if out.is_null() {
                return E_POINTER;
            }
            if celt > 1 && fetched.is_null() {
                return E_INVALIDARG; // COM rule for IEnum*::Next
            }
            let mut count = 0u32;
            while count < celt {
                let index = self.next.load(Ordering::SeqCst);
                let Some(command) = self.commands.get(index) else {
                    break;
                };
                // SAFETY: the caller provides room for `celt` entries.
                unsafe { out.add(count as usize).write(Some(command.clone())) };
                self.next.store(index + 1, Ordering::SeqCst);
                count += 1;
            }
            if !fetched.is_null() {
                // SAFETY: checked non-null; caller-provided out-parameter.
                unsafe { *fetched = count };
            }
            if count == celt { S_OK } else { S_FALSE }
        })
    }

    fn Skip(&self, celt: u32) -> windows_core::Result<()> {
        let index = self.next.load(Ordering::SeqCst);
        let target = index.saturating_add(celt as usize).min(self.commands.len());
        self.next.store(target, Ordering::SeqCst);
        if target - index == celt as usize {
            Ok(())
        } else {
            Err(S_FALSE.into())
        }
    }

    fn Reset(&self) -> windows_core::Result<()> {
        self.next.store(0, Ordering::SeqCst);
        Ok(())
    }

    fn Clone(&self) -> windows_core::Result<IEnumExplorerCommand> {
        guard("Enum::Clone", || {
            let copy =
                CommandEnum::starting_at(self.commands.clone(), self.next.load(Ordering::SeqCst));
            Ok(copy.into())
        })
    }
}
