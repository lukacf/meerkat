"""The typed mob destroy report: ``mob_destroy_incomplete_report``.

The report is ``WireMobDestroyReport`` in artifacts/schemas/wire-types.json,
emitted from the Rust contract (crates/meerkat-contracts/src/wire/mob.rs).
The generated dataclass has no fail-closed ``from_wire`` parser; the accessor
parses the report itself, and a refusal kind it does not know reads as the
server's own ``Unknown`` form instead of failing the parse.
"""

from __future__ import annotations

import asyncio
import dataclasses
import json
from pathlib import Path
from typing import Any

import pytest

from meerkat.errors import MeerkatError
from meerkat.streaming import _StdoutDispatcher

_WIRE_TYPES = (
    Path(__file__).resolve().parents[3] / "artifacts" / "schemas" / "wire-types.json"
)
_SCHEMA = json.loads(_WIRE_TYPES.read_text(encoding="utf-8"))["WireMobDestroyReport"]

# Every field set, as a server reports it.
_FULL_REPORT: dict[str, Any] = {
    "force_destroyed_members": ["forced"],
    "orphaned_remote_members": ["orphaned"],
    "remote_cleanup_deadline_exceeded": True,
    "metadata_scrubbed": True,
    "events_cleared": True,
    "namespace_cleaned": True,
    "errors": ["refused: the runtime refused the reload"],
    "reloaded_members": ["reloaded"],
    "member_reload_refusals": [
        {
            "member": "refused",
            "refusal": {
                "kind": "repair_blocked",
                "reason": "commit outcome not reported",
                "evidence_digest": "digest-1",
            },
        }
    ],
}


def _incomplete(report: Any) -> dict[str, Any]:
    return {"code": "mob_destroy_incomplete", "destroy_report": report, "retryable": True}


async def _rpc_error(data: dict[str, Any]) -> MeerkatError:
    """The error the client raises for a JSON-RPC error response carrying ``data``."""
    response = {
        "jsonrpc": "2.0",
        "id": 1,
        "error": {"code": -32603, "message": "mob destroy incomplete: refused", "data": data},
    }
    reader = asyncio.StreamReader()
    reader.feed_data((json.dumps(response) + "\n").encode())
    reader.feed_eof()
    dispatcher = _StdoutDispatcher(reader)
    dispatcher.start()
    try:
        with pytest.raises(MeerkatError) as exc_info:
            await asyncio.wait_for(dispatcher.expect_response(1), timeout=1.0)
    finally:
        await dispatcher.stop()
    return exc_info.value


@pytest.mark.asyncio
async def test_the_incomplete_destroy_report_survives_the_rpc_transport_typed() -> None:
    from meerkat import (
        WireMobDestroyMemberRefusal,
        WireMobDestroyReport,
        mob_destroy_incomplete_report,
    )

    error = await _rpc_error(_incomplete(_FULL_REPORT))

    assert error.code == "mob_destroy_incomplete"
    assert error.details == _incomplete(_FULL_REPORT), "details stay as sent"
    report = mob_destroy_incomplete_report(error)
    assert isinstance(report, WireMobDestroyReport)
    assert report.member_reload_refusals is not None
    assert report.member_reload_refusals[0].refusal["kind"] == "repair_blocked"
    assert report == WireMobDestroyReport(
        force_destroyed_members=["forced"],
        orphaned_remote_members=["orphaned"],
        remote_cleanup_deadline_exceeded=True,
        metadata_scrubbed=True,
        events_cleared=True,
        namespace_cleaned=True,
        errors=["refused: the runtime refused the reload"],
        reloaded_members=["reloaded"],
        member_reload_refusals=[
            WireMobDestroyMemberRefusal(
                member="refused",
                refusal={
                    "kind": "repair_blocked",
                    "reason": "commit outcome not reported",
                    "evidence_digest": "digest-1",
                },
            )
        ],
    )


