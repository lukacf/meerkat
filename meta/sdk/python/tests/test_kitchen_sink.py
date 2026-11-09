"""Downstream Kitchen Sink: Lab Closure + Calendar Conflict + Team Coordination.

Exercises the identity-first control plane on top of real multi-agent
coordination: triage receives connector events and fans out to domain agents via
comms, gate evaluates proposed actions, team-facing delivery goes to addressable
identities. Runtime shutdown/restore, respawn, and roster reconciliation happen
mid-incident.

Agents explicitly use turn_driven mode, matching the downstream app, with comms wiring via
role_wiring rules. The test dispatches events to triage and waits for the agent
graph to process - agents use the comms `send` tool to coordinate.

Run:
    PYTHONPATH=sdk/python ANTHROPIC_API_KEY=... \
        python3 -m pytest sdk/python/tests/test_kitchen_sink.py -v --timeout=300
"""
from __future__ import annotations

import asyncio
import copy
import json
import os
import re
import warnings
from urllib.parse import urlencode
from urllib.request import Request, urlopen
from uuid import UUID

import pytest

from meerkat_meta.builder import Meta
from meerkat_meta.identity_first_models import (
    DispatchInput,
    DurableAgentSpec,
    ManagedPeerEdge,
)
from meerkat_meta.errors import RpcError, TurnTrackingUnavailableWarning

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


_skip_no_key = pytest.mark.skipif(not _anthropic_key(), reason="No Anthropic API key")
_skip_no_binary = pytest.mark.skipif(
    not os.path.isfile(_GATEWAY_BIN), reason="Gateway binary not found"
)


# ---------------------------------------------------------------------------
# Mob definition - turn-driven agents with comms wiring
# ---------------------------------------------------------------------------

_TEAM_MOB_TOML = """\
[mob]
id = "example-team"

# Wiring rules: triage is the hub, wired to all domain agents and identities.
[wiring]
auto_wire_orchestrator = false

[[wiring.role_wiring]]
a = "personal"
b = "triage"

[[wiring.role_wiring]]
a = "team_group"
b = "triage"

[[wiring.role_wiring]]
a = "triage"
b = "research"

[[wiring.role_wiring]]
a = "triage"
b = "calendar"

[[wiring.role_wiring]]
a = "triage"
b = "gate"

[[wiring.role_wiring]]
a = "gate"
b = "team_group"

# --- Profiles ---
# All profiles explicitly use turn_driven mode, matching the downstream app. The runtime
# schedules admitted messages as turns. Agents can use the comms send tool to
# forward messages to wired peers.

[profiles.personal]
model = "claude-sonnet-4-5"
runtime_mode = "turn_driven"
skills = ["personal_role"]
external_addressable = true


[profiles.personal.tools]
comms = true

[profiles.team_group]
model = "claude-sonnet-4-5"
runtime_mode = "turn_driven"
skills = ["team_group_role"]
external_addressable = true


[profiles.team_group.tools]
comms = true

[profiles.triage]
model = "claude-sonnet-4-5"
runtime_mode = "turn_driven"
skills = ["triage_role"]
external_addressable = false


[profiles.triage.tools]
comms = true

[profiles.research]
model = "claude-sonnet-4-5"
runtime_mode = "turn_driven"
skills = ["research_role"]
external_addressable = false


[profiles.research.tools]
comms = true

[profiles.calendar]
model = "claude-sonnet-4-5"
runtime_mode = "turn_driven"
skills = ["calendar_role"]
external_addressable = false


[profiles.calendar.tools]
comms = true

[profiles.gate]
model = "claude-sonnet-4-5"
runtime_mode = "turn_driven"
skills = ["gate_role"]
external_addressable = false


[profiles.gate.tools]
comms = true

# --- Prompts ---
# A member's system prompt is assembled from its profile `skills`; a bare
# `system_prompt` key under [profiles.X] is not a meerkat Profile field and
# is silently dropped.

[skills.personal_role]
source = "inline"
content = "You are a personal assistant for a team member. When you receive information, acknowledge it briefly. Keep all responses to 1-2 sentences."

[skills.team_group_role]
source = "inline"
content = "You are a team group channel. When you receive team updates, acknowledge them briefly. Keep responses to 1-2 sentences."

[skills.triage_role]
source = "inline"
content = "You are the team triage agent. When you receive events from connectors, analyze them and forward relevant information to the appropriate domain agents using the send tool. For research-related events, send to the peer with 'research' in their name. For calendar/scheduling events, send to the peer with 'calendar' in their name. You MUST use the send tool to forward information. After forwarding, summarize what you did."

[skills.research_role]
source = "inline"
content = "You are the research domain agent. You track lab schedules, closures, and logistics. When you receive lab-related events, analyze the impact (remote work needs, schedule changes) and respond with a brief assessment."

[skills.calendar_role]
source = "inline"
content = "You are the calendar domain agent. You track appointments and schedules. When you receive scheduling events or conflicts, identify the conflict and propose a solution in 1-2 sentences."

[skills.gate_role]
source = "inline"
content = "You are the gate agent. You evaluate proposed actions before they reach team members. When you receive a proposed action, briefly approve or flag concerns. Keep responses to 1-2 sentences."
"""

# ---------------------------------------------------------------------------
# Providers
# ---------------------------------------------------------------------------


class TeamRoster:
    def __init__(self, specs: list[DurableAgentSpec]):
        self._specs = list(specs)

    def update(self, specs: list[DurableAgentSpec]) -> None:
        self._specs = list(specs)

    async def roster(self, context: dict) -> list[DurableAgentSpec]:
        return list(self._specs)


class TeamTopology:
    def __init__(self, edges: list[tuple[str, str]]):
        self._edges = list(edges)

    def update(self, edges: list[tuple[str, str]]) -> None:
        self._edges = list(edges)

    async def compute_edges(self, target_identities, context) -> list[ManagedPeerEdge]:
        return [ManagedPeerEdge(a=a, b=b) for a, b in self._edges]


class TeamCustomizer:
    async def customize_build(self, context, spec, draft) -> None:
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
                f"Your wired peers: {', '.join(peers)}"
            )

    async def after_create(self, identity, session_id, context) -> None:
        pass


# ---------------------------------------------------------------------------
# Roster and topology
# ---------------------------------------------------------------------------

_ROSTER = [
    DurableAgentSpec(identity="identity:luka", profile="personal", addressability="addressable"),
    DurableAgentSpec(identity="identity:alice", profile="personal", addressability="addressable"),
    DurableAgentSpec(identity="team-group:main", profile="team_group", addressability="addressable"),
    DurableAgentSpec(identity="triage:main", profile="triage", addressability="internal_only"),
    DurableAgentSpec(identity="domain:research", profile="research", addressability="internal_only"),
    DurableAgentSpec(identity="domain:calendar", profile="calendar", addressability="internal_only"),
    DurableAgentSpec(identity="gate:main", profile="gate", addressability="internal_only"),
]

_EDGES = [
    ("identity:luka", "triage:main"),
    ("identity:alice", "triage:main"),
    ("team-group:main", "triage:main"),
    ("triage:main", "domain:research"),
    ("triage:main", "domain:calendar"),
    ("triage:main", "gate:main"),
    ("gate:main", "team-group:main"),
]


# ---------------------------------------------------------------------------
# Boot helper
# ---------------------------------------------------------------------------


async def _boot(state_dir, roster, topology, customizer):
    return await (
        Meta.builder()
        .gateway(_GATEWAY_BIN)
        .mob_inline(_TEAM_MOB_TOML)
        .persistent_state(state_dir)
        .http_listen("127.0.0.1:0")
        .console_auth_required(False)
        .roster(roster)
        .topology_provider(topology)
        .agent_customizer(customizer)
        .build()
    )


_LEAD_CLOSURE_NOTICE = (
    "Hillside Lab closed tomorrow (pipe burst). The team must work remotely. "
    "This affects your morning schedule."
)

# The existing source-string incident correlation canonicalizes to this UUID.
# Supplying the canonical value preserves identity and lets the test match the
# public interaction_id directly, without reimplementing gateway normalization.
_INCIDENT_DISPATCH_ID = "a1ebb4f0-201d-50a3-9733-590e176210c4"
_GATE_DISPATCH_ID = "e8d33c60-3d31-4f18-8c2c-07eb3aaf4ed9"


def _content_text(content):
    if isinstance(content, str):
        return content
    if not isinstance(content, list):
        return ""
    return "\n".join(
        block["text"] for block in content
        if isinstance(block, dict) and block.get("type") == "text"
        and isinstance(block.get("text"), str)
    )


