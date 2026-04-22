"""Live smoke tests for the Python SDK against a real rkat-rpc subprocess."""

from __future__ import annotations

import asyncio
import os
from contextlib import contextmanager
from pathlib import Path
from uuid import uuid4

import pytest

from meerkat import RealtimeChannel
from meerkat.errors import MeerkatError

from .live_smoke_support import (
    has_anthropic_api_key,
    has_openai_api_key,
    live_client,
    make_prompt_input,
    openai_model,
    raw_request,
    resolve_rkat_rpc_path,
    smoke_model,
    wait_for,
)

pytestmark = pytest.mark.skipif(
    resolve_rkat_rpc_path() is None,
    reason="rkat-rpc binary not found",
)

requires_live_llm = pytest.mark.skipif(
    not has_anthropic_api_key(),
    reason="no ANTHROPIC_API_KEY",
)

requires_mixed_llms = pytest.mark.skipif(
    not (has_anthropic_api_key() and has_openai_api_key()),
    reason="need both ANTHROPIC_API_KEY and OPENAI_API_KEY",
)


def persistent_realm_kwargs(tmp_path: Path) -> dict[str, str]:
    state_root = tmp_path / "state"
    state_root.mkdir(parents=True, exist_ok=True)
    return {
        "realm_id": f"python-live-smoke-{uuid4()}",
        "realm_backend": "sqlite",
        "state_root": str(state_root),
    }


def include_scenario(scenario_id: int) -> bool:
    del scenario_id
    return True


async def collect_stream_text(stream) -> tuple[str, object]:
    """Consume a streaming turn and return accumulated text plus the final result."""
    chunks: list[str] = []
    async with stream as events:
        async for event in events:
            delta = getattr(event, "delta", None)
            if isinstance(delta, str):
                chunks.append(delta)
    return "".join(chunks), events.result


async def next_subscription_event(subscription, *, timeout_secs: float = 60.0):
    iterator = subscription.__aiter__()
    return await asyncio.wait_for(iterator.__anext__(), timeout=timeout_secs)


async def read_realtime_until(connection, predicate, *, timeout_secs: float = 60.0):
    frames: list[dict[str, object]] = []
    while True:
        frame = await asyncio.wait_for(connection.recv(), timeout=timeout_secs)
        assert frame is not None, "realtime websocket closed before expected frame arrived"
        if frame.get("type") == "channel.error":
            raise AssertionError(f"realtime channel error: {frame}")
        frames.append(frame)
        if predicate(frame, frames):
            return frames


def realtime_frame_event_type(frame: dict[str, object]) -> str | None:
    if frame.get("type") != "channel.event":
        return None
    event = frame.get("event")
    if not isinstance(event, dict):
        return None
    event_type = event.get("type")
    return event_type if isinstance(event_type, str) else None


def realtime_frames_show_turn_activity(frames: list[dict[str, object]]) -> bool:
    return any(
        realtime_frame_event_type(frame)
        in {
            "turn_started",
            "turn_committed",
            "turn_completed",
            "interrupted",
            "input_transcript_partial",
            "input_transcript_final",
            "output_text_delta",
            "output_audio_chunk",
            "tool_call_requested",
            "tool_call_completed",
            "tool_call_failed",
        }
        for frame in frames
    )


def realtime_frames_show_turn_completed(frames: list[dict[str, object]]) -> bool:
    return any(realtime_frame_event_type(frame) == "turn_completed" for frame in frames)


