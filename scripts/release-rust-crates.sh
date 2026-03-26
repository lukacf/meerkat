#!/usr/bin/env bash

set -euo pipefail

cat <<'EOF'
meerkat-models
meerkat-machine-schema
meerkat-machine-kernels
meerkat-core
meerkat-contracts
meerkat-client
meerkat-store
meerkat-tools
meerkat-runtime
meerkat-session
meerkat-memory
meerkat-mcp
meerkat-mcp-server
meerkat-hooks
meerkat-skills
meerkat-comms
meerkat-rpc
meerkat-rest
meerkat
meerkat-mob
meerkat-mob-mcp
meerkat-mob-pack
rkat
EOF
