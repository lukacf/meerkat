"""Trusted host G commissioning over the existing private listing owner.

This transports actual host selections, not grants or inferred workflow groups.
The caller durably owns the admitting application row before commissioning and
publishes it only in the async callback. Prepared does not submit G or establish
installation/use/continuation. Cold commissioning is not provided by this wire.
"""
from __future__ import annotations

import asyncio
import dataclasses
from dataclasses import dataclass
from typing import Any, Awaitable, Callable

from .host_control import (
    HostControlError, HostReplyCode, HostTransportError, PrivateAuthenticatedEvent,
    PrivateOriginalSubmit, PrivateOriginalRef, PrivateSourceEvidence, PrivateHostClient,
    WorkflowWorkItemRef, WorkflowWorkOwnerKey, _fields, _original_ref,
    _source_identifier, _workflow_token,
)
from .member_classes import (
    MemberClassAddAttempt, MemberClassClient, MemberClassObservationUnknown,
    _member_observation, _object,
)


@dataclass(frozen=True)
class WorkflowCommissionRequest:
    admission_id: str
    identity: str
    parent: PrivateOriginalSubmit
    instance: dict[str, Any]
    event: PrivateAuthenticatedEvent
    content: str
    workflow: str
    parent_work: WorkflowWorkItemRef
    parent_owner: WorkflowWorkOwnerKey
    cycle_label: str
    not_before_ms: int
    expires_at_ms: int
    deadline_ms: int
    max_waiters: int
    hold_timeout_ms: int
    controller_issuance_id: str
    planning_root_issuance_id: str
    planning_leaf_issuance_id: str

    def wire(self) -> dict[str, Any]:
        if (not isinstance(self.parent, PrivateOriginalSubmit)
                or not isinstance(self.parent.original, PrivateOriginalRef)
                or not isinstance(self.parent.transport_authentication, PrivateSourceEvidence)
                or not isinstance(self.parent.content, str)
                or not isinstance(self.event, PrivateAuthenticatedEvent) or not isinstance(self.content, str)):
            raise HostControlError("invalid workflow commissioning source")
        row = dataclasses.asdict(self)
        for key in ("admission_id", "identity", "workflow", "controller_issuance_id",
                    "planning_root_issuance_id", "planning_leaf_issuance_id"):
            _source_identifier(row[key])
        for key in ("not_before_ms", "expires_at_ms", "deadline_ms", "max_waiters", "hold_timeout_ms"):
            limit = 2**16 - 1 if key == "max_waiters" else 2**64 - 1
            if type(row[key]) is not int or not 0 <= row[key] <= limit:
                raise HostControlError("invalid workflow commissioning bound")
        instance = _object(row["instance"], {"class", "member"})
        _source_identifier(instance["class"])
        selected = _object(instance["member"], {"member", "logical_owner", "context_generation", "audience"})
        for value in _object(selected["member"], {"mob_id", "member", "role"}).values():
            _source_identifier(value)
        if type(selected["context_generation"]) is not int or not 0 <= selected["context_generation"] <= 2**64 - 1:
            raise HostControlError("invalid workflow generation")
        for value in _fields(row["parent_work"], {"realm_id", "namespace", "item_id"}).values():
            _workflow_token(value)
        owner = _fields(row["parent_owner"], {"kind", "id"})
        if owner["kind"] not in {"principal", "agent", "session", "mob", "label"}:
            raise HostControlError("invalid workflow parent owner")
        _workflow_token(owner["id"]); _workflow_token(row["cycle_label"])
        return row


def _control(value: Any) -> None:
    if isinstance(value, dict) and value.get("state") == "succeeded":
        _fields(value, {"state"})
    else:
        row = _fields(value, {"state", "error"})
        if row["state"] != "refused":
            raise HostControlError("invalid workflow cleanup outcome")
        HostReplyCode(row["error"])


def _response(value: Any, request: dict[str, Any], correlation: int) -> dict[str, Any]:
    if not isinstance(value, dict):
        raise HostControlError("invalid workflow commissioning response")
    state = value.get("state")
    keys = {
        "prepared": {"state", "parent", "planning", "admission_id", "native"},
        "created_without_planning": {"state", "parent", "planning", "admission_id", "native", "error"},
        "native": {"state", "outcome"},
        "add_not_accepted": {"state", "native", "retirement"},
        "preparation_refused": {"state", "error", "retirement", "physical_cleanup"},
    }
    if state not in keys:
        raise HostControlError("unknown workflow commissioning outcome")
    _fields(value, keys[state])
    if state in {"prepared", "created_without_planning"}:
        parent = _original_ref(value["parent"])
        planning = _original_ref(value["planning"])
        if (dataclasses.asdict(parent) != request["parent"]["original"]
                or value["admission_id"] != request["admission_id"]
                or planning.route != request["event"]["route"]
                or planning.original != request["event"]["original"]):
            raise HostControlError("workflow commissioning names another selection")
    for key in ("native", "outcome"):
        if key in value:
            _member_observation(value[key], request, correlation,
                                {"reservation_not_accepted", "preflight_refused", "settled", "native_refused"})
    if state in {"prepared", "created_without_planning"}:
        observed = value["native"]
        if (observed["state"] != "settled" or observed["native"]["state"] != "result"
                or observed["authorization"]["state"] != "succeeded" or observed["cleanup"]["state"] != "succeeded"):
            raise HostControlError("workflow prepared without successful native settlement")
    if "error" in value:
        HostReplyCode(value["error"])
    if "retirement" in value:
        _control(value["retirement"])
    if value.get("physical_cleanup") is not None:
        _control(value["physical_cleanup"])
    return value


class WorkflowCommissionAttempt(MemberClassAddAttempt):
    """One commission with the existing retained listing callback and no replay.

    result() observes the distinct native/planning outcome. join_listing() joins
    actual entered publication plus its ACK; it never establishes native success.
    Timeout/pipe loss use MemberClassObservationUnknown with this exact attempt.
    """
    def _await_result(self) -> dict[str, Any]:
        client = self._client._client
        reply = client._await_call(self.operation_id, self._deadline,
                                   "workflow_commissioned", {"kind", "outcome"})
        try:
            return _response(reply["outcome"], self._operation, self.operation_id)
        except (KeyError, TypeError, ValueError, HostControlError) as error:
            client._fail(error)
            raise HostTransportError("workflow commissioning response rejected; settlement is unknown") from error


def begin_workflow_commission(client: PrivateHostClient, request: WorkflowCommissionRequest, *,
                              publish: Callable[[], Awaitable[None]], timeout: float | None = None) -> WorkflowCommissionAttempt:
    if not isinstance(request, WorkflowCommissionRequest) or not callable(publish):
        raise HostControlError("workflow request and async publication callback are required")
    try:
        loop = asyncio.get_running_loop()
    except RuntimeError:
        raise HostControlError("application publication requires its running event loop") from None
    wire = request.wire()
    selected = {"admission_id": wire["admission_id"], "identity": wire["identity"], "class": wire["instance"]["class"]}
    attempt = None
    def published(identifier: int, deadline: float) -> None:
        nonlocal attempt
        attempt = WorkflowCommissionAttempt(MemberClassClient(client), wire, identifier, deadline,
                                            publish, loop, listing_selection=selected)
        client._member_listings[identifier] = attempt
    try:
        client._start_call({"kind": "commission_workflow", "request": wire}, timeout=timeout,
                           _publish=published, _member_listing=True)
    except HostTransportError as error:
        if attempt is None:
            raise
        attempt._error = MemberClassObservationUnknown(attempt)
        attempt._observed = True
        raise attempt._error from error
    assert attempt is not None
    return attempt
