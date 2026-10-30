#!/usr/bin/env bash
# Gate tests for the xtask-scripts dogma remediation rows (#285, #227, #203,
# #296, #266, #221). These are pure shell/make behaviors that do not require a
# cargo build, so they run as a standalone, self-contained gate.
#
# Run: ./scripts/tests/xtask_scripts_dogma_gates.sh

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

pass=0
fail=0

ok()  { printf '  OK:   %s\n' "$*"; pass=$((pass + 1)); }
bad() { printf '  FAIL: %s\n' "$*"; fail=$((fail + 1)); }

# ── #285: schema freshness gate must not mutate artifacts while validating ───
# The emitter receives an explicit temp output directory and the
# committed-vs-fresh diff happens against that directory, so the workspace
# tree is never written.
echo "#285 verify-schema-freshness leaves the workspace clean:"
if grep -Fq -- '-- "$FRESH_DIR"' scripts/verify-schema-freshness.sh; then
  ok "emit-schemas receives an explicit temp output directory"
else
  bad "emit-schemas does not receive an explicit temp output directory"
fi
if grep -Fq 'fresh="$FRESH_DIR/$fname"' scripts/verify-schema-freshness.sh \
  && ! grep -Fq 'git show HEAD:"artifacts/schemas/$fname"' scripts/verify-schema-freshness.sh; then
  ok "diff compares committed vs temp-dir fresh, not git-HEAD vs mutated tree"
else
  bad "stale git-HEAD-vs-working-tree comparison still present"
fi

# ── #227: Bazel CARGO_PKG_VERSION is gated and regenerated ───────────────────
echo "#227 Bazel CARGO_PKG_VERSION is gated + regenerated:"
if grep -Fq 'CARGO_PKG_VERSION' scripts/verify-version-parity.sh \
  && grep -Fq -- '-name BUILD.bazel' scripts/verify-version-parity.sh; then
  ok "verify-version-parity.sh asserts BUILD.bazel CARGO_PKG_VERSION"
else
  bad "verify-version-parity.sh does not assert BUILD.bazel CARGO_PKG_VERSION"
fi
if grep -Fq 'scripts/sync-bazel-package-version.sh' Makefile; then
  ok "regen-schemas wires sync-bazel-package-version.sh"
else
  bad "regen-schemas does not regenerate the Bazel CARGO_PKG_VERSION"
fi
# Behavioral: a workspace-version bump without regen makes the parity check fail.
WS_VER=$(grep -m1 -E '^version = "' Cargo.toml | sed -n 's/.*"\([^"]*\)".*/\1/p')
BAZEL_VERS=$(grep -E '"CARGO_PKG_VERSION":' crates/meerkat-machine-codegen/BUILD.bazel \
  | sed -n 's/.*"CARGO_PKG_VERSION": *"\([^"]*\)".*/\1/p' | sort -u)
mismatch=0
while IFS= read -r bv; do
  [ -n "$bv" ] || continue
  [ "$bv" = "$WS_VER" ] || mismatch=1
done <<EOF
$BAZEL_VERS
EOF
if [ "$mismatch" -eq 0 ]; then
  ok "BUILD.bazel CARGO_PKG_VERSION currently matches workspace $WS_VER (sync would fail-close on drift)"
else
  bad "BUILD.bazel CARGO_PKG_VERSION already drifted from workspace $WS_VER"
fi

# ── #203: governance source routes the machine-authority + edge lanes ────────
echo "#203 governance source escalates change-detection lanes:"
if scripts/machine-authority-changed -- crates/xtask/src/rmat_policy.rs >/dev/null; then
  ok "machine-authority-changed exits 0 (changed) for crates/xtask/src/rmat_policy.rs"
else
  bad "machine-authority-changed did not flag crates/xtask/src/rmat_policy.rs"
fi
# The machine and protocol generators both write through generated_files.rs;
# a change there changes what the drift checks compare.
if scripts/machine-authority-changed -- crates/xtask/src/generated_files.rs >/dev/null; then
  ok "machine-authority-changed exits 0 (changed) for the generators' writer crates/xtask/src/generated_files.rs"
else
  bad "machine-authority-changed did not flag the generators' writer crates/xtask/src/generated_files.rs"
fi
if scripts/machine-authority-changed -- crates/meerkat-mob/src/runtime/actor.rs >/dev/null; then
  ok "machine-authority-changed exits 0 (changed) for mob runtime authority"
else
  bad "machine-authority-changed did not flag mob runtime authority"
fi
if scripts/machine-authority-changed -- docs/reference/machine-authority.mdx >/dev/null; then
  ok "machine-authority-changed exits 0 (changed) for machine-authority docs"
else
  bad "machine-authority-changed did not flag machine-authority docs"
fi
for grant_input in crates/meerkat-authorization/src/grants/mod.rs crates/meerkat-authorization-contracts/src/constraints.rs; do
  if scripts/machine-authority-changed -- "$grant_input" >/dev/null; then
    ok "machine-authority-changed protects grant authority input $grant_input"
  else
    bad "machine-authority-changed ignored grant authority input $grant_input"
  fi
done
for poster_input in scripts/machine-posters/generate-machine-posters.mjs docs/internal/machine-posters/mob_machine.html crates/xtask/src/machine_alphabet.rs scripts/machine-posters-changed; do
  if scripts/machine-posters-changed -- "$poster_input" >/dev/null; then
    ok "machine-posters-changed routes poster input $poster_input to the poster gate"
  else
    bad "machine-posters-changed ignored poster input $poster_input"
  fi
  if scripts/machine-authority-changed -- "$poster_input" >/dev/null; then
    bad "machine-authority-changed routes poster-only input $poster_input to the machine lanes"
  else
    ok "poster-only input $poster_input stays out of the machine lanes"
  fi
done
if scripts/machine-posters-changed -- crates/meerkat-core/src/lib.rs >/dev/null; then
  bad "machine-posters-changed flagged an unrelated path"
else
  ok "machine-posters-changed ignores unrelated paths"
fi
for gate_owner in .github/workflows/ci.yml .github/workflows/cargo.yml Makefile scripts/machine-authority-changed scripts/tests/xtask_scripts_dogma_gates.sh crates/xtask/tests/ci_gate_requires_rmat.rs; do
  if scripts/machine-authority-changed -- "$gate_owner" >/dev/null; then
    ok "machine-authority-changed protects its gate owner $gate_owner"
  else
    bad "machine-authority-changed ignored its gate owner $gate_owner"
  fi
done
edge_out=$(printf 'crates/xtask/src/rmat_policy.rs\n' | scripts/buildbuddy-edge-changes --paths-from-stdin)
if printf '%s' "$edge_out" | grep -Fq 'changed=true'; then
  ok "buildbuddy-edge-changes marks rmat_policy.rs as changed"
else
  bad "buildbuddy-edge-changes ignored rmat_policy.rs"
fi
edge_dogma=$(printf 'docs/architecture/meerkat-dogma.md\n' | scripts/buildbuddy-edge-changes --paths-from-stdin)
if printf '%s' "$edge_dogma" | grep -Fq 'wasm_changed=true'; then
  ok "buildbuddy-edge-changes mark_all triggers on a dogma doc"
else
  bad "buildbuddy-edge-changes did not mark_all on a dogma doc"
fi
edge_internal_dogma=$(printf 'docs/internal/dogma-audits/PR759-final-ledger.md\n' | scripts/buildbuddy-edge-changes --paths-from-stdin)
if printf '%s' "$edge_internal_dogma" | grep -Fq 'wasm_changed=true'; then
  ok "buildbuddy-edge-changes mark_all triggers on internal dogma audit docs"
else
  bad "buildbuddy-edge-changes did not mark_all on internal dogma audit docs"
fi
finite_ledger='docs/internal/archive/public-docs-removed-2026-05-11/architecture/finite-ownership-ledger.md'
if scripts/machine-authority-changed -- "$finite_ledger" >/dev/null; then
  ok "machine-authority-changed exits 0 (changed) for finite ownership ledger"
else
  bad "machine-authority-changed did not flag finite ownership ledger"
fi
edge_finite_ledger=$(printf '%s\n' "$finite_ledger" | scripts/buildbuddy-edge-changes --paths-from-stdin)
if printf '%s' "$edge_finite_ledger" | grep -Fq 'changed=true'; then
  ok "buildbuddy-edge-changes marks finite ownership ledger as changed"
else
  bad "buildbuddy-edge-changes ignored finite ownership ledger"
fi
# Governance baselines are governance truth: a baseline-TOML-only diff must
# escalate both the machine-authority lane and the edge lanes.
for baseline in crates/xtask/rmat-baseline.toml crates/xtask/ownership-baseline.toml; do
  if scripts/machine-authority-changed -- "$baseline" >/dev/null; then
    ok "machine-authority-changed exits 0 (changed) for $baseline"
  else
    bad "machine-authority-changed did not flag $baseline"
  fi
  edge_baseline=$(printf '%s\n' "$baseline" | scripts/buildbuddy-edge-changes --paths-from-stdin)
  if printf '%s' "$edge_baseline" | grep -Fq 'changed=true'; then
    ok "buildbuddy-edge-changes marks $baseline as changed"
  else
    bad "buildbuddy-edge-changes ignored $baseline"
  fi
done

# ── #296: governance/spec changes route formal/governance checks ─────────────
echo "#296 agent gates route governance/spec changes:"
if grep -Fq 'is_governance_path' scripts/cargo-agent-gate \
  && grep -Fq 'machine-check-drift machine-authority-docs-gate runtime-authority-bypass seam-inventory rmat-audit' scripts/cargo-agent-gate; then
  ok "cargo-agent-gate has governance allowlist + formal gate routing"
else
  bad "cargo-agent-gate missing governance allowlist or formal gate routing"
