"""Operator instructions: standing System-plane instructions a host sets by
slot (a namespace it owns and a key), revision and body, for one session or
for the members of a mob.

The write and status results keep the wire shape (dicts with snake_case
keys) and are checked on read. Dispositions, effects, entry kinds and state
kinds are closed: a value this SDK does not know fails the read
(``INVALID_RESPONSE``). The advisory values a status read carries (why a
change is still pending, why a delivery is blocked, where a member's session
lives, how a model lowers entries) may grow: a value this SDK does not know
reads as ``{"unknown": <the value>}``, so a newer server never fails the
read.
"""

from __future__ import annotations

import sys
from typing import Any, Literal, cast, get_args

# Python 3.10 has no typing.NotRequired, and its TypedDict ignores it.
if sys.version_info >= (3, 11):
    from typing import NotRequired, TypedDict
else:
    from typing_extensions import NotRequired, TypedDict

from .generated.errors import MeerkatError
from .generated.types import (
    InstructionEffect,
    InstructionEntryKind,
    WireInstructionBlockReason,
    WireInstructionLowering,
    WireInstructionPendingReason,
    WireInstructionResidency,
    WireMobInstructionDisposition,
    WireSessionInstructionDisposition,
)


def _literal_values(alias: Any) -> frozenset[str]:
    values: set[str] = set()
    for arm in get_args(alias) or (alias,):
        args = get_args(arm)
        if args and all(isinstance(value, str) for value in args):
            values.update(args)
        elif isinstance(arm, str):
            values.add(arm)
        else:
            values.update(_literal_values(arm))
    return frozenset(values)


_EFFECTS = _literal_values(InstructionEffect)
_ENTRY_KINDS = _literal_values(InstructionEntryKind)
_MOB_DISPOSITIONS = _literal_values(WireMobInstructionDisposition)
_SESSION_DISPOSITIONS = _literal_values(WireSessionInstructionDisposition)
_PENDING_REASONS = _literal_values(WireInstructionPendingReason)
_BLOCK_REASONS = _literal_values(WireInstructionBlockReason)
_RESIDENCIES = _literal_values(WireInstructionResidency)
_LOWERINGS = _literal_values(WireInstructionLowering)

# An advisory value this SDK does not know, as the server sent it.
InstructionAdvisoryUnknown = TypedDict(
    "InstructionAdvisoryUnknown", {"unknown": str}
)
InstructionPendingReason = WireInstructionPendingReason | InstructionAdvisoryUnknown
InstructionBlockReason = WireInstructionBlockReason | InstructionAdvisoryUnknown
InstructionResidency = WireInstructionResidency | InstructionAdvisoryUnknown
InstructionLowering = WireInstructionLowering | InstructionAdvisoryUnknown

# Which members of a mob an operator instruction targets: every member, the
# members of one profile, or one member. A target is a predicate over the
# roster: a profile or member that does not exist yet selects nobody until it
# does.
InstructionTargetMob = TypedDict("InstructionTargetMob", {"kind": Literal["mob"]})
InstructionTargetProfile = TypedDict(
    "InstructionTargetProfile", {"kind": Literal["profile"], "profile": str}
)
InstructionTargetMember = TypedDict(
    "InstructionTargetMember",
    {"kind": Literal["member"], "agent_identity": str},
)
InstructionTarget = (
    InstructionTargetMob | InstructionTargetProfile | InstructionTargetMember
)

# The result of `mob/set_instruction` and `mob/revoke_instruction`, returned
# once the record is in the mob's journal. Delivery is asynchronous.
MobInstructionWriteResult = TypedDict(
    "MobInstructionWriteResult",
    {
        "mob_id": str,
        "namespace": str,
        "key": str,
        "revision": int,
        "disposition": WireMobInstructionDisposition,
        # The mob's operator-instruction generation after the write.
        "generation": int,
        # How many current members the record's target selects.
        "matched_members": int,
    },
)

# The result of `session/set_instruction` and `session/revoke_instruction`,
# returned once the target is durable in the session document.
SessionInstructionWriteResult = TypedDict(
    "SessionInstructionWriteResult",
    {
        "session_id": str,
        "namespace": str,
        "key": str,
        "revision": int,
        "disposition": WireSessionInstructionDisposition,
        "target_generation": int,
    },
)

