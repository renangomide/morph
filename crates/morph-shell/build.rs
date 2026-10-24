// SPDX-License-Identifier: GPL-3.0-or-later
//! Export the COM entry points as PRIVATE (not in the import library), as COM DLLs should.

fn main() {
    let def = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("exports.def");
    println!("cargo:rerun-if-changed={}", def.display());
    println!("cargo:rustc-cdylib-link-arg=/DEF:{}", def.display());
}
