#!/usr/bin/env bash
set -euo pipefail

ROOT="${ROOT:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}"
CARGO="${CARGO:-${ROOT}/scripts/repo-cargo}"
NEXTEST_BIN="${NEXTEST_BIN:-cargo-nextest}"

usage() {
  cat >&2 <<'EOF'
usage:
  scripts/ci-nextest-archive.sh build <family> <archive-file>
  scripts/ci-nextest-archive.sh run <family> <archive-file> <partition>

families: unit, int-heavy, int-mob, int-everything-else
EOF
  exit 2
}

family="${2:-}"
archive_file="${3:-}"
[[ -n "$family" && -n "$archive_file" ]] || usage

profile=default
status_level=none
final_status_level=fail
case "$family" in
  unit)
    cargo_args=(--workspace --lib)
    profile=ci-unit
    ;;
  int-heavy)
    cargo_args=(-p meerkat-integration-tests --tests --profile fast)
    profile=fast
    status_level=slow
    final_status_level=slow
    ;;
  int-mob)
    cargo_args=(
      -p meerkat-mob
      -p meerkat-mob-adaptive
      -p meerkat-mob-mcp
      -p meerkat-mob-pack
      -p meerkat-schedule
      -p meerkat-workgraph
      --tests
      --profile fast
    )
    profile=fast
    status_level=slow
    final_status_level=slow
    ;;
  int-everything-else)
    cargo_args=(
      --workspace
      --exclude meerkat-integration-tests
      --exclude meerkat-mob
      --exclude meerkat-mob-adaptive
      --exclude meerkat-mob-mcp
      --exclude meerkat-mob-pack
      --exclude meerkat-schedule
      --exclude meerkat-workgraph
      --exclude meerkat-core
      --exclude meerkat-models
      --exclude meerkat-machine-codegen
      --exclude meerkat-machine-derive
      --exclude meerkat-machine-dsl
      --exclude meerkat-machine-dsl-core
      --exclude meerkat-machine-kernels
      --exclude meerkat-machine-schema
      --exclude machine-dsl-tests
      --exclude meerkat-capabilities
      --exclude meerkat-agent-build-authority
      --exclude meerkat-contracts
      --exclude meerkat-anthropic
      --exclude meerkat-auth-core
      --exclude meerkat-client
      --exclude meerkat-comms
      --exclude meerkat-gemini
      --exclude meerkat-hooks
      --exclude meerkat-llm-core
      --exclude meerkat-mcp
      --exclude meerkat-memory
      --exclude meerkat-openai
      --exclude meerkat-providers
      --exclude meerkat-session
      --exclude meerkat-skills
      --exclude meerkat-store
      --exclude meerkat-tools
      --tests
      --profile fast
    )
    profile=fast
    status_level=slow
    final_status_level=slow
    ;;
  *)
    echo "error: unknown nextest archive family '${family}'" >&2
    usage
    ;;
esac

case "${1:-}" in
  build)
    mkdir -p "$(dirname "$archive_file")"
    "$CARGO" nextest archive "${cargo_args[@]}" --archive-file "$archive_file"
    [[ -s "$archive_file" ]] || {
      echo "error: nextest archive was not created at ${archive_file}" >&2
      exit 1
    }
    ;;
  run)
    partition="${4:-}"
    [[ -n "$partition" ]] || usage
    [[ -s "$archive_file" ]] || {
      echo "error: nextest archive is missing or empty at ${archive_file}" >&2
      exit 1
    }
    run_args=(
      nextest run
      --archive-file "$archive_file"
      --workspace-remap "$ROOT"
      --no-tests=fail
      --show-progress none
      --status-level "$status_level"
      --final-status-level "$final_status_level"
      --partition "$partition"
    )
    if [[ "$profile" != default ]]; then
      run_args+=(--profile "$profile")
    fi
    MEERKAT_WORKSPACE_ROOT="$ROOT" "$NEXTEST_BIN" "${run_args[@]}"
    ;;
  *)
    usage
    ;;
esac
