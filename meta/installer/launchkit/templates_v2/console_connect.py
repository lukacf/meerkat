"""Private, application-owned transport for the supplied Connect stdio contract.

This module supplies no binary acquisition, HTTP server, native runtime, effect
recovery or approval authority. The host supplies an acquired executable and an
exact trusted bootstrap for one current login/application/realm. It must retire
that binding on expiry/logout and join close before its runtime shuts down.
"""
from __future__ import annotations

import asyncio
import copy
from datetime import datetime, timezone
import json
import math
from pathlib import Path
import re
import time
import uuid


FRAME_LIMIT = 64 * 1024 * 1024
MAX_UINT = 2**53 - 1
PROMPT_KINDS = {"forge_recovery", "forge_origin", "forge_password", "approval"}
SERVICE_PROMPT_KINDS = {"provider_target", "provider_action", "provider_api_key",
                        "provider_browser", "provider_device", "approval"}
PROVIDER_ACTIONS = {"use", "enter_key", "sign_in", "back"}
COMPLETIONS = {"completed", "completed_without_result", "callback_pending",
               "callback_batch_pending", "cancelled", "abandoned", "abandoned_with_error",
               "completed_with_finalization_failure", "runtime_terminated"}
REFUSALS = {"unavailable", "busy", "invalid_message", "history_unavailable", "turn_uncertain",
            "stop_unavailable", "stop_uncertain", "invalid_binding", "binding_retired",
            "stale_prompt", "invalid_answer", "native_auth_required", "unsupported_presenter",
            "required_context_missing", "shutdown_unconfirmed"}
RETIRE_REASONS = {"logout", "login_expired", "realm_changed", "application_stopping", "bridge_lost"}
_HOST_ID = re.compile(r"h:[A-Za-z0-9_-]{1,64}\Z", re.ASCII)
_BROWSER_ASSOCIATION = re.compile(r"b:[0-9a-f]{32}\Z", re.ASCII)
_PROMPT_ID = re.compile(r"p:[A-Za-z0-9_-]{1,64}\Z", re.ASCII)
_END = object()


def _owned_task(coroutine):
    task = asyncio.create_task(coroutine)
    task.add_done_callback(lambda done: done.exception() if not done.cancelled() else None)
    return task


class ConnectError(RuntimeError):
    """A closed, payload-free adapter refusal, never a native lifecycle state."""
    def __init__(self, code):
        self.code = code if code in REFUSALS else "invalid_message"
        super().__init__("Toolkit Connect request could not be completed.")


def _object(value, required, optional=()):
    if not isinstance(value, dict) or not set(required) <= value.keys() or set(value) - set(required) - set(optional):
        raise ValueError("Invalid object.")
    return value


def _string(value, *, nonempty=True):
    if not isinstance(value, str) or nonempty and not value:
        raise ValueError("Invalid string.")
    value.encode("utf-8")  # Surrogate code points are not Unicode scalar values.
    return value


def _opaque(value):
    _string(value)
    if len(value.encode("utf-8")) > 256 or any(ord(char) < 32 or 127 <= ord(char) <= 159 for char in value):
        raise ValueError("Invalid opaque value.")
    return value


def _uint(value):
    if type(value) is not int or not 0 <= value <= MAX_UINT:
        raise ValueError("Invalid integer.")
    return value


def _boolean(value):
    if type(value) is not bool:
        raise ValueError("Invalid boolean.")
    return value


def _id(value, pattern):
    if not isinstance(value, str) or pattern.fullmatch(value) is None:
        raise ValueError("Invalid request identifier.")
    return value


def _origin(value):
    _string(value)
    match = re.fullmatch(r"http://127\.0\.0\.1:([1-9][0-9]{0,4})", value)
    if not match or not 1 <= int(match[1]) <= 65535:
        raise ValueError("Invalid application origin.")
    return value


def _json(value):
    data = json.dumps(value, ensure_ascii=False, allow_nan=False, separators=(",", ":")).encode("utf-8")
    if len(data) + 1 > FRAME_LIMIT:
        raise ValueError("Frame too large.")
    return data + b"\n"


def _pairs(items):
    result = {}
    for key, value in items:
        if key in result:
            raise ValueError("Duplicate JSON key.")
        result[key] = value
    return result


def _decode(data):
    def invalid_constant(_):
        raise ValueError("Invalid JSON constant.")
    def finite_float(text):
        value = float(text)
        if not math.isfinite(value):
            raise ValueError("Invalid JSON number.")
        return value
    return json.loads(data.decode("utf-8"), object_pairs_hook=_pairs,
                      parse_constant=invalid_constant, parse_float=finite_float)


