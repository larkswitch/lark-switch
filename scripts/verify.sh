#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."

cargo fmt --all -- --check
cargo clippy -p lpc-core -p lpc-shim -p lpcctl --all-targets -- -D warnings
cargo test -p lpc-core -p lpc-shim -p lpcctl
(
  cd apps/desktop
  pnpm install --frozen-lockfile
  pnpm run build
)

echo "Local deterministic verification passed."
echo "Cross-platform Tauri bundles and real OAuth gates run in GitHub Actions/manual E2E."
