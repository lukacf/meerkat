"""Downstream-scenario E2E smoke tests using the identity-first API.

Covers downstream scenarios DS-01 through DS-08: real round-trip through Python SDK -> rpc_gateway
-> Rust IdentityRuntime -> restore_flow -> SessionBridge -> real LLM API.

Run:
    PYTHONPATH=sdk/python ANTHROPIC_API_KEY=... \
        python3 -m pytest sdk/python/tests/test_identity_first_downstream_e2e.py -v --timeout=180
"""
from __future__ import annotations

import os
import asyncio

import pytest

from meerkat_meta.builder import Meta
from meerkat_meta.identity_first_models import (
    DispatchInput,
    DurableAgentSpec,
    IdentityBootstrapMode,
    IdentityBootstrapState,
    ManagedPeerEdge,
)
from meerkat_meta.identity_first_providers import (
    AgentCustomizerProtocol,
    RosterProviderProtocol,
    TopologyProviderProtocol,
)
from meerkat_meta.errors import RpcError

# ---------------------------------------------------------------------------
# Environment / skip helpers
# ---------------------------------------------------------------------------

# The gateway-backed suites must exercise THIS worktree's freshly built
# rpc_gateway, not whichever binary the main checkout last built. The default
# path below is the repository's plain cargo target directory; running from a
# feature worktree would silently test the wrong artifact (and hide the wire
# drift the branch introduces). Prefer an explicit override:
#   META_GATEWAY_BIN=$(./scripts/repo-cargo --print-env CARGO_TARGET_DIR)/debug/rpc_gateway
def _resolve_gateway_bin() -> str:
    override = os.environ.get("META_GATEWAY_BIN", "").strip()
    if override:
        return override
    return os.path.join(
        os.path.dirname(os.path.abspath(__file__)),
        "..", "..", "..", "target", "debug", "rpc_gateway",
    )


_GATEWAY_BIN = _resolve_gateway_bin()


def _anthropic_key() -> str | None:
    return os.environ.get("RKAT_ANTHROPIC_API_KEY") or os.environ.get(
        "ANTHROPIC_API_KEY"
    )


_skip_no_key = pytest.mark.skipif(
    not _anthropic_key(),
    reason="No Anthropic API key",
)

_skip_no_binary = pytest.mark.skipif(
    not os.path.isfile(_GATEWAY_BIN),
    reason=f"Gateway binary not found at {_GATEWAY_BIN}",
)

# ---------------------------------------------------------------------------
# Mob definition (TOML) - profiles for all downstream app roles
# ---------------------------------------------------------------------------

_DOWNSTREAM_MOB_TOML = """\
[mob]
id = "example-e2e"

[profiles.personal]
model = "claude-sonnet-4-5"
skills = ["personal_role"]
external_addressable = true

[profiles.personal.tools]
comms = true

[profiles.triage]
model = "claude-sonnet-4-5"
skills = ["triage_role"]
external_addressable = false

[profiles.triage.tools]
comms = true

[profiles.calendar]
model = "claude-sonnet-4-5"
skills = ["calendar_role"]
external_addressable = false

[profiles.calendar.tools]
comms = true

[profiles.gatekeeper]
model = "claude-sonnet-4-5"
skills = ["gatekeeper_role"]
external_addressable = false

[profiles.gatekeeper.tools]
comms = true

# --- Prompts ---
# A member's system prompt is assembled from its profile `skills`; a bare
# `system_prompt` key under [profiles.X] is not a meerkat Profile field and
# is silently dropped.

[skills.personal_role]
source = "inline"
content = "You are a helpful personal assistant. Keep responses brief (1-2 sentences)."

[skills.triage_role]
source = "inline"
content = "You are a triage agent. Route requests to the right domain. Keep responses brief."

[skills.calendar_role]
source = "inline"
content = "You are a calendar domain agent. Help with scheduling. Keep responses brief."

[skills.gatekeeper_role]
source = "inline"
content = "You are a gatekeeper agent. Validate requests. Keep responses brief."
"""

# ---------------------------------------------------------------------------
# Provider implementations
# ---------------------------------------------------------------------------


