# Helpers for driving the local Rid window in checks: dot-source, then use Rid-* functions.
Add-Type -AssemblyName UIAutomationClient, UIAutomationTypes, System.Windows.Forms
if (-not ('RidUi' -as [type])) {
    Add-Type @'
using System; using System.Text; using System.Runtime.InteropServices;
public static class RidUi {
    [StructLayout(LayoutKind.Sequential)] public struct POINT { public int X, Y; }
    [DllImport("user32.dll")] static extern IntPtr WindowFromPoint(POINT p);
    [DllImport("user32.dll")] static extern IntPtr GetAncestor(IntPtr h, uint f);
    [DllImport("user32.dll")] static extern void keybd_event(byte vk, byte scan, uint flags, UIntPtr extra);
    public static IntPtr RootAt(int x, int y) { return GetAncestor(WindowFromPoint(new POINT { X = x, Y = y }), 2); }
    // Windows only lets a process take the foreground after input; a tap of Alt counts as such.
    public static void Front(IntPtr h) { keybd_event(0x12, 0, 0, UIntPtr.Zero); keybd_event(0x12, 0, 2, UIntPtr.Zero); SetForegroundWindow(h); }
    public delegate bool Each(IntPtr h, IntPtr l);
    [DllImport("user32.dll")] public static extern bool EnumWindows(Each cb, IntPtr l);
    [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetClassName(IntPtr h, StringBuilder s, int n);
    // The program's own window: a process has helper windows too, and MainWindowHandle may name one of those.
    public static IntPtr Window(uint pid) { IntPtr found = IntPtr.Zero; EnumWindows((h, l) => { uint p; GetWindowThreadProcessId(h, out p); var c = new StringBuilder(64); GetClassName(h, c, 64); if (p == pid && c.ToString() == "Tauri Window") { found = h; return false; } return true; }, IntPtr.Zero); return found; }
    [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L, T, R, B; }
    [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
    [DllImport("user32.dll")] public static extern void mouse_event(uint f, uint dx, uint dy, uint d, UIntPtr e);
    [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
    [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
    [DllImport("user32.dll")] public static extern bool MoveWindow(IntPtr h, int x, int y, int w, int height, bool repaint);
    [DllImport("user32.dll")] public static extern bool IsIconic(IntPtr h);
    [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h, int cmd);
}
'@
}
$script:auto = [System.Windows.Automation.AutomationElement]
$script:scope = [System.Windows.Automation.TreeScope]::Descendants
$script:shots = "$env:TEMP\rid-shots"

function Rid-Proc { Get-Process rid-app -ErrorAction SilentlyContinue | Where-Object { $_.MainWindowHandle -ne [IntPtr]::Zero } | Select-Object -First 1 }
function Rid-Start { $p = Start-Process "D:\Project_Code\Rid\client\target\release\rid-app.exe" -PassThru; foreach ($i in 1..60) { $p.Refresh(); if ($p.MainWindowHandle -ne [IntPtr]::Zero) { break }; Start-Sleep -Milliseconds 100 }; Start-Sleep -Milliseconds 2500; $p }
function Rid-Front { $p = Rid-Proc; [void][RidUi]::SetForegroundWindow($p.MainWindowHandle); Start-Sleep -Milliseconds 400 }
function Rid-Rect { $r = New-Object RidUi+RECT; [void][RidUi]::GetWindowRect((Rid-Proc).MainWindowHandle, [ref]$r); $r }
function Rid-Shot($name) { Rid-Front; & "D:\Project_Code\Rid\client\scripts\shot.ps1" -ProcessId (Rid-Proc).Id -Out "$shots\$name.png" | Out-Null }
function Rid-Find($name) { $w = $auto::FromHandle((Rid-Proc).MainWindowHandle); @($w.FindAll($scope, [System.Windows.Automation.Condition]::TrueCondition) | Where-Object { $_.Current.Name -like $name })[0] }
# Set $RidTarget to a window handle and clicks are refused unless they would land on that window.
function Rid-Mouse($x, $y, [switch]$Right) { if ($x -le 0 -or $y -le 0) { throw "refusing to click at $x,$y" }; if ($RidTarget -and [RidUi]::RootAt($x, $y) -ne $RidTarget) { throw "another window is in the way at $x,$y" }; [void][RidUi]::SetCursorPos($x, $y); Start-Sleep -Milliseconds 80; if ($Right) { [RidUi]::mouse_event(8, 0, 0, 0, [UIntPtr]::Zero); [RidUi]::mouse_event(16, 0, 0, 0, [UIntPtr]::Zero) } else { [RidUi]::mouse_event(2, 0, 0, 0, [UIntPtr]::Zero); [RidUi]::mouse_event(4, 0, 0, 0, [UIntPtr]::Zero) }; Start-Sleep -Milliseconds 350 }
function Rid-Click($name, [switch]$Right, $dx = 0) { $e = Rid-Find $name; if (-not $e) { "NOT FOUND: $name"; return }; $r = $e.Current.BoundingRectangle; Rid-Mouse ([int]($r.X + $r.Width / 2 + $dx)) ([int]($r.Y + $r.Height / 2)) -Right:$Right }
function Rid-Drag($x1, $y1, $x2, $y2) { [void][RidUi]::SetCursorPos($x1, $y1); Start-Sleep -Milliseconds 80; [RidUi]::mouse_event(2, 0, 0, 0, [UIntPtr]::Zero); foreach ($i in 1..10) { [void][RidUi]::SetCursorPos([int]($x1 + ($x2 - $x1) * $i / 10), [int]($y1 + ($y2 - $y1) * $i / 10)); Start-Sleep -Milliseconds 25 }; [RidUi]::mouse_event(4, 0, 0, 0, [UIntPtr]::Zero); Start-Sleep -Milliseconds 350 }
