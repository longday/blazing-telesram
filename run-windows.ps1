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

# Owner-only access (the equivalent of mode 700): replace the DACL with one protected rule for the current user's SID,
# so neither inherited nor explicit entries of other accounts survive on an existing directory.
$sid = [System.Security.Principal.WindowsIdentity]::GetCurrent().User
$acl = New-Object System.Security.AccessControl.DirectorySecurity
$acl.SetAccessRuleProtection($true, $false)
$acl.AddAccessRule((New-Object System.Security.AccessControl.FileSystemAccessRule($sid, 'FullControl', 'ContainerInherit,ObjectInherit', 'None', 'Allow')))
Set-Acl -LiteralPath $env:BT_RUNTIME_DIR -AclObject $acl

cargo run --locked --release -p blazing-telesram @args
exit $LASTEXITCODE
