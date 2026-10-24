@echo off
rem SPDX-License-Identifier: GPL-3.0-or-later
rem Morph setup. Double-click it (or right-click > Run as administrator) in the folder extracted from the zip.
rem It trusts Morph.cer (LocalMachine\TrustedPeople; asks for administrator permission once, if needed)
rem and installs Morph.msix for the current user.
rem   Install.cmd /check     only check the files and this PC, change nothing
rem   Install.cmd /nopause   do not wait for a key at the end
setlocal EnableExtensions DisableDelayedExpansion
title Morph setup
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
$checkOnly = $ArgLine -match '(^|\s)/check(\s|$)'
$dir = Split-Path -Parent $env:MORPH_SELF
$msix = Join-Path $dir 'Morph.msix'
$cer = Join-Path $dir 'Morph.cer'

function Say([string]$Text, [string]$Color = 'Gray') { Write-Host $Text -ForegroundColor $Color }
function Stop-Setup([string]$Text, [string]$Details) {
    Write-Host ''
    Write-Host "Morph was NOT installed. $Text" -ForegroundColor Red
    if ($Details) { Write-Host ''; Write-Host "Details: $Details" -ForegroundColor DarkGray }
    exit 1
}
function Test-Admin {
    $p = New-Object Security.Principal.WindowsPrincipal([Security.Principal.WindowsIdentity]::GetCurrent())
    return $p.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
}
function Test-CertTrusted([string]$Thumbprint) {
    return (@(Get-ChildItem Cert:\LocalMachine\TrustedPeople | Where-Object { $_.Thumbprint -eq $Thumbprint }).Count -gt 0)
}

