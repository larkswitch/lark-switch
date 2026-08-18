$ErrorActionPreference = "Stop"
$PSNativeCommandUseErrorActionPreference = $true
Set-Location (Join-Path $PSScriptRoot "..")

function Assert-NativeSuccess([string]$Step) {
    if ($LASTEXITCODE -ne 0) {
        throw "$Step failed with exit code $LASTEXITCODE"
    }
}

cargo fmt --all -- --check
Assert-NativeSuccess "cargo fmt"
cargo clippy -p lpc-core -p lpc-shim -p lpcctl --all-targets -- -D warnings
Assert-NativeSuccess "cargo clippy"
cargo test -p lpc-core -p lpc-shim -p lpcctl
Assert-NativeSuccess "cargo test"
Push-Location apps/desktop
pnpm install --frozen-lockfile
Assert-NativeSuccess "pnpm install"
pnpm run build
Assert-NativeSuccess "desktop build"
Pop-Location
Write-Host "Local deterministic verification passed."
