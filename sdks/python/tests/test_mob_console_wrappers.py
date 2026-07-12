"""Phase 7 (T-A10): the multi-host console wrappers issue the exact RPC
method literals with snake_case params and parse canned results; the live
wrappers never touch logging (token opacity, ADJ-P6B-14)."""

from __future__ import annotations

import logging
from typing import Any

import pytest

from meerkat import MeerkatClient
from meerkat.errors import MeerkatError
from meerkat.mob import Mob

CANNED: dict[str, dict[str, Any]] = {
    "mob/grant_scopes": {
        "record": {
            "principal": "operator-1",
            "scopes": ["list", "admin_host"],
            "expires_at_ms": 9_000,
        }
    },
    "mob/revoke_scopes": {"removed": True},
    "mob/grants": {
        "grants": [
            {
                "principal": "operator-1",
                "scopes": ["list", "admin_host"],
                "expires_at_ms": 9_000,
            }
        ]
    },
    "mob/member_history": {
        "page": {
            "from_index": 5,
            "messages": [],
            "message_count": 5,
            "complete": True,
        },
        "generation": 1,
        "provenance": "controlling_host_verified",
    },
    "mob/hosts": {
        "hosts": [
            {
                "host_id": "host-peer-1",
                "endpoint": "tcp://10.0.0.2:7100",
                "bind_phase": "bound",
                "authority_epoch": 3,
                "capabilities": {
                    "protocol_min": 2,
                    "protocol_max": 2,
                    "engine_version": "0.7.26",
                    "durable_sessions": False,
                    "autonomous_members": True,
                    "hard_cancel_member": True,
                    "memory_store": False,
                    "mcp": False,
                    "approval_forwarding": False,
                },
                "materialized_member_count": 1,
            }
        ]
    },
    "mob/route_installs": {"outstanding": [], "complete": True},
    "mob/bind_host": {
        "host_id": "host-peer-1",
        "capabilities": {
            "protocol_min": 2,
            "protocol_max": 2,
            "engine_version": "0.7.26",
            "durable_sessions": True,
            "autonomous_members": True,
            "hard_cancel_member": True,
            "memory_store": True,
            "mcp": True,
            "approval_forwarding": False,
        },
        "authority_epoch": 1,
    },
    "mob/revoke_host": {"host_id": "host-peer-1", "released_members": ["b2"]},
    "mob/hard_cancel_member": {"cancelled": True},
    "mob/member_live_open": {
        "channel_id": "chan-1",
        "transport": {
            "transport": "websocket",
            "url": "wss://host.test.invalid/live?channel=chan-1",
            "token": "tok-single-use-secret",
        },
        "capabilities": {
            "audio_in": True,
            "audio_out": True,
            "barge_in_supported": True,
            "image_in": False,
            "provider_native_resume": False,
            "text_in": True,
            "text_out": True,
            "transcript_supported": True,
            "video_in": False,
        },
        "continuity": {"mode": "fresh"},
    },
    "mob/member_live_close": {"status": "closed"},
    "mob/member_live_status": {
        "channel_id": "chan-1",
        "status": {"status": "ready"},
    },
    "mob/member_live_control": {"verb": "commit_input", "status": "committed"},
}


def canned_client() -> tuple[MeerkatClient, list[tuple[str, dict[str, Any]]]]:
    client = MeerkatClient()
    calls: list[tuple[str, dict[str, Any]]] = []

    async def fake_request(method: str, params: dict[str, Any]) -> dict[str, Any]:
        calls.append((method, params))
        result = CANNED.get(method)
        if result is None:
            raise AssertionError(f"no canned result for {method}")
        return dict(result)

    client._request = fake_request  # type: ignore[assignment]
    return client, calls


def test_multi_host_client_surface_is_present() -> None:
    expected = {
        "grant_mob_scopes",
        "revoke_mob_scopes",
        "list_mob_grants",
        "mob_member_history",
        "mob_hosts",
        "mob_route_installs",
        "bind_mob_host",
        "revoke_mob_host",
        "hard_cancel_mob_member",
        "open_mob_member_live",
        "close_mob_member_live",
        "mob_member_live_status",
        "control_mob_member_live",
    }
    assert all(callable(getattr(MeerkatClient, name, None)) for name in expected)


