#!/usr/bin/env bash
# Build the static demo assets: NOAA image -> payload.bin -> wasm bundle.
# Re-runnable. Requires: curl, jq, cargo, wasm-bindgen (cargo install wasm-bindgen-cli).
set -euo pipefail
cd "$(dirname "$0")/.."                     # repo/worktree root
OUT=demo/site/assets
PKG=demo/site/pkg
mkdir -p "$OUT" "$PKG"

# --- 1. Fetch a public-domain NOAA ERI aerial (rendered to JPEG via Commons API) ---
# NOAA Emergency Response Imagery is a US-Government work (public domain).
FILE="File:20200827aC0884800w295530n.tif"   # 2020 Hurricane Laura, NOAA ERI
API="https://commons.wikimedia.org/w/api.php"
THUMB=$(curl -fsSL --get "$API" \
  --data-urlencode "action=query" \
  --data-urlencode "titles=$FILE" \
  --data-urlencode "prop=imageinfo" \
  --data-urlencode "iiprop=url" \
  --data-urlencode "iiurlwidth=1024" \
  --data-urlencode "format=json" \
  | jq -r '.query.pages[].imageinfo[0].thumburl')
echo "thumb: $THUMB"
curl -fsSL "$THUMB" -o "$OUT/source.jpg"
printf 'Source: NOAA Emergency Response Imagery (2020 Hurricane Laura), public domain (US Gov work).\nVia Wikimedia Commons: %s\n' "$FILE" > "$OUT/source-credit.txt"

# --- 2. Build the SITREP payload ---
cargo run --release -p sonde-demo-builder -- "$OUT/source.jpg" "$OUT" --target-bytes 5000 --max-dim 200

# --- 3. Build the wasm bundle (shared with .github/workflows/pages.yml) ---
"$(dirname "$0")/build-wasm-bundle.sh"
echo "assets built: $OUT (payload.bin, payload.offsets.json), $PKG (sonde_wasm.js + _bg.wasm)"