# A slot's revision and effect.
InstructionSlotView = TypedDict(
    "InstructionSlotView", {"revision": int, "effect": InstructionEffect}
)

# The committed entry that put a slot's state in effect.
InstructionEffectiveSlot = TypedDict(
    "InstructionEffectiveSlot",
    {
        "revision": int,
        "effect": InstructionEffect,
        "entry_sequence": int,
        "kind": InstructionEntryKind,
        "run_id": NotRequired[str],
        "message_index": int,
    },
)

InstructionSlotStateEffective = TypedDict(
    "InstructionSlotStateEffective", {"state": Literal["effective"]}
)
InstructionSlotStatePending = TypedDict(
    "InstructionSlotStatePending",
    {"state": Literal["pending"], "reason": InstructionPendingReason},
)
InstructionSlotStateBlocked = TypedDict(
    "InstructionSlotStateBlocked",
    {"state": Literal["blocked"], "reason": InstructionBlockReason},
)
InstructionSlotStateNotTargeted = TypedDict(
    "InstructionSlotStateNotTargeted", {"state": Literal["not_targeted"]}
)
InstructionSlotState = (
    InstructionSlotStateEffective
    | InstructionSlotStatePending
    | InstructionSlotStateBlocked
    | InstructionSlotStateNotTargeted
)

# One desired record of a mob.
InstructionRecord = TypedDict(
    "InstructionRecord",
    {
        "namespace": str,
        "key": str,
        "revision": int,
        "effect": InstructionEffect,
        "target": InstructionTarget,
        "content_sha256": NotRequired[str],
        "body_bytes": NotRequired[int],
    },
)

# One slot of one mob member.
MemberInstructionSlot = TypedDict(
    "MemberInstructionSlot",
    {
        "namespace": str,
        "key": str,
        "desired": InstructionSlotView,
        "recorded": NotRequired[InstructionSlotView],
        "effective": NotRequired[InstructionEffectiveSlot],
        "state": InstructionSlotState,
    },
)

MemberInstructionDeliveryCurrent = TypedDict(
    "MemberInstructionDeliveryCurrent", {"state": Literal["current"]}
)
MemberInstructionDeliveryOwed = TypedDict(
    "MemberInstructionDeliveryOwed",
    {
        "state": Literal["owed"],
        "generation": int,
        "last_failure": NotRequired[str],
    },
)
MemberInstructionDeliveryBlocked = TypedDict(
    "MemberInstructionDeliveryBlocked",
    {"state": Literal["blocked"], "reason": InstructionBlockReason},
)
MemberInstructionDeliveryState = (
    MemberInstructionDeliveryCurrent
    | MemberInstructionDeliveryOwed
    | MemberInstructionDeliveryBlocked
)

MemberInstructionDelivery = TypedDict(
    "MemberInstructionDelivery",
    {
        "recorded_generation": NotRequired[int],
        "state": MemberInstructionDeliveryState,
    },
)

InstructionEffectiveReadOk = TypedDict(
    "InstructionEffectiveReadOk", {"status": Literal["ok"]}
)
InstructionEffectiveReadUnreadable = TypedDict(
    "InstructionEffectiveReadUnreadable",
    {"status": Literal["unreadable"], "reason": str},
)
InstructionEffectiveRead = InstructionEffectiveReadOk | InstructionEffectiveReadUnreadable

# One member's operator-instruction status.
MemberInstructionStatus = TypedDict(
    "MemberInstructionStatus",
    {
        "agent_identity": str,
        "profile": str,
        "residency": InstructionResidency,
        "delivery": MemberInstructionDelivery,
        "slots": list[MemberInstructionSlot],
        "effective_read": InstructionEffectiveRead,
        # Operator instructions apply to the member's turns, not to a live
        # channel.
        "live_channel_open": bool,
    },
)

# The result of `mob/instruction_status`: the mob's records and one page of
# members, read from committed state without waking a member.
MobInstructionStatus = TypedDict(
    "MobInstructionStatus",
    {
        "mob_id": str,
        "generation": int,
        "desired": list[InstructionRecord],
        "members": list[MemberInstructionStatus],
        "next_offset": NotRequired[int],
    },
)

