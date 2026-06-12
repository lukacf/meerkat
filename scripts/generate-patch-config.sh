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
  "meerkat-machine-derive=${ROOT}/meerkat-machine-derive"
  "meerkat-machine-dsl-core=${ROOT}/meerkat-machine-dsl-core"
  "meerkat-machine-dsl=${ROOT}/meerkat-machine-dsl"
  "meerkat-machine-schema=${ROOT}/meerkat-machine-schema"
  "meerkat-machine-kernels=${ROOT}/meerkat-machine-kernels"
  "meerkat-core=${ROOT}/meerkat-core"
  "meerkat-models=${ROOT}/meerkat-models"
  "meerkat-capabilities=${ROOT}/meerkat-capabilities"
  "meerkat-llm-core=${ROOT}/meerkat-llm-core"
  "meerkat-live=${ROOT}/meerkat-live"
  "meerkat-agent-build-authority=${ROOT}/meerkat-agent-build-authority"
  "meerkat-auth-core=${ROOT}/meerkat-auth-core"
  "meerkat-anthropic=${ROOT}/meerkat-anthropic"
  "meerkat-openai=${ROOT}/meerkat-openai"
  "meerkat-gemini=${ROOT}/meerkat-gemini"
  "meerkat-schedule=${ROOT}/meerkat-schedule"
  "meerkat-workgraph=${ROOT}/meerkat-workgraph"
  "meerkat-client=${ROOT}/meerkat-client"
  "meerkat-providers=${ROOT}/meerkat-providers"
  "meerkat-store=${ROOT}/meerkat-store"
  "meerkat-tools=${ROOT}/meerkat-tools"
  "meerkat-runtime=${ROOT}/meerkat-runtime"
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
  "meerkat-mob-adaptive=${ROOT}/meerkat-mob-adaptive"
  "meerkat-mob-mcp=${ROOT}/meerkat-mob-mcp"
  "meerkat-mob-pack=${ROOT}/meerkat-mob-pack"
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
