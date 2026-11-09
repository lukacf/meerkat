"""Operator instructions on the Python SDK.

The session and mob verbs send the exact RPC literals with snake_case params.
The status reads parse fail-closed on the closed vocabularies (dispositions,
effects, entry kinds, state kinds) and keep an advisory value this SDK does
not know (a pending or blocked reason, a residency, a lowering) as
``{"unknown": <the value>}``. A refusal's closed code reads typed from the
error data, and the applied-instructions event is schema-known.
"""

from __future__ import annotations

import json
from types import SimpleNamespace
from typing import Any

import pytest

from meerkat import MeerkatClient
from meerkat.errors import MeerkatError
from meerkat.mob import Mob
from meerkat.session import Session

SESSION_ID = "019a2b3c-0000-7000-8000-000000000001"
RUN_ID = "019a2b3c-0000-7000-8000-000000000002"
DIGEST = "sha256:" + "a" * 64

MOB_WRITE = {
    "mob_id": "team",
    "namespace": "app.example",
    "key": "tone",
    "revision": 1,
    "disposition": "committed",
    "generation": 1,
    "matched_members": 2,
}

SESSION_WRITE = {
    "session_id": SESSION_ID,
    "namespace": "app.example",
    "key": "tone",
    "revision": 1,
    "disposition": "recorded",
    "target_generation": 1,
}


def member_status(
    *,
    residency: str = "dormant",
    delivery_state: dict[str, Any] | None = None,
    slot_state: dict[str, Any] | None = None,
) -> dict[str, Any]:
    return {
        "agent_identity": "worker-1",
        "profile": "worker",
        "residency": residency,
        "delivery": {
            "recorded_generation": 1,
            "state": delivery_state or {"state": "current"},
        },
        "slots": [
            {
                "namespace": "app.example",
                "key": "tone",
                "desired": {"revision": 1, "effect": "active"},
                "recorded": {"revision": 1, "effect": "active"},
                "effective": {
                    "revision": 1,
                    "effect": "active",
                    "entry_sequence": 3,
                    "kind": "delta",
                    "run_id": RUN_ID,
                    "message_index": 7,
                },
                "state": slot_state or {"state": "effective"},
            }
        ],
        "effective_read": {"status": "ok"},
        "live_channel_open": False,
    }


def mob_status(member: dict[str, Any]) -> dict[str, Any]:
    return {
        "mob_id": "team",
        "generation": 1,
        "desired": [
            {
                "namespace": "app.example",
                "key": "tone",
                "revision": 1,
                "effect": "active",
                "target": {"kind": "profile", "profile": "worker"},
                "content_sha256": DIGEST,
                "body_bytes": 16,
            }
        ],
        "members": [member],
        "next_offset": 1,
    }


def session_status(
    *, lowering: str = "mid_conversation", reason: str = "awaiting_run_start"
) -> dict[str, Any]:
    return {
        "session_id": SESSION_ID,
        "owner": {"kind": "session"},
        "target_generation": 2,
        "entry_sequence": 3,
        "lowering": lowering,
        "slots": [
            {
                "namespace": "app.example",
                "key": "tone",
                "recorded": {"revision": 2, "effect": "active"},
                "effective": {
                    "revision": 1,
                    "effect": "active",
                    "entry_sequence": 3,
                    "kind": "snapshot",
                    "message_index": 1,
                },
                "state": {"state": "pending", "reason": reason},
            }
        ],
    }


def fake_client(
    results: dict[str, dict[str, Any]],
) -> tuple[MeerkatClient, list[tuple[str, dict[str, Any]]]]:
    client = MeerkatClient()
    calls: list[tuple[str, dict[str, Any]]] = []

    async def fake_request(method: str, params: dict[str, Any]) -> dict[str, Any]:
        calls.append((method, params))
        return json.loads(json.dumps(results[method]))

    client._request = fake_request  # type: ignore[assignment]
    return client, calls


