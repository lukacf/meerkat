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

ROOT="${ROOT:-$(pwd)}"
source "${ROOT}/scripts/build-backend-env"

if meerkat_buildbuddy_enabled; then
  MEERKAT_BUILDBUDDY_CI_MODE="${MEERKAT_BUILDBUDDY_CI_MODE:-full-warm}" \
    "${ROOT}/scripts/buildbuddy-ci-lane" machine-authority
  exit 0
fi

CARGO="${CARGO:-./scripts/repo-cargo}"
# Stay in the dispatcher-owned RUST_LANE_ID selected by repo-cargo. The bridge
# classifier and every other lightweight xtask hook use this same target, so a
# push compiles the xtask closure once instead of once here and again in the
# next hook.
"$CARGO" xtask audit-generated-headers