async def read_realtime_until_ready_or_idle(
    connection, *, timeout_secs: float = 5.0, idle_timeout_secs: float = 0.5
):
    frames: list[dict[str, object]] = []
    saw_any_frame = False
    deadline = asyncio.get_running_loop().time() + timeout_secs

    while asyncio.get_running_loop().time() < deadline:
        remaining = min(idle_timeout_secs, deadline - asyncio.get_running_loop().time())
        try:
            frame = await asyncio.wait_for(connection.recv(), timeout=remaining)
        except asyncio.TimeoutError:
            if saw_any_frame:
                return frames
            continue

        assert frame is not None, "realtime websocket closed before expected frame arrived"
        if frame.get("type") == "channel.error":
            raise AssertionError(f"realtime channel error: {frame}")
        frames.append(frame)
        saw_any_frame = True

        if frame.get("type") == "channel.status":
            status = frame.get("status")
            if isinstance(status, dict) and status.get("state") == "ready":
                return frames
        event = frame.get("event")
        if (
            frame.get("type") == "channel.event"
            and isinstance(event, dict)
            and event.get("type") == "status_changed"
        ):
            status = event.get("status")
            if isinstance(status, dict) and status.get("state") == "ready":
                return frames

    return frames


async def ensure_realtime_channel_quiescent_after_out_of_band_turn(
    connection, *, timeout_secs: float = 5.0
):
    frames = await read_realtime_until_ready_or_idle(
        connection, timeout_secs=timeout_secs
    )
    if realtime_frames_show_turn_completed(frames) or not realtime_frames_show_turn_activity(
        frames
    ):
        return frames

    await connection.interrupt()
    drained = await read_realtime_until_ready_or_idle(
        connection, timeout_secs=timeout_secs
    )
    return [*frames, *drained]


@contextmanager
def without_openai_realtime_env():
    keys = ("RKAT_OPENAI_API_KEY", "OPENAI_API_KEY")
    saved = {key: os.environ.pop(key, None) for key in keys}
    try:
        yield
    finally:
        for key, value in saved.items():
            if value is not None:
                os.environ[key] = value


# ---------------------------------------------------------------------------
# Scenario 37: Full lifecycle + capabilities
# ---------------------------------------------------------------------------


if include_scenario(37):
    @pytest.mark.asyncio
    @requires_live_llm
    async def test_smoke_scenario_37_full_lifecycle_and_capabilities():
        async with live_client() as client:
            caps = await raw_request(client, "capabilities/get", {})
            assert caps.get("contract_version")
            assert client.has_capability("sessions")

            session = await client.create_session(
                "My name is PyBot37 and my favorite color is teal. Reply briefly.",
                model=smoke_model(),
                provider="anthropic",
            )

            assert session.id
            assert session.text

            turn = await session.turn(
                "What is my name and favorite color? Reply in one sentence."
            )
            text_lower = turn.text.lower()
            assert "pybot37" in text_lower
            assert "teal" in text_lower

            details = await client.read_session(session.id)
            assert details.session_id == session.id
            assert details.is_active is False
            assert details.message_count >= 4

            history = await client.read_session_history(session.id)
            assert history.session_id == session.id
            assert history.message_count >= 4
            assert len(history.messages) >= 4

            sessions = await client.list_sessions()
            assert session.id in {entry.session_id for entry in sessions}

            await session.archive()

            archived_history = await session.history()
            assert archived_history.session_id == session.id
            assert archived_history.message_count >= 4
            assert len(archived_history.messages) >= 4

            sessions_after_archive = await client.list_sessions()
            assert session.id not in {entry.session_id for entry in sessions_after_archive}


# ---------------------------------------------------------------------------
# Scenario 38: Context injection + streaming
# ---------------------------------------------------------------------------


if include_scenario(38):
    @pytest.mark.asyncio
    @requires_live_llm
    async def test_smoke_scenario_38_inject_context_and_streaming():
        async with live_client() as client:
            session = await client.create_session(
                "Remember the codeword ORBIT-38 for later and reply READY.",
                model=smoke_model(),
                provider="anthropic",
            )

            injected = await session.inject_context(
                "Always include the marker [PY-CTX-38] in your replies.",
                source="python-sdk-live-smoke",
            )
            assert injected["status"] in {"applied", "staged", "duplicate"}

            streamed_text, result = await collect_stream_text(
                session.stream(
                    "What is the codeword? Include any required markers in your reply."
                )
            )
            assert result is not None
            final_text = (result.text or streamed_text).lower()
            assert "orbit-38" in final_text or "orbit 38" in final_text
            assert "py-ctx-38" in final_text

            async with await session.subscribe_events() as subscription:
                await session.turn("Repeat the codeword in two words.")
                observed = await next_subscription_event(subscription)
                assert observed is not None


