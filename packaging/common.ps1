# SPDX-License-Identifier: GPL-3.0-or-later
# Shared helpers, dot-sourced by dev-register.ps1, dev-cert.ps1 and make-msix.ps1:
#   . (Join-Path $PSScriptRoot 'common.ps1')
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

# Package identity. Must match morph_formats::{PACKAGE_PUBLISHER, PACKAGE_NAME} (the shell DLL computes the
# package family name and AUMID from those; a morph-formats test checks this file).
$script:MorphPublisher = 'CN=OpenMorph'
$script:MorphPackageName = 'OpenMorph.Morph'
# Identities used by earlier builds; dev-register.ps1 removes these packages if they are still installed.
$script:MorphLegacyPackageNames = @('Morph.Dev')

function Get-RepoRoot {
    $root = Split-Path -Parent $PSScriptRoot
    if (-not (Test-Path -LiteralPath (Join-Path $root 'Cargo.toml'))) {
        throw "Cargo.toml not found in '$root'. Run the scripts from the Morph repository."
    }
    return $root
}

# Returns the workspace version (e.g. "0.1.0") from [workspace.package] in the root Cargo.toml.
function Get-MorphVersion {
    $toml = Join-Path (Get-RepoRoot) 'Cargo.toml'
    $inSection = $false
    foreach ($line in Get-Content -LiteralPath $toml) {
        if ($line -match '^\s*\[(.+?)\]\s*$') { $inSection = ($Matches[1] -eq 'workspace.package'); continue }
        if ($inSection -and $line -match '^\s*version\s*=\s*"(\d+\.\d+\.\d+)"') { return $Matches[1] }
    }
    throw "Could not find [workspace.package] version in '$toml'."
}

# Manifest versions have four parts: 0.1.0 -> 0.1.0.0
function Get-MorphManifestVersion {
    return ((Get-MorphVersion) + '.0')
}

# Finds a tool (makeappx.exe, signtool.exe) in the newest Windows Kits\10\bin\<ver>\x64.
function Find-SdkTool([string]$Name) {
    $bases = @()
    foreach ($pf in @(${env:ProgramFiles(x86)}, $env:ProgramFiles)) {
        if ($pf) { $bases += (Join-Path $pf 'Windows Kits\10\bin') }
    }
    $candidates = @()
    foreach ($b in $bases) {
        if (-not (Test-Path -LiteralPath $b)) { continue }
        foreach ($d in Get-ChildItem -LiteralPath $b -Directory | Where-Object { $_.Name -match '^\d+\.\d+\.\d+\.\d+$' }) {
            $exe = Join-Path $d.FullName "x64\$Name"
            if (Test-Path -LiteralPath $exe) { $candidates += [pscustomobject]@{ Version = [version]$d.Name; Path = $exe } }
        }
    }
    if ($candidates.Count -eq 0) {
        throw "$Name not found. Install the Windows 11 SDK (Windows Kits\10\bin\<version>\x64)."
    }
    return ($candidates | Sort-Object Version -Descending | Select-Object -First 1).Path
}