class DownstreamRoster:
    """Roster provider that returns a configurable list of DurableAgentSpecs."""

    def __init__(self, specs: list[DurableAgentSpec]):
        self._specs = list(specs)

    def update(self, specs: list[DurableAgentSpec]) -> None:
        self._specs = list(specs)

    async def roster(self, context: dict) -> list[DurableAgentSpec]:
        return list(self._specs)


class DownstreamTopology:
    """Topology provider that returns configurable edges."""

    def __init__(self, edges: list[tuple[str, str]]):
        self._edges = list(edges)

    def update(self, edges: list[tuple[str, str]]) -> None:
        self._edges = list(edges)

    async def compute_edges(
        self, target_identities: list[str], context: dict
    ) -> list[ManagedPeerEdge]:
        return [ManagedPeerEdge(a=a, b=b) for a, b in self._edges]


class DownstreamCustomizer:
    """Agent customizer that injects topology-aware prompts."""

    async def customize_build(self, context, spec, draft) -> None:
        """Mutate draft in-place based on context.

        context: AgentBuildContext (identity, active_peers, managed_edges)
        spec: DurableAgentSpec
        draft: AgentBuildDraft (mutable)
        """
        identity = context.identity
        peers = []
        for edge in context.managed_edges:
            if edge.a == identity:
                peers.append(edge.b)
            elif edge.b == identity:
                peers.append(edge.a)

        if peers:
            peers.sort()
            # Appended after the profile's skill-assembled prompt. Setting
            # draft.system_prompt here would REPLACE that prompt with the
            # peer line alone.
            draft.additional_instructions.append(
                f"Your connected peers: {', '.join(peers)}"
            )

    async def after_create(
        self, identity: str, session_id: str, context
    ) -> None:
        pass


# ---------------------------------------------------------------------------
# Default rosters and topology
# ---------------------------------------------------------------------------

_DEFAULT_ROSTER = [
    DurableAgentSpec(identity="identity:luka", profile="personal", addressability="addressable"),
    DurableAgentSpec(identity="identity:alice", profile="personal", addressability="addressable"),
    DurableAgentSpec(identity="triage:main", profile="triage", addressability="internal_only"),
    DurableAgentSpec(identity="domain:calendar", profile="calendar", addressability="internal_only"),
    DurableAgentSpec(identity="gate:main", profile="gatekeeper", addressability="internal_only"),
]

_DEFAULT_EDGES = [
    ("identity:luka", "triage:main"),
    ("identity:alice", "triage:main"),
    ("triage:main", "domain:calendar"),
    ("triage:main", "gate:main"),
]

# ---------------------------------------------------------------------------
# Boot helper
# ---------------------------------------------------------------------------


async def _boot_identity_runtime(
    state_dir: str,
    mob_toml_path: str,
    roster: DownstreamRoster,
    topology: DownstreamTopology | None = None,
    customizer: DownstreamCustomizer | None = None,
):
    """Boot a real rpc_gateway with identity-first providers."""
    builder = (
        Meta.builder()
        .gateway(_GATEWAY_BIN)
        .mob(mob_toml_path)
        .persistent_state(state_dir)
        .roster(roster)
    )
    if topology is not None:
        builder = builder.topology_provider(topology)
    if customizer is not None:
        builder = builder.agent_customizer(customizer)
    return await builder.build()


async def _wait_for_identity_convergence(runtime, *, timeout: float = 60.0):
    """Wait through diagnostic Broken states until the roster converges.

    Eager bootstrap is terminal once every identity has a classified outcome;
    a retryable resume failure is therefore observable as ``Broken`` before
    the runtime-owned repair supervisor's first backoff expires.  Recovery is
    level-triggered, so restart tests must assert eventual convergence rather
    than depend on whether the first observation won that race.
    """
    loop = asyncio.get_running_loop()
    deadline = loop.time() + timeout
    status = await runtime.identity_bootstrap_status()
    while not status.ready and loop.time() < deadline:
        await asyncio.sleep(0.5)
        status = await runtime.identity_bootstrap_status()
    assert status.ready, status.to_dict()

    # Active lifecycle is necessary but not sufficient: once the repair
    # supervisor converges every identity, use the typed member-readiness
    # barrier for the remaining time budget.
    remaining = max(0.0, deadline - loop.time())
    startup = await runtime.wait_identity_bootstrap(
        target="startup_ready", timeout=remaining
    )
    assert startup.startup_ready is True, startup.to_dict()
    return startup


