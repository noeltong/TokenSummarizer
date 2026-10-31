# Measure ink bounding boxes of tab labels in a screenshot.
# Prints top/bottom/center (physical px) of dark pixels per x-window, to
# diagnose vertical alignment of GDI+ DrawString output.
# Usage: powershell -File tools\measure-tab-ink.ps1 -InFile shots\main-total.png
param(
    [string]$InFile = "shots\main-total.png",
    [string]$Windows = "0-260,300-520,600-880,1000-1180",
    [int]$YTop = 8,
    [int]$YBottom = 60,
    [int]$Scale = 1
)
$ErrorActionPreference = "Stop"
Add-Type -AssemblyName System.Drawing
$src = [System.Drawing.Bitmap]::FromFile((Join-Path (Get-Location) $InFile))
$wins = $Windows.Split(",") | ForEach-Object {
    $p = $_.Split("-")
    [PSCustomObject]@{ X0 = [int]$p[0]; X1 = [int]$p[1] }
}
foreach ($w in $wins) {
    $top = -1; $bot = -1
    for ($y = $YTop; $y -lt $YBottom; $y++) {
        for ($x = $w.X0; $x -lt $w.X1; $x += $Scale) {
            $c = $src.GetPixel($x, $y)
            $sum = [int]$c.R + [int]$c.G + [int]$c.B
            if ($sum -lt 480) {
                if ($top -lt 0) { $top = $y }
                $bot = $y
                break
            }
        }
    }
    $center = if ($top -ge 0) { ($top + $bot) / 2.0 } else { -1 }
    Write-Host ("win {0,4}-{1,4}: top={2} bottom={3} center={4:F1} ink_h={5}" -f $w.X0, $w.X1, $top, $bot, $center, ($bot - $top + 1))
}
$src.Dispose()
