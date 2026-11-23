# Builds the program: one file, target\release\rid-app.exe, plus the command-line tool rid.exe.
#
#   scripts\build.ps1                 the program without the tracing driver inside
#   scripts\build.ps1 -Driver <file>  with the Microsoft-signed RidMonitor.sys inside
#   scripts\build.ps1 -BuildDriver    also compiles the driver (needs the Windows Driver Kit and LLVM)
#
# Needs: Rust (MSVC toolchain), Node + pnpm. Set RID_SERVER to the address of the server a
# release should talk to. The binaries come out UNSIGNED; run scripts\sign.ps1 afterwards on
# the machine that has the code-signing token. The driver is signed by Microsoft, not by us
# (see scripts\driver-package.ps1).
param([string]$Driver, [switch]$BuildDriver)
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent

function Step($name, [scriptblock]$run) {
    Write-Host "== $name"
    & $run
    if ($LASTEXITCODE -ne 0) { throw "$name failed (exit code $LASTEXITCODE)" }
}

Push-Location $root
try {
    Step 'tests' { cargo test -p rid-core }
    if ($BuildDriver) {
        Step 'driver' { Push-Location driver\rid-monitor; try { cargo build --release } finally { Pop-Location } }
    }
    Step 'command line' { cargo build --release -p rid-cli }
    if ($Driver) { $env:RID_DRIVER = (Resolve-Path $Driver).Path }
    Push-Location crates\rid-app
    try {
        Step 'front-end packages' { pnpm install --frozen-lockfile }
        Step 'application' { pnpm tauri build }
    } finally { Pop-Location; $env:RID_DRIVER = $null }
    "program:      $root\target\release\rid-app.exe"
    "command line: $root\target\release\rid.exe"
} finally { Pop-Location }
