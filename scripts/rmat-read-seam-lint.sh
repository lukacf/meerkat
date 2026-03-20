#!/usr/bin/env bash
# rmat-read-seam-lint.sh — CI lint for RMAT read-seam bypasses.
#
# Detects shell code that reads authority phase/state to gate authority
# input delivery. The RMAT invariant: shells must always call
# authority.apply() and let the authority reject — never pre-filter
# inputs by reading authority state.
#
# Exit 0 = clean, exit 1 = violations found.

set -euo pipefail

RED='\033[0;31m'
GREEN='\033[0;32m'
NC='\033[0m'

VIOLATIONS=0

# ── Mob actor: orchestrator phase reads ──────────────────────────────────────
#
# The mob actor shell must not call orch.phase() to decide whether to call
# orch.apply(). The only legitimate phase() call is lifecycle_authority.phase()
# which is the actor's own state accessor, not a gate on authority inputs.

ORCH_PHASE=$(grep -n '\.phase()' meerkat-mob/src/runtime/actor.rs 2>/dev/null \
    | grep -v 'lifecycle_authority\.phase' \
    | grep -v '^\s*//' \
    | grep -v '#\[' \
    || true)

if [ -n "$ORCH_PHASE" ]; then
    echo -e "${RED}RMAT read-seam violation: orchestrator phase() reads in mob actor shell${NC}"
    echo "$ORCH_PHASE"
    echo "  Fix: remove the phase pre-check and let authority.apply() reject illegal transitions."
    echo ""
    VIOLATIONS=$((VIOLATIONS + 1))
fi

# ── Runtime driver: ingress phase reads gating apply() ───────────────────────
#
# The ephemeral driver shell must not read ingress.phase() to decide whether
# to call ingress.apply(). (ingress().phase() for non-gating reads like
# logging is fine — we flag reads that appear in if-conditions near apply.)

INGRESS_PHASE_GATE=$(grep -n 'ingress\.phase()' meerkat-runtime/src/driver/ephemeral.rs 2>/dev/null \
    | grep -v '^\s*//' \
    || true)

if [ -n "$INGRESS_PHASE_GATE" ]; then
    echo -e "${RED}RMAT read-seam violation: ingress phase() reads in ephemeral driver shell${NC}"
    echo "$INGRESS_PHASE_GATE"
    echo "  Fix: remove the phase pre-check and let authority.apply() reject illegal transitions."
    echo ""
    VIOLATIONS=$((VIOLATIONS + 1))
fi

# ── OpsLifecycle shell: authority state reads gating apply() ─────────────────
#
# The ops_lifecycle shell must not pre-check authority state before calling
# authority.apply(). The peer_ready pre-check was already fixed; ensure no
# new ones appear.

OPS_PRECHECK=$(grep -n 'shell\.spec\.' meerkat-runtime/src/ops_lifecycle.rs 2>/dev/null \
    | grep -B1 'authority\.apply' \
    | grep 'shell\.spec\.' \
    || true)

if [ -n "$OPS_PRECHECK" ]; then
    echo -e "${RED}RMAT read-seam violation: shell spec pre-check before authority.apply() in ops_lifecycle${NC}"
    echo "$OPS_PRECHECK"
    echo "  Fix: remove the shell pre-check and let authority.apply() be the sole guard."
    echo ""
    VIOLATIONS=$((VIOLATIONS + 1))
fi

# ── Accept-time policy branching ─────────────────────────────────────────────
#
# The ephemeral driver's accept_input must not read policy.apply_mode,
# policy.queue_mode, or policy.consume_point to decide how to route inputs.
# The authority's admit() method owns that classification.

POLICY_BRANCH=$(grep -n 'policy\.apply_mode\|policy\.queue_mode\|policy\.consume_point' \
    meerkat-runtime/src/driver/ephemeral.rs 2>/dev/null \
    | grep -v '^\s*//' \
    || true)

if [ -n "$POLICY_BRANCH" ]; then
    echo -e "${RED}RMAT read-seam violation: policy field reads in ephemeral driver shell${NC}"
    echo "$POLICY_BRANCH"
    echo "  Fix: use authority.admit() which internally classifies based on policy."
    echo ""
    VIOLATIONS=$((VIOLATIONS + 1))
fi

# ── MCP router: shell-owned removal state ────────────────────────────────────
#
# The MCP router must not maintain its own removal_timeouts HashMap;
# timeout ownership belongs to the ExternalToolSurfaceAuthority.

REMOVAL_SHADOW=$(grep -n 'removal_timeouts' meerkat-mcp/src/router.rs 2>/dev/null \
    | grep -v '^\s*//' \
    || true)

if [ -n "$REMOVAL_SHADOW" ]; then
    echo -e "${RED}RMAT read-seam violation: shell-owned removal_timeouts in MCP router${NC}"
    echo "$REMOVAL_SHADOW"
    echo "  Fix: read timeout info from authority state via authority.removal_timing()."
    echo ""
    VIOLATIONS=$((VIOLATIONS + 1))
fi

# ── Mob actor: shadow state counters ─────────────────────────────────────────
#
# The mob actor must not maintain tracked_flows or pending_spawn_ids as
# shadows of authority state.

SHADOW_STATE=$(grep -n 'tracked_flows\|pending_spawn_ids' meerkat-mob/src/runtime/actor.rs 2>/dev/null \
    | grep -v '^\s*//' \
    || true)

if [ -n "$SHADOW_STATE" ]; then
    echo -e "${RED}RMAT read-seam violation: shadow state counters in mob actor${NC}"
    echo "$SHADOW_STATE"
    echo "  Fix: read counts from authority snapshots (orchestrator.snapshot().active_flow_count)."
    echo ""
    VIOLATIONS=$((VIOLATIONS + 1))
fi

# ── Summary ──────────────────────────────────────────────────────────────────

if [ "$VIOLATIONS" -gt 0 ]; then
    echo -e "${RED}Found $VIOLATIONS RMAT read-seam violation(s).${NC}"
    exit 1
fi

echo -e "${GREEN}RMAT read-seam lint: clean.${NC}"
