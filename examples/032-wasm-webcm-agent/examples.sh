#!/usr/bin/env bash
# 032 — Meerkat WebCM Agent
#
# Downloads pre-built WebCM WASM bundle, builds the Vite app, and serves.
# WebCM (https://github.com/edubart/webcm) by @edubart — Cartesi Machine
# RISC-V emulator compiled to WebAssembly.
#
# Prerequisites: node, npm, curl (wasm-pack needed if sdks/web not pre-built)
# Usage: ./examples.sh [--clean]

set -euo pipefail
cd "$(dirname "$0")"

WEBCM_BASE="https://edubart.github.io/webcm"
WEB_DIR="web"
PUBLIC_DIR="${WEB_DIR}/public"
MEERKAT_PKG="${PUBLIC_DIR}/meerkat-pkg"

# ── Clean flag ───────────────────────────────────────────────────────────────

if [[ "${1:-}" == "--clean" ]]; then
  echo "Cleaning cached WebCM and meerkat-pkg artifacts..."
  rm -f "${PUBLIC_DIR}/webcm.mjs" "${PUBLIC_DIR}/webcm.wasm"
  rm -rf "${MEERKAT_PKG}"
  echo "Done. Re-downloading and rebuilding."
fi

# ── Download WebCM WASM bundle ──────────────────────────────────────────────

if [[ ! -f "${PUBLIC_DIR}/webcm.mjs" ]]; then
  echo "Downloading WebCM (~30 MB)..."
  mkdir -p "${PUBLIC_DIR}"

  curl -fSL "${WEBCM_BASE}/webcm.mjs" -o "${PUBLIC_DIR}/webcm.mjs"
  curl -fSL "${WEBCM_BASE}/webcm.wasm" -o "${PUBLIC_DIR}/webcm.wasm"

  echo "WebCM downloaded to ${PUBLIC_DIR}/"
else
  echo "WebCM already downloaded"
fi

# ── Build meerkat WASM runtime (for mob mode) ────────────────────────────────

if [[ ! -f "${MEERKAT_PKG}/meerkat_web_runtime_bg.wasm" ]]; then
  echo "Building meerkat-web-runtime for wasm32..."
  REPO_ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
  SDK_WASM_DIR="${REPO_ROOT}/sdks/web/wasm"

  # Build via sdks/web's build:wasm script (handles --out-dir and .gitignore cleanup)
  if [[ -f "${REPO_ROOT}/sdks/web/package.json" ]]; then
    (cd "${REPO_ROOT}/sdks/web" && npm run build:wasm)
  else
    RUSTFLAGS='--cfg getrandom_backend="wasm_js"' \
      wasm-pack build "${REPO_ROOT}/meerkat-web-runtime" \
        --target web \
        --out-dir "${PWD}/${MEERKAT_PKG}" \
        --dev
  fi

  # Copy from sdks/web/wasm if built there
  if [[ -d "${SDK_WASM_DIR}" && -f "${SDK_WASM_DIR}/meerkat_web_runtime_bg.wasm" ]]; then
    mkdir -p "${MEERKAT_PKG}"
    cp "${SDK_WASM_DIR}/meerkat_web_runtime.js" "${MEERKAT_PKG}/"
    cp "${SDK_WASM_DIR}/meerkat_web_runtime_bg.wasm" "${MEERKAT_PKG}/"
  fi

  echo "Meerkat WASM runtime built to ${MEERKAT_PKG}/"
else
  echo "Meerkat WASM runtime already built"
fi

# ── Install deps and build ──────────────────────────────────────────────────

cd "${WEB_DIR}"
npm install

echo ""
echo "Starting dev server..."
echo "Open http://127.0.0.1:4032 in your browser"
echo ""
npx vite