fi
if grep -Fq 'is_governance_path' scripts/buildbuddy-agent-gate \
  && grep -Fq 'machine-check-drift machine-authority-docs-gate runtime-authority-bypass seam-inventory rmat-audit' scripts/buildbuddy-agent-gate; then
  ok "buildbuddy-agent-gate has governance allowlist + formal gate routing"
else
  bad "buildbuddy-agent-gate missing governance allowlist or formal gate routing"
fi
# Behavioral: a spec-only change is no longer reported as "no changes" and is
# routed to the governance gate (dry-run avoids invoking cargo/make work).
gate_out=$(scripts/cargo-agent-gate --dry-run -- specs/machines/example.tla 2>&1 || true)
if printf '%s' "$gate_out" | grep -Fq 'machine-check-drift machine-authority-docs-gate runtime-authority-bypass seam-inventory rmat-audit' \
  && ! printf '%s' "$gate_out" | grep -Fq 'no Rust build-relevant changes detected.'; then
  ok "cargo-agent-gate dry-run routes specs/machines change to governance gate"
else
  bad "cargo-agent-gate dry-run did not route specs/machines change to governance gate"
fi
docs_gate_out=$(scripts/buildbuddy-agent-gate --dry-run -- docs/reference/machine-authority.mdx 2>&1 || true)
if printf '%s' "$docs_gate_out" | grep -Fq 'machine-check-drift machine-authority-docs-gate runtime-authority-bypass seam-inventory rmat-audit' \
  && ! printf '%s' "$docs_gate_out" | grep -Fq 'no build-relevant changes detected.'; then
  ok "buildbuddy-agent-gate dry-run routes machine-authority docs to governance gate"
else
  bad "buildbuddy-agent-gate dry-run did not route machine-authority docs"
fi
generated_machine_gate_out=$(scripts/cargo-agent-gate --dry-run -- crates/meerkat-machine-schema/src/catalog/dsl/meerkat_machine.rs 2>&1 || true)
if printf '%s' "$generated_machine_gate_out" | grep -Fq 'machine-check-drift machine-authority-docs-gate seam-inventory rmat-audit' \
  && ! printf '%s' "$generated_machine_gate_out" | grep -Fq 'no Rust build-relevant changes detected.'; then
  ok "cargo-agent-gate dry-run routes generated machine catalog changes to governance gate"
else
  bad "cargo-agent-gate dry-run did not route generated machine catalog changes"
fi
mob_identity_gate_out=$(scripts/cargo-agent-gate --dry-run -- crates/meerkat-mob/src/ids.rs 2>&1 || true)
if printf '%s' "$mob_identity_gate_out" | grep -Fq 'machine-check-drift machine-authority-docs-gate runtime-authority-bypass seam-inventory rmat-audit' \
  && ! printf '%s' "$mob_identity_gate_out" | grep -Fq 'no Rust build-relevant changes detected.'; then
  ok "cargo-agent-gate dry-run routes mob identity authority changes to governance gate"
else
  bad "cargo-agent-gate dry-run did not route mob identity authority changes"
fi
generated_buildbuddy_gate_out=$(scripts/buildbuddy-agent-gate --dry-run -- crates/meerkat-core/src/generated/protocol_runtime.rs 2>&1 || true)
if printf '%s' "$generated_buildbuddy_gate_out" | grep -Fq 'machine-check-drift machine-authority-docs-gate seam-inventory rmat-audit' \
  && ! printf '%s' "$generated_buildbuddy_gate_out" | grep -Fq 'no build-relevant changes detected.'; then
  ok "buildbuddy-agent-gate dry-run routes generated protocol changes to governance gate"
else
  bad "buildbuddy-agent-gate dry-run did not route generated protocol changes"
fi
if grep -Fq '//crates/meerkat-machine-schema:docs_machine_authority_alignment_test' scripts/buildbuddy-bazel-poc \
  && grep -Fq 'docs_machine_authority_alignment' scripts/buildbuddy-ci-full; then
  ok "BuildBuddy machine-authority lanes include machine-authority docs alignment"
else
  bad "BuildBuddy machine-authority lanes omit machine-authority docs alignment"
fi
schema_gate_out=$(scripts/cargo-agent-gate --dry-run -- artifacts/schemas/rest-openapi.json 2>&1 || true)
if printf '%s' "$schema_gate_out" | grep -Fq 'verify-schema-freshness verify-sdk-codegen-freshness verify-sdk-event-inventory verify-rpc-surface-alignment verify-rest-surface-alignment' \
  && ! printf '%s' "$schema_gate_out" | grep -Fq 'no Rust build-relevant changes detected.'; then
  ok "cargo-agent-gate dry-run routes schema artifacts to generated contract ratchets"
else
  bad "cargo-agent-gate dry-run did not route schema artifacts to generated contract ratchets"
fi
sdk_gate_out=$(scripts/buildbuddy-agent-gate --dry-run -- sdks/typescript/src/generated/types.ts sdks/python/meerkat/generated/types.py 2>&1 || true)
if printf '%s' "$sdk_gate_out" | grep -Fq 'verify-schema-freshness verify-sdk-codegen-freshness verify-sdk-event-inventory verify-rpc-surface-alignment verify-rest-surface-alignment' \
  && ! printf '%s' "$sdk_gate_out" | grep -Fq 'no build-relevant changes detected.'; then
  ok "buildbuddy-agent-gate dry-run routes SDK generated artifacts to generated contract ratchets"
else
  bad "buildbuddy-agent-gate dry-run did not route SDK generated artifacts"
fi
surface_gate_out=$(scripts/cargo-agent-gate --dry-run -- crates/meerkat-mcp/src/router.rs crates/meerkat-mob-mcp/src/agent_tools.rs docs/api/mcp.mdx sdks/typescript/README.md 2>&1 || true)
if printf '%s' "$surface_gate_out" | grep -Fq 'verify-schema-freshness verify-sdk-codegen-freshness verify-sdk-event-inventory verify-rpc-surface-alignment verify-rest-surface-alignment' \
  && ! printf '%s' "$surface_gate_out" | grep -Fq 'no Rust build-relevant changes detected.'; then
  ok "cargo-agent-gate dry-run routes public surface docs/router/mob-mcp changes to generated contract ratchets"
else
  bad "cargo-agent-gate dry-run did not route public surface docs/router/mob-mcp changes"
fi
web_gate_out=$(scripts/cargo-agent-gate --dry-run -- sdks/web/src/runtime.ts 2>&1 || true)
if printf '%s' "$web_gate_out" | grep -Fq 'test-sdk-web' \
  && ! printf '%s' "$web_gate_out" | grep -Fq 'no Rust build-relevant changes detected.'; then
  ok "cargo-agent-gate dry-run routes Web SDK changes to Web SDK tests"
else
  bad "cargo-agent-gate dry-run did not route Web SDK changes"
fi
wasm_gate_out=$(scripts/cargo-agent-gate --dry-run -- crates/meerkat-web-runtime/src/lib.rs 2>&1 || true)
if printf '%s' "$wasm_gate_out" | grep -Fq 'wasm-check' \
  && ! printf '%s' "$wasm_gate_out" | grep -Fq 'no Rust build-relevant changes detected.'; then
  ok "cargo-agent-gate dry-run routes web runtime changes to wasm-check"
else
  bad "cargo-agent-gate dry-run did not route web runtime changes"
fi
rest_gate_out=$(scripts/buildbuddy-agent-gate --dry-run -- crates/meerkat-rest/src/lib.rs crates/meerkat-rpc/src/session_runtime.rs 2>&1 || true)
if printf '%s' "$rest_gate_out" | grep -Fq 'verify-schema-freshness verify-sdk-codegen-freshness verify-sdk-event-inventory verify-rpc-surface-alignment verify-rest-surface-alignment' \
  && ! printf '%s' "$rest_gate_out" | grep -Fq 'no build-relevant changes detected.'; then
  ok "buildbuddy-agent-gate dry-run routes RPC/REST surface changes to generated contract ratchets"
else
  bad "buildbuddy-agent-gate dry-run did not route RPC/REST surface changes"
fi

# ── #266: dogma doctrine mirror is a CI gate ─────────────────────────────────
echo "#266 dogma skill doctrine mirror is gated in CI:"
if grep -E '^ci:' Makefile | grep -Fq 'sync-meerkat-dogma-skill-docs' \
  && grep -E '^ci-smoke:' Makefile | grep -Fq 'sync-meerkat-dogma-skill-docs'; then
  ok "ci and ci-smoke both depend on sync-meerkat-dogma-skill-docs"
else
  bad "ci/ci-smoke do not both depend on sync-meerkat-dogma-skill-docs"
fi
if grep -E '^sync-meerkat-dogma-skill-docs:' Makefile >/dev/null; then
  ok "sync-meerkat-dogma-skill-docs Makefile target exists"
else
  bad "sync-meerkat-dogma-skill-docs Makefile target missing"
fi

# ── #221: TLC lane fails closed when tlc is absent ───────────────────────────
echo "#221 machine-verify TLC lane fails closed without tlc:"
if grep -Fq 'tlc not on PATH but this lane advertises TLC-backed verification' \
    crates/xtask/tests/machine_verify_all_tlc_test.sh \
  && grep -Fq 'MACHINE_VERIFY_TLC_DRIFT_ONLY' crates/xtask/tests/machine_verify_all_tlc_test.sh \
  && grep -Fq 'JAVA_TOOL_OPTIONS' crates/xtask/tests/machine_verify_all_tlc_test.sh \
  && grep -Fq -- '-Xss256m' crates/xtask/tests/machine_verify_all_tlc_test.sh \
  && grep -Fq -- '-XX:+UseParallelGC' crates/xtask/tests/machine_verify_all_tlc_test.sh; then
  ok "TLC test fails closed unless explicitly downgraded and gives direct witnesses the canonical JVM policy"
