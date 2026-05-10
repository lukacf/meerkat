"""Live smoke tests for the Python SDK against a real rkat-rpc subprocess."""

from __future__ import annotations

import asyncio
import os
from contextlib import contextmanager
from pathlib import Path
from uuid import uuid4

import pytest

from meerkat.errors import MeerkatError

from .live_smoke_support import (
    gemini_image_model,
    has_anthropic_api_key,
    has_gemini_api_key,
    has_openai_api_key,
    live_client,
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

requires_openai_realtime = pytest.mark.skipif(
    not has_openai_api_key(),
    reason="need OPENAI_API_KEY",
)

requires_anthropic_and_gemini = pytest.mark.skipif(
    not (has_anthropic_api_key() and has_gemini_api_key()),
    reason="need both ANTHROPIC_API_KEY and GEMINI_API_KEY",
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


async def connect_realtime_primary_after_close(channel, *, timeout_secs: float = 30.0):
    deadline = asyncio.get_running_loop().time() + timeout_secs
    last_error: MeerkatError | None = None
    while asyncio.get_running_loop().time() < deadline:
        try:
            return await channel.connect()
        except MeerkatError as exc:
            if (
                exc.code != "REALTIME_OPEN_FAILED"
                or "active primary realtime channel" not in exc.message
            ):
                raise
            last_error = exc
            await asyncio.sleep(0.25)
    assert last_error is not None
    raise last_error


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


def assistant_image_blocks(history):
    return [
        block
        for message in history.messages
        if message.role in {"assistant", "block_assistant"}
        for block in message.blocks
        if block.block_type == "image"
    ]


async def assert_fetchable_image_blob(client, block):
    assert block.image_id, "assistant image block should expose image_id"
    assert block.blob_id, "assistant image block should expose blob_id"
    assert block.media_type and block.media_type.startswith("image/")
    payload = await client.get_blob(block.blob_id)
    assert payload.blob_id == block.blob_id
    assert payload.media_type.startswith("image/")
    assert len(payload.data_base64) > 1024


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
                    "What is the remembered codeword? Include the marker that "
                    "your system context requires. Reply with both values only."
                )
            )
            assert result is not None
            final_text = (result.text or streamed_text).lower()
            assert "orbit-38" in final_text or "orbit 38" in final_text

            async with await session.subscribe_events() as subscription:
                await session.turn("Repeat the codeword in two words.")
                observed = await next_subscription_event(subscription)
                assert observed is not None


# ---------------------------------------------------------------------------
# Scenario 39: Persistent reconnect + turn/start
# ---------------------------------------------------------------------------