def _bootstrap(value):
    if not isinstance(value, dict) or type(value.get("protocol_version")) is not int or value["protocol_version"] not in {1, 3, 4}:
        raise ValueError("Invalid protocol.")
    required = {"protocol_version", "application", "viewer", "realm"}
    if value["protocol_version"] == 3:
        required |= {"adapter", "services"}
    elif value["protocol_version"] == 4:
        required |= {"adapter", "forge"}
    _object(value, required, {"model", "auth_binding"})
    if value["protocol_version"] == 3:
        if value["adapter"] != "forge_services":
            raise ValueError("Invalid service adapter.")
        services = _object(value["services"], {"resource_manifest_sha256", "preparation"})
        digest = services["resource_manifest_sha256"]
        if (not isinstance(digest, str) or re.fullmatch(r"[0-9a-f]{64}", digest, re.ASCII) is None
                or services["preparation"] != "permitted"):
            raise ValueError("Invalid service resource permission.")
    app = _object(value["application"], {"application_id", "instance_id", "origin"})
    _opaque(app["application_id"]); _opaque(app["instance_id"]); _origin(app["origin"])
    if value["protocol_version"] == 4:
        if value["adapter"] != "forge_application":
            raise ValueError("Invalid application adapter.")
        forge = _object(value["forge"], {"host", "private_fields"})
        host = _object(forge["host"], {"protocol", "host_id", "application_id", "source_digest"})
        if (type(host["protocol"]) is not int or host["protocol"] != 1
                or not isinstance(host["host_id"], str)
                or re.fullmatch(r"forge:[0-9a-f]{64}", host["host_id"], re.ASCII) is None
                or not isinstance(host["source_digest"], str)
                or re.fullmatch(r"[0-9a-f]{64}", host["source_digest"], re.ASCII) is None
                or not isinstance(host["application_id"], str)
                or re.fullmatch(r"[A-Za-z0-9_-]{1,128}", host["application_id"], re.ASCII) is None
                or host["application_id"] != app["application_id"]):
            raise ValueError("Invalid selected application host.")
        if not isinstance(forge["private_fields"], list):
            raise ValueError("Invalid declared private fields.")
        # Native BTreeSet<String> gives set semantics. Retain the trusted wire
        # input, including array order, without deriving declarations or policy.
        for field in forge["private_fields"]:
            _string(field, nonempty=False)
    viewer = _object(value["viewer"], {"principal_id", "login_scope_id", "expires_at_ms"})
    _string(viewer["principal_id"]); _opaque(viewer["login_scope_id"]); _uint(viewer["expires_at_ms"])
    realm = _object(value["realm"], {"realm_id", "state_root", "context_root", "user_config_root"})
    _string(realm["realm_id"])
    for key in ("state_root", "context_root", "user_config_root"):
        if not Path(_string(realm[key])).is_absolute() or "\x00" in realm[key]:
            raise ValueError("Realm paths must be host-owned absolute paths.")
    if "model" in value:
        _string(value["model"])
    if "auth_binding" in value:
        binding = _object(value["auth_binding"], {"realm", "binding"}, {"profile"})
        for item in binding.values():
            _string(item)
    _json(value)
    return copy.deepcopy(value)


def _state(value, *, protocol_version=1):
    if protocol_version not in {1, 3, 4}:
        raise ValueError("Invalid state protocol.")
    required = {"available", "can_send", "can_stop", "outcome_uncertain", "active_send", "last_completion"}
    if protocol_version in {3, 4}:
        required.add("private_notice_pending")
    _object(value, required)
    for key in ("available", "can_send", "can_stop", "outcome_uncertain", *(["private_notice_pending"] if protocol_version in {3, 4} else [])):
        _boolean(value[key])
    active = value["active_send"]
    if active is not None:
        _object(active, {"send_request_id", "cancellation_requested"})
        _id(active["send_request_id"], _HOST_ID); _boolean(active["cancellation_requested"])
    last = value["last_completion"]
    if last is not None:
        _object(last, {"send_request_id", "completion"})
        _id(last["send_request_id"], _HOST_ID)
        if last["completion"] not in COMPLETIONS:
            raise ValueError("Invalid completion.")
    return copy.deepcopy(value)


def _prompt(value, origin, *, protocol_version=1):
    if protocol_version == 3:
        return _service_prompt(value, origin)
    if not isinstance(value, dict):
        raise ValueError("Invalid prompt.")
    kind = value.get("kind")
    if kind == "forge_recovery":
        _object(value, {"kind", "choices", "more", "skipped_entries"})
        if not isinstance(value["choices"], list):
            raise ValueError("Invalid choices.")
        seen = set()
        for row in value["choices"]:
            _object(row, {"choice_id", "application_id", "change_id", "origin"})
            _opaque(row["choice_id"])
            if row["choice_id"] in seen:
                raise ValueError("Duplicate choice.")
            seen.add(row["choice_id"])
            _string(row["application_id"]); _string(row["change_id"])
            if _origin(row["origin"]) != origin:
                raise ValueError("Wrong application origin.")
        _boolean(value["more"]); _uint(value["skipped_entries"])
    elif kind == "forge_origin":
        _object(value, {"kind", "permitted_origin"})
        if _origin(value["permitted_origin"]) != origin:
            raise ValueError("Wrong application origin.")
    elif kind == "forge_password":
        _object(value, {"kind", "origin", "mode", "min_chars", "max_chars"})
        if (_origin(value["origin"]) != origin or value["mode"] not in {"initial", "reauthenticate"}
                or type(value["min_chars"]) is not int or value["min_chars"] != 12
                or type(value["max_chars"]) is not int or value["max_chars"] != 4096):
            raise ValueError("Invalid password prompt.")
    elif kind == "approval":
        _object(value, {"kind", "approval_id", "summary", "body", "allowed_decisions", "expires_at"})
        _string(value["approval_id"]); _string(value["summary"])
        decisions = value["allowed_decisions"]
        if (not isinstance(decisions, list) or not decisions or len(set(decisions)) != len(decisions)
                or any(item not in {"approve", "deny"} for item in decisions)):
            raise ValueError("Invalid approval decisions.")
        if value["expires_at"] is not None:
            _string(value["expires_at"])
            if re.fullmatch(r"[0-9]{4}-[0-9]{2}-[0-9]{2}[Tt][0-9]{2}:[0-9]{2}:[0-9]{2}(?:\.[0-9]+)?(?:[Zz]|[+-][0-9]{2}:[0-9]{2})", value["expires_at"]) is None:
                raise ValueError("Approval expiry must be RFC 3339.")
            expiry = datetime.fromisoformat(value["expires_at"].replace("z", "+00:00").replace("Z", "+00:00"))
            if expiry.tzinfo is None:
                raise ValueError("Approval expiry needs a timezone.")
    else:
        raise ValueError("Unsupported prompt.")
    _json(value)
    return copy.deepcopy(value)


def _shown_provider(value):
    _object(value, {"model", "provider", "realm", "binding"}, {"profile"})
    for item in value.values():
        _string(item)