else
  bad "TLC test silently downgrades without tlc or omits the canonical JVM policy for direct witnesses"
fi
if grep -Fq 'fails closed when tlc is absent' scripts/buildbuddy-doctor; then
  ok "buildbuddy-doctor no longer blesses the silent no-TLC fallback as TLC"
else
  bad "buildbuddy-doctor still blesses the no-TLC drift fallback as machine-verify/TLC"
fi
# Behavioral: invoke the TLC test with tlc removed from PATH; it must exit 1
# (fail closed) rather than exit 0. We point it at a dummy xtask binary that is
# never reached because the tlc guard fires first.
if ! command -v tlc >/dev/null 2>&1; then
  set +e
  PATH="/usr/bin:/bin" bash crates/xtask/tests/machine_verify_all_tlc_test.sh /bin/true >/dev/null 2>&1
  rc=$?
  set -e
  if [ "$rc" -ne 0 ]; then
    ok "TLC test exits non-zero ($rc) when tlc is absent and drift-only opt-in is unset"
  else
    bad "TLC test exited 0 without tlc (fail-open laundering)"
  fi
else
  ok "tlc present on PATH; fail-closed branch not exercised in this environment"
fi

# Behavioral: the bounded witness is launched directly rather than through
# xtask, so the wrapper itself must preserve caller JVM options while supplying
# the canonical stack/GC defaults. A fake tlc captures the child environment;
# the host `true` binary stands in for the final xtask sweep after the witness
# succeeds.
tlc_env_tmp="$(mktemp -d "${TMPDIR:-/tmp}/machine-verify-java-options.XXXXXX")"
trap 'rm -rf "$tlc_env_tmp"' EXIT
# The fake reports TLC's success line: the lane's direct audit runners
# (durable_in_turn_steer_audit.sh, live_context_outbox_audit.sh) require it
# before they accept a run. live_context_outbox_audit.sh also requires each of
# its goals to be reported violated, so the fake reports those too.
# durable_in_turn_steer_audit.sh also requires AuditLateNeverJoins reported
# violated once per content input kind, and AuditCapacityRefusalNeverReached
# reported violated under a run contributor bound of one.
# live_unregister_cleanup_audit.sh also walks a TLC state-graph dump
# (-dump dot <base>), so the fake writes a one-state graph whose state is
# unregistered.
# live_media_health_audit.sh also requires a non-empty trace for each goal and
# firing run, so the fake prints one trace state.
# live_context_causal_tail_batch_audit.sh requires its four goals violated,
# and with --mutants each seeded defect's property violated.
# run_start_hold_audit.sh requires each of its seven reachability witnesses
# to be reported violated.
# requeue_across_stop_audit.sh requires each of its eight reachability
# witnesses to be reported violated.
# materialization_rollback_keep_audit.sh requires each of its ten
# reachability witnesses violated, and with --mutants each seeded defect's
# invariant, the generated keep_unregister_only_without_live_input among them.
# live_delegation_worker_start_after_close_audit.sh requires its two goals to
# be reported violated.
# live_context_result_barrier_audit.sh with --mutants requires its
# summary-dropped mutant to violate AuditResultFollowsSummary.
# peer_request_maps_audit.sh requires its six witnesses reported violated,
# and with --mutants each seeded defect's property or invariant violated.
# mob_operator_grant_audit.sh requires each of its seven reachability
# witnesses violated, and with --mutants each seeded defect's property.
# generation_highwater_audit.sh requires each of its six reachability
# witnesses violated, and with --mutants each seeded defect's property or
# invariant; its exact-successor mutant needs the success line above.
# member_lifecycle_audit.sh requires a final distinct-state count at or
# above its floor, a coverage block in which each of its covered actions
# generated a state, its five state witnesses and three step witnesses
# violated, and with --mutants its unguarded mutant's property violated, its
# plain-ordering mutant's evaluation error, its two replay mutants' generated
# invariants violated, its two RetireAtScope mutants' invariants violated,
# its mint mutant's action property violated, and its three destroyed-runtime
# mutants' action property violated.
# member_residency_audit.sh requires a final distinct-state count at or above
# each run's floor, a coverage block in which each covered action of its eight
# runs generated a state and its thirty-two witnesses violated; the lane
# runs it without --mutants, whose refusal lines are listed for a run by hand.
# One of its arms is printed as TLC prints an application with a state-level
# argument, with the location suffix "(L C L C)" before the ">", so the
# runner's coverage parsing stays pinned to that form.
# empty_resume_residency_audit.sh requires its four witnesses violated, and
# with --mutants each seeded defect's invariant or property violated.
# durable_tail_classification_audit.sh requires its Ready witness violated,
# and with --mutants each seeded defect's target invariant violated.
# durable_tail_recovery_ownership_audit.sh requires its four witnesses
# violated, and with --mutants each seeded defect's target invariant violated.
# entered_step_audit.sh (approval_lifecycle) requires a final distinct-state
# count at or above its floor, a coverage block in which each of its covered
# actions (of module entered_step_audit) generated a state, its three
# witnesses violated, and with --mutants each seeded defect's invariant or
# property violated.
# input_controller_hold_audit.sh requires its nine witnesses violated, and
# with --mutants each seeded defect's property or the generated
# input_controller_holds_are_kept_unstaged_governed_inputs invariant violated.
printf '%s\n' \
  '#!/bin/sh' \
  'printf "%s\n" "$JAVA_TOOL_OPTIONS" > "$TLC_JAVA_OPTIONS_CAPTURE"' \
  'printf "%s\n" "$JDK_JAVA_OPTIONS" > "$TLC_JDK_JAVA_OPTIONS_CAPTURE"' \
  'echo "Model checking completed. No error has been found."' \
  'echo "Error: Action property AuditNeverCloseEndsALeftover is violated."' \
  'echo "Error: Action property AuditNeverRecoveryEndsCarriedRows is violated."' \
  'echo "Error: Invariant AuditReplacementNeverReceivesALaterRow is violated."' \
  'echo "Error: Invariant AuditLateNeverJoins is violated."' \
  'echo "Error: Invariant AuditCapacityRefusalNeverReached is violated."' \
  'echo "Error: Invariant AuditNeverReopensAfterFailedRealization is violated."' \
  'echo "Error: Invariant NotGoalDeliveredConfirmed is violated."' \
  'echo "Error: Invariant NotGoalNotDelivered is violated."' \
  'echo "Error: Invariant NotGoalMaterialConflict is violated."' \
  'echo "Error: Invariant NotGoalMissing is violated."' \
  'echo "Error: Invariant NotAuditWitnessRefused is violated."' \
  'echo "Error: Invariant AuditNeverDeliveredWithQueuedReplay is violated."' \
  'echo "Error: Invariant AuditResultFollowsSummary is violated."' \
  'echo "Error: Invariant AuditNeverBatchAuthorized is violated."' \
  'echo "Error: Invariant AuditNeverBatchDelivered is violated."' \
  'echo "Error: Invariant AuditNeverBatchRejected is violated."' \
  'echo "Error: Invariant AuditNeverRedeliveredAfterReject is violated."' \
  'echo "Error: Action property AuditPendingRunSkipsNoQueuedRow is violated."' \
  'echo "Error: Action property AuditPendingRunCoversOnlyHeardSpeechReplays is violated."' \
  'echo "Error: Action property AuditResolveIsPinned is violated."' \
  'echo "Error: Invariant live_context_outbox_is_above_every_seed is violated."' \
  'echo "Error: Invariant NotAuditWitnessReleasedRuns is violated."' \
  'echo "Error: Invariant NotAuditWitnessRunFinishesThenRefused is violated."' \
  'echo "Error: Invariant NotAuditWitnessRetiredDrainRefused is violated."' \
  'echo "Error: Invariant NotAuditWitnessRegisteredHeldRefused is violated."' \
  'echo "Error: Invariant NotAuditWitnessStillHeldRefused is violated."' \
  'echo "Error: Invariant NotAuditWitnessStoppedHoldSurvivesResume is violated."' \
  'echo "Error: Invariant NotAuditWitnessRetainedRunsAfterGracefulStop is violated."' \
  'echo "Error: Invariant NotAuditWitnessRetainedRunsAfterCrash is violated."' \
  'echo "Error: Invariant NotAuditWitnessExpiredSettled is violated."' \
  'echo "Error: Invariant NotAuditWitnessStagedNotRetained is violated."' \
  'echo "Error: Invariant NotAuditWitnessStopAbandonsEligible is violated."' \
  'echo "Error: Invariant NotAuditWitnessPlainStopAbandons is violated."' \
  'echo "Error: Invariant NotAuditWitnessRequeuedWaitAnswered is violated."' \
  'echo "Error: Invariant NotAuditWitnessSuspendedRunResumes is violated."' \
  'echo "Error: Invariant NotAuditWitnessKeptQueuedInputRuns is violated."' \
  'echo "Error: Invariant NotAuditWitnessKeptRequeuedInputRuns is violated."' \
  'echo "Error: Invariant NotAuditWitnessAdmittedInputStoppedUnderKeep is violated."' \
  'echo "Error: Invariant NotAuditWitnessStopRollbackEndsRecoveredInput is violated."' \
  'echo "Error: Invariant AuditKeepEndsNoRecoveredInput is violated."' \
  'echo "Error: Invariant AuditKeepStopsWhatItAdmitted is violated."' \
  'echo "Error: Invariant AuditRollbackVerdictIsAFunction is violated."' \
  'echo "Error: Invariant AuditKeepOnlyUnderKeep is violated."' \
  'echo "Error: Invariant NotAuditWitnessRecycledAdmittedInputStoppedUnderKeep is violated."' \
  'echo "Error: Invariant NotAuditWitnessDedupedInputStoppedUnderKeep is violated."' \
  'echo "Error: Invariant NotAuditWitnessDrainingLiveWorkStopsRecoveredInput is violated."' \
  'echo "Error: Invariant AuditLiveWorkForfeitsKeep is violated."' \
  'echo "Error: Invariant NotAuditWitnessUnservedAbortKeepsRecoveredInput is violated."' \
  'echo "Error: Invariant NotAuditWitnessUnservedRetireStopsRecoveredInput is violated."' \
  'echo "Error: Invariant NotAuditWitnessUnservedAbortAfterLiveWorkStops is violated."' \
  'echo "Error: Invariant AuditRetireOverUnservedStops is violated."' \
  'echo "Error: Invariant keep_unregister_only_without_live_input is violated."' \
  'echo "Error: Action property AuditNeverUnregisters is violated."' \
  'echo "Error: Action property AuditNeverUnregistersWithPreparation is violated."' \
  'echo "Error: Invariant AuditNeverCancelsRecovery is violated."' \
  'prev=""; for a in "$@"; do if [ "$prev" = "dot" ]; then printf "%s\n" "1 [label=\"\\n/\\\\ session_id = [tag |-> \\\"none\\\"]\"]" > "$a.dot"; fi; prev="$a"; done' \
  'echo "Error: Invariant NotGoalAudible is violated."' \
  'echo "Error: Invariant AuditNeverResolvedAfterClose is violated."' \
  'echo "Error: Invariant AuditNeverSettledAfterClose is violated."' \
  'echo "Error: Invariant NotGoalSilentReopen is violated."' \
  'echo "Error: Invariant NotGoalSilentExhausted is violated."' \
  'echo "Error: Invariant NotGoalFaultedChannelReportsClosed is violated."' \
  'echo "Error: Invariant NotGoalReRegisteredSilentReopen is violated."' \
  'echo "Error: Invariant NotGoalUnregisteredAfterFault is violated."' \
  'echo "Error: Action property NeverRequestAttached is violated."' \
  'echo "Error: Action property NeverRequestRunning is violated."' \
  'echo "Error: Action property NeverAudibleAttached is violated."' \
  'echo "Error: Action property NeverAudibleRunning is violated."' \
  'echo "Error: Action property NeverSilentReopenAttached is violated."' \
  'echo "Error: Action property NeverSilentReopenRunning is violated."' \
  'echo "Error: Action property NeverSilentExhaustedAttached is violated."' \
  'echo "Error: Action property NeverSilentExhaustedRunning is violated."' \
  'echo "Error: Invariant NotAuditWitnessAnswered is violated."' \
  'echo "Error: Invariant NotAuditWitnessProgressThenAnswered is violated."' \
  'echo "Error: Invariant NotAuditWitnessNotAddressee is violated."' \
  'echo "Error: Invariant NotAuditWitnessUnknownAfterAnswer is violated."' \
  'echo "Error: Invariant NotAuditWitnessReplied is violated."' \
  'echo "Error: Invariant NotAuditWitnessNotRequester is violated."' \
  'echo "Error: Action property AuditResponsesOnlyFromAddressee is violated."' \
  'echo "Error: Action property AuditSenderVerdictExact is violated."' \
  'echo "Error: Action property AuditAddresseeVerdictExact is violated."' \
  'echo "Error: Action property AuditRequesterStable is violated."' \
  'echo "Error: Invariant pending_peer_request_addressees_match_requests is violated."' \
  'echo "Error: Invariant inbound_peer_request_requesters_match_requests is violated."' \
  'echo "Error: Invariant NotAuditWitnessRebound is violated."' \
  'echo "Error: Invariant NotAuditWitnessRebindDroppedSpawn is violated."' \
  'echo "Error: Invariant NotAuditWitnessRebindDroppedManage is violated."' \
  'echo "Error: Invariant NotAuditWitnessBothScopes is violated."' \
  'echo "Error: Invariant NotAuditWitnessTwoIncarnations is violated."' \
  'echo "Error: Invariant NotAuditWitnessRestoredBindings is violated."' \
  'echo "Error: Invariant NotAuditWitnessResolveCleared is violated."' \
  'echo "Error: Action property AuditRebindDropsTheOtherScope is violated."' \
  'echo "Error: Invariant mob_operator_scopes_are_bound_to_an_incarnation is violated."' \
  'echo "Error: Invariant NotAuditWitnessRecorded is violated."' \
  'echo "Error: Invariant NotAuditWitnessRaised is violated."' \
  'echo "Error: Invariant NotAuditWitnessSealedWithHighwater is violated."' \
  'echo "Error: Invariant NotAuditWitnessRetainedBelowHighwater is violated."' \
  'echo "Error: Invariant NotAuditWitnessFirstRecoveredAboveZero is violated."' \
  'echo "Error: Invariant NotAuditWitnessResetGap is violated."' \
  'echo "Error: Invariant next_generation_above_highwater is violated."' \
  'echo "Error: Action property AuditSealClosesRecovery is violated."' \
  'echo "Error: Action property AuditHighwaterNeverLowers is violated."' \
  'echo "Error: Invariant NotAuditWitnessBound is violated."' \
  'echo "Error: Action property NotAuditWitnessObservationAdvances is violated."' \
  'echo "Error: Invariant NotAuditWitnessRetiring is violated."' \
  'echo "Error: Invariant NotAuditWitnessArchived is violated."' \
  'echo "Error: Action property NotAuditWitnessSuccessor is violated."' \
  'echo "Error: Action property NotAuditWitnessStaleIncarnation is violated."' \
  'echo "Error: Invariant NotAuditWitnessRetiringScopeCurrent is violated."' \
  'echo "Error: Invariant NotAuditWitnessCurrentRuntimeGone is violated."' \
  'echo "Error: Invariant AuditRetireAtScopeCoherent is violated."' \
  'echo "Error: Invariant AuditRetireAtScopeExactlyOne is violated."' \
  'echo "Error: Invariant pending_session_ingress_detach_has_session_correlation is violated."' \
  'echo "Error: Invariant pending_respawn_topology_has_no_current_runtime is violated."' \
  'echo "Error: Action property AuditObservationMonotonic is violated."' \
  'echo "Error: Action property AuditMintClearsExecutionHealth is violated."' \
  'echo "Error: Action property AuditRuntimeDestroyedRemovesOnlyItsRuntime is violated."' \
  'echo "Error: Invariant NotAuditWitnessReady is violated by the initial state:"' \
  'echo "Error: Invariant AuditClassificationIsTotalAndDisjoint is violated by the initial state:"' \
  'echo "Error: Invariant AuditRewriteClassifiesLikeStrict is violated by the initial state:"' \
  'echo "Error: Invariant AuditRewriteWithoutOneRunIdHolds is violated by the initial state:"' \
  'echo "Error: Invariant AuditRewriteEndTurnCompletes is violated by the initial state:"' \
  'echo "Error: Invariant AuditRewriteToolUseRepairs is violated by the initial state:"' \
  'echo "Error: Invariant AuditRewriteUnattributedHolds is violated by the initial state:"' \
  'echo "Error: Invariant AuditDisagreeingCursorHolds is violated by the initial state:"' \
  'echo "Error: Invariant AuditCursorOnEndTurnHolds is violated by the initial state:"' \
  'echo "Error: Invariant AuditContinuableNeedsAgreeingBoundCursor is violated by the initial state:"' \
  'echo "Error: Invariant NotAuditWitnessIdle is violated."' \
  'echo "Error: Invariant NotAuditWitnessRetired is violated."' \
  'echo "Error: Invariant NotAuditWitnessTerminalized is violated."' \
  'echo "Error: Invariant NotAuditWitnessOtherTerminal is violated."' \
  'echo "Error: Invariant AuditRecoveryIsTotalAndDisjoint is violated."' \
  'echo "Error: Invariant AuditUnprovenOwnershipGatesAsBefore is violated."' \
  'echo "Error: Invariant AuditOwnershipNeverAdmitsAnotherRunOrAnUndecodableRow is violated."' \
  'echo "Error: Invariant AuditHostRecoversAsARestartDoes is violated."' \
  'echo "Error: Invariant AuditWriterRecoversAsRetired is violated."' \
  'echo "Error: Invariant AuditWriterNeverContinues is violated."' \
  'echo "Error: Invariant NotAuditWitnessSealedDormant is violated."' \
  'echo "Error: Invariant NotAuditWitnessSealedEndpointPending is violated."' \
  'echo "Error: Invariant NotAuditWitnessStaleInStopped is violated."' \
  'echo "Error: Invariant NotAuditWitnessDrift is violated."' \
  'echo "Error: Invariant NotAuditWitnessBrokenWake is violated."' \
  'echo "Error: Invariant NotAuditWitnessHeldEdge is violated."' \
  'echo "Error: Invariant NotAuditWitnessEdgeRepairOwed is violated."' \
  'echo "Error: Action property NotAuditWitnessUnbornCreated is violated."' \
  'echo "Error: Action property NotAuditWitnessUnbornArchived is violated."' \
  'echo "Error: Invariant NotAuditWitnessSeatUnborn is violated."' \
  'echo "Error: Action property NotAuditWitnessRotatedUnborn is violated."' \
  'echo "Error: Action property NotAuditWitnessReseatCreated is violated."' \
  'echo "Error: Invariant NotAuditWitnessRotationGrant is violated."' \
  'echo "Error: Invariant NotAuditWitnessBothSeated is violated."' \
  'echo "Error: Invariant NotAuditWitnessSeatEndpointPending is violated."' \
  'echo "Error: Action property NotAuditWitnessWokeLive is violated."' \
  'echo "Error: Action property NotAuditWitnessReleased is violated."' \
  'echo "Error: Action property NotAuditWitnessStopCloses is violated."' \
  'echo "Error: Action property NotAuditWitnessRetireCloses is violated."' \
  'echo "Error: Action property NotAuditWitnessResumeJoins is violated."' \
  'echo "Error: Action property NotAuditWitnessResumeReservesLive is violated."' \
  'echo "Error: Action property NotAuditWitnessResumeFinishedLive is violated."' \
  'echo "Error: Action property NotAuditWitnessResumeJoinsAfterReadiness is violated."' \
  'echo "Error: Action property NotAuditWitnessRotated is violated."' \
  'echo "Error: Action property NotAuditWitnessReleasedSpawn is violated."' \
  'echo "Error: Action property NotAuditWitnessArchiveOnlyArchived is violated."' \
  'echo "Error: Action property NotAuditWitnessDestroyBeatsRotation is violated."' \
  'echo "Error: Action property NotAuditWitnessDestroyBeatsHostPending is violated."' \
  'echo "Error: Action property NotAuditWitnessRotatedStopped is violated."' \
  'echo "Error: Action property NotAuditWitnessCrossNotice is violated."' \
  'echo "Error: Action property NotAuditWitnessRetireBesideWaking is violated."' \
  'echo "Error: Action property NotAuditWitnessCommsLessWokeLive is violated."' \
  'echo "Error: Invariant materialization_domain_is_session_bound_membership is violated."' \
  'echo "Error: Invariant local_session_bound_member_is_live_or_wakeable is violated."' \
  'echo "Error: Invariant broken_member_has_no_phase is violated."' \
  'echo "Error: Invariant dormant_member_has_durable_endpoint is violated."' \
  'echo "Error: Invariant endpoint_pending_member_has_no_endpoint is violated."' \
  'echo "Error: Invariant dormant_member_requests_no_runtime_binding is violated."' \
  'echo "Error: Invariant live_member_records_its_epoch is violated."' \
  'echo "Error: Invariant waking_is_one_attempt is violated."' \
  'echo "Error: Invariant retiring_incarnation_never_waking is violated."' \
  'echo "Error: Invariant candidate_composition_never_wakes is violated."' \
  'echo "Error: Invariant waking_has_an_open_attempt is violated."' \
  'echo "Error: Invariant attempt_and_epoch_follow_the_phase is violated."' \
  'echo "Error: Invariant only_a_running_mob_wakes is violated."' \
  'echo "Error: Invariant AuditWakingClosable is violated."' \
  'echo "Error: Invariant AuditResumeJoinable is violated."' \
  'echo "Error: Invariant AuditResumeStagesHoldNoWork is violated."' \
  'echo "Error: Action property AuditResumeReservesBeforeReadiness is violated."' \
  'echo "Error: Invariant AuditWakeExactlyOne is violated."' \
  'echo "Error: Invariant wiring_edges_connect_seated_members is violated."' \
  'echo "Error: Invariant rotation_intent_is_a_retiring_member is violated."' \
  'echo "Error: Invariant member_peer_id_ownership_is_global_across_generations is violated."' \
  'echo "Error: Action property AuditRecoverKeepsPhase is violated."' \
  'echo "Error: Action property AuditAttemptEndsClosed is violated."' \
  'echo "Error: Action property AuditLifecycleNeverWakes is violated."' \
  'echo "Error: Action property AuditWakeNeverSkipsWaking is violated."' \
  'echo "Error: Action property AuditSucceededLeavesEndpoint is violated."' \
  'echo "Error: Action property AuditReleaseEpochFence is violated."' \
  'echo "Error: Action property AuditRetireRoutesOnlyRegistered is violated."' \
  'echo "Error: Action property AuditDestroyFencesRotation is violated."' \
  'echo "Error: Invariant AuditRetirementArchivable is violated."' \
  'echo "Error: Invariant archive_only_retirement_is_retiring is violated."' \
  'echo "Error: Invariant destroyed_mob_holds_no_member_lifecycle is violated."' \
  'echo "Error: Invariant AuditWakeClosesLiveUnlessEndpointOwed is violated."' \
  'echo "Error: Invariant AuditReleaseVerdictMatchesArm is violated."' \
  'echo "Error: Action property AuditBindingRefusalFenced is violated."' \
  'echo "Error: Action property AuditPlanLeavesSealable is violated."' \
  'echo "Error: Invariant owed_edge_repairs_join_seated_members is violated."' \
  'echo "Error: Invariant seat_session_unborn_is_seated is violated."' \
  'echo "Error: Invariant seat_session_unborn_is_never_bound is violated."' \
  'echo "Error: Action property AuditFreshCreateOnlyUnborn is violated."' \
  'echo "Error: Invariant unborn_seat_session_is_recorded is violated."' \
  'echo "Error: Action property AuditReseatUnbornIffNeverBorn is violated."' \
  'echo "Error: Invariant spawn_profile_authority_is_a_pending_grant is violated."' \
  'echo "Error: Action property NotAuditWitnessSessionOwnedRetires is violated."' \
  'echo "Error: Action property NotAuditWitnessCompositionKeepsServing is violated."' \
  'echo "Error: Action property NotAuditWitnessQueuedKeepsServing is violated."' \
  'echo "Error: Action property NotAuditWitnessCompositionClaimed is violated."' \
  'echo "Error: Invariant AuditVerdictExactlyOne is violated."' \
  'echo "Error: Action property AuditPlacementOwnership is violated."' \
  'echo "Error: Assumption line 1, col 1 to line 1, col 2 of module member_residency_audit is false."' \
  'echo "Error: The first argument of <= should be an integer, but instead it is:"' \
  'echo "200000 states generated, 200000 distinct states found, 0 states left on queue."' \
  'echo "The coverage statistics at 2026-01-01 00:00:00"' \
  'for action in RecoverRosterMemberRunning RecoverMemberSessionBindingFreshRunning ObserveMemberProgressChangedOpenRunning ObserveMemberProgressChangedIdleRunning ObserveMemberProgressUnchangedIdleRunning ObserveMemberProgressUnchangedOpenHealthyRunning ObserveMemberProgressUnchangedOpenDegradedRunning ObserveMemberProgressUnchangedOpenWedgedRunning ObserveMemberProgressStaleRunning RetireRunningReleasing RetireRunningPreservingBinding RetireRunningNoBinding RetireStoppedReleasing RetireStoppedPreservingBinding RetireStoppedNoBinding RetireAbsentRunning RetireAbsentStopped RetireMember SessionIngressDetachedForMobDestroyRunning ObserveRuntimeRetired ObserveRuntimeRetiredStopped ObserveMemberRetirementArchivedLive ObserveMemberRetirementArchivedRetired ObserveMemberRetirementArchivedLiveStopped ObserveMemberRetirementArchivedRetiredStopped ObserveMemberRetirementArchivedAlreadyCleared ObserveMemberRetirementArchivedStaleRuntimeAlreadyCleared ObserveMemberRetirementArchivedAlreadyClearedStopped ObserveMemberRetirementArchivedStaleRuntimeAlreadyClearedStopped BeginPlacedCompletionLifecycleQuiesceFreshStop StopRunning ResumeStopped BeginPlacedCompletionLifecycleQuiesceFresh MarkCompleted DestroyMob RetireAtScopeCurrentRunning RetireAtScopeCurrentStopped RetireAtScopeNoCurrentIncarnationRunning RetireAtScopeNoCurrentIncarnationStopped RetireAtScopeStaleFenceTokenRunning RetireAtScopeStaleFenceTokenStopped RetireAtScopeStaleSessionBindingRunning RetireAtScopeStaleSessionBindingStopped RetireAtScopeMobNotRunningCompleted RetireAtScopeMobNotRunningDestroyed RecoverRosterMemberResetRunning RecoverMemberSessionBindingReplacingRunning RecoverMemberSessionBindingAlreadyCurrentRunning RetireMemberPeerOnly SessionIngressDetachedForMobDestroyStopped RecoverRosterMemberRetiringRunning RecoverRosterMemberRetiringReplay RecoverRosterMemberRetiredRunning RecoverRosterMemberRetiredCurrentAlreadyAbsent RecoverRosterMemberRetiredStaleGeneration RecoverRosterMemberRetiredStaleIncarnation RecoverRosterMemberRetirementStartedReleasing RecoverRosterMemberRetirementStartedReleasingAlreadyApplied RecoverRosterMemberRetirementStartedPreservingBinding RecoverRosterMemberRetirementStartedPeerOnly RecoverRosterMemberAddressabilityRunning ObserveMemberRetirementArchivedLivePreservingTopology ObserveMemberRetirementArchivedRetiredPreservingTopology ObserveRuntimeDestroyed ObserveRuntimeDestroyedStopped ObserveRuntimeDestroyedCompleted AdmitDestroyCleanup AuditStartRun FinishRunRunning; do echo "<$action line 1, col 1 to line 1, col 2 of module model>: 1:1"; done' \
  'for action in RecoverRosterMemberRunning RecoverMemberSessionBindingFreshRunning RecoverMemberSessionBindingAlreadyCurrentRunning RecoverMemberPeerEndpointFreshRunning RegisterMemberPeerRunning RecoverRosterMemberRetiredRunning RecoverMemberRestoreFailureRunning RecoverMemberRestoreHostRefusedRunning SeedMemberResidencyRunning SeedMemberResidencyStopped SeedMemberResidencyCompleted ResolveMemberHostPreparationRunning ResolveMemberHostPreparationStopped ResolveMemberHostPreparationCompleted PlanBootMaterializationWake PlanBootMaterializationVerify PlanBootMaterializationStayDormant SealBootMaterializationPlan WakeMemberAuthorizedRunning WakeMemberJoinedRunning WakeMemberAlreadyLiveRunning WakeMemberRefusedRunning WakeMemberRefusedStopped WakeMemberRefusedCompleted WakeMemberRefusedDestroyed ResolveMemberRevivalBoundRunning ResolveMemberRevivalSucceededRunningLocal ResolveMemberRevivalFailedRunning ResolveMemberRevivalHostRefusedRunning ResolveMemberRevivalRetryableRunning AbandonMemberWakeDormantRunning AbandonMemberWakeBrokenRunning ResolveRecreatedMemberSessionSucceededRunningLocal ResolveMemberRevivalBoundStaleStopped ResolveMemberRevivalBoundStaleCompleted ResolveMemberRevivalBoundStaleDestroyed ResolveMemberRevivalSucceededStaleRunning ResolveMemberRevivalSucceededStaleStopped ResolveMemberRevivalSucceededStaleCompleted ResolveMemberRevivalSucceededStaleDestroyed ResolveMemberRevivalFailedStaleRunning ResolveMemberRevivalFailedStaleStopped ResolveMemberRevivalFailedStaleCompleted ResolveMemberRevivalFailedStaleDestroyed ResolveMemberRevivalHostRefusedStaleRunning ResolveMemberRevivalHostRefusedStaleStopped ResolveMemberRevivalHostRefusedStaleCompleted ResolveMemberRevivalHostRefusedStaleDestroyed ResolveMemberRevivalRetryableStaleRunning ResolveMemberRevivalRetryableStaleStopped ResolveMemberRevivalRetryableStaleCompleted ResolveMemberRevivalRetryableStaleDestroyed AbandonMemberWakeStaleRunning AbandonMemberWakeStaleStopped AbandonMemberWakeStaleCompleted AbandonMemberWakeStaleDestroyed ResolveRecreatedMemberSessionSucceededStaleRunning ResolveRecreatedMemberSessionSucceededStaleStopped ResolveRecreatedMemberSessionSucceededStaleCompleted ResolveRecreatedMemberSessionSucceededStaleDestroyed ResolveMemberRevivalSessionCreatedRunning ResolveMemberRevivalSessionCreatedStaleRunning ResolveMemberRevivalSessionCreatedStaleStopped ResolveMemberRevivalSessionCreatedStaleCompleted ResolveMemberRevivalSessionCreatedStaleDestroyed RecoverMemberSeatSessionUnbornRunning RecoverMemberSeatSessionBornRunning AuthorizeSpawnProfileSeatedRunning ObserveRuntimeReady ReleaseMemberMaterializationReleasedRunning ReleaseMemberMaterializationReleasedStopped ReleaseMemberMaterializationReleasedCompleted ReleaseMemberMaterializationStaleRunning ReleaseMemberMaterializationStaleStopped ReleaseMemberMaterializationStaleCompleted ReleaseMemberMaterializationStaleDestroyed ClassifyMemberLiveMaterializationRevivable ClassifyMemberLiveMaterializationTerminal ClassifyMemberLiveMaterializationTerminalDormant ClassifyPeerNoticeRunning ClassifyPeerNoticeStopped ClassifyPeerNoticeCompleted ClassifyPeerNoticeDestroyed BeginPlacedCompletionLifecycleQuiesceFreshStop StopRunning ResumeStopped BeginPlacedCompletionLifecycleQuiesceFresh CompleteRunning MarkCompleted AdmitDestroyCleanup DestroyMob ShutdownRunning RetireRunningReleasing RetireRunningPreservingBinding RetireStoppedReleasing SessionIngressDetachedForMobDestroyRunning SessionIngressDetachedForMobDestroyStopped ObserveRuntimeRetired ObserveRuntimeRetiredStopped ObserveMemberRetirementArchivedLive ObserveMemberRetirementArchivedRetired ObserveMemberRetirementArchivedLiveStopped ObserveMemberRetirementArchivedRetiredStopped RecoverMemberSessionBindingReplacingRunning BeginExplicitResumeStopped SettleExplicitResumePreparationStopped BeginExplicitResumeReadinessStopped SettleExplicitResumeReadinessStopped AuthorizeExplicitResumeMemberRunning AuthorizeExplicitResumeMemberLiveRunning AuthorizeExplicitResumeMemberJoinRunning ClassifyExplicitResumeMemberLiveRevivable ClassifyExplicitResumeMemberLiveMissing SettleExplicitResumeMemberRunning SettleExplicitResumeMemberReplacedRunning BeginExplicitResumeReadinessRunning SettleExplicitResumeReadinessRunning BeginExplicitResumeTopologyRunning SettleExplicitResumeTopologyRunning FinishExplicitResumeRunning RecoverRosterWiringRunning WireMembersRunning ObserveMemberRetirementArchivedLivePreservingTopology ResolveWiringEdgeRealizationRealizedRunning ResolveWiringEdgeRealizationOwedRunning ResolveWiringEdgeRealizationStaleRunning AuthorizeSpawnProfileRunning BeginSpawnExecFresh CommitDormantSeatRunning CommitSpawnMembershipFresh BeginMemberRotationRunning CommitMemberRotationLiveRunning CommitMemberRotationRetiredRunning RetireRunningReleasingArchiveOnly RetireRunningPreservingBindingArchiveOnly RetireStoppedReleasingArchiveOnly RetireStoppedPreservingBindingArchiveOnly RetireMemberArchiveOnly RetireStoppedPreservingBinding RetireMember RetryRuntimeRetireRunning RetryRuntimeRetireStopped RecoverRosterMemberRetirementStartedReleasing RecoverRosterMemberRetirementStartedPreservingBinding CommitMemberRotationLiveStopped CommitMemberRotationRetiredStopped AdmitDestroyMemberRetireLiveRunning AdmitDestroyMemberRetireArchiveOnlyRunning AdmitDestroyMemberRetireAlreadyRetiringSessionRunning AdmitDestroyMemberRetireAlreadyRetiringReleasedSessionRunning ObserveDestroyMemberRetirementArchivedLiveRunning ObserveDestroyMemberRetirementArchivedRetiredRunning ClassifyMemberReleaseRunning ClassifyMemberReleaseStopped ClassifyMemberReleaseCompleted ClassifyMemberReleaseDestroyed RecoverSpawnedMemberPeerEndpointFreshRunning ResolveRuntimeBindingRefusalRunning ResolveRuntimeBindingRefusalStaleRunning ResolveRuntimeBindingRefusalStaleStopped ResolveRuntimeBindingRefusalStaleCompleted ResolveRuntimeBindingRefusalStaleDestroyed; do echo "<$action line 1, col 1 to line 1, col 2 of module model>: 1:1"; done' \
  'echo "<ResolveMemberRevivalBoundStaleRunning line 1, col 1 to line 1, col 62 of module model (210 8 210 78)>: 0:490"' \
  'for action in AuditConsumeReviewForEntry AuditConsumeConsentForEntry AuditConsumeReviewRejectedUsed AuditConsumeConsentRejectedAlreadyConsumed AuditAdmitEnteredStep AuditAdmitEnteredStepRejectedMissing AuditAdmitEnteredStepRejectedConsentLinked AuditAdmitEnteredStepRejectedRetired AuditAdmitEnteredStepRejectedNotEntered AuditAdmitEnteredStepRejectedClosing AuditFinishEnteredStepOpen AuditFinishEnteredStepClosing AuditFinishEnteredStepReleases AuditFinishEnteredStepRejectedMissing AuditFinishEnteredStepRejectedNotInFlight AuditReleaseReviewUsed AuditReleaseReviewUsedClosing AuditReleaseReviewRejectedClosing AuditReleaseReviewRejectedMissing AuditRetireReviewRejectedSettled AuditWithdrawConsentRejectedMismatch; do echo "<$action line 1, col 1 to line 1, col 2 of module entered_step_audit>: 1:1"; done' \
  'echo "Error: Invariant NotAuditWitnessCancelWhileStepInFlight is violated."' \
  'echo "Error: Invariant NotAuditWitnessRefusedAfterRelease is violated."' \
  'echo "Error: Invariant NotAuditWitnessGraphAndTwoChildrenAfterConsent is violated."' \
  'echo "Error: Invariant entered_state_covers_exactly_the_used_attempts is violated."' \
  'echo "Error: Invariant closing_entered_effect_has_a_step_in_flight is violated."' \
  'echo "Error: Invariant AuditOneSpendPerAttempt is violated."' \
  'echo "Error: Invariant AuditStepOnlyAfterSpend is violated."' \
  'echo "Error: Invariant AuditNoStepAfterDispatchEnd is violated."' \
  'echo "Error: Invariant AuditFinishesNeverExceedAdmissions is violated."' \
  'echo "Error: Invariant AuditReleaseWaitsForSteps is violated."' \
  'echo "Error: Invariant AuditEnteredStateMatchesHistory is violated."' \
  'echo "Error: Action property AuditStepsNeverSpendOrSettle is violated."' \
  'echo "Error: Invariant NotAuditWitnessHeldWhileRunCurrent is violated."' \
  'echo "Error: Invariant NotAuditWitnessReleasedThenStaged is violated."' \
  'echo "Error: Invariant NotAuditWitnessHeldEndsRefused is violated."' \
  'echo "Error: Invariant NotAuditWitnessHeldRunWaits is violated."' \
  'echo "Error: Invariant NotAuditWitnessReleasedRunResumes is violated."' \
  'echo "Error: Invariant NotAuditWitnessSettledDropsHold is violated."' \
  'echo "Error: Invariant NotAuditWitnessReleasedRunRequeues is violated."' \
  'echo "Error: Invariant NotAuditWitnessResumeWaitsForDestination is violated."' \
  'echo "Error: Invariant NotAuditWitnessResumedUnderItsDestination is violated."' \
  'echo "Error: Action property AuditHeldWorkNeverAdvances is violated."' \
  'echo "Error: Action property AuditHoldAndReleaseTouchOnlyHolds is violated."' \
  'echo "Error: Action property AuditResumedRunWaitsForDestination is violated."' \
  'echo "Error: Action property AuditResumedRunBindsItsDestination is violated."' \
  'echo "Error: Invariant input_controller_holds_are_kept_unstaged_governed_inputs is violated."' \
  'echo "State 1: <Initial predicate>"' \
  > "$tlc_env_tmp/tlc"
