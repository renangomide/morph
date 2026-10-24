# Contributing to Morph

Thanks for helping! Bug reports, fixes and new conversions are all welcome.

## Before you start

- **Bigger changes: open an issue first.** New file types or actions, UI/UX changes, new dependencies or anything that changes the scope in [SPEC.md](SPEC.md) should be discussed in an issue before you write the code. Small fixes can go straight to a pull request.
- [SPEC.md](SPEC.md) is the design. [BUILDING.md](BUILDING.md) explains how to build and test. [CLAUDE.md](CLAUDE.md) is a short map of the code (written for AI coding tools, useful for humans too), including what to touch when adding an action.

## Before you open a pull request

```powershell
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

CI runs the same three. If your change touches the right-click menu, the shell extension or the app window, also try it for real with `.\packaging\dev-register.ps1` and the relevant parts of [docs/manual-tests.md](docs/manual-tests.md).

## Rules that must never be broken

- **Never modify, move or delete input files.** Write to a temp file in the target folder, then rename. Name collisions get ` (1)`, ` (2)`, …
- **The shell extension (`morph-shell`) must never crash or slow down File Explorer:** no `unwrap`/`expect`, `catch_unwind` on every COM entry point, decide the menu from file extensions only, no file I/O except the cached config read.
- **One failed file never stops a batch.** Every error has a short plain-English message plus copyable technical details.
- **All UI text is in English and in one place** (the `strings` modules of `morph-app` and `morph-cli`). The app name lives only in `morph_formats::APP_DISPLAY_NAME` and the MSIX manifest.
- **No network access** besides links the user clicks. No telemetry.
- Use the latest stable version of a crate when adding a dependency, and check that its license is compatible with GPL-3.0.

## Licensing

- Every source file starts with an SPDX header:

  ```rust
  // SPDX-License-Identifier: GPL-3.0-or-later
  ```

- Morph is licensed under GPL-3.0-or-later. By submitting a contribution you agree that it is licensed under the same terms.
- If you add a bundled third-party component, add its license to `THIRD_PARTY_NOTICES.md` (see `scripts/make-notices.ps1`) and `packaging/licenses/`.

## Reporting bugs and security issues

- Bugs and ideas: [GitHub issues](https://github.com/renangomide/morph/issues/new/choose).
- Security vulnerabilities: privately, see [SECURITY.md](SECURITY.md).