@pytest.mark.asyncio
async def test_grant_wrappers_preserve_nested_record_contract() -> None:
    client, calls = canned_client()
    mob = Mob(client, "mob-1")

    granted = await mob.grant_scopes(
        "operator-1",
        ["list", "admin_host"],
        expires_at_ms=9_000,
    )
    assert calls[0] == (
        "mob/grant_scopes",
        {
            "mob_id": "mob-1",
            "principal": "operator-1",
            "scopes": ["list", "admin_host"],
            "expires_at_ms": 9_000,
        },
    )
    assert granted == {
        "principal": "operator-1",
        "scopes": ["list", "admin_host"],
        "expires_at_ms": 9_000,
    }
    assert "record" not in granted

    assert await mob.revoke_scopes("operator-1") is True
    assert calls[1] == (
        "mob/revoke_scopes",
        {"mob_id": "mob-1", "principal": "operator-1"},
    )

    assert await mob.grants() == [granted]
    assert calls[2] == ("mob/grants", {"mob_id": "mob-1"})


@pytest.mark.asyncio
async def test_member_history_issues_exact_literal_and_paging() -> None:
    client, calls = canned_client()
    mob = Mob(client, "mob-1")
    result = await mob.member_history("worker", from_index=5, limit=10)
    assert calls == [
        (
            "mob/member_history",
            {
                "mob_id": "mob-1",
                "agent_identity": "worker",
                "from_index": 5,
                "limit": 10,
            },
        )
    ]
    assert result.generation == 1
    assert result.provenance == "controlling_host_verified"
    assert result.page.complete is True


@pytest.mark.asyncio
async def test_hosts_unwraps_rows_and_route_installs_keeps_envelope() -> None:
    client, calls = canned_client()
    mob = Mob(client, "mob-1")
    hosts = await mob.hosts()
    assert calls[0] == ("mob/hosts", {"mob_id": "mob-1"})
    assert len(hosts) == 1
    assert hosts[0].bind_phase == "bound"
    assert hosts[0].capabilities is not None
    assert hosts[0].capabilities.durable_sessions is False

    installs = await mob.route_installs()
    assert calls[1] == ("mob/route_installs", {"mob_id": "mob-1"})
    assert installs.complete is True
    assert installs.outstanding == []


@pytest.mark.asyncio
async def test_bind_and_revoke_host_round_trip() -> None:
    client, calls = canned_client()
    mob = Mob(client, "mob-1")
    descriptor = {
        "kind": "host",
        "address": "tcp://10.0.0.2:7100",
        "identity": {"kind": "ed25519_public_key", "public_key": "ed25519:abc"},
        "bootstrap_token": "token-1",
    }
    report = await mob.bind_host(descriptor)
    assert calls[0] == (
        "mob/bind_host",
        {"mob_id": "mob-1", "descriptor": descriptor},
    )
    assert report.host_id == "host-peer-1"

    revoked = await mob.revoke_host("host-peer-1")
    assert calls[1] == (
        "mob/revoke_host",
        {"mob_id": "mob-1", "host_id": "host-peer-1"},
    )
    assert revoked.released_members == ["b2"]


@pytest.mark.asyncio
async def test_hard_cancel_unwraps_cancelled() -> None:
    client, calls = canned_client()
    mob = Mob(client, "mob-1")
    cancelled = await mob.hard_cancel("worker", "operator interrupt")
    assert calls == [
        (
            "mob/hard_cancel_member",
            {
                "mob_id": "mob-1",
                "agent_identity": "worker",
                "reason": "operator interrupt",
            },
        )
    ]
    assert cancelled is True


@pytest.mark.asyncio
async def test_live_wrappers_pass_verbatim_and_never_log(caplog) -> None:
    client, calls = canned_client()
    mob = Mob(client, "mob-1")

    with caplog.at_level(logging.DEBUG):
        opened = await mob.member_live_open(
            "worker", turning_mode="provider_managed", transport="websocket"
        )
        closed = await mob.member_live_close("worker", "chan-1")
        # Discovery read: absent channel_id is OMITTED from the params.
        status = await mob.member_live_status("worker")
        outcome = await mob.member_live_control(
            "worker", "chan-1", {"verb": "commit_input"}
        )

    assert calls[0] == (
        "mob/member_live_open",
        {
            "mob_id": "mob-1",
            "agent_identity": "worker",
            "turning_mode": "provider_managed",
            "transport": "websocket",
        },
    )
    # Verbatim pass-through: the token survives untouched.
    assert opened.transport["token"] == "tok-single-use-secret"
    assert opened.capabilities.audio_in is True
    assert opened.continuity == {"mode": "fresh"}

    assert calls[1] == (
        "mob/member_live_close",
        {"mob_id": "mob-1", "agent_identity": "worker", "channel_id": "chan-1"},
    )
    assert closed.status == "closed"

    assert calls[2] == (
        "mob/member_live_status",
        {"mob_id": "mob-1", "agent_identity": "worker"},
    )
    assert status.channel_id == "chan-1"
    assert status.status == {"status": "ready"}

    assert calls[3] == (
        "mob/member_live_control",
        {
            "mob_id": "mob-1",
            "agent_identity": "worker",
            "channel_id": "chan-1",
            "verb": {"verb": "commit_input"},
        },
    )
    assert outcome["verb"] == "commit_input"

    # Token opacity: nothing on the live path logs, and the single-use
    # token never reaches a log record.
    assert all(
        "tok-single-use-secret" not in record.getMessage() for record in caplog.records
    ), "single-use token must never be logged"


