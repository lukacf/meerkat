"""Trusted-host provider-only Connect pipe. No model/session startup or token export.

The host selects the executable, four roots and authenticated viewer. Browser
answers are only answers to an offered prompt. Timeout keeps this same child
owned; call close again to observe its actual joined termination.
"""
from __future__ import annotations
import collections
import dataclasses
import json
import inspect
import io
import stat
import os
import subprocess
import threading
import time
from pathlib import Path
from typing import Any, Callable, Mapping

_PROTOCOL = "connect-provider-setup/1"
_MAX_FRAME = 1024 * 1024
_MAX_OBSERVATION_SECONDS = 2_147_483.647
_ENV = frozenset(("PATH", "LANG", "LC_ALL", "TMPDIR", "TMP", "TEMP", "SYSTEMROOT"))

class ProviderSetupError(RuntimeError):
    pass

@dataclasses.dataclass(frozen=True)
class ProviderSetupBinding:
    application_id: str
    instance_id: str
    principal_id: str
    login_scope_id: str
    expires_at_ms: int
    realm_id: str
    context_root: str
    state_root: str
    user_config_root: str
    credential_scope: Mapping[str, Any]
    browser: str = "local_loopback"

    def data(self) -> dict[str, Any]:
        value = dataclasses.asdict(self)
        for name in ("application_id", "instance_id", "principal_id", "login_scope_id", "realm_id"):
            text = value[name]
            if not isinstance(text, str) or not text or len(text) > 256 or any(ord(c) < 32 for c in text):
                raise ValueError("invalid trusted provider binding")
        for name in ("context_root", "state_root", "user_config_root"):
            if not isinstance(value[name], str) or not Path(value[name]).is_absolute() or "\0" in value[name]:
                raise ValueError("provider roots must be trusted absolute paths")
        scope = self.credential_scope
        if not isinstance(scope, dict) or scope not in ({"kind": "native_default"},):
            if not isinstance(scope, dict) or set(scope) != {"kind", "root"} or scope["kind"] != "file" or not isinstance(scope["root"], str) or not Path(scope["root"]).is_absolute() or "\0" in scope["root"]:
                raise ValueError("invalid trusted credential scope")
        if type(self.expires_at_ms) is not int or not int(time.time() * 1000) < self.expires_at_ms <= 9007199254740991:
            raise ValueError("provider viewer must have a current finite deadline")
        if self.browser not in ("local_loopback", "manual_return"):
            raise ValueError("invalid trusted callback transport")
        return {"protocol": _PROTOCOL, **value}

@dataclasses.dataclass(frozen=True)
class ProviderSelection:
    interaction_id: str
    model: str
    provider: str
    auth_binding: Mapping[str, Any]
    credential: str
    live_entitlement: str

    @classmethod
    def decode(cls, value: Any) -> "ProviderSelection":
        if not isinstance(value, dict) or set(value) != {f.name for f in dataclasses.fields(cls)}:
            raise ProviderSetupError("invalid provider selection fields")
        if any(not isinstance(value[k], str) or not value[k] for k in ("interaction_id", "model", "provider", "credential")):
            raise ProviderSetupError("invalid provider selection")
        auth = value["auth_binding"]
        if not isinstance(auth, dict) or not {"realm", "binding"} <= set(auth) or set(auth) - {"realm", "binding", "profile", "origin"}:
            raise ProviderSetupError("provider selection requires a native configured binding")
        if any(not isinstance(auth[k], str) or not auth[k] for k in ("realm", "binding")):
            raise ProviderSetupError("invalid native binding")
        if auth.get("profile") is not None and (not isinstance(auth["profile"], str) or not auth["profile"]):
            raise ProviderSetupError("invalid native profile")
        if "origin" in auth and auth["origin"] != "configured":
            raise ProviderSetupError("provider selection requires a configured origin")
        if value["live_entitlement"] != "unobserved":
            raise ProviderSetupError("provider selection cannot claim model entitlement")
        return cls(**value)