@pytest.fixture
def mob_toml(tmp_path):
    p = tmp_path / "mob.toml"
    p.write_text(_DOWNSTREAM_MOB_TOML)
    return str(p)


@pytest.fixture
def state_dir(tmp_path):
    d = str(tmp_path / "state")
    os.makedirs(d, exist_ok=True)
    return d


@_skip_no_binary
@pytest.mark.asyncio
@pytest.mark.timeout(30)
async def test_lazy_gateway_init_exposes_typed_dormant_bootstrap_status(
    mob_toml, state_dir
):
    """Lazy init is metadata-only and needs no model-provider credential."""
    roster = DownstreamRoster(_DEFAULT_ROSTER)
    runtime = await (
        Meta.builder()
        .gateway(_GATEWAY_BIN)
        .mob(mob_toml)
        .persistent_state(state_dir)
        .roster(roster)
        .identity_bootstrap_mode(IdentityBootstrapMode.lazy_materialize())
        .build()
    )
    try:
        status = await runtime.identity_bootstrap_status()
        assert status.mode == IdentityBootstrapMode.lazy_materialize()
        assert status.complete is True
        assert status.ready is False
        assert status.counts.dormant == len(_DEFAULT_ROSTER)
        assert status.counts.warming == 0
        assert status.counts.active == 0
        assert status.counts.broken == 0
        assert all(
            entry.state is IdentityBootstrapState.DORMANT
            for entry in status.identities.values()
        )

        immediate = await runtime.wait_identity_bootstrap(timeout=0)
        assert immediate.timed_out is True
        assert immediate.ready is False
    finally:
        await runtime.shutdown()


# ===========================================================================
# DS-01: Team Bootstrap And Mixed Delivery
# ===========================================================================


@_skip_no_key
@_skip_no_binary
class TestDS01TeamBootstrapAndMixedDelivery:
    @pytest.mark.asyncio
    @pytest.mark.timeout(120)
    async def test_send_dispatch_and_addressability(self, mob_toml, state_dir):
        roster = DownstreamRoster(_DEFAULT_ROSTER)
        topology = DownstreamTopology(_DEFAULT_EDGES)
        customizer = DownstreamCustomizer()

        rt = await _boot_identity_runtime(
            state_dir, mob_toml, roster, topology, customizer
        )
        try:
            # send() to Addressable identity succeeds
            result = await rt.send("identity:luka", "What's on my calendar tomorrow?")
            assert result is not None

            # dispatch() to InternalOnly identity succeeds
            di = DispatchInput(
                content="New email from a vendor about a delivery change",
                origin="connector",
            )
            result = await rt.dispatch("triage:main", di)
            assert result is not None

            # send() to InternalOnly returns NotAddressable error
            with pytest.raises(RpcError, match="not addressable"):
                await rt.send("triage:main", "Should be rejected")

            # status() shows correct state
            status = await rt.status("triage:main")
            assert status.state == "active"
            assert status.addressability == "internal_only"

            lead_status = await rt.status("identity:luka")
            assert lead_status.state == "active"
            assert lead_status.addressability == "addressable"
            assert lead_status.agent_runtime_id is not None
            assert lead_status.session_id is not None
        finally:
            await rt.shutdown()


# ===========================================================================
# DS-02: Unknown Sender Goes To Triage, Known Sender Goes To Identity
# ===========================================================================