def test_the_sdk_exposes_the_operator_instruction_verbs() -> None:
    for name in (
        "set_mob_instruction",
        "revoke_mob_instruction",
        "mob_instruction_status",
        "set_session_instruction",
        "revoke_session_instruction",
        "session_instruction_status",
    ):
        assert callable(getattr(MeerkatClient, name, None)), f"MeerkatClient.{name}"
    for name in ("set_instruction", "revoke_instruction", "instruction_status"):
        assert callable(getattr(Mob, name, None)), f"Mob.{name}"
        assert callable(getattr(Session, name, None)), f"Session.{name}"


@pytest.mark.asyncio
async def test_mob_verbs_send_the_exact_request_shapes() -> None:
    client, calls = fake_client(
        {
            "mob/set_instruction": MOB_WRITE,
            "mob/revoke_instruction": {**MOB_WRITE, "revision": 2, "generation": 2},
        }
    )
    mob = Mob(client, "team")

    written = await mob.set_instruction(
        "app.example",
        "tone",
        1,
        "Answer formally.",
        {"kind": "profile", "profile": "worker"},
        content_sha256=DIGEST,
    )
    revoked = await client.revoke_mob_instruction("team", "app.example", "tone", 2)

    assert calls == [
        (
            "mob/set_instruction",
            {
                "mob_id": "team",
                "namespace": "app.example",
                "key": "tone",
                "revision": 1,
                "body": "Answer formally.",
                "content_sha256": DIGEST,
                "target": {"kind": "profile", "profile": "worker"},
            },
        ),
        (
            "mob/revoke_instruction",
            {"mob_id": "team", "namespace": "app.example", "key": "tone", "revision": 2},
        ),
    ]
    assert written == MOB_WRITE
    assert revoked["generation"] == 2


@pytest.mark.asyncio
async def test_mob_status_sends_its_filters_and_parses_the_page() -> None:
    page = mob_status(member_status())
    client, calls = fake_client({"mob/instruction_status": page})

    status = await client.mob_instruction_status(
        "team", namespace="app.example", key="tone", profile="worker", offset=0, limit=50
    )

    assert calls == [
        (
            "mob/instruction_status",
            {
                "mob_id": "team",
                "namespace": "app.example",
                "key": "tone",
                "profile": "worker",
                "offset": 0,
                "limit": 50,
            },
        )
    ]
    assert status == page


@pytest.mark.asyncio
async def test_mob_status_keeps_unknown_advisory_values_as_the_unknown_form() -> None:
    owed = member_status(
        residency="orbiting",
        delivery_state={"state": "blocked", "reason": "host_on_holiday"},
        slot_state={"state": "pending", "reason": "awaiting_the_moon"},
    )
    client, _ = fake_client({"mob/instruction_status": mob_status(owed)})

    status = await client.mob_instruction_status("team")

    member = status["members"][0]
    assert member["residency"] == {"unknown": "orbiting"}
    assert member["delivery"]["state"] == {
        "state": "blocked",
        "reason": {"unknown": "host_on_holiday"},
    }
    assert member["slots"][0]["state"] == {
        "state": "pending",
        "reason": {"unknown": "awaiting_the_moon"},
    }


@pytest.mark.asyncio
async def test_mob_status_refuses_an_unknown_closed_value() -> None:
    for member in (
        member_status(slot_state={"state": "levitating"}),
        member_status(delivery_state={"state": "teleported"}),
    ):
        client, _ = fake_client({"mob/instruction_status": mob_status(member)})
        with pytest.raises(MeerkatError) as refused:
            await client.mob_instruction_status("team")
        assert refused.value.code == "INVALID_RESPONSE"