class ProviderSetupTransport:
    """One trusted root's original unbuffered provider pipe pair.

    input writes the original child's stdin; output reads its stdout. The root
    supplies a nonblocking retirement notification and a worker-only observer
    of that same child's actual exit. TimeoutError means pending, not cleanup.
    No process handle, spawn authority, HTTP surface or descendant ownership is
    transferred. A second carrier over aliases of these FDs is forbidden by
    the trusted-root contract; this class does not claim an OS alias detector.
    """
    __slots__ = ("_input", "_output", "_request_retirement", "_wait_process",
                 "_lock", "_client", "_preparation")

    def __init__(self, *, input: io.FileIO, output: io.FileIO,
                 request_retirement: Callable[[], None],
                 wait_process: Callable[[float], int]):
        self._input = input
        self._output = output
        self._request_retirement = request_retirement
        self._wait_process = wait_process
        self._lock = threading.Lock()
        self._client: ProviderSetupClient | None = None
        self._preparation: _AdoptedProviderSetupClient | None = None
        self._validate()

    @property
    def input(self) -> io.FileIO:
        return self._input

    @property
    def output(self) -> io.FileIO:
        return self._output

    @property
    def client(self) -> ProviderSetupClient | None:
        with self._lock:
            return self._client

    @property
    def transferred(self) -> bool:
        return self.client is not None

    def _validate(self) -> None:
        if type(self._input) is not io.FileIO or type(self._output) is not io.FileIO:
            raise ValueError("provider adoption requires original unbuffered FileIO pipes")
        try:
            if self._input is self._output or self._input.closed or self._output.closed:
                raise ValueError("provider pipes must be distinct and open")
            if not self._input.writable() or self._input.readable():
                raise ValueError("provider input must be the original write-only pipe")
            if not self._output.readable() or self._output.writable():
                raise ValueError("provider output must be the original read-only pipe")
            descriptors = (self._input.fileno(), self._output.fileno())
            if descriptors[0] == descriptors[1]:
                raise ValueError("provider pipe descriptors must be distinct")
            for descriptor in descriptors:
                if (not stat.S_ISFIFO(os.fstat(descriptor).st_mode)
                        or not os.get_blocking(descriptor)
                        or os.get_inheritable(descriptor)):
                    raise ValueError("provider pipes must be blocking and close-on-exec")
        except (OSError, io.UnsupportedOperation) as error:
            raise ValueError("provider original pipe is unavailable") from error
        for callback in (self._request_retirement, self._wait_process):
            if (not callable(callback) or inspect.iscoroutinefunction(callback)
                    or inspect.iscoroutinefunction(getattr(callback, "__call__", None))):
                raise ValueError("provider owner callbacks must be synchronous")


