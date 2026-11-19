#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")"

if [[ "$(uname -s)" != Darwin ]]; then
  echo "run.sh supports macOS only" >&2
  exit 1
fi

if ! command -v rustup >/dev/null; then
  echo "rustup not found: install it, then rerun" >&2
  exit 1
fi

# Install the toolchain pinned in rust-toolchain.toml when missing.
rustup toolchain install --no-self-update

exec cargo run --locked --release -p blazing-telesram "$@"
