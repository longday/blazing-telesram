$ErrorActionPreference = 'Stop'

Set-Location -LiteralPath $PSScriptRoot

if (-not (Get-Command rustup -ErrorAction SilentlyContinue)) {
    [Console]::Error.WriteLine('rustup not found: install it, then rerun')
    exit 1
}

# Install the toolchain pinned in rust-toolchain.toml when missing.
rustup toolchain install --no-self-update
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

$env:BT_RUNTIME_DIR = Join-Path $PSScriptRoot '.runtime'
New-Item -ItemType Directory -Force -Path $env:BT_RUNTIME_DIR | Out-Null

cargo run --locked --release -p blazing-telesram @args
exit $LASTEXITCODE