chmod +x "$tlc_env_tmp/tlc"

capture="$tlc_env_tmp/captured"
jdk_capture="$tlc_env_tmp/captured-jdk"
true_bin="$(type -P true)"
# show_tail <label> <output>: the end of a failed run's output, so a failure
# names its cause in the log.
show_tail() { printf '%s\n' "$2" | tail -n 12 | sed "s/^/    $1| /"; }
if witness_out="$(PATH="$tlc_env_tmp:$PATH" \
    TLC_JAVA_OPTIONS_CAPTURE="$capture" \
    TLC_JDK_JAVA_OPTIONS_CAPTURE="$jdk_capture" \
    JAVA_TOOL_OPTIONS='-Dmeerkat.sentinel=true' \
    bash crates/xtask/tests/machine_verify_all_tlc_test.sh "$true_bin" 2>&1)" \
  && grep -Fqw -- '-Dmeerkat.sentinel=true' "$capture" \
  && [ "$(tr ' ' '\n' < "$capture" | grep -Fxc -- '-Xss256m')" -eq 1 ] \
  && [ "$(tr ' ' '\n' < "$capture" | grep -Fxc -- '-XX:+UseParallelGC')" -eq 1 ]; then
  ok "direct TLC witness preserves caller JVM options and adds canonical stack/GC defaults"
