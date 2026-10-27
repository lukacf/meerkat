"""Real-API end-to-end: SDK option -> gateway -> real LLM turn -> observable.

Every test here boots the real ``rpc_gateway`` through the Python SDK, runs a
REAL provider turn, and asserts a POSITIVE observable that only exists if the
whole chain worked. This is the lane for one defect class: an option accepted
by the SDK and the gateway that changed nothing at turn time, invisible to
every unit test because the unit tests stop at the parser and the observable
lives in a store written during a real turn.

Rules for tests in this file:
  - assert a positive observable (a row, a value), never "no error"
  - carry a negative control where one is cheap, so the assertion is proven
    to discriminate in the same run
  - bounded polling; the bound is generous because the LLM is real and the
    test is about correctness, not latency

Run:  make e2e-live            (builds rpc_gateway, exports META_GATEWAY_BIN)
Cost: three short turns on the model named by META_E2E_LIVE_MODEL
      (default claude-sonnet-4-6).
"""
from __future__ import annotations

import asyncio
import os
import sqlite3
import time
from pathlib import Path

import pytest

from meerkat_meta.builder import Meta
from meerkat_meta.identity_first_models import DurableAgentSpec

MODEL = os.environ.get("META_E2E_LIVE_MODEL", "claude-sonnet-4-6")
IDENTITY = "identity:archivist"
PROFILE = "archivist"
TURN_BUDGET_S = float(os.environ.get("META_E2E_LIVE_TURN_BUDGET_S", "120"))

MOB_TOML = f"""\
[mob]
id = "e2e-live-turn-surfaces"

[profiles.archivist]
model = "{MODEL}"
skills = ["archivist_role"]
external_addressable = true
# meerkat-mob's default runtime mode is autonomous_host, on which the identity
# door refuses injected context outright ("autonomous inbox delivery carries no
# user-channel work boundary"). Production-shaped profiles are turn_driven; the
# lane tests that shape, and the omitted-mode trap is its own finding.
runtime_mode = "turn_driven"

[profiles.archivist.tools]
comms = true

# --- Prompts ---
# A member's system prompt is assembled from its profile `skills`; a bare
# `system_prompt` key under [profiles.X] is not a meerkat Profile field and
# is silently dropped.

[skills.archivist_role]
source = "inline"
content = "You are a terse archivist. Answer in one short sentence."
"""


AUTONOMOUS_IDENTITY = "identity:sentinel"
AUTONOMOUS_PROFILE = "sentinel"

MOB_TOML += f"""
# No runtime_mode: meerkat's default is autonomous_host. On that mode meerkat
# refuses injected context, so Meta must skip recall typed and let the turn
# proceed without memory (0.8.31), instead of the bridge refusing the turn.
[profiles.sentinel]
model = "{MODEL}"
skills = ["sentinel_role"]
external_addressable = true

[profiles.sentinel.tools]
comms = true

[skills.sentinel_role]
source = "inline"
content = "You are a terse sentinel. Answer in one short sentence."
"""


class _Roster:
    """Identity-first roster: agent memory requires one, and the durable
    identity is what every surface under test is keyed by."""

    async def roster(self, context: dict) -> list[DurableAgentSpec]:
        return [
            DurableAgentSpec(
                identity=IDENTITY,
                profile=PROFILE,
                addressability="addressable",
                labels={"role": "archivist"},
            ),
            DurableAgentSpec(
                identity=AUTONOMOUS_IDENTITY,
                profile=AUTONOMOUS_PROFILE,
                addressability="addressable",
                labels={"role": "sentinel"},
            ),
        ]


async def _boot(state_dir: Path, mob_toml: Path, gateway_bin: str, **agent_memory_kwargs):
    builder = (
        Meta.builder()
        .gateway(gateway_bin)
        .mob(str(mob_toml))
        .persistent_state(str(state_dir))
        .roster(_Roster())
        .agent_memory(selection="contextual", max_entries=3, **agent_memory_kwargs)
    )
    return await builder.build()


def _injection_ledger(state_dir: Path) -> Path | None:
    """The agent-memory sqlite store is the only file under the state dir with an
    ``injections`` table; locate it by content, not by a filename convention."""
    for candidate in state_dir.rglob("*"):
        if not candidate.is_file() or candidate.stat().st_size == 0:
            continue
        if candidate.suffix in {".mfence", ".json", ".toml", ".ed25519"} or candidate.name.endswith(("-wal", "-shm")):
            continue
        try:
            # plain connect, not mode=ro: the store is WAL-mode and a read-only
            # URI open fails when it cannot map the -shm file, which made the
            # locator report "no ledger" for a store that was right there.
            con = sqlite3.connect(str(candidate))
            try:
                row = con.execute(
                    "SELECT 1 FROM sqlite_master WHERE type='table' AND name='injections'"
                ).fetchone()
            finally:
                con.close()
        except sqlite3.DatabaseError:
            continue
        if row:
            return candidate
    return None


