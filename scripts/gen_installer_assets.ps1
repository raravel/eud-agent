#Requires -Version 5.1
<#
.SYNOPSIS
    Generate the NSIS installer branding bitmaps from the app icon.

.DESCRIPTION
    Renders two 24-bit BMPs (the format NSIS expects) into src-tauri\installer\:
      - header.bmp  (150x57)  : top-right banner shown on installer pages
      - sidebar.bmp (164x314) : Welcome/Finish page side image
    Both are derived from src-tauri\icons\128x128.png so re-running after an icon
    change regenerates the branding. Committed alongside the script (reproducible).
#>
[CmdletBinding()]
param()

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Drawing

$RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$iconPath = Join-Path $RepoRoot 'src-tauri\icons\128x128.png'
$outDir = Join-Path $RepoRoot 'src-tauri\installer'

if (-not (Test-Path -LiteralPath $iconPath)) { throw "icon not found: $iconPath" }
New-Item -ItemType Directory -Force $outDir | Out-Null

$icon = [System.Drawing.Image]::FromFile($iconPath)

# Brand palette (matches the dark panel theme): slate-900 -> slate-800, slate-400.
$dark = [System.Drawing.Color]::FromArgb(15, 23, 42)
$dark2 = [System.Drawing.Color]::FromArgb(30, 41, 59)
$muted = [System.Drawing.Color]::FromArgb(148, 163, 184)

function Save-Bmp24([System.Drawing.Bitmap]$bmp, [string]$path) {
    # Force a plain 24bpp BMP (NSIS rejects some V5-header / alpha BMPs).
    $clone = $bmp.Clone(
        (New-Object System.Drawing.Rectangle(0, 0, $bmp.Width, $bmp.Height)),
        [System.Drawing.Imaging.PixelFormat]::Format24bppRgb)
    $clone.Save($path, [System.Drawing.Imaging.ImageFormat]::Bmp)
    $clone.Dispose()
}

# --- header.bmp 150x57 : white background, right-aligned logo + name ------------------
$hw = 150; $hh = 57
$hb = New-Object System.Drawing.Bitmap($hw, $hh, [System.Drawing.Imaging.PixelFormat]::Format24bppRgb)
$g = [System.Drawing.Graphics]::FromImage($hb)
$g.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::AntiAlias
$g.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
$g.TextRenderingHint = [System.Drawing.Text.TextRenderingHint]::AntiAliasGridFit
$g.Clear([System.Drawing.Color]::White)
$logo = 41
$g.DrawImage($icon, ($hw - $logo - 8), [int](($hh - $logo) / 2), $logo, $logo)
$hfont = New-Object System.Drawing.Font('Segoe UI', 11, [System.Drawing.FontStyle]::Bold)
$dbrush = New-Object System.Drawing.SolidBrush($dark)
$hsf = New-Object System.Drawing.StringFormat
$hsf.Alignment = [System.Drawing.StringAlignment]::Far
$hsf.LineAlignment = [System.Drawing.StringAlignment]::Center
$g.DrawString('eud-agent', $hfont, $dbrush,
    (New-Object System.Drawing.RectangleF(4, 0, ($hw - $logo - 16), $hh)), $hsf)
$g.Dispose()
Save-Bmp24 $hb (Join-Path $outDir 'header.bmp')
$hb.Dispose()

# --- sidebar.bmp 164x314 : dark vertical gradient, centered logo + name + subtitle ----
$sw = 164; $sh = 314
$sb = New-Object System.Drawing.Bitmap($sw, $sh, [System.Drawing.Imaging.PixelFormat]::Format24bppRgb)
$g = [System.Drawing.Graphics]::FromImage($sb)
$g.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::AntiAlias
$g.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
$g.TextRenderingHint = [System.Drawing.Text.TextRenderingHint]::AntiAliasGridFit
$grad = New-Object System.Drawing.Drawing2D.LinearGradientBrush(
    (New-Object System.Drawing.Point(0, 0)),
    (New-Object System.Drawing.Point(0, $sh)),
    $dark, $dark2)
$g.FillRectangle($grad, 0, 0, $sw, $sh)
$big = 96
$g.DrawImage($icon, [int](($sw - $big) / 2), 64, $big, $big)
$nfont = New-Object System.Drawing.Font('Segoe UI', 15, [System.Drawing.FontStyle]::Bold)
$wbrush = New-Object System.Drawing.SolidBrush([System.Drawing.Color]::White)
$csf = New-Object System.Drawing.StringFormat
$csf.Alignment = [System.Drawing.StringAlignment]::Center
$g.DrawString('eud-agent', $nfont, $wbrush,
    (New-Object System.Drawing.RectangleF(0, 178, $sw, 30)), $csf)
$subfont = New-Object System.Drawing.Font('Segoe UI', 8)
$mbrush = New-Object System.Drawing.SolidBrush($muted)
$g.DrawString('EUD Editor 3 AI agent', $subfont, $mbrush,
    (New-Object System.Drawing.RectangleF(0, 210, $sw, 24)), $csf)
$g.Dispose()
Save-Bmp24 $sb (Join-Path $outDir 'sidebar.bmp')
$sb.Dispose()

$icon.Dispose()
Write-Output "generated header.bmp (150x57) + sidebar.bmp (164x314) in $outDir"
