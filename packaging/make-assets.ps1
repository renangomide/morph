# SPDX-License-Identifier: GPL-3.0-or-later
# Generates every logo the MSIX manifest needs, plus the multi-size morph.ico (Explorer menu icon,
# beside morph_shell.dll; also the tray icon), from ONE SVG: packaging\icon\morph.svg.
# Needs ImageMagick (`magick` on PATH, with its bundled librsvg).
# Usage: .\packaging\make-assets.ps1 [-OutDir <folder>]   (default: packaging\Assets)
#
# Writes the unqualified files the manifest names (Square44x44Logo.png, ...) and the MRT-qualified
# variants (.scale-*, .targetsize-*, _altform-unplated, _altform-lightunplated). common.ps1 (New-Layout)
# indexes them into resources.pri with makepri so Windows picks the right one.
[CmdletBinding()]
param(
    [string]$OutDir = (Join-Path $PSScriptRoot 'Assets')
)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

if (-not (Get-Command magick -ErrorAction SilentlyContinue)) { throw 'ImageMagick (magick) not found on PATH.' }
$svg = Join-Path $PSScriptRoot 'icon\morph.svg'
if (-not (Test-Path -LiteralPath $svg)) { throw "No icon SVG '$svg'." }
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
$OutDir = (Resolve-Path -LiteralPath $OutDir).Path
$tmp = Join-Path ([System.IO.Path]::GetTempPath()) ("morph-assets-" + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Force -Path $tmp | Out-Null

function Invoke-Magick { & magick @args; if ($LASTEXITCODE -ne 0) { throw "magick failed: $args" } }

# Renders the SVG (256x256 user units) straight at $Size px so librsvg anti-aliases at the final size.
function Export-Icon([int]$Size, [string]$OutPath) {
    $density = 72.0 * $Size / 256
    Invoke-Magick -background none -density $density $svg -resize "${Size}x${Size}!" "PNG32:$OutPath"
}

# Icon of $IconSize px centred on a transparent W x H canvas (tiles).
function Export-Tile([int]$W, [int]$H, [int]$IconSize, [string]$OutPath) {
    $icon = Join-Path $tmp "tile-$IconSize.png"
    Export-Icon $IconSize $icon
    Invoke-Magick -size "${W}x${H}" xc:none $icon -gravity center -composite "PNG32:$OutPath"
}

$scales = @(100, 125, 150, 200, 400)
function Get-Scaled([int]$Px, [int]$Scale) { [int][Math]::Round($Px * $Scale / 100.0) }

try {
    # App list / taskbar / Start icon (Square44x44Logo): full-bleed art.
    Export-Icon 44 (Join-Path $OutDir 'Square44x44Logo.png')
    foreach ($s in $scales) { Export-Icon (Get-Scaled 44 $s) (Join-Path $OutDir "Square44x44Logo.scale-$s.png") }
    foreach ($t in 16, 20, 24, 30, 32, 36, 40, 48, 60, 64, 72, 80, 96, 256) {
        $file = Join-Path $OutDir "Square44x44Logo.targetsize-$t.png"
        Export-Icon $t $file
        Copy-Item -LiteralPath $file -Destination (Join-Path $OutDir "Square44x44Logo.targetsize-${t}_altform-unplated.png") -Force
        Copy-Item -LiteralPath $file -Destination (Join-Path $OutDir "Square44x44Logo.targetsize-${t}_altform-lightunplated.png") -Force
    }

    # Store logo: full-bleed art.
    Export-Icon 50 (Join-Path $OutDir 'StoreLogo.png')
    foreach ($s in $scales) { Export-Icon (Get-Scaled 50 $s) (Join-Path $OutDir "StoreLogo.scale-$s.png") }

    # Tiles: icon at 2/3 of the tile height, centred.
    Export-Tile 150 150 100 (Join-Path $OutDir 'Square150x150Logo.png')
    Export-Tile 310 150 100 (Join-Path $OutDir 'Wide310x150Logo.png')
    foreach ($s in $scales) {
        $h = Get-Scaled 150 $s
        $icon = Get-Scaled 100 $s
        Export-Tile $h $h $icon (Join-Path $OutDir "Square150x150Logo.scale-$s.png")
        Export-Tile (Get-Scaled 310 $s) $h $icon (Join-Path $OutDir "Wide310x150Logo.scale-$s.png")
    }

    # morph.ico: every frame rendered at its own size (sharper than downscaling one master).
    $frames = foreach ($sz in 16, 20, 24, 32, 40, 48, 64, 256) {
        $f = Join-Path $tmp "ico-$sz.png"
        Export-Icon $sz $f
        $f
    }
    Invoke-Magick @frames (Join-Path $OutDir 'morph.ico')
    Write-Host "Assets from $svg written to $OutDir"
}
finally {
    Remove-Item -LiteralPath $tmp -Recurse -Force -ErrorAction SilentlyContinue
}