else
  bad "direct TLC witness did not merge caller JVM options with canonical stack/GC defaults"
  show_tail witness "$witness_out"
fi

# The launcher sizes TLC's main thread (module parse, initial states) from
# JDK_JAVA_OPTIONS only; -Xss in JAVA_TOOL_OPTIONS reaches JVM-created threads.
# The witness must hand the same stack flag to the launcher layer.
if [ "$(tr ' ' '\n' < "$jdk_capture" | grep -Fxc -- '-Xss256m')" -eq 1 ]; then
  ok "direct TLC witness gives the launcher main thread the canonical stack through JDK_JAVA_OPTIONS"
else
  bad "direct TLC witness leaves the launcher main thread on the default stack (JDK_JAVA_OPTIONS lacks -Xss256m)"
fi

if witness_out="$(PATH="$tlc_env_tmp:$PATH" \
    TLC_JAVA_OPTIONS_CAPTURE="$capture" \
    TLC_JDK_JAVA_OPTIONS_CAPTURE="$jdk_capture" \
    JAVA_TOOL_OPTIONS='-Dmeerkat.sentinel=true -Xss8m -XX:+UseParallelGC' \
    bash crates/xtask/tests/machine_verify_all_tlc_test.sh "$true_bin" 2>&1)" \
  && [ "$(tr ' ' '\n' < "$capture" | grep -Fxc -- '-Xss8m')" -eq 1 ] \
  && ! grep -Fqw -- '-Xss256m' "$capture" \
  && [ "$(tr ' ' '\n' < "$capture" | grep -Fxc -- '-XX:+UseParallelGC')" -eq 1 ] \
  && [ "$(tr ' ' '\n' < "$jdk_capture" | grep -Fxc -- '-Xss8m')" -eq 1 ] \
  && ! grep -Fqw -- '-Xss256m' "$jdk_capture"; then
  ok "direct TLC witness preserves explicit stack policy on both JVM layers without duplicating defaults"
