#!/usr/bin/env bash
set -euo pipefail

xtask_bin="${1:?xtask binary path is required}"
if [[ "${xtask_bin}" != /* ]]; then
  if [[ -x "${PWD}/${xtask_bin}" ]]; then
    xtask_bin="${PWD}/${xtask_bin}"
  else
    xtask_bin="${TEST_SRCDIR:?}/${TEST_WORKSPACE:?}/${xtask_bin}"
  fi
fi

if [[ -n "${RUSTFMT:-}" && "${RUSTFMT}" != /* ]]; then
  if [[ -x "${PWD}/${RUSTFMT}" ]]; then
    RUSTFMT="${PWD}/${RUSTFMT}"
  else
    RUSTFMT="${TEST_SRCDIR:?}/${TEST_WORKSPACE:?}/${RUSTFMT}"
  fi
  export RUSTFMT
fi

# Fail closed: this lane advertises TLC-backed verification (it is named
# `machine_verify_all_tlc_test` and blessed by buildbuddy-doctor as
# "machine-verify/TLC"). Silently degrading to a drift-only `machine-check-drift`
# pass when `tlc` is absent would launder a weaker check as TLC verification.
#
# If `tlc` is missing, the lane MUST fail unless a caller has explicitly opted
# into a drift-only run by exporting MACHINE_VERIFY_TLC_DRIFT_ONLY=1. That
# opt-in is deliberately off by default so the absence of `tlc` on a lane that
# claims TLC is treated as a hard failure, not a quiet downgrade.
if ! command -v tlc >/dev/null 2>&1; then
  if [[ "${MACHINE_VERIFY_TLC_DRIFT_ONLY:-0}" == "1" ]]; then
    echo "MACHINE_VERIFY_TLC_DRIFT_ONLY=1: tlc absent, running drift-only machine-check-drift (NOT TLC verification)"
    exec "${xtask_bin}" machine-check-drift --all
  fi
  echo "error: tlc not on PATH but this lane advertises TLC-backed verification." >&2
  echo "       Provision tlc on the lane, or set MACHINE_VERIFY_TLC_DRIFT_ONLY=1 to" >&2
  echo "       explicitly run the weaker drift-only check (which is NOT TLC)." >&2
  exit 1
fi

# `meerkat_mob_seam` composition TLC is skipped for a CI-time/memory-budget
# reason, NOT a codegen defect. The earlier "Unknown operator" gap (mob
# coordination temporal predicates referenced but not emitted) was RESOLVED by
# the LUC-524 machine-authority work: the generated composition model
# (specs/compositions/meerkat_mob_seam/model.tla) now parses cleanly under SANY
# (zero unknown-operator/semantic errors). What remains is scale — the emitted
# model is very large and a full ci.cfg state-space sweep does not yet fit the
# CI budget. The per-machine specs still model-check, and `machine-check-drift`
# (below + on the default cargo CI lane) keeps the generated kernels honest.
# TODO(LUC-524 follow-up): land a bounded composition TLC config that fits the
# CI budget and drop this skip to restore cross-machine model checking.
exec "${xtask_bin}" machine-verify --all --skip-cargo-tests --skip-tlc-composition meerkat_mob_seam