def _surface_counts(ledger: Path, identity: str) -> dict[str, int]:
    con = sqlite3.connect(str(ledger))
    try:
        rows = con.execute(
            "SELECT surface, COUNT(*) FROM injections WHERE identity = ? GROUP BY surface",
            (identity,),
        ).fetchall()
    finally:
        con.close()
    return {surface: count for surface, count in rows}


async def _poll(predicate, budget_s: float, what: str, diagnostics=None):
    deadline = time.monotonic() + budget_s
    last = None
    while time.monotonic() < deadline:
        last = await predicate()
        if last:
            return last
        await asyncio.sleep(1.0)
    extra = f"; diagnostics={diagnostics()!r}" if diagnostics else ""
    pytest.fail(f"{what} did not become observable within {budget_s:.0f}s; last={last!r}{extra}")


def _all_ledger_rows(state_dir: Path):
    ledger = _injection_ledger(state_dir)
    if ledger is None:
        return {"ledger": None, "files": sorted(str(p.relative_to(state_dir)) for p in state_dir.rglob("*") if p.is_file())[:40]}
    con = sqlite3.connect(str(ledger))
    try:
        rows = con.execute("SELECT identity, surface, COUNT(*) FROM injections GROUP BY 1, 2").fetchall()
        records = con.execute("SELECT scope_kind, scope_key, COUNT(*) FROM records GROUP BY 1, 2").fetchall()
    finally:
        con.close()
    return {"ledger": str(ledger), "injections": rows, "records_by_scope": records}


@pytest.fixture
def state_dir(tmp_path: Path) -> Path:
    d = tmp_path / "state"
    d.mkdir()
    return d


@pytest.fixture
def mob_toml(tmp_path: Path) -> Path:
    p = tmp_path / "mob.toml"
    p.write_text(MOB_TOML)
    return p


async def _seed_and_send(rt, token: str):
    handle = rt.mob_handle()
    await handle.remember_agent_memory(
        IDENTITY,
        title="Vault door code",
        body=f"The archive vault door code is {token}. Quote it when asked about the vault.",
        tags=["vault", "e2e"],
    )
    # Precondition, so a missing turn row is attributable: the seeded record
    # must be recallable for the exact query the turn will carry, through the
    # same contextual selection the injector uses. If this fails the store or
    # the lexical scorer is the problem, not the injection door.
    recalled = await handle.recall_agent_memory(
        IDENTITY,
        query_text="What is the archive vault door code?",
        selection="contextual",
        max_entries=3,
    )
    assert any(token in item.body for item in recalled), (
        f"seeded record not recallable for the turn's query; recall returned {recalled!r}"
    )
    # Two SDK doors reach a member. `send_message` is what handle.send() uses
    # and is the default here; `send` is the identity-first door. The knob lets
    # the same end-to-end assertion exercise either public SDK surface.
    door = os.environ.get("META_E2E_LIVE_SEND_DOOR", "send_message")
    if door == "send":
        # identity-first door: meta/send
        result = await rt.send(IDENTITY, "What is the archive vault door code?")
    else:
        # mob-handle door: meta/send_message (what handle.send uses)
        result = await handle.send(IDENTITY, message="What is the archive vault door code?")
    assert getattr(result, "accepted", True), result
    return handle


@pytest.mark.asyncio
@pytest.mark.timeout(400)
async def test_omitted_per_turn_injection_injects_on_the_turn_surface(
    live_preconditions, state_dir, mob_toml, tmp_path
):
    """The regression, end to end: an options object that OMITS
    per_turn_injection must get the documented default (budgeted) and therefore
    write surface=turn ledger rows during a real turn. Negative control in the
    same run: an explicit "off" on a fresh state completes its turn and writes
    no turn rows, proving the ledger read discriminates."""
    rt = await _boot(state_dir, mob_toml, live_preconditions["gateway_bin"])
    try:
        await _seed_and_send(rt, token="ORCHID-7734")

        async def turn_row_present():
            ledger = _injection_ledger(state_dir)
            if ledger is None:
                return None
            counts = _surface_counts(ledger, IDENTITY)
            if counts.get("turn", 0) >= 1:
                return counts
            return None

        counts = await _poll(
            turn_row_present,
            TURN_BUDGET_S,
            "a surface=turn injection ledger row",
            diagnostics=lambda: _all_ledger_rows(state_dir),
        )
        assert counts.get("turn", 0) >= 1, counts
    finally:
        await rt.shutdown()

    # Negative control: explicit off, fresh state, same seed and send.
    control_state = tmp_path / "control-state"
    control_state.mkdir()
    rt_off = await _boot(control_state, mob_toml, live_preconditions["gateway_bin"], per_turn_injection="off")
    try:
        await _seed_and_send(rt_off, token="ORCHID-7734")

        # Completion signal for the control turn: routing status hydrates once the
        # session has run. (A build-surface row is NOT a usable liveness signal
        # here: build-time injection runs at materialization, which precedes the
        # seed, so the control legitimately has zero build rows too.)
        control_handle = rt_off.mob_handle()

        async def control_hydrated():
            try:
                status = await control_handle.identity_routing_status(IDENTITY)
            except Exception:
                return None
            return status if isinstance(status.session_provider, str) else None

        await _poll(control_hydrated, TURN_BUDGET_S, "the control turn to hydrate its session")
        await asyncio.sleep(3.0)
        ledger_off = _injection_ledger(control_state)
        counts_off = _surface_counts(ledger_off, IDENTITY) if ledger_off else {}
        assert counts_off.get("turn", 0) == 0, (
            f"explicit per_turn_injection=off must write no turn-surface rows: {counts_off}"
        )
    finally:
        await rt_off.shutdown()


