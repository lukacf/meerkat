#!/usr/bin/env bash
# If machine-related files changed, run codegen + verify
set -euo pipefail

ROOT="${ROOT:-$(cd "$(dirname "$0")/.." && pwd)}"
CARGO="${CARGO:-$ROOT/scripts/repo-cargo}"
source "${ROOT}/scripts/build-backend-env"

changed=$(git diff --cached --name-only HEAD | grep -E '(machine-schema/src/catalog|_authority\.rs|machine-kernels/src/generated)' || true)
if [ -n "$changed" ]; then
    echo "Machine files changed, running codegen + verify..."
    if meerkat_buildbuddy_enabled; then
        MEERKAT_BUILDBUDDY_CI_MODE="${MEERKAT_BUILDBUDDY_CI_MODE:-full-warm}" \
            "${ROOT}/scripts/buildbuddy-ci-lane" machine-authority || exit 1
    else
        "$CARGO" xtask machine-codegen --all || exit 1
        # Route verification through the canonical TLC lane: it owns the
        # documented over-budget composition skips (meerkat_mob_seam /
        # adaptive_mob_bundle full sweeps) and the bounded adaptive witness
        # proof. A bare `machine-verify --all` runs the full mob-seam ci.cfg
        # sweep, which does not fit a pre-push budget.
        make -C "$ROOT" machine-verify || exit 1
    fi
fi