# ---------------------------------------------------------------------------
# Scenario 39: Persistent reconnect + session submit
# ---------------------------------------------------------------------------


if include_scenario(39):
    @pytest.mark.asyncio
    @requires_live_llm
    async def test_smoke_scenario_39_persistent_reconnect_and_session_submit(tmp_path: Path):
        realm = persistent_realm_kwargs(tmp_path)
        marker = f"python-runtime-{uuid4().hex[:8]}"

        async with live_client(**realm) as client_a:
            session = await client_a.create_session(
                f"Remember this persistent marker exactly: {marker}. Reply briefly.",
                model=smoke_model(),
                provider="anthropic",
            )
            session_id = session.id

        async with live_client(**realm) as client_b:
            read_back = await client_b.read_session(session_id)
            assert read_back.session_id == session_id
            assert read_back.message_count >= 2

            runtime_state = await client_b.status(session_id)
            assert runtime_state.state in {
                "attached",
                "idle",
                "running",
                "initializing",
            }

            accepted = await client_b.submit(
                session_id,
                make_prompt_input(
                    f"Reply with PY-RUNTIME-39 and the marker {marker}.",
                    turn_metadata={
                        "additional_instructions": [
                            "Always include the marker [PY-RUNTIME-OK].",
                        ],
                    },
                ),
            )
            assert accepted.outcome_type == "accepted"
            assert accepted.input_id is not None
            input_id = accepted.input_id

            input_state = await wait_for(
                "runtime input to be consumed",
                lambda: client_b.submission(session_id, input_id),
                lambda state: state is not None and state.current_state == "consumed",
                timeout_secs=120.0,
            )
            assert input_state is not None
            assert input_state.current_state == "consumed"

            after_runtime = await wait_for(
                "runtime-authored assistant reply",
                lambda: client_b.read_session(session_id),
                lambda state: "py-runtime-39" in (state.last_assistant_text or "").lower()
                and "py-runtime-ok" in (state.last_assistant_text or "").lower(),
                timeout_secs=120.0,
            )
            last_text = (after_runtime.last_assistant_text or "").lower()
            assert "py-runtime-39" in last_text
            assert "py-runtime-ok" in last_text
            assert marker in last_text

        async with live_client(**realm) as client_c:
            sessions = await client_c.list_sessions()
            assert session_id in {entry.session_id for entry in sessions}

            resumed = await client_c._start_turn(  # noqa: SLF001
                session_id,
                "What persistent marker was I asked to remember? Reply with just the marker.",
            )
            assert marker in resumed.text


# ---------------------------------------------------------------------------
# Scenario 40: Mixed-provider swarm probe
# ---------------------------------------------------------------------------


