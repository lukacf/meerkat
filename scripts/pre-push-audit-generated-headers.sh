#!/usr/bin/env bash
# Pre-push @generated header truthfulness gate.
#
# Runs `xtask audit-generated-headers` to verify every `@generated`
# marker in the tree corresponds to a codegen-emit path and every
# codegen-emit path carries the marker. Non-zero exit blocks the push.
#
# This is the companion guard to `pre-push-machines.sh` and
# `pre-push-clippy.sh`: machines/verify catches schema drift,
# clippy catches lint regressions, this catches "hand-editing a
# generated file" and "adding a codegen pass without marking its
# output."

set -euo pipefail

cd "$(git rev-parse --show-toplevel)"

# Reuse the same xtask build path as the existing machine-authority
# pre-push gate so both hooks share the isolated target dir.
XTASK_TARGET_DIR="${XTASK_TARGET_DIR:-$HOME/.cache/meerkat/xtask-target}"

export CARGO_TARGET_DIR="$XTASK_TARGET_DIR"
cargo run -p xtask -- audit-generated-headers