def _research_delivery_frame_matches(frame, identity, session_id, sender_peer_id):
    if (
        frame.get("identity") != identity
        or frame.get("session_id") != session_id
        or frame.get("kind") != "system_notice"
        or frame.get("source", {}).get("kind") != "session_history"
    ):
        return False
    for block in frame.get("payload", {}).get("blocks", []):
        if not isinstance(block, dict):
            continue
        if (
            block.get("type") != "comms"
            or block.get("kind") not in ("message", "request")
            or block.get("direction") != "incoming"
            or (block.get("peer") or {}).get("id") != sender_peer_id
        ):
            continue
        text = _content_text(block.get("content")).lower()
        if "hillside" in text and "closed" in text and "pipe" in text and "burst" in text:
            return True
    return False


def _closure_notice_frame_matches(frame, identity, session_id):
    return (
        frame.get("identity") == identity
        and frame.get("session_id") == session_id
        and frame.get("kind") == "user_input"
        and frame.get("source", {}).get("kind") == "session_history"
        and _content_text(frame.get("payload", {}).get("content")) == _LEAD_CLOSURE_NOTICE
    )


def _remembers_lab_closure(text):
    text = text.lower().replace("\u2019", "'")
    uncertain = any(phrase in text for phrase in (
        "do not know", "don't know", "not sure", "uncertain", "whether", "might be", "may be",
    ))
    negated = re.search(r"\b(?:not|isn't|won't be) closed\b", text)
    closure = re.search(r"\bclosed\b|\b(?:not (?:be )?|won't be )open\b", text)
    return (
        bool(closure) and "pipe" in text and "burst" in text
        and not uncertain and not negated and "?" not in text
    )


def _timeline_time():
    return asyncio.get_running_loop().time()


async def _timeline_pause(delay):
    await asyncio.sleep(delay)


async def _read_timeline_page(runtime, params, remaining):
    base = runtime.rust_http_base_url
    assert base, "gateway must expose its loopback console history endpoint"

    def read_page():
        with urlopen(
            base.rstrip("/") + "/console/timeline?" + urlencode(params),
            timeout=min(10, remaining),
        ) as response:
            return json.load(response)

    return await asyncio.to_thread(read_page)


async def _wait_for_timeline(runtime, identity, resolve, *, deadline):
    """Poll public frames under one absolute budget, including every page/read."""
    frames_by_id = {}
    while _timeline_time() < deadline:
        params = {"identity": identity, "mode": "since", "limit": 400}
        seen_cursors = set()
        while _timeline_time() < deadline:
            remaining = deadline - _timeline_time()
            page = await asyncio.wait_for(
                _read_timeline_page(runtime, params, remaining), timeout=remaining,
            )
            frames = page.get("frames")
            assert isinstance(frames, list), f"invalid console timeline page: {page}"
            for frame in frames:
                frames_by_id[frame["id"]] = frame
            if _timeline_time() >= deadline:
                break
            result = resolve(list(frames_by_id.values()))
            if result is not None:
                return result
            cursor = page.get("next_cursor")
            if page.get("exhausted") or not frames or cursor is None:
                break
            assert cursor not in seen_cursors, "console timeline pagination did not advance"
            seen_cursors.add(cursor)
            params["after"] = cursor
        await _timeline_pause(min(0.25, max(0, deadline - _timeline_time())))
    raise AssertionError(f"{identity}: required correlated committed evidence missing at deadline")


async def _wait_for_history_frame(runtime, identity, predicate, *, timeout=20):
    return await _wait_for_timeline(
        runtime, identity,
        lambda frames: next((frame for frame in frames if predicate(frame)), None),
        deadline=_timeline_time() + timeout,
    )


def _completed_interaction(frames, identity, session_id, interaction_id):
    owned = [frame for frame in frames if (
        frame.get("identity") == identity and frame.get("session_id") == session_id
        and frame.get("interaction_id") == interaction_id and frame.get("run_id")
    )]
    started = {frame["run_id"] for frame in owned if frame.get("kind") == "run_started"}
    for complete in owned:
        if (complete.get("kind") != "interaction_complete"
                or complete.get("status") != "completed" or complete["run_id"] not in started):
            continue
        output = complete.get("payload", {}).get("result")
        if not isinstance(output, str) or not output.strip():
            continue
        committed = next((frame for frame in owned if (
            frame.get("kind") == "text_complete"
            and frame.get("source", {}).get("kind") == "session_history"
            and frame["run_id"] == complete["run_id"]
            and frame.get("payload", {}).get("text") == output
        )), None)
        if committed:
            return {"run_id": complete["run_id"], "output": output, "history": committed}
    return None


def _research_fanout(frames, triage_session, dispatch_id, research_peer_id):
    completed = _completed_interaction(frames, "triage:main", triage_session, dispatch_id)
    if completed is None:
        return None
    owned = [frame for frame in frames if (
        frame.get("identity") == "triage:main" and frame.get("interaction_id") == dispatch_id
        and frame.get("run_id") == completed["run_id"]
        # Tool events omit session_id in the real gateway. Their exact run is
        # already anchored above to triage_session; reject contradictory owners.
        and frame.get("session_id") in (None, triage_session)
    )]
    for call in owned:
        payload = call.get("payload", {})
        name = payload.get("name")
        if call.get("kind") != "tool_call_requested" or name not in ("send_message", "send_request"):
            continue
        args = payload.get("args", {})
        if args.get("peer_id") != research_peer_id:
            continue
        incident = args.get("body") if name == "send_message" else json.dumps(args.get("params"))
        if not isinstance(incident, str) or not all(
            word in incident.lower() for word in ("hillside", "closed", "pipe", "burst")
        ):
            continue
        tool_id = payload.get("tool_call_id")
        if not tool_id:
            continue
        for result in owned:
            value = result.get("payload", {})
            # The send receipt is the tool's output, which travels on the result
            # event; the execution fact for the same call carries no body.
            if (result.get("kind") not in ("tool_result_received", "tool_execution_completed")
                    or value.get("is_error")
                    or value.get("tool_call_id") != tool_id or value.get("name") != name):
                continue
            try:
                response = json.loads(value["result"])
            except (KeyError, TypeError, ValueError):
                continue
            if not isinstance(response, dict) or response.get("status") != "sent":
                continue
            receipt = response.get("receipt", {})
            kind = "peer_message" if name == "send_message" else "peer_request"
            if not isinstance(receipt, dict) or receipt.get("kind") != kind + "_sent":
                continue
            delivery = receipt.get("delivery")
            if not isinstance(delivery, dict) or delivery.get("durably_resolved") != {"outcome": "accepted"}:
                continue
            envelope = receipt.get("envelope_id")
            interaction = envelope if name == "send_message" else receipt.get("interaction_id")
            if isinstance(envelope, str) and envelope and isinstance(interaction, str) and interaction:
                return {**completed, "envelope_id": envelope, "interaction_id": interaction}
    return None


def _research_incident_result(frames, session_id, triage_peer_id, interaction_id):
    completed = _completed_interaction(frames, "domain:research", session_id, interaction_id)
    if completed is None:
        return None
    history = next((frame for frame in frames if _research_delivery_frame_matches(
        frame, "domain:research", session_id, triage_peer_id,
    )), None)
    return {**completed, "incident_history": history} if history else None


def _conversation_result(frames, identity, session_id, interaction_id, content):
    completed = _completed_interaction(frames, identity, session_id, interaction_id)
    if completed is None:
        return None
    sent = next((frame for frame in frames if (
        frame.get("identity") == identity and frame.get("session_id") == session_id
        and frame.get("interaction_id") == interaction_id
        and frame.get("kind") == "user_input"
        and frame.get("source", {}).get("kind") == "session_history"
        and _content_text(frame.get("payload", {}).get("content")) == content
    )), None)
    return {**completed, "input_history": sent} if sent else None


async def _send_console_and_wait(runtime, identity, session_id, content, key, *, timeout):
    """Use the public send acceptance's interaction ID for conversational turns."""
    deadline = _timeline_time() + timeout
    base = runtime.rust_http_base_url
    assert base, "gateway must expose its loopback console send endpoint"

    def send():
        request = Request(base.rstrip("/") + "/console/send", data=json.dumps({
            "identity": identity, "content": content, "origin": "team-smoke",
            "idempotency_key": key, "handling_mode": "queue",
        }).encode(), headers={"Content-Type": "application/json"}, method="POST")
        with urlopen(request, timeout=min(10, timeout)) as response:
            return json.load(response)

    accepted = await asyncio.wait_for(asyncio.to_thread(send), timeout=timeout)
    assert accepted.get("identity") == identity and accepted.get("session_id") == session_id
    interaction_id = accepted.get("interaction_id")
    assert interaction_id, "console send must return its accepted interaction ID"
    return await _wait_for_timeline(
        runtime, identity,
        lambda frames: _conversation_result(frames, identity, session_id, interaction_id, content),
        deadline=deadline,
    )


