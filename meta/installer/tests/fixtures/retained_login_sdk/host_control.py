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
from typing import Any
from uuid import UUID

from meerkat_meta._transport import PersistentTransport
from meerkat_meta.runtime import MetaRuntime

_PROTOCOL = 1
_MAX_FRAME = 65_536


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
    for entry in value["lineage"]:
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
    # Native data for the trusted compiler, never proof of current permission.
    return value


class RemoteControllerReview:
    """One retained review and its final result. No new thread or native session.

    Use review_id only on the originating PrivateHostClient. Observe and answer
    while result() waits in the application's normal asynchronous composition.
    Calling result again returns the same observation or error without a resend.
    """
    def __init__(self, client: PrivateHostClient, review_id: int, deadline: float, route: str):
        self.review_id = review_id
        self._client = client
        self._deadline = deadline
        self._route = route
        self._lock = threading.Lock()
        self._finished = False
        self._outcome: dict[str, Any] | None = None
        self._error: BaseException | None = None

    def result(self) -> dict[str, Any]:
        with self._lock:
            if not self._finished:
                try:
                    reply = self._client._await_call(self.review_id, self._deadline,
                                                    "controller_parent_reviewed", {"kind", "outcome"})
                    try:
                        self._outcome = _controller_outcome(reply["outcome"], self._route)
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

    def _start_call(self, command: dict[str, Any], *, timeout: float | None = None, _retirement: bool = False) -> tuple[int, float]:
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

    def _await_call(self, identifier: int, deadline: float, expected: str, reply_fields: set[str] | None = None) -> dict[str, Any]:
        with self._condition:
            while self._pending[identifier] is None and self._failure is None:
                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    # Keep the correlation until its late response or close. No
                    # request is reissued, and no result is treated as success.
                    self._abandoned.add(identifier)
                    raise TimeoutError("native host observation timed out; settlement is unknown")
                self._condition.wait(remaining)
            if self._failure is not None:
                raise HostTransportError("native host control failed; settlement is unknown") from self._failure
            result = self._pending.pop(identifier)
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


class ToolkitPersistentTransport(PersistentTransport):
    """Start the private owner before sending the ordinary MobKit init request."""

    def __init__(self, gateway_bin: str, selection: HostBootstrapSelection, *,
                 env: dict[str, str] | None = None, timeout: float = 60.0,
                 initialize_native_owner: bool = False):
        if type(initialize_native_owner) is not bool:
            raise HostControlError("native owner initialization must be an explicit boolean")
        if os.name != "posix":
            raise HostControlError("Toolkit native host requires POSIX")
        super().__init__(gateway_bin, env=env, timeout=timeout, inherited_fds=(), extra_args=())
        self._selection = selection
        # The current ordinary-app gateway refuses this retained legacy
        # initialization flag. The first launch attempt consumes it, including
        # after a lost bootstrap response; later starts restore the existing
        # operator-provisioned grant and original owner stores.
        self._initialize_native_owner = initialize_native_owner
        self.host_control: PrivateHostClient | None = None

    def start(self) -> None:
        if self._process is not None and self._process.poll() is None:
            if self.host_control is None:
                raise HostControlError("running gateway lacks its private native owner")
            return
        if self.host_control is not None:
            self.host_control.close()
            self.host_control = None
        child_read, parent_write = os.pipe()
        try:
            parent_read, child_write = os.pipe()
        except BaseException:
            os.close(child_read)
            os.close(parent_write)
            raise
        parent_fds = [parent_write, parent_read]
        try:
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
            parent_fds.clear()
            self.host_control = client
        except BaseException:
            # Failure after Popen must also reap the child. Closing the parent
            # writer below first makes its native owner observe host loss.
            for fd in parent_fds:
                os.close(fd)
            parent_fds.clear()
            super().stop()
            raise
        finally:
            os.close(child_read)
            os.close(child_write)
            self._inherited_fds = ()
            self._extra_args = ()
            for fd in parent_fds:
                os.close(fd)
        try:
            client.bootstrap(self._selection)
        except BaseException:
            client.close()
            self.host_control = None
            # The native child sees private EOF and joins its owner. Ordinary
            # transport stop reaps it; a failed join stays an error.
            super().stop()
            raise

    def stop(self) -> None:
        process = self._process
        try:
            # Keep the private channel alive through gateway/runtime cleanup.
            super().stop()
            # Before public init, the ordinary SDK uses its legacy EOF fallback
            # and does not attest runtime cleanup. Our exact wrapper returns 0
            # only after both native and gateway cleanup succeed. A signal,
            # forced kill or bootstrap error cannot become successful teardown.
            if process is not None and process.returncode != 0:
                raise HostControlError("native gateway exited without complete settlement")
        finally:
            if self.host_control is not None:
                self.host_control.close()
                self.host_control = None


class ToolkitMobKitRuntime(MetaRuntime):
    """Local native Toolkit composition selected by the trusted Python host.

    Use with toolkit_gateway. Public init metadata cannot select this subclass
    or its exact bootstrap artifacts. The ordinary builder config is retained.
    """

    def __init__(self, config: Any, selection: HostBootstrapSelection, *,
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