@_skip_no_key
@_skip_no_binary
class TestDS02DispatchRouting:
    @pytest.mark.asyncio
    @pytest.mark.timeout(120)
    async def test_distinct_dispatch_targets(self, mob_toml, state_dir):
        roster = DownstreamRoster(_DEFAULT_ROSTER)
        topology = DownstreamTopology(_DEFAULT_EDGES)

        rt = await _boot_identity_runtime(state_dir, mob_toml, roster, topology)
        try:
            # Known sender → dispatch to identity
            di_lead = DispatchInput(
                content="Telegram DM from Luka: Can you remind me about the vendor review?",
                origin="connector",
                correlation_id="msg-1",
                idempotency_key="telegram:msg-1",
            )
            await rt.dispatch("identity:luka", di_lead)

            # Unknown sender → dispatch to triage
            di_triage = DispatchInput(
                content="Unknown WhatsApp sender asked about package delivery",
                origin="connector",
                correlation_id="msg-2",
                idempotency_key="whatsapp:msg-2",
            )
            await rt.dispatch("triage:main", di_triage)

            # Both are distinct identities with different session_ids
            lead_status = await rt.status("identity:luka")
            triage_status = await rt.status("triage:main")
            assert lead_status.session_id != triage_status.session_id
            assert lead_status.agent_runtime_id != triage_status.agent_runtime_id

            # Record pre-restart state
            lead_rt_id = lead_status.agent_runtime_id
            lead_session = lead_status.session_id
            triage_rt_id = triage_status.agent_runtime_id
            triage_session = triage_status.session_id
        finally:
            await rt.shutdown()

        # Restart and verify stability
        rt2 = await _boot_identity_runtime(state_dir, mob_toml, roster, topology)
        try:
            lead2 = await rt2.status("identity:luka")
            triage2 = await rt2.status("triage:main")
            assert lead2.agent_runtime_id == lead_rt_id
            assert lead2.session_id == lead_session
            assert triage2.agent_runtime_id == triage_rt_id
            assert triage2.session_id == triage_session
        finally:
            await rt2.shutdown()


# ===========================================================================
# DS-03: Reconcile New Team Member Without Breaking Existing Continuity
# ===========================================================================


@_skip_no_key
@_skip_no_binary
class TestDS03ReconcileNewMember:
    @pytest.mark.asyncio
    @pytest.mark.timeout(120)
    async def test_add_bob_via_in_process_reconcile(self, mob_toml, state_dir):
        roster = DownstreamRoster(_DEFAULT_ROSTER[:3])  # luka, alice, triage
        topology = DownstreamTopology(_DEFAULT_EDGES[:2])  # luka↔triage, alice↔triage

        rt = await _boot_identity_runtime(state_dir, mob_toml, roster, topology)
        try:
            before_lead = await rt.status("identity:luka")
            before_alice = await rt.status("identity:alice")
            lead_session = before_lead.session_id
            lead_rt_id = before_lead.agent_runtime_id
            alice_session = before_alice.session_id

            # Mutate the roster IN-PROCESS (not restart)
            roster.update([
                *_DEFAULT_ROSTER[:3],
                DurableAgentSpec(
                    identity="identity:bob",
                    profile="personal",
                    addressability="addressable",
                ),
            ])
            topology.update([
                *_DEFAULT_EDGES[:2],
                ("identity:bob", "triage:main"),
            ])

            # In-process reconcile — re-runs restore_flow with updated roster
            result = await rt.reconcile()
            assert result is not None

            after_lead = await rt.status("identity:luka")
            after_alice = await rt.status("identity:alice")
            after_bob = await rt.status("identity:bob")

            # bob is new
            assert after_bob.state == "active"
            assert after_bob.session_id is not None
            assert after_bob.generation == 0

            # luka and alice preserved - no silent respawn
            assert after_lead.session_id == lead_session
            assert after_lead.agent_runtime_id == lead_rt_id
            assert after_alice.session_id == alice_session
        finally:
            await rt.shutdown()


# ===========================================================================
# DS-04: Hot-Update Addressability And Metadata
# ===========================================================================