else
  bad "direct TLC witness replaced explicit stack policy, duplicated JVM defaults, or left the launcher layer inconsistent"
  show_tail witness "$witness_out"
fi

# The lane runs as parts (Bazel runs each as its own target): every hand
# audit runs in exactly one audit shard, each shard runs at least one, the
# machine-verify part runs none, and the shards together run exactly the
# audits of the whole lane (the member audits and the MobMachine arm
# coverage gate included). Every part runs and keeps its output whatever the
# others do: a failed part fails this check by name, and no later check
# reads an output its failure left unset (one such read ended in "a_out:
# unbound variable").
lane_part_output() {
  PATH="$tlc_env_tmp:$PATH" \
    TLC_JAVA_OPTIONS_CAPTURE="$capture" \
    TLC_JDK_JAVA_OPTIONS_CAPTURE="$jdk_capture" \
    bash crates/xtask/tests/machine_verify_all_tlc_test.sh "$true_bin" "$@" 2>&1
}
audit_lines() {
  grep -E '^running (bounded .* TLC audit|member [a-z]+ TLC audit|MobMachine arm coverage gate)$' | sort
}
audit_shards="audits-a audits-b audits-c audits-d audits-e audits-f audits-g audits-h"
shard_outputs="$(mktemp -d)"
failed_parts=""
idle_shards=""
all_out="$(lane_part_output)" || failed_parts+=" all"
for shard in $audit_shards; do
  lane_part_output --part "$shard" > "$shard_outputs/$shard" || failed_parts+=" $shard"
  [ -n "$(audit_lines < "$shard_outputs/$shard")" ] || idle_shards+=" $shard"
