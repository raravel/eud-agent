#!/usr/bin/env bash
# Run the Tauri app in dev mode (Rust core + panel dev server) on macOS/Unix.
#
# Mirror of scripts/dev_run.ps1: dev = `cargo tauri dev`, which builds the Rust
# core, links the isom static lib, and serves the React panel with hot-reload in
# the app's own WebView window. src-tauri/tauri.macos.conf.json is merged
# automatically on macOS.
#
# Requires the Tauri CLI (`cargo install tauri-cli --locked`) and panel
# dependencies (`npm ci` in panel/). AI provider binaries and credentials are
# selected, installed, and gated inside the app.
#
# Usage: scripts/dev_run.sh [extra `cargo tauri dev` args, e.g. -- --release]
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

fail() {
    echo "ERROR: $*" >&2
    exit 1
}

command -v cargo >/dev/null 2>&1 \
    || fail "cargo not found on PATH. Install the Rust toolchain (https://rustup.rs or 'brew install rust')."
cargo tauri --version >/dev/null 2>&1 \
    || fail "Tauri CLI not found. Install it with: cargo install tauri-cli --locked"
command -v npm >/dev/null 2>&1 \
    || fail "npm not found on PATH. Install Node.js (e.g. 'brew install node')."
[[ -d "$repo_root/panel/node_modules" ]] \
    || fail "panel dependencies are missing. Run: (cd panel && npm ci)"

echo "running: cargo tauri dev (cwd=$repo_root)"

# `cargo tauri dev` discovers src-tauri/tauri.conf.json from the repo root. Pin the
# frontend dir: its package.json auto-discovery follows directory order, which on
# APFS can pick site/ or tools/ before panel/ (`npm run dev` then fails).
cd "$repo_root"
export TAURI_FRONTEND_PATH="$repo_root/panel"
exec cargo tauri dev "$@"