if include_scenario(40):
    @pytest.mark.asyncio
    @requires_mixed_llms
    async def test_smoke_scenario_40_mixed_provider_swarm_probe():
        async with live_client() as client:
            if not client.has_capability("mob"):
                pytest.skip("mob capability not available")

            mob = await client.create_mob(
                definition={
                    "id": f"python-sdk-swarm-{uuid4().hex[:8]}",
                    "orchestrator": {"profile": "lead"},
                    "profiles": {
                        "lead": {
                            "model": smoke_model(),
                            "tools": {"comms": True},
                            "peer_description": "Lead coordinator",
                            "external_addressable": True,
                        },
                        "reviewer": {
                            "model": openai_model(),
                            "tools": {"comms": True},
                            "peer_description": "Review worker",
                            "external_addressable": True,
                        },
                        "broken": {
                            "model": "definitely-invalid-live-smoke-model",
                            "tools": {"comms": True},
                            "peer_description": "Deterministic failure worker",
                            "external_addressable": True,
                        },
                    },
                    "wiring": {
                        "auto_wire_orchestrator": False,
                        "role_wiring": [{"a": "lead", "b": "reviewer"}],
                    },
                }
            )

            lead = await mob.spawn(
                profile="lead",
                agent_identity="lead-1",
                initial_message="Acknowledge the lead role in one sentence.",
                runtime_mode="autonomous_host",
            )
            reviewer = await mob.spawn(
                profile="reviewer",
                agent_identity="reviewer-1",
                initial_message="Acknowledge the reviewer role in one sentence.",
                runtime_mode="turn_driven",
            )
            assert lead["agent_identity"] == "lead-1"
            assert reviewer["agent_identity"] == "reviewer-1"
            assert isinstance(lead.get("member_ref"), str) and lead["member_ref"]
            assert isinstance(reviewer.get("member_ref"), str) and reviewer["member_ref"]
            reviewer_member_ref = reviewer["member_ref"]
            await mob.wire("lead-1", "reviewer-1")

            append = await mob.append_system_context(
                "reviewer-1",
                "Remember the swarm marker [PY-SWARM-40].",
                source="python-sdk",
                idempotency_key="py-swarm-40",
            )
            assert append["status"] in {"staged", "duplicate"}
            assert append["agent_identity"] == "reviewer-1"

            async with await mob.subscribe_member_events("reviewer-1") as subscription:
                reviewer_receipt = await mob.member("reviewer-1").send(
                    "Reply with REVIEWER_READY_40 and include [PY-SWARM-40].",
                )
                assert reviewer_receipt["agent_identity"] == "reviewer-1"
                assert reviewer_receipt["member_ref"] == reviewer_member_ref
                observed = await next_subscription_event(subscription)
                assert observed is not None

            reviewer_state = await wait_for(
                "reviewer turn to finish",
                lambda: mob.member_status("reviewer-1"),
                lambda state: "reviewer_ready_40"
                in (state.get("output_preview") or "").lower(),
                timeout_secs=120.0,
            )
            reviewer_text = (reviewer_state.get("output_preview") or "").lower()
            assert "reviewer_ready_40" in reviewer_text
            assert "py-swarm-40" in reviewer_text

            members = await mob.members()
            assert {"lead-1", "reviewer-1"}.issubset(
                {entry["agent_identity"] for entry in members}
            )

            try:
                broken = await mob.spawn(
                    profile="broken",
                    agent_identity="broken-1",
                    runtime_mode="turn_driven",
                )
            except MeerkatError as err:
                assert "definitely-invalid-live-smoke-model" in str(err)
            else:
                assert broken["agent_identity"] == "broken-1"
                with pytest.raises(MeerkatError):
                    await mob.member("broken-1").send(
                        "This turn must fail because the member model is invalid.",
                    )
                broken_state = await mob.member_status("broken-1")
                assert not (broken_state.get("output_preview") or "").strip()

            respawn_result = await mob.respawn(
                "reviewer-1",
                "Come back online and say REVIEWER_RESPAWN_40.",
            )
            respawned_member_ref = respawn_result["receipt"]["member_ref"]
            respawned_snapshot = await wait_for(
                "reviewer runtime id changes after respawn",
                lambda: mob.member_status("reviewer-1"),
                lambda state: state.get("agent_runtime_id")
                and state.get("agent_runtime_id") != reviewer_state.get("agent_runtime_id"),
                timeout_secs=60.0,
            )
            respawn_receipt = await mob.member("reviewer-1").send(
                "Reply with REVIEWER_RESPAWN_40.",
            )
            assert respawn_receipt["agent_identity"] == "reviewer-1"
            assert respawn_receipt["member_ref"] == respawned_member_ref
            respawned_state = await wait_for(
                "reviewer reply after respawn",
                lambda: mob.member_status("reviewer-1"),
                lambda state: "reviewer_respawn_40"
                in (state.get("output_preview") or "").lower(),
                timeout_secs=120.0,
            )
            assert (
                respawned_state.get("agent_runtime_id")
                == respawned_snapshot.get("agent_runtime_id")
            )
            assert "reviewer_respawn_40" in (
                respawned_state.get("output_preview") or ""
            ).lower()

            await mob.retire("reviewer-1")
            members_after_retire = await wait_for(
                "reviewer retirement",
                mob.members,
                lambda entries: all(entry["agent_identity"] != "reviewer-1" for entry in entries),
                timeout_secs=60.0,
            )
            assert all(
                entry["agent_identity"] != "reviewer-1" for entry in members_after_retire
            )

            with pytest.raises(MeerkatError) as exc_info:
                await client.read_session("00000000-0000-0000-0000-000000000000")
            assert exc_info.value.code in {"SESSION_NOT_FOUND", "INVALID_PARAMS", "-32001"}


