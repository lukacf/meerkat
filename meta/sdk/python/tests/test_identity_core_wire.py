"""#306 (0.9.4): identity status and refusals read Core's authority.

Core's mob machine and journal hold an identity's session and generation, so
the gateway reports Core's answers: three typed refusal codes, the held
wakes an operator must clear, a view state of ``active``, ``dormant`` or
``broken``, the refused wake that explains it, and Core's convergence status
for an identity a host intent declaration manages. These tests pin the SDK
side of that wire.
"""

from __future__ import annotations

import dataclasses
from typing import Any

import pytest

import meerkat_meta
from meerkat_meta.errors import (
    IDENTITY_HISTORY_UNAVAILABLE_CODE,
    IDENTITY_HOST_ADOPTED_CODE,
    IDENTITY_UNRESUMABLE_CODE,
    IDENTITY_WAKE_HELD_KINDS,
    STORAGE_RESOLUTION_CODE,
    IdentityHistoryUnavailableError,
    IdentityHostAdoptedError,
    IdentityUnresumableError,
    IdentityWakeHeldError,
    RpcError,
    StorageResolutionError,
)
from meerkat_meta.identity_first_models import (
    DeferredIdentity,
    FirstBootPendingRow,
    IdentityBootstrapStatus,
    IdentityConvergence,
    IdentityInspection,
    IdentityRestoreProgress,
    IdentityStatus,
    ReplacedSession,
    WakeRefusal,
)
from meerkat_meta.runtime import _rpc_error_from_payload
from meerkat_meta.types import MemberHealth

CORE_CONVERGENCE = {
    "agent_identity": "triage:main",
    "desired_intent_revision": 4,
    "active_intent_revision": 3,
    "decision": "ensure_member_materialization",
    "condition": "reconciling",
    "observed_at_ms": 1_760_000_000_000,
    "detail": None,
}


def _refusal(code: int, data: dict | None) -> RpcError:
    payload = {"code": code, "message": "refused"}
    if data is not None:
        payload["data"] = data
    return _rpc_error_from_payload(payload, request_id="meta/send:1", method="meta/send")


def test_identity_codes_are_the_core_contract_codes() -> None:
    assert IDENTITY_UNRESUMABLE_CODE == -32051
    assert IDENTITY_HISTORY_UNAVAILABLE_CODE == -32052
    assert IDENTITY_HOST_ADOPTED_CODE == -32053
    for name in (
        "IDENTITY_UNRESUMABLE_CODE",
        "IDENTITY_HISTORY_UNAVAILABLE_CODE",
        "IDENTITY_HOST_ADOPTED_CODE",
        "IdentityUnresumableError",
        "IdentityHistoryUnavailableError",
        "IdentityHostAdoptedError",
        "IdentityWakeHeldError",
        "IDENTITY_WAKE_HELD_KINDS",
    ):
        assert name in meerkat_meta.__all__
        assert getattr(meerkat_meta, name) is not None


def test_unresumable_refusal_is_typed_and_final() -> None:
    err = _refusal(
        -32051,
        {
            "kind": "identity_unresumable",
            "identity": "triage:main",
            "session_id": "s-1",
            "reason": "archived_not_revivable",
            "remedy": "delete_identity",
            "retryable": False,
        },
    )
    assert type(err) is IdentityUnresumableError
    assert isinstance(err, RpcError)
    assert err.code == -32051
    assert err.retryable is False
    assert (err.identity, err.session_id, err.reason, err.remedy) == (
        "triage:main",
        "s-1",
        "archived_not_revivable",
        "delete_identity",
    )
    assert err.method == "meta/send"


@pytest.mark.parametrize(
    ("data", "kind", "session_id"),
    [
        (
            {"kind": "identity_history_unavailable", "identity": "w", "retryable": True},
            "identity_history_unavailable",
            None,
        ),
        (
            {
                "kind": "identity_wake_retryable",
                "identity": "w",
                "session_id": "s-2",
                "retryable": True,
            },
            "identity_wake_retryable",
            "s-2",
        ),
        (
            {
                "kind": "identity_wake_retryable",
                "identity": "w",
                "session_id": None,
                "retryable": True,
            },
            "identity_wake_retryable",
            None,
        ),
        (
            {
                "kind": "identity_session_absent",
                "identity": "w",
                "session_id": "s-3",
                "retryable": True,
            },
            "identity_session_absent",
            "s-3",
        ),
    ],
)
def test_every_retryable_kind_maps_to_one_typed_error(
    data: dict, kind: str, session_id: str | None
) -> None:
    err = _refusal(-32052, data)
    assert type(err) is IdentityHistoryUnavailableError
    assert err.retryable is True
    assert (err.kind, err.identity, err.session_id) == (kind, "w", session_id)


