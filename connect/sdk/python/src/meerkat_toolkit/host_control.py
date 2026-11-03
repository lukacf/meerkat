"""Typed private lifecycle control for a locally owned Toolkit gateway process.

Observations are never request permissions. This channel assumes the launcher
and native child execute trusted code under the same local OS account. It does
not protect against arbitrary native code executing as that account.
"""
from __future__ import annotations

import dataclasses
import hashlib
import json
import math
import os
import select
import threading
import time
import unicodedata
from dataclasses import dataclass
from enum import StrEnum
from typing import Any, Callable, Literal, Protocol, TypedDict, cast
from uuid import UUID

from meerkat_meta._transport import PersistentTransport
from meerkat_meta.runtime import MetaRuntime

_PROTOCOL = 1
_MAX_FRAME = 1_048_576


class HostControlError(RuntimeError):
    """A local validation, native reply or transport error, never cleanup proof."""


class HostTransportError(HostControlError):
    """This shared client has failed, closed or been poisoned.

    Outstanding native effects and settlement may be unknown. Reusing this
    client cannot recover them, and this error must never trigger a replay.
    """


class HostReplyCode(StrEnum):
    """The closed WireError code set on the selected native private protocol."""
    SOURCE_CONFLICT = "source_conflict"
    INVALID_SELECTION = "invalid_selection"
    UNAVAILABLE = "unavailable"
    DENIED = "denied"
    WRONG_OWNER = "wrong_owner"
    NOT_SETTLED = "not_settled"


class HostReplyError(HostControlError):
    """An exact decoded native error reply, not a no-effect or cleanup promise.

    The reply itself does not close the shared client. Another concurrent
    event may still fail that client independently.
    """
    def __init__(self, code: HostReplyCode):
        self.code = HostReplyCode(code)
        super().__init__(f"native host refused: {self.code.value}")


@dataclass(frozen=True)
class SelectedHostArtifact:
    path: str
    sha256: str


@dataclass(frozen=True)
class HostBootstrapSelection:
    deployment_root: str
    activation: SelectedHostArtifact
    policy_manifest: SelectedHostArtifact
    local_policy: SelectedHostArtifact


@dataclass(frozen=True)
class ApplicationUpdateStartup:
    """One admitted transaction's immutable replacement-process selection.

    The prior policy is the transaction's actual last accepted desired policy,
    including after restart. The existing private output directory holds only
    native preparation/visibility observations, never finish or retry receipts.
    """
    selection: SelectedHostArtifact
    prior_tool_policy: SelectedHostArtifact
    association_directory: str

    def __post_init__(self) -> None:
        for pin in (self.selection, self.prior_tool_policy):
            if not isinstance(pin, SelectedHostArtifact) or not isinstance(pin.path, str) or not os.path.isabs(pin.path) or "\0" in pin.path:
                raise HostControlError("application update requires absolute selected artifacts")
            _source_digest(pin.sha256)
        if not isinstance(self.association_directory, str) or not os.path.isabs(self.association_directory) or "\0" in self.association_directory:
            raise HostControlError("application update requires an absolute private output directory")


@dataclass(frozen=True)
class ApplicationPreparation:
    """One selected schema4 driver plan on the existing local private channel.

    The timeout is the caller's retained observation budget, not permission or
    proof of native cleanup. A preparation is never automatically restarted.
    """
    plan: SelectedHostArtifact
    timeout: float

    def __post_init__(self) -> None:
        if not isinstance(self.plan, SelectedHostArtifact) or not os.path.isabs(self.plan.path):
            raise HostControlError("application preparation requires an absolute selected plan")
        _source_digest(self.plan.sha256)
        if isinstance(self.timeout, bool) or not isinstance(self.timeout, (int, float)) or not math.isfinite(self.timeout) or self.timeout <= 0:
            raise HostControlError("application preparation requires a finite observation budget")


def _source_identifier(value: Any) -> str:
    if not isinstance(value, str) or not value.strip() or len(value.encode("utf-8")) > 256 or any(unicodedata.category(c) == "Cc" for c in value):
        raise HostControlError("invalid authenticated source identifier")
    return value


def _source_digest(value: Any) -> str:
    if not isinstance(value, str) or len(value) != 64 or any(c not in "0123456789abcdef" for c in value):
        raise HostControlError("invalid authenticated source digest")
    return value


@dataclass(frozen=True)
class PrivateSourceEvidence:
    resource_id: str
    revision: str
    sha256: str

    def __post_init__(self) -> None:
        _source_identifier(self.resource_id)
        _source_identifier(self.revision)
        _source_digest(self.sha256)


@dataclass(frozen=True)
class PrivateAuthenticatedEvent:
    """Facts from the application's authenticated event, with no body or grant.

    The installed native route owns requester, executor and audience. Calling
    this API is authorized only by the inherited private host endpoint, never
    by presenting this dataclass to public RPC.
    """
    route: str
    original: str
    occurrence: str
    original_authentication: PrivateSourceEvidence
    root_event: PrivateSourceEvidence
    # Previously admitted originals, not provider rows merged into this event.
    # root_event commits the ordered raw source membership owned by the app.
    contributors: tuple[str, ...]
    content_sha256: str

    def __post_init__(self) -> None:
        for value in (self.route, self.original, self.occurrence):
            _source_identifier(value)
        if not isinstance(self.original_authentication, PrivateSourceEvidence) or not isinstance(self.root_event, PrivateSourceEvidence):
            raise HostControlError("invalid authenticated source evidence")
        if not isinstance(self.contributors, tuple) or len(self.contributors) > 64:
            raise HostControlError("invalid authenticated source contributors")
        for value in self.contributors:
            _source_identifier(value)
        _source_digest(self.content_sha256)


@dataclass(frozen=True)
class PrivateOriginalRef:
    route: str
    original: str
    event_sha256: str

    def __post_init__(self) -> None:
        _source_identifier(self.route)
        _source_identifier(self.original)
        _source_digest(self.event_sha256)


class ServiceReceiptRunScope(TypedDict):
    kind: Literal["run"]
    run_id: str


class ServiceReceiptRuntimeScope(TypedDict):
    kind: Literal["runtime_termination"]


class ServiceInputPending(TypedDict):
    status: Literal["pending"]
    input_id: str
    phase: str
    terminal: dict[str, Any] | None
    last_run_id: str | None
    attempt_count: int


class ServiceInputFinalized(TypedDict):
    status: Literal["finalized"]
    input_id: str
    terminal: dict[str, Any]
    attempt_count: int
    scope: ServiceReceiptRunScope | ServiceReceiptRuntimeScope
    run_id: str | None
    owner_input_id: str
    recipient_input_ids: list[str]
    # Exact native CompletionOutcome JSON, tagged by completion_type. A Run
    # scope or a produced finalization failure is not Completed(result).
    outcome: dict[str, Any]


class ServiceInputWithoutReceipt(TypedDict):
    status: Literal["terminal_without_receipt"]
    input_id: str
    terminal: dict[str, Any]
    last_run_id: str | None
    attempt_count: int


class ServiceInputReceipt(TypedDict):
    source: Literal["live_runtime", "durable_store"]
    report: ServiceInputPending | ServiceInputFinalized | ServiceInputWithoutReceipt


class ServiceInputAdmitted(TypedDict):
    state: Literal["admitted"]
    session_id: str
    input_id: str
    receipt: ServiceInputReceipt | None


class ServiceInputAdmissionUnresolved(TypedDict):
    state: Literal["admission_unresolved"]


ServiceInputObservation = ServiceInputAdmitted | ServiceInputAdmissionUnresolved


@dataclass(frozen=True)
class PrivateOriginalRetention:
    original: PrivateOriginalRef
    state: str = "retained_pending_native_admission"


@dataclass(frozen=True)
class PrivateOriginalCancellation:
    original: PrivateOriginalRef
    state: str = "operations_cancelled"


def _original_ref(value: Any) -> PrivateOriginalRef:
    value = _fields(value, {"route", "original", "event_sha256"})
    return PrivateOriginalRef(**value)


@dataclass(frozen=True)
class PrivateServiceAccessDecision:
    """An authenticated application access decision, separate from installation.

    This application-host adapter selects a real installed source route. It is
    not the standalone stock-realm administration API. No original event is
    needed to issue persistent member access. Principals and native grants are
    not accepted in this payload.
    """
    route: str
    decision: str
    evidence: PrivateSourceEvidence
    choice: str
    permission: dict[str, Any]
    connection_revision: str

    def wire(self) -> dict[str, Any]:
        for value in (self.route, self.decision, self.connection_revision):
            _source_identifier(value)
        if not isinstance(self.evidence, PrivateSourceEvidence) or self.choice not in {"approve", "deny"}:
            raise HostControlError("invalid service access decision")
        # Snapshot the exact existing ServicePermission wire value before the
        # command is sent. Native owner checks still decide scope and authority.
        try:
            permission = json.loads(json.dumps(self.permission, allow_nan=False))
            _fields(permission, {"target", "connection", "operation", "resource", "lifetime"})
            for key in ("target", "connection", "operation", "resource"):
                _source_identifier(permission[key])
            lifetime = _fields(permission["lifetime"], {"scope", "expires_at_ms"})
            scope = lifetime["scope"]
            kind = scope.get("kind") if isinstance(scope, dict) else None
            if kind not in {"task", "member", "deployment"}:
                raise HostControlError("invalid service access lifetime")
            _fields(scope, {"kind", kind})
            _source_identifier(scope[kind])
            expiry = lifetime["expires_at_ms"]
            if expiry is not None and (type(expiry) is not int or not 0 <= expiry < 2**64):
                raise HostControlError("invalid service access expiry")
        except (TypeError, ValueError) as error:
            raise HostControlError("invalid service access permission") from error
        return {"route": self.route, "decision": self.decision,
                "evidence": dataclasses.asdict(self.evidence), "choice": self.choice,
                "permission": permission, "connection_revision": self.connection_revision}


@dataclass(frozen=True)
class PrivateServiceGrantRef:
    route: str
    decision: str
    decision_sha256: str

    def __post_init__(self) -> None:
        _source_identifier(self.route)
        _source_identifier(self.decision)
        _source_digest(self.decision_sha256)


class PrivateServiceGrantState(StrEnum):
    APPROVAL_DENIED = "approval_denied"
    GRANT_ISSUED = "grant_issued"
    GRANT_UNAVAILABLE = "grant_unavailable"
    PENDING_RECONCILIATION = "pending_reconciliation"


