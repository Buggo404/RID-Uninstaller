# Captures one window of a process to a PNG (the window only, not the desktop).
param([Parameter(Mandatory)][int]$ProcessId, [Parameter(Mandatory)][string]$Out)

Add-Type -AssemblyName System.Drawing
Add-Type @'
using System;
using System.Runtime.InteropServices;
public static class RidShot {
    [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L, T, R, B; }
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
    [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr h, IntPtr dc, uint flags);
    [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
}
'@
[void][RidShot]::SetProcessDPIAware()

$hwnd = (Get-Process -Id $ProcessId).MainWindowHandle
if ($hwnd -eq [IntPtr]::Zero) { throw "process $ProcessId has no main window" }
$r = New-Object RidShot+RECT
[void][RidShot]::GetWindowRect($hwnd, [ref]$r)
$bmp = New-Object System.Drawing.Bitmap ($r.R - $r.L), ($r.B - $r.T)
$g = [System.Drawing.Graphics]::FromImage($bmp)
$dc = $g.GetHdc()
# 2 = PW_RENDERFULLCONTENT, needed for WebView2 surfaces
$ok = [RidShot]::PrintWindow($hwnd, $dc, 2)
$g.ReleaseHdc($dc); $g.Dispose()
$bmp.Save($Out, [System.Drawing.Imaging.ImageFormat]::Png); $bmp.Dispose()
if (-not $ok) { throw 'PrintWindow failed' }
"$Out"