def test_a_roster_wide_history_failure_names_no_identity() -> None:
    # Reading the whole roster's history failed; no one identity is named.
    err = _refusal(-32052, {"kind": "identity_history_unavailable", "retryable": True})
    assert type(err) is IdentityHistoryUnavailableError
    assert err.retryable is True
    assert (err.kind, err.identity, err.session_id) == (
        "identity_history_unavailable",
        None,
        None,
    )


def test_host_adopted_refusal_is_typed() -> None:
    err = _refusal(
        -32053, {"kind": "identity_host_adopted", "identity": "w", "retryable": False}
    )
    assert type(err) is IdentityHostAdoptedError
    assert err.retryable is False
    assert err.identity == "w"


@pytest.mark.parametrize("code", [-32051, -32052, -32053])
def test_identity_refusals_without_data_stay_typed(code: int) -> None:
    err = _refusal(code, None)
    assert err.code == code
    assert err.data is None
    assert err.identity is None


def test_broken_status_carries_its_final_wake_refusal() -> None:
    wire = {
        "identity": "triage:main",
        "state": "broken",
        "agent_runtime_id": None,
        "session_id": "s-1",
        "generation": 3,
        "profile": "triage",
        "addressability": "addressable",
        "labels": {},
        "intent_managed": False,
        "wake_refusal": {"code": -32051, "session_id": "s-1", "remedy": "delete_identity"},
    }
    status = IdentityStatus.from_dict(wire)
    assert status.state == "broken"
    assert status.agent_runtime_id is None
    assert (status.session_id, status.generation) == ("s-1", 3)
    assert status.intent_managed is False
    assert status.convergence is None
    assert status.wake_refusal == WakeRefusal(
        code=-32051, session_id="s-1", remedy="delete_identity"
    )
    assert status.to_dict()["wake_refusal"] == wire["wake_refusal"]
    assert status.to_dict()["intent_managed"] is False


def test_intent_managed_status_carries_cores_convergence_unchanged() -> None:
    status = IdentityStatus.from_dict(
        {
            "identity": "triage:main",
            "state": "active",
            "agent_runtime_id": "rt-1",
            "session_id": "s-1",
            "generation": 7,
            "intent_managed": True,
            "convergence": dict(CORE_CONVERGENCE),
            "wake_refusal": None,
        }
    )
    assert status.intent_managed is True
    assert status.wake_refusal is None
    assert status.convergence == IdentityConvergence(
        agent_identity="triage:main",
        desired_intent_revision=4,
        active_intent_revision=3,
        decision="ensure_member_materialization",
        condition="reconciling",
        observed_at_ms=1_760_000_000_000,
        detail=None,
    )
    assert status.to_dict()["convergence"] == CORE_CONVERGENCE


def test_absent_and_null_optional_keys_read_as_none() -> None:
    for wire in (
        {"identity": "w", "state": "dormant"},
        {
            "identity": "w",
            "state": "dormant",
            "agent_runtime_id": None,
            "session_id": None,
            "generation": None,
            "convergence": None,
            "wake_refusal": None,
        },
    ):
        status = IdentityStatus.from_dict(wire)
        assert (status.agent_runtime_id, status.session_id, status.generation) == (
            None,
            None,
            None,
        )
        assert (status.intent_managed, status.convergence, status.wake_refusal) == (
            False,
            None,
            None,
        )
        assert not {"convergence", "wake_refusal", "session_id", "generation"} & (
            status.to_dict().keys()
        )


def test_newer_remedy_and_state_values_are_kept_as_sent() -> None:
    status = IdentityStatus.from_dict(
        {
            "identity": "w",
            "state": "a_future_state",
            "wake_refusal": {"code": -32603, "session_id": "s", "remedy": "a_future_remedy"},
        }
    )
    assert status.state == "a_future_state"
    assert status.wake_refusal is not None
    assert (status.wake_refusal.code, status.wake_refusal.remedy) == (-32603, "a_future_remedy")


