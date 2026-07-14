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
# The emitter is run with cwd=TEMP_ROOT and the committed-vs-fresh diff happens
# inside the temp dir, so the workspace tree is never written.
echo "#285 verify-schema-freshness leaves the workspace clean:"
if grep -Fq 'cd "$TEMP_ROOT" && "$CARGO" run -p meerkat-contracts' scripts/verify-schema-freshness.sh; then
  ok "emit-schemas runs with cwd=TEMP_ROOT"
else
  bad "emit-schemas is not run with cwd=TEMP_ROOT"
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
  && grep -Fq 'meerkat-machine-codegen/BUILD.bazel' scripts/verify-version-parity.sh; then
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
BAZEL_VERS=$(grep -E '"CARGO_PKG_VERSION":' meerkat-machine-codegen/BUILD.bazel \
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
if scripts/machine-authority-changed -- xtask/src/rmat_policy.rs >/dev/null; then
  ok "machine-authority-changed exits 0 (changed) for xtask/src/rmat_policy.rs"
else
  bad "machine-authority-changed did not flag xtask/src/rmat_policy.rs"
fi
if scripts/machine-authority-changed -- meerkat-mob/src/runtime/actor.rs >/dev/null; then
  ok "machine-authority-changed exits 0 (changed) for mob runtime authority"
else
  bad "machine-authority-changed did not flag mob runtime authority"
fi
if scripts/machine-authority-changed -- docs/reference/machine-authority.mdx >/dev/null; then
  ok "machine-authority-changed exits 0 (changed) for machine-authority docs"
else
  bad "machine-authority-changed did not flag machine-authority docs"
fi
for gate_owner in .github/workflows/ci.yml .github/workflows/cargo.yml Makefile scripts/machine-authority-changed scripts/tests/xtask_scripts_dogma_gates.sh xtask/tests/ci_gate_requires_rmat.rs; do
  if scripts/machine-authority-changed -- "$gate_owner" >/dev/null; then
    ok "machine-authority-changed protects its gate owner $gate_owner"
  else
    bad "machine-authority-changed ignored its gate owner $gate_owner"
  fi
done
edge_out=$(printf 'xtask/src/rmat_policy.rs\n' | scripts/buildbuddy-edge-changes --paths-from-stdin)
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
edge_internal_dogma=$(printf 'docs-internal/dogma-audits/PR759-final-ledger.md\n' | scripts/buildbuddy-edge-changes --paths-from-stdin)
if printf '%s' "$edge_internal_dogma" | grep -Fq 'wasm_changed=true'; then
  ok "buildbuddy-edge-changes mark_all triggers on internal dogma audit docs"
else
  bad "buildbuddy-edge-changes did not mark_all on internal dogma audit docs"
fi
finite_ledger='docs-internal/archive/public-docs-removed-2026-05-11/architecture/finite-ownership-ledger.md'
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
for baseline in xtask/rmat-baseline.toml xtask/ownership-baseline.toml; do
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
generated_machine_gate_out=$(scripts/cargo-agent-gate --dry-run -- meerkat-machine-schema/src/catalog/dsl/meerkat_machine.rs 2>&1 || true)
if printf '%s' "$generated_machine_gate_out" | grep -Fq 'machine-check-drift machine-authority-docs-gate seam-inventory rmat-audit' \
  && ! printf '%s' "$generated_machine_gate_out" | grep -Fq 'no Rust build-relevant changes detected.'; then
  ok "cargo-agent-gate dry-run routes generated machine catalog changes to governance gate"
else
  bad "cargo-agent-gate dry-run did not route generated machine catalog changes"
fi
mob_identity_gate_out=$(scripts/cargo-agent-gate --dry-run -- meerkat-mob/src/ids.rs 2>&1 || true)
if printf '%s' "$mob_identity_gate_out" | grep -Fq 'machine-check-drift machine-authority-docs-gate runtime-authority-bypass seam-inventory rmat-audit' \
  && ! printf '%s' "$mob_identity_gate_out" | grep -Fq 'no Rust build-relevant changes detected.'; then
  ok "cargo-agent-gate dry-run routes mob identity authority changes to governance gate"
else
  bad "cargo-agent-gate dry-run did not route mob identity authority changes"
fi
generated_buildbuddy_gate_out=$(scripts/buildbuddy-agent-gate --dry-run -- meerkat-core/src/generated/protocol_runtime.rs 2>&1 || true)
if printf '%s' "$generated_buildbuddy_gate_out" | grep -Fq 'machine-check-drift machine-authority-docs-gate seam-inventory rmat-audit' \
  && ! printf '%s' "$generated_buildbuddy_gate_out" | grep -Fq 'no build-relevant changes detected.'; then
  ok "buildbuddy-agent-gate dry-run routes generated protocol changes to governance gate"
else
  bad "buildbuddy-agent-gate dry-run did not route generated protocol changes"
fi
if grep -Fq '//meerkat-machine-schema:docs_machine_authority_alignment_test' scripts/buildbuddy-bazel-poc \
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
surface_gate_out=$(scripts/cargo-agent-gate --dry-run -- meerkat-mcp/src/router.rs meerkat-mob-mcp/src/agent_tools.rs docs/api/mcp.mdx sdks/typescript/README.md 2>&1 || true)
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
wasm_gate_out=$(scripts/cargo-agent-gate --dry-run -- meerkat-web-runtime/src/lib.rs 2>&1 || true)
if printf '%s' "$wasm_gate_out" | grep -Fq 'wasm-check' \
  && ! printf '%s' "$wasm_gate_out" | grep -Fq 'no Rust build-relevant changes detected.'; then
  ok "cargo-agent-gate dry-run routes web runtime changes to wasm-check"
else
  bad "cargo-agent-gate dry-run did not route web runtime changes"
fi
rest_gate_out=$(scripts/buildbuddy-agent-gate --dry-run -- meerkat-rest/src/lib.rs meerkat-rpc/src/session_runtime.rs 2>&1 || true)
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
    xtask/tests/machine_verify_all_tlc_test.sh \
  && grep -Fq 'MACHINE_VERIFY_TLC_DRIFT_ONLY' xtask/tests/machine_verify_all_tlc_test.sh; then
  ok "TLC test fails closed (exit 1) unless MACHINE_VERIFY_TLC_DRIFT_ONLY=1"
else
  bad "TLC test still silently downgrades to drift-only when tlc is absent"
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
  PATH="/usr/bin:/bin" bash xtask/tests/machine_verify_all_tlc_test.sh /bin/true >/dev/null 2>&1
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

echo ""
echo "gate summary: ${pass} passed, ${fail} failed"
if [ "$fail" -ne 0 ]; then
  exit 1
fi
