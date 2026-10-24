// SPDX-License-Identifier: GPL-3.0-or-later
//! Compiles the Slint UI with the Fluent style (it follows the system light/dark theme), and embeds
//! the app icon + version info as Win32 resources of Morph.exe (rc.exe from the Windows SDK).

use std::path::Path;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = slint_build::CompilerConfiguration::new().with_style("fluent".into());
    slint_build::compile_with_config("ui/main.slint", config)?;
    embed_resources()?;
    Ok(())
}

/// Writes `morph.rc` into OUT_DIR (absolute icon path, version from Cargo) and links it into the exe.
fn embed_resources() -> Result<(), Box<dyn std::error::Error>> {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let icon = manifest_dir
        .join("../../packaging/Assets/morph.ico")
        .canonicalize()?;
    println!("cargo:rerun-if-changed={}", icon.display());

    let version = env!("CARGO_PKG_VERSION");
    let mut parts = version.split('.').map(|p| p.parse::<u16>().unwrap_or(0));
    let mut next = || parts.next().unwrap_or(0);
    let (major, minor, patch) = (next(), next(), next());
    let name = morph_formats::APP_DISPLAY_NAME;

    // canonicalize() yields a \\?\ verbatim path; rc.exe wants a plain one with escaped backslashes.
    let icon = icon.display().to_string();
    let icon = icon
        .strip_prefix(r"\\?\")
        .unwrap_or(&icon)
        .replace('\\', "\\\\");
    let rc = format!(
        r#"#pragma code_page(65001)
1 ICON "{icon}"

1 VERSIONINFO
FILEVERSION {major},{minor},{patch},0
PRODUCTVERSION {major},{minor},{patch},0
FILEOS 0x40004
FILETYPE 0x1
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "040904B0"
    BEGIN
      VALUE "ProductName", "{name}"
      VALUE "FileDescription", "{name}"
      VALUE "InternalName", "{name}.exe"
      VALUE "OriginalFilename", "{name}.exe"
      VALUE "FileVersion", "{version}"
      VALUE "ProductVersion", "{version}"
      VALUE "LegalCopyright", "GPL-3.0-or-later"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x409, 1200
  END
END
"#
    );
    let out = Path::new(&std::env::var("OUT_DIR")?).join("morph.rc");
    std::fs::write(&out, rc)?;
    embed_resource::compile(&out, embed_resource::NONE).manifest_required()?;
    Ok(())
}