@dataclass(frozen=True)
class PrivateServiceGrantObservation:
    """Issuance observation only, not attachment or successful tool execution."""
    grant: PrivateServiceGrantRef
    state: PrivateServiceGrantState


class HostCloseReason(StrEnum):
    SHUTDOWN = "shutdown"
    OPERATOR_DISABLE = "operator_disable"
    QUARANTINE = "quarantine"
    ACTIVATION_REPLACED = "activation_replaced"
    HOST_LOST = "host_lost"


class HostCloseState(StrEnum):
    CLOSING = "closing"
    SETTLED = "settled"
    UNCERTAIN = "uncertain"
    FAILED = "failed"


class HostComponentState(StrEnum):
    STAGED = "staged"
    OPEN = "open"
    CLOSING = "closing"
    CLOSED = "closed"
    QUARANTINED = "quarantined"
    UNCERTAIN = "uncertain"
    FAILED = "failed"


@dataclass(frozen=True)
class HostCloseScope:
    kind: str
    component: str | None = None

    def wire(self) -> dict[str, str]:
        if self.kind == "activation" and self.component is None:
            return {"kind": self.kind}
        if self.kind == "component" and isinstance(self.component, str):
            return {"kind": self.kind, "component": self.component}
        raise HostControlError("invalid close scope")


@dataclass(frozen=True)
class HostCloseHandle:
    incarnation: UUID
    close_id: UUID
    scope: HostCloseScope

    def wire(self) -> dict[str, Any]:
        return {"incarnation": str(self.incarnation), "close_id": str(self.close_id),
                "scope": self.scope.wire()}


@dataclass(frozen=True)
class HostCloseObservation:
    handle: HostCloseHandle
    state: HostCloseState
    failure: str | None


@dataclass(frozen=True)
class HostComponentObservation:
    incarnation: UUID
    component: str
    revision: int
    state: HostComponentState


def _fields(value: Any, expected: set[str]) -> dict[str, Any]:
    if not isinstance(value, dict) or set(value) != expected:
        raise HostControlError("invalid native host response")
    return value


def _service_input_observation(value: Any) -> ServiceInputObservation:
    if isinstance(value, dict) and value.get("state") == "admission_unresolved":
        _fields(value, {"state"})
        return cast(ServiceInputObservation, value)
    _fields(value, {"state", "session_id", "input_id", "receipt"})
    if value["state"] != "admitted":
        raise HostControlError("invalid service input admission observation")
    _source_identifier(value["session_id"])
    selected = _source_identifier(value["input_id"])
    receipt = value["receipt"]
    if receipt is None:
        return cast(ServiceInputObservation, value)
    _fields(receipt, {"source", "report"})
    if receipt["source"] not in {"live_runtime", "durable_store"}:
        raise HostControlError("invalid native receipt witness")
    report = receipt["report"]
    if not isinstance(report, dict):
        raise HostControlError("invalid native receipt report")
    status = report.get("status")
    common = {"status", "input_id", "terminal", "attempt_count"}
    if status == "pending":
        _fields(report, common | {"phase", "last_run_id"})
        if report["phase"] not in {"accepted", "queued", "staged", "applied", "applied_pending_consumption", "consumed", "superseded", "coalesced", "abandoned"}:
            raise HostControlError("invalid native input phase")
    elif status == "terminal_without_receipt":
        _fields(report, common | {"last_run_id"})
    elif status == "finalized":
        _fields(report, common | {"scope", "run_id", "owner_input_id", "recipient_input_ids", "outcome"})
        scope = report["scope"]
        if isinstance(scope, dict) and scope.get("kind") == "run":
            _fields(scope, {"kind", "run_id"})
            if _source_identifier(scope["run_id"]) != report["run_id"]:
                raise HostControlError("native receipt run scope differs")
        else:
            _fields(scope, {"kind"})
            if scope["kind"] != "runtime_termination" or report["run_id"] is not None:
                raise HostControlError("invalid native receipt scope")
        owner = _source_identifier(report["owner_input_id"])
        recipients = report["recipient_input_ids"]
        if not isinstance(recipients, list) or not recipients:
            raise HostControlError("invalid native receipt recipients")
        for recipient in recipients:
            _source_identifier(recipient)
        if recipients[0] != owner or selected not in recipients or len(set(recipients)) != len(recipients):
            raise HostControlError("native receipt recipient association differs")
        outcome = report["outcome"]
        if not isinstance(outcome, dict) or outcome.get("completion_type") not in {
            "completed", "completed_without_result", "callback_pending", "callback_batch_pending", "cancelled",
            "abandoned", "abandoned_with_error", "completed_with_finalization_failure", "runtime_terminated",
        }:
            raise HostControlError("invalid native completion outcome")
    else:
        raise HostControlError("invalid native receipt status")
    if _source_identifier(report["input_id"]) != selected:
        raise HostControlError("native receipt names another input")
    count = report["attempt_count"]
    if type(count) is not int or count < 0 or count > 0xffff_ffff:
        raise HostControlError("invalid native attempt count")
    if status != "finalized" and report["last_run_id"] is not None:
        _source_identifier(report["last_run_id"])
    terminal = report["terminal"]
    if terminal is None:
        if status != "pending":
            raise HostControlError("native terminal outcome is missing")
    elif not isinstance(terminal, dict) or terminal.get("outcome_type") not in {"consumed", "superseded", "coalesced", "abandoned"}:
        raise HostControlError("invalid native terminal outcome")
    # Preserve native outcome/state payloads; this decoder never synthesizes a
    # completed result, flattens a batch or discards finalization failure data.
    return cast(ServiceInputObservation, value)


def _close(value: Any) -> HostCloseObservation:
    value = _fields(value, {"handle", "state", "failure"})
    handle = _fields(value["handle"], {"incarnation", "close_id", "scope"})
    scope = handle["scope"]
    if not isinstance(scope, dict):
        raise HostControlError("invalid native close scope")
    _fields(scope, {"kind", "component"} if scope.get("kind") == "component" else {"kind"})
    typed_scope = HostCloseScope(scope["kind"], scope.get("component"))
    typed_scope.wire()
    failure = value["failure"]
    if failure not in (None, "physical_settlement_uncertain", "policy_unavailable", "owner_failed"):
        raise HostControlError("invalid native close failure")
    return HostCloseObservation(
        HostCloseHandle(UUID(handle["incarnation"]), UUID(handle["close_id"]), typed_scope),
        HostCloseState(value["state"]), failure,
    )


def _component(value: Any) -> HostComponentObservation:
    value = _fields(value, {"incarnation", "component", "revision", "state"})
    revision = value["revision"]
    if type(revision) is not int or not 0 <= revision < 2**64 or not isinstance(value["component"], str):
        raise HostControlError("invalid native component observation")
    return HostComponentObservation(UUID(value["incarnation"]), value["component"],
                                    revision, HostComponentState(value["state"]))


def _review_integer(value: Any) -> int:
    if type(value) is not int or not 0 < value < 2**64:
        raise HostControlError("invalid remote review integer")
    return value


@dataclass(frozen=True)
class RemoteControllerLogin:
    """Application invocation correlation, never the native operator identity."""
    scope_id: str
    expires_at_ms: int

    def __post_init__(self) -> None:
        _source_identifier(self.scope_id)
        _review_integer(self.expires_at_ms)


def _native_lineage(lineage: Any) -> None:
    if not isinstance(lineage, list) or not 0 < len(lineage) <= 64:
        raise HostControlError("invalid native lineage data")
    for entry in lineage:
        _fields(entry, {"root_authority", "authority_namespace", "authority_generation", "authority_incarnation", "grant_id", "issued_revision"})
        if not isinstance(entry["root_authority"], dict):
            raise HostControlError("invalid native lineage data")
        _source_identifier(entry["authority_namespace"])
        _source_identifier(entry["grant_id"])
        _review_integer(entry["authority_generation"])
        _review_integer(entry["issued_revision"])
        if not isinstance(entry["authority_incarnation"], str):
            raise HostControlError("invalid native authority incarnation")
        UUID(entry["authority_incarnation"])


def _controller_outcome(value: Any, route: str) -> dict[str, Any]:
    if not isinstance(value, dict):
        raise HostControlError("invalid controller parent outcome")
    if value.get("state") != "selected":
        _fields(value, {"state"})
        if value["state"] not in {"denied", "unavailable", "pending_reconciliation"}:
            raise HostControlError("invalid controller parent state")
        return dict(value)
    _fields(value, {"state", "route", "purpose", "lineage"})
    if value["route"] != route or value["purpose"] != "controller" or not isinstance(value["lineage"], list) or not 0 < len(value["lineage"]) <= 64:
        raise HostControlError("controller parent selection does not match the review")
    _native_lineage(value["lineage"])
    # Native data for the trusted compiler, never proof of current permission.
    return value


def _setup_helper_targets(value: Any) -> None:
    """Check the wire shape, retaining each native target and its own bounds.

    Restriction contents and grant currentness remain native-owner decisions.
    This observation neither combines targets nor supplies authority to use them.
    """
    if not isinstance(value, list) or len(value) >= 64:
        raise HostControlError("invalid native setup targets")
    seen = set()
    for item in value:
        row = _fields(item, {"target", "parent", "restrictions"})
        target = row["target"]
        key = None
        if target is not None:
            if not isinstance(target, dict):
                raise HostControlError("invalid native setup target")
            if target.get("kind") == "mcp_server":
                _fields(target, {"kind", "server"})
                names = (target["server"],)
            elif target.get("kind") == "registered_tool_bundle_operation":
                _fields(target, {"kind", "bundle", "operation"})
                names = (target["bundle"], target["operation"])
            else:
                raise HostControlError("unknown native setup target")
            if any(not isinstance(name, str) or not name for name in names):
                raise HostControlError("invalid native setup target")
            key = (target["kind"], *names)
        if key in seen:
            raise HostControlError("duplicate native setup target")
        seen.add(key)
        _native_lineage([row["parent"]])
        restrictions = _fields(row["restrictions"], {
            "actions", "resource_domains", "processors", "audiences", "lifetime", "delegation_depth",
        })
        if any(not isinstance(dimension, dict) for dimension in restrictions.values()):
            raise HostControlError("invalid native setup restrictions")


