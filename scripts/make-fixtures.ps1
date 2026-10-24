# SPDX-License-Identifier: GPL-3.0-or-later
<#
.SYNOPSIS
  Regenerates the image test fixtures in tests\fixtures\ (see the README there).

.DESCRIPTION
  Needs ImageMagick 7 (magick) and ffmpeg with libx265 (only for the HEIC/HEIF
  samples) on PATH. Works in Windows PowerShell 5.1. Source is ASCII-only; the
  Unicode file names are built from code points.

  sample.svg is hand-written and is NOT touched by this script.
#>
[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'

$root = Split-Path -Parent $PSScriptRoot
$out = Join-Path $root 'tests\fixtures'
$stage = Join-Path ([System.IO.Path]::GetTempPath()) ('morph-fixtures-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Force -Path $out | Out-Null
New-Item -ItemType Directory -Force -Path $stage | Out-Null

function Invoke-Magick {
    & magick @args
    if ($LASTEXITCODE -ne 0) { throw ('magick failed: ' + ($args -join ' ')) }
}

function Out-Path([string]$name) { return (Join-Path $out $name) }
function Stage-Path([string]$name) { return (Join-Path $stage $name) }

# ---------------------------------------------------------------- byte helpers
function Bytes { return , ([byte[]]$args) }
function Asc([string]$s) { return , ([System.Text.Encoding]::ASCII.GetBytes($s)) }
function BE16([int]$n) { return , ([byte[]]@((($n -shr 8) -band 255), ($n -band 255))) }
function BE32([long]$n) {
    return , ([byte[]]@((($n -shr 24) -band 255), (($n -shr 16) -band 255), (($n -shr 8) -band 255), ($n -band 255)))
}
function Join-Bytes {
    $l = New-Object 'System.Collections.Generic.List[byte]'
    foreach ($p in $args) { $l.AddRange([byte[]]$p) }
    return , ($l.ToArray())
}
function Box([string]$type, [byte[]]$payload) {
    return , (Join-Bytes (BE32 ($payload.Length + 8)) (Asc $type) $payload)
}

# ---------------------------------------------------------------- EXIF helpers
function New-Writer {
    $ms = New-Object System.IO.MemoryStream
    $bw = New-Object System.IO.BinaryWriter($ms)
    return @{ Stream = $ms; Writer = $bw }
}
function Put-Entry($bw, [int]$tag, [int]$type, [long]$count, [long]$value) {
    $bw.Write([uint16]$tag)
    $bw.Write([uint16]$type)
    $bw.Write([uint32]$count)
    $bw.Write([uint32]$value)
}
function Put-Rational($bw, [long]$num, [long]$den) {
    $bw.Write([uint32]$num)
    $bw.Write([uint32]$den)
}

# TIFF block (little endian) with only an Orientation tag.
function New-OrientationExif([int]$orientation) {
    $w = New-Writer
    $bw = $w.Writer
    $bw.Write([byte[]]@(0x49, 0x49, 0x2A, 0x00))
    $bw.Write([uint32]8)
    $bw.Write([uint16]1)
    Put-Entry $bw 0x0112 3 1 $orientation
    $bw.Write([uint32]0)
    $bw.Flush()
    return , ($w.Stream.ToArray())
}

# TIFF block with camera tags, dates, ISO and a GPS IFD (San Francisco).
function New-GpsExif {
    $w = New-Writer
    $bw = $w.Writer
    $make = [System.Text.Encoding]::ASCII.GetBytes("Morph Test`0")
    $model = [System.Text.Encoding]::ASCII.GetBytes("Fixture Cam`0")
    $dt = [System.Text.Encoding]::ASCII.GetBytes("2024:05:17 10:30:00`0")

    $ifd0 = 8
    $exifIfd = $ifd0 + 2 + (6 * 12) + 4
    $gpsIfd = $exifIfd + 2 + (2 * 12) + 4
    $p = $gpsIfd + 2 + (4 * 12) + 4
    $offMake = $p; $p += $make.Length
    $offModel = $p; $p += $model.Length
    $offDt = $p; $p += $dt.Length
    $offDto = $p; $p += $dt.Length
    $offLat = $p; $p += 24
    $offLon = $p; $p += 24

    $bw.Write([byte[]]@(0x49, 0x49, 0x2A, 0x00))
    $bw.Write([uint32]$ifd0)

    $bw.Write([uint16]6)
    Put-Entry $bw 0x010F 2 $make.Length $offMake
    Put-Entry $bw 0x0110 2 $model.Length $offModel
    Put-Entry $bw 0x0112 3 1 1
    Put-Entry $bw 0x0132 2 $dt.Length $offDt
    Put-Entry $bw 0x8769 4 1 $exifIfd
    Put-Entry $bw 0x8825 4 1 $gpsIfd
    $bw.Write([uint32]0)

    $bw.Write([uint16]2)
    Put-Entry $bw 0x8827 3 1 200
    Put-Entry $bw 0x9003 2 $dt.Length $offDto
    $bw.Write([uint32]0)

    $bw.Write([uint16]4)
    Put-Entry $bw 0x0001 2 2 78    # GPSLatitudeRef "N"
    Put-Entry $bw 0x0002 5 3 $offLat
    Put-Entry $bw 0x0003 2 2 87    # GPSLongitudeRef "W"
    Put-Entry $bw 0x0004 5 3 $offLon
    $bw.Write([uint32]0)

    $bw.Write($make)
    $bw.Write($model)
    $bw.Write($dt)
    $bw.Write($dt)
    Put-Rational $bw 37 1; Put-Rational $bw 46 1; Put-Rational $bw 297 10
    Put-Rational $bw 122 1; Put-Rational $bw 25 1; Put-Rational $bw 99 10
    $bw.Flush()
    return , ($w.Stream.ToArray())
}

# Insert an APP1 "Exif" segment after SOI (and after the JFIF APP0 when present).
function Add-ExifToJpeg([string]$src, [string]$dst, [byte[]]$tiff) {
    $b = [System.IO.File]::ReadAllBytes($src)
    if ($b[0] -ne 0xFF -or $b[1] -ne 0xD8) { throw "not a JPEG: $src" }
    $at = 2
    if ($b[2] -eq 0xFF -and $b[3] -eq 0xE0) { $at = 4 + ($b[4] * 256 + $b[5]) }
    $payload = Join-Bytes (Asc 'Exif') (Bytes 0 0) $tiff
    $seg = Join-Bytes (Bytes 0xFF 0xE1) (BE16 ($payload.Length + 2)) $payload
    $head = New-Object byte[] $at
    [Array]::Copy($b, 0, $head, 0, $at)
    $tail = New-Object byte[] ($b.Length - $at)
    [Array]::Copy($b, $at, $tail, 0, $tail.Length)
    [System.IO.File]::WriteAllBytes($dst, [byte[]](Join-Bytes $head $seg $tail))
}

# ---------------------------------------------------------------- HEIF helper
# Wraps one HEVC intra picture (Annex B stream from x265) into a HEIF file.
function New-Heif([string]$hevcPath, [string]$dst, [int]$width, [int]$height, [string]$major) {
    $d = [System.IO.File]::ReadAllBytes($hevcPath)
    $starts = New-Object 'System.Collections.Generic.List[int]'
    for ($i = 0; $i -lt $d.Length - 3; $i++) {
        if ($d[$i] -eq 0 -and $d[$i + 1] -eq 0 -and $d[$i + 2] -eq 1) { $starts.Add($i + 3) }
    }
    $nals = @()
    for ($k = 0; $k -lt $starts.Count; $k++) {
        $s = $starts[$k]
        $e = $d.Length
        if ($k + 1 -lt $starts.Count) { $e = $starts[$k + 1] - 3 }
        while ($e -gt $s -and $d[$e - 1] -eq 0) { $e-- }
        $n = New-Object byte[] ($e - $s)
        [Array]::Copy($d, $s, $n, 0, $n.Length)
        $nals += , $n
    }
    $vps = $null; $sps = $null; $pps = $null
    $sample = New-Object 'System.Collections.Generic.List[byte]'
    foreach ($n in $nals) {
        $t = ($n[0] -shr 1) -band 0x3F
        if ($t -eq 32) { $vps = $n }
        elseif ($t -eq 33) { $sps = $n }
        elseif ($t -eq 34) { $pps = $n }
        elseif ($t -lt 32) {
            $sample.AddRange([byte[]](BE32 $n.Length))
            $sample.AddRange([byte[]]$n)
        }
    }
    if ($null -eq $vps -or $null -eq $sps -or $null -eq $pps -or $sample.Count -eq 0) {
        throw 'x265 output is missing parameter sets or slices'
    }
    $ptl = New-Object byte[] 12
    [Array]::Copy($sps, 3, $ptl, 0, 12)

    $arrays = Join-Bytes (Bytes 0xA0) (BE16 1) (BE16 $vps.Length) $vps `
        (Bytes 0xA1) (BE16 1) (BE16 $sps.Length) $sps `
        (Bytes 0xA2) (BE16 1) (BE16 $pps.Length) $pps
    $hvcc = Box 'hvcC' (Join-Bytes (Bytes 1) $ptl (Bytes 0xF0 0x00 0xFC 0xFD 0xF8 0xF8 0x00 0x00 0x0F 3) $arrays)
    $ispe = Box 'ispe' (Join-Bytes (BE32 0) (BE32 $width) (BE32 $height))

    $ftyp = Box 'ftyp' (Join-Bytes (Asc $major) (BE32 0) (Asc 'mif1') (Asc 'heic'))
    $hdlr = Box 'hdlr' (Join-Bytes (BE32 0) (BE32 0) (Asc 'pict') (New-Object byte[] 12) (Bytes 0))
    $pitm = Box 'pitm' (Join-Bytes (BE32 0) (BE16 1))
    $infe = Box 'infe' (Join-Bytes (BE32 0x02000000) (BE16 1) (BE16 0) (Asc 'hvc1') (Bytes 0))
    $iinf = Box 'iinf' (Join-Bytes (BE32 0) (BE16 1) $infe)
    $ipma = Box 'ipma' (Join-Bytes (BE32 0) (BE32 1) (BE16 1) (Bytes 2 0x01 0x82))
    $ipco = Box 'ipco' (Join-Bytes $ispe $hvcc)
    $iprp = Box 'iprp' (Join-Bytes $ipco $ipma)

    $offset = 0
    for ($pass = 0; $pass -lt 2; $pass++) {
        $iloc = Box 'iloc' (Join-Bytes (BE32 0) (Bytes 0x44 0x00) (BE16 1) (BE16 1) (BE16 0) (BE16 1) (BE32 $offset) (BE32 $sample.Count))
        $meta = Box 'meta' (Join-Bytes (BE32 0) $hdlr $pitm $iloc $iinf $iprp)
        $offset = $ftyp.Length + $meta.Length + 8
    }
    $mdat = Box 'mdat' ([byte[]]$sample.ToArray())
    [System.IO.File]::WriteAllBytes($dst, [byte[]](Join-Bytes $ftyp $meta $mdat))
}

try {
    # ------------------------------------------------------------ base pictures
    $base = Stage-Path 'base.png'
    Invoke-Magick -size 96x64 'gradient:#ff7a00-#0040ff' -swirl 90 `
        -fill 'white' -draw 'circle 30,30 30,42' -fill '#101010' -draw 'rectangle 60,40 86,56' $base

    $alpha = Stage-Path 'alpha.png'
    Invoke-Magick -size 96x64 'xc:none' -fill '#e03030' -draw 'circle 36,32 36,6' `
        -fill 'rgba(48,80,224,0.5)' -draw 'rectangle 30,34 90,60' "PNG32:$alpha"

    # ------------------------------------------------------------ plain formats
    Invoke-Magick $base -quality 80 (Out-Path 'sample.jpg')
    Copy-Item -LiteralPath (Out-Path 'sample.jpg') -Destination (Out-Path 'sample.jpeg') -Force
    Copy-Item -LiteralPath (Out-Path 'sample.jpg') -Destination (Out-Path 'sample.jfif') -Force

    Invoke-Magick $alpha "PNG32:$(Out-Path 'sample.png')"
    Invoke-Magick $base -quality 70 (Out-Path 'sample.webp')
    Invoke-Magick $base -quality 40 (Out-Path 'sample.avif')
    Invoke-Magick $base "BMP3:$(Out-Path 'sample.bmp')"
    Invoke-Magick $base -compress LZW (Out-Path 'sample.tif')
    Invoke-Magick $alpha -compress LZW (Out-Path 'sample.tiff')
    Invoke-Magick $base -compress RLE (Out-Path 'sample.tga')
    Invoke-Magick $base (Out-Path 'sample.qoi')

    # Animated GIF: 3 frames, 64x48.
    Invoke-Magick -delay 20 -size 64x48 'xc:#d03030' 'xc:#30d030' 'xc:#3030d0' -loop 0 (Out-Path 'sample.gif')

    # Multi-size ICO (largest 48x48 to stay small).
    $icoSrc = Stage-Path 'ico-src.png'
    Invoke-Magick -size 64x64 'xc:none' -fill '#e03030' -draw 'circle 32,32 32,4' `
        -fill '#ffffff' -draw 'rectangle 24,24 40,40' "PNG32:$icoSrc"
    Invoke-Magick $icoSrc -define 'icon:auto-resize=48,32,24,16' (Out-Path 'sample.ico')

    # ------------------------------------------------------------ HEIC / HEIF
    $hevc = Stage-Path 'sample.hevc'
    & ffmpeg -hide_banner -loglevel error -y -i $base -frames:v 1 -pix_fmt yuv420p -c:v libx265 `
        -x265-params 'log-level=none:keyint=1' -f hevc $hevc
    if ($LASTEXITCODE -ne 0) { throw 'ffmpeg (libx265) failed' }
    New-Heif $hevc (Out-Path 'sample.heic') 96 64 'heic'
    New-Heif $hevc (Out-Path 'sample.heif') 96 64 'mif1'

    # ------------------------------------------------------------ metadata JPEGs
    # Stored 64x32, EXIF orientation 6 (rotate 90 CW): displays as 32x64.
    $orientBase = Stage-Path 'orient-base.jpg'
    Invoke-Magick -size 64x32 'gradient:#ffd000-#0080ff' -fill 'white' -draw 'rectangle 2,2 20,12' `
        -fill 'black' -draw 'circle 50,16 50,24' -quality 85 $orientBase
    Add-ExifToJpeg $orientBase (Out-Path 'orientation6.jpg') (New-OrientationExif 6)

    # GPS + camera EXIF tags.
    $gpsBase = Stage-Path 'gps-base.jpg'
    Invoke-Magick -size 96x64 'gradient:#30d080-#6020c0' -fill 'white' -draw 'circle 48,32 48,44' -quality 85 $gpsBase
    Add-ExifToJpeg $gpsBase (Out-Path 'exif-gps.jpg') (New-GpsExif)

    # Embedded ICC profile (the sRGB profile shipped with Windows).
    $icc = Join-Path $env:SystemRoot 'System32\spool\drivers\color\sRGB Color Space Profile.icm'
    if (Test-Path -LiteralPath $icc) {
        Invoke-Magick -size 64x48 'gradient:#ff4060-#4060ff' -fill 'white' -draw 'circle 32,24 32,34' `
            -profile $icc -quality 85 (Out-Path 'icc-profile.jpg')
    }
    else {
        Write-Warning 'sRGB profile not found; icc-profile.jpg skipped.'
    }

    # ------------------------------------------------------------ Unicode names
    # Created under ASCII names first (PowerShell 5.1 native-argument encoding), then renamed.
    $ferias = 'F' + [char]0xE9 + 'rias ' + [char]0x590F + '.jpg'
    $cafe = 'caf' + [char]0xE9 + '.png'
    $t1 = Stage-Path 'u1.jpg'
    $t2 = Stage-Path 'u2.png'
    Invoke-Magick -size 64x48 'gradient:#40c0ff-#ff40c0' -fill 'white' -draw 'circle 20,24 20,34' -quality 85 $t1
    Invoke-Magick -size 48x48 'xc:none' -fill '#30a030' -draw 'circle 24,24 24,4' "PNG32:$t2"
    Copy-Item -LiteralPath $t1 -Destination (Out-Path $ferias) -Force
    Copy-Item -LiteralPath $t2 -Destination (Out-Path $cafe) -Force

    # ------------------------------------------------------------ corrupt file
    $png = [System.IO.File]::ReadAllBytes((Out-Path 'sample.png'))
    $keep = [Math]::Min(120, $png.Length)
    $broken = New-Object byte[] $keep
    [Array]::Copy($png, 0, $broken, 0, $keep)
    [System.IO.File]::WriteAllBytes((Out-Path 'broken.png'), $broken)

    Write-Host ('Fixtures written to ' + $out)
    Get-ChildItem -LiteralPath $out -File | Sort-Object Name | ForEach-Object {
        Write-Host ('  {0,-22} {1,7} bytes' -f $_.Name, $_.Length)
    }
}
finally {
    Remove-Item -LiteralPath $stage -Recurse -Force -ErrorAction SilentlyContinue
}