async def _dispatch_sdk_and_wait(agent, dispatch_input, *, deadline):
    """Require this fixture's admission ticket under the phase's total budget."""
    remaining = max(0, deadline - _timeline_time())
    admission = await asyncio.wait_for(
        agent.dispatch(dispatch_input, track_turn=True), timeout=remaining,
    )
    ticket = getattr(admission, "turn_ticket", None)
    assert isinstance(ticket, str) and ticket, (
        f"dispatch returned no turn ticket: {getattr(admission, 'turn_unavailable', None)}"
    )
    try:
        UUID(ticket)
    except ValueError as error:
        raise AssertionError(f"dispatch returned malformed turn ticket: {ticket!r}") from error
    # Ticket ownership comes from the admission receipt. Correlation remains
    # the independent committed-history oracle; never derive one from the other.
    remaining = max(0, deadline - _timeline_time())
    return await asyncio.wait_for(
        agent.wait_for_output(turn=ticket, timeout=remaining), timeout=remaining,
    )


async def _wait_for_research_sdk_and_verify(
    runtime, research, baseline, session_id, triage_peer_id, interaction_id, *, deadline,
):
    """Observe peer work through the SDK and verify the exact committed turn.

    Comms receipts do not expose SDK turn tickets. The research cursor is captured
    before triage dispatch and is only an identity-wide observation barrier;
    the accepted peer interaction and committed run remain the ownership proof.
    """
    assert baseline is not None, "research completion cursor is required before peer delivery"
    remaining = max(0, deadline - _timeline_time())
    sdk_output = await asyncio.wait_for(
        research.wait_for_output(after=baseline, timeout=remaining), timeout=remaining,
    )
    result = await _wait_for_timeline(
        runtime, "domain:research",
        lambda frames: _research_incident_result(
            frames, session_id, triage_peer_id, interaction_id,
        ),
        deadline=deadline,
    )
    assert sdk_output == result["output"], "SDK research waiter returned an unrelated turn"
    return result


def _sdk_conversation_result(frames, identity, session_id, content):
    inputs = [frame for frame in frames if (
        frame.get("identity") == identity and frame.get("session_id") == session_id
        and frame.get("kind") == "user_input"
        and frame.get("source", {}).get("kind") == "session_history"
        and _content_text(frame.get("payload", {}).get("content")) == content
    )]
    assert len(inputs) <= 1, "scenario input was admitted more than once"
    if not inputs:
        return None
    sent = inputs[0]
    interaction = sent.get("interaction_id")
    if interaction:
        return _conversation_result(frames, identity, session_id, interaction, content)
    if interaction is not None:
        return None

    # Plain SDK sends carry a ticket but need not carry an interaction ID. The
    # unique authored fixture locates a content run start; that nonempty run ID
    # owns the terminal and committed assistant evidence. Text alone never does.
    starts = [frame for frame in frames if (
        frame.get("identity") == identity and frame.get("session_id") == session_id
        and frame.get("kind") == "run_started"
        and frame.get("source", {}).get("kind") == "console_event"
        and isinstance(frame.get("run_id"), str) and frame["run_id"]
        and frame.get("payload", {}).get("input", {}).get("kind") == "content"
        and _content_text(frame["payload"]["input"].get("content")) == content
    )]
    assert len({frame["run_id"] for frame in starts}) <= 1, "scenario input was admitted more than once"
    if (not starts or starts[0].get("interaction_id") is not None
            or sent.get("run_id") not in (None, starts[0]["run_id"])
            or sent.get("status") != "completed"):
        return None
    run_id = starts[0]["run_id"]
    owned = [frame for frame in frames if (
        frame.get("identity") == identity and frame.get("session_id") == session_id
        and frame.get("run_id") == run_id and frame.get("interaction_id") is None
    )]
    for terminal in owned:
        if (terminal.get("kind") != "interaction_complete" or terminal.get("status") != "completed"
                or terminal.get("source", {}).get("kind") != "console_event"
                or terminal.get("payload", {}).get("source_event_type") != "run_completed"):
            continue
        output = terminal.get("payload", {}).get("result")
        assistant_id = terminal.get("payload", {}).get("assistant_message_id")
        if (not isinstance(output, str) or not output.strip()
                or not isinstance(assistant_id, str) or not assistant_id):
            continue
        history = next((frame for frame in owned if (
            frame.get("kind") == "text_complete" and frame.get("status") == "completed"
            and frame.get("source", {}).get("kind") == "session_history"
            and frame.get("payload", {}).get("assistant_message_id") == assistant_id
            and frame.get("payload", {}).get("text") == output
        )), None)
        if history:
            return {"run_id": run_id, "output": output, "history": history, "input_history": sent}
    return None


async def _send_sdk_and_verify(runtime, agent, identity, session_id, content, *, timeout):
    """Verify the ticketed SDK result against its terminal and committed history."""
    deadline = _timeline_time() + timeout
    with warnings.catch_warnings():
        warnings.simplefilter("error", TurnTrackingUnavailableWarning)
        sdk_output = await asyncio.wait_for(
            agent.send_and_wait(content, timeout=timeout), timeout=timeout,
        )
    result = await _wait_for_timeline(
        runtime, identity,
        lambda frames: _sdk_conversation_result(frames, identity, session_id, content),
        deadline=deadline,
    )
    assert sdk_output == result["output"], (
        f"SDK send_and_wait returned another turn for {identity}: "
        f"{sdk_output!r}; committed requested run {result['run_id']}: {result['output']!r}"
    )
    return result


def _research_history_fixture():
    return {
        "identity": "domain:research",
        "session_id": "research-session",
        "kind": "system_notice",
        "source": {"kind": "session_history"},
        "payload": {"blocks": [{
            "type": "comms",
            "kind": "request",
            "direction": "incoming",
            "peer": {"id": "triage-peer-id", "display_name": "example-team/triage/mk--triage_cmain"},
            "content": [{
                "type": "text",
                "text": "Hillside Lab is closed tomorrow due to a pipe burst.",
            }],
        }]},
    }


@pytest.mark.parametrize("mismatch", [
    None, "old_output", "other_identity", "other_session", "outgoing",
    "other_peer", "terminal_response", "missing_incident", "body_only",
])
def test_kitchen_research_history_oracle_requires_received_incident(mismatch):
    frame = copy.deepcopy(_research_history_fixture())
    block = frame["payload"]["blocks"][0]
    if mismatch == "old_output":
        frame["source"]["kind"] = "console_event"
        frame["kind"] = "text_complete"
    elif mismatch == "other_identity":
        frame["identity"] = "domain:calendar"
    elif mismatch == "other_session":
        frame["session_id"] = "previous-research-session"
    elif mismatch == "outgoing":
        block["direction"] = "outgoing"
    elif mismatch == "other_peer":
        block["peer"]["id"] = "calendar-peer-id"
    elif mismatch == "terminal_response":
        block["kind"] = "response_terminal"
    elif mismatch == "missing_incident":
        block["content"][0]["text"] = "Ready to track lab closures."
    elif mismatch == "body_only":
        frame["payload"]["body"] = block["content"][0]["text"]
        block["content"] = []
    assert _research_delivery_frame_matches(
        frame, "domain:research", "research-session", "triage-peer-id"
    ) is (mismatch is None)


@pytest.mark.parametrize("text, expected", [
    ("The lab is closed tomorrow because a pipe burst.", True),
    ("Hillside Lab will not be open due to burst pipes.", True),
    ("The lab won't be open tomorrow because of the pipe burst.", True),
    ("Is the lab open or closed tomorrow?", False),
    ("The lab is closed tomorrow.", False),
    ("The lab is open despite the pipe burst.", False),
    ("I do not know whether the lab is closed after the pipe burst.", False),
    ("Is the lab closed due to a pipe burst?", False),
    ("The lab is not closed after the pipe burst.", False),
])
def test_kitchen_continuity_oracle_requires_closure_and_cause(text, expected):
    assert _remembers_lab_closure(text) is expected


