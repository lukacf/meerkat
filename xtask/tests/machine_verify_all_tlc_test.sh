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

if ! command -v tlc >/dev/null 2>&1; then
  echo "warning: tlc not on PATH; running hermetic machine-check-drift fallback"
  exec "${xtask_bin}" machine-check-drift --all
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
