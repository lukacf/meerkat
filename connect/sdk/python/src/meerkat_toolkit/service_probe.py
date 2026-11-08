"""Typed private protocol 6 client for an explicitly selected trusted host.

The launcher is a host selection, not an authority certificate. The stock
--stdio entrypoint does not support protocol 6. No downgrade or replay occurs.
"""
from __future__ import annotations

import dataclasses
import enum
from pathlib import Path
import re
import time
from types import MappingProxyType
from typing import Any, Mapping

from .service_setup import (
    ServiceSetupBinding, ServiceSetupClient, ServiceSetupError, ServiceSetupEvent,
    _ENV, _EVENTS, _COMPLETIONS, _encoded, _text,
)


class ProbeStatus(str, enum.Enum):
    GOVERNED_PREREQUISITE_UNAVAILABLE = "governed_prerequisite_unavailable"
    SELECTION_CHANGED = "selection_changed"
    NOT_OBSERVED = "not_observed"
    NATIVE_REFUSED = "native_refused"
    OUTCOME_UNKNOWN = "outcome_unknown"
    TOOL_FAILED = "tool_failed"
    TOOL_COMPLETED = "tool_completed"
    CAPABILITY_NOT_VERIFIED = "capability_not_verified"
    CAPABILITY_VERIFIED = "capability_verified"
    CANCELLED = "cancelled"
    CLEANUP_UNCONFIRMED = "cleanup_unconfirmed"
    UNSUPPORTED = "unsupported"


class ProbeDispatchOutcome(str, enum.Enum):
    COMMITTED = "committed"
    FAILED = "failed"
    UNKNOWN = "unknown"


def _freeze(value: Any) -> Any:
    if isinstance(value, dict):
        return MappingProxyType({key: _freeze(item) for key, item in value.items()})
    if isinstance(value, list):
        return tuple(_freeze(item) for item in value)
    return value


