#!/usr/bin/env bash
set -euo pipefail

# Binary smoke tests: verify each surface binary starts with minimal features.
# Feature-combination cargo checks are handled by CI's surface-checks job.
echo "==> Surface modularity: binary smoke (--help)"
cargo run -p rkat --no-default-features --features session-store -- --help >/dev/null
cargo run -p meerkat-rpc --no-default-features --bin rkat-rpc -- --help >/dev/null
cargo run -p meerkat-rest --no-default-features --bin rkat-rest -- --help >/dev/null
cargo run -p meerkat-mcp-server --no-default-features --bin rkat-mcp -- --help >/dev/null

echo "Surface modularity checks passed"
