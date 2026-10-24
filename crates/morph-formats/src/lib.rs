// SPDX-License-Identifier: GPL-3.0-or-later
//! Tiny shared tables for Morph.
//!
//! This crate is linked into the Explorer shell DLL, so it must stay small:
//! no heavy dependencies, no file I/O except the cached config read.

pub mod actions;
pub mod category;
pub mod config;
pub mod menu;
pub mod paths;
pub mod request;

pub use category::{Category, DocKind, MenuGroup, SelectionSummary};
pub use config::EnabledCategories;
pub use menu::{MenuEntry, MenuItem, build_menu};
pub use request::Request;

/// Display name shown in the menu and window titles. Rename the app here
/// (plus `packaging/AppxManifest.xml.in`).
pub const APP_DISPLAY_NAME: &str = "Morph";

/// Folder name used for app data when running unpackaged (development).
pub const APP_DATA_FOLDER: &str = "Morph";

/// Package identity `Name` (manifest `<Identity Name>`; also `packaging/common.ps1`).
pub const PACKAGE_NAME: &str = "OpenMorph.Morph";

/// Package identity `Publisher` (manifest `<Identity Publisher>`; also `packaging/common.ps1`).
/// Must match exactly, or the DLL computes the wrong data folder and AUMID.
pub const PACKAGE_PUBLISHER: &str = "CN=OpenMorph";

/// `Id` of the `<Application>` element in the MSIX manifest.
/// The AUMID is `<PackageFamilyName>!<APP_ID>`.
pub const APP_ID: &str = "Morph";

/// CLSID of the root "Morph" Explorer command. Fixed forever; also in the manifest.
pub const ROOT_CLSID_STR: &str = "BCC8EAF0-12A9-452A-87A5-B52F2DCC77F5";

/// Same CLSID as a `u128`, for `windows::core::GUID::from_u128`.
pub const ROOT_CLSID_U128: u128 = 0xBCC8EAF0_12A9_452A_87A5_B52F2DCC77F5;

/// Command-line flag that passes a request file to `Morph.exe`.
pub const ARG_REQUEST: &str = "--request";
/// `Morph.exe --open-folder <path>`: open a folder in Explorer and exit (toast button).
pub const ARG_OPEN_FOLDER: &str = "--open-folder";
/// `Morph.exe --show-results <id>`: show a finished job's saved results (toast click).
pub const ARG_SHOW_RESULTS: &str = "--show-results";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clsid_constants_agree() {
        let hex = ROOT_CLSID_STR.replace('-', "");
        assert_eq!(u128::from_str_radix(&hex, 16).ok(), Some(ROOT_CLSID_U128));
    }

    /// The packaging scripts write the manifest identity; the DLL computes the family name
    /// from these constants. They must agree or the menu activates a package that does not exist.
    #[test]
    fn package_identity_matches_packaging_scripts() {
        let ps1 = include_str!("../../../packaging/common.ps1");
        assert!(
            ps1.contains(&format!("$script:MorphPublisher = '{PACKAGE_PUBLISHER}'")),
            "packaging/common.ps1 publisher differs from PACKAGE_PUBLISHER"
        );
        assert!(
            ps1.contains(&format!("$script:MorphPackageName = '{PACKAGE_NAME}'")),
            "packaging/common.ps1 package name differs from PACKAGE_NAME"
        );
        let manifest = include_str!("../../../packaging/AppxManifest.xml.in");
        assert!(manifest.contains(r#"<Identity Name="{{NAME}}" Publisher="{{PUBLISHER}}""#));
        assert!(manifest.contains(&format!(r#"<Application Id="{APP_ID}""#)));
        assert!(manifest.contains(&format!(r#"Clsid="{ROOT_CLSID_STR}""#)));
        let uninstall = include_str!("../../../packaging/release/Uninstall.cmd");
        assert!(uninstall.contains(&format!("$packageName = '{PACKAGE_NAME}'")));
        assert!(uninstall.contains(&format!("$publisher = '{PACKAGE_PUBLISHER}'")));
    }
}