class ProviderSetupClient:
    """One actual child and one immutable trusted initialization.

    The independent reader owns the process through exit, even if an observer
    times out. It never kills an unknown group or reports a timeout as joined.
    """
    def __init__(self, executable: str, binding: ProviderSetupBinding, *, environment: Mapping[str, str] | None = None):
        initial = binding.data()
        if not Path(executable).is_absolute():
            raise ValueError("provider executable must be an absolute host selection")
        native_default = initial["credential_scope"]["kind"] == "native_default"
        if native_default and environment is None:
            raise ValueError("native-default launch requires the host's explicit fixed environment")
        env = {"PATH": os.defpath, "LANG": "C"} if environment is None else dict(environment)
        allowed = _ENV | {"HOME", "XDG_CONFIG_HOME", "XDG_DATA_HOME"} if native_default else _ENV
        if native_default and (not isinstance(env.get("HOME"), str) or not Path(env["HOME"]).is_absolute()):
            raise ValueError("native-default launch requires fixed absolute HOME")
        if set(env) - allowed or any(not isinstance(v, str) or "\0" in v for v in env.values()):
            raise ValueError("provider launch environment must exclude ambient auth/config overrides")
        encoded = json.dumps({"method": "initialize", "id": "initialize", "binding": initial}, separators=(",", ":"), allow_nan=False).encode() + b"\n"
        if len(encoded) > _MAX_FRAME:
            raise ValueError("provider initialization exceeds frame bound")
        self._condition = threading.Condition()
        self._write_lock = threading.Lock()
        self._events: collections.deque[dict[str, Any]] = collections.deque()
        self._failure: BaseException | None = None
        self._terminal: dict[str, Any] | None = None
        self._joined = False
        self._counter = 0
        self._interaction: str | None = None
        self._pending: str | None = None
        self._cancelled = False
        scope_args = ["--provider-native-default"] if native_default else ["--provider-store-root", initial["credential_scope"]["root"]]
        self._process = None
        self._reader = threading.Thread(target=self._read, name="connect-provider-owner", daemon=True)
        try:
            self._process = subprocess.Popen([executable, "--provider-stdio", *scope_args], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                stderr=subprocess.DEVNULL, env=env, start_new_session=True, close_fds=True)
            self._reader.start()
            self._send({"method": "initialize", "id": "initialize", "binding": initial})
        except BaseException:
            # One returned-handle region, including the first line after Popen.
            # If reader creation failed, this observer itself retains the owner.
            if self._process is not None:
                if self._process.stdin is not None:
                    self._process.stdin.close()
                if not self._reader.is_alive():
                    self._reap()
            raise

    @classmethod
    def adopt(cls, binding: ProviderSetupBinding, *, transport: ProviderSetupTransport) -> ProviderSetupClient:
        """Adopt original provider FileIO pipes without spawning or replacing its child.

        The trusted root retains process authority. After transfer, the same
        client remains recoverable through transport.client even if this caller
        is interrupted before receiving the return value. Close observes only
        SDK I/O and the original direct-child exit, never full root cleanup.
        """
        if cls is not ProviderSetupClient or type(transport) is not ProviderSetupTransport:
            raise ValueError("provider adoption requires the original transport carrier")
        initial = binding.data()
        encoded = json.dumps({"method": "initialize", "id": "initialize", "binding": initial},
                             separators=(",", ":"), allow_nan=False).encode() + b"\n"
        if len(encoded) > _MAX_FRAME:
            raise ValueError("provider initialization exceeds frame bound")
        with transport._lock:
            transport._validate()
            if transport._client is not None or transport._preparation is not None:
                raise ValueError("provider transport already used")
            client = _AdoptedProviderSetupClient(transport, encoded)
            # Retain preparation before Thread.start. All targets wait for the
            # publication gate and cannot access pipes or callbacks beforehand.
            transport._preparation = client
            try:
                for worker in client._workers:
                    client._start_attempts.append(worker)
                    worker.start()
                transport._client = client
            finally:
                # Publication is the sole transfer fact. No second boolean can
                # claim custody before this association or undo it afterward.
                client._activation.set()
                if transport._client is not client:
                    # Even a partially started target can only return from its
                    # inactive gate. Keep uncertain starts retained on carrier.
                    for worker in client._start_attempts:
                        if worker.ident is not None:
                            worker.join(.5)
            return client

    def _reap(self) -> None:
        assert self._process is not None
        while self._process.returncode is None:
            try:
                self._process.wait(timeout=1.0)
            except subprocess.TimeoutExpired:
                continue
            except BaseException:
                # Keep the same direct owner. A failed observation is not reap.
                with self._condition:
                    self._failure = ProviderSetupError("provider exit observation failed")
        with self._condition:
            self._joined = True
            self._condition.notify_all()

    def _send(self, value: Mapping[str, Any]) -> None:
        encoded = json.dumps(value, separators=(",", ":"), allow_nan=False).encode() + b"\n"
        if len(encoded) > _MAX_FRAME:
            raise ProviderSetupError("provider request too large")
        with self._write_lock:
            if self._process.stdin is None or self._process.stdin.closed:
                raise ProviderSetupError("provider input closed")
            self._process.stdin.write(encoded)
            self._process.stdin.flush()

    def _read_frames(self, readline: Callable[[int], bytes]) -> None:
        while True:
            line = readline(_MAX_FRAME + 2)
            if not line:
                break
            if len(line) > _MAX_FRAME or not line.endswith(b"\n"):
                raise ProviderSetupError("invalid provider response frame")
            value = json.loads(line)
            if not isinstance(value, dict) or not isinstance(value.get("kind"), str):
                raise ProviderSetupError("invalid provider response")
            kind = value["kind"]
            with self._condition:
                if self._terminal is not None:
                    raise ProviderSetupError("provider output after terminal result")
                if kind == "initialized":
                    if self._interaction is not None or value.get("protocol") != _PROTOCOL or value.get("id") != "initialize":
                        raise ProviderSetupError("invalid provider initialization response")
                    self._interaction = value.get("interaction_id")
                    if not isinstance(self._interaction, str) or not self._interaction:
                        raise ProviderSetupError("invalid provider interaction")
                elif kind == "selected":
                    selection = ProviderSelection.decode(value.get("selection"))
                    if selection.interaction_id != self._interaction:
                        raise ProviderSetupError("provider selection belongs to another interaction")
                    self._terminal = value
                elif kind in ("cancelled", "failed"):
                    if value.get("interaction_id") != self._interaction or value.get("joined") is not True or value.get("join_scope") != "provider_interaction":
                        raise ProviderSetupError("unjoined provider terminal result")
                    self._terminal = value
                elif kind == "prompt":
                    if value.get("interaction_id") != self._interaction or not isinstance(value.get("prompt"), dict):
                        raise ProviderSetupError("foreign provider prompt")
                    if self._pending is not None:
                        raise ProviderSetupError("overlapping provider prompts")
                    self._pending = value.get("prompt_id")
                    if not isinstance(self._pending, str) or not self._pending:
                        raise ProviderSetupError("invalid provider prompt identity")
                elif kind in ("answered", "prompt_closed"):
                    if value.get("prompt_id") != self._pending:
                        raise ProviderSetupError("foreign provider prompt closure")
                    self._pending = None
                elif kind in ("notice", "cancelling"):
                    if value.get("interaction_id") != self._interaction:
                        raise ProviderSetupError("foreign provider notice")
                else:
                    raise ProviderSetupError("unknown provider response kind")
                if len(self._events) >= 64:
                    raise ProviderSetupError("provider observer fell behind")
                self._events.append(value)
                self._condition.notify_all()
        if self._terminal is None:
            raise ProviderSetupError("provider ended without terminal observation")

    def _read(self) -> None:
        try:
            assert self._process.stdout is not None
            self._read_frames(self._process.stdout.readline)
        except BaseException as error:
            with self._condition:
                self._failure = error
                self._condition.notify_all()
            if self._process.stdin is not None:
                self._process.stdin.close()
        finally:
            # This actual owner remains alive while wait is pending. Observer
            # timeout cannot turn a live child into successful close.
            self._reap()
            if self._process.stdout is not None:
                self._process.stdout.close()

    def next_event(self, timeout: float = 60.0) -> dict[str, Any]:
        if isinstance(timeout, bool) or not isinstance(timeout, (int, float)) or not 0 < timeout <= _MAX_OBSERVATION_SECONDS:
            raise ValueError("observation timeout must be positive and finite, at most 2147483.647 seconds")
        deadline = time.monotonic() + timeout
        with self._condition:
            while not self._events:
                if self._failure is not None:
                    raise ProviderSetupError("provider transport failed") from None
                if self._joined:
                    raise EOFError("provider interaction ended")
                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    raise TimeoutError("provider observation pending; owner retained")
                self._condition.wait(remaining)
            return self._events.popleft()

    def answer(self, prompt_id: str, answer: Mapping[str, Any]) -> None:
        with self._condition:
            if prompt_id != self._pending or self._cancelled or self._terminal is not None:
                raise ProviderSetupError("provider prompt is no longer current")
            self._counter += 1
            request_id = f"answer-{self._counter}"
        self._send({"method": "answer", "id": request_id, "prompt_id": prompt_id, "answer": dict(answer)})

    def close(self, timeout: float = 150.0) -> dict[str, Any] | None:
        if isinstance(timeout, bool) or not isinstance(timeout, (int, float)) or not 0 < timeout <= _MAX_OBSERVATION_SECONDS:
            raise ValueError("observation timeout must be positive and finite, at most 2147483.647 seconds")
        with self._condition:
            send = not self._cancelled and self._terminal is None and not self._joined
            self._cancelled = True
        if send:
            try:
                self._send({"method": "cancel", "id": "close"})
            except (BrokenPipeError, OSError, ProviderSetupError):
                if self._process.stdin is not None:
                    self._process.stdin.close()
        deadline = time.monotonic() + timeout
        with self._condition:
            while not self._joined:
                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    raise TimeoutError("native provider cleanup pending; same child retained")
                self._condition.wait(remaining)
        self._reader.join()
        if self._process.stdin is not None:
            self._process.stdin.close()
        if self._failure is not None or self._process.returncode != 0:
            raise ProviderSetupError("provider operation or transport failed") from None
        return self._terminal


