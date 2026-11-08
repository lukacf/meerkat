"""Typed private protocol 7 client for an explicitly selected trusted host.

The launcher is a host selection, not an authority certificate. The stock
--stdio entrypoint does not support protocol 7. No downgrade or replay occurs. Programs and credentials stay with the native host.
"""
from __future__ import annotations

import dataclasses
import enum
import json
from pathlib import Path
import re
import time
from types import MappingProxyType
from typing import Any, Mapping, Literal

from .service_setup import (
    ServiceSetupBinding, ServiceSetupClient, ServiceSetupError, ServiceSetupEvent,
    _ENV, _EVENTS, _COMPLETIONS, _ERROR_CODES, _encoded, _text,
)


from .service_probe import ServiceProbeReport, ProbeStatus, ProbeDispatchOutcome
from .service_collection_transport import ServiceCollectionTransport


class CollectionStatus(str, enum.Enum):
    HISTORICAL_CHECKS_COMPLETED = "historical_checks_completed"
    NOT_VERIFIED = "not_verified"
    NOT_OBSERVED = "not_observed"
    NATIVE_STOPPED = "native_stopped"
    GOVERNED_PREREQUISITE_UNAVAILABLE = "governed_prerequisite_unavailable"
    NATIVE_REFUSED = "native_refused"
    OUTCOME_UNKNOWN = "outcome_unknown"
    TOOL_FAILED = "tool_failed"
    CLEANUP_UNCONFIRMED = "cleanup_unconfirmed"
    UNSUPPORTED = "unsupported"
    SELECTION_CHANGED = "selection_changed"
    CANCELLED = "cancelled"
    PROGRAM_REFUSED = "program_refused"
    PROGRAM_PANICKED = "program_panicked"
    ATTEMPT_ALREADY_OBSERVED = "attempt_already_observed"
    REPLAY_REFUSED = "replay_refused"
    BUDGET_EXCEEDED = "budget_exceeded"


def _freeze(value: Any) -> Any:
    if isinstance(value, dict):
        return MappingProxyType({key: _freeze(item) for key, item in value.items()})
    if isinstance(value, list):
        return tuple(_freeze(item) for item in value)
    return value


@dataclasses.dataclass(frozen=True)
class ServiceCollectionLauncher:
    """Host-selected command and persistence policy, snapshotted before spawn.

    command is executed exactly, without appended flags or inherited environment.
    The embedding must implement run_with_service_collections and connect its control
    to actual host logout. A successful handshake checks only the wire protocol.
    """

    command: tuple[str, ...] = dataclasses.field(repr=False)
    credential_scope: Mapping[str, str] = dataclasses.field(repr=False)
    environment: Mapping[str, str] = dataclasses.field(repr=False)

    def __post_init__(self) -> None:
        try:
            command = tuple(self.command)
            scope, env = dict(self.credential_scope), dict(self.environment)
            if (not command or len(command) > 128
                    or any(not isinstance(arg, str) or "\0" in arg for arg in command)
                    or not Path(command[0]).is_absolute()
                    or sum(len(arg.encode("utf-8")) for arg in command) > 65536):
                raise ValueError()
            native = scope == {"kind": "native_default"}
            if not native and (set(scope) != {"kind", "root"} or scope["kind"] != "file"
                    or not isinstance(scope["root"], str) or "\0" in scope["root"]
                    or not Path(scope["root"]).is_absolute()):
                raise ValueError()
            allowed = _ENV | {"HOME", "XDG_CONFIG_HOME", "XDG_DATA_HOME"} if native else _ENV
            if set(env) - allowed or any(not isinstance(v, str) or "\0" in v for v in env.values()):
                raise ValueError()
            if native and (not isinstance(env.get("HOME"), str) or not Path(env["HOME"]).is_absolute()):
                raise ValueError()
            # Validate encoding before any owned process exists.
            _encoded({"command": command, "scope": scope, "environment": env})
        except Exception:
            raise ValueError("explicit private collection launcher required") from None
        object.__setattr__(self, "command", command)
        object.__setattr__(self, "credential_scope", MappingProxyType(scope))
        object.__setattr__(self, "environment", MappingProxyType(env))


@dataclasses.dataclass(frozen=True)
class ServiceCollectionState:
    """Last native projection, never current resource or credential authority."""

    available: bool
    can_send: bool
    can_stop: bool
    outcome_uncertain: bool
    private_notice_pending: bool
    active_send: Mapping[str, Any] | None = dataclasses.field(repr=False)
    last_completion: Mapping[str, Any] | None = dataclasses.field(repr=False)
    commissioning: bool
    can_collect: bool
    active_collection: str | None
    collection_sources: tuple[str, ...] | None
    reusable_source_coverage: Literal["unqualified_credential_continuity"]
    service_ids: tuple[str, ...] | None = None
    service_id: str | None = None
    can_open_next_service: bool = False