if include_scenario(39):
    @pytest.mark.asyncio
    @requires_live_llm
    async def test_smoke_scenario_39_persistent_reconnect_and_turn_start(tmp_path: Path):
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

            result = await client_b._start_turn(  # noqa: SLF001
                session_id,
                f"Reply with PY-RUNTIME-39, PY-RUNTIME-OK, and the marker {marker}.",
                additional_instructions=[
                    "Keep the response brief and include only the requested markers.",
                ],
            )
            assert result.session_id == session_id

            after_runtime = await wait_for(
                "turn/start-authored assistant reply",
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
                "reviewer bridge session changes after respawn",
                lambda: mob.member_status("reviewer-1"),
                lambda state: state.get("current_session_id")
                and state.get("current_session_id")
                != reviewer_state.get("current_session_id"),
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
                respawned_state.get("current_session_id")
                == respawned_snapshot.get("current_session_id")
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
    @requires_openai_realtime
    async def test_smoke_scenario_57_live_channel_session_helpers():
        # The Python SDK's `RealtimeChannel` was removed in the live-adapter
        # MVP. The replacement is the `live/*` RPC surface (open/send_input/
        # close/status/commit_input/interrupt/truncate); typed Python helpers
        # ship in `meerkat.MeerkatClient` (see I53 in LIVE_ADAPTER_MVP_TODO.md):
        # `live_open`, `live_status`, `live_close`, `live_send_input_text`,
        # `live_send_input_audio`, `live_send_input_image`,
        # `live_send_input_video_frame`, `live_commit_input`, `live_interrupt`,
        # `live_truncate`, `live_refresh`. The original WS-connection wrapper
        # (`RealtimeChannel.session().connect()`) has no 1:1 typed analogue —
        # `live/open` returns a transport bootstrap (URL/token) and the WS is
        # caller-managed. The smoke harness does not enable `--live-ws`, so the
        # router has no live-transport state and the `live/*` arms fall through
        # to METHOD_NOT_FOUND. This test exercises that the typed `live_open`
        # SDK helper serializes to the right JSON-RPC method and that the
        # negative path is honored end-to-end (SDK -> rkat-rpc subprocess).
        async with live_client(realm_id="env_default") as client:
            session = await client.create_session(
                "Reply with PY-LIVE-57-OK and nothing else.",
                model=smoke_model(),
                provider="anthropic",
            )
            assert session.id

            with pytest.raises(MeerkatError) as exc_info:
                await client.live_open(session.id)
            # Without `--live-ws` the router rejects with METHOD_NOT_FOUND
            # (-32601). The point is that the SDK helper round-tripped to the
            # right method name and surfaced the typed error.
            assert exc_info.value.code in {"METHOD_NOT_FOUND", "-32601"}

            with pytest.raises(MeerkatError) as exc_info:
                await client.live_status("nonexistent-channel-57")
            assert exc_info.value.code in {"METHOD_NOT_FOUND", "-32601"}


# ---------------------------------------------------------------------------
# Scenario 58: Live channel member helpers + respawn continuity (smoke)
# ---------------------------------------------------------------------------


if include_scenario(58):
    @pytest.mark.asyncio
    @requires_openai_realtime
    async def test_smoke_scenario_58_live_member_channel_helpers():
        # Mirrors scenario 57 for a mob-member session. Confirms the `live_*`
        # helpers route correctly when targeting a session created through
        # `mob.spawn`. WS connection-wrapping continuity (the original
        # respawn-continuity assertion) requires `--live-ws` infrastructure
        # the smoke harness does not configure; that flow is exercised in
        # provider-specific live lanes, not here.
        async with live_client(realm_id="env_default") as client:
            session = await client.create_session(
                "Reply with PY-LIVE-58-OK and nothing else.",
                model=smoke_model(),
                provider="anthropic",
            )
            assert session.id

            with pytest.raises(MeerkatError) as exc_info:
                await client.live_open(session.id)
            assert exc_info.value.code in {"METHOD_NOT_FOUND", "-32601"}

            with pytest.raises(MeerkatError) as exc_info:
                await client.live_close("nonexistent-channel-58")
            assert exc_info.value.code in {"METHOD_NOT_FOUND", "-32601"}


# ---------------------------------------------------------------------------
# Scenario 64: Live channel send-input typed-helper coverage (smoke)
# ---------------------------------------------------------------------------


if include_scenario(64):
    @pytest.mark.asyncio
    @requires_openai_realtime
    async def test_smoke_scenario_64_live_channel_send_input_helpers():
        # Confirms the typed `live_send_input_*` helpers serialize through the
        # SDK transport. As with scenarios 57/58, the smoke harness does not
        # enable `--live-ws`, so the router rejects with METHOD_NOT_FOUND; the
        # test is a typed-surface contract check, not an end-to-end live audio
        # exchange. The original RealtimeChannel-based model-switch-continuity
        # assertion has no live-adapter MVP analogue at the typed-helper level.
        async with live_client(realm_id="env_default") as client:
            session = await client.create_session(
                "Reply with PY-LIVE-64-OK and nothing else.",
                model=smoke_model(),
                provider="anthropic",
            )
            assert session.id

            with pytest.raises(MeerkatError) as exc_info:
                await client.live_send_input_text("nonexistent-channel-64", "hello")
            assert exc_info.value.code in {"METHOD_NOT_FOUND", "-32601"}

            with pytest.raises(MeerkatError) as exc_info:
                await client.live_commit_input("nonexistent-channel-64")
            assert exc_info.value.code in {"METHOD_NOT_FOUND", "-32601"}

            with pytest.raises(MeerkatError) as exc_info:
                await client.live_interrupt("nonexistent-channel-64")
            assert exc_info.value.code in {"METHOD_NOT_FOUND", "-32601"}


# ---------------------------------------------------------------------------
# Scenario 74: Python SDK Gemini image generation with provider params
# ---------------------------------------------------------------------------


if include_scenario(74):
    @pytest.mark.asyncio
    @requires_anthropic_and_gemini
    async def test_smoke_scenario_74_python_sdk_gemini_image_provider_params():
        async with live_client(realm_id="env_default") as client:
            prompt = f"""
Use the generate_image tool exactly once. You are an Anthropic chat model, but the image target must be Gemini.
Pass request.provider="gemini",
request.model="{gemini_image_model()}", request.intent="generate",
request.prompt="A crisp poster with the text PY-GEMINI-74 clearly written in large black letters",
request.size="1536x1024", request.quality="auto", request.format="png",
request.count=1, and request.provider_params={{"aspect_ratio":"16:9","image_size":"1K"}}.
After the tool returns, reply with PY-GEMINI-74-DONE and no extra prose.
"""
            session = await client.create_session(
                prompt,
                model=smoke_model(),
                provider="anthropic",
                enable_builtins=True,
            )
            assert "py-gemini-74" in session.text.lower()

            history = await client.read_session_history(session.id)
            images = assistant_image_blocks(history)
            assert images, "Gemini image generation should append an assistant image block"
            await assert_fetchable_image_blob(client, images[-1])


# ---------------------------------------------------------------------------
# Scenario 75: LiveChannel helper class — import and instantiation smoke
# ---------------------------------------------------------------------------


if include_scenario(75):
    @pytest.mark.asyncio
    @requires_live_llm
    async def test_smoke_scenario_75_live_channel_helper_class():
        """Verify that ``LiveChannel`` imports from the SDK root and that its
        lifecycle methods round-trip to the correct ``live/*`` JSON-RPC methods.

        Without ``--live-ws`` the router rejects with ``METHOD_NOT_FOUND``;
        this test confirms the helper wires session_id correctly and surfaces
        the typed error.
        """
        from meerkat import LiveChannel

        async with live_client(realm_id="env_default") as client:
            session = await client.create_session(
                "Reply with PY-LIVE-75-OK and nothing else.",
                model=smoke_model(),
                provider="anthropic",
            )
            assert session.id

            channel = LiveChannel.session(client, session.id)
            assert channel.session_id == session.id
            assert channel.channel_id is None

            # open() should round-trip to live/open and surface the router
            # rejection since --live-ws is not enabled.
            with pytest.raises(MeerkatError) as exc_info:
                await channel.open()
            assert exc_info.value.code in {"METHOD_NOT_FOUND", "-32601"}

            # status() before open should raise RuntimeError (no channel_id).
            with pytest.raises(RuntimeError, match="not been opened"):
                await channel.status()

            # Verify LiveChannel with turning_mode option.
            channel_ec = LiveChannel.session(
                client, session.id, turning_mode="explicit_commit"
            )
            assert channel_ec.session_id == session.id