InstructionTargetOwnerUnowned = TypedDict(
    "InstructionTargetOwnerUnowned", {"kind": Literal["unowned"]}
)
InstructionTargetOwnerSession = TypedDict(
    "InstructionTargetOwnerSession", {"kind": Literal["session"]}
)
InstructionTargetOwnerMob = TypedDict(
    "InstructionTargetOwnerMob",
    {"kind": Literal["mob"], "mob_id": str, "agent_identity": str},
)
InstructionTargetOwner = (
    InstructionTargetOwnerUnowned
    | InstructionTargetOwnerSession
    | InstructionTargetOwnerMob
)

# One slot of a session.
SessionInstructionSlot = TypedDict(
    "SessionInstructionSlot",
    {
        "namespace": str,
        "key": str,
        "recorded": NotRequired[InstructionSlotView],
        "effective": NotRequired[InstructionEffectiveSlot],
        "state": InstructionSlotState,
    },
)

# The result of `session/instruction_status`: the session's committed state,
# read without materializing it.
SessionInstructionStatus = TypedDict(
    "SessionInstructionStatus",
    {
        "session_id": str,
        "owner": InstructionTargetOwner,
        "target_generation": int,
        "entry_sequence": int,
        "lowering": NotRequired[InstructionLowering],
        "slots": list[SessionInstructionSlot],
    },
)


# ---------------------------------------------------------------------------
# Fail-closed parsers
# ---------------------------------------------------------------------------


def _invalid(context: str, message: str) -> MeerkatError:
    return MeerkatError("INVALID_RESPONSE", f"{context}: {message}")


def _object(value: Any, context: str) -> dict[str, Any]:
    if not isinstance(value, dict):
        raise _invalid(context, "expected an object")
    return value


def _string(data: dict[str, Any], key: str, context: str) -> str:
    value = data.get(key)
    if not isinstance(value, str):
        raise _invalid(context, f"{key} must be a string")
    return value


def _count(data: dict[str, Any], key: str, context: str) -> int:
    value = data.get(key)
    if not isinstance(value, int) or isinstance(value, bool) or value < 0:
        raise _invalid(context, f"{key} must be a non-negative integer")
    return value


def _closed(value: Any, known: frozenset[str], field: str, context: str) -> Any:
    if not isinstance(value, str) or value not in known:
        raise _invalid(context, f"unsupported {field} {value!r}")
    return value


def _tolerant(value: Any, known: frozenset[str], field: str, context: str) -> Any:
    if not isinstance(value, str):
        raise _invalid(context, f"{field} must be a string")
    return value if value in known else {"unknown": value}


def _list(data: dict[str, Any], key: str, context: str) -> list[Any]:
    value = data.get(key)
    if not isinstance(value, list):
        raise _invalid(context, f"{key} must be a list")
    return value


def _slot_view(value: Any, context: str) -> InstructionSlotView:
    data = _object(value, context)
    return {
        "revision": _count(data, "revision", context),
        "effect": _closed(data.get("effect"), _EFFECTS, "effect", context),
    }


def _effective_slot(value: Any, context: str) -> InstructionEffectiveSlot:
    data = _object(value, context)
    slot: dict[str, Any] = {
        "revision": _count(data, "revision", context),
        "effect": _closed(data.get("effect"), _EFFECTS, "effect", context),
        "entry_sequence": _count(data, "entry_sequence", context),
        "kind": _closed(data.get("kind"), _ENTRY_KINDS, "kind", context),
        "message_index": _count(data, "message_index", context),
    }
    if data.get("run_id") is not None:
        slot["run_id"] = _string(data, "run_id", context)
    return cast(InstructionEffectiveSlot, slot)