# Reads the identity from the package's AppxManifest.xml and the signer from AppxSignature.p7x.
function Get-MsixInfo([string]$Path) {
    Add-Type -AssemblyName System.IO.Compression, System.IO.Compression.FileSystem, System.Security
    $zip = [IO.Compression.ZipFile]::OpenRead($Path)
    try {
        $read = {
            param($name)
            $e = $zip.GetEntry($name)
            if (-not $e) { throw "$name is missing from the package." }
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

try {
    Say 'Morph setup' 'Cyan'
    Say "Folder: $dir"
    if ($checkOnly) { Say 'Check only (/check): nothing will be changed.' 'Yellow' }
    Say ''

    if (-not (Test-Path -LiteralPath $msix) -or -not (Test-Path -LiteralPath $cer)) {
        Stop-Setup 'Morph.msix and Morph.cer must be in the same folder as Install.cmd. Extract the whole zip first (right-click the zip > Extract All...), then run Install.cmd from the extracted folder.'
    }
    $os = [Environment]::OSVersion.Version
    if ($os.Major -lt 10 -or $os.Build -lt 19041) {
        Stop-Setup "Morph needs Windows 11 (or Windows 10 version 2004 or newer). This PC runs Windows $os."
    }

    # Files from a downloaded zip carry the "mark of the web"; clear it on the two files we use.
    if (-not $checkOnly) { foreach ($f in @($msix, $cer)) { try { Unblock-File -LiteralPath $f } catch { } } }

    try {
        $info = Get-MsixInfo $msix
        $cert = New-Object Security.Cryptography.X509Certificates.X509Certificate2($cer)
    }
    catch { Stop-Setup 'Morph.msix or Morph.cer is damaged. Download the zip again.' $_.Exception.Message }
    Say "Package:     $($info.Name) $($info.Version)"
    Say "Publisher:   $($info.Publisher)"
    Say "Certificate: $($cert.Subject), valid until $($cert.NotAfter.ToString('yyyy-MM-dd')), thumbprint $($cert.Thumbprint)"
    if ($info.Signer.Thumbprint -ne $cert.Thumbprint -or $info.Publisher -ne $cert.Subject) {
        Stop-Setup 'Morph.msix was not signed with Morph.cer. Use the files from the same zip (download it again).' "package signer $($info.Signer.Subject) $($info.Signer.Thumbprint); publisher $($info.Publisher)"
    }
    if ($cert.NotAfter -lt (Get-Date)) {
        Stop-Setup "The Morph certificate expired on $($cert.NotAfter.ToString('yyyy-MM-dd')). Download a newer Morph zip."
    }
    $installed = @(Get-AppxPackage -Name $info.Name)
    if ($installed.Count) { Say "Installed now: $($installed[0].Name) $($installed[0].Version) (will be replaced)" }
    Say ''

    # 1. Trust the certificate (machine-wide, Trusted People only: it is not a root CA).
    if (Test-CertTrusted $cert.Thumbprint) {
        Say '[1/2] The Morph certificate is already trusted on this PC.' 'Green'
    }
    elseif ($checkOnly) {
        Say "[1/2] Would trust the Morph certificate (Local Machine > Trusted People). Running as administrator: $(Test-Admin)"
    }
    elseif (Test-Admin) {
        Say '[1/2] Trusting the Morph certificate (Local Machine > Trusted People)...'
        try { Import-Certificate -FilePath $cer -CertStoreLocation Cert:\LocalMachine\TrustedPeople | Out-Null }
        catch { Stop-Setup 'Windows did not accept the Morph certificate.' $_.Exception.Message }
    }
    else {
        # Only this step runs elevated; the package itself is installed for the signed-in user below.
        Say '[1/2] Windows will now ask for administrator permission to trust the Morph certificate. Click Yes.'
        $errFile = Join-Path ([IO.Path]::GetTempPath()) ('morph-setup-' + [guid]::NewGuid().ToString('N') + '.txt')
        $q = { param($s) "'" + $s.Replace("'", "''") + "'" }
        $child = "`$ErrorActionPreference='Stop'; try { Import-Certificate -FilePath $(& $q $cer) -CertStoreLocation Cert:\LocalMachine\TrustedPeople | Out-Null; exit 0 } catch { try { [IO.File]::WriteAllText($(& $q $errFile), `$_.Exception.Message) } catch { }; exit 1 }"
        $enc = [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes($child))
        try {
            $p = Start-Process -FilePath "$env:SystemRoot\System32\WindowsPowerShell\v1.0\powershell.exe" -Verb RunAs -WindowStyle Hidden -Wait -PassThru `
                -ArgumentList '-NoLogo', '-NoProfile', '-NonInteractive', '-ExecutionPolicy', 'Bypass', '-EncodedCommand', $enc
        }
        catch { Stop-Setup 'Administrator permission is needed once, to trust the Morph certificate. Run Install.cmd again and click Yes.' $_.Exception.Message }
        $msg = $null
        if (Test-Path -LiteralPath $errFile) { $msg = [IO.File]::ReadAllText($errFile); Remove-Item -LiteralPath $errFile -Force -ErrorAction SilentlyContinue }
        if ($p.ExitCode -ne 0 -or -not (Test-CertTrusted $cert.Thumbprint)) {
            Stop-Setup 'Windows did not accept the Morph certificate.' $msg
        }
        Say '      Done.' 'Green'
    }

    # 2. Install the package for the current user.
    if ($checkOnly) {
        Say "[2/2] Would install $msix"
        Say ''
        Say 'Check passed: the files are complete and belong together. Nothing was changed.' 'Green'
        exit 0
    }
    Say '[2/2] Installing Morph...'
    try { Add-AppxPackage -Path $msix -ForceApplicationShutdown -ForceUpdateFromAnyVersion }
    catch {
        $hr = '0x{0:X8}' -f $_.Exception.HResult
        $why = switch ($hr) {
            '0x800B0109' { 'Windows does not trust the Morph certificate. Run Install.cmd again and click Yes when asked for permission.'; break }
            '0x80073D02' { 'Morph is running. Close it (also its icon near the clock) and run Install.cmd again.'; break }
            '0x80073CFB' { 'A different build with the same version is already installed. Run Uninstall.cmd, then Install.cmd again.'; break }
            '0x80073CFF' { 'This PC does not allow installing apps from outside the Store. Settings > System > For developers: turn on "Install apps from any source", then try again.'; break }
            default { 'Windows could not install the package.' }
        }
        Stop-Setup $why "$hr $($_.Exception.Message)"
    }
    $now = @(Get-AppxPackage -Name $info.Name)
    if (-not $now.Count) { Stop-Setup 'Windows reported success, but the package is not listed as installed.' }
    Say ''
    Say "Morph $($now[0].Version) is installed." 'Green'
    Say 'Right-click photos, PDFs, videos, music or Office files in File Explorer and choose "Morph".'
    Say 'Morph is also in the Start menu. You can close this window.'
    exit 0
}
catch {
    Stop-Setup 'Something unexpected went wrong.' ($_ | Out-String).Trim()
}