def _setup_request_outcome(value: Any, original: PrivateOriginalRef) -> dict[str, Any]:
    if not isinstance(value, dict):
        raise HostControlError("invalid setup request outcome")
    if value.get("state") != "setup_request_selected":
        _fields(value, {"state"})
        if value["state"] not in {"denied", "unavailable", "pending_reconciliation"}:
            raise HostControlError("invalid setup request state")
        return dict(value)
    _fields(value, {"state", "selection"})
    selection = value["selection"]
    expected = {"original", "session_id", "parent"}
    if isinstance(selection, dict) and "helper_targets" in selection:
        expected.add("helper_targets")
    selected = _fields(selection, expected)
    if _original_ref(selected["original"]) != original or not isinstance(selected["session_id"], str):
        raise HostControlError("setup request selection does not match the original")
    UUID(selected["session_id"])
    _native_lineage(selected["parent"])
    if len(selected["parent"]) != 1:
        raise HostControlError("invalid setup request parent")
    if "helper_targets" in selected:
        _setup_helper_targets(selected["helper_targets"])
    return value


class RemoteControllerReview:
    """One retained review and its final result. No new thread or native session.

    Use review_id only on the originating PrivateHostClient. Observe and answer
    while result() waits in the application's normal asynchronous composition.
    Calling result again returns the same observation or error without a resend.
    """
    def __init__(self, client: PrivateHostClient, review_id: int, deadline: float, route: str, *, setup_original: PrivateOriginalRef | None = None):
        self.review_id = review_id
        self._client = client
        self._deadline = deadline
        self._route = route
        self._setup_original = setup_original
        self._lock = threading.Lock()
        self._finished = False
        self._outcome: dict[str, Any] | None = None
        self._error: BaseException | None = None

    def result(self) -> dict[str, Any]:
        with self._lock:
            if not self._finished:
                try:
                    reply = self._client._await_call(self.review_id, self._deadline,
                                                    "setup_request_reviewed" if self._setup_original is not None else "controller_parent_reviewed", {"kind", "outcome"})
                    try:
                        self._outcome = (_setup_request_outcome(reply["outcome"], self._setup_original)
                                         if self._setup_original is not None else _controller_outcome(reply["outcome"], self._route))
                    except BaseException as error:
                        self._client._fail(error)
                        raise HostTransportError("controller parent outcome was rejected") from error
                except BaseException as error:
                    self._error = error
                self._finished = True
            if self._error is not None:
                raise self._error
            assert self._outcome is not None
            # Preserve the retained native result if the caller edits its copy.
            return json.loads(json.dumps(self._outcome, allow_nan=False))


@dataclass(frozen=True)
class ConsoleLoginSelection:
    """Trusted-parent lifetime labels, not authentication or permission."""
    issuer: str
    subject: str
    login_scope_id: str
    expires_at_ms: int

    def __post_init__(self) -> None:
        for value, bound in ((self.issuer, 4096), (self.subject, 4096), (self.login_scope_id, 256)):
            if not isinstance(value, str) or not value.strip() or len(value.encode("utf-8")) > bound or any(unicodedata.category(c) == "Cc" for c in value):
                raise HostControlError("invalid Console login label")
        if type(self.expires_at_ms) is not int or not 0 < self.expires_at_ms <= 2**53 - 1:
            raise HostControlError("invalid Console login expiry")


@dataclass(frozen=True)
class ConsoleLoginRetirement:
    """Local fence/drain observation, not proof of native or account cleanup."""
    uncertain: bool
    fenced: bool = True
    reservations_drained: bool = True


class ConsoleLoginEnrollment:
    """One same-connection enrollment and retirement, retained even after timeout.

    result() must succeed before minting a token for this enrollment. A repeated
    result() returns its original observation/error; it cannot recover a discarded
    late reply. retire() addresses the original enrollment even if result is unknown.
    Use the application's existing owned tasks for these blocking waits.
    """
    def __init__(self, client: PrivateHostClient, identifier: int, deadline: float):
        self._client, self._identifier, self._deadline = client, identifier, deadline
        self._result_lock, self._retire_lock = threading.Lock(), threading.Lock()
        self._finished = self._retired = False
        self._retiring = False
        self._error: BaseException | None = None
        self._retire_error: BaseException | None = None
        self._retirement: ConsoleLoginRetirement | None = None

    @property
    def enrollment_id(self) -> int:
        return self._identifier

    def result(self) -> None:
        with self._result_lock:
            if not self._finished:
                try:
                    reply = self._client._await_call(self._identifier, self._deadline,
                        "console_login_enrolled", {"kind", "enrollment_id"})
                    if type(reply["enrollment_id"]) is not int or reply["enrollment_id"] != self._identifier:
                        error = HostTransportError("Console login correlation was rejected")
                        self._client._fail(error)
                        raise error
                except BaseException as error:
                    self._error = error
                self._finished = True
            if self._error is not None:
                raise self._error

    def retire(self, *, timeout: float | None = None) -> ConsoleLoginRetirement:
        self._retiring = True
        # This lock is independent of result(): an unobserved enrollment must
        # still be retireable while the original observer waits.
        with self._retire_lock:
            if not self._retired:
                try:
                    identifier, deadline = self._client._start_call(
                        {"kind": "retire_console_login", "enrollment_id": self._identifier},
                        timeout=timeout, _retirement=True)
                    reply = self._client._await_call(identifier, deadline, "console_login_retired",
                        {"kind", "enrollment_id", "observation"})
                    try:
                        if type(reply["enrollment_id"]) is not int or reply["enrollment_id"] != self._identifier:
                            raise HostControlError("Console retirement correlation was rejected")
                        observation = _fields(reply["observation"], {"fenced", "reservations_drained", "uncertain"})
                        if observation["fenced"] is not True or observation["reservations_drained"] is not True or type(observation["uncertain"]) is not bool:
                            raise HostControlError("invalid Console retirement observation")
                        self._retirement = ConsoleLoginRetirement(observation["uncertain"])
                    except BaseException as error:
                        self._client._fail(error)
                        raise HostTransportError("Console retirement response was rejected") from error
                except BaseException as error:
                    self._retire_error = error
                self._retired = True
            if self._retire_error is not None:
                raise self._retire_error
            assert self._retirement is not None
            return self._retirement


class ApplicationAdministrationReservation(Protocol):
    """The application's actual held authorization exclusion for one proposal.

    Implement this in the existing credential/rule owner. Fields and Python
    objects cannot authenticate an administrator. check_current is synchronous,
    returns None only while current, and raises a typed HostReplyError otherwise.
    The owner must fence this exchange before invalidating its held permission.
    release runs only after an exact operation-owner Joined observation; a
    standing reservation release does not revoke the underlying standing rule.
    """
    @property
    def proposal_sha256(self) -> str: ...
    @property
    def authorization_ref(self) -> str: ...
    @property
    def provenance(self) -> dict[str, Any]: ...
    def check_current(self) -> None: ...
    def release(self) -> None: ...


@dataclass(frozen=True)
class CurrentLoginDecision:
    """Actual login-bound decision, including a login-bound standing rule.

    The stable rule attribution belongs in the reservation's provenance. The
    enrollment supplies lifetime only and must already be acknowledged on this
    same client. It never authenticates the reservation or substitutes for R3.
    """
    reservation: ApplicationAdministrationReservation
    login: ConsoleLoginEnrollment


@dataclass(frozen=True)
class StandingRuleDecision:
    """Actual finite-rule reservation independent of a current human login."""
    reservation: ApplicationAdministrationReservation


def _service_parent_copy(value: Any) -> dict[str, Any]:
    try:
        copied = json.loads(json.dumps(value, allow_nan=False))
    except (TypeError, ValueError, UnicodeError) as error:
        raise HostControlError("invalid service-parent projection") from error
    if not isinstance(copied, dict):
        raise HostControlError("invalid service-parent projection")
    return copied


class _ServiceParentCall:
    """One reply retained in the existing pending-call owner across observers."""
    def __init__(self, client: PrivateHostClient):
        self.client = client
        self.lock = threading.RLock()
        self.started = False
        self.correlation: tuple[int, float] | None = None
        self.error: BaseException | None = None
        self.decoded: dict[str, Any] | None = None
        self.cleanup = False

    def publish(self, identifier: int, deadline: float) -> None:
        self.correlation = (identifier, deadline)

    def observe(self, command: dict[str, Any], expected: str, reply_fields: set[str],
                decode: Callable[[dict[str, Any]], dict[str, Any]], *,
                timeout: float | None, retirement: bool = False,
                schedule: Callable[[_ServiceParentCall], None] | None = None) -> dict[str, Any]:
        duration = self.client._timeout if timeout is None else timeout
        if isinstance(duration, bool) or not isinstance(duration, (int, float)) or not math.isfinite(duration) or duration <= 0:
            raise ValueError("native host timeout must be positive and finite")
        deadline = time.monotonic() + duration
        if not self.lock.acquire(timeout=max(0.0, deadline - time.monotonic())):
            raise TimeoutError("service-parent observer timed out; original exchange is retained")
        try:
            if self.decoded is not None:
                return _service_parent_copy(self.decoded)
            if self.error is not None:
                raise self.error
            if not self.started:
                # Record the attempt before any write or application interruption.
                if schedule is None:
                    self.started = True
                else:
                    schedule(self)
                try:
                    self.client._start_call(command, timeout=duration,
                        _retirement=retirement, _publish=self.publish)
                except BaseException as error:
                    self.error = error
                    raise
            if self.correlation is None:
                raise HostControlError("service-parent send observation is unavailable")
            try:
                reply = self.client._await_call(self.correlation[0], deadline,
                    expected, reply_fields, _retain=True)
            except (TimeoutError, KeyboardInterrupt):
                if self.cleanup:
                    self._forget_locked()
                # Only this observer stopped. The same correlation and response
                # remain owned; another observer may await that original reply.
                raise
            except BaseException as error:
                self.error = error
                raise
            try:
                self.decoded = decode(reply)
            except BaseException as error:
                self.client._fail(error)
                self.error = HostTransportError("service-parent response was rejected")
                raise self.error from error
            copied = _service_parent_copy(self.decoded)
            if self.cleanup:
                self._forget_locked()
            return copied
        finally:
            self.lock.release()

    def _forget_locked(self) -> None:
        if self.correlation is None:
            return
        identifier = self.correlation[0]
        with self.client._condition:
            if self.client._pending.get(identifier) is not None:
                self.client._pending.pop(identifier, None)
            elif identifier in self.client._pending:
                self.client._abandoned.add(identifier)
        if self.decoded is None and self.error is None:
            self.error = HostControlError("service-parent response was not observed before joined retirement")

    def forget(self) -> None:
        self.cleanup = True
        if self.lock.acquire(blocking=False):
            try:
                self._forget_locked()
            finally:
                self.lock.release()


