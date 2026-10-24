# SPDX-License-Identifier: GPL-3.0-or-later
<#
.SYNOPSIS
  Downloads the pinned third-party binaries into vendor\ (git-ignored), checking SHA-256.

.DESCRIPTION
  Idempotent: a dependency whose vendor folder already has the matching stamp file is skipped.
  Use -Force to download again. Works in Windows PowerShell 5.1 (uses the built-in tar.exe).

  Pinned now:
    PDFium chromium/7881 (bblanchon/pdfium-binaries, win-x64, no V8/XFA)
      -> vendor\pdfium\pdfium.dll + licenses\  (BSD-3-Clause / Apache-2.0 + bundled third-party licenses)
      Must match the pdfium-render feature `pdfium_7881` in crates\morph-core\Cargo.toml.
    FFmpeg 9.0.2, morph-ffmpeg build 9.0.2-1 (https://github.com/renangomide/morph-ffmpeg, release v9.0.2-1, win64)
      -> vendor\ffmpeg\ffmpeg.exe + ffprobe.exe + 7 shared DLLs + licenses\  (GPL-3.0; FFmpeg tag n9.0.2, commit 946fcce07b)
      Trimmed build (~62 MB): only what Morph uses (libx264, libx265, libsvtav1, libvpx, dav1d, libopus, libmp3lame,
      libvorbis, aac, libvmaf with built-in models, palettegen/paletteuse, h264/hevc/av1 _nvenc/_qsv/_amf, lavfi).
      Exact libraries, revisions and configure flags: licenses\BUILDINFO.txt. Complete source:
      morph-ffmpeg-9.0.2-1-source.tar.xz on the same release. Plain download, no auth needed; if it
      fails and the GitHub CLI is installed, 'gh release download' is tried as a fallback.
#>
[CmdletBinding()]
param([switch]$Force)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'   # Invoke-WebRequest is very slow with the progress bar on PS 5.1
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12

$root = Split-Path -Parent $PSScriptRoot
$vendor = Join-Path $root 'vendor'

$deps = @(
    @{
        Name    = 'pdfium'
        Version = 'chromium/7881'
        Url     = 'https://github.com/bblanchon/pdfium-binaries/releases/download/chromium/7881/pdfium-win-x64.tgz'
        Sha256  = '73cc0de638ac2095e7445bf56a38200a5b7c7ca0e9f4ba144598f2457377ac08'
        # Archive path -> vendor\<Name>\ path. Folders are copied recursively.
        Files   = @(
            @{ From = 'bin\pdfium.dll'; To = 'pdfium.dll' },
            @{ From = 'LICENSE'; To = 'licenses\LICENSE' },
            @{ From = 'licenses'; To = 'licenses' },
            @{ From = 'VERSION'; To = 'VERSION' }
        )
    },
    @{
        Name    = 'ffmpeg'
        Version = 'morph-ffmpeg 9.0.2-1'
        Url     = 'https://github.com/renangomide/morph-ffmpeg/releases/download/v9.0.2-1/morph-ffmpeg-9.0.2-1-win64.zip'
        Sha256  = '7716e9d8553b67d0a5ddd72475189905887aef467bbfeaa41b460342b5158acc'   # = GitHub asset digest
        # Fallback when the plain URL fails: gh release download (logged in / GH_TOKEN).
        GhRepo  = 'renangomide/morph-ffmpeg'
        GhTag   = 'v9.0.2-1'
        Files   = @(
            @{ From = 'morph-ffmpeg-9.0.2-1-win64\ffmpeg.exe'; To = 'ffmpeg.exe' },
            @{ From = 'morph-ffmpeg-9.0.2-1-win64\ffprobe.exe'; To = 'ffprobe.exe' },
            @{ From = 'morph-ffmpeg-9.0.2-1-win64\LICENSE.txt'; To = 'licenses\LICENSE.txt' },
            @{ From = 'morph-ffmpeg-9.0.2-1-win64\BUILDINFO.txt'; To = 'licenses\BUILDINFO.txt' },
            @{ From = 'morph-ffmpeg-9.0.2-1-win64\licenses'; To = 'licenses\licenses' }
        )
        # The shared FFmpeg DLLs are copied flat into vendor\ffmpeg\ next to the exes.
        Globs   = @('morph-ffmpeg-9.0.2-1-win64\*.dll')
    }
)

foreach ($dep in $deps) {
    $dest = Join-Path $vendor $dep.Name
    $stamp = Join-Path $dest '.fetched'
    $stampText = "$($dep.Version) $($dep.Sha256)"
    if (-not $Force -and (Test-Path -LiteralPath $stamp) -and ((Get-Content -LiteralPath $stamp -Raw).Trim() -eq $stampText)) {
        $allThere = $true
        foreach ($f in $dep.Files) { if (-not (Test-Path -LiteralPath (Join-Path $dest $f.To))) { $allThere = $false } }
        if ($allThere) {
            Write-Host "$($dep.Name) $($dep.Version): already in vendor\$($dep.Name)"
            continue
        }
    }

    $work = Join-Path ([System.IO.Path]::GetTempPath()) ('morph-fetch-' + [guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Force -Path $work | Out-Null
    try {
        $archive = Join-Path $work ([System.IO.Path]::GetFileName($dep.Url))
        Write-Host "$($dep.Name) $($dep.Version): downloading $($dep.Url)"
        try {
            Invoke-WebRequest -Uri $dep.Url -OutFile $archive -UseBasicParsing
        }
        catch {
            # If the plain download fails (network hiccup, rate limit, restricted network), fall back to the
            # GitHub CLI when it is installed; it uses the developer's login (or GH_TOKEN in CI).
            if (-not $dep.ContainsKey('GhRepo') -or -not (Get-Command gh -ErrorAction SilentlyContinue)) { throw }
            Write-Host "$($dep.Name): public download failed ($($_.Exception.Message)); trying gh release download"
            & gh release download $dep.GhTag -R $dep.GhRepo -p ([System.IO.Path]::GetFileName($dep.Url)) -D $work --clobber
            if ($LASTEXITCODE -ne 0) { throw "gh release download failed for $($dep.GhRepo) $($dep.GhTag) (exit code $LASTEXITCODE). Run 'gh auth login' first." }
        }

        $hash = (Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash.ToLowerInvariant()
        if ($hash -ne $dep.Sha256) {
            throw "SHA-256 mismatch for $($dep.Url): expected $($dep.Sha256), got $hash. Not installing it."
        }

        $extract = Join-Path $work 'x'
        New-Item -ItemType Directory -Force -Path $extract | Out-Null
        # Windows' tar.exe (bsdtar) reads .tgz and .zip.
        $members = @()
        if ($dep.ContainsKey('Members')) { $members = $dep.Members }
        & "$env:SystemRoot\System32\tar.exe" -xf $archive -C $extract @members
        if ($LASTEXITCODE -ne 0) { throw "tar failed to extract $archive (exit code $LASTEXITCODE)." }

        if (Test-Path -LiteralPath $dest) { Remove-Item -LiteralPath $dest -Recurse -Force }
        New-Item -ItemType Directory -Force -Path $dest | Out-Null
        foreach ($f in $dep.Files) {
            $src = Join-Path $extract $f.From
            $dst = Join-Path $dest $f.To
            if (-not (Test-Path -LiteralPath $src)) { throw "'$($f.From)' not found in $archive." }
            New-Item -ItemType Directory -Force -Path (Split-Path -Parent $dst) | Out-Null
            if ((Get-Item -LiteralPath $src).PSIsContainer) {
                New-Item -ItemType Directory -Force -Path $dst | Out-Null
                Copy-Item -Path (Join-Path $src '*') -Destination $dst -Recurse -Force
            }
            else {
                Copy-Item -LiteralPath $src -Destination $dst -Force
            }
        }
        if ($dep.ContainsKey('Globs')) {
            foreach ($g in $dep.Globs) {
                $found = @(Get-ChildItem -Path (Join-Path $extract $g) -File)
                if ($found.Count -eq 0) { throw "No files match '$g' in $archive." }
                $found | Copy-Item -Destination $dest -Force
            }
        }
        [System.IO.File]::WriteAllText($stamp, $stampText, (New-Object System.Text.UTF8Encoding($false)))
        Write-Host "$($dep.Name) $($dep.Version): installed in vendor\$($dep.Name)"
    }
    finally {
        Remove-Item -LiteralPath $work -Recurse -Force -ErrorAction SilentlyContinue
    }
}