# ---------------------------------------------------------------------------
# Scenario 57: Realtime channel session exchange
# ---------------------------------------------------------------------------


if include_scenario(57):
    @pytest.mark.asyncio
    @requires_live_llm
    async def test_smoke_scenario_57_realtime_channel_session_exchange():
        with without_openai_realtime_env():
            async with live_client() as client:
                session = await client.create_session(
                    "When asked through realtime, reply with PY-REALTIME-57 and mention cedar.",
                    model=smoke_model(),
                    provider="anthropic",
                )

                channel = RealtimeChannel.session(client, session.id)
                open_info = await channel.open_info()
                assert open_info.ws_url.startswith("ws://")
                assert open_info.default_protocol_version

                connection = await channel.connect()
                await connection.send_input(
                    {
                        "kind": "text_chunk",
                        "text": "Reply with PY-REALTIME-57 and the word cedar.",
                    }
                )

                frames = await read_realtime_until(
                    connection,
                    lambda frame, _frames: frame.get("type") == "channel.event"
                    and isinstance(frame.get("event"), dict)
                    and frame["event"].get("type") == "turn_committed",
                )
                event_types = [
                    frame["event"]["type"]
                    for frame in frames
                    if frame.get("type") == "channel.event"
                    and isinstance(frame.get("event"), dict)
                ]
                assert event_types[:4] == [
                    "turn_started",
                    "input_transcript_partial",
                    "input_transcript_final",
                    "turn_committed",
                ]

                session_state = await wait_for(
                    "realtime session reply",
                    lambda: client.read_session(session.id),
                    lambda state: "py-realtime-57"
                    in (state.last_assistant_text or "").lower(),
                    timeout_secs=120.0,
                )
                last_text = (session_state.last_assistant_text or "").lower()
                assert "py-realtime-57" in last_text
                assert "cedar" in last_text

                history = await client.read_session_history(session.id)
                assert any(
                    message.role == "user"
                    and isinstance(message.content, str)
                    and "Reply with PY-REALTIME-57 and the word cedar."
                    in message.content
                    for message in history.messages
                )

                await connection.close()
                closed_frames = await read_realtime_until(
                    connection,
                    lambda frame, _frames: frame.get("type") == "channel.closed",
                )
                assert closed_frames[-1]["type"] == "channel.closed"


# ---------------------------------------------------------------------------
# Scenario 58: Realtime channel member respawn continuity
# ---------------------------------------------------------------------------