def _history_result(message: dict[str, Any]) -> dict[str, Any]:
    return {
        "page": {
            "from_index": 0,
            "messages": [message],
            "message_count": 1,
            "complete": True,
        },
        "generation": 1,
        "provenance": "controlling_host_verified",
    }


@pytest.mark.parametrize(
    "message",
    [
        {
            "role": "user",
            "created_at": "2026-07-12T10:00:00Z",
            "content": [{"type": "image", "media_type": "image/png"}],
        },
        {
            "role": "system_notice",
            "created_at": "2026-07-12T10:00:00Z",
            "kind": "comms",
            "blocks": [{"type": "comms"}],
        },
        {
            "role": "block_assistant",
            "created_at": "2026-07-12T10:00:00Z",
            "blocks": [{"block_type": "tool_use", "data": {"args": {}}}],
            "stop_reason": "tool_use",
        },
        {
            "role": "tool_results",
            "created_at": "2026-07-12T10:00:00Z",
            "results": [
                {
                    "tool_use_id": "tool-1",
                    "content": [{"garbage": True}],
                    "is_error": False,
                }
            ],
        },
    ],
)
def test_member_history_recursively_rejects_malformed_rows(
    message: dict[str, Any],
) -> None:
    with pytest.raises(MeerkatError) as exc_info:
        MeerkatClient._parse_mob_member_history_result(_history_result(message))
    assert exc_info.value.code == "INVALID_RESPONSE"


def test_history_validator_accepts_only_schema_nullable_nested_fields() -> None:
    nullable_notice = {
        "role": "system_notice",
        "created_at": "2026-07-12T10:00:00Z",
        "kind": "generic",
        "body": None,
        "blocks": [
            {
                "type": "comms",
                "direction": "incoming",
                "kind": "message",
                "intent": None,
                "request_id": None,
                "status": None,
                "summary": None,
                "peer": None,
                "sender_taint": None,
            },
            {
                "type": "comms",
                "direction": "outgoing",
                "kind": "response",
                "peer": {"id": "peer-1", "display_name": None},
            },
            {
                "type": "external_event",
                "source": "scheduler",
                "event_type": "tick",
                "body": None,
                "summary": None,
            },
            {
                "type": "tool_config",
                "payload": {
                    "operation": "add",
                    "target": "search",
                    "status_info": {
                        "kind": "external_tool_delta",
                        "phase": "pending",
                        "detail": None,
                    },
                    "persisted": True,
                    "applied_at_turn": None,
                    "domain": None,
                    "deferred_catalog_delta": None,
                },
            },
            {
                "type": "mcp",
                "detail": None,
                "server_id": None,
                "operation": None,
                "phase": None,
                "persisted": False,
            },
            {
                "type": "background_job",
                "job_id": "job-1",
                "status": "completed",
                "detail": None,
                "display_name": None,
            },
            {"type": "auth", "state": "ready", "binding": None, "detail": None},
            {"type": "runtime_notice", "category": "runtime", "detail": None},
            {"type": "unknown", "summary": None},
        ],
    }
    nullable_assistant = {
        "role": "block_assistant",
        "created_at": "2026-07-12T10:00:01Z",
        "blocks": [
            {"block_type": "text", "data": {"text": "ok", "meta": None}},
            {
                "block_type": "server_tool_content",
                "data": {
                    "id": None,
                    "kind": {"kind": "web_search"},
                    "content": {},
                    "meta": None,
                },
            },
        ],
        "stop_reason": "end_turn",
    }

    assert (
        MeerkatClient._validate_wire_history_row(nullable_notice, "history")
        is nullable_notice
    )
    assert (
        MeerkatClient._validate_wire_history_row(nullable_assistant, "history")
        is nullable_assistant
    )


