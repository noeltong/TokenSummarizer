# Find the first column whose color deviates from the local background,
# to verify tab pill / panel card left-edge alignment.
param(
    [string]$InFile = "shots\main-total.png",
    [string]$Rows = "23,100,120,200,400,500,600",
    [int]$MaxX = 60,
    [int]$Delta = 30
)
$ErrorActionPreference = "Stop"
Add-Type -AssemblyName System.Drawing
$src = [System.Drawing.Bitmap]::FromFile((Join-Path (Get-Location) $InFile))
foreach ($yStr in $Rows.Split(",")) {
    $y = [int]$yStr
    $bg = $src.GetPixel(2, $y)
    $found = -1
    for ($x = 4; $x -lt $MaxX; $x++) {
        $c = $src.GetPixel($x, $y)
        $d = [Math]::Abs([int]$c.R - [int]$bg.R) + [Math]::Abs([int]$c.G - [int]$bg.G) + [Math]::Abs([int]$c.B - [int]$bg.B)
        if ($d -gt $Delta) { $found = $x; break }
    }
    Write-Host ("y={0,4}: first non-background at x={1}" -f $y, $found)
}
$src.Dispose()