def test_the_accessor_reads_every_field_the_schema_names() -> None:
    from meerkat import WireMobDestroyReport, mob_destroy_incomplete_report

    fields = {field.name for field in dataclasses.fields(WireMobDestroyReport)}
    assert fields == set(_SCHEMA["properties"])
    report = mob_destroy_incomplete_report(
        MeerkatError("mob_destroy_incomplete", "incomplete", _incomplete(_FULL_REPORT))
    )
    assert report is not None
    assert {name for name in fields if getattr(report, name) is None} == set()


@pytest.mark.asyncio
async def test_an_unknown_refusal_kind_reads_as_unknown_without_an_error() -> None:
    from meerkat import WireMobDestroyMemberRefusal, mob_destroy_incomplete_report

    error = await _rpc_error(
        _incomplete(
            {
                "errors": ["m: refused"],
                "member_reload_refusals": [
                    {"member": "m", "refusal": {"kind": "a_later_kind", "detail": 1}}
                ],
            }
        )
    )

    report = mob_destroy_incomplete_report(error)
    assert report is not None
    assert report.errors == ["m: refused"]
    assert report.member_reload_refusals == [
        WireMobDestroyMemberRefusal(member="m", refusal={"kind": "unknown"})
    ]


def test_absent_fields_stay_none() -> None:
    from meerkat import WireMobDestroyMemberRefusal, WireMobDestroyReport, mob_destroy_incomplete_report

    report = mob_destroy_incomplete_report(
        MeerkatError(
            "mob_destroy_incomplete",
            "incomplete",
            _incomplete(
                {
                    "metadata_scrubbed": False,
                    "member_reload_refusals": [
                        {"member": "m", "refusal": {"kind": "repair_blocked", "reason": "r"}},
                        {
                            "member": "n",
                            "refusal": {"kind": "repair_blocked", "reason": "r", "evidence_digest": None},
                        },
                    ],
                }
            ),
        )
    )

    assert report == WireMobDestroyReport(
        metadata_scrubbed=False,
        member_reload_refusals=[
            WireMobDestroyMemberRefusal(member="m", refusal={"kind": "repair_blocked", "reason": "r"}),
            WireMobDestroyMemberRefusal(member="n", refusal={"kind": "repair_blocked", "reason": "r"}),
        ],
    )


def _refusal(value: Any) -> dict[str, Any]:
    return _incomplete({"member_reload_refusals": [value]})


@pytest.mark.parametrize(
    "details",
    [
        None,
        "mob_destroy_incomplete",
        {"code": "send_failed", "destroy_report": _FULL_REPORT},
        {"code": "mob_destroy_incomplete"},
        _incomplete(None),
        _incomplete([]),
        _incomplete({"errors": "x"}),
        _incomplete({"errors": [1]}),
        _incomplete({"reloaded_members": None}),
        _incomplete({"metadata_scrubbed": "yes"}),
        _incomplete({"member_reload_refusals": {}}),
        _refusal({"refusal": {"kind": "repair_blocked", "reason": "r"}}),
        _refusal({"member": "m"}),
        _refusal({"member": "m", "refusal": {"reason": "r"}}),
        _refusal({"member": "m", "refusal": {"kind": "repair_blocked"}}),
        _refusal({"member": "m", "refusal": {"kind": "repair_blocked", "reason": "r", "evidence_digest": 1}}),
    ],
    ids=lambda value: json.dumps(value)[:80],
)
def test_the_accessor_is_none_for_another_code_or_a_report_that_does_not_parse(
    details: Any,
) -> None:
    from meerkat import mob_destroy_incomplete_report

    assert (
        mob_destroy_incomplete_report(MeerkatError("mob_destroy_incomplete", "x", details))
        is None
    )


def test_the_accessor_is_none_for_other_exceptions() -> None:
    from meerkat import mob_destroy_incomplete_report

    assert mob_destroy_incomplete_report(ValueError("not a meerkat error")) is None
