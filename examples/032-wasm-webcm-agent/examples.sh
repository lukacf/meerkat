#!/usr/bin/env bash
# 032 — Meerkat WebCM Agent
#
# Downloads pre-built WebCM WASM bundle, builds the Vite app, and serves.
#
# Prerequisites: node, npm
# Usage: ./examples.sh

set -euo pipefail
cd "$(dirname "$0")"

WEBCM_BASE="https://edubart.github.io/webcm"
WEB_DIR="web"
PUBLIC_DIR="${WEB_DIR}/public"

# ── Download WebCM WASM bundle ──────────────────────────────────────────────

if [[ ! -f "${PUBLIC_DIR}/webcm.mjs" ]]; then
  echo "Downloading WebCM..."
  mkdir -p "${PUBLIC_DIR}"

  curl -fSL "${WEBCM_BASE}/webcm.mjs" -o "${PUBLIC_DIR}/webcm.mjs"
  curl -fSL "${WEBCM_BASE}/webcm.wasm" -o "${PUBLIC_DIR}/webcm.wasm"

  echo "WebCM downloaded (~30 MB) to ${PUBLIC_DIR}/"
else
  echo "WebCM already downloaded"
fi

# ── Install deps and build ──────────────────────────────────────────────────

cd "${WEB_DIR}"
npm install

echo ""
echo "Starting dev server..."
echo "Open http://127.0.0.1:4032 in your browser"
echo ""
npx vite
