# SPDX-License-Identifier: GPL-3.0-or-later
# Creates (or reuses) the self-signed code-signing cert from common.ps1 (CN=OpenMorph) and exports packaging\cert\morph-dev.pfx/.cer.
# Usage: .\packaging\dev-cert.ps1 [-Password morph-dev]
# It does NOT trust the cert; it prints the admin command for that.
[CmdletBinding()]
param([string]$Password = 'morph-dev')
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'common.ps1')

$subject = $script:MorphPublisher
$certDir = Join-Path $PSScriptRoot 'cert'
New-Item -ItemType Directory -Force -Path $certDir | Out-Null
$pfx = Join-Path $certDir 'morph-dev.pfx'
$cer = Join-Path $certDir 'morph-dev.cer'

$cert = Get-ChildItem Cert:\CurrentUser\My |
    Where-Object { $_.Subject -eq $subject -and $_.HasPrivateKey -and $_.NotAfter -gt (Get-Date) } |
    Sort-Object NotAfter -Descending | Select-Object -First 1

if ($cert) {
    Write-Host "Reusing existing certificate $($cert.Thumbprint)"
}
else {
    $cert = New-SelfSignedCertificate -Type Custom -Subject $subject -KeyUsage DigitalSignature `
        -FriendlyName 'OpenMorph (Morph package signing)' -CertStoreLocation 'Cert:\CurrentUser\My' -KeyExportPolicy Exportable `
        -NotAfter (Get-Date).AddYears(3) `
        -TextExtension @('2.5.29.37={text}1.3.6.1.5.5.7.3.3', '2.5.29.19={text}')
    Write-Host "Created certificate $($cert.Thumbprint)"
}

$secure = ConvertTo-SecureString -String $Password -AsPlainText -Force
Export-PfxCertificate -Cert $cert -FilePath $pfx -Password $secure | Out-Null
Export-Certificate -Cert $cert -FilePath $cer | Out-Null
Write-Host "Exported: $pfx"
Write-Host "Exported: $cer"
Write-Host ''
Write-Host 'To trust it for installing a signed .msix, run this in an ADMIN PowerShell:'
Write-Host "  Import-Certificate -FilePath `"$cer`" -CertStoreLocation Cert:\LocalMachine\TrustedPeople"
