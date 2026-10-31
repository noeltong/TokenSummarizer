# Crop and magnify the tab strip region of a screenshot for visual inspection.
# Usage: powershell -File tools\crop-tabs.ps1 -InFile shots\main-total.png -OutFile shots\_tabs_crop.png [-Scale 4]
param(
    [string]$InFile = "shots\main-total.png",
    [string]$OutFile = "shots\_tabs_crop.png",
    [int]$Scale = 4,
    [int]$Top = 0,
    [int]$Height = 62
)
$ErrorActionPreference = "Stop"
Add-Type -AssemblyName System.Drawing
$srcPath = Join-Path (Get-Location) $InFile
$src = [System.Drawing.Bitmap]::FromFile($srcPath)
$h = [Math]::Min($Height, $src.Height)
$w = $src.Width
$rect = New-Object System.Drawing.Rectangle -ArgumentList 0, $Top, $w, $h
$crop = $src.Clone($rect, $src.PixelFormat)
$bw = $w * $Scale
$bh = $h * $Scale
$big = New-Object System.Drawing.Bitmap -ArgumentList $bw, $bh
$g = [System.Drawing.Graphics]::FromImage($big)
$g.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::NearestNeighbor
$g.PixelOffsetMode = [System.Drawing.Drawing2D.PixelOffsetMode]::Half
$g.DrawImage($crop, 0, 0, $bw, $bh)
$g.Dispose()
$outPath = Join-Path (Get-Location) $OutFile
$outDir = Split-Path -Parent $outPath
if ($outDir -and !(Test-Path $outDir)) { New-Item -ItemType Directory -Path $outDir | Out-Null }
$big.Save($outPath, [System.Drawing.Imaging.ImageFormat]::Png)
$big.Dispose(); $crop.Dispose(); $src.Dispose()
Write-Host "saved $OutFile ($bw x $bh)"