def _slot_state(value: Any, context: str) -> InstructionSlotState:
    data = _object(value, context)
    state = data.get("state")
    if state in ("effective", "not_targeted"):
        return cast(InstructionSlotState, {"state": state})
    if state == "pending":
        return {
            "state": "pending",
            "reason": _tolerant(
                data.get("reason"), _PENDING_REASONS, "pending reason", context
            ),
        }
    if state == "blocked":
        return {
            "state": "blocked",
            "reason": _tolerant(
                data.get("reason"), _BLOCK_REASONS, "block reason", context
            ),
        }
    raise _invalid(context, f"unsupported slot state {state!r}")


def _optional_views(
    data: dict[str, Any], out: dict[str, Any], context: str
) -> None:
    if data.get("recorded") is not None:
        out["recorded"] = _slot_view(data["recorded"], f"{context}: recorded")
    if data.get("effective") is not None:
        out["effective"] = _effective_slot(data["effective"], f"{context}: effective")


def _target(value: Any, context: str) -> InstructionTarget:
    data = _object(value, context)
    kind = data.get("kind")
    if kind == "mob":
        return {"kind": "mob"}
    if kind == "profile":
        return {"kind": "profile", "profile": _string(data, "profile", context)}
    if kind == "member":
        return {
            "kind": "member",
            "agent_identity": _string(data, "agent_identity", context),
        }
    raise _invalid(context, f"unsupported target kind {kind!r}")


def parse_mob_instruction_write_result(
    value: Any, context: str
) -> MobInstructionWriteResult:
    """Check a `mob/set_instruction` or `mob/revoke_instruction` result."""
    data = _object(value, context)
    return {
        "mob_id": _string(data, "mob_id", context),
        "namespace": _string(data, "namespace", context),
        "key": _string(data, "key", context),
        "revision": _count(data, "revision", context),
        "disposition": _closed(
            data.get("disposition"), _MOB_DISPOSITIONS, "disposition", context
        ),
        "generation": _count(data, "generation", context),
        "matched_members": _count(data, "matched_members", context),
    }


def parse_session_instruction_write_result(
    value: Any, context: str
) -> SessionInstructionWriteResult:
    """Check a `session/set_instruction` or `session/revoke_instruction`
    result."""
    data = _object(value, context)
    return {
        "session_id": _string(data, "session_id", context),
        "namespace": _string(data, "namespace", context),
        "key": _string(data, "key", context),
        "revision": _count(data, "revision", context),
        "disposition": _closed(
            data.get("disposition"), _SESSION_DISPOSITIONS, "disposition", context
        ),
        "target_generation": _count(data, "target_generation", context),
    }


def _record(value: Any, context: str) -> InstructionRecord:
    data = _object(value, context)
    record: dict[str, Any] = {
        "namespace": _string(data, "namespace", context),
        "key": _string(data, "key", context),
        "revision": _count(data, "revision", context),
        "effect": _closed(data.get("effect"), _EFFECTS, "effect", context),
        "target": _target(data.get("target"), f"{context}: target"),
    }
    if data.get("content_sha256") is not None:
        record["content_sha256"] = _string(data, "content_sha256", context)
    if data.get("body_bytes") is not None:
        record["body_bytes"] = _count(data, "body_bytes", context)
    return cast(InstructionRecord, record)


def _delivery_state(value: Any, context: str) -> MemberInstructionDeliveryState:
    data = _object(value, context)
    state = data.get("state")
    if state == "current":
        return {"state": "current"}
    if state == "owed":
        owed: dict[str, Any] = {
            "state": "owed",
            "generation": _count(data, "generation", context),
        }
        if data.get("last_failure") is not None:
            owed["last_failure"] = _string(data, "last_failure", context)
        return cast(MemberInstructionDeliveryState, owed)
    if state == "blocked":
        return {
            "state": "blocked",
            "reason": _tolerant(
                data.get("reason"), _BLOCK_REASONS, "block reason", context
            ),
        }
    raise _invalid(context, f"unsupported delivery state {state!r}")


def _effective_read(value: Any, context: str) -> InstructionEffectiveRead:
    data = _object(value, context)
    status = data.get("status")
    if status == "ok":
        return {"status": "ok"}
    if status == "unreadable":
        return {"status": "unreadable", "reason": _string(data, "reason", context)}
    raise _invalid(context, f"unsupported effective_read status {status!r}")


