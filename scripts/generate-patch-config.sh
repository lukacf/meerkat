#!/usr/bin/env bash
# Generates [patch.crates-io] config for dry-run and CI publishing.
# Canonical crate-to-directory mapping lives here; callers redirect to a file.
#
# Usage: generate-patch-config.sh [ROOT_DIR] [EXCLUDE_CRATE]
#
# EXCLUDE_CRATE: omit this crate from the patch list (avoids lockfile
# collision when cargo publish verifies the package being published).

set -euo pipefail

ROOT="${1:-$(cd "$(dirname "$0")/.." && pwd)}"
EXCLUDE="${2:-}"

# Entries as "crate=path" pairs (bash 3.x compatible, no associative arrays).
ENTRIES=(
  "meerkat-core=${ROOT}/meerkat-core"
  "meerkat-client=${ROOT}/meerkat-client"
  "meerkat-store=${ROOT}/meerkat-store"
  "meerkat-tools=${ROOT}/meerkat-tools"
  "meerkat-session=${ROOT}/meerkat-session"
  "meerkat-memory=${ROOT}/meerkat-memory"
  "meerkat-mcp=${ROOT}/meerkat-mcp"
  "meerkat-mcp-server=${ROOT}/meerkat-mcp-server"
  "meerkat-hooks=${ROOT}/meerkat-hooks"
  "meerkat-skills=${ROOT}/meerkat-skills"
  "meerkat-comms=${ROOT}/meerkat-comms"
  "meerkat-rpc=${ROOT}/meerkat-rpc"
  "meerkat-rest=${ROOT}/meerkat-rest"
  "meerkat-contracts=${ROOT}/meerkat-contracts"
  "meerkat=${ROOT}/meerkat"
  "meerkat-mob=${ROOT}/meerkat-mob"
  "meerkat-mob-mcp=${ROOT}/meerkat-mob-mcp"
  "rkat=${ROOT}/meerkat-cli"
)

echo "[patch.crates-io]"
for entry in "${ENTRIES[@]}"; do
  crate="${entry%%=*}"
  path="${entry#*=}"
  if [[ "$crate" != "$EXCLUDE" ]]; then
    echo "${crate} = { path = \"${path}\" }"
  fi
done
