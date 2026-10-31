# Generate assets/app.ico (multi-size) and assets/preview.png.
# Design: rounded square, diagonal blue gradient, white bold "T",
# ascending mini bars for sizes >= 48.
param(
    [string]$AssetsDir = ""
)

$ErrorActionPreference = 'Stop'
if (-not $AssetsDir) {
    $scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
    $AssetsDir = Join-Path (Split-Path -Parent $scriptDir) 'assets'
}
$OutIco = Join-Path $AssetsDir 'app.ico'
$OutPng = Join-Path $AssetsDir 'preview.png'
New-Item -ItemType Directory -Force -Path $AssetsDir | Out-Null

Add-Type -AssemblyName System.Drawing

function New-IconBitmap([int]$size) {
    $bmp = New-Object System.Drawing.Bitmap($size, $size, [System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::AntiAlias

    $radius = [Math]::Max(2.0, $size * 0.22)
    $d = $radius * 2
    $path = New-Object System.Drawing.Drawing2D.GraphicsPath
    $path.AddArc(0, 0, $d, $d, 180, 90)
    $path.AddArc($size - $d, 0, $d, $d, 270, 90)
    $path.AddArc($size - $d, $size - $d, $d, $d, 0, 90)
    $path.AddArc(0, $size - $d, $d, $d, 90, 90)
    $path.CloseFigure()

    $rect = New-Object System.Drawing.Rectangle(0, 0, $size, $size)
    $brush = New-Object System.Drawing.Drawing2D.LinearGradientBrush($rect,
        [System.Drawing.Color]::FromArgb(79, 140, 255),
        [System.Drawing.Color]::FromArgb(29, 78, 187), 55.0)
    $g.FillPath($brush, $path)

    $fontSize = [float]($size * ([math]::Pow($size / 256.0, 0.15)) * 0.56)
    $font = New-Object System.Drawing.Font('Segoe UI', $fontSize,
        [System.Drawing.FontStyle]::Bold, [System.Drawing.GraphicsUnit]::Pixel)
    $fmt = New-Object System.Drawing.StringFormat
    $fmt.Alignment = [System.Drawing.StringAlignment]::Center
    $fmt.LineAlignment = [System.Drawing.StringAlignment]::Center
    $tBox = New-Object System.Drawing.RectangleF(0, (-$size * 0.06), $size, $size)
    $white = [System.Drawing.Brushes]::White
    $g.DrawString('T', $font, $white, $tBox, $fmt)

    if ($size -ge 48) {
        $barW = $size * 0.09
        $baseY = $size * 0.86
        $maxBarH = $size * 0.13
        $heights = @(0.45, 0.7, 1.0)
        $gap = $size * 0.06
        $totalW = $barW * 3 + $gap * 2
        $startX = ($size - $totalW) / 2
        for ($i = 0; $i -lt 3; $i++) {
            $h = $maxBarH * $heights[$i]
            $x = $startX + $i * ($barW + $gap)
            $barRect = New-Object System.Drawing.RectangleF($x, ($baseY - $h), $barW, $h)
            $barPath = New-Object System.Drawing.Drawing2D.GraphicsPath
            $r = $barW / 2
            $barPath.AddArc($barRect.X, $barRect.Y, $r * 2, $r * 2, 180, 180)
            $barPath.AddArc($barRect.Right - $r * 2, $barRect.Y, $r * 2, $r * 2, 270, 180)
            $barPath.CloseFigure()
            $g.FillPath($white, $barPath)
            $barPath.Dispose()
        }
    }

    $g.Dispose()
    return $bmp
}

function ConvertTo-IcoImageBytes([System.Drawing.Bitmap]$bmp) {
    # Pure managed path: save BMP (BITMAPFILEHEADER + BITMAPINFOHEADER + BGRA
    # bottom-up), strip the 14-byte file header, patch biHeight to h*2
    # (XOR + AND mask), append an all-zero AND mask.
    $w = $bmp.Width
    $h = $bmp.Height
    $ms = New-Object System.IO.MemoryStream
    $bmp.Save($ms, [System.Drawing.Imaging.ImageFormat]::Bmp)
    $all = $ms.ToArray()
    $ms.Dispose()
    [byte[]]$dib = $all[14..($all.Length - 1)]
    $doubleHeight = $h * 2
    $dib[8] = [byte]($doubleHeight -band 0xFF)
    $dib[9] = [byte](($doubleHeight -shr 8) -band 0xFF)
    $dib[10] = [byte](($doubleHeight -shr 16) -band 0xFF)
    $dib[11] = [byte](($doubleHeight -shr 24) -band 0xFF)
    $andStride = [int][Math]::Ceiling($w / 32.0) * 4
    $andLength = $andStride * $h
    [byte[]]$out = New-Object byte[] ($dib.Length + $andLength)
    [Array]::Copy($dib, 0, $out, 0, $dib.Length)
    return ,$out
}

$sizes = @(16, 24, 32, 48, 64, 128, 256)
$images = @{}
foreach ($s in $sizes) { $images[$s] = New-IconBitmap $s }

$ico = New-Object System.IO.MemoryStream
$bw = New-Object System.IO.BinaryWriter($ico)
$bw.Write([uint16]0)
$bw.Write([uint16]1)
$bw.Write([uint16]$sizes.Count)
$offset = 6 + 16 * $sizes.Count
$encoded = @{}
foreach ($s in $sizes) {
    $encoded[$s] = ConvertTo-IcoImageBytes $images[$s]
}
foreach ($s in $sizes) {
    $bytes = $encoded[$s]
    if ($s -ge 256) { $b = 0 } else { $b = $s }
    $bw.Write([byte]$b)
    $bw.Write([byte]$b)
    $bw.Write([byte]0)
    $bw.Write([byte]0)
    $bw.Write([uint16]1)
    $bw.Write([uint16]32)
    $bw.Write([uint32]$bytes.Length)
    $bw.Write([uint32]$offset)
    $offset += $bytes.Length
}
foreach ($s in $sizes) {
    $bw.Write($encoded[$s])
}
$bw.Flush()
[System.IO.File]::WriteAllBytes($OutIco, $ico.ToArray())
$bw.Dispose()
$ico.Dispose()

$images[128].Save($OutPng, [System.Drawing.Imaging.ImageFormat]::Png)
foreach ($s in $sizes) { $images[$s].Dispose() }
Write-Host ("OK: " + $OutIco + " (" + (Get-Item $OutIco).Length + " bytes), " + $OutPng)