if include_scenario(58):
    @pytest.mark.asyncio
    @requires_live_llm
    async def test_smoke_scenario_58_realtime_member_channel_respawn_continuity():
        async with live_client() as client:
            if not client.has_capability("mob"):
                pytest.skip("mob capability not available")

            mob = await client.create_mob(
                definition={
                    "id": f"python-realtime-mob-{uuid4().hex[:8]}",
                    "orchestrator": {"profile": "lead"},
                    "profiles": {
                        "lead": {
                            "model": "gpt-realtime-1.5",
                            "tools": {"comms": True},
                            "peer_description": "Lead realtime worker",
                            "external_addressable": True,
                        }
                    },
                }
            )
            spawned = await mob.spawn(
                profile="lead",
                agent_identity="lead-rt",
                initial_message="Reply READY_RT_58.",
                runtime_mode="turn_driven",
            )
            assert spawned["agent_identity"] == "lead-rt"

            channel = RealtimeChannel.mob_member(client, mob.id, "lead-rt")
            connection = await channel.connect()
            status = await channel.status()
            assert status.status["state"] in {"opening", "ready", "reconnecting"}

            await connection.send_input(
                {
                    "kind": "text_chunk",
                    "text": "Reply with PY-MEMBER-58 and spruce.",
                }
            )
            await read_realtime_until(
                connection,
                lambda frame, _frames: frame.get("type") == "channel.event"
                and isinstance(frame.get("event"), dict)
                and frame["event"].get("type") == "turn_committed",
            )

            initial_state = await wait_for(
                "member realtime reply",
                lambda: mob.member_status("lead-rt"),
                lambda state: "py-member-58"
                in (state.get("output_preview") or "").lower(),
                timeout_secs=120.0,
            )
            assert "spruce" in (initial_state.get("output_preview") or "").lower()

            await mob.force_cancel("lead-rt")
            post_cancel_status = await channel.status()
            assert post_cancel_status.status["state"] in {
                "opening",
                "ready",
                "reconnecting",
            }

            pre_respawn_state = await mob.member_status("lead-rt")
            pre_respawn_session_id = pre_respawn_state.get("current_session_id")

            respawn = await mob.respawn(
                "lead-rt",
                "Come back online and say PY-RESPAWN-58.",
            )
            receipt = respawn["receipt"]
            assert receipt["agent_identity"] == "lead-rt"
            # Dogma #10 retired `agent_runtime_id` from app-facing receipts;
            # callers wait on the canonical `member_realtime_bindings` map
            # instead — the rotated `current_session_id` is the wire-level
            # signal that the respawn completed and the MobMachine emitted
            # MemberRealtimeBindingRotated.
            await wait_for(
                "respawned member status",
                lambda: mob.member_status("lead-rt"),
                lambda state: isinstance(state.get("current_session_id"), str)
                and state["current_session_id"] != pre_respawn_session_id,
                timeout_secs=60.0,
            )

            await connection.send_input(
                {
                    "kind": "text_chunk",
                    "text": "Reply with PY-MEMBER-RESPAWN-58 and maple.",
                }
            )
            await read_realtime_until(
                connection,
                lambda frame, _frames: frame.get("type") == "channel.event"
                and isinstance(frame.get("event"), dict)
                and frame["event"].get("type") == "turn_committed",
            )

            respawned_state = await wait_for(
                "respawned member realtime reply",
                lambda: mob.member_status("lead-rt"),
                lambda state: "py-member-respawn-58"
                in (state.get("output_preview") or "").lower(),
                timeout_secs=120.0,
            )
            respawned_preview = (
                respawned_state.get("output_preview") or ""
            ).lower()
            assert "py-member-respawn-58" in respawned_preview
            assert "maple" in respawned_preview

            await connection.close()
            closed_frames = await read_realtime_until(
                connection,
                lambda frame, _frames: frame.get("type") == "channel.closed",
            )
            assert closed_frames[-1]["type"] == "channel.closed"


# ---------------------------------------------------------------------------
# Scenario 64: Realtime channel member model-switch continuity
# ---------------------------------------------------------------------------