@dataclasses.dataclass(frozen=True)
class CollectionLimits:
    calls: int
    argument_bytes: int
    result_bytes: int


@dataclasses.dataclass(frozen=True)
class CollectionCallObservation:
    source_id: str
    required_operation_id: str
    resource: Any = dataclasses.field(repr=False)
    ordinal: int
    raw_operation: str
    arguments_sha256: str
    native: ServiceProbeReport = dataclasses.field(repr=False)


@dataclasses.dataclass(frozen=True)
class RequiredOperationObservation:
    required_operation_id: str
    resource: Any = dataclasses.field(repr=False)
    raw_operation_ceiling: tuple[str, ...]
    assessment: Literal["verified", "not_verified", "unknown"] | None
    calls: tuple[CollectionCallObservation, ...] = dataclasses.field(repr=False)


@dataclasses.dataclass(frozen=True)
class ServiceCollectionReport:
    """Historical native facts, never reusable credential coverage or a grant."""

    source_id: str
    attempt_id: str
    application_id: str
    instance_id: str
    login_scope_id: str
    viewer_id: str
    origin: str
    realm: str
    session_id: str
    job_id: str
    family: str
    profile: str
    agent: str
    status: CollectionStatus
    requirements: tuple[RequiredOperationObservation, ...] = dataclasses.field(repr=False)
    observed_selection: Literal["not_observed", "rechecked_after_last_close", "changed_or_unconfirmed"]
    native_own_config_generation: int | None
    reusable_source_coverage: Literal["unqualified_credential_continuity"]
    limits: CollectionLimits
    total_calls: int
    total_argument_bytes: int
    total_result_bytes: int


@dataclasses.dataclass(frozen=True)
class ServiceCollectionEvent(ServiceSetupEvent):
    """Immutable private observation with an optional attributed typed report.

    viewer_current_at_observation is a local snapshot, not future permission.
    A later cancel/expiry can retire the binding without rewriting past facts.
    """

    report: ServiceCollectionReport | None = dataclasses.field(default=None, repr=False)
    viewer_current_at_observation: bool = False


_COLLECTION_CAPS = {
    "selection": "trusted_process_sources",
    "requires": "successful_finalization_and_governed_reopen",
    "result": "historical_required_operation_observations",
    "reusable_source_coverage": "unqualified_credential_continuity",
    "slack_inbound": "unsupported",
}
_REPORT_FIELDS = {field.name for field in dataclasses.fields(ServiceCollectionReport)}
_STRINGS = {"source_id", "attempt_id", "application_id", "instance_id", "login_scope_id", "viewer_id", "origin",
            "realm", "session_id", "job_id", "family", "profile", "agent"}
_COVERAGE = "unqualified_credential_continuity"


def _integer(value: Any, maximum: int = (1 << 64) - 1) -> bool:
    return type(value) is int and 0 <= value <= maximum


def _json_key(value: Any) -> str:
    return json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False, allow_nan=False)


def _resource(value: Any) -> bool:
    return value is not None and len(_json_key(value).encode("utf-8")) <= 4096


def _digest(value: Any) -> bool:
    return isinstance(value, str) and re.fullmatch(r"[0-9a-f]{64}", value) is not None


def _native(value: Any, expected: dict[str, Any]) -> ServiceProbeReport:
    """Check transport attribution, without evaluating the host's predicate."""
    if not isinstance(value, dict) or set(value) != {f.name for f in dataclasses.fields(ServiceProbeReport)}:
        raise ServiceSetupError("invalid nested native observation")
    for key in ("profile_id", "application_id", "instance_id", "login_scope_id", "viewer_id", "origin",
                "realm", "session_id", "job_id", "server", "agent", "operation"):
        if not _text(value[key], 4096):
            raise ServiceSetupError("invalid nested native attribution")
    if any(_json_key(value[key]) != _json_key(item) for key, item in expected.items()):
        raise ServiceSetupError("foreign nested native attribution")
    try:
        status = ProbeStatus(value["status"])
        dispatch = None if value["dispatch_outcome"] is None else ProbeDispatchOutcome(value["dispatch_outcome"])
    except (ValueError, TypeError):
        raise ServiceSetupError("unknown nested native outcome") from None
    if (any(value[key] is not None and not _text(value[key], 4096) for key in ("input_id", "run_id", "tool_call_id"))
            or any(value[key] is not None and not _digest(value[key]) for key in ("result_sha256", "installed_connection_sha256"))
            or any(type(value[key]) is not bool for key in ("native_terminal_committed", "native_close_joined"))):
        raise ServiceSetupError("invalid nested native completion")
    complete = {ProbeStatus.TOOL_COMPLETED, ProbeStatus.CAPABILITY_VERIFIED, ProbeStatus.CAPABILITY_NOT_VERIFIED}
    if status in complete and (dispatch is not ProbeDispatchOutcome.COMMITTED
            or not value["native_terminal_committed"] or not value["native_close_joined"]
            or any(value[key] is None for key in ("input_id", "run_id", "tool_call_id", "result_sha256", "installed_connection_sha256"))):
        raise ServiceSetupError("unconfirmed nested native completion")
    if dispatch is ProbeDispatchOutcome.UNKNOWN and status not in {
            ProbeStatus.OUTCOME_UNKNOWN, ProbeStatus.CANCELLED, ProbeStatus.CLEANUP_UNCONFIRMED}:
        raise ServiceSetupError("unknown effect cannot verify a collection")
    fields = dict(value)
    fields.update(status=status, dispatch_outcome=dispatch, resource=_freeze(value["resource"]))
    return ServiceProbeReport(**fields)


