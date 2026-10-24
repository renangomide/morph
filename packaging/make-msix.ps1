# SPDX-License-Identifier: GPL-3.0-or-later
# Builds a release MSIX: target\Morph-<version>-x64.msix, signed with -PfxPath (default: packaging\cert\morph-dev.pfx
# from dev-cert.ps1). The certificate subject must be the manifest publisher (CN=OpenMorph, see common.ps1).
# Usage: .\packaging\make-msix.ps1 [-NoBuild] [-PfxPath <file>] [-PfxPassword <pwd>] [-TimestampUrl <url>] [-RequireSigned]
[CmdletBinding()]
param(
    [switch]$NoBuild,
    [string]$PfxPath,
    [string]$PfxPassword = 'morph-dev',
    # Optional RFC 3161 timestamp server (keeps a trusted signature valid after the cert expires).
    [string]$TimestampUrl,
    # Fail instead of writing an unsigned package when no .pfx is found (CI).
    [switch]$RequireSigned
)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'common.ps1')

if (-not $PfxPath) { $PfxPath = Join-Path $PSScriptRoot 'cert\morph-dev.pfx' }
if ($RequireSigned -and -not (Test-Path -LiteralPath $PfxPath)) {
    throw "No certificate at '$PfxPath'. Run .\packaging\dev-cert.ps1 or pass -PfxPath."
}

if (-not $NoBuild) { Invoke-MorphBuild -Profile 'release' }

$root = Get-RepoRoot
# Not layout\: that folder is the dev-registered package (dev-register.ps1).
$layout = New-Layout -Profile 'release' -Path (Join-Path $root 'target\release-layout')
$out = Join-Path $root ("target\Morph-{0}-x64.msix" -f (Get-MorphVersion))
New-Item -ItemType Directory -Force -Path (Split-Path -Parent $out) | Out-Null

# makeappx validates the manifest against the schema while packing.
$makeappx = Find-SdkTool 'makeappx.exe'
& $makeappx pack /o /h SHA256 /d $layout /p $out
if ($LASTEXITCODE -ne 0) { throw "makeappx failed (exit code $LASTEXITCODE)." }

if (Test-Path -LiteralPath $PfxPath) {
    $signtool = Find-SdkTool 'signtool.exe'
    $signArgs = @('sign', '/fd', 'SHA256', '/f', $PfxPath, '/p', $PfxPassword)
    if ($TimestampUrl) { $signArgs += @('/tr', $TimestampUrl, '/td', 'SHA256') }
    & $signtool @signArgs $out
    if ($LASTEXITCODE -ne 0) { throw "signtool failed (exit code $LASTEXITCODE)." }
    Write-Host "Signed package: $out"
}
else {
    Write-Host "Package is UNSIGNED (no pfx at '$PfxPath'). Run .\packaging\dev-cert.ps1 to create one." -ForegroundColor Yellow
    Write-Host "Package: $out"
}
Write-Host ("Size: {0:N1} MB" -f ((Get-Item -LiteralPath $out).Length / 1MB))
