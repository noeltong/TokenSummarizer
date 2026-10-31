# capture-screens.ps1
# Disabled by default: screenshots may contain hosts, usernames, and usage data.
# This script intentionally fails closed and never writes screenshot files.
# Drives the TokenSummarizer popup via WM_APP messages and captures every screen
# (total / local / each server tab + settings / aliases / add-server dialogs).
#
# Usage:
#   powershell -NoProfile -ExecutionPolicy Bypass -File tools\capture-screens.ps1
#   ... -Exe target\release\token_summarizer.exe -OutDir screenshots -KeepRunning
#
# Notes:
#  * Windows are located by class name via EnumWindows (FindWindow+null title is
#    unreliable from PowerShell).
#  * Bitmaps are Format24bppRgb: a 32bppArgb bitmap holding GDI content has
#    alpha=0 and saves as a fully transparent (appears white) PNG.
# Exit codes: 0 ok, 2 a capture came out blank, 3 window not found.

param(
    [string]$Exe = "",
    [string]$OutDir = "",
    [int]$SettleMs = 700,
    [int]$WarmupSeconds = 0,
    [switch]$KeepRunning
)

$ErrorActionPreference = "Stop"
Write-Error "Screenshot capture is permanently disabled because captures may contain hosts, usernames, and token usage. Use a separately reviewed redacted workflow."
exit 4
$root = Split-Path -Parent (Split-Path -Parent $MyInvocation.MyCommand.Path)
if (-not $OutDir) { $OutDir = Join-Path $root "screenshots" }
if (-not $Exe) {
    $release = Join-Path $root "target\release\token_summarizer.exe"
    $debug = Join-Path $root "target\debug\token_summarizer.exe"
    if (Test-Path $release) { $Exe = $release } else { $Exe = $debug }
}
if (-not (Test-Path $Exe)) { Write-Host "exe not found: $Exe"; exit 3 }
if (-not (Test-Path $OutDir)) { New-Item -ItemType Directory -Path $OutDir | Out-Null }

Add-Type -AssemblyName System.Drawing