@pytest.mark.parametrize("mismatch", [None, "other_session", "live_echo", "changed_text"])
def test_kitchen_notice_oracle_requires_exact_persisted_content(mismatch):
    frame = {
        "identity": "identity:luka",
        "session_id": "luka-session",
        "kind": "user_input",
        "source": {"kind": "session_history", "source_cursor": "luka-session:4"},
        "payload": {"content": [{"type": "text", "text": _LEAD_CLOSURE_NOTICE}]},
    }
    if mismatch == "other_session":
        frame["session_id"] = "unrelated-session"
    elif mismatch == "live_echo":
        frame["source"]["kind"] = "send"
    elif mismatch == "changed_text":
        frame["payload"]["content"][0]["text"] = "Is the lab closed?"
    assert _closure_notice_frame_matches(frame, "identity:luka", "luka-session") is (mismatch is None)


def _interaction_fixture(identity="triage:main", session="triage-session",
                         interaction="dispatch-id", run="incident-run"):
    common = {
        "identity": identity, "session_id": session,
        "interaction_id": interaction, "run_id": run,
    }
    return [
        {**common, "id": f"{run}-start", "kind": "run_started",
         "source": {"kind": "console_event"}, "payload": {}},
        {**common, "id": f"{run}-complete", "kind": "interaction_complete",
         "status": "completed", "source": {"kind": "console_event"},
         "payload": {"result": "incident processed"}},
        {**common, "id": f"{run}-history", "kind": "text_complete",
         "source": {"kind": "session_history", "source_cursor": f"{session}:9"},
         "payload": {"text": "incident processed"}},
    ]


@pytest.mark.parametrize("mismatch", [
    None, "unrelated_completion", "other_run", "other_session", "other_identity",
    "live_text_only", "history_only", "no_run_start", "failed",
])
def test_kitchen_correlated_completion_requires_exact_run_and_committed_output(mismatch):
    frames = _interaction_fixture()
    if mismatch == "unrelated_completion":
        frames[1]["interaction_id"] = "startup-id"
    elif mismatch == "other_run":
        frames[2]["run_id"] = "startup-run"
    elif mismatch == "other_session":
        frames[1]["session_id"] = "old-session"
    elif mismatch == "other_identity":
        frames[2]["identity"] = "domain:calendar"
    elif mismatch == "live_text_only":
        frames[2]["source"]["kind"] = "console_event"
    elif mismatch == "history_only":
        frames.pop(1)
    elif mismatch == "no_run_start":
        frames.pop(0)
    elif mismatch == "failed":
        frames[1]["status"] = "failed"
    result = _completed_interaction(frames, "triage:main", "triage-session", "dispatch-id")
    assert (result is not None) is (mismatch is None)
    if result:
        assert result["run_id"] == "incident-run"
        assert result["output"] == "incident processed"


def _research_fanout_fixture():
    frames = _interaction_fixture()
    common = {"identity": "triage:main", "interaction_id": "dispatch-id",
              "run_id": "incident-run", "source": {"kind": "console_event"}}
    frames.extend([
        {**common, "id": "send-call", "kind": "tool_call_requested", "payload": {
            "name": "send_message", "tool_call_id": "send-tool",
            "args": {"peer_id": "research-peer-id", "body":
                     "Hillside Lab is closed tomorrow due to a pipe burst."},
        }},
        {**common, "id": "send-result", "kind": "tool_result_received", "payload": {
            "name": "send_message", "tool_call_id": "send-tool", "is_error": False,
            "result": json.dumps({"status": "sent", "kind": "peer_message", "receipt": {
                "kind": "peer_message_sent", "envelope_id": "research-envelope",
                "delivery": {"durably_resolved": {"outcome": "accepted"}},
            }}),
        }},
    ])
    return frames


@pytest.mark.parametrize("mismatch", [
    None, "unrelated_dispatch", "other_run", "other_peer", "other_call",
    "rejected", "queued_only", "tool_error", "unrelated_incident", "wrong_receipt_kind",
    "other_call_session", "other_result_session",
])
def test_kitchen_fanout_requires_accepted_receipt_from_exact_dispatch_tool(mismatch):
    frames = _research_fanout_fixture()
    call, result = frames[-2:]
    receipt = json.loads(result["payload"]["result"])
    if mismatch == "unrelated_dispatch":
        call["interaction_id"] = "startup-id"
    elif mismatch == "other_run":
        result["run_id"] = "startup-run"
    elif mismatch == "other_peer":
        call["payload"]["args"]["peer_id"] = "calendar-peer-id"
    elif mismatch == "other_call":
        result["payload"]["tool_call_id"] = "other-tool"
    elif mismatch == "rejected":
        receipt["receipt"]["delivery"]["durably_resolved"]["outcome"] = "rejected"
    elif mismatch == "queued_only":
        receipt["receipt"]["delivery"] = "queued"
    elif mismatch == "tool_error":
        result["payload"]["is_error"] = True
    elif mismatch == "unrelated_incident":
        call["payload"]["args"]["body"] = "Ready for lab tasks."
    elif mismatch == "wrong_receipt_kind":
        receipt["receipt"]["kind"] = "peer_response_sent"
    elif mismatch == "other_call_session":
        call["session_id"] = "other-triage-session"
    elif mismatch == "other_result_session":
        result["session_id"] = "other-triage-session"
    result["payload"]["result"] = json.dumps(receipt)
    accepted = _research_fanout(
        frames, "triage-session", "dispatch-id", "research-peer-id"
    )
    assert (accepted is not None) is (mismatch is None)
    if accepted:
        assert accepted["envelope_id"] == "research-envelope"
        assert accepted["interaction_id"] == "research-envelope"


def test_kitchen_fanout_reads_the_receipt_from_the_result_event():
    frames = _research_fanout_fixture()
    result = frames[-1]
    execution_fact = {**result, "id": "send-completed", "kind": "tool_execution_completed",
                      "payload": {"name": "send_message", "tool_call_id": "send-tool",
                                  "is_error": False, "duration_ms": 3}}
    assert _research_fanout([*frames[:-1], execution_fact],
                            "triage-session", "dispatch-id", "research-peer-id") is None
    accepted = _research_fanout([*frames[:-1], execution_fact, result],
                                "triage-session", "dispatch-id", "research-peer-id")
    assert accepted and accepted["envelope_id"] == "research-envelope"


def test_kitchen_fanout_accepts_explicit_matching_session():
    frames = _research_fanout_fixture()
    for frame in frames[-2:]:
        frame["session_id"] = "triage-session"
    assert _research_fanout(frames, "triage-session", "dispatch-id", "research-peer-id")


def test_kitchen_request_fanout_uses_receipt_interaction_not_envelope_identity():
    frames = _research_fanout_fixture()
    frames[-2]["payload"].update(name="send_request", args={
        "peer_id": "research-peer-id", "intent": "lab.closure",
        "params": {"event": "Hillside closed tomorrow due to pipe burst"},
    })
    frames[-1]["payload"].update(name="send_request", result=json.dumps({
        "status": "sent", "kind": "peer_request", "receipt": {
            "kind": "peer_request_sent", "envelope_id": "transport-envelope",
            "interaction_id": "research-request", "stream_reserved": True,
            "delivery": {"durably_resolved": {"outcome": "accepted"}},
        },
    }))
    accepted = _research_fanout(frames, "triage-session", "dispatch-id", "research-peer-id")
    assert accepted["envelope_id"] == "transport-envelope"
    assert accepted["interaction_id"] == "research-request"


@pytest.mark.parametrize("mismatch", [None, "unrelated_completion", "no_history", "live_ingest"])
def test_kitchen_research_requires_envelope_run_completion_and_durable_incident(mismatch):
    frames = _interaction_fixture("domain:research", "research-session", "research-envelope")
    notice = _research_history_fixture()
    notice["id"] = "research-notice"
    frames.append(notice)
    if mismatch == "unrelated_completion":
        frames[1]["interaction_id"] = "startup-envelope"
    elif mismatch == "no_history":
        frames.pop()
    elif mismatch == "live_ingest":
        notice["source"]["kind"] = "console_event"
        notice["kind"] = "peer_content_ingested"
    result = _research_incident_result(
        frames, "research-session", "triage-peer-id", "research-envelope"
    )
    assert (result is not None) is (mismatch is None)


@pytest.mark.parametrize("mismatch", [None, "other_interaction", "live_echo", "changed_content"])
def test_kitchen_conversation_requires_accepted_interaction_input_history(mismatch):
    frames = _interaction_fixture("identity:luka", "luka-session", "accepted-send")
    notice = {
        "identity": "identity:luka", "session_id": "luka-session",
        "interaction_id": "accepted-send", "kind": "user_input",
        "source": {"kind": "session_history"},
        "payload": {"content": [{"type": "text", "text": _LEAD_CLOSURE_NOTICE}]},
    }
    frames.append(notice)
    if mismatch == "other_interaction":
        notice["interaction_id"] = "prior-send"
    elif mismatch == "live_echo":
        notice["source"]["kind"] = "send"
    elif mismatch == "changed_content":
        notice["payload"]["content"] = "Ready for lab tasks."
    result = _conversation_result(
        frames, "identity:luka", "luka-session", "accepted-send", _LEAD_CLOSURE_NOTICE,
    )
    assert (result is not None) is (mismatch is None)