@pytest.mark.asyncio
@pytest.mark.timeout(300)
async def test_routing_status_reports_the_declared_provider_after_a_real_turn(
    live_preconditions, state_dir, mob_toml
):
    """After a real turn the session is hydrated and routing_status must carry
    the TYPED provider matching the profile declaration, plus the declared
    model. Before hydration session_provider is None by contract; the test
    waits for the positive value rather than accepting None."""
    rt = await _boot(state_dir, mob_toml, live_preconditions["gateway_bin"])
    try:
        handle = await _seed_and_send(rt, token="ORCHID-7734")

        async def hydrated():
            try:
                status = await handle.identity_routing_status(IDENTITY)
            except Exception:  # typed unavailable before hydration
                return None
            return status if isinstance(status.session_provider, str) else None

        status = await _poll(hydrated, TURN_BUDGET_S, "a hydrated routing status")
        assert status.identity == IDENTITY
        assert status.session_provider == "anthropic", status
        effective = getattr(status, "effective_model", None) or getattr(status, "model", None)
        assert effective is None or effective == MODEL, status
    finally:
        await rt.shutdown()


@pytest.mark.asyncio
@pytest.mark.timeout(300)
async def test_resolved_tools_after_a_completed_real_turn(live_preconditions, state_dir, mob_toml):
    """A regression discriminator as a permanent test: resolved_tools on an
    identity that has completed a real turn must resolve (the session is held),
    and the resolved set must include the memory tool the profile declares.
    Read after the turn, never right after the ingress ack."""
    rt = await _boot(state_dir, mob_toml, live_preconditions["gateway_bin"])
    try:
        handle = await _seed_and_send(rt, token="ORCHID-7734")

        async def resolved():
            try:
                tools = await handle.identity_resolved_tools(IDENTITY)
            except Exception:
                return None
            return tools or None

        tools = await _poll(resolved, TURN_BUDGET_S, "a non-empty resolved tool set")
        # Positive, exact: the profile declares `comms = true` and nothing else,
        # so the live tool scope must be exactly meerkat's comms surface. A
        # superset or subset is a finding, not a pass.
        assert set(tools) == {
            "peers",
            "reply_to_peer",
            "send_message",
            "send_request",
            "send_response",
        }, tools
    finally:
        await rt.shutdown()


@pytest.mark.asyncio
@pytest.mark.timeout(300)
async def test_autonomous_host_member_turn_completes_without_memory(
    live_preconditions, state_dir, mob_toml
):
    """An autonomous-host member (meerkat's default runtime mode) cannot carry
    injected context: meerkat refuses "autonomous inbox delivery carries no
    user-channel work boundary". On 0.8.30 that refusal was the WHOLE turn, as
    a hard RpcError from the identity door. MobKit 0.8.31 skips recall typed
    for that mode instead, so the send is accepted, the turn completes, and the
    ledger shows zero turn-surface rows for that identity. Positive observable
    is the hydrated routing status; the seeded record proves recall WOULD have
    had something to inject."""
    rt = await _boot(state_dir, mob_toml, live_preconditions["gateway_bin"])
    try:
        handle = rt.mob_handle()
        await handle.remember_agent_memory(
            AUTONOMOUS_IDENTITY,
            title="Perimeter code",
            body="The perimeter gate code is LARKSPUR-2210. Quote it when asked about the gate.",
            tags=["gate", "e2e"],
        )
        # identity door: on 0.8.30 this raised RpcError for autonomous hosts
        result = await rt.send(AUTONOMOUS_IDENTITY, "What is the perimeter gate code?")
        assert getattr(result, "accepted", True), result

        async def hydrated():
            try:
                status = await handle.identity_routing_status(AUTONOMOUS_IDENTITY)
            except Exception:
                return None
            return status if isinstance(status.session_provider, str) else None

        status = await _poll(hydrated, TURN_BUDGET_S, "a hydrated routing status for the autonomous host")
        assert status.session_provider == "anthropic", status
        ledger = _injection_ledger(state_dir)
        counts = _surface_counts(ledger, AUTONOMOUS_IDENTITY) if ledger else {}
        assert counts.get("turn", 0) == 0, (
            f"autonomous-host member must not receive turn-surface injection: {counts}"
        )
    finally:
        await rt.shutdown()
