@echo off
rem SPDX-License-Identifier: GPL-3.0-or-later
rem Removes Morph for the current user (with its settings and logs; converted files stay).
rem Then offers to remove the Morph certificate from LocalMachine\TrustedPeople (asks for admin permission).
rem   Uninstall.cmd /check       only show what would be removed, change nothing
rem   Uninstall.cmd /removecert  remove the certificate without asking;  /keepcert  keep it without asking
rem   Uninstall.cmd /nopause     do not wait for a key at the end
setlocal EnableExtensions DisableDelayedExpansion
title Morph uninstall
set "MORPH_SELF=%~f0"
set "MORPH_ARGS=%*"
"%SystemRoot%\System32\WindowsPowerShell\v1.0\powershell.exe" -NoLogo -NoProfile -ExecutionPolicy Bypass -Command "$t=[IO.File]::ReadAllText($env:MORPH_SELF); $i=$t.LastIndexOf('#### POWERSHELL ####'); & ([ScriptBlock]::Create($t.Substring($i))) ([string]$env:MORPH_ARGS)"
set "MORPH_RC=%ERRORLEVEL%"
echo(%MORPH_ARGS%| find /i "/nopause" >nul || pause
exit /b %MORPH_RC%

#### POWERSHELL ####
# Everything below runs in Windows PowerShell 5.1 (the batch part above never reaches it).
param([string]$ArgLine)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
# Must match morph_formats::{PACKAGE_NAME, PACKAGE_PUBLISHER} (checked by a morph-formats test).
$packageName = 'OpenMorph.Morph'
$publisher = 'CN=OpenMorph'
function Has-Flag([string]$f) { return ($ArgLine -match "(^|\s)/$f(\s|$)") }
$checkOnly = Has-Flag 'check'

function Say([string]$Text, [string]$Color = 'Gray') { Write-Host $Text -ForegroundColor $Color }
function Stop-Uninstall([string]$Text, [string]$Details) {
    Write-Host ''
    Write-Host $Text -ForegroundColor Red
    if ($Details) { Write-Host ''; Write-Host "Details: $Details" -ForegroundColor DarkGray }
    exit 1
}
function Test-Admin {
    $p = New-Object Security.Principal.WindowsPrincipal([Security.Principal.WindowsIdentity]::GetCurrent())
    return $p.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
}
function Get-MorphCerts { return @(Get-ChildItem Cert:\LocalMachine\TrustedPeople | Where-Object { $_.Subject -eq $publisher }) }

try {
    Say 'Morph uninstall' 'Cyan'
    if ($checkOnly) { Say 'Check only (/check): nothing will be changed.' 'Yellow' }
    Say ''

    # 1. The package (for the signed-in user; Windows also deletes its settings and logs).
    $pkgs = @(Get-AppxPackage -Name $packageName)
    if (-not $pkgs.Count) {
        Say "Morph is not installed for this user ($env:USERNAME)."
    }
    foreach ($p in $pkgs) {
        if ($checkOnly) { Say "Would remove $($p.PackageFullName)"; continue }
        Say "Removing Morph $($p.Version)..."
        try { Remove-AppxPackage -Package $p.PackageFullName }
        catch { Stop-Uninstall 'Windows could not remove Morph. Close Morph (also its icon near the clock) and try again, or use Settings > Apps > Installed apps > Morph > Uninstall.' $_.Exception.Message }
        Say 'Morph was removed. Files you converted stay where they are.' 'Green'
    }

    # 2. The certificate (optional).
    $certs = @(Get-MorphCerts)
    if (-not $certs.Count) { Say 'The Morph certificate is not in Trusted People.'; exit 0 }
    Say ''
    if ($checkOnly) { Say "Would offer to remove $($certs.Count) Morph certificate(s) ($publisher) from Local Machine > Trusted People."; exit 0 }
    $remove = Has-Flag 'removecert'
    if (-not $remove -and -not (Has-Flag 'keepcert')) {
        $answer = Read-Host "Also remove the Morph certificate ($publisher) from the trusted certificates? Type Y and press Enter (just Enter keeps it)"
        $remove = $answer -match '^\s*(y|yes)\s*$'
    }
    if (-not $remove) { Say 'The certificate was kept.'; exit 0 }

    $cmd = "Get-ChildItem Cert:\LocalMachine\TrustedPeople | Where-Object { `$_.Subject -eq '$publisher' } | Remove-Item"
    if (Test-Admin) {
        Invoke-Expression $cmd
    }
    else {
        Say 'Windows will now ask for administrator permission to remove the certificate. Click Yes.'
        $enc = [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes("`$ErrorActionPreference='Stop'; $cmd"))
        try {
            Start-Process -FilePath "$env:SystemRoot\System32\WindowsPowerShell\v1.0\powershell.exe" -Verb RunAs -WindowStyle Hidden -Wait `
                -ArgumentList '-NoLogo', '-NoProfile', '-NonInteractive', '-ExecutionPolicy', 'Bypass', '-EncodedCommand', $enc
        }
        catch { Stop-Uninstall 'The certificate was kept: administrator permission was not given.' $_.Exception.Message }
    }
    if (@(Get-MorphCerts).Count) { Stop-Uninstall 'The certificate could not be removed.' }
    Say 'The Morph certificate was removed.' 'Green'
    exit 0
}
catch {
    Stop-Uninstall 'Something unexpected went wrong.' ($_ | Out-String).Trim()
}
