#!/usr/bin/env bash
# Unit gate of the manual hook stage, run on demand before a push
# (`pre-commit run --hook-stage manual cargo-test --from-ref origin/main
# --to-ref HEAD`); CI's test job runs every lib and bin test on every pull
# request that touches Rust. Workspace lib tests, with incremental
# compilation OFF.
#
# Same reasoning as scripts/pre-push-clippy.sh: a stale incremental cache makes
# rustc abort with an internal compiler error (verify_ich "unstable
# fingerprints") that reproduces across runs and therefore looks like a real,
# deterministic failure in the change being pushed. It is not. Gates run rarely
# and usually against a cold or foreign cache, so incremental buys them little
# and costs a false push failure that is expensive to diagnose.
set -euo pipefail

export CARGO_INCREMENTAL=0

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
exec "${SCRIPT_DIR}/repo-cargo" nextest run --workspace --lib --no-fail-fast "$@"