def _member(value: Any, context: str) -> MemberInstructionStatus:
    data = _object(value, context)
    delivery = _object(data.get("delivery"), f"{context}: delivery")
    parsed_delivery: dict[str, Any] = {
        "state": _delivery_state(delivery.get("state"), f"{context}: delivery.state"),
    }
    if delivery.get("recorded_generation") is not None:
        parsed_delivery["recorded_generation"] = _count(
            delivery, "recorded_generation", f"{context}: delivery"
        )
    slots: list[MemberInstructionSlot] = []
    for index, raw in enumerate(_list(data, "slots", context)):
        slot_context = f"{context}: slots[{index}]"
        slot_data = _object(raw, slot_context)
        slot: dict[str, Any] = {
            "namespace": _string(slot_data, "namespace", slot_context),
            "key": _string(slot_data, "key", slot_context),
            "desired": _slot_view(slot_data.get("desired"), f"{slot_context}: desired"),
        }
        _optional_views(slot_data, slot, slot_context)
        slot["state"] = _slot_state(slot_data.get("state"), f"{slot_context}: state")
        slots.append(cast(MemberInstructionSlot, slot))
    live = data.get("live_channel_open")
    if not isinstance(live, bool):
        raise _invalid(context, "live_channel_open must be a boolean")
    return {
        "agent_identity": _string(data, "agent_identity", context),
        "profile": _string(data, "profile", context),
        "residency": _tolerant(
            data.get("residency"), _RESIDENCIES, "residency", context
        ),
        "delivery": cast(MemberInstructionDelivery, parsed_delivery),
        "slots": slots,
        "effective_read": _effective_read(
            data.get("effective_read"), f"{context}: effective_read"
        ),
        "live_channel_open": live,
    }


def parse_mob_instruction_status(value: Any, context: str) -> MobInstructionStatus:
    """Check a `mob/instruction_status` result."""
    data = _object(value, context)
    status: dict[str, Any] = {
        "mob_id": _string(data, "mob_id", context),
        "generation": _count(data, "generation", context),
        "desired": [
            _record(raw, f"{context}: desired[{index}]")
            for index, raw in enumerate(_list(data, "desired", context))
        ],
        "members": [
            _member(raw, f"{context}: members[{index}]")
            for index, raw in enumerate(_list(data, "members", context))
        ],
    }
    if data.get("next_offset") is not None:
        status["next_offset"] = _count(data, "next_offset", context)
    return cast(MobInstructionStatus, status)


def _owner(value: Any, context: str) -> InstructionTargetOwner:
    data = _object(value, context)
    kind = data.get("kind")
    if kind in ("unowned", "session"):
        return cast(InstructionTargetOwner, {"kind": kind})
    if kind == "mob":
        return {
            "kind": "mob",
            "mob_id": _string(data, "mob_id", context),
            "agent_identity": _string(data, "agent_identity", context),
        }
    raise _invalid(context, f"unsupported owner kind {kind!r}")


def parse_session_instruction_status(
    value: Any, context: str
) -> SessionInstructionStatus:
    """Check a `session/instruction_status` result."""
    data = _object(value, context)
    slots: list[SessionInstructionSlot] = []
    for index, raw in enumerate(_list(data, "slots", context)):
        slot_context = f"{context}: slots[{index}]"
        slot_data = _object(raw, slot_context)
        slot: dict[str, Any] = {
            "namespace": _string(slot_data, "namespace", slot_context),
            "key": _string(slot_data, "key", slot_context),
        }
        _optional_views(slot_data, slot, slot_context)
        slot["state"] = _slot_state(slot_data.get("state"), f"{slot_context}: state")
        slots.append(cast(SessionInstructionSlot, slot))
    status: dict[str, Any] = {
        "session_id": _string(data, "session_id", context),
        "owner": _owner(data.get("owner"), f"{context}: owner"),
        "target_generation": _count(data, "target_generation", context),
        "entry_sequence": _count(data, "entry_sequence", context),
    }
    if data.get("lowering") is not None:
        status["lowering"] = _tolerant(
            data.get("lowering"), _LOWERINGS, "lowering", context
        )
    status["slots"] = slots
    return cast(SessionInstructionStatus, status)
