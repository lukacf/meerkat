"""Host-owned Connect Services v5 transport and protected presentation.

This client owns one real native child. Its private events are for the trusted
host, not model messages or logs. Configuration, credentials, grants, and actual
resource-operation results remain distinct native facts. No readiness is inferred.
"""
from __future__ import annotations

import collections
import copy
import dataclasses
import io
import json
import math
import os
import re
import subprocess
import threading
import time
import unicodedata
from pathlib import Path
from typing import Any, Mapping
from urllib.parse import urlsplit

_MAX_FRAME = 64 * 1024 * 1024
_MAX_QUEUE = 128
_MAX_WAIT = 2_147_483.647
_ENV = frozenset(("PATH", "LANG", "LC_ALL", "TMPDIR", "TMP", "TEMP", "SYSTEMROOT"))
_PROMPTS = frozenset(("approval", "provider_target", "provider_action", "provider_api_key",
                      "provider_browser", "provider_device", "service_secret"))
_EVENTS = frozenset(("state", "conversation", "prompt_closed", "unavailable", "service_finalization"))
_ERROR_CODES = frozenset(("invalid_binding", "unavailable", "binding_retired", "native_auth_required",
    "connector_unavailable", "connector_refused", "invalid_message", "turn_uncertain", "busy",
    "stale_prompt", "invalid_answer", "stop_uncertain", "shutdown_unconfirmed", "required_context_missing",
    "installation_metadata_required", "installation_unconfirmed", "unsupported_native_entry"))
_COMPLETIONS = frozenset(("completed", "completed_without_result", "callback_pending", "callback_batch_pending",
    "cancelled", "abandoned", "abandoned_with_error", "completed_with_finalization_failure",
    "runtime_terminated", "outcome_unknown"))


class ServiceSetupError(RuntimeError):
    """Value-free failure; never retain native/provider diagnostic bodies."""


class ServiceSetupLaunchError(ServiceSetupError):
    """Post-spawn/adoption failure. Retain owner and close that same lifetime."""

    def __init__(self, owner: ServiceSetupClient):
        super().__init__("service transport startup failed; native owner retained")
        self.owner = owner


def _text(value: Any, limit: int = 256) -> bool:
    return _utf8(value, limit) and not any(unicodedata.category(c) == "Cc" for c in value)


def _utf8(value: Any, limit: int) -> bool:
    if not isinstance(value, str):
        return False
    try:
        return 0 < len(value.encode("utf-8")) <= limit
    except UnicodeError:
        return False


def _wait(value: float) -> float:
    if type(value) not in (float, int) or not math.isfinite(value) or not 0 < value <= _MAX_WAIT:
        raise ValueError("invalid observation timeout")
    return time.monotonic() + value