@pytest.mark.asyncio
async def test_kitchen_timeline_wait_ignores_startup_then_requires_commit(monkeypatch):
    startup = _interaction_fixture(interaction="startup-id", run="startup-run")
    actual = _interaction_fixture()
    pages = [startup, startup + actual[:2], startup + actual]
    reads = []

    async def read_page(runtime, params, remaining):
        reads.append(remaining)
        return {"frames": pages.pop(0), "exhausted": True}

    async def no_sleep(delay):
        pass

    monkeypatch.setitem(globals(), "_read_timeline_page", read_page)
    monkeypatch.setitem(globals(), "_timeline_pause", no_sleep)
    result = await _wait_for_timeline(
        object(), "triage:main",
        lambda frames: _completed_interaction(frames, "triage:main", "triage-session", "dispatch-id"),
        deadline=_timeline_time() + 90,
    )
    assert len(reads) == 3
    assert result["run_id"] == "incident-run"


@pytest.mark.asyncio
async def test_kitchen_timeline_deadline_does_not_reset_on_progress_or_pagination(monkeypatch):
    clock = [0.0]
    reads = []

    async def read_page(runtime, params, remaining):
        reads.append((dict(params), remaining))
        clock[0] += 3
        return {"frames": [{"id": str(len(reads))}], "next_cursor": f"opaque-{len(reads)}"}

    async def pause(delay):
        clock[0] += delay

    monkeypatch.setitem(globals(), "_timeline_time", lambda: clock[0])
    monkeypatch.setitem(globals(), "_read_timeline_page", read_page)
    monkeypatch.setitem(globals(), "_timeline_pause", pause)
    with pytest.raises(AssertionError, match="deadline"):
        await _wait_for_timeline(object(), "triage:main", lambda frames: None, deadline=9)
    assert [remaining for _, remaining in reads] == [9, 6, 3]
    assert [params.get("after") for params, _ in reads] == [None, "opaque-1", "opaque-2"]
    assert clock[0] == 9


@pytest.mark.asyncio
@pytest.mark.parametrize("mismatch", [None, "other_identity", "other_session"])
async def test_kitchen_console_send_uses_acceptance_id_and_original_deadline(monkeypatch, mismatch):
    from types import SimpleNamespace

    clock = [0.0]
    requests = []
    remaining_budgets = []
    accepted = {"identity": "identity:luka", "session_id": "luka-session",
                "interaction_id": "accepted-send"}
    if mismatch == "other_identity":
        accepted["identity"] = "identity:alice"
    elif mismatch == "other_session":
        accepted["session_id"] = "old-session"

    class Response:
        def __enter__(self):
            return self

        def __exit__(self, *args):
            pass

        def read(self):
            return json.dumps(accepted).encode()

    def send(request, timeout):
        requests.append((request, timeout))
        clock[0] = 5
        return Response()

    async def read_page(runtime, params, remaining):
        remaining_budgets.append(remaining)
        frames = _interaction_fixture("identity:luka", "luka-session", "accepted-send")
        frames.append({
            "id": "input-history", "identity": "identity:luka", "session_id": "luka-session",
            "interaction_id": "accepted-send", "kind": "user_input",
            "source": {"kind": "session_history"}, "payload": {"content": _LEAD_CLOSURE_NOTICE},
        })
        return {"frames": frames, "exhausted": True}

    monkeypatch.setitem(globals(), "urlopen", send)
    monkeypatch.setitem(globals(), "_timeline_time", lambda: clock[0])
    monkeypatch.setitem(globals(), "_read_timeline_page", read_page)
    operation = _send_console_and_wait(
        SimpleNamespace(rust_http_base_url="http://127.0.0.1:8080"),
        "identity:luka", "luka-session", _LEAD_CLOSURE_NOTICE,
        "research:luka-notice-1", timeout=60,
    )
    if mismatch:
        with pytest.raises(AssertionError):
            await operation
        assert not remaining_budgets
    else:
        result = await operation
        assert result["history"]["interaction_id"] == "accepted-send"
        assert remaining_budgets == [55]
    request, timeout = requests[0]
    assert request.full_url == "http://127.0.0.1:8080/console/send"
    assert json.loads(request.data)["idempotency_key"] == "research:luka-notice-1"
    assert timeout == 10


@pytest.mark.asyncio
@pytest.mark.parametrize("sdk_output", ["incident processed", "unrelated peer reply"])
async def test_kitchen_sdk_completion_must_match_the_requested_committed_run(monkeypatch, sdk_output):
    from types import SimpleNamespace
    calls = []
    clock = [0.0]

    async def send_and_wait(content, timeout):
        calls.append((content, timeout))
        clock[0] = 5
        return sdk_output

    async def read_page(runtime, params, remaining):
        assert remaining == 55
        frames = _interaction_fixture("identity:luka", "luka-session", "sdk-input")
        frames.append({"id": "sdk-user", "identity": "identity:luka", "session_id": "luka-session",
                       "interaction_id": "sdk-input", "kind": "user_input", "source": {"kind": "session_history"},
                       "payload": {"content": "Unique SDK input"}})
        return {"frames": frames, "exhausted": True}

    monkeypatch.setitem(globals(), "_timeline_time", lambda: clock[0])
    monkeypatch.setitem(globals(), "_read_timeline_page", read_page)
    operation = _send_sdk_and_verify(object(), SimpleNamespace(send_and_wait=send_and_wait),
                                    "identity:luka", "luka-session", "Unique SDK input", timeout=60)
    if sdk_output == "unrelated peer reply":
        with pytest.raises(AssertionError, match="returned another turn"):
            await operation
    else:
        assert (await operation)["run_id"] == "incident-run"
    assert calls == [("Unique SDK input", 60)]


def _sdk_run_only_fixture():
    """Match the retained candidate's plain SDK send without interaction IDs."""
    frames = _interaction_fixture(
        "identity:luka", "luka-session", interaction=None, run="requested-run",
    )
    frames[0]["payload"] = {
        "input": {"kind": "content", "content": _LEAD_CLOSURE_NOTICE},
    }
    for frame in frames[1:]:
        frame["status"] = "completed"
        frame["payload"]["assistant_message_id"] = "requested-assistant"
    frames[1]["payload"]["source_event_type"] = "run_completed"
    frames.append({
        "id": "sdk-user", "identity": "identity:luka", "session_id": "luka-session",
        "interaction_id": None, "run_id": None, "kind": "user_input", "status": "completed",
        "source": {"kind": "session_history"},
        "payload": {"content": [{"type": "text", "text": _LEAD_CLOSURE_NOTICE}]},
    })
    return frames


@pytest.mark.asyncio
@pytest.mark.parametrize("sdk_output", ["incident processed", "unrelated peer reply"])
@pytest.mark.parametrize("intermediate", [False, True])
async def test_kitchen_sdk_run_only_lineage_requires_own_committed_output(monkeypatch, sdk_output, intermediate):
    from functools import partial
    from meerkat_meta.runtime import IdentityAgentHandle
    from .test_identity_first_turn_tickets import TicketTransport, _completed, _make_runtime, _sent

    ticket = "ac8832d0-ce0e-43cb-810a-fbd30f65be4f"
    transport = TicketTransport(
        sends=[_sent(ticket)], turn_results={ticket: [_completed(sdk_output)]},
    )
    handle = IdentityAgentHandle(_make_runtime(transport), "identity:luka")
    monkeypatch.setattr(handle, "send_and_wait", partial(handle.send_and_wait, poll_interval=0.001))
    clock = [0.0]

    async def read_page(runtime, params, remaining):
        assert transport.waited_turns() == [{"identity": "identity:luka", "ticket": ticket}]
        foreign = _interaction_fixture("identity:luka", "luka-session", None, "peer-run")
        foreign[0]["payload"] = {"input": {"kind": "peer_message", "content": _LEAD_CLOSURE_NOTICE}}
        if intermediate:
            earlier = copy.deepcopy(_sdk_run_only_fixture()[2])
            earlier["id"] = "earlier-identical-text"
            earlier["payload"]["assistant_message_id"] = "earlier-assistant"
            foreign.append(earlier)
        return {"frames": foreign + _sdk_run_only_fixture(), "exhausted": True}

    async def pause(delay):
        clock[0] = 60

    monkeypatch.setitem(globals(), "_read_timeline_page", read_page)
    monkeypatch.setitem(globals(), "_timeline_time", lambda: clock[0])
    monkeypatch.setitem(globals(), "_timeline_pause", pause)
    operation = _send_sdk_and_verify(
        object(), handle, "identity:luka", "luka-session", _LEAD_CLOSURE_NOTICE, timeout=60,
    )
    if sdk_output == "unrelated peer reply":
        with pytest.raises(AssertionError, match="returned another turn"):
            await operation
    else:
        result = await operation
        assert result["run_id"] == "requested-run"
        assert result["input_history"]["id"] == "sdk-user"
        assert result["history"]["payload"]["assistant_message_id"] == "requested-assistant"
    assert transport.params_of("meta/send")[0]["track_turn"] is True
    assert transport.params_of("meta/inspect_identity") == []