class ServiceCollectionClient(ServiceSetupClient):
    """One retained v7 process, sharing the v5 private transport owner.

    submit/answer/connector methods keep their existing closed vocabulary.
    collect accepts only two observed opaque selectors. It never accepts a profile,
    resource, operation, grant or continuation over the wire.
    """

    _protocol_version = 7
    _error_codes = _ERROR_CODES | {"collection_unavailable", "collection_already_attempted"}
    _event_kinds = _EVENTS | {"service_collection_state", "service_commissioning"}

    def __init__(self, launcher: ServiceCollectionLauncher, binding: ServiceSetupBinding):
        try:
            if type(launcher) is not ServiceCollectionLauncher or type(binding) is not ServiceSetupBinding:
                raise ValueError()
            initial = binding._wire()
            if dict(binding.credential_scope) != dict(launcher.credential_scope):
                raise ValueError()
            initial["protocol_version"] = 7
            _encoded(initial)
        except Exception:
            raise ValueError("matching explicit collection binding and launcher required") from None
        self._configure(initial)
        self._launch_inputs = (initial, list(launcher.command), dict(launcher.environment))
        super().__init__(launcher.command[0], binding, environment=dict(launcher.environment))

    @classmethod
    def adopt(cls, transport: ServiceCollectionTransport, binding: ServiceSetupBinding) -> ServiceCollectionClient:
        """Use the original root-launched pipes, with no child spawn or fallback.

        The root must already have authenticated and activated this exact child.
        Invalid inputs before transfer leave transport with the host. A failure
        after transfer raises ServiceSetupLaunchError with this same SDK owner;
        retain it and observe close again. Root retirement/settlement remain
        independent responsibilities even when the SDK close observation passes.
        """
        try:
            if cls is not ServiceCollectionClient or type(transport) is not ServiceCollectionTransport or type(binding) is not ServiceSetupBinding:
                raise ValueError()
            initial = binding._wire()
            if dict(binding.credential_scope) != dict(transport.credential_scope):
                raise ValueError()
            initial["protocol_version"] = 7
            _encoded(initial)
        except Exception:
            raise ValueError("matching original collection transport and binding required") from None
        owner = cls.__new__(cls)
        owner._configure(initial)
        owner._initialize_owner(initial)
        token = object()
        try:
            owner._external_transport = True
            owner._process = transport._claim(token)
            owner._start_workers()
        except BaseException:
            if owner._process is None:
                owner._process = transport._claimed_owner(token)
                if owner._process is None:
                    raise ServiceSetupError("collection transport not adopted") from None
            owner._startup_failed()
        return owner

    def _configure(self, initial: dict[str, Any]) -> None:
        self._binding_snapshot = _freeze(initial)
        self._session_id: str | None = None
        self._state_snapshot: ServiceCollectionState | None = None
        self._sources: tuple[str, ...] | None = None
        self._collection_job: str | None = None
        self._can_collect = False
        self._can_send = False
        self._setup_available = False
        self._setup_retired = False
        self._commissioning = False
        self._commission_job: str | None = None
        self._commission_agent: str | None = None
        self._active_collection: str | None = None
        self._last_collection: str | None = None
        self._collection_requests: dict[str, tuple[str, str]] = {}
        self._attempted_sources: set[str] = set()
        self._refused_selections: set[tuple[str, str]] = set()
        self._active_collection_observed = False
        self._attempt_ids: set[str] = set()
        self._service_ids: tuple[str, ...] | None = None
        self._service_id: str | None = None
        self._pending_service: tuple[str, str] | None = None
        self._service_sessions: set[str] = set()
        self._service_jobs: set[str] = set()
        self._first_fence: dict[str, Any] | None = None
        self._can_open_next_service = False

    def _launch(self, executable: str, binding: ServiceSetupBinding,
                environment: Mapping[str, str] | None) -> tuple[dict[str, Any], list[str], dict[str, str]]:
        # Already validated and copied before calling the same v5 constructor.
        return self._launch_inputs

    @property
    def state(self) -> ServiceCollectionState | None:
        with self._condition:
            return self._state_snapshot

    @property
    def viewer_current(self) -> bool:
        """Local owner observation only; it never qualifies credential continuity."""
        with self._condition:
            return not (self._retired or self._closing or self._failure or self._deadline <= int(time.time() * 1000))

    def collect(self, *, job_id: str, source_id: str) -> str:
        """Queue one attempt. Returned ID is correlation, never success."""
        if not _text(job_id) or not _text(source_id):
            raise ValueError("exact observed collection selectors required")
        with self._condition:
            self._current()
            if (self._scope is None or not self._can_collect or self._commissioning or self._pending_service is not None
                    or job_id != self._collection_job or self._sources is None or source_id not in self._sources
                    or source_id in self._attempted_sources or (job_id, source_id) in self._refused_selections
                    or self._active_collection is not None):
                raise ServiceSetupError("collection selection unavailable")
            identity = self._request("connect/collect", {"scope": self._scope, "job_id": job_id, "source_id": source_id})
            self._collection_requests[identity] = (job_id, source_id)
            self._active_collection = identity
            self._active_collection_observed = False
            self._attempted_sources.add(source_id)
            return identity

    def open_next_service(self, *, service_id: str) -> str:
        """Open the next preselected service on these same original pipes.

        Only an observed service ID is sent. The native owner checks completed
        setup/collection joins before advancing. This starts no new process and
        supplies no caller, grant, receipt, account identity or service metadata.
        A refused or lost transition is never replayed automatically.
        """
        if not _text(service_id):
            raise ValueError("exact observed next service required")
        with self._condition:
            self._current()
            if (self._scope is None or not self._can_open_next_service
                    or self._service_ids is None or self._service_id not in self._service_ids
                    or self._pending_service is not None or self._requests
                    or self._collection_requests or self._active_collection is not None):
                raise ServiceSetupError("next service unavailable")
            index = self._service_ids.index(self._service_id) + 1
            if index >= len(self._service_ids) or self._service_ids[index] != service_id:
                raise ServiceSetupError("next service unavailable")
            identity = self._request("connect/collection/open_next", {"scope": self._scope, "service_id": service_id})
            self._pending_service = (identity, service_id)
            # Old state frames may already be queued on the original pipe.
            # Keep their phase until the first observed next-service frame.
            self._can_open_next_service = False
            return identity

    def submit(self, text: str) -> str:
        with self._condition:
            if self._setup_retired or not self._can_send or self._commissioning:
                raise ServiceSetupError("setup input owner unavailable")
            return super().submit(text)

    def _scoped(self, method: str, params: dict[str, Any]) -> str:
        with self._condition:
            if self._setup_retired or not self._setup_available or self._commissioning:
                raise ServiceSetupError("setup connector owner unavailable")
            return super()._scoped(method, params)

    def cancel(self, send_request_id: str) -> str:
        if not _text(send_request_id):
            raise ServiceSetupError("exact current request required")
        try:
            with self._condition:
                self._current()
                # Once next setup is queued, an earlier collection is no longer
                # a cancellation target, even before its first state arrives.
                if (self._scope is not None and self._pending_service is None and isinstance(send_request_id, str)
                        and (send_request_id == self._active_collection
                             or self._active_collection is None and send_request_id == self._last_collection)):
                    try:
                        return self._request("connect/cancel", {"scope": self._scope, "send_request_id": send_request_id})
                    except ServiceSetupError:
                        self._failure = True
                        self._end_input()
                        raise
                    finally:
                        self._retired = True
            return super().cancel(send_request_id)
        except ServiceSetupError:
            if self._failure:
                self._retire_external()
            raise

    def answer(self, prompt_id: str, answer: Mapping[str, Any]) -> None:
        try:
            super().answer(prompt_id, answer)
        except Exception:
            if self._failure:
                self._retire_external()
            raise ServiceSetupError("protected service answer refused") from None

    def connector_start(self, *, job_id: str, connector: Mapping[str, Any], redirect_uri: str,
                        operations: Mapping[str, Any]) -> str:
        try:
            return super().connector_start(job_id=job_id, connector=connector,
                                           redirect_uri=redirect_uri, operations=operations)
        except Exception:
            raise ServiceSetupError("connector start refused") from None

    def connector_complete(self, *, state: str, code: str, host_attempt_handle: str | None = None) -> str:
        try:
            return super().connector_complete(state=state, code=code, host_attempt_handle=host_attempt_handle)
        except Exception:
            raise ServiceSetupError("connector completion refused") from None

    def _unavailable_retires(self, params: Mapping[str, Any]) -> bool:
        reason = params.get("reason")
        if (set(params) not in ({"target", "kind", "reason"}, {"target", "kind", "reason", "first_fence"})
                or not _text(reason)):
            raise ServiceSetupError("invalid native retirement fence")
        if reason == "finalizing" and (self._scope is None or self._session_id is None
                or params["target"] != {"scope": self._scope}):
            raise ServiceSetupError("invalid setup finalization fence")
        if "first_fence" in params:
            fence = params["first_fence"]
            # Native may add diagnostic labels without changing authority or
            # the protocol. Retain the bounded text, never interpret its value.
            if (not isinstance(fence, dict) or set(fence) != {"origin", "scope", "send_request_id", "cancellation_requested_before"}
                    or not _text(fence["origin"]) or fence["scope"] != self._scope
                    or fence["send_request_id"] is not None and not _text(fence["send_request_id"])
                    or fence["cancellation_requested_before"] is not None and type(fence["cancellation_requested_before"]) is not bool
                    or self._first_fence is not None and self._first_fence != fence):
                raise ServiceSetupError("invalid first fence observation")
            self._first_fence = dict(fence)
        if reason != "finalizing":
            return True
        # Native retires the old input/connector owner before commissioning.
        # This exception never clears an earlier whole-binding retirement.
        self._setup_retired = True
        self._setup_available = self._can_send = False
        return False

    def _state(self, state: Any, *, early: bool = False) -> None:
        if not isinstance(state, dict):
            raise ServiceSetupError("invalid collection state")
        booleans = {"available", "can_send", "can_stop", "outcome_uncertain", "private_notice_pending", "commissioning", "can_collect"}
        fields = booleans | {"active_send", "last_completion", "active_collection", "collection_sources", "reusable_source_coverage"}
        service_fields = {"service_ids", "service_id", "can_open_next_service"}
        if set(state) not in (fields, fields | service_fields) or any(type(state[key]) is not bool for key in booleans):
            raise ServiceSetupError("invalid collection state")
        if service_fields <= set(state):
            services = state["service_ids"]
            current = state["service_id"]
            if (not isinstance(services, list) or not 2 <= len(services) <= 16
                    or any(not _text(item) for item in services) or len(set(services)) != len(services)
                    or current not in services or type(state["can_open_next_service"]) is not bool):
                raise ServiceSetupError("invalid service selection")
            services = tuple(services)
            pending = self._pending_service[1] if self._pending_service is not None else None
            expected = {self._service_id or services[0]}
            if pending is not None:
                expected.add(pending)
            if (self._service_ids is not None and services != self._service_ids
                    or current not in expected or state["can_open_next_service"] and (
                        current == services[-1] or not state["can_collect"] or current == pending)):
                raise ServiceSetupError("foreign service transition")
            if current == pending and current != self._service_id:
                # Native sent the accepted transition before polling the next
                # setup. Retain all earlier attempts/events and original scope.
                self._session_id = self._collection_job = self._commission_job = self._commission_agent = None
                self._setup_retired = self._setup_available = self._can_send = self._can_collect = False
                self._commissioning = False
                self._last_collection = None
            self._service_ids, self._service_id = services, current
            self._can_open_next_service = state["can_open_next_service"] and pending is None
        elif self._service_ids is not None:
            raise ServiceSetupError("service selection disappeared")
        if state["reusable_source_coverage"] != _COVERAGE:
            raise ServiceSetupError("credential continuity is unqualified")
        active_send, last = state["active_send"], state["last_completion"]
        if active_send is not None and (not isinstance(active_send, dict)
                or set(active_send) != {"send_request_id", "cancellation_requested"}
                or not _text(active_send["send_request_id"]) or type(active_send["cancellation_requested"]) is not bool):
            raise ServiceSetupError("invalid setup send observation")
        if last is not None and (not isinstance(last, dict) or set(last) != {"send_request_id", "completion"}
                or not _text(last["send_request_id"]) or not isinstance(last["completion"], str) or last["completion"] not in _COMPLETIONS):
            raise ServiceSetupError("invalid setup completion observation")
        if early and self._session_id is None and (any(state[key] for key in ("available", "can_send", "can_stop", "commissioning", "can_collect"))
                or active_send is not None or last is not None or state["active_collection"] is not None):
            raise ServiceSetupError("native owner unavailable before initialization")
        if (self._setup_retired and (state["available"] or state["can_send"])
                or state["commissioning"] and (not self._setup_retired or state["can_collect"])
                or state["can_collect"] and (self._collection_job is None or state["active_collection"] is not None or state["commissioning"])):
            raise ServiceSetupError("invalid native owner phase")
        sources = state["collection_sources"]
        if sources is None:
            if self._sources is not None or state["can_collect"] or state["active_collection"] is not None:
                raise ServiceSetupError("collection selection disappeared")
        else:
            if (not isinstance(sources, list) or not 1 <= len(sources) <= 16
                    or any(not _text(source) for source in sources) or len(set(sources)) != len(sources)):
                raise ServiceSetupError("invalid collection source observation")
            sources = tuple(sources)
            if self._sources is not None and self._sources != sources:
                raise ServiceSetupError("collection selection changed")
            self._sources = sources
        active = state["active_collection"]
        if active is not None and (not _text(active) or active != self._active_collection):
            raise ServiceSetupError("foreign active collection")
        self._can_collect, self._can_send = state["can_collect"], state["can_send"]
        self._setup_available, self._commissioning = state["available"], state["commissioning"]
        fields = dict(state)
        fields.update(active_send=_freeze(active_send), last_completion=_freeze(last), collection_sources=sources)
        if "service_ids" in fields:
            fields["service_ids"] = self._service_ids
        self._state_snapshot = ServiceCollectionState(**fields)
        if active is not None:
            self._active_collection_observed = True

    def _report(self, identity: str, value: Any) -> ServiceCollectionReport:
        selected = self._collection_requests.get(identity)
        if selected is None or not isinstance(value, dict) or set(value) != _REPORT_FIELDS:
            raise ServiceSetupError("unmatched collection report")
        if any(not _text(value[key], 4096) for key in _STRINGS):
            raise ServiceSetupError("invalid collection attribution")
        snapshot = self._binding_snapshot
        expected = {"source_id": selected[1], "job_id": selected[0], "session_id": self._session_id,
                    "application_id": snapshot["application"]["application_id"],
                    "instance_id": snapshot["application"]["instance_id"], "origin": snapshot["application"]["origin"],
                    "viewer_id": snapshot["viewer"]["principal_id"], "login_scope_id": snapshot["viewer"]["login_scope_id"],
                    "realm": snapshot["realm"]["realm_id"]}
        if self._commission_agent is not None:
            expected["agent"] = self._commission_agent
        if any(value[key] != item for key, item in expected.items()):
            raise ServiceSetupError("foreign collection attribution")
        attempt = value["attempt_id"]
        if (re.fullmatch(r"[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}", attempt) is None
                or attempt in self._attempt_ids):
            raise ServiceSetupError("invalid collection attempt attribution")
        try:
            status = CollectionStatus(value["status"])
        except (ValueError, TypeError):
            raise ServiceSetupError("unknown collection status") from None
        if (value["reusable_source_coverage"] != _COVERAGE
                or value["observed_selection"] not in ("not_observed", "rechecked_after_last_close", "changed_or_unconfirmed")
                or value["native_own_config_generation"] is not None and not _integer(value["native_own_config_generation"])):
            raise ServiceSetupError("invalid historical collection selection")
        limits = value["limits"]
        maxima = {"calls": 256, "argument_bytes": 8 * 1024 * 1024, "result_bytes": 32 * 1024 * 1024}
        if (not isinstance(limits, dict) or set(limits) != set(maxima)
                or any(not _integer(limits[k], bound) or limits[k] < 1 for k, bound in maxima.items())
                or any(not _integer(value[k]) for k in ("total_calls", "total_argument_bytes", "total_result_bytes"))
                or value["total_calls"] > limits["calls"] or value["total_argument_bytes"] > limits["argument_bytes"]):
            raise ServiceSetupError("invalid collection budget observation")
        rows = value["requirements"]
        if not isinstance(rows, list) or not 1 <= len(rows) <= 64:
            raise ServiceSetupError("invalid collection census")
        retained, ids, count = [], set(), 0
        native_calls, call_keys = set(), set()
        for row in rows:
            if (not isinstance(row, dict) or set(row) != {"required_operation_id", "resource", "raw_operation_ceiling", "assessment", "calls"}
                    or not _text(row["required_operation_id"]) or row["required_operation_id"] in ids
                    or not _resource(row["resource"]) or row["assessment"] not in (None, "verified", "not_verified", "unknown")):
                raise ServiceSetupError("invalid required operation observation")
            ids.add(row["required_operation_id"])
            ceiling, calls = row["raw_operation_ceiling"], row["calls"]
            if (not isinstance(ceiling, list) or not 1 <= len(ceiling) <= 32
                    or any(not _text(op) for op in ceiling) or len(set(ceiling)) != len(ceiling)
                    or not isinstance(calls, list) or len(calls) > limits["calls"]):
                raise ServiceSetupError("invalid required operation ceiling")
            observed = []
            for call in calls:
                count += 1
                if (not isinstance(call, dict) or set(call) != {"source_id", "required_operation_id", "resource", "ordinal", "raw_operation", "arguments_sha256", "native"}
                        or call["source_id"] != value["source_id"] or call["required_operation_id"] != row["required_operation_id"]
                        or _json_key(call["resource"]) != _json_key(row["resource"])
                        or not _integer(call["ordinal"]) or call["ordinal"] != count
                        or not isinstance(call["raw_operation"], str) or call["raw_operation"] not in ceiling
                        or not _digest(call["arguments_sha256"])):
                    raise ServiceSetupError("foreign collection call")
                native_expected = {k: value[k] for k in ("application_id", "instance_id", "login_scope_id", "viewer_id", "origin", "realm", "session_id", "job_id", "agent")}
                native_expected.update(profile_id=row["required_operation_id"], operation=call["raw_operation"], resource=row["resource"])
                native = _native(call["native"], native_expected)
                correlation = (native.input_id, native.run_id, native.tool_call_id)
                # Native ReobserveRead may repeat the arguments with a new
                # observation, including partial admission on failure. Dedup
                # observations here; only the native owner can admit a read.
                key = (row["required_operation_id"], call["raw_operation"], call["arguments_sha256"], correlation)
                if key in call_keys or None not in correlation and correlation in native_calls:
                    raise ServiceSetupError("replayed collection call observation")
                call_keys.add(key)
                if None not in correlation:
                    native_calls.add(correlation)
                observed.append(CollectionCallObservation(call["source_id"], call["required_operation_id"], _freeze(call["resource"]), count, call["raw_operation"], call["arguments_sha256"], native))
            if row["assessment"] == "verified" and (not observed or any(c.native.status is not ProbeStatus.TOOL_COMPLETED for c in observed)):
                raise ServiceSetupError("unconfirmed required operation assessment")
            retained.append(RequiredOperationObservation(row["required_operation_id"], _freeze(row["resource"]), tuple(ceiling), row["assessment"], tuple(observed)))
        # A reserved call can fail before producing an observation. Results may
        # exceed their budget; the native owner retains the actual byte count.
        if count > value["total_calls"]:
            raise ServiceSetupError("inconsistent collection call count")
        if status is CollectionStatus.HISTORICAL_CHECKS_COMPLETED and (
                count != value["total_calls"] or any(r.assessment != "verified" for r in retained)
                or value["observed_selection"] != "rechecked_after_last_close"
                or value["native_own_config_generation"] is None
                or value["total_result_bytes"] > limits["result_bytes"]):
            raise ServiceSetupError("unconfirmed historical collection completion")
        self._attempt_ids.add(attempt)
        fields = dict(value)
        fields.update(status=status, requirements=tuple(retained), limits=CollectionLimits(**limits))
        return ServiceCollectionReport(**fields)

    def _accept(self, value: Any, size: int) -> None:
        # The same condition covers validation, base-owner queueing and freezing:
        # no observer can see an unvalidated or mutable v7 result between them.
        with self._condition:
            typed = None
            identity = value.get("id") if isinstance(value, dict) else None
            request = self._requests.get(identity) if isinstance(identity, str) else None
            if isinstance(value, dict) and value.get("method") == "connect/prompt" and self._setup_retired:
                inactive = self._retired or self._closing or self._deadline <= int(time.time() * 1000)
                params = value.get("params")
                prompt = params.get("prompt") if isinstance(params, dict) else None
                if not inactive and (not self._commissioning or not isinstance(prompt, dict) or prompt.get("kind") != "approval"):
                    raise ServiceSetupError("setup prompt owner retired")
            if isinstance(value, dict) and value.get("method") == "connect/event":
                params = value.get("params")
                if not isinstance(params, dict) or params.get("target") != self._target():
                    raise ServiceSetupError("foreign service observation")
                kind = params.get("kind")
                if kind == "state":
                    self._state(params.get("state"), early=True)
                elif kind == "service_collection_state":
                    state = params.get("state")
                    attributed = state in ("available", "retired")
                    keys = {"target", "kind", "state", "capability_verified"} | ({"job_id", "session_id"} if attributed else set())
                    if (set(params) != keys or params.get("capability_verified") is not False
                            or state not in ("available", "retired", "unsupported_source", "finalization_not_selected")):
                        raise ServiceSetupError("invalid collection availability")
                    if self._session_id is None or not self._setup_retired:
                        raise ServiceSetupError("collection owner not initialized")
                    if attributed:
                        if (self._session_id is None or params["session_id"] != self._session_id
                                or not _text(params["job_id"]) or self._collection_job not in (None, params["job_id"])
                                or self._commission_job not in (None, params["job_id"])):
                            raise ServiceSetupError("foreign collection job")
                        if self._collection_job is None and params["job_id"] in self._service_jobs:
                            raise ServiceSetupError("reused service installation job")
                        self._service_jobs.add(params["job_id"])
                        self._collection_job = params["job_id"]
                    if state != "available":
                        self._retired = True
                        self._can_collect = False
                elif kind == "service_commissioning":
                    self._commission_observation(params)
            if isinstance(value, dict) and "result" in value and request is not None:
                result = value["result"]
                if not isinstance(result, dict):
                    raise ServiceSetupError("invalid native result")
                if request == "connect/initialize":
                    caps = result.get("capabilities")
                    if not isinstance(caps, dict) or caps.get("service_collections") != _COLLECTION_CAPS:
                        raise ServiceSetupError("explicit native collection protocol unavailable")
                    self._state(result.get("state"))
                elif request == "connect/collection/open_next":
                    if (self._pending_service is None or self._pending_service[0] != identity
                            or set(result) != {"scope", "service_id", "session_id", "state"}
                            or result["scope"] != self._scope or result["service_id"] != self._pending_service[1]
                            or not isinstance(result["state"], dict) or result["state"].get("service_id") != result["service_id"]
                            or not _text(result["session_id"]) or result["session_id"] in self._service_sessions
                            or self._retired or self._closing or self._failure):
                        raise ServiceSetupError("foreign service setup continuation")
                    self._state(result["state"])
                elif request == "connect/collect":
                    if (set(result) != {"scope", "collection_request_id", "report", "state"}
                            or result.get("scope") != self._scope or result.get("collection_request_id") != identity
                            or identity != self._active_collection):
                        raise ServiceSetupError("foreign collection response")
                    typed = self._report(identity, result["report"])
                    self._state(result["state"])
                    if result["state"]["active_collection"] is not None:
                        raise ServiceSetupError("collection response still active")
                elif request == "connect/send":
                    self._state(result.get("state"))
            # The base reader appends exactly one normalized event for this
            # frame. Our enclosing condition lock keeps it at the queue tail
            # until refusal handling and the typed-event replacement below finish.
            super()._accept(value, size)
            if request in ("connect/initialize", "connect/collection/open_next") and "result" in value:
                self._session_id = value["result"]["session_id"]
                self._service_sessions.add(self._session_id)
            if request == "connect/collection/open_next":
                self._pending_service = None
                if "error" in value:
                    self._retired = True
                    self._can_send = self._can_collect = self._can_open_next_service = False
            if request == "connect/collect":
                selected = self._collection_requests.pop(identity, None)
                event = self._events[-1][0]
                if (selected is not None and not self._active_collection_observed
                        and event.kind == "refused" and event.payload["code"] == "invalid_binding"):
                    # begin_collection refuses a foreign service source before
                    # admission. Keep that job/source blocked, while allowing an
                    # explicit selection under a later native installation job.
                    self._refused_selections.add(selected)
                    self._attempted_sources.remove(selected[1])
                self._active_collection = None
                self._active_collection_observed = False
                if typed is not None:
                    self._last_collection = identity
            event, retained_size = self._events.pop()
            current = not (self._retired or self._closing or self._failure or self._deadline <= int(time.time() * 1000))
            self._events.append((ServiceCollectionEvent(event.kind, event.request_id, _freeze(dict(event.payload)), typed, current), retained_size))

    def _commission_observation(self, params: dict[str, Any]) -> None:
        # The commissioning event is deliberately not a collection success signal.
        if (set(params) != {"target", "kind", "state", "observation", "capability_verified"}
                or params.get("state") != "observed" or params.get("capability_verified") is not False
                or not isinstance(params.get("observation"), dict)):
            raise ServiceSetupError("invalid commissioning observation")
        observed = params["observation"]
        fields = {"realm", "session_id", "job_id", "agent", "state", "governance", "grants",
                  "controller_disposal", "issuance", "custody_released", "capability_verified"}
        snapshot = self._binding_snapshot
        if (set(observed) != fields or self._session_id is None or not self._setup_retired
                or observed["realm"] != snapshot["realm"]["realm_id"]
                or observed["session_id"] != self._session_id
                or any(not _text(observed[key], 4096) for key in ("job_id", "agent"))
                or self._commission_job not in (None, observed["job_id"])
                or self._collection_job not in (None, observed["job_id"])
                or observed["state"] not in ("completed", "incomplete")
                or observed["governance"] not in ("not_entered", "entered", "committed", "outcome_unknown", "already_governed")
                or observed["controller_disposal"] not in (None, "not_prepared", "drained", "taken", "already_retired")
                or observed["issuance"] not in (None, "not_entered", "committed", "uncertain")
                or type(observed["custody_released"]) is not bool or observed["capability_verified"] is not False):
            raise ServiceSetupError("foreign or invalid commissioning observation")
        grants = observed["grants"]
        if not isinstance(grants, list) or len(grants) > 2:
            raise ServiceSetupError("invalid commissioning grant observations")
        for grant in grants:
            if (not isinstance(grant, dict) or set(grant) != {"state", "issued", "candidate_grant_id", "superseded", "cleanup"}
                    or grant["state"] not in ("committed", "refused", "issued_not_selected", "issuance_unknown", "selection_unknown", "outcome_unknown")
                    or any(grant[key] is not None and not _text(grant[key], 4096) for key in ("issued", "candidate_grant_id", "superseded"))
                    or not isinstance(grant["cleanup"], list)
                    or any(item not in ("revoked", "controller_in_use", "deferred", "references_unknown", "custody_unavailable", "refused", "uncertain") for item in grant["cleanup"])):
                raise ServiceSetupError("invalid commissioning grant observation")
        if self._commission_job is not None:
            raise ServiceSetupError("duplicate commissioning observation")
        self._commission_job, self._commission_agent = observed["job_id"], observed["agent"]

    def next_event(self, timeout: float = 60) -> ServiceCollectionEvent:
        event = super().next_event(timeout)
        assert isinstance(event, ServiceCollectionEvent)
        return event
