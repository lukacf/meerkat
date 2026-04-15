"""Live smoke tests for the Python SDK against a real rkat-rpc subprocess."""

from __future__ import annotations

import asyncio
import os
from pathlib import Path
from uuid import uuid4

import pytest

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
# Scenario 39: Persistent reconnect + runtime accept
# ---------------------------------------------------------------------------


if include_scenario(39):
    @pytest.mark.asyncio
    @requires_live_llm
    async def test_smoke_scenario_39_persistent_reconnect_and_runtime_accept(tmp_path: Path):
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

            runtime_state = await client_b.runtime_state(session_id)
            assert runtime_state.state in {
                "attached",
                "idle",
                "running",
                "initializing",
            }

            accepted = await client_b.runtime_accept(
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
                lambda: client_b.input_state(session_id, input_id),
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
            reviewer_runtime_id = reviewer["agent_runtime_id"]
            reviewer_fence_token = reviewer["fence_token"]
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
                assert reviewer_receipt["agent_runtime_id"] == reviewer_runtime_id
                assert reviewer_receipt["fence_token"] == reviewer_fence_token
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
            respawned_runtime_id = respawn_result["receipt"]["agent_runtime_id"]
            respawned_fence_token = respawn_result["receipt"]["fence_token"]
            respawned_members = await wait_for(
                "reviewer active after respawn",
                mob.members,
                lambda entries: any(
                    entry["agent_identity"] == "reviewer-1"
                    and entry["agent_runtime_id"] == respawned_runtime_id
                    and entry.get("state") == "Active"
                    for entry in entries
                ),
                timeout_secs=60.0,
            )
            assert any(
                entry["agent_identity"] == "reviewer-1"
                and entry["agent_runtime_id"] == respawned_runtime_id
                for entry in respawned_members
            )
            respawn_receipt = await mob.member("reviewer-1").send(
                "Reply with REVIEWER_RESPAWN_40.",
            )
            assert respawn_receipt["agent_identity"] == "reviewer-1"
            assert respawn_receipt["agent_runtime_id"] == respawned_runtime_id
            assert respawn_receipt["fence_token"] == respawned_fence_token
            respawned_state = await wait_for(
                "reviewer reply after respawn",
                lambda: mob.member_status("reviewer-1"),
                lambda state: "reviewer_respawn_40"
                in (state.get("output_preview") or "").lower(),
                timeout_secs=120.0,
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