# Builds a package folder from target\<Profile>: binaries, icon, Assets and the rendered manifest.
# Default <repo>\layout is the folder dev-register.ps1 registers; release builds pass their own -Path
# (make-msix.ps1: target\release-layout) so they never rewrite or lock the registered dev package.
function New-Layout([ValidateSet('debug', 'release')][string]$Profile = 'debug', [string]$Path) {
    $root = Get-RepoRoot
    $target = Join-Path $root "target\$Profile"
    $pkg = Join-Path $root 'packaging'
    $layout = if ($Path) { $Path } else { Join-Path $root 'layout' }

    # Third-party binaries (pdfium.dll, ffmpeg.exe, ffprobe.exe). Idempotent: skips what is already in vendor\.
    & (Join-Path $root 'scripts\fetch-deps.ps1')

    $needed = @(
        @{ Src = (Join-Path $target 'Morph.exe'); Dst = 'Morph.exe' },
        @{ Src = (Join-Path $target 'morph-cli.exe'); Dst = 'morph-cli.exe' },
        @{ Src = (Join-Path $target 'morph_shell.dll'); Dst = 'morph_shell.dll' },
        @{ Src = (Join-Path $pkg 'Assets\morph.ico'); Dst = 'morph.ico' },
        # PDF engine: loaded from the exe's own folder (morph_core::pdf::pdfium).
        @{ Src = (Join-Path $root 'vendor\pdfium\pdfium.dll'); Dst = 'pdfium.dll' },
        # Video/audio engine: found next to the exe (morph_core::media::ffmpeg).
        @{ Src = (Join-Path $root 'vendor\ffmpeg\ffmpeg.exe'); Dst = 'ffmpeg.exe' },
        @{ Src = (Join-Path $root 'vendor\ffmpeg\ffprobe.exe'); Dst = 'ffprobe.exe' }
    )
    foreach ($n in $needed) {
        if (-not (Test-Path -LiteralPath $n.Src)) {
            throw "Missing '$($n.Src)'. Build first: cargo build -p morph-app -p morph-cli -p morph-shell$(if ($Profile -eq 'release') { ' --release' })"
        }
    }

    if (Test-Path -LiteralPath $layout) { Remove-Item -LiteralPath $layout -Recurse -Force }
    New-Item -ItemType Directory -Force -Path $layout | Out-Null
    foreach ($n in $needed) { Copy-Item -LiteralPath $n.Src -Destination (Join-Path $layout $n.Dst) -Force }
    # FFmpeg is the shared build: its DLLs (avcodec-*.dll, ...) must sit next to ffmpeg.exe/ffprobe.exe.
    $ffDlls = @(Get-ChildItem -Path (Join-Path $root 'vendor\ffmpeg\*.dll') -File)
    if ($ffDlls.Count -eq 0) { throw "No FFmpeg DLLs in vendor\ffmpeg. Run scripts\fetch-deps.ps1 -Force." }
    $ffDlls | Copy-Item -Destination $layout -Force
    Copy-Item -LiteralPath (Join-Path $pkg 'Assets') -Destination (Join-Path $layout 'Assets') -Recurse -Force
    New-Item -ItemType Directory -Force -Path (Join-Path $layout 'licenses') | Out-Null
    Copy-Item -LiteralPath (Join-Path $root 'vendor\pdfium\licenses') -Destination (Join-Path $layout 'licenses\pdfium') -Recurse -Force
    Copy-Item -LiteralPath (Join-Path $root 'vendor\ffmpeg\licenses') -Destination (Join-Path $layout 'licenses\ffmpeg') -Recurse -Force
    # Committed texts for the other native components (scripts\make-notices.ps1), the notices and Morph's own license.
    Copy-Item -Path (Join-Path $pkg 'licenses\*') -Destination (Join-Path $layout 'licenses') -Recurse -Force
    Copy-Item -LiteralPath (Join-Path $root 'THIRD_PARTY_NOTICES.md') -Destination $layout -Force
    Copy-Item -LiteralPath (Join-Path $root 'LICENSE') -Destination (Join-Path $layout 'LICENSE.txt') -Force

    $text = [System.IO.File]::ReadAllText((Join-Path $pkg 'AppxManifest.xml.in'))
    $text = $text.Replace('{{VERSION}}', (Get-MorphManifestVersion)).Replace('{{PUBLISHER}}', $script:MorphPublisher).Replace('{{NAME}}', $script:MorphPackageName)
    # UTF-8 without BOM, identical on PS 5.1 and 7.
    [System.IO.File]::WriteAllText((Join-Path $layout 'AppxManifest.xml'), $text, (New-Object System.Text.UTF8Encoding($false)))
    New-ResourcesPri -Layout $layout
    return $layout
}