@pytest.mark.parametrize(
    "message",
    [
        {
            "role": "block_assistant",
            "created_at": "2026-07-12T10:00:00Z",
            "blocks": [{"block_type": "reasoning", "data": {"text": None}}],
            "stop_reason": "end_turn",
        },
        {
            "role": "system_notice",
            "created_at": "2026-07-12T10:00:00Z",
            "kind": "mcp",
            "blocks": [{"type": "mcp", "persisted": None}],
        },
        {
            "role": "tool_results",
            "created_at": "2026-07-12T10:00:00Z",
            "results": [{"tool_use_id": "tool-1", "content": "ok", "is_error": None}],
        },
    ],
)
def test_history_validator_does_not_widen_non_nullable_fields(
    message: dict[str, Any],
) -> None:
    with pytest.raises(MeerkatError) as exc_info:
        MeerkatClient._validate_wire_history_row(message, "history")
    assert exc_info.value.code == "INVALID_RESPONSE"


@pytest.mark.parametrize(
    "meta",
    [
        {"provider": "anthropic", "signature": "sig"},
        {"provider": "anthropic_redacted", "data": "redacted"},
        {"provider": "anthropic_compaction", "content": "compact"},
        {"provider": "gemini", "thoughtSignature": "thought"},
        {
            "provider": "open_ai",
            "id": "item-1",
            "encrypted_content": None,
            "phase": None,
            "response_id": None,
        },
        {"provider": "open_ai_response", "response_id": "response-1"},
        {"provider": "unknown"},
    ],
)
def test_assistant_validator_accepts_closed_provider_metadata(
    meta: dict[str, Any],
) -> None:
    MeerkatClient._validate_wire_assistant_block(
        {"block_type": "text", "data": {"text": "ok", "meta": meta}},
        "assistant",
    )


@pytest.mark.parametrize(
    "kind",
    [
        {"kind": "web_search"},
        {"kind": "google_search"},
        {"kind": "provider_native", "name": "search"},
        {"kind": "unknown", "debug": "future_tool"},
    ],
)
def test_assistant_validator_accepts_closed_server_tool_kinds(
    kind: dict[str, Any],
) -> None:
    MeerkatClient._validate_wire_assistant_block(
        {
            "block_type": "server_tool_content",
            "data": {"kind": kind, "content": {}, "id": None},
        },
        "assistant",
    )


def _assistant_image_block(
    *,
    meta: dict[str, Any] | None = None,
    revised_prompt: dict[str, Any] | None = None,
) -> dict[str, Any]:
    return {
        "block_type": "image",
        "data": {
            "image_id": "image-1",
            "blob_ref": {"blob_id": "blob-1", "media_type": "image/png"},
            "media_type": "image/png",
            "width": 1024,
            "height": 768,
            "meta": meta or {"provider": "not_emitted"},
            "revised_prompt": revised_prompt or {"disposition": "not_requested"},
        },
    }


@pytest.mark.parametrize(
    ("meta", "revised_prompt"),
    [
        ({"provider": "not_emitted"}, {"disposition": "not_requested"}),
        (
            {
                "provider": "openai",
                "target_model": "gpt-image-1",
                "response_id": None,
                "image_generation_call_id": None,
            },
            {"disposition": "unsupported_by_backend"},
        ),
        (
            {
                "provider": "gemini",
                "target_model": "gemini-image",
                "response_id": None,
                "continuity_ref": None,
            },
            {
                "disposition": "revised",
                "source": "meerkat_projection",
                "text": {"content": "revised prompt"},
            },
        ),
    ],
)
def test_assistant_validator_accepts_closed_image_metadata_and_revisions(
    meta: dict[str, Any],
    revised_prompt: dict[str, Any],
) -> None:
    MeerkatClient._validate_wire_assistant_block(
        _assistant_image_block(meta=meta, revised_prompt=revised_prompt),
        "assistant",
    )