if (-not ("ShotNative" -as [type])) {
    Add-Type @"
using System;
using System.Text;
using System.Runtime.InteropServices;
public struct RECT { public int Left, Top, Right, Bottom; }
public static class ShotNative {
    public delegate bool EnumProc(IntPtr h, IntPtr l);
    [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr l);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetClassName(IntPtr h, StringBuilder sb, int max);
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
    [DllImport("user32.dll")] public static extern bool IsWindow(IntPtr h);
    [DllImport("user32.dll")] public static extern IntPtr SendMessage(IntPtr h, uint msg, IntPtr w, IntPtr l);
    [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr h, uint msg, IntPtr w, IntPtr l);
    [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr h, IntPtr hdc, uint flags);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
    [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
    [DllImport("user32.dll")] public static extern bool SetProcessDpiAwarenessContext(IntPtr ctx);
    [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
}
"@
}

if (-not [ShotNative]::SetProcessDpiAwarenessContext([IntPtr](-4))) { [void][ShotNative]::SetProcessDPIAware() }

$WM_CLOSE = 0x0010
$WM_APP_SHOW = 0x8003
$WM_APP_OPEN_ADD = 0x8009
$WM_APP_OPEN_ALIASES = 0x800A
$WM_APP_OPEN_SETTINGS = 0x800B
$WM_APP_SELECT_TAB = 0x800C
$PW_RENDERFULLCONTENT = 2

$CLS_MAIN = "TokenSummarizerPopupWnd"
$CLS_SETTINGS = "TokenSummarizerSettingsWnd"
$CLS_ALIAS = "TokenSummarizerAliasWnd"
$CLS_EDIT = "TokenSummarizerEditWnd"

function Find-Class([string]$cls) {
    $script:hit = [IntPtr]::Zero
    $cb = [ShotNative+EnumProc]{
        param($w, $l)
        $sb = New-Object System.Text.StringBuilder 256
        [void][ShotNative]::GetClassName($w, $sb, 256)
        if ($sb.ToString() -eq $cls) { $script:hit = $w; return $false }
        return $true
    }
    [void][ShotNative]::EnumWindows($cb, [IntPtr]::Zero)
    return $script:hit
}

function Send-Msg([IntPtr]$hwnd, [uint32]$msg, [int]$wparam) {
    [void][ShotNative]::SendMessage($hwnd, $msg, [IntPtr]$wparam, [IntPtr]::Zero)
}

function Wait-Window([string]$cls, [int]$timeoutMs) {
    $deadline = (Get-Date).AddMilliseconds($timeoutMs)
    while ((Get-Date) -lt $deadline) {
        $h = Find-Class $cls
        if ($h -ne [IntPtr]::Zero -and [ShotNative]::IsWindow($h)) { return $h }
        Start-Sleep -Milliseconds 120
    }
    return [IntPtr]::Zero
}

function Test-Blank($bmp) {
    $rect = New-Object System.Drawing.Rectangle(0, 0, $bmp.Width, $bmp.Height)
    $data = $bmp.LockBits($rect, [System.Drawing.Imaging.ImageLockMode]::ReadOnly, [System.Drawing.Imaging.PixelFormat]::Format24bppRgb)
    try {
        $bytes = New-Object byte[] ($data.Stride * $data.Height)
        [System.Runtime.InteropServices.Marshal]::Copy($data.Scan0, $bytes, 0, $bytes.Length)
        $stride = $data.Stride
        $interesting = 0
        $sampled = 0
        for ($y = 0; $y -lt $bmp.Height; $y += 3) {
            $row = $y * $stride
            for ($x = 0; $x -lt $bmp.Width; $x += 3) {
                $i = $row + $x * 3
                $b = $bytes[$i]; $g = $bytes[$i + 1]; $r = $bytes[$i + 2]
                $sampled++
                $max = [Math]::Max($r, [Math]::Max($g, $b))
                $min = [Math]::Min($r, [Math]::Min($g, $b))
                if (($max - $min) -gt 6 -or $min -lt 200) { $interesting++ }
            }
        }
        if ($sampled -eq 0) { return $true }
        return (($interesting / $sampled) -lt 0.02)
    } finally {
        $bmp.UnlockBits($data)
    }
}

function Ensure-Visible([IntPtr]$hwnd) {
    if ([ShotNative]::IsWindowVisible($hwnd)) { return $true }
    Send-Msg $hwnd $WM_APP_SHOW 0
    Start-Sleep -Milliseconds 400
    return [ShotNative]::IsWindowVisible($hwnd)
}

function Save-Window([IntPtr]$hwnd, [string]$name) {
    if (-not (Ensure-Visible $hwnd)) { Write-Host "$name : window hidden"; return $false }
    $r = New-Object RECT
    [void][ShotNative]::GetWindowRect($hwnd, [ref]$r)
    $w = $r.Right - $r.Left
    $h = $r.Bottom - $r.Top
    if ($w -le 0 -or $h -le 0) { Write-Host "$name : bad rect"; return $false }
    $path = Join-Path $OutDir $name
    $bmp = New-Object System.Drawing.Bitmap($w, $h, [System.Drawing.Imaging.PixelFormat]::Format24bppRgb)
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $hdc = $g.GetHdc()
    $printed = [ShotNative]::PrintWindow($hwnd, $hdc, $PW_RENDERFULLCONTENT)
    $g.ReleaseHdc($hdc)
    $g.Dispose()
    $blank = $true
    if ($printed) { $blank = Test-Blank $bmp }
    if ($printed -and -not $blank) {
        $bmp.Save($path, [System.Drawing.Imaging.ImageFormat]::Png)
    } else {
        # Fallback: foreground + screen copy
        $bmp.Dispose()
        [void][ShotNative]::SetForegroundWindow($hwnd)
        Start-Sleep -Milliseconds 300
        $bmp = New-Object System.Drawing.Bitmap($w, $h, [System.Drawing.Imaging.PixelFormat]::Format24bppRgb)
        $g = [System.Drawing.Graphics]::FromImage($bmp)
        $g.CopyFromScreen($r.Left, $r.Top, 0, 0, (New-Object System.Drawing.Size($w, $h)))
        $g.Dispose()
        $bmp.Save($path, [System.Drawing.Imaging.ImageFormat]::Png)
        $blank = Test-Blank $bmp
    }
    $size = (Get-Item $path).Length
    $bmp.Dispose()
    $state = if ($blank) { "BLANK" } else { "ok" }
    Write-Host ("{0,-20} {1,4}x{2,-4} {3,9} bytes  {4}" -f $name, $w, $h, $size, $state)
    return (-not $blank)
}

# --- start app ---------------------------------------------------------------
$started = $false
$hwnd = Find-Class $CLS_MAIN
if ($hwnd -eq [IntPtr]::Zero) {
    Write-Host "starting $Exe"
    $proc = Start-Process -FilePath $Exe -PassThru
    $started = $true
    $hwnd = Wait-Window $CLS_MAIN 20000
    if ($hwnd -eq [IntPtr]::Zero) {
        Write-Host "main window not found (process $($proc.Id))"
        Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue
        exit 3
    }
} else {
    Write-Host "reusing running instance"
}
Start-Sleep -Milliseconds 500

Get-ChildItem -Path $OutDir -Filter "main-*.png" -ErrorAction SilentlyContinue | Remove-Item -Force

# --- make the popup visible (WM_APP_SHOW is a toggle) -------------------------
if (-not [ShotNative]::IsWindowVisible($hwnd)) {
    Send-Msg $hwnd $WM_APP_SHOW 0
    Start-Sleep -Milliseconds $SettleMs
}
[void][ShotNative]::SetForegroundWindow($hwnd)
Start-Sleep -Milliseconds 300
Write-Host "main visible=$([ShotNative]::IsWindowVisible($hwnd))"

$cfgPath = Join-Path $env:APPDATA "TokenSummarizer\config.json"
$serverCount = 0
if (Test-Path $cfgPath) {
    try {
        $cfg = Get-Content -Raw -Path $cfgPath | ConvertFrom-Json
        if ($cfg.servers) { $serverCount = @($cfg.servers).Count }
    } catch { Write-Host "config.json unreadable, assuming 0 servers" }
}
$tabCount = $serverCount + 2
Write-Host "servers=$serverCount tabs=$tabCount"

if ($WarmupSeconds -gt 0) {
    Write-Host "warming up ${WarmupSeconds}s (let servers finish first sync)"
    for ($i = 0; $i -lt $WarmupSeconds; $i++) {
        Start-Sleep -Seconds 1
        # 气泡面板失焦即自动隐藏，预热期间要保持它可见
        if (-not [ShotNative]::IsWindowVisible($hwnd)) {
            [void][ShotNative]::SetForegroundWindow($hwnd)
            [void](Ensure-Visible $hwnd)
        }
    }
}

$failures = 0
for ($tab = 0; $tab -lt $tabCount; $tab++) {
    [void](Ensure-Visible $hwnd)
    Send-Msg $hwnd $WM_APP_SELECT_TAB $tab
    Start-Sleep -Milliseconds $SettleMs
    $name = if ($tab -eq 0) { "main-total.png" } elseif ($tab -eq 1) { "main-local.png" } else { "main-server$($tab - 1).png" }
    if (-not (Save-Window $hwnd $name)) { $failures++ }
}

function Capture-Dialog([uint32]$msg, [string]$cls, [string]$file) {
    Send-Msg $hwnd $msg 0
    $dlg = Wait-Window $cls 6000
    if ($dlg -eq [IntPtr]::Zero) { Write-Host "dialog not found: $cls"; return $false }
    Start-Sleep -Milliseconds $SettleMs
    $ok = Save-Window $dlg $file
    [void][ShotNative]::PostMessage($dlg, $WM_CLOSE, [IntPtr]::Zero, [IntPtr]::Zero)
    Start-Sleep -Milliseconds 400
    return $ok
}

if (-not (Capture-Dialog $WM_APP_OPEN_SETTINGS $CLS_SETTINGS "settings.png")) { $failures++ }
if (-not (Capture-Dialog $WM_APP_OPEN_ALIASES $CLS_ALIAS "aliases.png")) { $failures++ }
if (-not (Capture-Dialog $WM_APP_OPEN_ADD $CLS_EDIT "server-edit.png")) { $failures++ }

# --- cleanup -----------------------------------------------------------------
if ($started -and -not $KeepRunning) {
    if ([ShotNative]::IsWindowVisible($hwnd)) { Send-Msg $hwnd $WM_APP_SHOW 0 }
    Start-Sleep -Milliseconds 300
    Get-Process -Name "token_summarizer" -ErrorAction SilentlyContinue | Stop-Process -Force
}

if ($failures -gt 0) {
    Write-Host "$failures capture(s) blank or missing"
    exit 2
}
Write-Host "all captures ok"
exit 0
