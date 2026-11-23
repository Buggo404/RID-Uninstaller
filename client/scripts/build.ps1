# Builds the installer:  target\release\bundle\nsis\Rid Uninstaller_<version>_x64-setup.exe
#
# Needs: Rust (MSVC toolchain), Node + pnpm. The files come out UNSIGNED; scripts\sign.ps1 signs
# them on the machine that has the code-signing token and writes the installer's .sig, which is
# what a release needs for clients to accept it as an update.
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
    # Before the program: the installer carries rid.exe.
    Step 'command line' { cargo build --release -p rid-cli }
    Push-Location crates\rid-app
    try {
        Step 'front-end packages' { pnpm install --frozen-lockfile }
        Step 'application and installer' { pnpm tauri build }
    } finally { Pop-Location }
    Get-ChildItem target\release\bundle\nsis\*.exe | ForEach-Object { "installer: $($_.FullName)" }
} finally { Pop-Location }