def test_status_drops_meta_minted_continuity_facts() -> None:
    removed = {
        "checkpoint_version",
        "continuity_unrecoverable",
        "session_repair_required",
    }
    assert not removed & {f.name for f in dataclasses.fields(IdentityStatus)}
    # An older gateway that still sends them is read without them.
    status = IdentityStatus.from_dict(
        {
            "identity": "w",
            "state": "active",
            "checkpoint_version": 5,
            "continuity_unrecoverable": {"reason": "x"},
            "session_repair_required": {"commands": []},
        }
    )
    assert not removed & status.to_dict().keys()


@pytest.mark.parametrize(
    ("data", "hold", "reason", "operator_path"),
    [
        (
            {
                "kind": "mob_member_session_repair_required",
                "durable_resume_hold": "audited_endpoint_divergence",
                "session_id": "s-1",
                "content_retained": True,
                "retryable": False,
                "operator_path": ["session repair-wholeblob", "meta/reload_member"],
            },
            "audited_endpoint_divergence",
            None,
            ["session repair-wholeblob", "meta/reload_member"],
        ),
        (
            {
                "kind": "mob_member_session_resume_held",
                "durable_resume_hold": "tail_held_for_recovery",
                "session_id": "s-1",
                "content_retained": True,
                "retryable": False,
            },
            "tail_held_for_recovery",
            None,
            [],
        ),
        (
            {
                "kind": "mob_member_session_resume_held",
                "resume_unavailable_reason": "committed_boundary_unprovable",
                "session_id": "s-1",
                "content_retained": True,
                "retryable": False,
            },
            None,
            "committed_boundary_unprovable",
            [],
        ),
        (
            {
                "kind": "identity_role_migration_refused",
                "identity": "w",
                "session_id": "s-1",
                "content_retained": True,
                "retryable": False,
            },
            None,
            None,
            [],
        ),
    ],
)
def test_held_wakes_are_typed(
    data: dict, hold: str | None, reason: str | None, operator_path: list[str]
) -> None:
    err = _refusal(-32603, data)
    assert type(err) is IdentityWakeHeldError
    assert err.code == -32603
    assert err.retryable is False
    assert (err.kind, err.session_id, err.durable_resume_hold) == (
        data["kind"],
        "s-1",
        hold,
    )
    assert err.resume_unavailable_reason == reason
    assert err.operator_path == operator_path
    assert data["kind"] in IDENTITY_WAKE_HELD_KINDS


@pytest.mark.parametrize(
    "data",
    [None, {"kind": "actor_terminated", "operation": "send"}, {"no_kind": True}],
)
def test_other_internal_errors_stay_plain(data: dict | None) -> None:
    err = _refusal(-32603, data)
    assert type(err) is RpcError


def test_an_owed_held_completion_stays_a_plain_rpc_error_with_its_data() -> None:
    # An identity a held pass did not publish owes its host a held completion.
    # The refusal rides -32603 with typed data: an admission still owed, not a
    # held wake, so it stays a plain RpcError and keeps its data.
    data = {"kind": "held_completion_owed", "identity": "w"}
    err = _refusal(-32603, data)
    assert type(err) is RpcError
    assert err.code == -32603
    assert err.data == data
    assert data["kind"] not in IDENTITY_WAKE_HELD_KINDS
    progress = IdentityRestoreProgress.from_dict(
        {"stage": "refused", "code": -32603, "kind": "held_completion_owed"}
    )
    assert (progress.stage, progress.code, progress.kind) == (
        "refused",
        -32603,
        "held_completion_owed",
    )


def test_status_reports_runtime_mode_and_pending_customizer_tools() -> None:
    pending = {"reason": "customize_build failed"}
    status = IdentityStatus.from_dict(
        {
            "identity": "w",
            "state": "active",
            "runtime_mode": "turn_driven",
            "customizer_tools_pending": pending,
        }
    )
    assert status.runtime_mode == "turn_driven"
    assert status.customizer_tools_pending == pending
    assert status.to_dict()["runtime_mode"] == "turn_driven"
    assert status.to_dict()["customizer_tools_pending"] == pending