@pytest.mark.asyncio
async def test_session_verbs_send_the_exact_request_shapes() -> None:
    client, calls = fake_client(
        {
            "session/set_instruction": SESSION_WRITE,
            "session/revoke_instruction": {
                **SESSION_WRITE,
                "revision": 2,
                "target_generation": 2,
            },
            "session/instruction_status": session_status(),
        }
    )
    session = Session(client, SimpleNamespace(session_id=SESSION_ID, session_ref=None))  # type: ignore[arg-type]

    written = await session.set_instruction("app.example", "tone", 1, "Answer formally.")
    revoked = await session.revoke_instruction("app.example", "tone", 2)
    status = await session.instruction_status()

    assert calls == [
        (
            "session/set_instruction",
            {
                "session_id": SESSION_ID,
                "namespace": "app.example",
                "key": "tone",
                "revision": 1,
                "body": "Answer formally.",
            },
        ),
        (
            "session/revoke_instruction",
            {
                "session_id": SESSION_ID,
                "namespace": "app.example",
                "key": "tone",
                "revision": 2,
            },
        ),
        ("session/instruction_status", {"session_id": SESSION_ID}),
    ]
    assert written == SESSION_WRITE
    assert revoked["target_generation"] == 2
    assert status == session_status()


@pytest.mark.asyncio
async def test_session_status_keeps_unknown_advisory_values_as_the_unknown_form() -> None:
    client, _ = fake_client(
        {
            "session/instruction_status": session_status(
                lowering="carrier_pigeon", reason="awaiting_the_tide"
            )
        }
    )

    status = await client.session_instruction_status(SESSION_ID)

    assert status["lowering"] == {"unknown": "carrier_pigeon"}
    assert status["slots"][0]["state"] == {
        "state": "pending",
        "reason": {"unknown": "awaiting_the_tide"},
    }


@pytest.mark.asyncio
async def test_session_status_refuses_an_unknown_owner_kind() -> None:
    page = session_status()
    page["owner"] = {"kind": "committee"}
    client, _ = fake_client({"session/instruction_status": page})

    with pytest.raises(MeerkatError) as refused:
        await client.session_instruction_status(SESSION_ID)
    assert refused.value.code == "INVALID_RESPONSE"


def test_a_refusal_reads_its_closed_instruction_code() -> None:
    from meerkat.errors import instruction_set_code

    governed = MeerkatError(
        "-32602",
        "operator instruction writes on a governed runtime need prepared authorization",
        details={"instruction_set_code": "prepared_authorization_required"},
    )
    stale = MeerkatError(
        "-32602", "stale", details={"instruction_set_code": "stale_revision"}
    )
    invented = MeerkatError(
        "-32602", "invented", details={"instruction_set_code": "invented_code"}
    )

    assert instruction_set_code(governed) == "prepared_authorization_required"
    assert instruction_set_code(stale) == "stale_revision"
    assert instruction_set_code(invented) is None
    assert instruction_set_code(MeerkatError("-32602", "plain")) is None


def test_the_applied_instructions_event_is_known_and_keeps_its_payload() -> None:
    from meerkat.events import UnknownEvent, parse_event
    from meerkat.generated.event_inventory import KNOWN_AGENT_EVENT_TYPES

    raw = {
        "type": "operator_instructions_applied",
        "session_id": SESSION_ID,
        "entry_sequence": 3,
        "kind": "delta",
        "run_id": RUN_ID,
        "active": [
            {
                "namespace": "app.example",
                "key": "tone",
                "revision": 1,
                "content_sha256": DIGEST,
                "body_bytes": 16,
            }
        ],
        "changes": [
            {
                "change": "activate",
                "namespace": "app.example",
                "key": "tone",
                "revision": 1,
                "content_sha256": DIGEST,
                "body_bytes": 16,
            }
        ],
    }
    assert raw["type"] in KNOWN_AGENT_EVENT_TYPES
    event = parse_event(json.loads(json.dumps(raw)))
    # Schema-known without a handwritten class: the raw-preserving path keeps
    # every field, so a kind or change kind this SDK does not know is kept too.
    assert isinstance(event, UnknownEvent)
    assert event.data == raw
    future = {**raw, "kind": "replay", "changes": [{"change": "suspend"}]}
    assert parse_event(json.loads(json.dumps(future))).data == future
