// SPDX-License-Identifier: GPL-3.0-or-later
//! Where Morph keeps its data. The shell DLL and the app must agree, so this lives here.
//!
//! The shared folder is the package's `LocalState`:
//! `%LOCALAPPDATA%\Packages\<PackageFamilyName>\LocalState`
//! (the same folder as `ApplicationData::Current().LocalFolder()`).
//!
//! The family name is **computed** from the package identity constants instead of
//! asked from the current process: on Windows 11 the context-menu DLL may be
//! loaded straight into explorer.exe, which has no package identity.
//!
//! - [`shell_data_dir`]: always the package folder (used by the DLL).
//! - [`app_data_dir`]: the package folder when the app runs packaged; when it runs
//!   unpackaged (`cargo run` during development) `%LOCALAPPDATA%\Morph`.

use std::path::{Path, PathBuf};
use windows::Win32::Foundation::ERROR_SUCCESS;
use windows::Win32::Storage::Packaging::Appx::{
    GetCurrentPackageFamilyName, PACKAGE_ID, PackageFamilyNameFromId,
};
use windows::core::PWSTR;

/// Buffer size for package family names (PACKAGE_FAMILY_NAME_MAX_LENGTH is 65 + 13 + 1).
const PFN_BUF: usize = 128;

fn utf16_until(buf: &[u16], len_with_nul: u32) -> Option<String> {
    let chars = (len_with_nul as usize).saturating_sub(1).min(buf.len());
    String::from_utf16(&buf[..chars]).ok()
}

/// Package family name of the current process, or `None` when it has no identity.
pub fn current_package_family_name() -> Option<String> {
    let mut buf = [0u16; PFN_BUF];
    let mut len = buf.len() as u32;
    // SAFETY: `len` holds the buffer capacity in UTF-16 units; the API writes at most that many.
    let rc = unsafe { GetCurrentPackageFamilyName(&mut len, Some(PWSTR(buf.as_mut_ptr()))) };
    if rc != ERROR_SUCCESS {
        return None; // APPMODEL_ERROR_NO_PACKAGE when running unpackaged
    }
    utf16_until(&buf, len)
}

/// Package family name of the Morph package, computed from
/// [`crate::PACKAGE_NAME`] and [`crate::PACKAGE_PUBLISHER`] (pure computation, no I/O).
pub fn morph_package_family_name() -> Option<String> {
    let mut name: Vec<u16> = crate::PACKAGE_NAME.encode_utf16().chain([0]).collect();
    let mut publisher: Vec<u16> = crate::PACKAGE_PUBLISHER.encode_utf16().chain([0]).collect();
    let id = PACKAGE_ID {
        name: PWSTR(name.as_mut_ptr()),
        publisher: PWSTR(publisher.as_mut_ptr()),
        ..Default::default()
    };
    let mut buf = [0u16; PFN_BUF];
    let mut len = buf.len() as u32;
    // SAFETY: `id` points to NUL-terminated strings that outlive the call; `len` is the capacity.
    let rc = unsafe { PackageFamilyNameFromId(&id, &mut len, Some(PWSTR(buf.as_mut_ptr()))) };
    if rc != ERROR_SUCCESS {
        return None;
    }
    utf16_until(&buf, len)
}

/// AppUserModelID used to activate `Morph.exe`: `<PackageFamilyName>!Morph`.
pub fn app_user_model_id() -> Option<String> {
    morph_package_family_name().map(|pfn| format!("{pfn}!{}", crate::APP_ID))
}

fn local_app_data() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
}

fn local_state_for(pfn: &str) -> Option<PathBuf> {
    Some(
        local_app_data()?
            .join("Packages")
            .join(pfn)
            .join("LocalState"),
    )
}

/// Data folder used by the shell DLL: always the package's `LocalState`.
pub fn shell_data_dir() -> Option<PathBuf> {
    local_state_for(&morph_package_family_name()?)
}

/// Data folder used by the app (see module docs).
pub fn app_data_dir() -> Option<PathBuf> {
    match current_package_family_name() {
        Some(pfn) => local_state_for(&pfn),
        None => Some(local_app_data()?.join(crate::APP_DATA_FOLDER)),
    }
}

pub fn requests_dir(root: &Path) -> PathBuf {
    root.join("requests")
}

pub fn config_file(root: &Path) -> PathBuf {
    root.join("config.toml")
}

pub fn state_file(root: &Path) -> PathBuf {
    root.join("state.toml")
}

pub fn logs_dir(root: &Path) -> PathBuf {
    root.join("logs")
}

pub fn tmp_dir(root: &Path) -> PathBuf {
    root.join("tmp")
}

/// Saved results of finished jobs (`<id>.json`), for "click the toast to see results".
pub fn results_dir(root: &Path) -> PathBuf {
    root.join("results")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn computed_family_name_has_publisher_hash() {
        let pfn = morph_package_family_name().unwrap();
        let (name, hash) = pfn.split_once('_').unwrap();
        assert_eq!(name, crate::PACKAGE_NAME);
        assert_eq!(hash.len(), 13, "publisher id is 13 base32 chars: {pfn}");
        assert_eq!(app_user_model_id().unwrap(), format!("{pfn}!Morph"));
    }

    /// Pinned so docs (manual tests, uninstall checks) stay right when the identity changes.
    #[test]
    fn family_name_is_pinned() {
        assert_eq!(
            morph_package_family_name().as_deref(),
            Some("OpenMorph.Morph_kxvcpsh9twvzt")
        );
    }

    #[test]
    fn shell_uses_package_folder_and_unpackaged_app_uses_morph_folder() {
        // `cargo test` runs without package identity.
        assert_eq!(current_package_family_name(), None);
        assert!(app_data_dir().unwrap().ends_with(crate::APP_DATA_FOLDER));
        let shell = shell_data_dir().unwrap();
        assert!(shell.ends_with("LocalState"));
        assert!(requests_dir(&shell).ends_with("requests"));
    }
}
