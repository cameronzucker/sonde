#!/usr/bin/env bash
# Build the demo WASM bundle into demo/site/pkg/ (sonde_wasm.js + _bg.wasm).
#
# Single source of truth for the wasm-bundle step, called by BOTH:
#   - demo/build-assets.sh (step 3, local full asset build), and
#   - .github/workflows/pages.yml (the GitHub Pages deploy).
#
# Assumes on PATH / installed:
#   - the wasm32-unknown-unknown rust target, and
#   - a `wasm-bindgen` CLI whose version MATCHES the wasm-bindgen crate version
#     (mismatched CLI vs crate is the classic wasm-bindgen failure — the Pages
#     workflow pins the CLI to the resolved crate version; locally:
#     `cargo install wasm-bindgen-cli --version <crate-version>`).
set -euo pipefail
cd "$(dirname "$0")/.."                      # repo / worktree root
PKG=demo/site/pkg
mkdir -p "$PKG"

cargo build --release -p sonde-wasm --target wasm32-unknown-unknown
wasm-bindgen target/wasm32-unknown-unknown/release/sonde_wasm.wasm \
  --out-dir "$PKG" --target web
echo "wasm bundle built: $PKG (sonde_wasm.js + sonde_wasm_bg.wasm)"