def _unique(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    result: dict[str, Any] = {}
    for key, value in pairs:
        if key in result:
            raise ServiceSetupError("duplicate native frame field")
        result[key] = value
    return result


def _invalid_constant(_: str) -> Any:
    raise ServiceSetupError("invalid native JSON value")


def _encoded(value: Mapping[str, Any]) -> bytes:
    try:
        data = json.dumps(value, ensure_ascii=True, separators=(",", ":"), allow_nan=False).encode() + b"\n"
    except (TypeError, ValueError, OverflowError):
        raise ServiceSetupError("invalid private request") from None
    if len(data) > _MAX_FRAME:
        raise ServiceSetupError("private request exceeds frame limit")
    return data


@dataclasses.dataclass(frozen=True)
class ServiceSetupBinding:
    """Captured by the authenticated host, never decoded from browser input."""

    application_id: str
    instance_id: str
    origin: str
    principal_id: str
    login_scope_id: str
    expires_at_ms: int
    realm_id: str
    context_root: str
    state_root: str
    user_config_root: str
    resource_manifest_sha256: str
    credential_scope: Mapping[str, Any] = dataclasses.field(repr=False)
    adapter: str = "services"
    model: str | None = None
    auth_binding: Mapping[str, Any] | None = dataclasses.field(default=None, repr=False)

    def _wire(self) -> dict[str, Any]:
        if any(not _text(getattr(self, name)) for name in
               ("application_id", "instance_id", "principal_id", "login_scope_id", "realm_id")):
            raise ValueError("invalid trusted service binding")
        if self.adapter not in ("services", "forge_services"):
            raise ValueError("service adapter required")
        if type(self.expires_at_ms) is not int or not int(time.time() * 1000) < self.expires_at_ms <= 9_007_199_254_740_991:
            raise ValueError("current finite viewer deadline required")
        try:
            origin = urlsplit(self.origin)
            valid_origin = origin.scheme in ("http", "https") and bool(origin.hostname) and not origin.username and not origin.password
            valid_origin &= not origin.query and not origin.fragment and origin.path in ("", "/")
            _ = origin.port
        except (TypeError, ValueError):
            valid_origin = False
        if not valid_origin or not _text(self.origin, 4096):
            raise ValueError("invalid trusted application origin")
        for name in ("context_root", "state_root", "user_config_root"):
            value = getattr(self, name)
            if not isinstance(value, str) or not Path(value).is_absolute() or "\0" in value:
                raise ValueError("trusted absolute realm roots required")
        if not isinstance(self.resource_manifest_sha256, str) or not re.fullmatch("[0-9a-f]{64}", self.resource_manifest_sha256):
            raise ValueError("exact resource manifest digest required")
        if self.model is not None and not _text(self.model):
            raise ValueError("invalid model selection")
        if self.auth_binding is not None:
            auth = self.auth_binding
            if (not isinstance(auth, Mapping) or not {"realm", "binding"} <= set(auth)
                    or set(auth) - {"realm", "binding", "profile"}
                    or any(not _text(value) for value in auth.values())):
                raise ValueError("closed native auth binding required")
        scope = self.credential_scope
        if type(scope) is not dict or scope != {"kind": "native_default"}:
            if type(scope) is not dict or set(scope) != {"kind", "root"} or scope.get("kind") != "file" or not isinstance(scope.get("root"), str) or not Path(scope["root"]).is_absolute() or "\0" in scope["root"]:
                raise ValueError("explicit trusted credential scope required")
        result: dict[str, Any] = {
            "protocol_version": 5, "adapter": self.adapter,
            "application": {"application_id": self.application_id, "instance_id": self.instance_id, "origin": self.origin},
            "viewer": {"principal_id": self.principal_id, "login_scope_id": self.login_scope_id, "expires_at_ms": self.expires_at_ms},
            "realm": {"realm_id": self.realm_id, "context_root": self.context_root,
                      "state_root": self.state_root, "user_config_root": self.user_config_root},
            "services": {"resource_manifest_sha256": self.resource_manifest_sha256, "preparation": "permitted"},
        }
        for name in ("model", "auth_binding"):
            value = getattr(self, name)
            if value is not None:
                result[name] = value
        # Freeze a JSON copy before spawn; callers cannot mutate the initialization.
        return json.loads(_encoded(result))


@dataclasses.dataclass(frozen=True)
class ServiceSetupEvent:
    """Private host delivery. repr intentionally omits all protected payloads."""

    kind: str
    request_id: str | None
    payload: Mapping[str, Any] = dataclasses.field(repr=False)


@dataclasses.dataclass(frozen=True)
class ServiceSetupClose:
    process_joined: bool
    exit_code: int | None
    native_resources_closed: bool


class ServiceSetupClient:
    """One Services v5 child, retained through timeout, cancellation and close.

    Submit returns a request ID, not its result. Read next_event to obtain actual
    native responses and protected prompts. A timeout never authorizes resubmission.
    Keep draining observations during close: retained events apply backpressure
    at the count or byte bound. Without a consumer, close can time out while the
    same owner remains retained. Calling close again never replaces that owner.
    """

    _protocol_version = 5
    _error_codes = _ERROR_CODES
    _event_kinds = _EVENTS

    def _launch(self, executable: str, binding: ServiceSetupBinding,
                environment: Mapping[str, str] | None) -> tuple[dict[str, Any], list[str], dict[str, str]]:
        """Pre-spawn preparation; the constructor below retains child ownership."""
        initial = binding._wire()
        if not isinstance(executable, str) or not Path(executable).is_absolute() or "\0" in executable:
            raise ValueError("trusted absolute native executable required")
        scope = dict(binding.credential_scope)
        native_default = scope["kind"] == "native_default"
        if native_default and environment is None:
            raise ValueError("native-default launch requires an explicit fixed environment")
        env = {"PATH": os.defpath, "LANG": "C"} if environment is None else dict(environment)
        allowed = _ENV | {"HOME", "XDG_CONFIG_HOME", "XDG_DATA_HOME"} if native_default else _ENV
        if set(env) - allowed or any(not isinstance(v, str) or "\0" in v for v in env.values()):
            raise ValueError("private launch must exclude ambient credential overrides")
        if native_default and (not isinstance(env.get("HOME"), str) or not Path(env["HOME"]).is_absolute()):
            raise ValueError("native-default launch requires fixed absolute HOME")
        args = [executable, "--stdio"]
        if not native_default:
            args += ["--provider-store-root", scope["root"]]
        return initial, args, env

    def __init__(self, executable: str, binding: ServiceSetupBinding, *, environment: Mapping[str, str] | None = None):
        initial, args, env = self._launch(executable, binding, environment)
        self._initialize_owner(initial)
        try:
            self._process = subprocess.Popen(args, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                stderr=subprocess.DEVNULL, env=env, close_fds=True, start_new_session=True, bufsize=0)
            self._start_workers()
        except BaseException:
            self._startup_failed()

    def _initialize_owner(self, initial: dict[str, Any]) -> None:
        """Prepare queues before acquiring a process or transferring pipes."""
        self._deadline = initial["viewer"]["expires_at_ms"]
        self._condition = threading.Condition()
        self._outbound: collections.deque[bytes] = collections.deque()
        self._outbound_bytes = 0
        self._input_stopping = False
        self._reader_done = False
        self._writer_done = False
        self._events: collections.deque[tuple[ServiceSetupEvent, int]] = collections.deque()
        self._queued_bytes = 0
        self._requests: dict[str, str] = {"h:initialize": "connect/initialize"}
        self._send_references: dict[str, str] = {}
        self._counter = 0
        self._scope: str | None = None
        self._prompt: tuple[str, dict[str, Any]] | None = None
        self._answered = False
        self._retired = False
        self._closing = False
        self._joined = False
        self._failure = False
        self._native_closed = False
        self._close_id: str | None = None
        self._close_target: dict[str, str] | None = None
        self._process: subprocess.Popen[bytes] | None = None
        self._external_transport = False
        self._reader = threading.Thread(target=self._read, name="connect-services-owner", daemon=True)
        self._writer = threading.Thread(target=self._write_loop, name="connect-services-input", daemon=True)
        frame = _encoded({"jsonrpc": "2.0", "id": "h:initialize", "method": "connect/initialize", "params": initial})
        self._enqueue(frame)

    def _start_workers(self) -> None:
        self._writer.start()
        self._reader.start()

    def _startup_failed(self) -> None:
        if self._process is None:
            raise ServiceSetupError("service setup launch failed") from None
        with self._condition:
            self._failure = True
            self._retired = True
            self._reader_done = self._reader_done or self._reader.ident is None
            self._writer_done = self._writer_done or self._writer.ident is None
        self._end_input()
        self._retire_external()
        for worker, pipe in ((self._writer, self._process.stdin), (self._reader, self._process.stdout)):
            if worker.ident is None and pipe is not None:
                # No worker touched this unbuffered pipe: no flush or wait.
                try:
                    pipe.close()
                except BaseException:
                    pass
        raise ServiceSetupLaunchError(self) from None

    def _retire_external(self) -> None:
        if self._external_transport and self._process is not None:
            try:
                self._process.request_retirement()
            except BaseException:
                with self._condition:
                    self._failure = True
                    self._retired = True
                    self._condition.notify_all()

    def _target(self) -> dict[str, str]:
        return {"scope": self._scope} if self._scope is not None else {"initialize_request_id": "h:initialize"}

    def _current(self) -> None:
        if self._retired or self._closing or self._joined or self._failure or self._deadline <= int(time.time() * 1000):
            raise ServiceSetupError("service binding unavailable")

    def _enqueue(self, frame: bytes) -> None:
        """Caller holds condition, except construction before threads exist."""
        if self._input_stopping or len(self._outbound) >= _MAX_QUEUE or self._outbound_bytes + len(frame) > _MAX_FRAME:
            raise ServiceSetupError("private transport queue unavailable")
        self._outbound.append(frame)
        self._outbound_bytes += len(frame)

    def _write(self, frame: bytes) -> None:
        """Only the retained writer calls this potentially blocking operation."""
        assert self._process is not None
        if self._process.stdin is None or self._process.stdin.closed:
            raise ServiceSetupError("service input closed")
        try:
            remaining = memoryview(frame)
            while remaining:
                written = self._process.stdin.write(remaining[:65536])
                if not written:
                    raise ServiceSetupError("service input closed")
                remaining = remaining[written:]
        except (BrokenPipeError, OSError, ValueError):
            raise ServiceSetupError("service request outcome unknown") from None

    def _end_input(self) -> None:
        with self._condition:
            self._input_stopping = True
            self._outbound_bytes -= sum(map(len, self._outbound))
            self._outbound.clear()
            self._condition.notify_all()

    def _write_loop(self) -> None:
        assert self._process is not None and self._process.stdin is not None
        try:
            while True:
                with self._condition:
                    while not self._outbound and not self._input_stopping:
                        self._condition.wait()
                    if self._input_stopping:
                        return
                    frame = self._outbound.popleft()
                try:
                    self._write(frame)
                finally:
                    with self._condition:
                        self._outbound_bytes -= len(frame)
                        frame = b""
                        self._condition.notify_all()
        except BaseException:
            with self._condition:
                self._failure = True
                self._retired = True
                self._condition.notify_all()
            self._end_input()
            self._retire_external()
        finally:
            try:
                self._process.stdin.close()
            except (OSError, ValueError):
                pass
            with self._condition:
                self._writer_done = True
                self._condition.notify_all()

    def _request(self, method: str, params: Mapping[str, Any], *, control: bool = False) -> str:
        with self._condition:
            if not control:
                self._current()
            if self._joined or self._failure or len(self._requests) >= 256:
                raise ServiceSetupError("service request unavailable")
            if method == "connect/send" and "connect/send" in self._requests.values():
                raise ServiceSetupError("a native setup send is already owned")
            self._counter += 1
            request_id = f"h:{self._counter}"
            frame = _encoded({"jsonrpc": "2.0", "id": request_id, "method": method, "params": params})
            self._enqueue(frame)
            self._requests[request_id] = method
            if method == "connect/cancel":
                self._send_references[request_id] = params["send_request_id"]
            self._condition.notify_all()
        return request_id

    def submit(self, text: str) -> str:
        if not isinstance(text, str) or not text.strip():
            raise ValueError("nonempty setup message required")
        with self._condition:
            self._current()
            scope = self._scope
            if scope is None or "connect/send" in self._requests.values():
                raise ServiceSetupError("service setup is not available for another message")
        return self._request("connect/send", {"scope": scope, "text": text})

    def cancel(self, send_request_id: str) -> str:
        with self._condition:
            self._current()
            scope = self._scope
            if scope is None or self._requests.get(send_request_id) != "connect/send":
                raise ServiceSetupError("setup send is not current")
            try:
                return self._request("connect/cancel", {"scope": scope, "send_request_id": send_request_id})
            except ServiceSetupError:
                self._failure = True
                self._end_input()
                raise
            finally:
                self._retired = True

    def connector_start(self, *, job_id: str, connector: Mapping[str, Any], redirect_uri: str,
                        operations: Mapping[str, Any]) -> str:
        """Trusted host selections only. Native validates the actual checked job."""
        if not _text(job_id, 4096) or not _text(redirect_uri, 4096):
            raise ValueError("exact connector job and redirect required")
        connector = _connector_target(connector)
        operations = _connector_operations(operations)
        return self._scoped("connect/connector/login/start", {
            "job_id": job_id, "connector": connector, "redirect_uri": redirect_uri, "operations": operations})

    def connector_complete(self, *, state: str, code: str, host_attempt_handle: str | None = None) -> str:
        if not _text(state, 16384) or not _text(code, 16384) or host_attempt_handle is not None and not _text(host_attempt_handle, 4096):
            raise ValueError("closed connector completion required")
        params = {"state": state, "code": code}
        if host_attempt_handle is not None:
            params["host_attempt_handle"] = host_attempt_handle
        return self._scoped("connect/connector/login/complete", params)

    def connector_cancel(self) -> str:
        return self._scoped("connect/connector/login/cancel", {})

    def _scoped(self, method: str, params: dict[str, Any]) -> str:
        with self._condition:
            self._current()
            if self._scope is None:
                raise ServiceSetupError("service initialization pending")
            params["scope"] = self._scope
        return self._request(method, params)

    def answer(self, prompt_id: str, answer: Mapping[str, Any]) -> None:
        """Queue one protected answer; retain only its bounded transport frame."""
        with self._condition:
            self._current()
            if self._prompt is None or self._prompt[0] != prompt_id or self._answered:
                raise ServiceSetupError("protected prompt is not current")
            prompt = self._prompt[1]
            value = dict(answer)
            if not _answer_matches(prompt, value):
                raise ServiceSetupError("invalid protected answer")
            frame = _encoded({"jsonrpc": "2.0", "id": prompt_id, "result": {"answer": value}})
            cancelling = value == {"kind": "cancel"}
            if cancelling:
                self._retired = True
            try:
                self._enqueue(frame)
            except ServiceSetupError:
                if cancelling:
                    self._failure = True
                    self._end_input()
                raise
            self._answered = True
            self._condition.notify_all()

    def _unavailable_retires(self, params: Mapping[str, Any]) -> bool:
        return True

    def _accept(self, value: Any, size: int) -> None:
        if not isinstance(value, dict) or value.get("jsonrpc") != "2.0":
            raise ServiceSetupError("invalid native service frame")
        with self._condition:
            method, identity = value.get("method"), value.get("id")
            if method in ("connect/prompt", "connect/event"):
                expected_keys = {"jsonrpc", "method", "params"} | ({"id"} if method == "connect/prompt" else set())
                params = value.get("params")
                if set(value) != expected_keys or not isinstance(params, dict) or params.get("target") != self._target():
                    raise ServiceSetupError("foreign service observation")
                if method == "connect/prompt":
                    prompt = params.get("prompt")
                    if set(params) != {"target", "prompt"} or not isinstance(prompt, dict) or prompt.get("kind") not in _PROMPTS:
                        raise ServiceSetupError("unsupported protected prompt")
                    if not isinstance(identity, str) or not re.fullmatch(r"p:[A-Za-z0-9_-]{1,64}", identity) or self._prompt is not None:
                        raise ServiceSetupError("overlapping protected prompt")
                    # Native may have emitted this prompt before observing a
                    # queued stop. Preserve its closure correlation, not a new
                    # actionable prompt or late private browser/device payload.
                    inactive = self._retired or self._closing or self._deadline <= int(time.time() * 1000)
                    self._prompt = (identity, {} if inactive else copy.deepcopy(prompt))
                    self._answered = False
                    event = (ServiceSetupEvent("retired_prompt", identity, {"prompt_kind": prompt["kind"]})
                             if inactive else ServiceSetupEvent("prompt", identity, prompt))
                else:
                    kind = params.get("kind")
                    if kind not in self._event_kinds:
                        raise ServiceSetupError("unknown service event")
                    if kind == "prompt_closed":
                        if self._prompt is None or params.get("prompt_id") != self._prompt[0]:
                            raise ServiceSetupError("foreign protected prompt closure")
                        self._prompt = None
                        self._answered = False
                    elif kind == "unavailable" and self._unavailable_retires(params):
                        self._retired = True
                    event = ServiceSetupEvent(kind, None, params)
            else:
                if method is not None or not isinstance(identity, str):
                    raise ServiceSetupError("invalid native response")
                request = self._requests.get(identity)
                if request is None or set(value) not in ({"jsonrpc", "id", "result"}, {"jsonrpc", "id", "error"}):
                    raise ServiceSetupError("unmatched native response")
                del self._requests[identity]
                send_reference = self._send_references.pop(identity, None)
                if "error" in value:
                    error = value["error"]
                    if not isinstance(error, dict) or type(error.get("code")) is not int:
                        raise ServiceSetupError("invalid native refusal")
                    # Never forward arbitrary native/provider error text.
                    code = error.get("data", {}).get("code") if isinstance(error.get("data", {}), dict) else None
                    payload = {"method": request, "code": code if isinstance(code, str) and code in self._error_codes else "native_refused"}
                    event = ServiceSetupEvent("refused", identity, payload)
                else:
                    result = value["result"]
                    if not isinstance(result, dict):
                        raise ServiceSetupError("invalid native result")
                    if request == "connect/initialize":
                        if set(result) != {"protocol_version", "scope", "session_id", "capabilities", "state"} or result["protocol_version"] != self._protocol_version or not _text(result["scope"]) or not _text(result["session_id"]):
                            raise ServiceSetupError(f"Services v{self._protocol_version} initialization required")
                        capabilities = result["capabilities"]
                        if not isinstance(capabilities, dict) or capabilities.get("service_preparation") is not True or set(capabilities.get("prompt_kinds", [])) != _PROMPTS:
                            raise ServiceSetupError("protected service capabilities unavailable")
                        self._scope = result["scope"]
                    elif request == "connect/close":
                        if result != {"target": self._close_target, "resources_closed": True}:
                            raise ServiceSetupError("unconfirmed service cleanup")
                        self._native_closed = True
                    else:
                        if result.get("scope") != self._scope:
                            raise ServiceSetupError("foreign native result")
                        if request == "connect/send":
                            if (set(result) != {"scope", "send_request_id", "completion", "state"}
                                    or result["send_request_id"] != identity or result["completion"] not in _COMPLETIONS
                                    or not isinstance(result["state"], dict)):
                                raise ServiceSetupError("unmatched native send completion")
                        elif request == "connect/cancel":
                            if (set(result) != {"scope", "send_request_id", "status"}
                                    or result["send_request_id"] != send_reference
                                    or result["status"] not in ("cancellation_requested", "not_current")):
                                raise ServiceSetupError("unmatched native cancellation")
                        elif request == "connect/connector/login/start":
                            if set(result) != {"scope", "started"} or not isinstance(result["started"], dict):
                                raise ServiceSetupError("invalid connector start observation")
                        elif request == "connect/connector/login/complete":
                            if set(result) != {"scope", "completed"} or not isinstance(result["completed"], dict):
                                raise ServiceSetupError("invalid connector completion observation")
                        elif request == "connect/connector/login/cancel":
                            if result != {"scope": self._scope, "cancelled": True}:
                                raise ServiceSetupError("invalid connector cancellation observation")
                    event = ServiceSetupEvent("result", identity, {"method": request, "result": result})
            if len(self._events) >= _MAX_QUEUE or self._queued_bytes + size > _MAX_FRAME:
                raise ServiceSetupError("service observer fell behind")
            self._events.append((event, size))
            self._queued_bytes += size
            self._condition.notify_all()

    def _read(self) -> None:
        assert self._process is not None and self._process.stdout is not None
        try:
            # stdin is unbuffered for one-owner partial writes. Buffer stdout
            # separately so large native frames do not require bytewise reads.
            stream = io.BufferedReader(self._process.stdout)
            while True:
                line = stream.readline(_MAX_FRAME + 1)
                if not line:
                    break
                if len(line) > _MAX_FRAME or not line.endswith(b"\n"):
                    raise ServiceSetupError("invalid native frame boundary")
                with self._condition:
                    # Wait before subclass validation or state mutation. The
                    # single reader retains this bounded frame while consumers
                    # free queue capacity; a burst is not transport failure.
                    while len(self._events) >= _MAX_QUEUE or self._queued_bytes + len(line) > _MAX_FRAME:
                        if self._failure:
                            raise ServiceSetupError("service transport failed while observation pending")
                        self._condition.wait()
                    self._accept(json.loads(line, object_pairs_hook=_unique, parse_constant=_invalid_constant), len(line))
        except BaseException:
            with self._condition:
                self._failure = True
                self._retired = True
                self._condition.notify_all()
            self._end_input()
        finally:
            self._end_input()
            # Dispose this exact output reader before observing process exit.
            # A root owner can now fence the original native invocation without
            # waiting on an SDK reader that is itself waiting on that owner.
            try:
                self._process.stdout.close()
            except (OSError, ValueError):
                with self._condition:
                    self._failure = True
            self._retire_external()
            self._reap()
            with self._condition:
                self._reader_done = True
                self._condition.notify_all()

    def _reap(self) -> None:
        assert self._process is not None
        while self._process.returncode is None:
            try:
                self._process.wait(timeout=1)
            except subprocess.TimeoutExpired:
                pass
            except BaseException:
                with self._condition:
                    self._failure = True
                    self._condition.wait(1)
        with self._condition:
            self._joined = True
            self._condition.notify_all()

    def next_event(self, timeout: float = 60) -> ServiceSetupEvent:
        deadline = _wait(timeout)
        with self._condition:
            while not self._events:
                if self._failure:
                    raise ServiceSetupError("service transport failed; retain native owner")
                # Process exit can precede draining its retained pipe frames.
                if self._joined and self._reader_done:
                    raise EOFError("native service process ended")
                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    raise TimeoutError("service observation pending; native owner retained")
                self._condition.wait(remaining)
            event, size = self._events.popleft()
            self._queued_bytes -= size
            self._condition.notify_all()
            return event

    def close(self, timeout: float = 150) -> ServiceSetupClose:
        deadline = _wait(timeout)
        with self._condition:
            first = not self._closing and not self._joined
            self._closing = True
            target = self._target()
            if first:
                self._close_target = target
        if first:
            try:
                self._close_id = self._request("connect/close", {"target": target}, control=True)
            except ServiceSetupError:
                with self._condition:
                    self._failure = True
                self._end_input()
                self._retire_external()
        assert self._process is not None
        remaining = deadline - time.monotonic()
        try:
            self._process.wait(timeout=max(0, remaining))
        except subprocess.TimeoutExpired:
            raise TimeoutError("native service close pending; same owner retained") from None
        with self._condition:
            self._joined = True
            while not (self._reader_done and self._writer_done):
                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    raise TimeoutError("native service close pending; same owner retained")
                self._condition.wait(remaining)
        for worker in (self._reader, self._writer):
            if worker.ident is not None:
                worker.join(max(0, deadline - time.monotonic()))
                if worker.is_alive():
                    raise TimeoutError("native service close pending; same owner retained")
        return ServiceSetupClose(True, self._process.returncode,
            self._native_closed and not self._failure and self._process.returncode == 0
            and (not self._external_transport or self._process.healthy))


def _connector_target(value: Mapping[str, Any]) -> dict[str, Any]:
    required = {"slot_id", "issuer", "client", "resource", "scopes", "strategy_id", "account_selection"}
    if not isinstance(value, Mapping) or not required <= set(value) or set(value) - required - {"client_auth_profile"}:
        raise ValueError("closed native connector target required")
    if any(not _text(value[key], 4096) for key in set(value) - {"scopes", "account_selection"}):
        raise ValueError("invalid native connector selector")
    if not _scopes(value["scopes"]):
        raise ValueError("explicit connector scopes required")
    account = value["account_selection"]
    if account is not None and not _text(account, 4096) and account != {"mode": "unverified"}:
        raise ValueError("closed native account selection required")
    return json.loads(_encoded(dict(value)))


def _scopes(value: Any) -> bool:
    return isinstance(value, list) and all(_text(scope, 4096) for scope in value) and len(value) <= 256


def _connector_operations(value: Mapping[str, Any]) -> dict[str, Any]:
    if not isinstance(value, Mapping) or not value or len(value) > 256:
        raise ValueError("explicit connector operation requirements required")
    for name, row in value.items():
        if (not _text(name, 4096) or not isinstance(row, Mapping) or "destination_origin" not in row
                or set(row) - {"destination_origin", "required_scopes", "read_only"}
                or not _text(row["destination_origin"], 4096)
                or "required_scopes" in row and not _scopes(row["required_scopes"])
                or "read_only" in row and type(row["read_only"]) is not bool):
            raise ValueError("closed native connector operation required")
    return json.loads(_encoded(dict(value)))


def _answer_matches(prompt: Mapping[str, Any], answer: dict[str, Any]) -> bool:
    if answer == {"kind": "cancel"}:
        return True
    kind = prompt.get("kind")
    if answer.get("kind") != kind:
        return False
    if kind == "service_secret":
        return (set(answer) == {"kind", "job_id", "service", "slot", "value"}
                and all(answer[k] == prompt.get(k) and _text(answer[k], 4096) for k in ("job_id", "service", "slot"))
                and _utf8(answer["value"], 16384) and "\0" not in answer["value"])
    if kind == "approval":
        return set(answer) == {"kind", "decision"} and answer["decision"] in ("approve", "deny")
    if kind == "provider_target":
        return set(answer) == {"kind", "choice_id"} and any(isinstance(c, dict) and c.get("choice_id") == answer["choice_id"] for c in prompt.get("choices", []))
    if kind == "provider_action":
        return set(answer) == {"kind", "action"} and answer["action"] in prompt.get("actions", [])
    if kind == "provider_api_key":
        return set(answer) == {"kind", "key"} and _text(answer["key"], 8192) and bool(answer["key"].strip())
    return False
