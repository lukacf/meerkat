#!/usr/bin/env bash
# The one hand-ordered enumeration of the publishable workspace crates, in
# dependency order. make check-rust-release-config binds it to the
# publishable workspace members, the patch map and the order documented in
# CLAUDE.md; nothing else may list the crates by hand.
#
#   release-rust-crates.sh                 one crate per line, in this order
#   release-rust-crates.sh --levels [ARGS]  the same crates grouped into the
#       dependency levels a parallel publish walks, derived from this order
#       and `cargo metadata` by scripts/release_crate_levels.py (ARGS pass
#       through, for example `--format summary`)

set -euo pipefail

if [[ "${1:-}" == "--levels" ]]; then
  shift
  root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
  python="$("${root}/scripts/require-python" 3.10 "$(basename "${BASH_SOURCE[0]}") --levels")" || exit 1
  "${BASH_SOURCE[0]}" | "${python}" "${root}/scripts/release_crate_levels.py" --root "${root}" "$@"
  exit
fi
if [[ "$#" -ne 0 ]]; then
  echo "usage: $(basename "$0") [--levels [--format tsv|summary]]" >&2
  exit 2
fi

printf '%s\n' \
  meerkat-sqlite \
  meerkat-machine-derive \
  meerkat-machine-dsl-core \
  meerkat-agent-build-authority \
  meerkat-core \
  meerkat-sandbox \
  meerkat-authorization-contracts \
  meerkat-atif \
  meerkat-store-conformance \
  meerkat-models \
  meerkat-capabilities \
  meerkat-machine-dsl \
  meerkat-machine-schema \
  meerkat-machine-codegen \
  meerkat-authorization \
  meerkat-machine-kernels \
  meerkat-skills \
  meerkat-schedule \
  meerkat-jobs \
  meerkat-workgraph \
  meerkat-contracts \
  meerkat-store \
  meerkat-llm-core \
  meerkat-live \
  meerkat-auth-core \
  meerkat-memory \
  meerkat-mcp \
  meerkat-hooks \
  meerkat-comms \
  meerkat-runtime \
  meerkat-copilot \
  meerkat-anthropic \
  meerkat-openai \
  meerkat-gemini \
  meerkat-providers \
  meerkat-tools \
  meerkat-session \
  meerkat-client \
  meerkat \
  meerkat-mob \
  meerkat-mob-adaptive \
  meerkat-mob-mcp \
  meerkat-mob-pack \
  meerkat-mcp-server \
  meerkat-rpc \
  meerkat-rest \
  rkat