done
mv_out="$(lane_part_output --part machine-verify)" || failed_parts+=" machine-verify"
# A shard without an audit is reported above, so its empty grep must not end
# the script here.
shard_audits="$(for shard in $audit_shards; do audit_lines < "$shard_outputs/$shard" || true; done | sort)"
if [ -z "$failed_parts" ] && [ -z "$idle_shards" ] \
  && [ -z "$(printf '%s\n' "$mv_out" | audit_lines)" ] \
  && printf '%s\n' "$mv_out" | grep -Fq 'running bounded adaptive_mob_bundle layer_terminal_feedback TLC witness' \
  && ! (for shard in $audit_shards; do cat "$shard_outputs/$shard"; done) | grep -Fq 'running bounded adaptive_mob_bundle' \
  && [ -z "$(printf '%s\n' "$shard_audits" | uniq -d)" ] \
  && [ "$shard_audits" = "$(printf '%s\n' "$all_out" | audit_lines)" ]; then
  ok "TLC lane parts: each hand audit runs in exactly one shard, every shard runs one, machine-verify runs none, and the shards cover the whole lane"
else
  bad "TLC lane parts drop, duplicate, or misplace a hand audit, leave a shard without one, or the machine-verify part runs one${failed_parts:+ (failed parts:${failed_parts})}${idle_shards:+ (shards without an audit:${idle_shards})}"
  for part in $failed_parts; do
    case "$part" in
      all) show_tail all "$all_out" ;;
      machine-verify) show_tail machine-verify "$mv_out" ;;
      *) show_tail "$part" "$(cat "$shard_outputs/$part")" ;;
    esac
  done
fi

# The member residency runner counts an arm printed with TLC's location
# suffix (the fake above prints one so; the lane parts run it), and still
# refuses a covered arm that is absent from the coverage block: a copy of the
# fake without that arm's line must fail the runner on exactly that arm.
absent_tlc_tmp="$(mktemp -d)"
grep -vF 'ResolveMemberRevivalBoundStaleRunning line 1, col 1 to line 1, col 62 of module model (210 8 210 78)' \
  "$tlc_env_tmp/tlc" > "$absent_tlc_tmp/tlc"
chmod +x "$absent_tlc_tmp/tlc"
absent_rc=0
absent_out="$(PATH="$absent_tlc_tmp:$PATH" \
  TLC_JAVA_OPTIONS_CAPTURE="$absent_tlc_tmp/capture" \
  TLC_JDK_JAVA_OPTIONS_CAPTURE="$absent_tlc_tmp/capture-jdk" \
  TLC_WORKERS=1 bash specs/machines/mob_machine/member_residency_audit.sh 2>&1)" || absent_rc=$?
if [ "$absent_rc" != 0 ] \
  && printf '%s\n' "$absent_out" \
    | grep -Fq "wake run: ResolveMemberRevivalBoundStaleRunning never fired (coverage 'absent')"; then
  ok "member residency audit refuses a covered arm absent from TLC's coverage block"
else
  bad "member residency audit accepted a coverage block without one of its covered arms (rc=$absent_rc)"
  show_tail residency-absent "$absent_out"
fi
rm -rf "$absent_tlc_tmp"

# tlc_run_cap.sh: the per-run watchdog must never outlive a run that ended
# first, or hold a caller's `$(...)` capture open. A sleep orphaned between
# its start and the recording of its pid used to block such a capture for
# the whole cap (900 s). The watchdog is its own process group, killed as a
# group, with its stdio on /dev/null. Each probe runs an instant TLC inside
# a capture, in a child bounded by `timeout` that writes to a file, so a
# regression fails here in seconds instead of hanging this script.
# cap_probe <tlc_run_cap.sh>: succeeds when the capture returned rc=0
# within 5 s.
cap_fake_bin="$(mktemp -d)"
printf '#!/bin/sh\nexit 0\n' > "$cap_fake_bin/tlc"
chmod +x "$cap_fake_bin/tlc"
cap_probe() {
  local start_s rc
  start_s="$(date +%s)"
  rc=0
  PATH="$cap_fake_bin:$PATH" TLC_RUN_CAP_SECS=120 timeout 20 bash -c \
    'out="$(source "$1"; tlc_run_capped selftest cfg "$2/log"; echo "rc=$?")"; printf "%s" "$out"' \
    _ "$1" "$cap_fake_bin" > "$cap_fake_bin/out" 2>/dev/null || rc=$?
  [ "$rc" = 0 ] && [ "$(cat "$cap_fake_bin/out")" = "rc=0" ] \
    && [ $(( $(date +%s) - start_s )) -le 5 ]
}
cap_runs_ok=true
for _ in $(seq 1 30); do
  cap_probe specs/machines/meerkat_machine/tlc_run_cap.sh || { cap_runs_ok=false; break; }