# Writes <layout>\resources.pri so Windows resolves the qualified logos (.scale-*, .targetsize-*,
# _altform-unplated) behind the plain names in the manifest. Only Assets\ is indexed: makepri runs on
# a temp copy of it, so the binaries in layout\ stay out of the index.
function New-ResourcesPri([string]$Layout) {
    $makepri = Find-SdkTool 'makepri.exe'
    $work = Join-Path ([System.IO.Path]::GetTempPath()) ("morph-pri-" + [guid]::NewGuid().ToString('N'))
    $project = Join-Path $work 'project'
    New-Item -ItemType Directory -Force -Path $project | Out-Null
    try {
        Copy-Item -LiteralPath (Join-Path $Layout 'Assets') -Destination (Join-Path $project 'Assets') -Recurse -Force
        $config = Join-Path $work 'priconfig.xml'
        & $makepri createconfig /cf $config /dq en-US /pv 10.0.0 /o | Out-Null
        if ($LASTEXITCODE -ne 0) { throw "makepri createconfig failed (exit code $LASTEXITCODE)." }
        # Drop <packaging> (auto resource packs): one single-package MSIX keeps every scale in resources.pri.
        $xml = New-Object System.Xml.XmlDocument
        $xml.Load($config)
        $packaging = $xml.SelectSingleNode('/resources/packaging')
        if ($packaging) { [void]$xml.DocumentElement.RemoveChild($packaging) }
        $xml.Save($config)
        & $makepri new /pr $project /cf $config /mn (Join-Path $Layout 'AppxManifest.xml') /of (Join-Path $Layout 'resources.pri') /o | Out-Null
        if ($LASTEXITCODE -ne 0) { throw "makepri new failed (exit code $LASTEXITCODE)." }
    }
    finally { Remove-Item -LiteralPath $work -Recurse -Force -ErrorAction SilentlyContinue }
}

# Reads an .msix: identity from AppxManifest.xml, signing certificate from AppxSignature.p7x.
# (packaging\release\Install.cmd has its own copy: it runs on PCs without this repo.)
function Get-MsixInfo([string]$Path) {
    Add-Type -AssemblyName System.IO.Compression, System.IO.Compression.FileSystem, System.Security
    $zip = [IO.Compression.ZipFile]::OpenRead($Path)
    try {
        $read = {
            param($name)
            $e = $zip.GetEntry($name)
            if (-not $e) { throw "$name is missing from '$Path'." }
            $s = $e.Open(); $ms = New-Object IO.MemoryStream
            try { $s.CopyTo($ms) } finally { $s.Dispose() }
            , $ms.ToArray()
        }
        [xml]$manifest = [Text.Encoding]::UTF8.GetString((& $read 'AppxManifest.xml'))
        $p7x = & $read 'AppxSignature.p7x'
    }
    finally { $zip.Dispose() }
    # AppxSignature.p7x = "PKCX" + a PKCS #7 signature.
    $cms = New-Object Security.Cryptography.Pkcs.SignedCms
    $cms.Decode([byte[]]$p7x[4..($p7x.Length - 1)])
    $id = $manifest.Package.Identity
    [pscustomobject]@{
        Name = $id.Name; Publisher = $id.Publisher; Version = $id.Version
        Signer = $cms.SignerInfos[0].Certificate
    }
}

# Runs cargo build for the packaged crates from the repo root.
function Invoke-MorphBuild([ValidateSet('debug', 'release')][string]$Profile = 'debug') {
    $cargoArgs = @('build', '-p', 'morph-app', '-p', 'morph-cli', '-p', 'morph-shell')
    if ($Profile -eq 'release') { $cargoArgs += '--release' }
    Push-Location (Get-RepoRoot)
    try {
        & cargo @cargoArgs
        if ($LASTEXITCODE -ne 0) { throw "cargo build failed (exit code $LASTEXITCODE)." }
    }
    finally { Pop-Location }
}
