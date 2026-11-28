#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")"

if [[ "$(uname -s)" != Darwin ]]; then
  echo "run-mac.sh supports macOS only" >&2
  exit 1
fi

if ! command -v rustup >/dev/null; then
  echo "rustup not found: install it, then rerun" >&2
  exit 1
fi

# Install the toolchain pinned in rust-toolchain.toml when missing.
rustup toolchain install --no-self-update

export BT_RUNTIME_DIR="$PWD/.runtime"
mkdir -p -m 700 "$BT_RUNTIME_DIR"

exec cargo run --locked --release -p blazing-telesram "$@"
