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

# Owner-only access (the equivalent of mode 700): drop inherited ACEs, grant the current user's SID only.
$sid = [System.Security.Principal.WindowsIdentity]::GetCurrent().User.Value
icacls $env:BT_RUNTIME_DIR /inheritance:r /grant:r "*${sid}:(OI)(CI)F" | Out-Null
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

cargo run --locked --release -p blazing-telesram @args
exit $LASTEXITCODE