def _service_prompt(value, origin):
    if not isinstance(value, dict):
        raise ValueError("Invalid private provider prompt.")
    kind = value.get("kind")
    if kind == "approval":
        return _prompt(value, origin)
    if kind == "provider_target":
        _object(value, {"kind", "choices"})
        if not isinstance(value["choices"], list):
            raise ValueError("Invalid provider choices.")
        seen = set()
        for choice in value["choices"]:
            _object(choice, {"choice_id", "target", "status"})
            _opaque(choice["choice_id"])
            if choice["choice_id"] in seen:
                raise ValueError("Duplicate provider choice.")
            seen.add(choice["choice_id"])
            _shown_provider(choice["target"])
            _string(choice["status"], nonempty=False)
    elif kind == "provider_action":
        _object(value, {"kind", "target", "actions"})
        _shown_provider(value["target"])
        actions = value["actions"]
        if (not isinstance(actions, list) or any(not isinstance(action, str) or action not in PROVIDER_ACTIONS for action in actions)
                or len(set(actions)) != len(actions)):
            raise ValueError("Invalid provider actions.")
    elif kind == "provider_api_key":
        _object(value, {"kind", "target", "max_bytes"})
        _shown_provider(value["target"])
        if type(value["max_bytes"]) is not int or value["max_bytes"] != 8192:
            raise ValueError("Invalid private key limit.")
    elif kind == "provider_browser":
        _object(value, {"kind", "target", "authorization_url"})
        _shown_provider(value["target"]); _string(value["authorization_url"])
    elif kind == "provider_device":
        _object(value, {"kind", "target", "verification_uri", "user_code"})
        _shown_provider(value["target"])
        _string(value["verification_uri"]); _string(value["user_code"])
    else:
        raise ValueError("Unsupported private provider prompt.")
    _json(value)
    return copy.deepcopy(value)


def _service_target(value):
    if not isinstance(value, dict):
        raise ValueError("Invalid private target.")
    if set(value) == {"initialize_request_id"}:
        _id(value["initialize_request_id"], _HOST_ID)
    elif set(value) == {"scope"}:
        _opaque(value["scope"])
    else:
        raise ValueError("Invalid private target.")
    return copy.deepcopy(value)


def _prompt_expired(prompt):
    if prompt.get("kind") != "approval" or prompt.get("expires_at") is None:
        return False
    return datetime.fromisoformat(prompt["expires_at"].replace("z", "+00:00").replace("Z", "+00:00")) <= datetime.now(timezone.utc)


def _answer(value, prompt, *, protocol_version=1):
    if protocol_version == 3:
        return _service_answer(value, prompt)
    if not isinstance(value, dict):
        raise ValueError("Invalid answer.")
    kind = value.get("kind")
    if kind == "cancel":
        _object(value, {"kind"})
    elif kind != prompt["kind"]:
        raise ValueError("Wrong answer kind.")
    elif kind == "forge_recovery":
        _object(value, {"kind", "choice"})
        choice = value["choice"]
        if not isinstance(choice, dict):
            raise ValueError("Invalid choice.")
        if choice.get("kind") == "existing":
            _object(choice, {"kind", "choice_id"})
            if choice["choice_id"] not in {row["choice_id"] for row in prompt["choices"]}:
                raise ValueError("Unknown retained choice.")
        elif choice.get("kind") in {"more", "new"}:
            _object(choice, {"kind"})
            if choice["kind"] == "more" and not prompt["more"]:
                raise ValueError("No further page.")
        else:
            raise ValueError("Invalid choice.")
    elif kind == "forge_origin":
        _object(value, {"kind", "origin"})
        if value["origin"] != prompt["permitted_origin"]:
            raise ValueError("Wrong retained origin.")
    elif kind == "forge_password":
        _object(value, {"kind", "password"})
        password = _string(value["password"])
        if not 12 <= len(password) <= 4096:
            raise ValueError("Invalid password length.")
    elif kind == "approval":
        _object(value, {"kind", "decision"})
        if value["decision"] not in prompt["allowed_decisions"]:
            raise ValueError("Invalid decision.")
    _json(value)
    return copy.deepcopy(value)


def _service_answer(value, prompt):
    if not isinstance(value, dict):
        raise ValueError("Invalid private provider answer.")
    kind = value.get("kind")
    if kind == "cancel":
        _object(value, {"kind"})
    elif kind != prompt["kind"]:
        raise ValueError("Wrong private answer kind.")
    elif kind == "provider_target":
        _object(value, {"kind", "choice_id"})
        _string(value["choice_id"])
        if value["choice_id"] not in {choice["choice_id"] for choice in prompt["choices"]}:
            raise ValueError("Unknown retained provider choice.")
    elif kind == "provider_action":
        _object(value, {"kind", "action"})
        _string(value["action"])
        if value["action"] not in prompt["actions"]:
            raise ValueError("Unknown retained provider action.")
    elif kind == "provider_api_key":
        _object(value, {"kind", "key"})
        key = _string(value["key"])
        if (not key.strip() or len(key.encode("utf-8")) > 8192
                or any(ord(char) < 32 or 127 <= ord(char) <= 159 for char in key)):
            raise ValueError("Invalid private provider key.")
    elif kind == "approval":
        return _answer(value, prompt)
    else:
        # Browser/device authorization is completed only by the native owner.
        raise ValueError("This private notice only accepts cancellation.")
    _json(value)
    return copy.deepcopy(value)


class ConnectSubscription:
    """One disposable view. Disposal never cancels a pipe operation."""
    def __init__(self, owner, private):
        self.owner, self.private = owner, private
        self.queue = asyncio.Queue(maxsize=256)
        self.closed = False

    def __aiter__(self):
        return self

    async def __anext__(self):
        while True:
            item = await self.queue.get()
            if item is _END or not self.owner._authorized():
                await self.close()
                raise StopAsyncIteration
            if item["type"] == "prompt":
                retained = self.owner._prompts.get(item["id"])
                if retained is None or not self.owner._prompt_current(retained) or _prompt_expired(self.owner._prompt_body(retained)):
                    continue
            return item

    async def close(self):
        async with self.owner._ordering:
            self.owner._detach(self)


