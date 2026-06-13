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

workspace_root="${PWD}"
if [[ -n "${TEST_SRCDIR:-}" && -n "${TEST_WORKSPACE:-}" && -d "${TEST_SRCDIR}/${TEST_WORKSPACE}" ]]; then
  workspace_root="${TEST_SRCDIR}/${TEST_WORKSPACE}"
fi

adaptive_model="${workspace_root}/specs/compositions/adaptive_mob_bundle/model.tla"
adaptive_witness="${workspace_root}/specs/compositions/adaptive_mob_bundle/witness-layer_terminal_feedback.cfg"
if [[ ! -f "${adaptive_model}" || ! -f "${adaptive_witness}" ]]; then
  echo "error: adaptive_mob_bundle bounded TLC witness files are missing from workspace runfiles." >&2
  echo "       model: ${adaptive_model}" >&2
  echo "       cfg:   ${adaptive_witness}" >&2
  exit 1
fi

tlc_workers="${TLC_WORKERS:-}"
if [[ -z "${tlc_workers}" ]]; then
  tlc_workers="$(getconf _NPROCESSORS_ONLN 2>/dev/null || sysctl -n hw.ncpu 2>/dev/null || echo 1)"
fi

# The full adaptive composition includes two complete MobMachine instances and
# is too large for the CI TLC budget. Before applying that broad skip, prove the
# generated route that matters for the adaptive bundle: terminal layer-mob
# classification is emitted, delivered, and observed through the canonical
# `layer_terminal_reaches_adaptive_kernel` route.
echo "running bounded adaptive_mob_bundle layer_terminal_feedback TLC witness"
tlc -workers "${tlc_workers}" -config "${adaptive_witness}" "${adaptive_model}"

# Broad composition full-TLC skips are CI-time/memory-budget exceptions, NOT
# codegen defects. `machine-verify` still validates drift and the generated
# ci.cfg structural-invariant contract before honoring these skips. The earlier
# "Unknown operator" gap (mob
# coordination temporal predicates referenced but not emitted) was RESOLVED by
# the LUC-524 machine-authority work: the generated composition model
# (specs/compositions/meerkat_mob_seam/model.tla) now parses cleanly under SANY
# (zero unknown-operator/semantic errors). What remains is scale — the emitted
# model is very large and a full ci.cfg state-space sweep does not yet fit the
# CI budget. The per-machine specs still model-check, and `machine-check-drift`
# (below + on the default cargo CI lane) keeps the generated kernels honest.
#
# `adaptive_mob_bundle` has a canonical route for layer-terminal feedback,
# generated driver, checked-in witness config, ci.cfg structural invariant, and
# the bounded witness TLC proof above. It still composes two full MobMachine
# instances, so the full composition TLC sweep exceeds the required CI budget.
exec "${xtask_bin}" machine-verify --all --skip-cargo-tests \
  --skip-tlc-composition meerkat_mob_seam \
  --skip-tlc-composition adaptive_mob_bundle