@_skip_no_key
@_skip_no_binary
class TestDS04HotUpdateAddressability:
    @pytest.mark.asyncio
    @pytest.mark.timeout(120)
    async def test_flip_addressability_via_reconcile(self, mob_toml, state_dir):
        roster = DownstreamRoster(_DEFAULT_ROSTER)
        topology = DownstreamTopology(_DEFAULT_EDGES)

        rt = await _boot_identity_runtime(state_dir, mob_toml, roster, topology)
        try:
            # luka starts Addressable
            status = await rt.status("identity:luka")
            assert status.addressability == "addressable"

            # send works
            await rt.send("identity:luka", "Hello")

            # Mutate roster IN-PROCESS: flip luka to InternalOnly, add label
            new_lead = DurableAgentSpec(
                identity="identity:luka",
                profile="personal",
                addressability="internal_only",
                labels={"timezone": "Europe/Stockholm"},
            )
            roster.update([new_lead, *_DEFAULT_ROSTER[1:]])

            # In-process reconcile
            await rt.reconcile()

            status2 = await rt.status("identity:luka")
            assert status2.addressability == "internal_only"
            assert status2.labels.get("timezone") == "Europe/Stockholm"

            # send now rejected
            with pytest.raises(RpcError, match="not addressable"):
                await rt.send("identity:luka", "Should fail now")

            # dispatch still works
            di = DispatchInput(content="system notice", origin="system")
            await rt.dispatch("identity:luka", di)
        finally:
            await rt.shutdown()


# ===========================================================================
# DS-05: Durable Respawn After Wedged Agent
# ===========================================================================


@_skip_no_key
@_skip_no_binary
class TestDS05DurableRespawn:
    @pytest.mark.asyncio
    @pytest.mark.timeout(120)
    async def test_respawn_preserves_identity(self, mob_toml, state_dir):
        roster = DownstreamRoster(_DEFAULT_ROSTER)
        topology = DownstreamTopology(_DEFAULT_EDGES)

        rt = await _boot_identity_runtime(state_dir, mob_toml, roster, topology)
        try:
            # Dispatch to calendar
            di = DispatchInput(
                content="Long-running calendar sync event",
                origin="scheduler",
            )
            await rt.dispatch("domain:calendar", di)

            before = await rt.status("domain:calendar")
            assert before.state == "active"
            before_rt_id = before.agent_runtime_id
            before_session = before.session_id
            before_gen = before.generation

            # Respawn — non-destructive recovery
            result = await rt.respawn("domain:calendar")
            assert result is not None

            after = await rt.status("domain:calendar")
            assert after.state == "active"
            assert after.agent_runtime_id == before_rt_id
            assert after.session_id == before_session  # session preserved
            assert after.generation == before_gen  # generation NOT advanced
        finally:
            await rt.shutdown()


# ===========================================================================
# DS-06: Dispatch To An Active Identity
# ===========================================================================


@_skip_no_key
@_skip_no_binary
class TestDS06Dispatch:
    @pytest.mark.asyncio
    @pytest.mark.timeout(60)
    async def test_dispatch_succeeds_on_an_active_identity(self, mob_toml, state_dir):
        """Dispatch to a booted identity succeeds and leaves it active.

        Meta holds no identity leases since 0.9.4: Core's realm lock and
        hosting claim give host exclusivity, so there is no lease to lose.
        """
        roster = DownstreamRoster(_DEFAULT_ROSTER)

        rt = await _boot_identity_runtime(state_dir, mob_toml, roster)
        try:
            di = DispatchInput(content="dispatch check", origin="connector")
            result = await rt.dispatch("triage:main", di)
            assert result is not None

            status = await rt.status("triage:main")
            assert status.state == "active"
        finally:
            await rt.shutdown()


# ===========================================================================
# DS-07: Reset vs Delete Semantics
# ===========================================================================


