#!/usr/bin/env bash
set -euo pipefail

ROOT="${ROOT:-$(cd "$(dirname "$0")/.." && pwd)}"
CARGO="${CARGO:-$ROOT/scripts/repo-cargo}"

# Binary smoke tests: verify each surface binary starts with minimal features.
# Feature-combination cargo checks are handled by CI's surface-checks job.
echo "==> Surface modularity: binary smoke (--help)"
"$CARGO" run -p rkat --no-default-features --features session-store -- --help >/dev/null
"$CARGO" run -p meerkat-rpc --no-default-features --bin rkat-rpc -- --help >/dev/null
"$CARGO" run -p meerkat-rest --no-default-features --bin rkat-rest -- --help >/dev/null
"$CARGO" run -p meerkat-mcp-server --no-default-features --bin rkat-mcp -- --help >/dev/null

echo "Surface modularity checks passed"