@dataclasses.dataclass(frozen=True)
class ServiceProbeLauncher:
    """Host-selected command and persistence policy, snapshotted before spawn.

    command is executed exactly, without appended flags or inherited environment.
    The embedding must implement run_with_service_probes and connect its control
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
            raise ValueError("explicit private probe launcher required") from None
        object.__setattr__(self, "command", command)
        object.__setattr__(self, "credential_scope", MappingProxyType(scope))
        object.__setattr__(self, "environment", MappingProxyType(env))


@dataclasses.dataclass(frozen=True)
class ServiceProbeReport:
    """Native observation only; never a reusable grant or proof of live access."""

    profile_id: str
    application_id: str
    instance_id: str
    login_scope_id: str
    viewer_id: str
    origin: str
    realm: str
    session_id: str
    job_id: str
    server: str
    agent: str
    operation: str
    resource: Any = dataclasses.field(repr=False)
    status: ProbeStatus
    input_id: str | None
    run_id: str | None
    tool_call_id: str | None
    result_sha256: str | None
    installed_connection_sha256: str | None
    dispatch_outcome: ProbeDispatchOutcome | None
    native_terminal_committed: bool
    native_close_joined: bool


@dataclasses.dataclass(frozen=True)
class ServiceProbeEvent(ServiceSetupEvent):
    """Immutable private observation with an optional attributed typed report.

    viewer_current_at_observation is a local snapshot, not future permission.
    A later cancel/expiry can retire the binding without rewriting past facts.
    """

    report: ServiceProbeReport | None = dataclasses.field(default=None, repr=False)
    viewer_current_at_observation: bool = False


_PROBE_CAPS = {
    "selection": "trusted_process_profiles",
    "requires": "successful_finalization_and_governed_reopen",
    "result": "native_tool_completion_or_profile_verified_resource",
    "slack_inbound": "unsupported",
}
_REPORT_FIELDS = {field.name for field in dataclasses.fields(ServiceProbeReport)}
_STRINGS = {"profile_id", "application_id", "instance_id", "login_scope_id", "viewer_id", "origin",
            "realm", "session_id", "job_id", "server", "agent", "operation"}
_COMPLETE = {ProbeStatus.TOOL_COMPLETED, ProbeStatus.CAPABILITY_NOT_VERIFIED, ProbeStatus.CAPABILITY_VERIFIED}


class ServiceProbeClient(ServiceSetupClient):
    """One retained v6 process, sharing the v5 private transport owner.

    submit/answer/connector methods keep their existing closed vocabulary.
    probe accepts only two observed opaque selectors. It never accepts a profile,
    resource, operation, grant or continuation over the wire.
    """

    _protocol_version = 6
    _event_kinds = _EVENTS | {"service_probe_state", "service_commissioning"}

    def __init__(self, launcher: ServiceProbeLauncher, binding: ServiceSetupBinding):
        try:
            if type(launcher) is not ServiceProbeLauncher or type(binding) is not ServiceSetupBinding:
                raise ValueError()
            initial = binding._wire()
            if dict(binding.credential_scope) != dict(launcher.credential_scope):
                raise ValueError()
            initial["protocol_version"] = 6
            _encoded(initial)
        except Exception:
            raise ValueError("matching explicit probe binding and launcher required") from None
        self._binding_snapshot = _freeze(initial)
        self._session_id: str | None = None
        self._profiles: tuple[str, ...] | None = None
        self._probe_job: str | None = None
        self._can_probe = False
        self._can_send = False
        self._setup_available = False
        self._setup_retired = False
        self._commissioning = False
        self._commission_job: str | None = None
        self._commission_agent: str | None = None
        self._active_probe: str | None = None
        self._last_probe: str | None = None
        self._probe_requests: dict[str, tuple[str, str]] = {}
        self._attempted_profiles: set[str] = set()
        self._launch_inputs = (initial, list(launcher.command), dict(launcher.environment))
        super().__init__(launcher.command[0], binding, environment=dict(launcher.environment))

    def _launch(self, executable: str, binding: ServiceSetupBinding,
                environment: Mapping[str, str] | None) -> tuple[dict[str, Any], list[str], dict[str, str]]:
        # Already validated and copied before calling the same v5 constructor.
        return self._launch_inputs

    def probe(self, *, job_id: str, profile_id: str) -> str:
        """Queue one attempt. Returned ID is correlation, never success."""
        if not _text(job_id) or not _text(profile_id):
            raise ValueError("exact observed probe selectors required")
        with self._condition:
            self._current()
            if (self._scope is None or not self._can_probe or self._commissioning
                    or job_id != self._probe_job or self._profiles is None or profile_id not in self._profiles
                    or profile_id in self._attempted_profiles or self._active_probe is not None):
                raise ServiceSetupError("probe selection unavailable")
            identity = self._request("connect/probe", {"scope": self._scope, "job_id": job_id, "profile_id": profile_id})
            self._probe_requests[identity] = (job_id, profile_id)
            self._active_probe = identity
            self._attempted_profiles.add(profile_id)
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
        with self._condition:
            self._current()
            if (self._scope is not None and isinstance(send_request_id, str)
                    and (send_request_id == self._active_probe
                         or self._active_probe is None and send_request_id == self._last_probe)):
                try:
                    return self._request("connect/cancel", {"scope": self._scope, "send_request_id": send_request_id})
                except ServiceSetupError:
                    self._failure = True
                    self._end_input()
                    raise
                finally:
                    self._retired = True
            return super().cancel(send_request_id)

    def answer(self, prompt_id: str, answer: Mapping[str, Any]) -> None:
        try:
            super().answer(prompt_id, answer)
        except Exception:
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
        if params.get("reason") != "finalizing":
            return True
        if (self._scope is None or self._session_id is None
                or set(params) != {"target", "kind", "reason"}
                or params["target"] != {"scope": self._scope}):
            raise ServiceSetupError("invalid setup finalization fence")
        # Native retires the old input/connector owner before commissioning.
        # This exception never clears an earlier whole-binding retirement.
        self._setup_retired = True
        self._setup_available = self._can_send = False
        return False

    def _state(self, state: Any, *, early: bool = False) -> None:
        if not isinstance(state, dict):
            raise ServiceSetupError("invalid probe state")
        booleans = {"available", "can_send", "can_stop", "outcome_uncertain", "private_notice_pending", "commissioning", "can_probe"}
        fields = booleans | {"active_send", "last_completion", "active_probe", "probe_profiles"}
        if set(state) != fields or any(type(state[key]) is not bool for key in booleans):
            raise ServiceSetupError("invalid probe state")
        active_send, last = state["active_send"], state["last_completion"]
        if active_send is not None and (not isinstance(active_send, dict)
                or set(active_send) != {"send_request_id", "cancellation_requested"}
                or not _text(active_send["send_request_id"]) or type(active_send["cancellation_requested"]) is not bool):
            raise ServiceSetupError("invalid setup send observation")
        if last is not None and (not isinstance(last, dict) or set(last) != {"send_request_id", "completion"}
                or not _text(last["send_request_id"]) or not isinstance(last["completion"], str) or last["completion"] not in _COMPLETIONS):
            raise ServiceSetupError("invalid setup completion observation")
        if early and (any(state[key] for key in ("available", "can_send", "can_stop", "commissioning", "can_probe"))
                or active_send is not None or last is not None or state["active_probe"] is not None):
            raise ServiceSetupError("native owner unavailable before initialization")
        if (self._setup_retired and (state["available"] or state["can_send"])
                or state["commissioning"] and (not self._setup_retired or state["can_probe"])
                or state["can_probe"] and (self._probe_job is None or state["active_probe"] is not None or state["commissioning"])):
            raise ServiceSetupError("invalid native owner phase")
        profiles = state["probe_profiles"]
        if (not isinstance(profiles, list) or not 1 <= len(profiles) <= 16
                or any(not _text(profile) for profile in profiles) or len(set(profiles)) != len(profiles)):
            raise ServiceSetupError("invalid probe profile observation")
        profiles = tuple(profiles)
        if self._profiles is not None and self._profiles != profiles:
            raise ServiceSetupError("probe selection changed")
        active = state["active_probe"]
        if active is not None and (not _text(active) or active != self._active_probe):
            raise ServiceSetupError("foreign active probe")
        self._profiles = profiles
        self._can_probe, self._can_send = state["can_probe"], state["can_send"]
        self._setup_available, self._commissioning = state["available"], state["commissioning"]

    def _report(self, identity: str, value: Any) -> ServiceProbeReport:
        expected = self._probe_requests.get(identity)
        if expected is None or not isinstance(value, dict) or set(value) != _REPORT_FIELDS:
            raise ServiceSetupError("unmatched probe report")
        if any(not _text(value[key], 4096) for key in _STRINGS):
            raise ServiceSetupError("invalid probe attribution")
        snapshot = self._binding_snapshot
        match = {"profile_id": expected[1], "job_id": expected[0], "session_id": self._session_id,
                 "application_id": snapshot["application"]["application_id"],
                 "instance_id": snapshot["application"]["instance_id"], "origin": snapshot["application"]["origin"],
                 "viewer_id": snapshot["viewer"]["principal_id"], "login_scope_id": snapshot["viewer"]["login_scope_id"],
                 "realm": snapshot["realm"]["realm_id"]}
        if self._commission_agent is not None:
            match["agent"] = self._commission_agent
        if any(value[key] != selected for key, selected in match.items()):
            raise ServiceSetupError("foreign probe attribution")
        try:
            status = ProbeStatus(value["status"])
            dispatch = None if value["dispatch_outcome"] is None else ProbeDispatchOutcome(value["dispatch_outcome"])
        except (ValueError, TypeError):
            raise ServiceSetupError("unknown native probe outcome") from None
        for key in ("input_id", "run_id", "tool_call_id"):
            if value[key] is not None and not _text(value[key], 4096):
                raise ServiceSetupError("invalid native probe correlation")
        for key in ("result_sha256", "installed_connection_sha256"):
            if value[key] is not None and (not isinstance(value[key], str) or not re.fullmatch(r"[0-9a-f]{64}", value[key])):
                raise ServiceSetupError("invalid native probe digest")
        if any(type(value[key]) is not bool for key in ("native_terminal_committed", "native_close_joined")):
            raise ServiceSetupError("invalid native probe completion")
        if status in _COMPLETE and (dispatch is not ProbeDispatchOutcome.COMMITTED
                or not value["native_terminal_committed"] or not value["native_close_joined"]
                or any(value[key] is None for key in ("input_id", "run_id", "tool_call_id", "result_sha256", "installed_connection_sha256"))):
            raise ServiceSetupError("unconfirmed native probe completion")
        # Retirement or a close failure can supersede an observed unknown result.
        if dispatch is ProbeDispatchOutcome.UNKNOWN and status not in {
                ProbeStatus.OUTCOME_UNKNOWN, ProbeStatus.CANCELLED, ProbeStatus.CLEANUP_UNCONFIRMED}:
            raise ServiceSetupError("unknown effect cannot verify capability")
        fields = dict(value)
        fields.update(status=status, dispatch_outcome=dispatch, resource=_freeze(value["resource"]))
        return ServiceProbeReport(**fields)

    def _accept(self, value: Any, size: int) -> None:
        # The same condition covers validation, base-owner queueing and freezing:
        # no observer can see an unvalidated or mutable v6 result between them.
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
                    self._state(params.get("state"), early=self._scope is None)
                elif kind == "service_probe_state":
                    state = params.get("state")
                    attributed = state in ("available", "retired")
                    keys = {"target", "kind", "state", "capability_verified"} | ({"job_id", "session_id"} if attributed else set())
                    if (set(params) != keys or params.get("capability_verified") is not False
                            or state not in ("available", "retired", "unsupported_source", "finalization_not_selected")):
                        raise ServiceSetupError("invalid probe availability")
                    if attributed:
                        if (self._session_id is None or params["session_id"] != self._session_id
                                or not _text(params["job_id"]) or self._probe_job not in (None, params["job_id"])
                                or self._commission_job not in (None, params["job_id"])):
                            raise ServiceSetupError("foreign probe job")
                        self._probe_job = params["job_id"]
                    if state == "retired":
                        self._retired = True
                    if state != "available":
                        self._can_probe = False
                elif kind == "service_commissioning":
                    self._commission_observation(params)
            if isinstance(value, dict) and "result" in value and request is not None:
                result = value["result"]
                if not isinstance(result, dict):
                    raise ServiceSetupError("invalid native result")
                if request == "connect/initialize":
                    caps = result.get("capabilities")
                    if not isinstance(caps, dict) or caps.get("service_probes") != _PROBE_CAPS:
                        raise ServiceSetupError("explicit native probe protocol unavailable")
                    self._state(result.get("state"))
                elif request == "connect/probe":
                    if (set(result) != {"scope", "probe_request_id", "report", "state"}
                            or result.get("scope") != self._scope or result.get("probe_request_id") != identity
                            or identity != self._active_probe):
                        raise ServiceSetupError("foreign probe response")
                    typed = self._report(identity, result["report"])
                    self._state(result["state"])
                elif request == "connect/send":
                    self._state(result.get("state"))
            super()._accept(value, size)
            if request == "connect/initialize" and "result" in value:
                self._session_id = value["result"]["session_id"]
            if request == "connect/probe":
                self._active_probe = None
                self._probe_requests.pop(identity, None)
                if typed is not None:
                    self._last_probe = identity
            event, retained_size = self._events.pop()
            current = not (self._retired or self._closing or self._failure or self._deadline <= int(time.time() * 1000))
            self._events.append((ServiceProbeEvent(event.kind, event.request_id, _freeze(dict(event.payload)), typed, current), retained_size))

    def _commission_observation(self, params: dict[str, Any]) -> None:
        # The commissioning event is deliberately not a probe success signal.
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
                or self._probe_job not in (None, observed["job_id"])
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
        self._commission_job, self._commission_agent = observed["job_id"], observed["agent"]

    def next_event(self, timeout: float = 60) -> ServiceProbeEvent:
        event = super().next_event(timeout)
        assert isinstance(event, ServiceProbeEvent)
        return event