done
# Force the ordering that used to orphan the sleep: a copy of the script
# whose watchdog pauses before starting its sleep, so the caller's kill
# always lands while the sleep is not yet running.
cap_window_ok=false
if python3 - specs/machines/meerkat_machine/tlc_run_cap.sh "$cap_fake_bin/tlc_run_cap.sh" <<'PY'
import sys
src, dst = sys.argv[1], sys.argv[2]
text = open(src).read()
anchor = '  (\n    sleep "${tlc_run_cap_secs}"\n'
assert text.count(anchor) == 1, "watchdog sleep anchor not found exactly once"
open(dst, "w").write(text.replace(anchor, '  (\n    /bin/sleep 1\n    sleep "${tlc_run_cap_secs}"\n'))
PY
then
  cap_window_ok=true
  for _ in 1 2 3; do
    cap_probe "$cap_fake_bin/tlc_run_cap.sh" || { cap_window_ok=false; break; }
  done
fi
rm -rf "$cap_fake_bin"
cap_script=specs/machines/meerkat_machine/tlc_run_cap.sh
if [ "$cap_runs_ok" = true ] && [ "$cap_window_ok" = true ] \
  && grep -Fq 'set -m' "$cap_script" \
  && grep -Fq ') </dev/null >/dev/null 2>&1 &' "$cap_script" \
  && grep -Fq 'kill -KILL -- "-${watchdog_pgid}"' "$cap_script" \
  && grep -Fq 'kill -KILL -- "-${tlc_run_cap_watchdog_pgid}"' "$cap_script" \
  && ! grep -Eq 'kill -TERM -- "-\$' "$cap_script"; then
  ok "tlc_run_cap: the watchdog is a process group with null stdio, ended with SIGKILL; instant runs inside captures return at once, even when the kill lands before its sleep starts"
else
  bad "tlc_run_cap: an instant run inside a capture hung or failed (or did when the kill landed before the sleep), or the watchdog is no longer an isolated process group ended with SIGKILL"
fi

# The watchdog is a forked copy of the caller's shell, ended by the caller
# as soon as TLC exits. Until bash resets the traps the fork inherited, a
# TERM runs the caller's EXIT trap in it, and an audit's trap removes its
# work directory: a TERM sent right after an instant run deleted the log the
# audit was about to read, so the lane failed now and then (more often under
# load). Here a watchdog-shaped subshell, launched as the script launches
# its watchdog, is ended at once with the signal the script uses, 300 times,
# inside a shell whose EXIT trap removes its work directory; the directory
# must survive every time (TERM loses it in a few percent of runs, SIGKILL
# never). The probe reads the signal from exactly one kill line, and each
# run is bounded: a signal this shell cannot send would otherwise leave the
# run waiting out the subshell's sleep, 300 times.
watchdog_signals="$(sed -n 's/^  kill -\([A-Z]*\) -- "-\${watchdog_pgid}".*/\1/p' "$cap_script")"
watchdog_signal_lines="$(printf '%s' "$watchdog_signals" | grep -c . || true)"
watchdog_signal=""
if [ "$watchdog_signal_lines" = 1 ]; then
  watchdog_signal="$watchdog_signals"
fi
trap_probe_dir="$(mktemp -d)"
trap_runs_ok=true
if [ -z "$watchdog_signal" ]; then
  trap_runs_ok=false
else
  for _ in $(seq 1 300); do
    trap_work="$(mktemp -d "$trap_probe_dir/work.XXXXXX")"
    if ! timeout 10 bash -c '
        trap "rm -rf \"$1\"" EXIT
        set -m
        ( sleep 120 ) </dev/null >/dev/null 2>&1 &
        pgid=$!
        set +m
        kill -"$2" -- "-${pgid}" 2>/dev/null || true
        wait "${pgid}" 2>/dev/null || true
        [ -d "$1" ]
      ' _ "$trap_work" "$watchdog_signal"; then
      trap_runs_ok=false
      break
    fi
  done
fi
rm -rf "$trap_probe_dir"
if [ "$trap_runs_ok" = true ]; then
  ok "tlc_run_cap: ending the watchdog with SIG${watchdog_signal} never runs the caller's EXIT trap (an audit's work directory survives 300 immediate ends)"
elif [ "$watchdog_signal_lines" != 1 ]; then
  bad "tlc_run_cap: the trap probe reads the watchdog's signal from exactly one kill line; tlc_run_cap.sh has ${watchdog_signal_lines}"
else
  bad "tlc_run_cap: ending the watchdog with SIG${watchdog_signal} ran the caller's EXIT trap and deleted an audit's work directory, or a run did not finish within 10 s"
fi

# The audit shards never execute xtask, so the PR lane runs them without an
# xtask argument; every other part still requires one.
if noxtask_a="$(PATH="$tlc_env_tmp:$PATH" \
      TLC_JAVA_OPTIONS_CAPTURE="$capture" \
      TLC_JDK_JAVA_OPTIONS_CAPTURE="$jdk_capture" \
      bash crates/xtask/tests/machine_verify_all_tlc_test.sh --part audits-a 2>&1)" \
  && [ "$(printf '%s\n' "$noxtask_a" | audit_lines)" = "$(audit_lines < "$shard_outputs/audits-a")" ] \
  && ! PATH="$tlc_env_tmp:$PATH" bash crates/xtask/tests/machine_verify_all_tlc_test.sh --part machine-verify >/dev/null 2>&1; then
  ok "TLC lane: audit shards run without an xtask argument; machine-verify still requires one"
else
  bad "TLC lane: an audit shard needs an xtask argument, or machine-verify runs without one"
fi
rm -rf "$shard_outputs"

# setup-tlc-ci is the one TLC install: the repository's immutable mirror of
# the tlaplus v1.8.0 build (tlaplus rebuilds that pre-release in place),
# pinned by sha256, with no fallback. Every workflow and action installs TLC
# through it, so PR, nightly and BuildBuddy lanes run one build.
tlc_action=.github/actions/setup-tlc-ci/action.yml
tlc_mirror='https://github.com/lukacf/meerkat/releases/download/tlc-tla2tools-v1.8.0-20261004/tla2tools.jar'
tlc_sha256='c2fe4e56e43bde19f213b4a7e441d037297fda733e503579623e859b79348239'
if [ -f "$tlc_action" ] \
  && grep -Fq "default: ${tlc_mirror}" "$tlc_action" \
  && grep -Fq "default: ${tlc_sha256}" "$tlc_action" \
  && grep -Fq 'distribution: temurin' "$tlc_action" \
  && grep -Fq 'sha256sum' "$tlc_action" \
  && grep -Fq 'needs a sha256 pin' "$tlc_action" \
  && grep -Fq 'does not match the pinned' "$tlc_action" \
  && grep -Fq 'GITHUB_PATH' "$tlc_action"; then
  ok "setup-tlc-ci installs the mirrored TLC jar pinned by sha256 and reports its digest"
else
  bad "setup-tlc-ci is missing, unpinned, off the mirror, or no longer reports the jar digest"
fi
if [ -z "$(grep -rlF 'tlaplus/tlaplus/releases' .github)" ] \
  && [ "$(grep -rlF 'uses: ./.github/actions/setup-tlc-ci' .github | sort | tr '\n' ' ')" = ".github/actions/setup-buildbuddy-ci/action.yml .github/workflows/cargo.yml .github/workflows/ci.yml .github/workflows/nightly.yml " ]; then
  ok "every TLC install goes through setup-tlc-ci; no workflow downloads the rolling tlaplus jar"
else
  bad "a workflow or action installs TLC outside setup-tlc-ci, or downloads the rolling tlaplus jar"
fi

# The MobMachine arm-coverage gate reads the generated Next relation, the
# audit runners' covered_actions arrays and the not-covered list. On a
# synthetic spec directory it must pass a complete classification and refuse
# an unlisted arm, a stale entry, an exact entry for a covered arm and an
# entry without a reason.
arm_gate_tmp="$(mktemp -d)"
cp specs/machines/mob_machine/arm_coverage_gate.sh "$arm_gate_tmp/"
printf '%s\n' \
  'Next ==' \
  '    \/ (phase = "Running") /\ \E agent_identity \in AgentIdentityValues : ArmCoveredRunning(agent_identity)' \
  '    \/ ArmListedRunning' \
  '    \/ (phase = "Stopped") /\ ArmFamilyOneStopped' \
  '    \/ ArmFamilyTwoStopped' \
  '    \/ TerminalStutter' \
  '' \
  'Spec == Init /\ [][Next]_vars' \
  > "$arm_gate_tmp/model.tla"
printf '%s\n' '#!/usr/bin/env bash' 'covered_actions=(' '  ArmCoveredRunning' '  AuditWrapper' ')' \
  > "$arm_gate_tmp/probe_audit.sh"
arm_gate_run() {
  printf '%s\n' "$@" > "$arm_gate_tmp/arm_coverage_not_covered.txt"
  bash "$arm_gate_tmp/arm_coverage_gate.sh" >/dev/null 2>&1
}
if arm_gate_run '# header' 'ArmListedRunning listed with a reason' 'ArmFamily* a family with a reason' \
  && ! arm_gate_run 'ArmFamily* a family with a reason' \
  && ! arm_gate_run 'ArmListedRunning listed' 'ArmFamily* family' 'ArmGone stale entry' \
  && ! arm_gate_run 'ArmListedRunning listed' 'ArmFamily* family' 'ArmCoveredRunning contradicts the audit' \
  && ! arm_gate_run 'ArmListedRunning' 'ArmFamily* family'; then
  ok "MobMachine arm-coverage gate passes a complete classification and refuses unlisted, stale, contradictory and reasonless entries"
else
  bad "MobMachine arm-coverage gate accepts an unclassified arm or a bad not-covered entry, or refuses a complete one"
fi
rm -rf "$arm_gate_tmp"

echo ""
echo "gate summary: ${pass} passed, ${fail} failed"
if [ "$fail" -ne 0 ]; then
  exit 1
fi