@pytest.mark.parametrize(
    "block",
    [
        {"block_type": "text", "data": {"text": "ok", "meta": {"provider": "future"}}},
        {
            "block_type": "text",
            "data": {"text": "ok", "meta": {"provider": "anthropic"}},
        },
        {
            "block_type": "text",
            "data": {
                "text": "ok",
                "meta": {"provider": "open_ai", "id": "item-1", "phase": False},
            },
        },
        {
            "block_type": "server_tool_content",
            "data": {"kind": {"kind": "provider_native"}, "content": {}},
        },
        {
            "block_type": "server_tool_content",
            "data": {"kind": {"kind": "unknown"}, "content": {}},
        },
        {
            "block_type": "server_tool_content",
            "data": {"kind": {"kind": "future"}, "content": {}},
        },
        _assistant_image_block(meta={"provider": "openai"}),
        _assistant_image_block(
            meta={"provider": "gemini", "target_model": "model", "response_id": 7}
        ),
        _assistant_image_block(revised_prompt={"disposition": "future"}),
        _assistant_image_block(
            revised_prompt={
                "disposition": "revised",
                "source": "future",
                "text": {"content": "prompt"},
            }
        ),
        _assistant_image_block(
            revised_prompt={
                "disposition": "revised",
                "source": "provider",
                "text": {},
            }
        ),
    ],
)
def test_assistant_validator_recursively_rejects_malformed_nested_unions(
    block: dict[str, Any],
) -> None:
    with pytest.raises(MeerkatError) as exc_info:
        MeerkatClient._validate_wire_assistant_block(block, "assistant")
    assert exc_info.value.code == "INVALID_RESPONSE"


def test_assistant_validator_recursively_rejects_malformed_blob_ref() -> None:
    block = _assistant_image_block()
    block["data"]["blob_ref"] = {"blob_id": "blob-1"}
    with pytest.raises(MeerkatError) as exc_info:
        MeerkatClient._validate_wire_assistant_block(block, "assistant")
    assert exc_info.value.code == "INVALID_RESPONSE"


@pytest.mark.parametrize(
    "page",
    [
        {
            "from_index": 0,
            "messages": [
                {
                    "role": "system",
                    "content": "rules",
                    "created_at": "2026-07-12T10:00:00Z",
                }
            ],
            "message_count": 2,
            "complete": True,
        },
        {
            "from_index": 0,
            "messages": [],
            "message_count": 1,
            "next_index": 0,
            "complete": False,
        },
    ],
)
def test_member_history_rejects_non_monotonic_pagination(
    page: dict[str, Any],
) -> None:
    with pytest.raises(MeerkatError) as exc_info:
        MeerkatClient._parse_mob_member_history_result(
            {
                "page": page,
                "generation": 1,
                "provenance": "controlling_host_verified",
            }
        )
    assert exc_info.value.code == "INVALID_RESPONSE"


def _host_capabilities() -> dict[str, Any]:
    return {
        "protocol_min": 1,
        "protocol_max": 1,
        "engine_version": "0.7.0",
        "durable_sessions": True,
        "autonomous_members": True,
        "hard_cancel_member": True,
        "tracked_input_cancel": True,
        "memory_store": True,
        "mcp": True,
        "approval_forwarding": True,
    }


def test_host_capabilities_reject_explicit_null_default_collection() -> None:
    capabilities = _host_capabilities()
    capabilities["resolvable_providers"] = None
    with pytest.raises(MeerkatError) as exc_info:
        MeerkatClient._parse_mob_host_capabilities(
            capabilities,
            "Invalid mob/hosts response: capabilities",
        )
    assert exc_info.value.code == "INVALID_RESPONSE"


@pytest.mark.parametrize(
    "reason",
    [
        {"kind": "future_reason"},
        {"kind": "other"},
        {"kind": "unknown"},
    ],
)
def test_member_live_status_recursively_rejects_invalid_degradation_reason(
    reason: dict[str, Any],
) -> None:
    with pytest.raises(MeerkatError) as exc_info:
        MeerkatClient._parse_live_status_result(
            {
                "channel_id": "channel-1",
                "status": {"status": "degraded", "reason": reason},
            },
            "Invalid mob/member_live_status response",
        )
    assert exc_info.value.code == "INVALID_RESPONSE"


@pytest.mark.asyncio
async def test_member_live_control_correlates_response_to_requested_verb() -> None:
    client = MeerkatClient()

    async def fake_request(_method: str, _params: dict[str, Any]) -> dict[str, Any]:
        return {"verb": "commit_input", "status": "committed"}

    client._request = fake_request  # type: ignore[method-assign]
    with pytest.raises(MeerkatError) as exc_info:
        await client.control_mob_member_live(
            "mob-1",
            "worker",
            "channel-1",
            {"verb": "interrupt"},
        )
    assert exc_info.value.code == "INVALID_RESPONSE"