class ServiceParentAdministrationAttempt:
    """The request's same retained authorized exchange, never a replay handle."""
    def __init__(self, request: ServiceParentAdministrationRequest):
        self._request = request

    def submit(self, *, timeout: float | None = None) -> dict[str, Any]:
        """Forward the one retained decision; an ACK is not operation success."""
        return self._request._submit(timeout=timeout)

    def result(self, *, timeout: float | None = None) -> dict[str, Any]:
        return self._request.result(timeout=timeout)

    def retire(self, *, timeout: float | None = None) -> dict[str, Any]:
        return self._request.retire(timeout=timeout)


class ServiceParentAdministrationRequest:
    """One exact same-client proposal, with retained result and explicit cleanup.

    The actual app owner must retain this client through cleanup. EOF, a canceled
    wait or close() cannot release its held authorization or establish Joined.
    Use the application's existing owned workers for the blocking methods.
    """
    def __init__(self, client: PrivateHostClient):
        self._client = client
        self._lock = threading.RLock()
        self._next = _ServiceParentCall(client)
        self._answer = _ServiceParentCall(client)
        self._result = _ServiceParentCall(client)
        self._retirement = _ServiceParentCall(client)
        self._request: dict[str, Any] | None = None
        self._decision: dict[str, Any] | None = None
        self._authorization: tuple[ApplicationAdministrationReservation | None, ConsoleLoginEnrollment | None, ServiceParentAdministrationAttempt, dict[str, Any]] | None = None
        self._fenced = False
        self._released = False
        self._release_error: BaseException | None = None
        self._joined: dict[str, Any] | None = None
        self._retired = False

    @property
    def exchange_id(self) -> int:
        return self._selected()["exchange_id"]

    @property
    def proposal_sha256(self) -> str:
        return self._selected()["proposal_sha256"]

    @property
    def proposal(self) -> dict[str, Any]:
        return _service_parent_copy(self._selected()["proposal"])

    @property
    def attempt(self) -> ServiceParentAdministrationAttempt | None:
        with self._lock:
            return self._authorization[2] if self._authorization is not None else None

    def _selected(self) -> dict[str, Any]:
        if self._request is None:
            raise HostControlError("service-parent proposal has not been observed")
        return self._request

    def _observe_request(self, *, timeout: float | None) -> ServiceParentAdministrationRequest:
        try:
            selected = self._next.observe({"kind": "next_service_parent_administration"},
                "service_parent_administration_request",
                {"kind", "exchange_id", "proposal_sha256", "proposal"},
                _service_parent_request, timeout=timeout)
        except HostReplyError:
            # A decoded refusal ended this unanswered query. Timeouts and
            # transport loss retain its correlation and any selected custody.
            with self._lock:
                if (self._request is None and self._authorization is None
                        and self._decision is None and not self._answer.started):
                    with self._client._condition:
                        if self._client._service_exchange is self:
                            self._client._service_exchange = None
                    self._next.forget()
            raise
        with self._lock:
            if self._request is None:
                self._request = selected
            elif self._request != selected:
                raise HostControlError("service-parent proposal changed on the same exchange")
        return self

    @property
    def _guard(self) -> ApplicationAdministrationReservation | None:
        return self._authorization[0] if self._authorization is not None else None

    @property
    def _login(self) -> ConsoleLoginEnrollment | None:
        return self._authorization[1] if self._authorization is not None else None

    def _current(self) -> None:
        if self._guard is None:
            raise HostControlError("service-parent authorization was not retained")
        if self._login is not None and self._login._retiring:
            raise HostReplyError(HostReplyCode.DENIED)
        if self._guard.check_current() is not None:
            raise HostControlError("administration currentness must return None or raise")

    @property
    def tool_policy_sha256(self) -> str | None:
        """Digest of the complete optional policy proposal, not an approval."""
        proposal = self._selected()["proposal"].get("tool_policy")
        return _service_tool_policy_digest(proposal) if proposal is not None else None

    def authorize(self, decision: CurrentLoginDecision | StandingRuleDecision) -> ServiceParentAdministrationAttempt:
        """Transfer service-only approval; an amended proposal needs explicit scope."""
        return self._authorize(decision, None)

    def authorize_tool_policy(self, decision: CurrentLoginDecision | StandingRuleDecision,
                              tool_policy_sha256: str) -> ServiceParentAdministrationAttempt:
        """Approve the exact COMPLETE policy until superseded in the same decision.

        Inspect candidate_canonical_json, including preserved permissions. If
        catalog is selected, allow the waiting original agent to find tools
        available in the original conversation. Service/catalog grants still
        have their task lifetime.
        The existing actual reservation must cover these scopes; this method
        does not authenticate it or imply another human prompt. No write occurs.
        """
        return self._authorize(decision, _source_digest(tool_policy_sha256))

    def _authorize(self, decision: CurrentLoginDecision | StandingRuleDecision,
                   tool_policy_sha256: str | None) -> ServiceParentAdministrationAttempt:
        with self._lock:
            selected = self._selected()
            if self._fenced or self._decision is not None or self._authorization is not None:
                raise HostControlError("service-parent exchange is already answered or retired")
            self._client.assert_available()
            expected_policy = self.tool_policy_sha256
            if tool_policy_sha256 != expected_policy:
                raise HostControlError("the complete tool policy requires its exact explicit approval")
            if not isinstance(decision, (CurrentLoginDecision, StandingRuleDecision)):
                raise HostControlError("an actual administration reservation is required")
            guard = decision.reservation
            if (_source_digest(guard.proposal_sha256) != selected["proposal_sha256"]
                    or not callable(guard.check_current) or not callable(guard.release)):
                raise HostControlError("administration reservation does not match the proposal")
            reference = _source_identifier(guard.authorization_ref)
            provenance = _service_parent_copy(guard.provenance)
            if not provenance or len(json.dumps(provenance, separators=(",", ":"), ensure_ascii=False).encode("utf-8")) > 12 * 1024:
                raise HostControlError("administration provenance is empty or exceeds its bound")
            login = decision.login if isinstance(decision, CurrentLoginDecision) else None
            source: dict[str, Any] = {"kind": "standing_rule_decision"}
            if isinstance(decision, CurrentLoginDecision):
                if (not isinstance(login, ConsoleLoginEnrollment) or login._client is not self._client
                        or not login._finished or login._error is not None or login._retiring):
                    raise HostControlError("administration needs the acknowledged same-client login")
                source = {"kind": "current_login_decision", "enrollment_id": login.enrollment_id}
            if guard.check_current() is not None:
                raise HostControlError("administration currentness must return None or raise")
            # All fallible validation precedes transfer. Store the attempt alias
            # with the real guard before returning to application code.
            attempt = ServiceParentAdministrationAttempt(self)
            answer = {"kind": "authorized", "source": source,
                      "authorization_ref": reference, "provenance": provenance}
            if tool_policy_sha256 is not None:
                answer["kind"] = "authorized_with_tool_policy"
                answer["tool_policy_sha256"] = tool_policy_sha256
            self._authorization = (guard, login, attempt, answer)
            return attempt

    def _negative(self, kind: str, *, timeout: float | None) -> dict[str, Any]:
        with self._lock:
            self._selected()
            if self._fenced or self._authorization is not None:
                raise HostControlError("service-parent exchange is authorized or retired")
            decision = {"kind": kind}
            if self._decision is None:
                self._decision = decision
            elif self._decision != decision:
                raise HostControlError("service-parent exchange already retained another answer")
        return self._submit(timeout=timeout)

    def requires_human(self, *, timeout: float | None = None) -> dict[str, Any]:
        return self._negative("requires_human", timeout=timeout)

    def deny(self, *, timeout: float | None = None) -> dict[str, Any]:
        return self._negative("denied", timeout=timeout)

    def unavailable(self, *, timeout: float | None = None) -> dict[str, Any]:
        return self._negative("unavailable", timeout=timeout)

    def _submit(self, *, timeout: float | None) -> dict[str, Any]:
        with self._lock:
            answer = self._authorization[3] if self._authorization is not None else self._decision
            if answer is None:
                raise HostControlError("service-parent decision has not been retained")
            selected = self._selected()
            command = {"kind": "submit_service_parent_administration",
                "exchange_id": selected["exchange_id"],
                "proposal_sha256": selected["proposal_sha256"], "decision": answer}
        def schedule(call: _ServiceParentCall) -> None:
            # This is the linearization point with concurrent retirement. Once
            # scheduled, a partial or queued write remains potentially entered.
            with self._lock:
                if self._fenced:
                    raise HostControlError("service-parent exchange is retired before submission")
                if self._guard is not None:
                    try:
                        self._current()
                    except BaseException:
                        self._fenced = True
                        raise
                call.started = True
        def decode(reply: dict[str, Any]) -> dict[str, Any]:
            if _review_integer(reply["exchange_id"]) != self.exchange_id:
                raise HostControlError("service-parent answer correlation was rejected")
            return {"state": "answer_forwarded"}
        return self._answer.observe(command, "service_parent_administration_submitted",
            {"kind", "exchange_id"}, decode, timeout=timeout, schedule=schedule)

    def _accept_joined(self, value: dict[str, Any]) -> dict[str, Any]:
        with self._lock:
            if self._joined is not None and self._joined != value:
                raise HostControlError("service-parent terminal observation changed")
            self._joined = value
            if not self._released:
                self._released = True
                if self._guard is not None:
                    try:
                        if self._guard.release() is not None:
                            raise HostControlError("administration release must be synchronous")
                        assert self._authorization is not None
                        self._authorization = (None, self._authorization[1], self._authorization[2], self._authorization[3])
                    except BaseException as error:
                        self._release_error = error
            if self._release_error is not None:
                raise self._release_error
            return _service_parent_copy(value)

    def _decode_observation(self, reply: dict[str, Any]) -> dict[str, Any]:
        if _review_integer(reply["exchange_id"]) != self.exchange_id:
            raise HostControlError("service-parent result correlation was rejected")
        return _service_parent_observation(reply["observation"], self._selected())

    def result(self, *, timeout: float | None = None) -> dict[str, Any]:
        """Read only; before submission returns local pending, never refusal."""
        with self._lock:
            self._selected()
            if self._joined is not None:
                return self._accept_joined(self._joined)
            if not self._answer.started:
                return {"state": "not_submitted"}
        observed = self._result.observe({"kind": "result_service_parent_administration",
            "exchange_id": self.exchange_id}, "service_parent_administration_result",
            {"kind", "exchange_id", "observation"}, self._decode_observation, timeout=timeout)
        return self._accept_joined(observed)

    def retire(self, *, timeout: float | None = None) -> dict[str, Any]:
        """Fence locally, then observe actual Joined on the same exchange."""
        with self._lock:
            self._selected()
            self._fenced = True
        observed = self._retirement.observe({"kind": "retire_service_parent_administration",
            "exchange_id": self.exchange_id}, "service_parent_administration_retired",
            {"kind", "exchange_id", "observation"}, self._decode_observation,
            timeout=timeout, retirement=True)
        result = self._accept_joined(observed)
        with self._client._condition:
            self._retired = True
            if self._client._service_exchange is self:
                self._client._service_exchange = None
        for call in (self._next, self._answer, self._result, self._retirement):
            call.forget()
        return result