@pytest.mark.asyncio
@pytest.mark.parametrize("mismatch", [
    "missing_input", "live_input", "changed_input", "other_input_session", "other_input_run",
    "other_input_identity", "uncommitted_input", "input_interaction", "empty_input_interaction",
    "missing_start", "missing_run", "other_start_session", "other_start_identity", "peer_start",
    "changed_start_content", "start_interaction", "duplicate_input", "duplicate_start",
    "missing_terminal", "failed_terminal", "empty_terminal", "other_terminal_run",
    "other_terminal_session", "other_terminal_identity", "terminal_interaction", "terminal_assistant",
    "mirrored_terminal", "history_terminal",
    "missing_history", "live_history", "other_history_run", "other_history_session",
    "other_history_identity", "history_interaction", "history_assistant",
    "uncommitted_history", "intermediate_history",
])
async def test_kitchen_sdk_run_only_lineage_rejects_unowned_or_uncommitted_frames(monkeypatch, mismatch):
    from types import SimpleNamespace

    frames = _sdk_run_only_fixture()
    start, terminal, history, user = frames
    changes = {
        "live_input": (user["source"], "kind", "send"),
        "changed_input": (user["payload"], "content", "Earlier input"),
        "other_input_session": (user, "session_id", "old-session"),
        "other_input_run": (user, "run_id", "other-run"),
        "other_input_identity": (user, "identity", "identity:alice"),
        "uncommitted_input": (user, "status", "delivered"),
        "input_interaction": (user, "interaction_id", "other-interaction"),
        "empty_input_interaction": (user, "interaction_id", ""),
        "missing_run": (start, "run_id", None),
        "other_start_session": (start, "session_id", "old-session"),
        "other_start_identity": (start, "identity", "identity:alice"),
        "peer_start": (start["payload"]["input"], "kind", "peer_message"),
        "changed_start_content": (start["payload"]["input"], "content", "Earlier input"),
        "start_interaction": (start, "interaction_id", "other-interaction"),
        "failed_terminal": (terminal, "status", "failed"),
        "empty_terminal": (terminal["payload"], "result", ""),
        "other_terminal_run": (terminal, "run_id", "other-run"),
        "other_terminal_session": (terminal, "session_id", "old-session"),
        "other_terminal_identity": (terminal, "identity", "identity:alice"),
        "terminal_interaction": (terminal, "interaction_id", "other-interaction"),
        "terminal_assistant": (terminal["payload"], "assistant_message_id", "other-assistant"),
        "mirrored_terminal": (terminal["payload"], "source_event_type", "interaction_complete"),
        "history_terminal": (terminal["source"], "kind", "session_history"),
        "live_history": (history["source"], "kind", "console_event"),
        "other_history_run": (history, "run_id", "other-run"),
        "other_history_session": (history, "session_id", "old-session"),
        "other_history_identity": (history, "identity", "identity:alice"),
        "history_interaction": (history, "interaction_id", "other-interaction"),
        "history_assistant": (history["payload"], "assistant_message_id", None),
        "uncommitted_history": (history, "status", "delivered"),
        "intermediate_history": (history["payload"], "text", "Working on the incident"),
    }
    if mismatch in changes:
        target, key, value = changes[mismatch]
        target[key] = value
    elif mismatch.startswith("missing_"):
        frames.remove({"missing_input": user, "missing_start": start,
                       "missing_terminal": terminal, "missing_history": history}[mismatch])
    elif mismatch == "duplicate_input":
        frames.append({**copy.deepcopy(user), "id": "second-user"})
    elif mismatch == "duplicate_start":
        frames.append({**copy.deepcopy(start), "id": "second-start", "run_id": "second-run"})
    clock = [0.0]

    async def send_and_wait(content, timeout):
        return "incident processed"

    async def read_page(runtime, params, remaining):
        return {"frames": frames, "exhausted": True}

    async def pause(delay):
        clock[0] = 60

    monkeypatch.setitem(globals(), "_read_timeline_page", read_page)
    monkeypatch.setitem(globals(), "_timeline_time", lambda: clock[0])
    monkeypatch.setitem(globals(), "_timeline_pause", pause)
    with pytest.raises(AssertionError, match="deadline|admitted more than once"):
        await _send_sdk_and_verify(
            object(), SimpleNamespace(send_and_wait=send_and_wait),
            "identity:luka", "luka-session", _LEAD_CLOSURE_NOTICE, timeout=60,
        )


@pytest.mark.asyncio
@pytest.mark.parametrize("ticket", [
    None, "", 17, "not-a-uuid", "adc2b7b0-f82a-44f4-a452-c01dd93f75b7",
], ids=["missing", "empty", "wrong-type", "malformed-uuid", "opaque-ticket"])
async def test_kitchen_dispatch_requires_its_ticket_and_one_deadline(monkeypatch, ticket):
    from types import SimpleNamespace
    clock = [0.0]
    calls = []
    wait_budgets = []
    original_wait_for = asyncio.wait_for

    async def timed_wait(awaitable, timeout):
        wait_budgets.append(timeout)
        return await original_wait_for(awaitable, timeout=timeout)

    async def dispatch(dispatch_input, *, track_turn):
        calls.append(("dispatch", dispatch_input, track_turn))
        clock[0] = 5
        return SimpleNamespace(turn_ticket=ticket, turn_unavailable="unsupported bridge")

    async def wait_for_output(*, turn, timeout):
        calls.append(("wait", turn, timeout))
        clock[0] = 15
        return "incident processed"

    async def read_page(runtime, params, remaining):
        assert remaining == 45
        return {"frames": _interaction_fixture(interaction=_INCIDENT_DISPATCH_ID),
                "exhausted": True}

    monkeypatch.setitem(globals(), "_timeline_time", lambda: clock[0])
    monkeypatch.setitem(globals(), "_read_timeline_page", read_page)
    monkeypatch.setattr(asyncio, "wait_for", timed_wait)
    dispatch_input = DispatchInput(
        content="Lab closed", origin="connector", correlation_id=_INCIDENT_DISPATCH_ID,
        idempotency_key="research:lab-closure-1",
    )
    agent = SimpleNamespace(dispatch=dispatch, wait_for_output=wait_for_output)
    operation = _dispatch_sdk_and_wait(agent, dispatch_input, deadline=60)
    if ticket != "adc2b7b0-f82a-44f4-a452-c01dd93f75b7":
        with pytest.raises(AssertionError, match="turn ticket"):
            await operation
        assert calls == [("dispatch", dispatch_input, True)]
        assert wait_budgets == [60]
        return

    assert ticket != dispatch_input.correlation_id
    output = await operation
    result = await _wait_for_timeline(
        object(), "triage:main",
        lambda frames: _completed_interaction(
            frames, "triage:main", "triage-session", _INCIDENT_DISPATCH_ID,
        ),
        deadline=60,
    )
    assert output == result["output"]
    assert calls == [("dispatch", dispatch_input, True), ("wait", ticket, 55)]
    assert wait_budgets == [60, 55, 45]


