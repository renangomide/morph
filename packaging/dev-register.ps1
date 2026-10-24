# SPDX-License-Identifier: GPL-3.0-or-later
# Dev loop: build, stage layout\, register it as a loose MSIX package (needs Developer Mode), restart Explorer.
# Usage: .\packaging\dev-register.ps1 [-Release] [-NoBuild] [-NoRestartExplorer]
#        .\packaging\dev-register.ps1 -Unregister     (only removes the package)
[CmdletBinding()]
param(
    [switch]$Release,
    [switch]$NoBuild,
    [switch]$Unregister,
    [switch]$NoRestartExplorer
)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'common.ps1')

$profileName = 'debug'
if ($Release) { $profileName = 'release' }

function Test-DeveloperMode {
    try {
        $v = Get-ItemPropertyValue -Path 'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\AppModelUnlock' -Name 'AllowDevelopmentWithoutDevLicense' -ErrorAction Stop
        return ($v -eq 1)
    }
    catch { return $false }
}

# Explorer and any dllhost.exe hosting morph_shell.dll keep the DLL locked.
function Stop-ShellHosts {
    Write-Host 'Stopping Explorer and Morph COM surrogates...'
    foreach ($p in @(Get-Process -Name dllhost -ErrorAction SilentlyContinue)) {
        $loaded = $false
        try {
            foreach ($m in $p.Modules) { if ($m.ModuleName -ieq 'morph_shell.dll') { $loaded = $true; break } }
        }
        catch { $loaded = $false }   # access denied on protected processes: not ours
        if ($loaded) { Stop-Process -Id $p.Id -Force -ErrorAction SilentlyContinue }
    }
    Stop-Process -Name explorer -Force -ErrorAction SilentlyContinue
    Start-Sleep -Milliseconds 800
}

function Start-ExplorerIfNeeded {
    # Windows normally restarts the shell by itself; give it a moment before starting it manually.
    for ($i = 0; $i -lt 10; $i++) {
        if (Get-Process -Name explorer -ErrorAction SilentlyContinue) { return }
        Start-Sleep -Milliseconds 500
    }
    Start-Process explorer.exe
}

function Remove-MorphPackage {
    $existing = @(Get-AppxPackage -Name $script:MorphPackageName)
    foreach ($p in $existing) {
        Write-Host "Removing package $($p.PackageFullName)..."
        Remove-AppxPackage -Package $p.PackageFullName
    }
    # Packages from an earlier identity (e.g. Morph.Dev) register the same CLSID: remove them too.
    # Their settings/logs folder (%LOCALAPPDATA%\Packages\<old family name>) goes with them.
    foreach ($name in $script:MorphLegacyPackageNames) {
        foreach ($p in @(Get-AppxPackage -Name $name)) {
            Write-Host "Removing old-identity package $($p.PackageFullName) (publisher $($p.Publisher))..." -ForegroundColor Yellow
            Remove-AppxPackage -Package $p.PackageFullName
        }
    }
}

if ($Unregister) {
    Stop-ShellHosts
    try { Remove-MorphPackage }
    finally { if (-not $NoRestartExplorer) { Start-ExplorerIfNeeded } }
    Write-Host 'Morph dev package removed.'
    return
}

if (-not (Test-DeveloperMode)) {
    Write-Host 'Developer Mode is off, so loose packages cannot be registered.' -ForegroundColor Yellow
    Write-Host 'Enable it: Settings > System > For developers > Developer Mode (On), then run this script again.'
    Write-Host 'Or run: start ms-settings:developers'
    exit 1
}

if (-not $NoBuild) { Invoke-MorphBuild -Profile $profileName }

Stop-ShellHosts
try {
    Remove-MorphPackage
    $layout = New-Layout -Profile $profileName
    $manifest = Join-Path $layout 'AppxManifest.xml'
    Write-Host "Registering $manifest ..."
    Add-AppxPackage -Register $manifest
}
finally {
    # Always bring the desktop back, even if registration failed.
    if (-not $NoRestartExplorer) { Start-ExplorerIfNeeded }
}

$pkg = Get-AppxPackage -Name $script:MorphPackageName
if ($pkg) { Write-Host "Package family name: $($pkg.PackageFamilyName)" }
Write-Host 'Done. Now right-click a JPG and look for "Morph" in the modern context menu.'
