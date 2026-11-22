# Builds everything and produces the installer.
#   target\release\bundle\nsis\Rid Uninstaller_<version>_x64-setup.exe
#
# Needs: Rust (MSVC toolchain), Node + pnpm, the Windows Driver Kit and LLVM (for the driver).
# The binaries come out UNSIGNED; run scripts\sign.ps1 afterwards on the machine that has
# the code-signing token. The driver additionally has to be signed by Microsoft before it
# loads on machines without test signing (see scripts\driver-package.ps1).
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
    Step 'driver' { Push-Location driver\rid-monitor; try { cargo build --release } finally { Pop-Location } }
    Step 'command line' { cargo build --release -p rid-cli }
    Push-Location crates\rid-app
    try {
        Step 'front-end packages' { pnpm install --frozen-lockfile }
        Step 'application and installer' { pnpm tauri build }
    } finally { Pop-Location }
    Get-ChildItem target\release\bundle\nsis\*.exe | ForEach-Object { "installer: $($_.FullName)" }
} finally { Pop-Location }