if include_scenario(64):
    @pytest.mark.asyncio
    @requires_mixed_llms
    async def test_smoke_scenario_64_realtime_member_channel_model_switch_continuity():
        async with live_client() as client:
            if not client.has_capability("mob"):
                pytest.skip("mob capability not available")

            mob = await client.create_mob(
                definition={
                    "id": f"python-realtime-switch-mob-{uuid4().hex[:8]}",
                    "orchestrator": {"profile": "lead"},
                    "profiles": {
                        "lead": {
                            "model": "gpt-realtime-1.5",
                            "tools": {"comms": True},
                            "peer_description": "Lead realtime worker",
                            "external_addressable": True,
                        }
                    },
                }
            )
            spawned = await mob.spawn(
                profile="lead",
                agent_identity="lead-rt-switch",
                initial_message="Reply READY_RT_64.",
                runtime_mode="turn_driven",
            )
            assert spawned["agent_identity"] == "lead-rt-switch"

            channel = RealtimeChannel.mob_member(client, mob.id, "lead-rt-switch")
            connection = await channel.connect()
            initial_status = await channel.status()
            assert initial_status.status["state"] in {"opening", "ready", "reconnecting"}

            await connection.send_input(
                {
                    "kind": "text_chunk",
                    "text": "Reply with PY-MEMBER-64-INITIAL and cedar.",
                }
            )
            await read_realtime_until(
                connection,
                lambda frame, _frames: frame.get("type") == "channel.event"
                and isinstance(frame.get("event"), dict)
                and frame["event"].get("type") == "turn_committed",
            )

            initial_state = await wait_for(
                "initial member realtime reply",
                lambda: mob.member_status("lead-rt-switch"),
                lambda state: "py-member-64-initial"
                in (state.get("output_preview") or "").lower(),
                timeout_secs=120.0,
            )
            assert "cedar" in (initial_state.get("output_preview") or "").lower()

            switched = await client.mob_turn_start(
                mob.id,
                "lead-rt-switch",
                "Reply with PY-MEMBER-SWITCH-64 and birch.",
                model=openai_model(),
                provider="openai",
            )
            switched_text = str(switched.get("text") or "").lower()
            assert "py-member-switch-64" in switched_text
            assert "birch" in switched_text

            switched_state = await wait_for(
                "member model-switch reply",
                lambda: mob.member_status("lead-rt-switch"),
                lambda state: "py-member-switch-64"
                in (state.get("output_preview") or "").lower(),
                timeout_secs=120.0,
            )
            assert "birch" in (switched_state.get("output_preview") or "").lower()
            # `mob_turn_start(..., model=...)` is still a real semantic turn.
            # When the member already has an attached realtime channel, the
            # switch-turn reply can continue flowing on that channel after the
            # RPC result is back. Quiesce it explicitly before the next
            # provider-managed input so this smoke proves continuity through
            # the switch rather than accidental barge-in against leftover
            # switch-turn output.
            await ensure_realtime_channel_quiescent_after_out_of_band_turn(connection)

            # Member-target channels preserve continuity through the mob/runtime
            # substrate even when the projected attachment state has not yet
            # converged to session-target "ready" semantics.
            settled_status = await channel.status()
            assert settled_status.status["state"] in {
                "opening",
                "ready",
                "reconnecting",
            }
            assert settled_status.status["attempt_count"] >= 0

            await connection.send_input(
                {
                    "kind": "text_chunk",
                    "text": "Reply with PY-MEMBER-POST-SWITCH-64 and maple.",
                }
            )
            await read_realtime_until(
                connection,
                lambda frame, _frames: frame.get("type") == "channel.event"
                and isinstance(frame.get("event"), dict)
                and frame["event"].get("type") == "turn_committed",
            )

            final_state = await wait_for(
                "post-switch member realtime reply",
                lambda: mob.member_status("lead-rt-switch"),
                lambda state: "py-member-post-switch-64"
                in (state.get("output_preview") or "").lower(),
                timeout_secs=120.0,
            )
            final_preview = (final_state.get("output_preview") or "").lower()
            assert "py-member-post-switch-64" in final_preview
            assert "maple" in final_preview
            assert final_state.get("realtime_attachment_status") in {
                "binding_not_ready",
                "binding_ready",
                "replacement_pending",
                "reattach_required",
            }

            await connection.close()
            closed_frames = await read_realtime_until(
                connection,
                lambda frame, _frames: frame.get("type") == "channel.closed",
            )
            assert closed_frames[-1]["type"] == "channel.closed"
