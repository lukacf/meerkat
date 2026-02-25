#!/usr/bin/env bash
# Generates [patch.crates-io] config for dry-run and CI publishing.
# Canonical crate-to-directory mapping lives here; callers redirect to a file.
#
# Usage: generate-patch-config.sh [ROOT_DIR]

set -euo pipefail

ROOT="${1:-$(cd "$(dirname "$0")/.." && pwd)}"

cat <<EOF
[patch.crates-io]
meerkat-core = { path = "${ROOT}/meerkat-core" }
meerkat-client = { path = "${ROOT}/meerkat-client" }
meerkat-store = { path = "${ROOT}/meerkat-store" }
meerkat-tools = { path = "${ROOT}/meerkat-tools" }
meerkat-session = { path = "${ROOT}/meerkat-session" }
meerkat-memory = { path = "${ROOT}/meerkat-memory" }
meerkat-mcp = { path = "${ROOT}/meerkat-mcp" }
meerkat-mcp-server = { path = "${ROOT}/meerkat-mcp-server" }
meerkat-hooks = { path = "${ROOT}/meerkat-hooks" }
meerkat-skills = { path = "${ROOT}/meerkat-skills" }
meerkat-comms = { path = "${ROOT}/meerkat-comms" }
meerkat-rpc = { path = "${ROOT}/meerkat-rpc" }
meerkat-rest = { path = "${ROOT}/meerkat-rest" }
meerkat-contracts = { path = "${ROOT}/meerkat-contracts" }
meerkat = { path = "${ROOT}/meerkat" }
meerkat-mob = { path = "${ROOT}/meerkat-mob" }
meerkat-mob-mcp = { path = "${ROOT}/meerkat-mob-mcp" }
rkat = { path = "${ROOT}/meerkat-cli" }
EOF