@_skip_no_key
@_skip_no_binary
class TestDS07ResetVsDelete:
    @pytest.mark.asyncio
    @pytest.mark.timeout(120)
    async def test_reset_advances_generation(self, mob_toml, state_dir):
        roster = DownstreamRoster(_DEFAULT_ROSTER)
        topology = DownstreamTopology(_DEFAULT_EDGES)

        rt = await _boot_identity_runtime(state_dir, mob_toml, roster, topology)
        try:
            before = await rt.status("identity:luka")
            old_session = before.session_id
            old_gen = before.generation

            result = await rt.reset("identity:luka")
            assert result is not None

            after = await rt.status("identity:luka")
            assert after.generation == old_gen + 1
            assert after.session_id != old_session
        finally:
            await rt.shutdown()

    @pytest.mark.asyncio
    @pytest.mark.timeout(120)
    async def test_delete_and_reappearance(self, mob_toml, state_dir):
        roster = DownstreamRoster(_DEFAULT_ROSTER)

        rt = await _boot_identity_runtime(state_dir, mob_toml, roster)
        try:
            before = await rt.status("identity:luka")
            old_rt_id = before.agent_runtime_id

            await rt.delete_identity("identity:luka")

            # identity:luka is gone
            with pytest.raises(RpcError, match="unknown identity"):
                await rt.status("identity:luka")
        finally:
            await rt.shutdown()

        # Restart with same roster - luka reappears as fresh
        rt2 = await _boot_identity_runtime(state_dir, mob_toml, roster)
        try:
            after = await rt2.status("identity:luka")
            assert after.state == "active"
            assert after.generation == 0
            assert after.session_id != before.session_id
        finally:
            await rt2.shutdown()


# ===========================================================================
# DS-08: External-Authoritative Restart Smoke
# ===========================================================================


@_skip_no_key
@_skip_no_binary
class TestDS08PersistentRestart:
    @pytest.mark.asyncio
    @pytest.mark.timeout(360)
    async def test_session_and_continuity_preserved_across_restart(self, mob_toml, state_dir):
        roster = DownstreamRoster(_DEFAULT_ROSTER)
        topology = DownstreamTopology(_DEFAULT_EDGES)
        customizer = DownstreamCustomizer()

        rt = await _boot_identity_runtime(
            state_dir, mob_toml, roster, topology, customizer
        )
        try:
            # Give luka a memorable fact
            await rt.send(
                "identity:luka",
                "Remember that we moved the piano on Friday. This is important.",
            )
            before = await rt.status("identity:luka")
            session_before = before.session_id
            rt_id_before = before.agent_runtime_id

            # Also dispatch to triage to build multi-actor state
            di = DispatchInput(
                content="Project notification: design review next Tuesday",
                origin="connector",
            )
            await rt.dispatch("triage:main", di)
            triage_before = await rt.status("triage:main")
            triage_session_before = triage_before.session_id
        finally:
            await rt.shutdown()

        # Restart with same state_dir
        rt2 = await _boot_identity_runtime(
            state_dir, mob_toml, roster, topology, customizer
        )
        try:
            # Verify stable identifiers across restart
            after = await rt2.status("identity:luka")
            assert after.session_id == session_before
            assert after.agent_runtime_id == rt_id_before
            await _wait_for_identity_convergence(rt2)
            after = await rt2.status("identity:luka")
            assert after.state == "active"

            triage_after = await rt2.status("triage:main")
            assert triage_after.session_id == triage_session_before

            # Verify conversational continuity — ask about the pre-restart fact.
            # The LLM should reference the piano if session history is intact.
            await rt2.send(
                "identity:luka",
                "What did we move on Friday? Reply with just the item.",
            )
            # If this dispatch completes without error, the session is alive.
            # Full memory verification would require inspecting the LLM response,
            # but the stable session_id + successful delivery proves the session
            # was restored (not fresh-created).
        finally:
            await rt2.shutdown()

        # Restart once more after the resumed runtime has committed another
        # turn. This catches a latent same-base sibling fork that can look
        # healthy during the first recovery but wedge the following restart.
        rt3 = await _boot_identity_runtime(
            state_dir, mob_toml, roster, topology, customizer
        )
        try:
            after_second_restart = await rt3.status("identity:luka")
            assert after_second_restart.session_id == session_before
            assert after_second_restart.agent_runtime_id == rt_id_before
            await _wait_for_identity_convergence(rt3)
            after_second_restart = await rt3.status("identity:luka")
            assert after_second_restart.state == "active"

            triage_after_second_restart = await rt3.status("triage:main")
            assert triage_after_second_restart.session_id == triage_session_before

            await rt3.send(
                "identity:luka",
                "Confirm this session is still writable after the second restart.",
            )
        finally:
            await rt3.shutdown()
