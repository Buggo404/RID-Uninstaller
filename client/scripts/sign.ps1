# Signs Rid's binaries with a code-signing certificate held on a hardware token.
# Run this on the machine the token is plugged into; signtool will ask for the token password.
#
#   scripts\sign.ps1 -Thumbprint <SHA-1 thumbprint of the certificate>
#
# The thumbprint is shown by:  Get-ChildItem Cert:\CurrentUser\My -CodeSigningCert
# Order matters: the files inside the installer must be signed BEFORE the installer is
# built, so the flow is  build.ps1 (once, to produce the binaries)  ->  sign.ps1 -Stage binaries
# ->  pnpm tauri build (repackages the signed files)  ->  sign.ps1 -Stage installer.
#
# The installer stage also writes <installer>.sig with the updater key. Clients only accept an
# update whose .sig matches the installer byte for byte, so it is made after the code signature;
# its contents go into the release on the server's admin page.
#
# This does NOT make the driver loadable on ordinary machines: kernel drivers must be signed
# by Microsoft through the Hardware Dev Center. See scripts\driver-package.ps1.
param(
    [Parameter(Mandatory)][string]$Thumbprint,
    [ValidateSet('binaries', 'installer')][string]$Stage = 'binaries',
    [string]$TimestampUrl = 'http://timestamp.globalsign.com/tsa/r6advanced1',
    [string]$UpdaterKey = "$HOME\.tauri\rid-updater.key"
)
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent

$signtool = Get-ChildItem "${env:ProgramFiles(x86)}\Windows Kits\10\bin\*\x64\signtool.exe" -ErrorAction SilentlyContinue |
    Sort-Object FullName | Select-Object -Last 1
if (-not $signtool) { throw 'signtool.exe not found; install the Windows SDK' }

$files = if ($Stage -eq 'binaries') {
    "$root\target\release\rid-app.exe", "$root\target\release\rid.exe", "$root\driver\rid-monitor\target\release\rid_monitor.dll"
} else {
    (Get-ChildItem "$root\target\release\bundle\nsis\*.exe").FullName
}
foreach ($file in $files) {
    if (-not (Test-Path $file)) { throw "not built yet: $file" }
    & $signtool.FullName sign /sha1 $Thumbprint /fd SHA256 /tr $TimestampUrl /td SHA256 /d 'Rid Uninstaller' $file
    if ($LASTEXITCODE -ne 0) { throw "signing failed: $file" }
    & $signtool.FullName verify /pa $file
    if ($LASTEXITCODE -ne 0) { throw "signature does not verify: $file" }
}
if ($Stage -eq 'installer') {
    $env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD = ''
    Push-Location "$root\crates\rid-app"
    try {
        foreach ($file in $files) {
            pnpm tauri signer sign -f $UpdaterKey $file | Out-Null
            if ($LASTEXITCODE -ne 0) { throw "updater signature failed: $file" }
            "update signature: $file.sig"
        }
    } finally { Pop-Location }
}
"signed $($files.Count) file(s)"