@pytest.mark.asyncio
@pytest.mark.parametrize("sdk_output", ["incident processed", "unrelated peer reply"])
async def test_kitchen_research_sdk_peer_output_requires_cursor_and_exact_history(monkeypatch, sdk_output):
    from meerkat_meta.runtime import IdentityAgentHandle
    from .test_identity_first_turn_tickets import TicketTransport, _inspection, _make_runtime

    clock = [0.0]
    calls = []
    transport = TicketTransport(inspections=[
        _inspection(None, turns=0),
        # Matching text without cursor progress must not satisfy the SDK wait.
        _inspection("incident processed", turns=0),
        _inspection(sdk_output, turns=1),
    ])
    research = IdentityAgentHandle(_make_runtime(transport), "domain:research")
    baseline = (await research.inspect()).completion_cursor
    original_wait = research.wait_for_output

    async def wait_for_output(*, after, timeout):
        calls.append((after, timeout))
        output = await original_wait(after=after, timeout=timeout, poll_interval=0.001)
        clock[0] = 5
        return output

    async def read_page(runtime, params, remaining):
        assert remaining == 55
        # The baseline read, then the member read once at completion; the
        # wait itself is one server-side wait.
        assert len(transport.params_of("meta/inspect_identity")) == 2
        assert len(transport.params_of("meta/wait_for_completion")) == 1
        frames = _interaction_fixture("domain:research", "research-session", "research-envelope")
        frames.append({"id": "research-notice", **_research_history_fixture()})
        return {"frames": frames, "exhausted": True}

    monkeypatch.setitem(globals(), "_timeline_time", lambda: clock[0])
    monkeypatch.setitem(globals(), "_read_timeline_page", read_page)
    monkeypatch.setattr(research, "wait_for_output", wait_for_output)
    operation = _wait_for_research_sdk_and_verify(
        object(), research, baseline, "research-session", "triage-peer-id", "research-envelope",
        deadline=60,
    )
    if sdk_output == "unrelated peer reply":
        with pytest.raises(AssertionError, match="SDK research waiter returned an unrelated turn"):
            await operation
    else:
        assert (await operation)["run_id"] == "incident-run"
    assert calls == [(baseline, 60)]
    assert transport.params_of("meta/turn_result") == []
    assert transport.params_of("meta/dispatch") == []
    assert transport.params_of("meta/send") == []


@pytest.mark.asyncio
async def test_kitchen_research_sdk_peer_wait_rejects_missing_baseline():
    from types import SimpleNamespace

    async def wait_for_output(**kwargs):
        pytest.fail("missing research baseline must fail before an unscoped SDK wait")

    with pytest.raises(AssertionError, match="research completion cursor"):
        await _wait_for_research_sdk_and_verify(
            object(), SimpleNamespace(wait_for_output=wait_for_output), None,
            "research-session", "triage-peer-id", "research-envelope",
            deadline=_timeline_time() + 60,
        )


@pytest.mark.asyncio
@pytest.mark.parametrize("duplicate_input", [False, True])
async def test_kitchen_sdk_ticket_ignores_identical_foreign_output(monkeypatch, duplicate_input):
    from functools import partial
    from meerkat_meta.runtime import IdentityAgentHandle
    from .test_identity_first_turn_tickets import (
        TicketTransport, _completed, _inspection, _make_runtime, _sent,
    )
    ticket = "ac8832d0-ce0e-43cb-810a-fbd30f65be4f"
    requested_interaction = "e2b7d28d-4108-4425-a6a6-3e77eb2b33c2"
    transport = TicketTransport(
        sends=[_sent(ticket)],
        turn_results={ticket: [{"state": "pending"}, _completed("incident processed")]},
        inspections=[_inspection("incident processed", turns=2)],
    )
    handle = IdentityAgentHandle(_make_runtime(transport), "identity:luka")
    monkeypatch.setattr(handle, "send_and_wait", partial(handle.send_and_wait, poll_interval=0.001))

    async def read_page(runtime, params, remaining):
        assert len(transport.waited_turns()) == 1
        frames = _interaction_fixture("identity:luka", "luka-session", "previous-input", "previous-run")
        frames += _interaction_fixture("identity:luka", "luka-session", requested_interaction, "requested-run")
        for interaction, content in [
            ("previous-input", "Unique SDK input" if duplicate_input else "Earlier input"),
            (requested_interaction, "Unique SDK input"),
        ]:
            frames.append({"id": f"user-{interaction}", "identity": "identity:luka",
                           "session_id": "luka-session", "interaction_id": interaction,
                           "kind": "user_input", "source": {"kind": "session_history"},
                           "payload": {"content": content}})
        return {"frames": frames, "exhausted": True}

    monkeypatch.setitem(globals(), "_read_timeline_page", read_page)
    operation = _send_sdk_and_verify(
        object(), handle, "identity:luka", "luka-session", "Unique SDK input", timeout=60,
    )
    if duplicate_input:
        with pytest.raises(AssertionError, match="admitted more than once"):
            await operation
    else:
        assert (await operation)["run_id"] == "requested-run"
    assert transport.params_of("meta/send")[0]["track_turn"] is True
    assert transport.params_of("meta/inspect_identity") == []
    assert transport.params_of("meta/turn_result") == []
    assert transport.waited_turns() == [{"identity": "identity:luka", "ticket": ticket}]


@pytest.mark.asyncio
async def test_kitchen_sdk_rejects_cursor_fallback_even_with_matching_output(monkeypatch):
    from meerkat_meta.runtime import IdentityAgentHandle
    from .test_identity_first_turn_tickets import TicketTransport, _inspection, _make_runtime, _sent
    transport = TicketTransport(
        sends=[_sent(None)],
        inspections=[_inspection("incident processed", turns=1)],
    )
    handle = IdentityAgentHandle(_make_runtime(transport), "identity:luka")

    async def read_page(*args):
        pytest.fail("an untracked send must fail before any history comparison")

    monkeypatch.setitem(globals(), "_read_timeline_page", read_page)
    with pytest.raises(TurnTrackingUnavailableWarning):
        await _send_sdk_and_verify(
            object(), handle, "identity:luka", "luka-session", "Unique SDK input", timeout=60,
        )
    assert transport.params_of("meta/send")[0]["track_turn"] is True
    assert transport.params_of("meta/inspect_identity") == []
    assert transport.params_of("meta/turn_result") == []


# ===========================================================================
# THE KITCHEN SINK
# ===========================================================================


