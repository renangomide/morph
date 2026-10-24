# SPDX-License-Identifier: GPL-3.0-or-later
# Builds the distribution zip: target\release-zip\Morph-<version>-x64.zip with
#   Morph.msix (signed), Morph.cer, Install.cmd, Uninstall.cmd, README.txt   (from packaging\release\)
# Signs with -PfxPath (default: packaging\cert\morph-dev.pfx; created by dev-cert.ps1 if missing).
# Checks that the package signer and the manifest publisher are the identity in common.ps1, then runs
# "Install.cmd /check" on the staged folder (read-only: trusts nothing, installs nothing).
# Usage: .\packaging\make-release.ps1 [-NoBuild] [-PfxPath <file>] [-PfxPassword <pwd>] [-TimestampUrl <url>]
[CmdletBinding()]
param(
    [switch]$NoBuild,
    [string]$PfxPath,
    [string]$PfxPassword = 'morph-dev',
    [string]$TimestampUrl
)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'common.ps1')

$root = Get-RepoRoot
$version = Get-MorphVersion
if (-not $PfxPath) {
    $PfxPath = Join-Path $PSScriptRoot 'cert\morph-dev.pfx'
    if (-not (Test-Path -LiteralPath $PfxPath)) { & (Join-Path $PSScriptRoot 'dev-cert.ps1') -Password $PfxPassword }
}

$msixArgs = @{ RequireSigned = $true; PfxPath = $PfxPath; PfxPassword = $PfxPassword; NoBuild = $NoBuild }
if ($TimestampUrl) { $msixArgs.TimestampUrl = $TimestampUrl }
& (Join-Path $PSScriptRoot 'make-msix.ps1') @msixArgs
$msix = Join-Path $root ("target\Morph-{0}-x64.msix" -f $version)

# Verify identity and signature.
$cert = New-Object Security.Cryptography.X509Certificates.X509Certificate2($PfxPath, $PfxPassword)
$info = Get-MsixInfo $msix
$checks = @(
    @{ What = 'manifest Name'; Got = $info.Name; Want = $script:MorphPackageName },
    @{ What = 'manifest Publisher'; Got = $info.Publisher; Want = $script:MorphPublisher },
    @{ What = 'manifest Version'; Got = $info.Version; Want = (Get-MorphManifestVersion) },
    @{ What = 'signer subject'; Got = $info.Signer.Subject; Want = $script:MorphPublisher },
    @{ What = 'signer thumbprint'; Got = $info.Signer.Thumbprint; Want = $cert.Thumbprint }
)
foreach ($c in $checks) {
    if ($c.Got -ne $c.Want) { throw "Package check failed: $($c.What) is '$($c.Got)', expected '$($c.Want)'." }
    Write-Host ("OK  {0,-18} {1}" -f $c.What, $c.Got)
}

# Stage the zip contents.
$zipDir = Join-Path $root 'target\release-zip'
$stage = Join-Path $zipDir ("Morph-{0}-x64" -f $version)
$zip = "$stage.zip"
if (Test-Path -LiteralPath $stage) { Remove-Item -LiteralPath $stage -Recurse -Force }
if (Test-Path -LiteralPath $zip) { Remove-Item -LiteralPath $zip -Force }
New-Item -ItemType Directory -Force -Path $stage | Out-Null
Copy-Item -LiteralPath $msix -Destination (Join-Path $stage 'Morph.msix')
[IO.File]::WriteAllBytes((Join-Path $stage 'Morph.cer'), $cert.Export([Security.Cryptography.X509Certificates.X509ContentType]::Cert))
# Text files: CRLF (cmd.exe needs it for batch files), ASCII, whatever the checkout's line endings.
foreach ($name in @('Install.cmd', 'Uninstall.cmd', 'README.txt')) {
    $text = [IO.File]::ReadAllText((Join-Path $PSScriptRoot "release\$name"))
    $text = ($text -replace "`r?`n", "`r`n").Replace('{{VERSION}}', $version)
    if ($text -match '[^\x00-\x7F]') { throw "packaging\release\$name must be plain ASCII." }
    [IO.File]::WriteAllText((Join-Path $stage $name), $text, [Text.Encoding]::ASCII)
}

Add-Type -AssemblyName System.IO.Compression.FileSystem
[IO.Compression.ZipFile]::CreateFromDirectory($stage, $zip, [IO.Compression.CompressionLevel]::Optimal, $false)

# Dry run of the installer on the staged files (reads only).
Write-Host ''
Write-Host 'Running Install.cmd /check on the staged folder...'
& (Join-Path $stage 'Install.cmd') /check /nopause
if ($LASTEXITCODE -ne 0) { throw "Install.cmd /check failed (exit code $LASTEXITCODE)." }

Write-Host ''
Write-Host "Release zip: $zip"
Write-Host ("Size: {0:N1} MB" -f ((Get-Item -LiteralPath $zip).Length / 1MB))