def test_status_names_the_session_a_fresh_wake_replaced() -> None:
    wire = {
        "identity": "w",
        "state": "active",
        "agent_runtime_id": "rt-2",
        "session_id": "s-2",
        "generation": 1,
        "intent_managed": False,
        "replaced_session": {"session_id": "s-1", "reason": "absent"},
    }
    status = IdentityStatus.from_dict(wire)
    assert status.replaced_session == ReplacedSession(session_id="s-1", reason="absent")
    assert status.to_dict()["replaced_session"] == wire["replaced_session"]
    later = IdentityStatus.from_dict(
        {
            "identity": "w",
            "state": "active",
            "replaced_session": {"session_id": "s-1", "reason": "a_future_reason"},
        }
    )
    assert later.replaced_session is not None
    assert later.replaced_session.reason == "a_future_reason"
    assert IdentityStatus.from_dict({"identity": "w", "state": "active"}).replaced_session is None


def test_status_carries_cores_member_id_of_the_last_binding() -> None:
    wire = {
        "identity": "agent:w",
        "state": "active",
        "session_id": "s-2",
        "member_id": "mk--agent_cw",
        "intent_managed": False,
    }
    status = IdentityStatus.from_dict(wire)
    assert status.member_id == "mk--agent_cw"
    assert status.to_dict()["member_id"] == "mk--agent_cw"
    unbound = IdentityStatus.from_dict({"identity": "agent:w", "state": "dormant"})
    assert unbound.member_id is None
    assert "member_id" not in unbound.to_dict()


@pytest.mark.parametrize(
    "remedy", ["delete_identity", "repair_session", "resolve_role_migration", "operator_review"]
)
def test_every_documented_remedy_reads_as_sent(remedy: str) -> None:
    status = IdentityStatus.from_dict(
        {
            "identity": "w",
            "state": "dormant",
            "wake_refusal": {"code": -32603, "session_id": "s", "remedy": remedy},
        }
    )
    assert status.wake_refusal == WakeRefusal(code=-32603, session_id="s", remedy=remedy)


def test_inspection_carries_the_identity_view_atoms() -> None:
    wire = {
        "identity": "triage:main",
        "state": "dormant",
        "agent_runtime_id": None,
        "session_id": "s-1",
        "generation": 2,
        "intent_managed": True,
        "convergence": dict(CORE_CONVERGENCE),
        "wake_refusal": {"code": -32603, "session_id": "s-1", "remedy": "operator_review"},
        "replaced_session": {"session_id": "s-0", "reason": "absent"},
        "is_final": False,
        "peer_reachable_count": 0,
    }
    inspection = IdentityInspection.from_dict(wire)
    assert (inspection.agent_runtime_id, inspection.session_id) == (None, "s-1")
    assert inspection.intent_managed is True
    assert inspection.convergence is not None
    assert inspection.convergence.to_dict() == CORE_CONVERGENCE
    assert inspection.wake_refusal == WakeRefusal(
        code=-32603, session_id="s-1", remedy="operator_review"
    )
    assert inspection.replaced_session == ReplacedSession(session_id="s-0", reason="absent")
    out = inspection.to_dict()
    assert out["session_id"] == "s-1"
    assert "agent_runtime_id" not in out
    assert out["intent_managed"] is True
    for key in ("convergence", "wake_refusal", "replaced_session"):
        assert out[key] == wire[key]


def test_inspection_reads_an_older_frame_without_its_continuity_facts() -> None:
    removed = {"checkpoint_version", "continuity_health", "lease_healthy", "lease", "continuity"}
    assert not removed & {f.name for f in dataclasses.fields(IdentityInspection)}
    inspection = IdentityInspection.from_dict(
        {
            "identity": "w",
            "checkpoint_version": 4,
            "continuity_health": "healthy",
            "lease_healthy": True,
            "lease": {"holder": "h"},
            "continuity": {"session_id": "s-0"},
        }
    )
    assert (inspection.agent_runtime_id, inspection.session_id) == (None, None)
    assert inspection.intent_managed is False
    assert (inspection.convergence, inspection.wake_refusal, inspection.replaced_session) == (
        None,
        None,
        None,
    )
    assert not removed & inspection.to_dict().keys()