@_skip_no_key
@_skip_no_binary
class TestTeamIncident:

    @pytest.mark.asyncio
    @pytest.mark.timeout(300)
    async def test_lab_closure_with_turn_driven_coordination(self, tmp_path):
        state_dir = str(tmp_path / "state")
        os.makedirs(state_dir, exist_ok=True)

        roster = TeamRoster(_ROSTER)
        topology = TeamTopology(_EDGES)
        customizer = TeamCustomizer()

        # =============================================================
        # Phase 1: Bootstrap — 7 actors, assert topology wiring
        # =============================================================
        print("\n--- Phase 1: Bootstrap + topology validation ---")
        rt = await _boot(state_dir, roster, topology, customizer)
        try:
            # Agent handles — identity-scoped, no member IDs
            all_names = [
                "identity:luka", "identity:alice", "team-group:main",
                "triage:main", "domain:research", "domain:calendar", "gate:main",
            ]
            agents = {name: rt.agent(name) for name in all_names}
            triage = agents["triage:main"]
            luka = agents["identity:luka"]
            research = agents["domain:research"]
            gate = agents["gate:main"]

            # All 7 Active
            for name, agent in agents.items():
                s = await agent.status()
                assert s.state == "active", f"{name}: {s.state}"

            # ASSERT topology wiring via identity-first inspect
            triage_inspection = await triage.inspect()
            assert triage_inspection.peer_reachable_count >= 5, (
                f"triage should be wired to at least 5 peers (research, calendar, gate, "
                f"luka, alice), got {triage_inspection.peer_reachable_count}"
            )
            print(f"[Phase 1] triage peers: {triage_inspection.peer_reachable_count} reachable")
            research_session = (await research.status()).session_id
            lead_session = (await luka.status()).session_id
            triage_session = (await triage.status()).session_id
            gate_session = (await gate.status()).session_id
            triage_peer = await rt.mob_handle().peer_info("triage:main")
            assert triage_peer.get("peer_id"), "triage must expose its canonical comms peer ID"
            research_peer = await rt.mob_handle().peer_info("domain:research")
            assert research_peer.get("peer_id"), "research must expose its canonical comms peer ID"

            # Addressability enforcement
            with pytest.raises(RpcError, match="not addressable"):
                await rt.send("triage:main", "should fail")
            with pytest.raises(RpcError, match="not addressable"):
                await rt.send("domain:research", "should fail")
            with pytest.raises(RpcError, match="not addressable"):
                await rt.send("gate:main", "should fail")
            print("[Phase 1] InternalOnly enforcement OK for triage, research, gate")

            # Turn-driven members are ready before they complete any work.
            startup = await rt.wait_identity_bootstrap(
                target="startup_ready", timeout=30
            )
            assert startup.startup_ready is True, startup.to_dict()

            # =============================================================
            # Phase 2: Lab closure → triage → ASSERT domain fan-out
            # =============================================================
            print("\n--- Phase 2: Lab closure + peer fan-out ---")

            triage_deadline = _timeline_time() + 90
            research_baseline = (await asyncio.wait_for(
                research.inspect(), timeout=max(0, triage_deadline - _timeline_time()),
            )).completion_cursor
            assert research_baseline is not None, "research completion cursor is required before peer delivery"
            triage_sdk_output = await _dispatch_sdk_and_wait(triage, DispatchInput(
                content=(
                    "URGENT from facilities connector: Hillside Lab closed tomorrow "
                    "due to pipe burst. All staff must work remotely. This affects the "
                    "team's morning schedule. Forward this to the research domain agent."
                ),
                origin="connector",
                correlation_id=_INCIDENT_DISPATCH_ID,
                idempotency_key="research:lab-closure-1",
            ), deadline=triage_deadline)
            # Independently require this dispatch's committed run and accepted
            # research send. Another completion returned by the SDK must fail.
            fanout = await _wait_for_timeline(
                rt, "triage:main",
                lambda frames: _research_fanout(
                    frames, triage_session, _INCIDENT_DISPATCH_ID, research_peer["peer_id"],
                ),
                deadline=triage_deadline,
            )
            assert triage_sdk_output == fanout["output"], "SDK triage waiter returned an unrelated turn"
            print(f"[Phase 2] triage output: {fanout['output']}")

            # Match the accepted envelope's interaction, then insist the exact
            # incident and assistant output are committed within the same budget.
            research_deadline = _timeline_time() + 60
            research_result = await _wait_for_research_sdk_and_verify(
                rt, research, research_baseline, research_session,
                triage_peer["peer_id"], fanout["interaction_id"],
                deadline=research_deadline,
            )
            received = research_result["incident_history"]
            print(f"[Phase 2] research accepted triage incident in history frame {received['id']}")
            print(f"[Phase 2] domain:research received comms: {research_result['output']}")

            # Deliver closure notice to luka (simulates end of triage→domain→gate→identity chain).
            # Exercise the downstream app's actual SDK completion path and verify its run.
            lead_notice = await _send_sdk_and_verify(
                rt, luka, "identity:luka", lead_session, _LEAD_CLOSURE_NOTICE,
                timeout=60,
            )
            closure_history = lead_notice["input_history"]
            print("[Phase 2] identity:luka notified about lab closure")

            # =============================================================
            # Phase 3: Calendar event + gate evaluation
            # =============================================================
            print("\n--- Phase 3: Calendar conflict + gate ---")

            # Dispatch to gate for policy evaluation
            gate_deadline = _timeline_time() + 60
            gate_sdk_output = await _dispatch_sdk_and_wait(gate, DispatchInput(
                content=(
                    "Proposed action: notify the team group that the lab is closed tomorrow "
                    "and Luka's 09:00 vendor review conflicts with the on-site session. "
                    "Evaluate whether this notification is appropriate to send."
                ),
                origin="system",
                correlation_id=_GATE_DISPATCH_ID,
                idempotency_key="research:gate-evaluation-1",
            ), deadline=gate_deadline)
            gate_result = await _wait_for_timeline(
                rt, "gate:main",
                lambda frames: _completed_interaction(
                    frames, "gate:main", gate_session, _GATE_DISPATCH_ID,
                ),
                deadline=gate_deadline,
            )
            assert gate_sdk_output == gate_result["output"], "SDK gate waiter returned an unrelated turn"
            print(f"[Phase 3] gate evaluated: {gate_result['output']}")

            # =============================================================
            # Phase 4: Shutdown MID-FLIGHT (not after idle)
            # =============================================================
            print("\n--- Phase 4: Mid-flight shutdown ---")

            # Dispatch calendar event and shutdown IMMEDIATELY
            # without waiting for processing. This tests checkpoint/restore
            # during active work, not after completion.
            await triage.dispatch(DispatchInput(
                content=(
                    "Calendar connector: Luka has a vendor review at 09:00 tomorrow. "
                    "The lab is closed. Forward to calendar domain agent for conflict analysis."
                ),
                origin="connector",
                correlation_id="calendar-review-1",
                idempotency_key="calendar:calendar-review-1",
            ))
            # Record state BEFORE waiting for calendar processing
            pre_shutdown = {}
            for name in all_names:
                pre_shutdown[name] = await agents[name].status()

            # Shutdown while triage/calendar may still be processing
            await rt.shutdown()
            print("[Phase 4] Runtime shut down with calendar event potentially in-flight")

        except Exception:
            await rt.shutdown()
            raise

        # =============================================================
        # Phase 5: Restore — assert continuity content, not just IDs
        # =============================================================
        print("\n--- Phase 5: Restore + continuity verification ---")
        rt2 = await _boot(state_dir, roster, topology, customizer)
        try:
            # Re-create agent handles on the new runtime
            agents2 = {name: rt2.agent(name) for name in all_names}
            lead2 = agents2["identity:luka"]

            # Stable IDs across restart
            for name, before in pre_shutdown.items():
                after = await agents2[name].status()
                assert after.session_id == before.session_id, (
                    f"{name} session changed: {before.session_id} -> {after.session_id}"
                )
                assert after.agent_runtime_id == before.agent_runtime_id, (
                    f"{name} runtime_id changed"
                )
            print("[Phase 5] All 7 actors restored with stable IDs")

            # Wait on the same typed lifecycle readiness barrier after restore.
            # Output previews may stay empty until fresh work is delivered.
            startup = await rt2.wait_identity_bootstrap(
                target="startup_ready", timeout=30
            )
            assert startup.startup_ready is True, startup.to_dict()

            restored_history = await _wait_for_history_frame(
                rt2, "identity:luka",
                lambda frame: _closure_notice_frame_matches(frame, "identity:luka", lead_session),
            )
            assert restored_history["source"]["source_cursor"] == closure_history["source"]["source_cursor"], (
                "restart must preserve the exact durable lab notice in Luka's session"
            )

            # ASSERT conversational continuity via LLM content.
            # Luka received the lab closure notice before shutdown.
            # After restore, asking about the lab should reference the closure.
            # Correlate this question's accepted interaction and committed answer;
            # a resumed peer reply must not satisfy the continuity assertion.
            lead_result = await _send_sdk_and_verify(
                rt2, lead2, "identity:luka", lead_session,
                "Is the lab open or closed tomorrow, and why? Answer in one sentence only.",
                timeout=90,
            )
            lead_output = lead_result["output"]
            assert _remembers_lab_closure(lead_output), (
                "Luka must recall both the closure and its pipe-burst cause from persisted "
                f"history, not merely respond after restart: {lead_output}"
            )
            print(f"[Phase 5] luka remembers lab closure: {lead_output}")

            # =============================================================
            # Phase 6: Respawn domain:calendar
            # =============================================================
            print("\n--- Phase 6: Respawn domain:calendar ---")

            calendar2 = rt2.agent("domain:calendar")
            research2 = rt2.agent("domain:research")

            cal_before = await calendar2.status()
            await calendar2.respawn()
            cal_after = await calendar2.status()

            assert cal_after.agent_runtime_id == cal_before.agent_runtime_id
            assert cal_after.session_id == cal_before.session_id
            assert cal_after.generation == cal_before.generation
            print("[Phase 6] domain:calendar respawned — identity preserved")

            research_after = await research2.status()
            assert research_after.session_id == pre_shutdown["domain:research"].session_id
            print("[Phase 6] domain:research unaffected")

            # =============================================================
            # Phase 7: Reconcile - add bob
            # =============================================================
            print("\n--- Phase 7: Reconcile ---")

            bob = DurableAgentSpec(
                identity="identity:bob",
                profile="personal",
                addressability="addressable",
            )
            roster.update([*_ROSTER, bob])
            topology.update([*_EDGES, ("identity:bob", "triage:main")])

            await rt2.reconcile()

            bob2 = rt2.agent("identity:bob")
            bob_status = await bob2.status()
            assert bob_status.state == "active"
            assert bob_status.generation == 0

            lead_post = await lead2.status()
            assert lead_post.session_id == pre_shutdown["identity:luka"].session_id
            triage_post = await rt2.agent("triage:main").status()
            assert triage_post.session_id == pre_shutdown["triage:main"].session_id
            print("[Phase 7] bob added, existing actors preserved")

            # =============================================================
            # Phase 8: Final coherence
            # =============================================================
            print("\n--- Phase 8: Final coherence ---")

            all_identities = [
                "identity:luka", "identity:alice", "identity:bob",
                "team-group:main", "triage:main", "domain:research",
                "domain:calendar", "gate:main",
            ]
            for name in all_identities:
                s = await rt2.status(name)
                assert s.state == "active", f"{name}: {s.state}"

            print(f"[Phase 8] All {len(all_identities)} actors Active")
            print("\n=== TEAM KITCHEN SINK PASSED ===")

        finally:
            await rt2.shutdown()
