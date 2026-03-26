#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")" && pwd)"
WEB_DIR="$ROOT/web"
WEB_DIST="$WEB_DIR/dist"
REPO_ROOT="$(cd "$ROOT/../.." && pwd)"

if [[ "${1:-}" == "--clean" ]]; then
  echo "Cleaning generated web artifacts..."
  rm -rf "$WEB_DIR/public/meerkat-pkg" "$WEB_DIST"
fi

echo "Building current repo-local @rkat/web WASM runtime..."
(cd "$REPO_ROOT/sdks/web" && npm run build:wasm)

cd "$WEB_DIR"
npm install
npm run build

PORT="${PORT:-4174}"
echo "Built The Office demo: $WEB_DIST"
echo "Serve with: python3 -m http.server \"$PORT\" --directory \"$WEB_DIST\""
echo "Then open: http://127.0.0.1:$PORT"