def _service_policy_digest(value: Any) -> str:
    if not isinstance(value, str) or not value.startswith("sha256:"):
        raise HostControlError("invalid native policy digest")
    _source_digest(value[7:])
    return value


def _service_policy_version(value: Any) -> dict[str, Any]:
    version = _fields(value, {"revision", "digest"})
    # Native u64 is projected only within this channel's safe-integer profile.
    # In Python an unvalidated bool would compare equal to revision zero or one.
    if type(version["revision"]) is not int or not 0 <= version["revision"] <= 2**53 - 1:
        raise HostControlError("tool policy revision is outside the SDK integer profile")
    _service_policy_digest(version["digest"])
    return version


def _service_tool_policy(value: Any) -> dict[str, Any]:
    proposal = _fields(value, {"provider_id", "policy_id", "member", "selection", "catalog_session",
        "prior", "candidate", "candidate_canonical_json", "lifetime"})
    for name in ("provider_id", "policy_id", "member"):
        _source_identifier(proposal[name])
    selection = _fields(proposal["selection"], {"tools", "catalog"})
    tools = selection["tools"]
    if not isinstance(tools, list) or len(tools) > 128 or selection["catalog"] not in {None, "r0", "r1", "r2", "r3"}:
        raise HostControlError("invalid exact tool selection")
    names = set()
    for value in tools:
        tool = _fields(value, {"name", "consequence"})
        name = tool["name"]
        if (not isinstance(name, str) or not name or len(name.encode("utf-8")) > 512
                or name.strip() != name or any(unicodedata.category(ch) == "Cc" for ch in name)
                or tool["consequence"] not in {"r0", "r1", "r2", "r3"} or name in names):
            raise HostControlError("invalid exact tool grant")
        names.add(name)
    if selection["catalog"] is not None:
        if names.intersection({"tool_catalog_search", "tool_catalog_load"}):
            raise HostControlError("catalog tools are selected through catalog scope")
        if not isinstance(proposal["catalog_session"], str) or not proposal["catalog_session"]:
            raise HostControlError("catalog scope has no observed original session")
    elif proposal["catalog_session"] is not None:
        raise HostControlError("unexpected catalog session")
    if not tools and selection["catalog"] is None:
        raise HostControlError("empty tool policy selection")
    for field in ("prior", "candidate"):
        _service_policy_version(proposal[field])
    canonical = proposal["candidate_canonical_json"]
    if (proposal["lifetime"] != "until_superseded" or not isinstance(canonical, str)
            or not canonical.endswith("\n")
            or "sha256:" + hashlib.sha256(canonical.encode("utf-8")).hexdigest() != proposal["candidate"]["digest"]):
        raise HostControlError("invalid complete canonical tool policy")
    return proposal


def _service_tool_policy_digest(proposal: dict[str, Any]) -> str:
    # Closed proposal uses ASCII keys, strings and safe integer revisions. The
    # embedded canonical policy remains an exact string, including its newline.
    value = _service_tool_policy(proposal)
    return hashlib.sha256(json.dumps(value, sort_keys=True, separators=(",", ":"),
        ensure_ascii=False, allow_nan=False).encode("utf-8")).hexdigest()


def _service_parent_request(reply: dict[str, Any]) -> dict[str, Any]:
    selected = _service_parent_copy(reply)
    _review_integer(selected["exchange_id"])
    _source_digest(selected["proposal_sha256"])
    proposal_value = selected["proposal"]
    proposal = _fields(proposal_value, {
        "operator", "rule", "issuance_id", "deployment", "source_policy_sha256",
        "original", "acquisition", "instruction_sha256", "expires_in_ms", "application",
        "represented_subject", "decision", "connector_selection", "restrictions",
    } | ({"tool_policy"} if isinstance(proposal_value, dict) and "tool_policy" in proposal_value else set()))
    if "tool_policy" in proposal:
        _service_tool_policy(proposal["tool_policy"])
    for key in ("operator", "application", "decision", "connector_selection", "restrictions"):
        if not isinstance(proposal[key], dict):
            raise HostControlError("invalid service-parent proposal projection")
    if proposal["represented_subject"] is not None and not isinstance(proposal["represented_subject"], dict):
        raise HostControlError("invalid represented subject projection")
    for key in ("issuance_id", "deployment"):
        _source_identifier(proposal[key])
    for key in ("source_policy_sha256", "instruction_sha256"):
        _source_digest(proposal[key])
    _original_ref(proposal["original"])
    acquisition = _fields(proposal["acquisition"], {"realm", "owner", "id"})
    for value in acquisition.values():
        _source_identifier(value)
    expiry = proposal["expires_in_ms"]
    if expiry is not None and (type(expiry) is not int or not 0 <= expiry < 2**64):
        raise HostControlError("invalid service-parent expiry projection")
    rule = _fields(proposal["rule"], {"kind", "routes"})
    if rule["kind"] != "application_policy_tasks" or not isinstance(rule["routes"], list) or not rule["routes"]:
        raise HostControlError("invalid service-parent finite rule")
    for route in rule["routes"]:
        _source_identifier(route)
    if len(set(rule["routes"])) != len(rule["routes"]):
        raise HostControlError("duplicate service-parent route")
    return {key: selected[key] for key in ("exchange_id", "proposal_sha256", "proposal")}


def _service_parent_observation(value: Any, selected: dict[str, Any]) -> dict[str, Any]:
    value = _service_parent_copy(value)
    value = _fields(value, {
        "outcome", "custody", "reason", "parent_issuance_id", "parent_issued", "child", "source",
    } | ({"tool_policy"} if "tool_policy" in value else set()))
    if "tool_policy" in value:
        policy = _fields(value["tool_policy"], {"proposal_sha256", "candidate", "native", "retained_as_current"})
        proposal = selected["proposal"].get("tool_policy")
        if (proposal is None or _source_digest(policy["proposal_sha256"]) != _service_tool_policy_digest(proposal)
                or _service_policy_version(policy["candidate"]) != proposal["candidate"]
                or policy["native"] not in {None, "accepted", "refused"}
                or type(policy["retained_as_current"]) is not bool
                or (policy["retained_as_current"] and policy["native"] != "accepted")):
            raise HostControlError("tool-policy observation does not match the reviewed candidate")
    if value["outcome"] not in {"succeeded", "refused_before_entry", "failed_after_effects", "uncertain"} or value["custody"] != "joined":
        raise HostControlError("service-parent result is not a terminal Joined observation")
    if value["reason"] not in {None, "requires_human", "denied", "unavailable", "not_settled", "conflict", "cancelled"}:
        raise HostControlError("invalid service-parent result reason")
    if type(value["parent_issued"]) is not bool:
        raise HostControlError("invalid service-parent issuance observation")
    if value["parent_issuance_id"] is not None:
        if _source_identifier(value["parent_issuance_id"]) != selected["proposal"]["issuance_id"]:
            raise HostControlError("service-parent issuance correlation was rejected")
    if value["child"] is not None:
        child = _fields(value["child"], {"grant", "state"})
        grant = _fields(child["grant"], {"route", "decision", "decision_sha256"})
        PrivateServiceGrantRef(**grant)
        PrivateServiceGrantState(child["state"])
    if value["source"] is not None:
        _original_ref(value["source"])
    # These are the owner's observations. Joined never rewrites a failed or
    # uncertain outcome into success, attachment or original-task completion.
    return value