@pytest.mark.parametrize(
    ("wire", "code", "kind"),
    [
        (
            {"stage": "refused", "code": -32051, "kind": "identity_unresumable"},
            -32051,
            "identity_unresumable",
        ),
        (
            {"stage": "refused", "code": -32603, "kind": "mob_member_session_resume_held"},
            -32603,
            "mob_member_session_resume_held",
        ),
        ({"stage": "refused", "code": -32052}, -32052, None),
    ],
)
def test_restore_progress_reports_a_refused_restore(
    wire: dict, code: int, kind: str | None
) -> None:
    progress = IdentityRestoreProgress.from_dict(wire)
    assert (progress.stage, progress.code, progress.kind) == ("refused", code, kind)
    assert progress.to_dict() == wire


def test_restore_progress_keeps_other_stages_without_a_code() -> None:
    for stage in ("registered", "resuming", "minted", "resumed", "a_future_stage"):
        progress = IdentityRestoreProgress.from_dict({"stage": stage})
        assert (progress.stage, progress.code, progress.kind) == (stage, None, None)
        assert progress.to_dict() == {"stage": stage}


def test_member_health_carries_the_last_wake_refusal() -> None:
    health = MemberHealth.from_dict(
        {
            "identity": "w",
            "state": "dormant",
            "generation": 4,
            "wake_refusal": {"code": -32603, "session_id": "s-1", "remedy": "repair_session"},
            "session_repair_required": {"commands": ["repair"]},
        }
    )
    assert health.wake_refusal == WakeRefusal(
        code=-32603, session_id="s-1", remedy="repair_session"
    )
    assert health.generation == 4
    # An older frame still carrying the continuity-era park reads without it.
    older = MemberHealth.from_dict(
        {"identity": "w", "state": "active", "continuity_unrecoverable": {"reason": "x"}}
    )
    assert older.wake_refusal is None
    assert "continuity_unrecoverable" not in {f.name for f in dataclasses.fields(MemberHealth)}


FIRST_BOOT_ROWS = [
    {"identity": "triage:main", "kind": "resume", "waiting_for": "carry_out"},
    {"identity": "triage:aux", "kind": "fresh", "waiting_for": "roster"},
    {"identity": "triage:new", "kind": "a_future_kind", "waiting_for": "a_future_wait"},
]


def _bootstrap_wire(**extra: Any) -> dict[str, Any]:
    return {
        "mode": {"mode": "lazy_materialize"},
        "complete": False,
        "ready": False,
        "counts": {},
        "identities": {},
        **extra,
    }


def test_bootstrap_status_lists_the_pending_first_boot_rows() -> None:
    status = IdentityBootstrapStatus.from_dict(
        _bootstrap_wire(first_boot_pending=FIRST_BOOT_ROWS)
    )
    assert status.first_boot_pending == (
        FirstBootPendingRow("triage:main", "resume", "carry_out"),
        FirstBootPendingRow("triage:aux", "fresh", "roster"),
        # Newer kinds and waits are kept as sent.
        FirstBootPendingRow("triage:new", "a_future_kind", "a_future_wait"),
    )
    assert status.to_dict()["first_boot_pending"] == FIRST_BOOT_ROWS
    assert meerkat_meta.FirstBootPendingRow is FirstBootPendingRow


def test_bootstrap_status_without_pending_rows_has_none() -> None:
    status = IdentityBootstrapStatus.from_dict(_bootstrap_wire())
    assert status.first_boot_pending == ()
    # The gateway sends no key when no row is pending, and neither does to_dict.
    assert "first_boot_pending" not in status.to_dict()


DEFERRED = [
    {"identity": "agent:dormant", "reason": "resume_member"},
    {"identity": "agent:lead", "reason": "create_member"},
    {"identity": "agent:specialist", "reason": "move_role"},
    {"identity": "agent:next", "reason": "a_future_reason"},
]


def test_bootstrap_status_lists_a_candidates_deferred_identities() -> None:
    status = IdentityBootstrapStatus.from_dict(
        _bootstrap_wire(
            complete=True,
            counts={"active": 1, "broken": 0, "deferred": 4},
            deferred=DEFERRED,
        )
    )
    assert status.deferred == (
        DeferredIdentity("agent:dormant", "resume_member"),
        DeferredIdentity("agent:lead", "create_member"),
        DeferredIdentity("agent:specialist", "move_role"),
        # Newer reasons are kept as sent.
        DeferredIdentity("agent:next", "a_future_reason"),
    )
    assert (status.counts.deferred, status.counts.broken) == (4, 0)
    assert status.to_dict()["deferred"] == DEFERRED
    assert status.to_dict()["counts"]["deferred"] == 4
    assert meerkat_meta.DeferredIdentity is DeferredIdentity