class ConsoleConnect:
    """One retained stdio child for exactly one trusted host login binding.

    ``is_current`` is synchronous and verifies the actual host login, realm and
    application lifetime. It is not browser input. The host schedules retirement
    on expiry/logout even when no browser is attached. No request or prompt body
    is logged. ``retained_result`` observes only this original pipe correlation.
    It cannot recover native effects, repeat a send or replace canonical history.
    """
    def __init__(self, binary, bootstrap, *, is_current, environment=None):
        self.binary = Path(binary)
        if not self.binary.is_absolute() or not callable(is_current):
            raise ValueError("Supply the exact acquired executable and current-login owner.")
        self.bootstrap = _bootstrap(bootstrap)
        self.protocol_version = self.bootstrap["protocol_version"]
        self.is_current = is_current
        self.environment = dict(environment) if environment is not None else None
        self._ordering = asyncio.Lock()
        self._outgoing = asyncio.Queue()
        self._requests, self._prompts, self._seen_prompts, self._observers = {}, {}, set(), set()
        self._scope = self._initialize_id = self._send_id = self._retirement_id = None
        self._retirement_target = self._retirement_task = self._close_task = None
        self._process = self._reader_task = self._writer_task = self._stderr_task = self._start_task = None
        self._initialized = self._retired = self._failed = self._resources_closed = False
        self._unavailable = False
        self._state = {"available": False, "can_send": False, "can_stop": False,
                       "outcome_uncertain": False, "active_send": None, "last_completion": None}
        if self.protocol_version in {3, 4}:
            self._state["private_notice_pending"] = False
        self._state_observed = False
        self.capabilities = None

    def _authorized(self):
        if self._retired or self._failed or self.bootstrap["viewer"]["expires_at_ms"] <= int(time.time() * 1000):
            return False
        try:
            return self.is_current() is True
        except Exception:
            return False

    def _require_current(self):
        if not self._authorized():
            raise ConnectError("binding_retired" if self._retired else "invalid_binding")

    def _request(self, method, params):
        if self._failed or self._process is None:
            raise ConnectError("unavailable")
        identity = "h:" + uuid.uuid4().hex
        try:
            data = _json({"jsonrpc": "2.0", "id": identity, "method": method, "params": params})
        except (ValueError, UnicodeError, TypeError, RecursionError):
            raise ConnectError("invalid_message") from None
        future = asyncio.get_running_loop().create_future()
        # A disconnected caller does not abandon a result or emit its exception.
        future.add_done_callback(lambda done: done.exception() if not done.cancelled() else None)
        self._requests[identity] = {"method": method, "params": copy.deepcopy(params), "future": future}
        self._outgoing.put_nowait(data)
        return identity, future

    async def start(self):
        if self._start_task is None:
            self._require_current()
            self._start_task = _owned_task(self._start())
        return await asyncio.shield(self._start_task)

    async def _start(self):
        async with self._ordering:
            self._require_current()
            try:
                self._process = await asyncio.create_subprocess_exec(str(self.binary), "--stdio",
                    stdin=asyncio.subprocess.PIPE, stdout=asyncio.subprocess.PIPE,
                    stderr=asyncio.subprocess.PIPE, limit=FRAME_LIMIT, env=self.environment)
            except Exception:
                self._failed = True
                raise ConnectError("unavailable") from None
            self._reader_task = asyncio.create_task(self._read())
            self._writer_task = asyncio.create_task(self._write())
            self._stderr_task = asyncio.create_task(self._discard_stderr())
            self._require_current()  # Spawn awaited; fence again before bootstrap.
            self._initialize_id, future = self._request("connect/initialize", self.bootstrap)
            self._publish_binding()
        return await asyncio.shield(future)

    async def _discard_stderr(self):
        # Keep the child's pipe responsive without retaining arbitrary private
        # diagnostics. Protocol stdout is validated by the separate reader.
        try:
            while await self._process.stderr.read(65536):
                pass
        except Exception:
            await self._fail()

    async def _write(self):
        try:
            while True:
                data = await self._outgoing.get()
                if data is _END:
                    return
                self._process.stdin.write(data)
                await self._process.stdin.drain()
        except Exception:
            await self._fail()

    async def _read(self):
        try:
            while True:
                data = await self._process.stdout.readline()
                if not data:
                    if not self._resources_closed:
                        await self._fail()
                    else:
                        async with self._ordering:
                            # Joined resources do not manufacture any missing
                            # original input/initialization transport response.
                            for request in self._requests.values():
                                if not request["future"].done():
                                    request["future"].set_exception(ConnectError(
                                        "turn_uncertain" if request["method"] == "connect/send" else "unavailable"))
                    return
                if len(data) > FRAME_LIMIT or not data.endswith(b"\n"):
                    raise ValueError("Invalid frame.")
                frame = _decode(data)
                async with self._ordering:
                    self._receive(frame)
        except Exception:
            await self._fail()

    def _bind_scope(self, scope):
        _opaque(scope)
        if self._scope is not None and self._scope != scope:
            raise ValueError("Wrong pipe scope.")
        self._scope = scope

    def _publish(self, event):
        if not self._authorized():
            return
        for observer in list(self._observers):
            self._queue_view(observer, {"type": "event", "event": copy.deepcopy(event)})

    def presentation_binding(self):
        """Host-private transport target, never a native state or capability."""
        self._require_current()
        if self.protocol_version != 3 or self._initialize_id is None:
            return None
        return {"type": "binding", "protocol_version": 3, "target": self._target()}

    def _publish_binding(self):
        if self.protocol_version != 3 or not self._authorized():
            return
        binding = self.presentation_binding()
        if binding is not None:
            for observer in list(self._observers):
                if observer.private:
                    self._queue_view(observer, copy.deepcopy(binding))

    def _prompt_body(self, retained):
        return retained["prompt"] if self.protocol_version == 3 else retained

    def _prompt_current(self, retained):
        if self.protocol_version != 3:
            return True
        try:
            return (not retained.get("answered", False) and not retained.get("retired", False)
                    and retained["target"] == self._target())
        except ConnectError:
            return False

    def _prompt_view(self, identity, retained):
        if self.protocol_version == 3:
            return {"type": "prompt", "protocol_version": 3, "id": identity,
                    "target": copy.deepcopy(retained["target"]), "prompt": copy.deepcopy(retained["prompt"])}
        return {"type": "prompt", "id": identity, "scope": self._scope, "prompt": copy.deepcopy(retained)}

    def _queue_view(self, observer, value):
        if observer.closed:
            return
        try:
            observer.queue.put_nowait(value)
        except asyncio.QueueFull:
            # A slow view loses only its subscription. Its remount refreshes the
            # same retained child; input and protected requests are not replayed.
            self._detach(observer)

    def _detach(self, observer):
        self._observers.discard(observer)
        observer.closed = True
        while not observer.queue.empty():
            observer.queue.get_nowait()
        observer.queue.put_nowait(_END)

    def _retain_state(self, state):
        state = _state(state, protocol_version=self.protocol_version)
        if self.protocol_version == 3 and not self._initialized:
            initial = {"available": False, "can_send": False, "can_stop": False,
                       "outcome_uncertain": False, "active_send": None,
                       "last_completion": None, "private_notice_pending": False}
            if state != initial:
                raise ValueError("Invalid initializing state.")
        for control in (state["active_send"], state["last_completion"]):
            if control is not None:
                request = self._requests.get(control["send_request_id"])
                if request is None or request["method"] != "connect/send":
                    raise ValueError("Unknown original send control.")
        if self._state["last_completion"] is not None and state["last_completion"] is None:
            raise ValueError("Known completion cannot disappear.")
        # A host request already queued to the sole writer is retained before
        # the child observes admission. An earlier state/history frame cannot
        # erase that exact forwarding control during this transport interval.
        # Only the child's correlated completion can settle the original send.
        last = state["last_completion"]
        if (self._send_id is not None and state["active_send"] is None
                and (last is None or last["send_request_id"] != self._send_id)):
            active = self._state["active_send"]
            cancellation_requested = (active is not None
                and active["send_request_id"] == self._send_id
                and active["cancellation_requested"])
            state.update(can_send=False, can_stop=True, active_send={
                "send_request_id": self._send_id, "cancellation_requested": cancellation_requested})
        if self._unavailable:
            # A standalone native unavailable event fences this binding even
            # when an earlier response/state frame is still in the pipe. Keep
            # original work and known completion; invent no terminal outcome.
            state.update(available=False, can_send=False, can_stop=False)
        self._state = state
        self._state_observed = True
        self._publish(self._state_event())

    def _state_event(self):
        target = {"target": self._target()} if self.protocol_version == 3 else {"scope": self._scope}
        return {**target, "kind": "state", "state": copy.deepcopy(self._state)}

    def _receive(self, frame):
        if not isinstance(frame, dict) or frame.get("jsonrpc") != "2.0":
            raise ValueError("Invalid JSON-RPC frame.")
        if "method" in frame:
            if frame.get("method") == "connect/event":
                _object(frame, {"jsonrpc", "method", "params"})
                self._event(frame["params"])
                return
            _object(frame, {"jsonrpc", "id", "method", "params"})
            identity = _id(frame["id"], _PROMPT_ID)
            if identity in self._seen_prompts:
                raise ValueError("Duplicate child request.")
            self._seen_prompts.add(identity)
            if frame["method"] != "connect/prompt":
                self._prompt_error(identity, -32601, "invalid_message")
                return
            try:
                params = _object(frame["params"], {"target", "prompt"} if self.protocol_version == 3 else {"scope", "prompt"})
                if self.protocol_version == 3:
                    target = _service_target(params["target"])
                    if target != self._target():
                        raise ValueError("Wrong retained private target.")
                else:
                    self._bind_scope(params["scope"])
                prompt = _prompt(params["prompt"], self.bootstrap["application"]["origin"], protocol_version=self.protocol_version)
                if prompt["kind"] == "forge_recovery" and any(row["application_id"] != self.bootstrap["application"]["application_id"] for row in prompt["choices"]):
                    raise ValueError("Wrong application recovery choice.")
            except (ValueError, TypeError, UnicodeError, RecursionError, ConnectError):
                self._prompt_error(identity, -32602, "invalid_message")
                return
            if not self._authorized() or self._unavailable:
                self._prompt_error(identity, -32010, "binding_retired")
                return
            retained = {"target": target, "prompt": prompt} if self.protocol_version == 3 else prompt
            self._prompts[identity] = retained
            for observer in list(self._observers):
                if observer.private:
                    self._queue_view(observer, self._prompt_view(identity, retained))
            return
        _object(frame, {"jsonrpc", "id"}, {"result", "error"})
        identity = _id(frame["id"], _HOST_ID)
        request = self._requests.get(identity)
        if request is None or request["future"].done() or ("result" in frame) == ("error" in frame):
            raise ValueError("Unknown or duplicate response.")
        if "error" in frame:
            error = _object(frame["error"], {"code", "message"}, {"data"})
            _string(error["message"], nonempty=False)
            if type(error["code"]) is not int:
                raise ValueError("Invalid error.")
            if error["code"] == -32010:
                data = _object(error.get("data"), {"code"})
                if data["code"] not in REFUSALS:
                    raise ValueError("Invalid refusal.")
                code = data["code"]
            elif error["code"] in {-32700, -32600, -32601, -32602} and "data" not in error:
                code = "invalid_message"
            else:
                raise ValueError("Invalid error.")
            request["future"].set_exception(ConnectError(code))
            if self.protocol_version == 3 and request["method"] == "connect/initialize":
                # The original initialization error is a terminal transition,
                # even if its following unavailable notification is delayed.
                self._unavailable = True
                self._prompts.clear()
            if identity == self._send_id:
                self._send_id = None
            return
        result = self._result(identity, request, frame["result"])
        request["future"].set_result(copy.deepcopy(result))

    def _prompt_error(self, identity, code, refusal):
        value = {"code": code, "message": "Toolkit Connect prompt could not be handled."}
        if code == -32010:
            value["data"] = {"code": refusal}
        self._outgoing.put_nowait(_json({"jsonrpc": "2.0", "id": identity, "error": value}))

    def _result(self, identity, request, value):
        method = request["method"]
        if method == "connect/initialize":
            _object(value, {"protocol_version", "scope", "session_id", "capabilities", "state"})
            if type(value["protocol_version"]) is not int or value["protocol_version"] != self.protocol_version:
                raise ValueError("Invalid protocol.")
            if self.protocol_version == 3:
                cap = _object(value["capabilities"], {"prompt_kinds", "forge_configuration", "service_preparation", "homecore", "tools"})
                tools = cap["tools"]
                if not isinstance(tools, list) or any(not isinstance(name, str) or not name for name in tools):
                    raise ValueError("Invalid initial tool names.")
                for name in tools:
                    _string(name)
                if tools != sorted(set(tools)):
                    raise ValueError("Initial tool names must be sorted and unique.")
                if cap["forge_configuration"] is not False or cap["service_preparation"] is not True or cap["homecore"] is not False:
                    raise ValueError("Invalid service composition capabilities.")
                if self._unavailable or self._prompts:
                    raise ValueError("Initialization has terminal or unclosed private custody.")
                # Validate the complete successful transition before publishing
                # its actual scope binding to an attached private observer.
                _opaque(value["scope"]); _string(value["session_id"])
                returned_state = _state(value["state"], protocol_version=3)
                for control in (returned_state["active_send"], returned_state["last_completion"]):
                    if control is not None:
                        original = self._requests.get(control["send_request_id"])
                        if original is None or original["method"] != "connect/send":
                            raise ValueError("Unknown original send control.")
                prompt_kinds = SERVICE_PROMPT_KINDS
            elif self.protocol_version == 4:
                cap = _object(value["capabilities"], {"prompt_kinds", "forge_configuration", "service_preparation", "homecore", "tools"})
                if not isinstance(cap["tools"], list):
                    raise ValueError("Invalid initial tool names.")
                for name in cap["tools"]:
                    _string(name)
                if cap["forge_configuration"] is not True or cap["service_preparation"] is not False or cap["homecore"] is not False:
                    raise ValueError("Invalid application composition capabilities.")
                # V4 retains ordinary scope/prompt envelopes. The native state
                # adds this notice field without selecting service presenters.
                _opaque(value["scope"]); _string(value["session_id"])
                returned_state = _state(value["state"], protocol_version=4)
                for control in (returned_state["active_send"], returned_state["last_completion"]):
                    if control is not None:
                        original = self._requests.get(control["send_request_id"])
                        if original is None or original["method"] != "connect/send":
                            raise ValueError("Unknown original send control.")
                prompt_kinds = PROMPT_KINDS
            else:
                cap = _object(value["capabilities"], {"prompt_kinds", "forge_configuration", "service_preparation", "homecore"})
                prompt_kinds = PROMPT_KINDS
            if not isinstance(cap["prompt_kinds"], list) or any(not isinstance(kind, str) or kind not in prompt_kinds for kind in cap["prompt_kinds"]) or len(set(cap["prompt_kinds"])) != len(cap["prompt_kinds"]):
                raise ValueError("Invalid presenters.")
            for key in ("forge_configuration", "service_preparation", "homecore"):
                _boolean(cap[key])
            if set(cap["prompt_kinds"]) != prompt_kinds:
                raise ValueError("Connect requires the complete negotiated private presenters.")
            # Only this actual successful response can establish a v3 scope.
            self._bind_scope(value["scope"]); _string(value["session_id"])
            self.capabilities = copy.deepcopy(cap)
            self._initialized = True
            self._publish_binding()
            self._retain_state(value["state"])
            value["state"] = copy.deepcopy(self._state)
        elif method == "connect/history":
            _object(value, {"history", "state"})
            history = _object(value["history"], {"messages", "offset", "total", "hasMore"})
            _uint(history["offset"]); _uint(history["total"]); _boolean(history["hasMore"])
            if not isinstance(history["messages"], list) or len(history["messages"]) > 50 or history["offset"] > history["total"]:
                raise ValueError("Invalid history.")
            for message in history["messages"]:
                _object(message, {"role", "text"})
                if message["role"] not in {"user", "assistant"}:
                    raise ValueError("Nonpublic history.")
                _string(message["text"], nonempty=False)
            self._retain_state(value["state"])
            value["state"] = copy.deepcopy(self._state)
        elif method == "connect/send":
            _object(value, {"scope", "send_request_id", "completion", "state"})
            self._bind_scope(value["scope"])
            if value["send_request_id"] != identity or value["completion"] not in COMPLETIONS:
                raise ValueError("Wrong send completion.")
            if value["state"].get("last_completion") != {"send_request_id": identity, "completion": value["completion"]}:
                raise ValueError("Completion projection does not match the original send.")
            self._retain_state(value["state"])
            value["state"] = copy.deepcopy(self._state)
            self._send_id = None
        elif method == "connect/cancel":
            _object(value, {"scope", "send_request_id", "status"})
            self._bind_scope(value["scope"])
            if value["send_request_id"] != request["params"]["send_request_id"] or value["status"] not in {"cancellation_requested", "not_current"}:
                raise ValueError("Wrong cancellation acknowledgement.")
        elif method == "connect/retire":
            _object(value, {"target", "retirement_id", "admission_closed", "prompts_retired", "entered_work"})
            if (value["target"] != request["params"]["target"] or value["retirement_id"] != self._retirement_id
                    or value["admission_closed"] is not True or value["prompts_retired"] is not True
                    or value["entered_work"] not in ({"settled"} if self.protocol_version == 3 else {"settling", "settled"})):
                raise ValueError("Unconfirmed retirement.")
        elif method == "connect/close":
            _object(value, {"target", "resources_closed"})
            if value["target"] != request["params"]["target"] or value["resources_closed"] is not True:
                raise ValueError("Unconfirmed close.")
            self._resources_closed = True
        return value

    def _event(self, value):
        if not isinstance(value, dict):
            raise ValueError("Invalid event.")
        if self.protocol_version == 3:
            target = _service_target(value.get("target"))
            if target != self._target():
                raise ValueError("Wrong retained event target.")
            binding = "target"
        else:
            self._bind_scope(value.get("scope"))
            binding = "scope"
        kind = value.get("kind")
        if kind == "state":
            _object(value, {binding, "kind", "state"})
            self._retain_state(value["state"])
            return
        if kind == "conversation":
            _object(value, {binding, "kind", "send_request_id", "event"})
            if self.protocol_version == 3 and not self._initialized:
                raise ValueError("Conversation requires the successful initialization scope.")
            send = _id(value["send_request_id"], _HOST_ID)
            if send not in self._requests or self._requests[send]["method"] != "connect/send":
                raise ValueError("Unknown conversation input.")
            event = value["event"]
            if not isinstance(event, dict):
                raise ValueError("Invalid conversation event.")
            if event.get("kind") == "run_observed":
                _object(event, {"kind"})
            elif event.get("kind") == "text_reset":
                _object(event, {"kind"}, {"messageId"})
            elif event.get("kind") in {"text_delta", "text_complete"}:
                _object(event, {"kind", "text"}, {"messageId"}); _string(event["text"], nonempty=False)
            else:
                raise ValueError("Invalid conversation event.")
            if "messageId" in event:
                _string(event["messageId"])
        elif kind == "prompt_closed":
            _object(value, {binding, "kind", "prompt_id", "disposition"})
            prompt = _id(value["prompt_id"], _PROMPT_ID)
            dispositions = {"answered", "decision_recorded", "refused", "expired", "retired", "cancelled"}
            if self.protocol_version == 3:
                dispositions.add("owner_completed")
            if prompt not in self._seen_prompts or value["disposition"] not in dispositions:
                raise ValueError("Invalid prompt closure.")
            if self.protocol_version == 3:
                retained = self._prompts.get(prompt)
                if retained is None or retained["target"] != target:
                    raise ValueError("Wrong retained prompt closure.")
                if value["disposition"] == "owner_completed" and retained["prompt"]["kind"] not in {"provider_browser", "provider_device"}:
                    raise ValueError("Owner completion is only a private authorization notice closure.")
            self._prompts.pop(prompt, None)
        elif kind == "unavailable":
            _object(value, {binding, "kind", "reason"})
            if value["reason"] not in {"retired", "expired", "closing", "failed"}:
                raise ValueError("Invalid unavailable event.")
            self._unavailable = True
            self._retire_prompts()
            self._retain_state(self._state)
        else:
            raise ValueError("Unknown event.")
        self._publish(value)

    async def _fail(self):
        async with self._ordering:
            if self._failed:
                return
            self._failed = True
            self._state.update(available=False, can_send=False, can_stop=False,
                               outcome_uncertain=self._state["outcome_uncertain"] or self._send_id is not None)
            self._prompts.clear()
            for request in self._requests.values():
                if not request["future"].done():
                    request["future"].set_exception(ConnectError("shutdown_unconfirmed"))
            for observer in list(self._observers):
                self._detach(observer)

    def snapshot(self):
        self._require_current()
        if not self._initialized:
            raise ConnectError("unavailable")
        return self._state_event()

    async def attach(self, *, private=False):
        if type(private) is not bool:
            raise ConnectError("invalid_message")
        async with self._ordering:
            self._require_current()
            if self._scope is None and not (self.protocol_version == 3 and private):
                raise ConnectError("unavailable")
            observer = ConnectSubscription(self, private)
            self._observers.add(observer)
            # Snapshot and observer registration share the pipe ordering. Even
            # before initialization, no later event can pass this first state.
            if self.protocol_version == 3:
                binding = self.presentation_binding()
                if private and binding is not None:
                    self._queue_view(observer, binding)
            if self._scope is not None or self.protocol_version == 3 and self._state_observed:
                self._queue_view(observer, {"type": "event", "event": self._state_event()})
            if private:
                for identity, retained in self._prompts.items():
                    if self._prompt_current(retained) and not _prompt_expired(self._prompt_body(retained)):
                        self._queue_view(observer, self._prompt_view(identity, retained))
            return observer

    async def history(self, offset=None):
        async with self._ordering:
            self._require_current()
            if not self._initialized or not self._state["available"]:
                raise ConnectError("unavailable")
            params = {"scope": self._scope}
            if offset is not None:
                try:
                    params["offset"] = _uint(offset)
                except ValueError:
                    raise ConnectError("invalid_message") from None
            _, future = self._request("connect/history", params)
        result = await asyncio.shield(future)
        self._require_current()
        return result

    async def begin_send(self, text, *, browser_association=None):
        try:
            _string(text); _json(text)
            if browser_association is not None:
                _id(browser_association, _BROWSER_ASSOCIATION)
        except (ValueError, UnicodeError, TypeError):
            raise ConnectError("invalid_message") from None
        async with self._ordering:
            self._require_current()
            # Browser metadata belongs only to this original retained pipe
            # request. Reusing it never creates or rebinds another native send.
            if browser_association is not None and any(
                    request.get("browser_association") == browser_association
                    for request in self._requests.values()):
                raise ConnectError("invalid_message")
            if not self._initialized or not self._state["available"]:
                raise ConnectError("unavailable")
            if self._send_id is not None or not self._state["can_send"]:
                raise ConnectError("busy")
            self._send_id, _ = self._request("connect/send", {"scope": self._scope, "text": text})
            if browser_association is not None:
                self._requests[self._send_id]["browser_association"] = browser_association
            self._state.update(can_send=False, can_stop=True, active_send={
                "send_request_id": self._send_id, "cancellation_requested": False})
            self._publish(self._state_event())
            return self._send_id

    async def send(self, text):
        identity = await self.begin_send(text)
        return await self.observe_result(identity)

    async def observe_result(self, request_id):
        self._require_current()
        request = self._requests.get(request_id)
        if request is None:
            raise ConnectError("invalid_message")
        result = await asyncio.shield(request["future"])
        self._require_current()
        return copy.deepcopy(result)

    def retained_result(self, request_id):
        self._require_current()
        request = self._requests.get(request_id)
        if request is None or request["method"] != "connect/send":
            raise ConnectError("invalid_message")
        future = request["future"]
        if not future.done():
            return {"request_id": request_id, "status": "pending"}
        error = future.exception()
        if error:
            return {"request_id": request_id, "status": "refused", "error": {"code": error.code}}
        return {"request_id": request_id, "status": "completed", "result": copy.deepcopy(future.result())}

    def retained_send(self, browser_association):
        """Observe one browser association on the original retained request.

        Unknown is not proof of rejection or permission to repeat a send. This
        metadata neither enters the child protocol nor recovers native effects.
        """
        self._require_current()
        try:
            _id(browser_association, _BROWSER_ASSOCIATION)
        except ValueError:
            raise ConnectError("invalid_message") from None
        for identity, request in self._requests.items():
            if request["method"] == "connect/send" and request.get("browser_association") == browser_association:
                return {"association": browser_association, **self.retained_result(identity)}
        return {"association": browser_association, "status": "unknown"}

    async def cancel(self, send_request_id):
        async with self._ordering:
            self._require_current()
            if self._unavailable:
                raise ConnectError("stop_unavailable")
            request = self._requests.get(send_request_id)
            if request is None or request["method"] != "connect/send":
                raise ConnectError("stop_unavailable")
            _, future = self._request("connect/cancel", {"scope": self._scope, "send_request_id": send_request_id})
        result = await asyncio.shield(future)
        self._require_current()
        return result  # A cancellation acknowledgement is never a stopped run.

    async def answer(self, prompt_id, answer):
        async with self._ordering:
            self._require_current()
            retained = self._prompts.get(prompt_id)
            if retained is None or not self._prompt_current(retained) or _prompt_expired(self._prompt_body(retained)):
                raise ConnectError("stale_prompt")
            prompt = self._prompt_body(retained)
            try:
                value = _answer(answer, prompt, protocol_version=self.protocol_version)
                data = _json({"jsonrpc": "2.0", "id": prompt_id, "result": {"answer": value}})
            except (ValueError, TypeError, UnicodeError, RecursionError):
                raise ConnectError("invalid_answer") from None
            # No asynchronous gap between this login/retirement fence, the
            # one-shot claim and the single writer's ordered forwarding.
            if self.protocol_version == 3:
                # Keep the same request's closure custody until the producer
                # confirms prompt_closed. It cannot be answered twice or shown.
                retained["answered"] = True
            else:
                self._prompts.pop(prompt_id)
            self._outgoing.put_nowait(data)

    def _target(self):
        if self._initialized:
            return {"scope": self._scope}
        if self._initialize_id is not None:
            return {"initialize_request_id": self._initialize_id}
        raise ConnectError("unavailable")

    async def retire(self, reason):
        if reason not in RETIRE_REASONS:
            raise ConnectError("invalid_message")
        async with self._ordering:
            if self._retirement_task is None:
                self._retired = True
                self._retire_prompts()
                for observer in list(self._observers):
                    self._detach(observer)
                self._retirement_id = uuid.uuid4().hex
                self._retirement_target = self._target()
                self._retirement_task = _owned_task(self._retire(reason))
        return await asyncio.shield(self._retirement_task)

    def _retire_prompts(self):
        if self.protocol_version == 3:
            # Private display/answer admission closes immediately. The existing
            # request still owns its ordered native prompt_closed notification.
            for retained in self._prompts.values():
                retained["retired"] = True
        else:
            self._prompts.clear()

    async def _retire(self, reason):
        async with self._ordering:
            _, future = self._request("connect/retire", {"target": self._retirement_target,
                "retirement_id": self._retirement_id, "reason": reason})
        return await asyncio.shield(future)

    async def close(self):
        if self._close_task is None:
            self._close_task = _owned_task(self._close())
        return await asyncio.shield(self._close_task)

    async def _close(self):
        if self._start_task is None:
            self._retired = True
            return {"resources_closed": True}  # No child or startup was entered.
        error = None
        # start() creates its owned process/pipe under this same ordering. Its
        # pending native initialization is fenced by request ID, not awaited
        # before close can release a presenter/admission waiter.
        async with self._ordering:
            self._retired = True
            self._retire_prompts()
            for observer in list(self._observers):
                self._detach(observer)
            target = self._target() if self._process is not None and self._initialize_id is not None else None
            future = None
            if target is not None and not self._failed:
                try:
                    _, future = self._request("connect/close", {"target": target})
                except ConnectError as failure:
                    error = failure
        if future is not None:
            try:
                await asyncio.shield(future)
            except ConnectError as failure:
                error = failure
        if self._process is not None:
            # EOF is custody cleanup, never a synthesized resources_closed ACK.
            self._outgoing.put_nowait(_END)
            if self._writer_task is not None:
                writer = await asyncio.gather(self._writer_task, return_exceptions=True)
                if any(isinstance(result, BaseException) for result in writer):
                    error = ConnectError("shutdown_unconfirmed")
            try:
                self._process.stdin.close()
            except Exception:
                # A pipe cleanup error does not release the process join owner.
                error = ConnectError("shutdown_unconfirmed")
            await self._process.wait()
            pipes = await asyncio.gather(*(task for task in (self._reader_task, self._stderr_task) if task is not None), return_exceptions=True)
            if any(isinstance(result, BaseException) for result in pipes):
                error = ConnectError("shutdown_unconfirmed")
        if self._start_task is not None:
            await asyncio.gather(self._start_task, return_exceptions=True)
        if self._retirement_task is not None:
            await asyncio.gather(self._retirement_task, return_exceptions=True)
        if error is not None or self._process is not None and not self._resources_closed or self._process is not None and self._process.returncode != 0:
            raise ConnectError("shutdown_unconfirmed")
        return {"target": target, "resources_closed": True} if target is not None else {"resources_closed": True}