class PrivateHostClient:
    """Own non-inheritable parent pipe ends and match typed observations.

    The reader runs independently of waiters. A pending join does not stop a
    concurrent disable, nor does a timeout cancel or invent native settlement.
    """

    def __init__(self, command_fd: int, response_fd: int, *, timeout: float = 60.0):
        # Per-call timeouts may extend observation, never this transport budget.
        if os.name != "posix":
            raise HostControlError("private host control requires POSIX")
        if command_fd < 3 or response_fd < 3 or command_fd == response_fd:
            raise HostControlError("invalid private host descriptors")
        if isinstance(timeout, bool) or not isinstance(timeout, (int, float)) or not math.isfinite(timeout) or timeout <= 0:
            raise ValueError("native host timeout must be positive and finite")
        for fd in (command_fd, response_fd):
            os.set_inheritable(fd, False)
            os.set_blocking(fd, False)
        self._command_fd: int | None = command_fd
        self._response_fd: int | None = response_fd
        self._timeout = timeout
        self._write_lock = threading.Lock()
        self._condition = threading.Condition()
        self._pending: dict[int, dict[str, Any] | None] = {}
        self._abandoned: set[int] = set()
        self._failure: BaseException | None = None
        self._next_id = 1
        self._service_exchange: ServiceParentAdministrationRequest | None = None
        self._reader = threading.Thread(target=self._read, name="toolkit-host-control", daemon=True)
        self._reader.start()

    def _fail(self, failure: BaseException) -> None:
        with self._condition:
            if self._failure is None:
                self._failure = failure
            self._condition.notify_all()
        # Close the command writer under the same lock as writes. EOF informs
        # native ownership even when the response half alone failed.
        with self._write_lock:
            fd, self._command_fd = self._command_fd, None
            if fd is not None:
                os.close(fd)

    def _read(self) -> None:
        buffer = bytearray()
        try:
            while True:
                fd = self._response_fd
                if fd is None:
                    return
                with self._condition:
                    if self._failure is not None:
                        return
                ready, _, _ = select.select([fd], [], [], 0.1)
                if not ready:
                    continue
                try:
                    chunk = os.read(fd, 4096)
                except BlockingIOError:
                    continue
                if not chunk:
                    raise HostControlError("native host pipe closed; settlement is unknown")
                buffer.extend(chunk)
                while b"\n" in buffer:
                    line, _, tail = buffer.partition(b"\n")
                    buffer = bytearray(tail)
                    if len(line) + 1 > _MAX_FRAME:
                        raise HostControlError("native host frame exceeds limit")
                    def unique(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
                        result: dict[str, Any] = {}
                        for key, value in pairs:
                            if key in result:
                                raise HostControlError("duplicate native response key")
                            result[key] = value
                        return result
                    response = _fields(json.loads(line, object_pairs_hook=unique), {"version", "id", "result"})
                    if type(response["id"]) is not int or type(response["version"]) is not int or response["version"] != _PROTOCOL:
                        raise HostControlError("native host protocol version or correlation is invalid")
                    if not isinstance(response["result"], dict):
                        raise HostControlError("native host response result is invalid")
                    with self._condition:
                        if response["id"] not in self._pending or self._pending[response["id"]] is not None:
                            raise HostControlError("native host response has unknown or duplicate correlation")
                        if response["id"] in self._abandoned:
                            self._abandoned.remove(response["id"])
                            del self._pending[response["id"]]
                        else:
                            self._pending[response["id"]] = response["result"]
                        self._condition.notify_all()
                if len(buffer) > _MAX_FRAME:
                    raise HostControlError("native host frame exceeds limit")
        except BaseException as error:
            self._fail(error)
        finally:
            fd, self._response_fd = self._response_fd, None
            if fd is not None:
                os.close(fd)

    def _start_call(self, command: dict[str, Any], *, timeout: float | None = None, _retirement: bool = False, _expected_id: int | None = None,
                    _publish: Callable[[int, float], None] | None = None) -> tuple[int, float]:
        duration = self._timeout if timeout is None else timeout
        if isinstance(duration, bool) or not isinstance(duration, (int, float)) or not math.isfinite(duration) or duration <= 0:
            raise ValueError("native host timeout must be positive and finite")
        started = time.monotonic()
        deadline = started + duration
        # A human review may wait much longer than the transport/control budget.
        # Queueing for the writer consumes that same bounded write budget.
        write_deadline = started + min(self._timeout, duration)
        error: BaseException | None = None
        acquired = self._write_lock.acquire(timeout=max(0.0, write_deadline - time.monotonic()))
        if not acquired:
            error = TimeoutError("native host command write timed out")
        else:
            try:
                with self._condition:
                    if self._failure is not None:
                        raise HostTransportError("native host control unavailable") from self._failure
                    if len(self._pending) >= 32 + int(_retirement):
                        if _retirement:
                            raise HostTransportError("Console retirement could not enter reserved capacity; settlement is unknown")
                        raise HostControlError("native host observation limit reached")
                    identifier = self._next_id
                    if _expected_id is not None and identifier != _expected_id:
                        raise HostControlError("private startup request is out of phase")
                # Local validation precedes correlation publication and every
                # write. An invalid command must not poison other reviews.
                try:
                    frame = json.dumps({"version": _PROTOCOL, "id": identifier, "command": command},
                                       separators=(",", ":"), allow_nan=False).encode() + b"\n"
                except (TypeError, ValueError, UnicodeError) as failure:
                    raise HostControlError("invalid private host command") from failure
                if len(frame) > _MAX_FRAME:
                    raise HostControlError("private host command exceeds limit")
                with self._condition:
                    if self._failure is not None:
                        raise HostTransportError("native host control unavailable") from self._failure
                    self._next_id += 1
                    self._pending[identifier] = None
                    if _publish is not None:
                        # A retained owner takes correlation before the first byte.
                        _publish(identifier, deadline)
                try:
                    while frame:
                        with self._condition:
                            if self._failure is not None:
                                raise HostTransportError("native host control failed") from self._failure
                        remaining = write_deadline - time.monotonic()
                        if remaining <= 0:
                            raise TimeoutError("native host command write timed out")
                        if self._command_fd is None:
                            raise HostTransportError("native host control is closed")
                        _, ready, _ = select.select([], [self._command_fd], [], min(remaining, 0.1))
                        if not ready:
                            continue
                        try:
                            count = os.write(self._command_fd, frame)
                        except BlockingIOError:
                            continue
                        frame = frame[count:]
                except BaseException as failure:
                    error = failure
                    # Poison the writer before unlocking. A queued control must
                    # never append bytes after an uncertain partial command.
                    with self._condition:
                        if self._failure is None:
                            self._failure = failure
                            self._condition.notify_all()
            except HostTransportError as failure:
                error = failure
                # Fence a failed mandatory control before another writer enters.
                with self._condition:
                    if self._failure is None:
                        self._failure = failure
                        self._condition.notify_all()
            finally:
                self._write_lock.release()
        if error is not None:
            self._fail(error)
            raise HostTransportError("native command failed; settlement is unknown") from error
        return identifier, deadline

    def _await_call(self, identifier: int, deadline: float, expected: str, reply_fields: set[str] | None = None, *, _retain: bool = False) -> dict[str, Any]:
        with self._condition:
            while self._pending[identifier] is None and self._failure is None:
                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    # Keep the correlation until its late response or close. No
                    # request is reissued, and no result is treated as success.
                    if not _retain:
                        self._abandoned.add(identifier)
                    raise TimeoutError("native host observation timed out; settlement is unknown")
                self._condition.wait(remaining)
            if self._failure is not None:
                raise HostTransportError("native host control failed; settlement is unknown") from self._failure
            result = self._pending[identifier] if _retain else self._pending.pop(identifier)
        try:
            if not isinstance(result, dict):
                raise HostControlError("native host response is invalid")
            if result.get("kind") == "error":
                _fields(result, {"kind", "error"})
                try:
                    HostReplyCode(result["error"])
                except (TypeError, ValueError) as error:
                    raise HostControlError("native host error is invalid") from error
            else:
                _fields(result, reply_fields if reply_fields is not None else ({"kind"} if expected == "bootstrap_accepted" else {"kind", "observation"}))
                if result["kind"] != expected:
                    raise HostControlError("native host response kind is invalid")
        except BaseException as error:
            self._fail(error)
            raise HostTransportError("native host response was rejected") from error
        if result["kind"] == "error":
            raise HostReplyError(HostReplyCode(result["error"]))
        return result

    def _call(self, command: dict[str, Any], expected: str, *, timeout: float | None = None, reply_fields: set[str] | None = None) -> dict[str, Any]:
        identifier, deadline = self._start_call(command, timeout=timeout)
        return self._await_call(identifier, deadline, expected, reply_fields)

    def next_service_parent_administration(self, *, timeout: float | None = None) -> ServiceParentAdministrationRequest:
        """Observe one actual proposal on this connection; never select its facts.

        Reobservation returns the same request/attempt until explicit retirement
        observes Joined. A timeout retains the original pending correlation.
        """
        with self._condition:
            if self._service_exchange is None:
                self.assert_available()
                self._service_exchange = ServiceParentAdministrationRequest(self)
            request = self._service_exchange
        return request._observe_request(timeout=timeout)

    def begin_console_login(self, login: ConsoleLoginSelection, *, timeout: float | None = None) -> ConsoleLoginEnrollment:
        """Enroll the actual parent's login lifetime once on this pipe.

        Native verified issuer/subject/sid must independently match these labels.
        This result cannot authenticate a caller, create a principal or grant access.
        """
        if not isinstance(login, ConsoleLoginSelection):
            raise HostControlError("invalid Console login selection")
        login.__post_init__()
        identifier, deadline = self._start_call({"kind": "enroll_console_login", "login": dataclasses.asdict(login)}, timeout=timeout)
        return ConsoleLoginEnrollment(self, identifier, deadline)

    def begin_remote_controller_review(self, original: PrivateOriginalRef, content: str,
                                       transport_authentication: PrivateSourceEvidence,
                                       login: RemoteControllerLogin, *,
                                       timeout: float | None = None) -> RemoteControllerReview:
        """Begin once and return before human review, on this same private connection.

        The application authenticates the operator and claims the exact human
        answer before forwarding it. This method cannot select an operator or
        enroll a root. Timeout never retries review, issuance or original work.
        """
        if not isinstance(original, PrivateOriginalRef) or not isinstance(content, str) or not isinstance(transport_authentication, PrivateSourceEvidence) or not isinstance(login, RemoteControllerLogin):
            raise HostControlError("invalid remote controller review")
        content.encode("utf-8")
        identifier, deadline = self._start_call({"kind": "review_remote_controller_parent", "submission": {
            "original": dataclasses.asdict(original), "content": content,
            "transport_authentication": dataclasses.asdict(transport_authentication),
        }, "login": dataclasses.asdict(login)}, timeout=timeout)
        return RemoteControllerReview(self, identifier, deadline, original.route)

    def begin_remote_setup_request_review(self, original: PrivateOriginalRef, content: str,
                                          transport_authentication: PrivateSourceEvidence,
                                          login: RemoteControllerLogin, *,
                                          timeout: float | None = None) -> RemoteControllerReview:
        """Review setup requests for this pending original, separately from controller access.

        Uses the same retained observe/answer/retire transport. A selected result
        is source-row selection only: submit the same original afterward. It is
        neither native admission nor approval to install or use any service.
        """
        if not isinstance(original, PrivateOriginalRef) or not isinstance(content, str) or not isinstance(transport_authentication, PrivateSourceEvidence) or not isinstance(login, RemoteControllerLogin):
            raise HostControlError("invalid remote setup request review")
        content.encode("utf-8")
        identifier, deadline = self._start_call({"kind": "review_remote_setup_request", "submission": {
            "original": dataclasses.asdict(original), "content": content,
            "transport_authentication": dataclasses.asdict(transport_authentication),
        }, "login": dataclasses.asdict(login)}, timeout=timeout)
        return RemoteControllerReview(self, identifier, deadline, original.route, setup_original=original)

    def _controller_review_call(self, kind: str, review_id: int, allowed: set[str], **extra: Any) -> dict[str, Any]:
        _review_integer(review_id)
        reply = self._call({"kind": kind, "review_id": review_id, **extra}, "controller_review",
                           reply_fields={"kind", "review_id", "status"})
        try:
            if _review_integer(reply["review_id"]) != review_id:
                raise HostControlError("remote response names another review")
            status = reply["status"]
            if not isinstance(status, dict) or status.get("state") not in allowed:
                raise HostControlError("invalid remote review status")
            _fields(status, {"state", "prompt"} if status["state"] == "prompt" else {"state"})
            if status["state"] == "prompt" and not isinstance(status["prompt"], dict):
                raise HostControlError("invalid remote approval projection")
            return dict(status)
        except BaseException as error:
            self._fail(error)
            raise HostTransportError("remote review response was rejected") from error

    def observe_controller_review(self, review_id: int) -> dict[str, Any]:
        return self._controller_review_call("observe_controller_review", review_id,
                                            {"awaiting_prompt", "prompt", "answer_forwarded"})

    def answer_controller_review(self, review_id: int, approval_id: str, decision: str) -> dict[str, Any]:
        _source_identifier(approval_id)
        if decision not in {"approve", "deny"}:
            raise HostControlError("invalid remote approval choice")
        return self._controller_review_call("answer_controller_review", review_id, {"answer_forwarded"},
                                            approval_id=approval_id, decision=decision)

    def retire_controller_review(self, review_id: int) -> dict[str, Any]:
        return self._controller_review_call("retire_controller_review", review_id, {"retired"})

    def bootstrap(self, selection: HostBootstrapSelection) -> None:
        self._call({"kind": "bootstrap", "selection": dataclasses.asdict(selection)}, "bootstrap_accepted")

    def replace_application(self, selected: ApplicationUpdateStartup) -> None:
        """Initial private selection only; native init and settlement follow."""
        if not isinstance(selected, ApplicationUpdateStartup):
            raise HostControlError("invalid application update startup")
        selected.__post_init__()
        identifier, deadline = self._start_call(
            {"kind": "replace_application", "startup": dataclasses.asdict(selected)}, _expected_id=1,
        )
        try:
            self._await_call(identifier, deadline, "bootstrap_accepted")
        except BaseException as error:
            self._fail(error)
            raise

    def _begin_application_preparation(self, selected: ApplicationPreparation) -> ApplicationPreparationAttempt:
        identifier, deadline = self._start_call(
            {"kind": "prepare_application", "plan": dataclasses.asdict(selected.plan)},
            timeout=selected.timeout, _expected_id=1,
        )
        return ApplicationPreparationAttempt(self, identifier, deadline)

    def retain_authenticated_event(self, event: PrivateAuthenticatedEvent, *, timeout: float | None = None) -> PrivateOriginalRetention:
        """Persist source facts only. This does not submit or admit native work."""
        if not isinstance(event, PrivateAuthenticatedEvent):
            raise HostControlError("invalid authenticated event")
        wire = dataclasses.asdict(event)
        expected_digest = hashlib.sha256(json.dumps(wire, sort_keys=True, separators=(",", ":"), ensure_ascii=False, allow_nan=False).encode()).hexdigest()
        reply = self._call({"kind": "retain_authenticated_event", "event": wire}, "authenticated_event_retained", timeout=timeout, reply_fields={"kind", "original", "state"})
        try:
            original = _original_ref(reply["original"])
            if original != PrivateOriginalRef(event.route, event.original, expected_digest) or reply["state"] != "retained_pending_native_admission":
                raise HostControlError("source retention response names another original or state")
            return PrivateOriginalRetention(original)
        except BaseException as error:
            self._fail(error)
            raise HostTransportError("authenticated source response was rejected") from error

    def submit_authenticated_original(self, original: PrivateOriginalRef, content: str,
                                      transport_authentication: PrivateSourceEvidence, *,
                                      timeout: float | None = None) -> dict[str, Any]:
        """Submit the exact retained, bound original through native admission.

        Authenticate the occurrence in the application before this call. The
        retained content digest is SHA-256 of the exact UTF-8 text. This API
        cannot supply grants, model facts, requester or destination. Admission
        returns its own native turn ticket, not a claim of task completion.
        A timeout is unknown settlement and must not trigger a blind retry.
        """
        if not isinstance(original, PrivateOriginalRef) or not isinstance(content, str) or not isinstance(transport_authentication, PrivateSourceEvidence):
            raise HostControlError("invalid authenticated original submission")
        reply = self._call({"kind": "submit_authenticated_original", "submission": {
            "original": dataclasses.asdict(original), "content": content,
            "transport_authentication": dataclasses.asdict(transport_authentication),
        }}, "original_submitted", timeout=timeout, reply_fields={"kind", "original", "outcome"})
        try:
            if _original_ref(reply["original"]) != original:
                raise HostControlError("native submission names another original")
            outcome = reply["outcome"]
            if not isinstance(outcome, dict):
                raise HostControlError("invalid native submission observation")
            if outcome.get("state") == "admitted":
                _fields(outcome, {"state", "session_id", "input_id", "deduplicated", "turn_ticket", "source_settlement"})
                _source_identifier(outcome["session_id"])
                _source_identifier(outcome["input_id"])
                if type(outcome["deduplicated"]) is not bool or outcome["source_settlement"] not in {"fresh_acceptance_promoted", "retry_withdrawn", "post_admission_publication_failed"}:
                    raise HostControlError("invalid native admission state")
                if outcome["turn_ticket"] is not None:
                    if not isinstance(outcome["turn_ticket"], str):
                        raise HostControlError("invalid native turn ticket")
                    UUID(outcome["turn_ticket"])
            elif outcome.get("state") in {"admission_unresolved", "native_outcome_unavailable"}:
                _fields(outcome, {"state"})
            else:
                raise HostControlError("invalid native submission state")
            return dict(outcome)
        except BaseException as error:
            self._fail(error)
            raise HostTransportError("native submission response was rejected; settlement is unknown") from error

    def observe_service_input_receipt(self, original: PrivateOriginalRef, *,
                                      timeout: float | None = None) -> ServiceInputObservation:
        """Read one canonical approved B without submitting or waking work.

        Admission unresolved and a native null receipt are different from a
        failed read. Pending remains pending even with a terminal field. Native
        batch scope, witness and CompletionOutcome are preserved, not reduced
        to success. Later execution cancellation does not erase saved facts.
        Oversized results are a bounded unavailable reply, never truncated.
        """
        if not isinstance(original, PrivateOriginalRef):
            raise HostControlError("invalid original reference")
        reply = self._call({"kind": "observe_service_input_receipt", "original": dataclasses.asdict(original)},
                           "service_input_receipt", timeout=timeout, reply_fields={"kind", "original", "observation"})
        try:
            if _original_ref(reply["original"]) != original:
                raise HostControlError("native receipt names another original")
            return _service_input_observation(reply["observation"])
        except BaseException as error:
            self._fail(error)
            raise HostTransportError("native receipt response was rejected") from error

    def cancel_original_operation(self, original: PrivateOriginalRef, *, timeout: float | None = None) -> PrivateOriginalCancellation:
        """Durably cancel this original's operations without revoking shared access.

        This does not stop a runtime run or its controller. A timeout remains
        unknown. The command is never automatically repeated.
        """
        if not isinstance(original, PrivateOriginalRef):
            raise HostControlError("invalid original reference")
        reply = self._call({"kind": "cancel_original_operation", "original": dataclasses.asdict(original)}, "original_operation_cancelled", timeout=timeout, reply_fields={"kind", "original", "state"})
        try:
            if _original_ref(reply["original"]) != original or reply["state"] != "operations_cancelled":
                raise HostControlError("original cancellation response names another original or state")
            return PrivateOriginalCancellation(original)
        except BaseException as error:
            self._fail(error)
            raise HostTransportError("original cancellation response was rejected") from error

    def apply_service_access_decision(self, decision: PrivateServiceAccessDecision, *, timeout: float | None = None) -> PrivateServiceGrantObservation:
        """Record an actual access decision before native issuance.

        This does not request another approval, install an integration, attach
        a tool or submit work. A timeout is unknown and is never retried here.
        An explicit exact retry observes the native host's retained decision.
        """
        if not isinstance(decision, PrivateServiceAccessDecision):
            raise HostControlError("invalid service access decision")
        wire = decision.wire()
        expected_digest = hashlib.sha256(json.dumps(wire, sort_keys=True, separators=(",", ":"), ensure_ascii=False, allow_nan=False).encode()).hexdigest()
        expected = PrivateServiceGrantRef(decision.route, decision.decision, expected_digest)
        reply = self._call({"kind": "apply_service_access_decision", "decision": wire}, "service_access_decision_applied", timeout=timeout)
        try:
            observed = _fields(reply["observation"], {"grant", "state"})
            grant = PrivateServiceGrantRef(**_fields(observed["grant"], {"route", "decision", "decision_sha256"}))
            state = PrivateServiceGrantState(observed["state"])
            if grant != expected:
                raise HostControlError("service access response names another decision")
            return PrivateServiceGrantObservation(grant, state)
        except BaseException as error:
            self._fail(error)
            raise HostTransportError("service access response was rejected") from error

    def open_component(self, component: str) -> HostComponentObservation:
        reply = self._call({"kind": "open_component", "component": component}, "component")
        try:
            observation = _component(reply["observation"])
            if observation.component != component:
                raise HostControlError("native response names another component")
            return observation
        except BaseException as error:
            self._fail(error)
            raise HostTransportError("native component observation was rejected") from error

    def _close_reply(self, command: dict[str, Any], scope: HostCloseScope,
                     handle: HostCloseHandle | None = None, *,
                     timeout: float | None = None) -> HostCloseObservation:
        reply = self._call(command, "close", timeout=timeout)
        try:
            observation = _close(reply["observation"])
            if observation.handle.scope != scope or (handle is not None and observation.handle != handle):
                raise HostControlError("native response names another close owner")
            return observation
        except BaseException as error:
            self._fail(error)
            raise HostTransportError("native close observation was rejected") from error

    def begin_close_component(self, component: str, reason: HostCloseReason) -> HostCloseObservation:
        return self._close_reply({"kind": "begin_close_component", "component": component,
                                  "reason": HostCloseReason(reason).value}, HostCloseScope("component", component))

    def begin_close_activation(self, reason: HostCloseReason) -> HostCloseObservation:
        return self._close_reply({"kind": "begin_close_activation", "reason": HostCloseReason(reason).value},
                                 HostCloseScope("activation"))

    def observe_close(self, handle: HostCloseHandle) -> HostCloseObservation:
        return self._close_reply({"kind": "observe_close", "handle": handle.wire()}, handle.scope, handle)

    def join_close(self, handle: HostCloseHandle, *, timeout: float | None = None) -> HostCloseObservation:
        return self._close_reply({"kind": "join_close", "handle": handle.wire()}, handle.scope, handle, timeout=timeout)

    def close(self) -> None:
        self._fail(HostControlError("local host pipe closed; settlement is unknown"))
        if threading.current_thread() is not self._reader:
            self._reader.join()

    def assert_available(self) -> None:
        """Fence a known failed/closed channel without asserting startup completion.

        A pending application preparation must still permit native init so its
        callbacks can run. This reads the existing failure owner, not authority.
        """
        with self._condition:
            if self._failure is not None or self._command_fd is None:
                raise HostTransportError("native host control unavailable") from self._failure


class ApplicationPreparationAttempt:
    """One pending private request and its exact later final selection.

    Observation and adoption results are retained on this existing process
    attempt. Repeating either Python call never writes a replacement request.
    A recorded final selection does not imply BootstrapAccepted or native Ready.
    """
    def __init__(self, client: PrivateHostClient, identifier: int, deadline: float):
        self._client = client
        self._identifier = identifier
        self._deadline = deadline
        self._lock = threading.Lock()
        self.manifest: SelectedHostArtifact | None = None
        self.selection: HostBootstrapSelection | None = None
        self._observed = False
        self._observation_error: BaseException | None = None
        self._adopted = False
        self._adoption_error: BaseException | None = None
        self.settlement: ApplicationPreparationSettlement | None = None

    def observe(self) -> SelectedHostArtifact:
        with self._lock:
            if not self._observed:
                try:
                    reply = self._client._await_call(self._identifier, self._deadline,
                        "application_prepared", {"kind", "manifest"})
                    try:
                        artifact = _fields(reply["manifest"], {"path", "sha256"})
                        if not isinstance(artifact["path"], str) or not os.path.isabs(artifact["path"]):
                            raise HostControlError("native application manifest path is invalid")
                        _source_digest(artifact["sha256"])
                        self.manifest = SelectedHostArtifact(**artifact)
                    except BaseException as error:
                        self._client._fail(error)
                        raise HostTransportError("native application result was rejected") from error
                except BaseException as error:
                    self._observation_error = error
                self._observed = True
            if self._observation_error is not None:
                raise self._observation_error
            assert self.manifest is not None
            return self.manifest

    def adopt(self, selection: HostBootstrapSelection) -> None:
        if not isinstance(selection, HostBootstrapSelection):
            raise HostControlError("final application selection is invalid")
        with self._lock:
            if self.manifest is None or self._observation_error is not None:
                raise HostControlError("application preparation has not completed")
            if self.selection is not None and self.selection != selection:
                raise HostControlError("application attempt already retained another final selection")
            if not self._adopted:
                # The application has already published this exact selection.
                # Retain it even if the write, reply or later native init fails.
                self.selection = selection
                try:
                    remaining = self._deadline - time.monotonic()
                    if remaining <= 0:
                        raise TimeoutError("application adoption observation budget expired; published selection is retained")
                    identifier, deadline = self._client._start_call(
                        {"kind": "bootstrap", "selection": dataclasses.asdict(selection)},
                        timeout=remaining, _expected_id=2,
                    )
                    self._client._await_call(identifier, deadline, "bootstrap_accepted")
                except BaseException as error:
                    self._adoption_error = error
                self._adopted = True
            if self._adoption_error is not None:
                raise self._adoption_error


@dataclass(frozen=True)
class ApplicationPreparationSettlement:
    """The actual joined outcomes retained by this one process attempt.

    Exceptions remain exceptions, including native unknown/durable-effect
    errors. A close result never implies rollback of the published selection.
    """
    native: Any
    finalization: Any
    private_close: Any


class ToolkitPersistentTransport(PersistentTransport):
    """Start the private owner before sending the ordinary MobKit init request."""

    def __init__(self, gateway_bin: str, selection: HostBootstrapSelection | ApplicationPreparation | ApplicationUpdateStartup, *,
                 env: dict[str, str] | None = None, timeout: float = 60.0,
                 initialize_native_owner: bool = False):
        if type(initialize_native_owner) is not bool:
            raise HostControlError("native owner initialization must be an explicit boolean")
        if os.name != "posix":
            raise HostControlError("Toolkit native host requires POSIX")
        super().__init__(gateway_bin, env=env, timeout=timeout, inherited_fds=(), extra_args=())
        self._selection = selection
        self.preparation: ApplicationPreparationAttempt | None = None
        self._preparation_started = False
        # The current ordinary-app gateway refuses this retained legacy
        # initialization flag. The first launch attempt consumes it, including
        # after a lost bootstrap response; later starts restore the existing
        # operator-provisioned grant and original owner stores.
        self._initialize_native_owner = initialize_native_owner
        self.host_control: PrivateHostClient | None = None
        self._host_stop_settled = False

    def start(self) -> None:
        """Begin private startup, preserving its exact failure for the caller.

        The native runtime owns composed stop after a failed startup. Direct
        transport callers must invoke this same transport's stop in finally.
        """
        if self._process is not None and self._process.poll() is None:
            if self.host_control is None:
                raise HostControlError("running gateway lacks its private native owner")
            return
        if self.host_control is not None:
            self.host_control.close()
            self.host_control = None
        if isinstance(self._selection, (ApplicationPreparation, ApplicationUpdateStartup)):
            if self._preparation_started:
                raise HostTransportError("application preparation already started; inspect the original attempt")
            self._preparation_started = True
        # Ordinary bootstrap may reuse this transport for a new child. Its
        # private join cannot inherit acknowledgement of the previous child.
        self._host_stop_settled = False
        temporary_fds: list[int] = []
        startup_error: BaseException | None = None
        try:
            child_read, parent_write = os.pipe()
            temporary_fds.extend((child_read, parent_write))
            parent_read, child_write = os.pipe()
            temporary_fds.extend((parent_read, child_write))
            self._inherited_fds = (child_read, child_write)
            self._extra_args = ("--host-control-read-fd", str(child_read),
                                "--host-control-write-fd", str(child_write))
            if self._initialize_native_owner:
                self._extra_args += ("--initialize-native-owner",)
            # Consume the retained flag before Popen so an automatic respawn
            # never retries the unsupported legacy initialization path.
            self._initialize_native_owner = False
            super().start()
            client = PrivateHostClient(parent_write, parent_read, timeout=self.request_timeout)
            temporary_fds = [child_read, child_write]
            self.host_control = client
        except BaseException as error:
            # The native failed-bootstrap owner must receive this exact error
            # and call composed stop, including after Popen created a child.
            startup_error = error
            raise
        finally:
            self._inherited_fds = ()
            self._extra_args = ()
            close_error: BaseException | None = None
            for fd in temporary_fds:
                try:
                    os.close(fd)
                except BaseException as error:
                    # Attempt each local descriptor once. A failed close does
                    # not authorize retrying its possibly reused number.
                    if close_error is None:
                        close_error = error
            if startup_error is None and close_error is not None:
                raise close_error
        try:
            if isinstance(self._selection, ApplicationPreparation):
                # Return after writing request 1. The application's same event
                # loop must be free to serve native SDK provider callbacks.
                self.preparation = client._begin_application_preparation(self._selection)
            elif isinstance(self._selection, ApplicationUpdateStartup):
                client.replace_application(self._selection)
            else:
                client.bootstrap(self._selection)
        except BaseException:
            try:
                client.close()
            except BaseException:
                # Keep the same client for composed stop to join. Local close
                # must not replace the startup error received by native cleanup.
                pass
            else:
                self.host_control = None
            raise

    def send_sync(self, request: dict[str, Any], *, timeout: float | None = None) -> Any:
        # Native send_async dispatches this method in its worker. A check only
        # before that dispatch would miss private failure while the worker waits.
        if request.get("method") == "meta/init":
            client = self.host_control
            if client is None:
                raise HostTransportError("native host control is unavailable")
            client.assert_available()
        return super().send_sync(request, timeout=timeout)

    def stop(self) -> None:
        process = self._process
        native_error: BaseException | None = None
        try:
            # Keep the private channel alive through gateway/runtime cleanup.
            super().stop()
        except BaseException as error:
            native_error = error
        termination = self.last_termination
        # Admission closes before physical exit. Both the original child and
        # native's retained record must permit joining its private endpoints.
        if (termination is None or termination.state == "unknown"
                or (process is not None and process.returncode is None)):
            if native_error is not None:
                raise native_error
            raise HostControlError("native gateway cleanup remains unknown; original child exit is unobserved")
        if self._host_stop_settled:
            # A prior call joined both owners and reported its outcome. The
            # same runtime can acknowledge it without erasing the record.
            if native_error is not None:
                raise native_error
            return
        if self.host_control is not None:
            try:
                self.host_control.close()
            except BaseException as private_error:
                if native_error is not None:
                    raise BaseExceptionGroup(
                        "native stop and private close both failed",
                        [native_error, private_error],
                    ) from native_error
                raise
            else:
                self.host_control = None
        self._host_stop_settled = True
        if native_error is not None:
            raise native_error
        if termination.state == "exited" and (
            termination.exit_code != 0 or termination.signal is not None
            or termination.escalation != "none"
            or termination.native_shutdown in ("refused", "unanswered")
        ):
            raise HostControlError(
                "native gateway exited without complete settlement: "
                f"exit={termination.exit_code}, signal={termination.signal}, "
                f"escalation={termination.escalation}, native_shutdown={termination.native_shutdown}"
            )


class ToolkitMobKitRuntime(MetaRuntime):
    """Local native Toolkit composition selected by the trusted Python host.

    Use with toolkit_gateway. Public init metadata cannot select this subclass
    or its exact bootstrap artifacts. The ordinary builder config is retained.
    """

    def __init__(self, config: Any, selection: HostBootstrapSelection | ApplicationPreparation | ApplicationUpdateStartup, *,
                 initialize_native_owner: bool = False):
        if not config.gateway_bin:
            raise HostControlError("Toolkit native host requires its local gateway")
        if type(initialize_native_owner) is not bool:
            raise HostControlError("native owner initialization must be an explicit boolean")
        super().__init__(config)
        self._host_selection = selection
        self._initialize_native_owner = initialize_native_owner

    def _make_gateway_transport(self) -> ToolkitPersistentTransport:
        if self._config.gateway_bin is None:
            raise HostControlError("Toolkit native host requires its local gateway")
        transport = ToolkitPersistentTransport(self._config.gateway_bin, self._host_selection,
                                               initialize_native_owner=self._initialize_native_owner)
        # A later runtime bootstrap uses restore, including when the first
        # transport lost an initialization reply. Missing state remains an error.
        self._initialize_native_owner = False
        return transport

    @property
    def host_control(self) -> PrivateHostClient:
        transport = self._transport
        if not isinstance(transport, ToolkitPersistentTransport) or transport.host_control is None:
            raise HostControlError("native host control is unavailable")
        return transport.host_control
