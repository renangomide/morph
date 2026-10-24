# Building Morph from source

## Prerequisites

Windows 11, x64. Development must happen on Windows (not WSL): it needs MSVC, the Windows SDK and File Explorer.

- [Rust](https://rustup.rs/) (MSVC toolchain, stable).
- Visual Studio 2022 Build Tools with **Desktop development with C++** and the **Windows 11 SDK** (`makeappx.exe` and `signtool.exe` come from the SDK).
- [NASM](https://www.nasm.us/) on `PATH` (assembly in the AVIF and JPEG encoders).
- [vcpkg](https://github.com/microsoft/vcpkg) with `VCPKG_ROOT` set (for the HEIC/AVIF decoders, linked statically).
- Developer Mode on, to register a development package: Settings ▸ System ▸ For developers ▸ Developer Mode.
- Optional: LibreOffice or Microsoft Office, for the Office conversions and their tests.

If a native build fails for no clear reason, try a checkout path without spaces (e.g. `C:\dev\morph`).

## Build and test

From the repository root, in PowerShell:

```powershell
# once per machine (~20 min): libheif + aom, static
vcpkg install --triplet x64-windows-static --x-install-root="$env:VCPKG_ROOT\installed"

# once per checkout: pinned PDFium + FFmpeg binaries into vendor\ (~70 MB, SHA-256 checked)
.\scripts\fetch-deps.ps1

cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check

# build, stage layout\, register the development package and restart File Explorer
.\packaging\dev-register.ps1
.\packaging\dev-register.ps1 -Unregister   # remove it again
```

The C runtime is linked statically (`.cargo/config.toml`: `+crt-static`), so vcpkg libraries must use the `x64-windows-static` triplet. If you set `RUSTFLAGS` yourself, it replaces the config's flags: include `-C target-feature=+crt-static`.

Tests that need LibreOffice print `SKIPPED: …` and pass when it is not installed. Tests that need Microsoft Office, an open File Explorer or a release build are `#[ignore]`d: `cargo test -p morph-core -- --ignored`.

FFmpeg comes from the [morph-ffmpeg](https://github.com/renangomide/morph-ffmpeg) release pinned in `scripts/fetch-deps.ps1`. Set `MORPH_FFMPEG_DIR=<folder with ffmpeg.exe and ffprobe.exe>` to use another copy.

## Trying converters without the GUI

```powershell
.\target\debug\morph-cli.exe actions        # action IDs and their default options
.\target\debug\morph-cli.exe convert photo.jpg --action image.convert --format webp
```

## Building an MSIX

```powershell
.\packaging\dev-cert.ps1     # once: self-signed "CN=OpenMorph" cert -> packaging\cert\morph-dev.pfx/.cer
.\packaging\make-msix.ps1    # release build -> target\Morph-<version>-x64.msix, signed with that cert
```

`make-msix.ps1 -PfxPath <file> -PfxPassword <pwd>` signs with another certificate; its subject must be `CN=OpenMorph`. It stages the package in `target\release-layout\`, never in `layout\` (the folder `dev-register.ps1` registers), so building a release does not disturb an installed development package. The version comes from `Cargo.toml` (`[workspace.package]`).

## Building the release zip

```powershell
.\packaging\make-release.ps1
```

It builds the MSIX like `make-msix.ps1`, then `target\release-zip\Morph-<version>-x64.zip` (the msix, `Morph.cer`, `Install.cmd`, `Uninstall.cmd`, `README.txt` from `packaging\release\`), checks the signer against the manifest publisher and dry-runs `Install.cmd /check`.

Pushing a `v*` tag runs `.github/workflows/release.yml`, which does the same on GitHub Actions and creates the GitHub Release.

## Other docs

- [SPEC.md](SPEC.md): the full design.
- [docs/manual-tests.md](docs/manual-tests.md): manual test checklist (right-click menu, background jobs, install/uninstall).
- [CONTRIBUTING.md](CONTRIBUTING.md): how to contribute.
