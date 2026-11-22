# Drives the Rid Uninstaller window through UI Automation, for end-to-end checks in a test VM.
# Steps (one per line in -StepsFile):
#   click|<text>      click the first element whose name contains <text> (or equals it: click|=<text>)
#   type|<text>       type text into the focused element
#   paste|<text>      replace the focused field's content with <text>
#   keys|<sendkeys>   send keys in SendKeys notation
#   waitfor|<text>    wait until an element whose name contains <text> exists (30 s)
#   gone|<text>       wait until no such element exists (120 s)
#   wait|<ms>
#   dump              write the names of all named elements to the log
param([Parameter(Mandatory)][string]$StepsFile, [Parameter(Mandatory)][string]$Log)

Add-Type -AssemblyName UIAutomationClient, UIAutomationTypes, System.Windows.Forms
Add-Type @'
using System;
using System.Runtime.InteropServices;
public static class RidGui {
    [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
    [DllImport("user32.dll")] public static extern void mouse_event(uint flags, uint dx, uint dy, uint data, UIntPtr extra);
    [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
    [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
}
'@
[void][RidGui]::SetProcessDPIAware()
$auto = [System.Windows.Automation.AutomationElement]
$scope = [System.Windows.Automation.TreeScope]
$all = [System.Windows.Automation.Condition]::TrueCondition

function Say($text) { Add-Content -Path $Log -Value $text -Encoding UTF8 }

function Window {
    $cond = New-Object System.Windows.Automation.PropertyCondition ($auto::NameProperty, 'Rid Uninstaller')
    $w = $auto::RootElement.FindFirst($scope::Children, $cond)
    if (-not $w) { throw 'Rid Uninstaller window not found' }
    $w
}

function Named {
    @((Window).FindAll($scope::Descendants, $all) | Where-Object { $_.Current.Name -and -not $_.Current.IsOffscreen })
}

function Find($text) {
    # A leading "=" asks for the exact name instead of a substring.
    if ($text.StartsWith('=')) { return Named | Where-Object { $_.Current.Name.Trim() -eq $text.Substring(1) } | Select-Object -First 1 }
    Named | Where-Object { $_.Current.Name.Contains($text) } | Select-Object -First 1
}

function WaitFor($text, $seconds, $wantPresent) {
    $until = (Get-Date).AddSeconds($seconds)
    do {
        $e = Find $text
        if ([bool]$e -eq $wantPresent) { return $e }
        Start-Sleep -Milliseconds 400
    } while ((Get-Date) -lt $until)
    throw "timed out waiting for '$text' to $(if ($wantPresent) { 'appear' } else { 'disappear' })"
}

Set-Content -Path $Log -Value "started $(Get-Date -Format o)" -Encoding UTF8
try {
    [void][RidGui]::SetForegroundWindow([IntPtr](Window).Current.NativeWindowHandle)
    Start-Sleep -Milliseconds 500
    foreach ($line in Get-Content -Path $StepsFile -Encoding UTF8) {
        if (-not $line.Trim()) { continue }
        $verb, $arg = $line.Split('|', 2)
        switch ($verb) {
            'click' {
                $e = WaitFor $arg 30 $true
                $r = $e.Current.BoundingRectangle
                [void][RidGui]::SetCursorPos([int]($r.X + $r.Width / 2), [int]($r.Y + $r.Height / 2))
                Start-Sleep -Milliseconds 150
                [RidGui]::mouse_event(2, 0, 0, 0, [UIntPtr]::Zero)
                [RidGui]::mouse_event(4, 0, 0, 0, [UIntPtr]::Zero)
                Start-Sleep -Milliseconds 400
            }
            'type' { [System.Windows.Forms.SendKeys]::SendWait(($arg -replace '([+^%~(){}\[\]])', '{$1}')); Start-Sleep -Milliseconds 500 }
            # Through the clipboard, so an active input method editor cannot rewrite the text.
            'paste' { [System.Windows.Forms.Clipboard]::SetText($arg); [System.Windows.Forms.SendKeys]::SendWait('^a^v'); Start-Sleep -Milliseconds 500 }
            'keys' { [System.Windows.Forms.SendKeys]::SendWait($arg); Start-Sleep -Milliseconds 500 }
            'waitfor' { [void](WaitFor $arg 30 $true) }
            'gone' { [void](WaitFor $arg 120 $false) }
            'wait' { Start-Sleep -Milliseconds ([int]$arg) }
            'dump' { Named | ForEach-Object { Say ("  [{0}] {1}" -f $_.Current.LocalizedControlType, $_.Current.Name) } }
            default { throw "unknown step '$verb'" }
        }
        Say "ok   $line"
    }
    Say 'DONE'
} catch {
    Say "FAIL $($_.Exception.Message)"
}