def test_bootstrap_status_without_deferred_identities_has_none() -> None:
    status = IdentityBootstrapStatus.from_dict(_bootstrap_wire())
    assert (status.deferred, status.counts.deferred) == ((), 0)
    # The gateway omits both when nothing is deferred, and so does to_dict.
    assert "deferred" not in status.to_dict()
    assert "deferred" not in status.to_dict()["counts"]


@pytest.mark.parametrize(
    "row",
    [{"identity": "w"}, {"identity": "w", "reason": 1}, "w"],
)
def test_a_malformed_deferred_identity_is_refused(row: Any) -> None:
    with pytest.raises(TypeError):
        IdentityBootstrapStatus.from_dict(_bootstrap_wire(deferred=[row]))


@pytest.mark.parametrize(
    "row",
    [
        {"identity": "w", "kind": "resume"},
        {"identity": 1, "kind": "fresh", "waiting_for": "roster"},
        "w",
    ],
)
def test_a_malformed_first_boot_row_is_refused(row: Any) -> None:
    with pytest.raises(TypeError):
        IdentityBootstrapStatus.from_dict(_bootstrap_wire(first_boot_pending=[row]))


@pytest.mark.parametrize(
    ("data", "fields"),
    [
        (
            {
                "kind": "first_boot_manifest_unreadable",
                "path": "/state/storage-migrate-native.first-boot.json",
                "retryable": False,
            },
            (
                "first_boot_manifest_unreadable",
                None,
                None,
                None,
                None,
                None,
                "/state/storage-migrate-native.first-boot.json",
                None,
                None,
                False,
            ),
        ),
        (
            {
                "kind": "first_boot_row_refused",
                "identity": "w",
                "row_kind": "resume",
                "manifest_session": "s-1",
                "core_session": "s-2",
                "retryable": False,
            },
            ("first_boot_row_refused", "w", "resume", "s-1", "s-2", None, None, None, None, False),
        ),
        (
            {
                "kind": "first_boot_row_refused",
                "identity": "w",
                "row_kind": "fresh",
                "manifest_session": "s-1",
                "core_session": None,
                "retryable": False,
            },
            ("first_boot_row_refused", "w", "fresh", "s-1", None, None, None, None, None, False),
        ),
        (
            {
                "kind": "first_boot_row_pending",
                "identity": "w",
                "row_kind": "resume",
                "waiting_for": "roster",
                "operation": "delete_identity",
                "remedy": "add_to_roster",
                "retryable": True,
            },
            (
                "first_boot_row_pending",
                "w",
                "resume",
                None,
                None,
                "delete_identity",
                None,
                "roster",
                "add_to_roster",
                True,
            ),
        ),
        (
            {
                "kind": "first_boot_row_pending",
                "identity": "w",
                "row_kind": "fresh",
                "waiting_for": "carry_out",
                "operation": "reset",
                "remedy": "wake_identity",
                "retryable": True,
            },
            (
                "first_boot_row_pending",
                "w",
                "fresh",
                None,
                None,
                "reset",
                None,
                "carry_out",
                "wake_identity",
                True,
            ),
        ),
    ],
)
def test_first_boot_refusals_are_typed_storage_resolution_errors(
    data: dict[str, Any], fields: tuple[Any, ...]
) -> None:
    err = _refusal(STORAGE_RESOLUTION_CODE, data)
    assert type(err) is StorageResolutionError
    assert (
        err.kind,
        err.identity,
        err.row_kind,
        err.manifest_session,
        err.core_session,
        err.operation,
        err.path,
        err.waiting_for,
        err.remedy,
        err.retryable,
    ) == fields


def test_a_storage_refusal_without_data_is_not_retryable() -> None:
    err = _refusal(STORAGE_RESOLUTION_CODE, None)
    assert type(err) is StorageResolutionError
    assert (err.kind, err.identity, err.retryable) == (None, None, False)