def _adopted_write_once(stream: io.FileIO, data: memoryview) -> int:
    """One raw operation; kept separate for real-endpoint partial-write tests."""
    return stream.write(data)


def _adopted_readline(stream: io.FileIO, limit: int) -> bytes:
    """One bounded original-stream read, with no endpoint wrapper or reopen."""
    return stream.readline(limit)


class _AdoptedProviderSetupClient(ProviderSetupClient):
    # At most two pending requests, in addition to the one actual writer call.
    # Cancellation has a separate one-shot flag and never waits on a pipe here.
    _REQUEST_LIMIT = 2

    def __init__(self, transport: ProviderSetupTransport, initial: bytes):
        self._transport = transport
        self._condition = threading.Condition()
        self._events: collections.deque[dict[str, Any]] = collections.deque()
        self._failure: BaseException | None = None
        self._terminal: dict[str, Any] | None = None
        self._joined = False
        self._counter = 0
        self._interaction: str | None = None
        self._pending: str | None = None
        self._cancelled = False
        self._requests = collections.deque((initial,))
        self._cancel_request = False
        self._writer_stop = False
        self._input_settled = False
        self._output_settled = False
        self._reader_done = False
        self._writer_done = False
        self._monitor_done = False
        self._returncode: int | None = None
        self._activation = threading.Event()
        self._retirement = threading.Event()
        self._retirement_attempted = False
        self._start_attempts: list[threading.Thread] = []
        self._reader = threading.Thread(target=self._activated, args=(self._read_adopted,),
                                        name="connect-provider-adopted-reader", daemon=True)
        self._writer = threading.Thread(target=self._activated, args=(self._write_adopted,),
                                        name="connect-provider-adopted-writer", daemon=True)
        self._monitor = threading.Thread(target=self._activated, args=(self._observe_original,),
                                         name="connect-provider-original-exit", daemon=True)
        self._workers = (self._reader, self._writer, self._monitor)

    def _activated(self, target: Callable[[], None]) -> None:
        self._activation.wait()
        # This immutable association is the transfer commit, including recovery
        # when the caller was interrupted before receiving adopt's return value.
        if self._transport._client is self:
            target()

    def _failed(self, error: BaseException) -> None:
        with self._condition:
            if self._failure is None:
                self._failure = error
            self._writer_stop = True
            self._requests.clear()
            self._cancel_request = False
            self._condition.notify_all()
        self._retirement.set()

    def _settle_endpoint(self, endpoint: io.FileIO, *, input: bool) -> None:
        # Only the worker using this endpoint closes it. Never close a descriptor
        # concurrently with its blocked operation or retry an uncertain close.
        try:
            endpoint.close()
        except BaseException as error:
            self._failed(error)
        else:
            with self._condition:
                if input:
                    self._input_settled = True
                else:
                    self._output_settled = True
                self._condition.notify_all()

    def _send(self, value: Mapping[str, Any]) -> None:
        encoded = json.dumps(value, separators=(",", ":"), allow_nan=False).encode() + b"\n"
        if len(encoded) > _MAX_FRAME:
            raise ProviderSetupError("provider request too large")
        with self._condition:
            if self._failure is not None or self._writer_stop or self._cancelled:
                raise ProviderSetupError("provider input no longer accepts requests")
            if len(self._requests) >= self._REQUEST_LIMIT:
                raise ProviderSetupError("provider request writer is pending")
            # Acceptance into this bounded queue is not native acknowledgement.
            # Only the unchanged native answered/selected events provide that.
            self._requests.append(encoded)
            self._condition.notify_all()

    def _write_adopted(self) -> None:
        try:
            while True:
                with self._condition:
                    while not self._requests and not self._cancel_request and not self._writer_stop:
                        self._condition.wait()
                    if self._writer_stop:
                        break
                    if self._requests:
                        encoded = self._requests.popleft()
                    else:
                        self._cancel_request = False
                        encoded = b'{"method":"cancel","id":"close"}\n'
                remaining = memoryview(encoded)
                while remaining:
                    with self._condition:
                        if self._writer_stop:
                            return
                    count = _adopted_write_once(self._transport.input, remaining)
                    if type(count) is not int or not 0 < count <= len(remaining):
                        raise ProviderSetupError("provider raw write made invalid progress")
                    remaining = remaining[count:]
                # FileIO is unbuffered. There is no flush or replay of a prefix.
        except BaseException as error:
            self._failed(error)
        finally:
            self._settle_endpoint(self._transport.input, input=True)
            with self._condition:
                self._writer_done = True
                self._condition.notify_all()

    def _read_adopted(self) -> None:
        try:
            self._read_frames(lambda limit: _adopted_readline(self._transport.output, limit))
        except BaseException as error:
            self._failed(error)
        finally:
            self._settle_endpoint(self._transport.output, input=False)
            with self._condition:
                self._reader_done = True
                self._writer_stop = True
                self._requests.clear()
                self._cancel_request = False
                self._condition.notify_all()
            self._retirement.set()

    def _observe_original(self) -> None:
        try:
            while True:
                if self._retirement.is_set() and not self._retirement_attempted:
                    # The callback must only notify the retained root operation.
                    # Invocation is on this SDK worker, never the broker loop.
                    self._retirement_attempted = True
                    try:
                        result = self._transport._request_retirement()
                        if inspect.iscoroutine(result):
                            result.close()
                        if result is not None:
                            raise ProviderSetupError("provider retirement notification returned a value")
                    except BaseException as error:
                        self._failed(error)
                with self._condition:
                    code = self._returncode
                if code is not None:
                    if self._retirement_attempted:
                        break
                    self._retirement.wait(.05)
                    continue
                try:
                    code = self._transport._wait_process(.1)
                    if inspect.iscoroutine(code):
                        code.close()
                    if type(code) is not int:
                        raise ProviderSetupError("provider process observation is not an exit code")
                except TimeoutError:
                    # A hostile/immediate pending callback cannot create a busy
                    # loop. This is observation pacing, not a retirement timer.
                    time.sleep(.01)
                    continue
                except BaseException as error:
                    self._failed(error)
                    time.sleep(.01)
                    continue
                with self._condition:
                    self._returncode = code
                    self._writer_stop = True
                    self._requests.clear()
                    self._cancel_request = False
                    self._condition.notify_all()
        finally:
            with self._condition:
                self._monitor_done = True
                self._condition.notify_all()

    def close(self, timeout: float = 150.0) -> dict[str, Any] | None:
        if isinstance(timeout, bool) or not isinstance(timeout, (int, float)) or not 0 < timeout <= _MAX_OBSERVATION_SECONDS:
            raise ValueError("observation timeout must be positive and finite, at most 2147483.647 seconds")
        if threading.current_thread() in self._workers:
            raise ProviderSetupError("provider worker cannot join its own cleanup")
        deadline = time.monotonic() + timeout
        # Resume the same committed preparation if adopt's caller was interrupted
        # immediately after association. No new worker/process is created here.
        self._activation.set()
        with self._condition:
            if not self._cancelled:
                self._cancelled = True
                if self._terminal is None and self._failure is None and not self._writer_stop:
                    self._cancel_request = True
            self._condition.notify_all()
        self._retirement.set()
        with self._condition:
            while not (self._returncode is not None and self._input_settled and self._output_settled
                       and self._reader_done and self._writer_done and self._monitor_done):
                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    raise TimeoutError("provider cleanup pending; original process and I/O retained")
                self._condition.wait(remaining)
        # Completion flags precede thread return. Observe each actual join within
        # this same deadline, rather than treating a flag or process exit as join.
        for worker in self._workers:
            worker.join(max(0.0, deadline - time.monotonic()))
            if worker.is_alive():
                raise TimeoutError("provider SDK worker still pending; original owner retained")
        with self._condition:
            self._joined = True
            self._condition.notify_all()
            if self._failure is not None or self._returncode != 0:
                raise ProviderSetupError("provider operation or transport failed") from None
            return self._terminal
