# SPDX-License-Identifier: GPL-3.0-or-later
<#
.SYNOPSIS
  Regenerates THIRD_PARTY_NOTICES.md and packaging\licenses\ (both committed).

.DESCRIPTION
  Run after changing dependencies (Cargo.lock, vcpkg.json, fetch-deps.ps1 pins).
  - Rust crates: cargo-about (dev tool, not a runtime dependency; install once with
    `cargo install cargo-about --locked`) with scripts\notices\about.toml + about.hbs.
    The native-component section lives in about.hbs: update its versions and links by hand.
  - Native license texts copied into packaging\licenses\<component>\: libheif, libde265,
    aom (vcpkg, from $env:VCPKG_ROOT), libwebp, mozjpeg, libdeflate, Slint (cargo registry).
    FFmpeg and PDFium texts come from vendor\ at layout time (packaging\common.ps1).
#>
[CmdletBinding()]
param()

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$root = Split-Path -Parent $PSScriptRoot
$notices = Join-Path $PSScriptRoot 'notices'
$licenses = Join-Path $root 'packaging\licenses'

& cargo about --version *> $null
if ($LASTEXITCODE -ne 0) { throw 'cargo-about not found. Install it with: cargo install cargo-about --locked' }

Push-Location $root
try {
    & cargo about generate --workspace --locked -c (Join-Path $notices 'about.toml') `
        (Join-Path $notices 'about.hbs') -o (Join-Path $root 'THIRD_PARTY_NOTICES.md')
    if ($LASTEXITCODE -ne 0) { throw "cargo about failed (exit code $LASTEXITCODE)." }

    $meta = & cargo metadata --format-version 1 --locked | ConvertFrom-Json
    if ($LASTEXITCODE -ne 0) { throw "cargo metadata failed (exit code $LASTEXITCODE)." }
}
finally { Pop-Location }

# Folder of a crate in the cargo registry (the version Cargo.lock picked).
function Get-CrateDir([string]$Name) {
    $pkg = @($meta.packages | Where-Object { $_.name -eq $Name })
    if ($pkg.Count -ne 1) { throw "Expected exactly one '$Name' package in Cargo.lock, found $($pkg.Count)." }
    return Split-Path -Parent $pkg[0].manifest_path
}

if (-not $env:VCPKG_ROOT) { throw 'VCPKG_ROOT is not set (needed for the libheif / libde265 / aom license texts).' }
$vcpkgShare = Join-Path $env:VCPKG_ROOT 'installed\x64-windows-static\share'

# component -> list of @(source file, file name in packaging\licenses\<component>\)
$copies = [ordered]@{
    'libheif'    = @(, @((Join-Path $vcpkgShare 'libheif\copyright'), 'COPYING.txt'))
    'libde265'   = @(, @((Join-Path $vcpkgShare 'libde265\copyright'), 'COPYING.txt'))
    'aom'        = @(, @((Join-Path $vcpkgShare 'aom\copyright'), 'LICENSE-and-PATENTS.txt'))
    'libwebp'    = @(
        @((Join-Path (Get-CrateDir 'libwebp-sys') 'vendor\COPYING'), 'COPYING.txt'),
        @((Join-Path (Get-CrateDir 'libwebp-sys') 'vendor\PATENTS'), 'PATENTS.txt'))
    'mozjpeg'    = @(, @((Join-Path (Get-CrateDir 'mozjpeg-sys') 'LICENSE'), 'LICENSE.txt'))
    'libdeflate' = @(, @((Join-Path (Get-CrateDir 'libdeflate-sys') 'libdeflate\COPYING'), 'COPYING.txt'))
    'slint'      = @(, @((Join-Path (Get-CrateDir 'slint') 'LICENSES\GPL-3.0-only.txt'), 'GPL-3.0-only.txt'))
}

if (Test-Path -LiteralPath $licenses) { Remove-Item -LiteralPath $licenses -Recurse -Force }
foreach ($component in $copies.Keys) {
    $dir = Join-Path $licenses $component
    New-Item -ItemType Directory -Force -Path $dir | Out-Null
    foreach ($c in $copies[$component]) {
        if (-not (Test-Path -LiteralPath $c[0])) { throw "License file not found: $($c[0])" }
        Copy-Item -LiteralPath $c[0] -Destination (Join-Path $dir $c[1]) -Force
    }
}
Write-Host "Wrote THIRD_PARTY_NOTICES.md and packaging\licenses\ ($($copies.Count) components)."
