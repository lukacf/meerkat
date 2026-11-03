#!/usr/bin/env python3
"""Opt-in physical gateway verifier. Supply already-built binaries and SDK roots.

This script builds nothing. The installed positive uses a controlled loopback
model, acquires an offline public library package, and requests one live public
Hacker News read through that package. Other cases send no model turn.
Business acceptance requires the exact fresh input's finalized native result.
Canonical test policy artifacts are NOT host compiler output.
Raw member construction proves native registration composition, not Scoped use.
The held module proves actual constructor entry, not pre-module registration.
"""
from __future__ import annotations

import argparse
import asyncio
import dataclasses
import hashlib
import hmac
import importlib
import importlib.util
import json
import os
from pathlib import Path
import platform
import re
import select
import signal
import socket
import sqlite3
import stat
import subprocess
import sys
import threading
import time
import traceback
import uuid

INIT_ID = 1
MODULE = "held-module"
COMPONENT = "fixture-extension"
MEMBER = "worker-one"
HOLD_LIMIT = 18.0  # Below the production first-module-event timeout of 30s.
RPC_LIMIT = 45.0
EXIT_LIMIT = 45.0
SDK = None
INBOX = None
APP_RUNTIME = None
META_BUILDER = None
META_IDENTITY = None


class FixturePrerequisiteError(RuntimeError):
    """The fixture never established the claimed positive/control condition."""


def require(condition, message):
    if not condition:
        raise AssertionError(message)


def canonical(value):
    return json.dumps(value, sort_keys=True, separators=(",", ":"),
                      ensure_ascii=False, allow_nan=False).encode("utf-8")


def digest(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def save_json(path, value):
    Path(path).write_bytes(json.dumps(value, indent=2, sort_keys=True,
                                    ensure_ascii=False).encode("utf-8") + b"\n")


def alive(pid):
    try:
        os.kill(pid, 0)
        return True
    except ProcessLookupError:
        return False


def until(predicate, timeout, message):
    deadline = time.monotonic() + timeout
    wake = threading.Event()
    while True:
        value = predicate()
        if value:
            return value
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            raise AssertionError(message)
        wake.wait(min(0.02, remaining))


def pipe_identity(fd):
    info = os.fstat(fd)
    return [info.st_dev, info.st_ino, stat.S_IFMT(info.st_mode)]


def fixture_child(argv):
    """Production module subprocess: no first stdout event before release."""
    host, port, nonce, identities_text = argv
    identities = json.loads(identities_text)
    directory = Path("/proc/self/fd") if sys.platform.startswith("linux") else Path("/dev/fd")
    matches = []
    scan_errors = []
    try:
        descriptors = [int(item.name) for item in directory.iterdir() if item.name.isdigit()]
    except OSError as error:
        descriptors = []
        scan_errors.append(str(error))
    for fd in descriptors:
        try:
            identity = pipe_identity(fd)
        except OSError:
            continue  # Directory enumeration may expose its already-closed fd.
        if identity in identities:
            matches.append({"fd": fd, "identity": identity})
    with socket.create_connection((host, int(port)), timeout=10.0) as connection:
        connection.settimeout(25.0)
        stream = connection.makefile("rb")
        entered = {"kind": "entered", "nonce": nonce, "pid": os.getpid(),
                   "ppid": os.getppid(), "private_pipe_matches": matches,
                   "scan_errors": scan_errors, "scan_directory": str(directory)}
        connection.sendall(canonical(entered) + b"\n")
        if stream.readline(4096) != canonical({"release": nonce}) + b"\n":
            return 22
        connection.sendall(canonical({"kind": "released", "nonce": nonce}) + b"\n")
        print(canonical({"event_id": "fixture-first-event", "source": "module",
                         "timestamp_ms": 1, "module": MODULE,
                         "event_type": "fixture.ready", "payload": {}}).decode(),
              flush=True)
        # Only the real gateway process owner terminates this child. Parent
        # socket EOF is deliberately not an exit/cleanup signal for the module.
        while True:
            signal.pause()


def fixture_lifetime_child(argv):
    """First invocation starts; a real replacement retains stdout in a child."""
    first_args = argv[:4]
    marker, reader_host, reader_port, reader_nonce = argv[4:]
    try:
        descriptor = os.open(marker, os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o600)
    except FileExistsError:
        descriptor = None
    if descriptor is not None:
        os.close(descriptor)
        return fixture_child(first_args)
    direct_pid = os.getpid()
    gateway_pid = os.getppid()
    if os.fork() != 0:
        while True:
            signal.pause()
    # The descendant deliberately owns the final stdout writer. Direct-child
    # reap cannot terminate the actual first-line reader until this fd closes.
    identities = json.loads(first_args[3])
    directory = Path("/proc/self/fd") if sys.platform.startswith("linux") else Path("/dev/fd")
    matches = []
    for entry in directory.iterdir():
        if not entry.name.isdigit():
            continue
        try:
            identity = pipe_identity(int(entry.name))
        except OSError:
            continue
        if identity in identities:
            matches.append({"fd": int(entry.name), "identity": identity})
    with socket.create_connection((reader_host, int(reader_port)), timeout=10.0) as connection:
        connection.settimeout(410.0)
        stream = connection.makefile("rb")
        connection.sendall(canonical({"kind": "reader_held", "nonce": reader_nonce,
                                     "pid": os.getpid(), "direct_pid": direct_pid,
                                     "gateway_pid": gateway_pid, "ppid": os.getppid(),
                                     "private_pipe_matches": matches}) + b"\n")
        if stream.readline(4096) != canonical({"release": reader_nonce}) + b"\n":
            os._exit(23)
        os.close(1)
        connection.sendall(canonical({"kind": "writer_closed", "nonce": reader_nonce}) + b"\n")
    os._exit(0)


class EventLog:
    def __init__(self, directory):
        self.directory = directory
        self.started = time.monotonic()
        self.rows = []
        self.lock = threading.Lock()

    def add(self, event, **fields):
        with self.lock:
            self.rows.append({"elapsed_s": time.monotonic() - self.started,
                              "event": event, **fields})

    def flush(self):
        with self.lock:
            save_json(self.directory / "events.json", self.rows)


class Lines:
    """Single owner of a nonblocking output fd; stop before forced peer loss."""
    def __init__(self, stream, label, events):
        self.stream = stream
        self.fd = stream.fileno()
        self.label = label
        self.events = events
        self.stop = threading.Event()
        self.condition = threading.Condition()
        self.lines = []
        self.error = None
        self.eof = False
        os.set_blocking(self.fd, False)
        self.thread = threading.Thread(target=self._read, daemon=True,
                                       name=f"fixture-{label}")
        self.thread.start()

    def _read(self):
        pending = bytearray()
        try:
            while not self.stop.is_set():
                ready, _, _ = select.select([self.fd], [], [], 0.05)
                if not ready:
                    continue
                try:
                    chunk = os.read(self.fd, 8192)
                except BlockingIOError:
                    continue
                if not chunk:
                    self.eof = True
                    break
                pending.extend(chunk)
                while b"\n" in pending:
                    line, _, pending = pending.partition(b"\n")
                    text = line.decode("utf-8", errors="replace")
                    with self.condition:
                        self.lines.append(text)
                        self.condition.notify_all()
                    self.events.add(self.label, line=text)
            if pending:
                with self.condition:
                    self.lines.append(pending.decode("utf-8", errors="replace"))
        except BaseException as error:
            self.error = repr(error)
        finally:
            with self.condition:
                self.condition.notify_all()

    def messages(self):
        with self.condition:
            rows = list(self.lines)
        messages = []
        for line in rows:
            try:
                messages.append(json.loads(line))
            except ValueError as error:
                raise FixturePrerequisiteError(f"non-JSON public stdout: {line!r}") from error
        return messages

    def response(self, identifier, process, timeout=RPC_LIMIT):
        deadline = time.monotonic() + timeout
        while True:
            for message in self.messages():
                if message.get("id") == identifier:
                    return message
            remaining = deadline - time.monotonic()
            if remaining <= 0 or self.eof or self.error:
                raise FixturePrerequisiteError(
                    f"missing public response {identifier}, exit={process.poll()}, "
                    f"EOF={self.eof}, reader_error={self.error}")
            with self.condition:
                self.condition.wait(min(0.1, remaining))

    def close_peer(self):
        self.stop.set()
        self.thread.join(2.0)
        require(not self.thread.is_alive(), f"{self.label} reader did not relinquish fd")
        self.stream.close()
        self.events.add(f"{self.label}_reader_closed")

    def join_after_exit(self):
        self.thread.join(3.0)
        require(not self.thread.is_alive(), f"{self.label} reader outlived reaped gateway")
        require(self.error is None, f"{self.label} reader error: {self.error}")
        self.stream.close()


class ModuleHandshake:
    def __init__(self, events, identities):
        self.events = events
        self.identities = identities
        self.nonce = uuid.uuid4().hex
        self.server = socket.socket()
        self.server.bind(("127.0.0.1", 0))
        self.server.listen(1)
        self.server.settimeout(20.0)
        self.address = self.server.getsockname()
        self.connection = None
        self.stream = None
        self.pid = None
        self.entered_at = None

    def module_config(self):
        return {"id": MODULE, "command": sys.executable,
                "args": [str(Path(__file__).resolve()), "--fixture-child",
                         self.address[0], str(self.address[1]), self.nonce,
                         json.dumps(self.identities, separators=(",", ":"))],
                "restart_policy": "never"}

    def entered(self, process):
        try:
            self.connection, _ = self.server.accept()
            self.connection.settimeout(4.0)
            self.stream = self.connection.makefile("rb")
            observation = json.loads(self.stream.readline(65536))
        except (OSError, ValueError) as error:
            raise FixturePrerequisiteError(
                f"real module constructor handshake absent; gateway exit={process.poll()}") from error
        require(observation.get("kind") == "entered" and observation.get("nonce") == self.nonce,
                "module entered correlation mismatch")
        self.pid = observation["pid"]
        require(observation["ppid"] == process.pid, "module is not actual gateway child")
        require(not observation["scan_errors"], "module descriptor inventory unavailable")
        require(not observation["private_pipe_matches"], "private control pipe leaked to module")
        require(alive(self.pid) and process.poll() is None, "constructor child/gateway already exited")
        self.entered_at = time.monotonic()
        self.events.add("real_module_constructor_entered", **observation)

    def release(self, process):
        require(time.monotonic() - self.entered_at < HOLD_LIMIT,
                "fixture exceeded hold deadline; production startup timeout cannot count as release")
        require(alive(self.pid) and process.poll() is None,
                "gateway/module retired before real constructor release")
        self.connection.sendall(canonical({"release": self.nonce}) + b"\n")
        observation = json.loads(self.stream.readline(4096))
        require(observation == {"kind": "released", "nonce": self.nonce},
                "real module did not acknowledge explicit release")
        self.events.add("real_module_first_event_released", pid=self.pid)

    def assert_reaped(self):
        require(self.pid is not None, "no entered real module to prove cleanup")
        # Process absence is the independent physical observation. A killed
        # peer may reset its control socket when unread input remains; neither
        # a reset nor EOF alone proves that the actual child was reaped.
        until(lambda: not alive(self.pid), 3.0, "actual module remains alive or unreaped")
        control_close = "eof"
        try:
            require(self.stream.read(1) == b"", "module handshake still open after gateway exited")
        except ConnectionResetError:
            control_close = "reset_after_process_absence"
        self.events.add("module_absent_after_gateway_reap", pid=self.pid,
                        control_close=control_close)

    def close(self):
        if self.stream is not None:
            self.stream.close()
        if self.connection is not None:
            self.connection.close()
        self.server.close()


class LocalProviderSentinel:
    """Any model socket connection fails this verifier, even with no HTTP body."""
    def __init__(self):
        self.server = socket.socket()
        self.server.bind(("127.0.0.1", 0))
        self.server.listen()
        self.server.settimeout(0.1)
        self.port = self.server.getsockname()[1]
        self.connections = []
        self.stop = threading.Event()
        self.thread = threading.Thread(target=self._serve, daemon=True)
        self.thread.start()

    def _serve(self):
        while not self.stop.is_set():
            try:
                connection, address = self.server.accept()
            except socket.timeout:
                continue
            except OSError:
                return
            self.connections.append({"address": list(address), "at": time.monotonic()})
            with connection:
                try:
                    connection.sendall(b"HTTP/1.1 503 Fixture forbids model work\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                except OSError:
                    pass

    def close(self, *, drain=False):
        self.stop.set()
        self.thread.join(2.0)
        try:
            require(not self.thread.is_alive(), "provider sentinel did not retire")
            if drain:
                # Installed cases call this only after the owned children stop.
                # The joined thread may already have accepted a connection;
                # drain the existing listen backlog before taking its final log.
                self.server.setblocking(False)
                for _ in range(256):
                    try:
                        connection, address = self.server.accept()
                    except BlockingIOError:
                        break
                    self.connections.append({"address": list(address), "at": time.monotonic()})
                    connection.close()
                else:
                    raise AssertionError("provider sentinel drain exceeded its bound")
        finally:
            self.server.close()


def http_probe(port):
    """A TCP listener can be reserved before Ready. Only HTTP bytes mean serve."""
    try:
        with socket.create_connection(("127.0.0.1", port), timeout=0.15) as connection:
            connection.settimeout(0.15)
            connection.sendall(b"GET / HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n")
            return connection.recv(64)
    except (OSError, socket.timeout):
        return b""


class HttpObserver:
    def __init__(self, port, events):
        self.port = port
        self.events = events
        self.responses = []
        self.probes = 0
        self.stop = threading.Event()
        self.thread = threading.Thread(target=self._watch, daemon=True)
        self.thread.start()

    def _watch(self):
        while not self.stop.is_set():
            result = http_probe(self.port)
            self.probes += 1
            if result:
                self.responses.append(result.decode("ascii", errors="replace"))
                self.events.add("http_bytes_observed", prefix=self.responses[-1])
            self.stop.wait(0.01)

    def close(self):
        self.stop.set()
        self.thread.join(2.0)
        require(not self.thread.is_alive(), "HTTP observer did not retire")


def vacant_port():
    with socket.socket() as reserved:
        reserved.bind(("127.0.0.1", 0))
        return reserved.getsockname()[1]


def write_artifacts(root, mapped_member):
    root.mkdir(parents=True)
    bundle = root / "shared/policy/bundle.1"
    bundle.mkdir(parents=True)
    (root / "shared/activations").mkdir()
    (root / "shared/control").mkdir()
    source = bundle / "policy.json"
    source.write_bytes(b'{"schema":1}')
    local = bundle / "toolkit-local-policy.json"
    local.write_bytes(canonical({
        "schema": 1, "tenant": "household", "deployment": "home",
        "source_policy_sha256": digest(source), "components": [COMPONENT],
        "channels": [{
            "component": COMPONENT, "channel_id": "fixture",
            "service_subject": "service:fixture", "provider_account": "account:fixture",
            "sender_id": "sender", "requester": {"kind": "human", "id": "person:original"},
            "target_identity": MEMBER,
            "member": {"mob_id": "fixture", "role": "worker", "member": mapped_member},
            "destination": {"realm_id": "mob.fixture",
                            "receiver": {"kind": "shared_agent", "id": MEMBER},
                            "scope": {"scope_type": "mob", "mob_id": "fixture"}},
            "required_surfaces": ["channel:fixture"],
            "ceiling": {"actions": [], "services": [], "frontends": [], "output_audiences": []},
            "lifetime_ms": 60000, "max_hops": 0, "local_checkpoint": True, "local_read": False,
        }]}))
    manifest = bundle / "manifest.json"
    manifest.write_bytes(canonical({"artifacts": [
        {"path": "policy.json", "sha256": digest(source)},
        {"path": "toolkit-local-policy.json", "sha256": digest(local)}]}))
    activation = root / "shared/activations/1.json"
    activation.write_bytes(canonical({"activation_id": "1", "policy_bundle": {
        "path": "shared/policy/bundle.1", "policy_digest": "policy-v1:sha256:" + digest(manifest)}}))
    (root / "current").symlink_to("shared/activations/1.json")
    issuer = root / "issuer.key"
    issuer.write_bytes(bytes([17]) * 32)
    issuer.chmod(0o600)
    inbox = root / "channels.sqlite3"
    INBOX.SQLiteChannelInbox(path=inbox)
    selected = lambda path: SDK.SelectedHostArtifact(str(path), digest(path))
    selection = SDK.HostBootstrapSelection(str(root), selected(activation), selected(manifest),
                                          selected(local))
    save_json(root / "fixture-selection.json", dataclasses.asdict(selection))
    return selection


def config_text(port, *, acquisition=False):
    source = f'''[self_hosted]
default_model = "fixture-model"

[self_hosted.servers.local]
base_url = "http://127.0.0.1:{port}/v1"

[self_hosted.models.fixture-model]
server = "local"
remote_model = "fixture-model"

[realm."mob.fixture"]
default_binding = "local"

[realm."mob.fixture".backend.local]
provider = "self_hosted"
backend_kind = "self_hosted"
server = "local"

[realm."mob.fixture".auth.local_auth]
provider = "self_hosted"
auth_method = "none"
source = {{ kind = "platform_default" }}

[realm."mob.fixture".binding.local]
backend_profile = "local"
auth_profile = "local_auth"
'''
    if acquisition:
        for model in ("fixture-original-model", "fixture-helper-model"):
            source += '\n[self_hosted.models.' + model + ']\nserver = "local"\nremote_model = "' + model + '"\n'
    return source


MOB_CONFIG = '''[mob]
id = "fixture"

[profiles.worker]
model = "fixture-model"
provider = "self_hosted"
self_hosted_server_id = "local"
runtime_mode = "turn_driven"

[profiles.worker.tools]
comms = true
'''


def process_env(directory):
    home = directory / "isolated-home"
    home.mkdir()
    temp = directory / "tmp"
    temp.mkdir()
    # No inherited provider credentials, proxy variables, service endpoints,
    # user config, control descriptors or Python imports in the native child.
    return {"PATH": "/usr/bin:/bin:/usr/sbin:/sbin", "HOME": str(home),
            "TMPDIR": str(temp), "XDG_CONFIG_HOME": str(home / "config"),
            "XDG_DATA_HOME": str(home / "data"), "XDG_STATE_HOME": str(home / "state"),
            "RUST_LOG": "info", "TOKIO_WORKER_THREADS": "4", "LANG": "C.UTF-8"}


class Gateway:
    def __init__(self, args, directory, state, selection, native, *, memory_fault=False):
        self.args = args
        self.directory = directory
        directory.mkdir()
        self.events = EventLog(directory)
        self.native = native
        self.private = None
        self.process = None
        self.stdout = None
        self.stderr = None
        self.module = None
        self.http = None
        self.forced_cleanup = False
        self.cleaned = False
        self.reaped = False
        self.port = vacant_port()
        self.provider = LocalProviderSentinel()
        config = directory / "meerkat.toml"
        config.write_text(config_text(self.provider.port))
        identities = []
        inherited = []
        parent_ends = []
        try:
            binary = args.toolkit_gateway if native else args.raw_gateway
            command = [str(binary), "--persistent"]
            if native:
                read_child, write_parent = os.pipe()
                read_parent, write_child = os.pipe()
                inherited = [read_child, write_child]
                parent_ends = [write_parent, read_parent]
                identities = [pipe_identity(read_child), pipe_identity(write_child)]
                command += ["--host-control-read-fd", str(read_child),
                            "--host-control-write-fd", str(write_child)]
            self.module = ModuleHandshake(self.events, identities)
            self.events.add("launch", binary=str(binary), binary_sha256=digest(binary),
                            private_pipe_identities=identities, http_port=self.port)
            self.process = subprocess.Popen(command, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                            stderr=subprocess.PIPE, bufsize=0, close_fds=True,
                                            pass_fds=tuple(inherited), start_new_session=True,
                                            cwd=directory, env=process_env(directory))
            for fd in inherited:
                os.close(fd)
            inherited.clear()
            self.stdout = Lines(self.process.stdout, "stdout", self.events)
            self.stderr = Lines(self.process.stderr, "stderr", self.events)
            if native:
                self.private = SDK.PrivateHostClient(*parent_ends, timeout=4.0)
                parent_ends.clear()
                self.private.bootstrap(selection)
                observed = self.private.open_component(COMPONENT)
                require(observed.state.value == "open", "actual native component did not open")
                self.events.add("native_bootstrap_and_open", observation={
                    "incarnation": str(observed.incarnation), "component": observed.component,
                    "revision": observed.revision, "state": observed.state.value})
            init = {"mob_config": MOB_CONFIG, "modules": [self.module.module_config()],
                    "persistent_state": str(state), "has_session_builder": False,
                    "runtime_options": {"http_listen": f"127.0.0.1:{self.port}",
                                        "meerkat_config_path": str(config)}}
            if memory_fault:
                state.mkdir(parents=True, exist_ok=True)
                ledger = state / "memory-ledger-state.json"
                ledger.write_bytes(b"{fixture-deliberately-corrupt-memory-ledger")
                init["runtime_options"]["memory_config"] = {"backend": "local_json"}
                self.events.add("corrupt_local_memory_fixture", path=str(ledger), sha256=digest(ledger))
            save_json(directory / "init-params.json", init)
            self.send(INIT_ID, "meta/init", init)
            if not memory_fault:
                self.module.entered(self.process)
            require(not self.ready_messages(), "Ready published before real first module event")
            self.http = HttpObserver(self.port, self.events)
            require(not http_probe(self.port), "HTTP served while actual constructor was held")
        except BaseException:
            for fd in inherited + parent_ends:
                os.close(fd)
            self.cleanup(failure=True)
            raise

    def send(self, identifier, method, params):
        self.send_bytes(canonical({"jsonrpc": "2.0", "id": identifier,
                                   "method": method, "params": params}) + b"\n")
        self.events.add("public_request", id=identifier, method=method)

    def send_bytes(self, data):
        require(self.process is not None and self.process.stdin is not None,
                "public input already relinquished")
        self.process.stdin.write(data)
        self.process.stdin.flush()

    def ready_messages(self):
        return [row for row in self.stdout.messages()
                if row.get("id") == INIT_ID and isinstance(row.get("result"), dict)]

    def release_ready(self):
        self.module.release(self.process)
        response = self.stdout.response(INIT_ID, self.process)
        if "error" in response:
            raise FixturePrerequisiteError(f"actual constructor refused fixture: {response}")
        result = response["result"]
        require(result.get("http_base_url") == f"http://127.0.0.1:{self.port}",
                "Ready did not report fixed selected HTTP address")
        require(MODULE in result.get("loaded_modules", []), "Ready omitted actual held module")
        until(lambda: http_probe(self.port).startswith(b"HTTP/"), 5.0,
              "successful Ready did not start actual HTTP service")
        self.events.add("ready_and_http", response=response)
        return result

    def spawn(self, expected=True):
        self.send(2, "meta/spawn_member", {"profile": "worker", "meerkat_id": MEMBER})
        response = self.stdout.response(2, self.process)
        if expected:
            if (response.get("result") or {}).get("accepted") is not True:
                raise FixturePrerequisiteError(f"actual positive member spawn failed: {response}")
            require(response["result"].get("meerkat_id") == MEMBER, "spawn target changed")
        else:
            require(response.get("error", {}).get("code") == -32602 and
                    "Toolkit member destination is unavailable" in
                    response.get("error", {}).get("message", ""),
                    f"negative did not reach exact native registration hook: {response}")
            require("result" not in response or response["result"] is None,
                    "failed registration also returned success")
        self.send(3, "meta/member_status", {"member_id": MEMBER})
        status = self.stdout.response(3, self.process)
        if expected:
            session = (status.get("result") or {}).get("current_session_id")
            require(isinstance(session, str) and bool(session),
                    f"positive spawn has no actual current session: {status}")
            require(status["result"].get("error") is None, f"positive member is failed: {status}")
        else:
            snapshot = status.get("result")
            if isinstance(snapshot, dict):
                require(status.get("error") is None and
                        snapshot.get("status") in ("unknown", "broken") and
                        type(snapshot.get("is_final")) is bool and
                        type(snapshot.get("tokens_used")) is int and
                        snapshot["tokens_used"] >= 0 and "error" in snapshot and
                        (snapshot["error"] is None or isinstance(snapshot["error"], str)),
                        f"negative status is not a real unavailable-member snapshot: {status}")
                require(not snapshot.get("current_session_id"),
                        f"refused registration still advertises a current session: {status}")
            else:
                require(snapshot is None and status.get("error") == {
                    "code": -32000,
                    "message": f"member_status failed: mob member not found: {MEMBER}",
                }, f"status query failure is not proof of absent session: {status}")
        self.events.add("actual_member_spawn", accepted=expected, response=response, status=status)
        return status

    def shutdown(self):
        self.send(90, "meta/shutdown", {})
        response = self.stdout.response(90, self.process, timeout=EXIT_LIMIT)
        result = response.get("result", {})
        require(result.get("shutdown") is True and result.get("runtime_cleanup_completed") is True,
                f"actual runtime shutdown did not attest cleanup: {response}")
        if self.native:
            require(result.get("host_cleanup_completed") is True,
                    f"actual host shutdown did not attest cleanup: {response}")
        self.events.add("shutdown_attestation", response=response)
        self.assert_exit(expected_code=0)

    def observe_public_loss(self, kind):
        require(self.private is not None, "native loss requires actual private owner")
        if kind == "stdin_eof":
            self.process.stdin.close()
            self.process.stdin = None
            self.events.add("last_public_stdin_writer_closed")
        elif kind == "stdout_loss":
            self.stdout.close_peer()
            # Actual reader generates a wire-error response before constructor
            # returns, so writing that response exercises EPIPE, not a mock.
            self.send_bytes(b"{deliberately-invalid-json\n")
            self.events.add("malformed_public_line_for_actual_epipe")
        else:
            raise AssertionError(kind)
        wake = threading.Event()
        deadline = min(time.monotonic() + 8.0, self.module.entered_at + HOLD_LIMIT - 2.0)
        observations = 0
        while time.monotonic() < deadline:
            observations += 1
            try:
                self.private.open_component(COMPONENT)
            except SDK.HostControlError as error:
                require(str(error) == "native host refused: denied",
                        f"transport error is not owner-close evidence: {error}")
                self.events.add("actual_native_owner_refused_after_public_loss", kind=kind,
                                observations=observations, error=str(error))
                return
            wake.wait(0.01)
        raise AssertionError(f"actual {kind} did not close native admission before module release")

    def observe_private_eof(self):
        # Half-close only the last command writer, not the SDK response owner.
        # A normal client.close() would manufacture local failure and could not
        # demonstrate that the gateway consumed EOF or invalidated admission.
        with self.private._write_lock:
            fd, self.private._command_fd = self.private._command_fd, None
            require(fd is not None, "last command writer already gone")
            os.close(fd)
        self.events.add("last_private_command_writer_closed")
        self.private._reader.join(6.0)
        require(not self.private._reader.is_alive(), "private EOF did not reach native response EOF")
        require(str(self.private._failure) == "native host pipe closed; settlement is unknown",
                f"missing actual peer EOF: {self.private._failure}")
        self.events.add("actual_private_peer_eof", error=str(self.private._failure),
                        settlement_receipt=False)

    def observe_private_protocol_failure(self):
        require(self.private is not None, "private protocol fault requires the native owner")
        frame = b"{deliberately-invalid-private-json\n"
        # Use the actual retained command writer. Do not call client.close():
        # only the native reader rejecting this frame may initiate peer EOF.
        with self.private._write_lock:
            with self.private._condition:
                require(self.private._failure is None and not self.private._pending,
                        "private client was failed or had an outstanding request before injection")
            fd = self.private._command_fd
            require(fd is not None, "private command writer already gone")
            _, writable, _ = select.select([], [fd], [], 1.0)
            require(fd in writable, "private command pipe was not writable")
            require(os.write(fd, frame) == len(frame), "malformed private frame was not fully written")
        self.events.add("malformed_private_line_written", frame_bytes=len(frame))
        self.private._reader.join(6.0)
        require(not self.private._reader.is_alive(),
                "private protocol rejection did not reach native response EOF")
        require(str(self.private._failure) == "native host pipe closed; settlement is unknown",
                f"missing actual peer EOF after malformed frame: {self.private._failure}")
        self.events.add("actual_private_peer_eof_after_protocol_fault",
                        error=str(self.private._failure), settlement_receipt=False)

    def complete_lost_constructor(self, kind):
        require(not self.ready_messages(), "Ready published while actual constructor held")
        require(self.process.poll() is None and alive(self.module.pid),
                "loss dropped constructor or killed held physical participant")
        require(not self.http.responses, "HTTP served before held release")
        self.module.release(self.process)
        self.assert_exit(expected_code=1)
        require(not self.ready_messages(), "late constructor completion published Ready after close")
        require(not self.http.responses, "late constructor completion served HTTP after close")
        if kind == "private_protocol":
            diagnostics = [line for line in self.stderr.lines if line.startswith("toolkit_gateway: ")]
            suffix = "; private host cleanup also failed: private host protocol was rejected"
            require(len(diagnostics) == 1 and diagnostics[0].endswith(suffix),
                    f"gateway lost either failure diagnostic: {diagnostics}")
            primary = diagnostics[0][len("toolkit_gateway: "):-len(suffix)]
            # Closed admission can be reported by bootstrap or the final Ready
            # owner. An Init diagnostic must preserve its actual public error.
            replies = [row for row in self.stdout.messages() if row.get("id") == INIT_ID]
            if primary == "host transport closed before gateway admission":
                require(not replies, "host-close diagnostic conflicts with a public init reply")
            elif primary == "gateway shutdown did not attest complete settlement":
                # A refused seeded restart can leave the mob Stopped. Preserve
                # that native failure without calling physical reaping settlement.
                require(replies == [{"id": INIT_ID, "jsonrpc": "2.0", "error": {
                    "code": -32603, "message": "native host composition refused admission"}}],
                        "shutdown diagnostic did not retain the actual admission-refused init reply")
                require(any("gateway runtime shutdown completed without cleanup attestation" in line and
                            "mob_stop=Err(Mob(InvalidTransition { from: Stopped, to: Stopped }))" in line
                            for line in self.stderr.lines),
                        "shutdown diagnostic did not match the observed stopped-restart failure")
            else:
                prefix = "gateway initialization failed: "
                require(primary.startswith(prefix), f"unexpected gateway failure: {primary}")
                reported = json.loads(primary[len(prefix):])
                require(len(replies) == 1 and reported == replies[0] and
                        isinstance(reported.get("error"), dict) and "result" not in reported,
                        "gateway diagnostic did not retain the actual init-error reply")
            self.events.add("both_gateway_and_private_protocol_errors_retained",
                            gateway_error=primary, private_error="private host protocol was rejected")
        self.events.add("late_constructor_settled_without_ready_or_http", kind=kind,
                        http_probes=self.http.probes,
                        stdout_observable=kind != "stdout_loss",
                        scoped_execution_proven=False,
                        registration_before_held_module_proven=False)

    def assert_exit(self, expected_code, *, module_expected=True):
        try:
            code = self.process.wait(EXIT_LIMIT)
        except subprocess.TimeoutExpired as error:
            raise AssertionError("gateway did not settle naturally within deadline") from error
        self.reaped = True
        self.events.add("gateway_reaped_without_harness_signal", exit_code=code)
        require(code == expected_code, f"gateway exit {code}; expected {expected_code}")
        if module_expected:
            self.module.assert_reaped()
        if not self.stdout.stream.closed:
            self.stdout.join_after_exit()
        self.stderr.join_after_exit()
        self.http.close()
        require(not self.provider.connections, "fixture unexpectedly attempted model work")
        with socket.socket() as probe:
            probe.settimeout(0.2)
            require(probe.connect_ex(("127.0.0.1", self.port)) != 0,
                    "HTTP socket remained listening after gateway exited")
        self.events.add("no_remaining_http_listener")

    def cleanup(self, failure=False):
        if self.cleaned:
            return
        if self.process is not None and self.process.poll() is None:
            self.forced_cleanup = True
            self.events.add("unwind_only_forced_process_group_kill", failure=True)
            try:
                os.killpg(self.process.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            self.process.wait(10.0)
        if self.module is not None and self.module.pid is not None and alive(self.module.pid):
            self.forced_cleanup = True
            require(os.getpgid(self.module.pid) == self.process.pid,
                    "refuse unwind signal outside the exact fixture process group")
            self.events.add("unwind_only_leaked_module_kill", pid=self.module.pid, failure=True)
            os.kill(self.module.pid, signal.SIGKILL)
            until(lambda: not alive(self.module.pid), 5.0, "harness could not retire leaked fixture module")
        if self.process is not None and self.process.stdin is not None:
            self.process.stdin.close()
            self.process.stdin = None
        if self.private is not None:
            self.private.close()
        if self.http is not None:
            self.http.close()
        if self.module is not None:
            self.module.close()
        for output in (self.stdout, self.stderr):
            if output is not None and not output.stream.closed:
                output.stop.set()
                output.thread.join(2.0)
                output.stream.close()
        self.provider.close()
        if not failure:
            require(self.reaped and not self.forced_cleanup,
                    "successful case depended on harness process termination")
        self.events.flush()
        self.cleaned = True


def run_case(args, name, native=True, mismatch=False, loss=None):
    directory = args.output / name
    directory.mkdir()
    report = {"case": name, "status": "running", "native": native,
              "fixture_policy_source": "canonical native test artifact shape; not a host compiler",
              "scoped_execution_proven": False, "registration_before_held_module_proven": False}
    gateways = []
    try:
        selection = write_artifacts(directory / "deployment", "policy-only-member" if mismatch else MEMBER) if native else None
        state = directory / "persistent-state"
        first = Gateway(args, directory / "positive-seed" if loss else directory / "process", state, selection, native)
        gateways.append(first)
        first.release_ready()
        status = first.spawn(expected=not mismatch)
        report["member_status"] = status
        first.shutdown()
        first.cleanup()
        report["positive_control_proven"] = not mismatch
        if loss:
            # Same durable state and exact policy selection as successful seed.
            # Resume may defer member materialization until module startup;
            # nothing here claims the hook already ran at the held boundary.
            restarted = Gateway(args, directory / "loss-restart", state, selection, native=True)
            gateways.append(restarted)
            if loss == "private_eof":
                restarted.observe_private_eof()
            elif loss == "private_protocol":
                restarted.observe_private_protocol_failure()
            else:
                restarted.observe_public_loss(loss)
            restarted.complete_lost_constructor(loss)
            restarted.cleanup()
            report["seeded_restart_loss_proven"] = loss
        report["status"] = "passed"
    except BaseException as error:
        report["status"] = "prerequisite_failed" if isinstance(error, FixturePrerequisiteError) else "failed"
        report["error"] = f"{type(error).__name__}: {error}"
        report["traceback"] = traceback.format_exc()
    finally:
        for gateway in gateways:
            try:
                gateway.cleanup(failure=report["status"] != "passed")
            except BaseException as error:
                report["status"] = "failed"
                report.setdefault("cleanup_errors", []).append(repr(error))
        report["forced_cleanup"] = any(gateway.forced_cleanup for gateway in gateways)
        save_json(directory / "report.json", report)
    return report



def run_memory_cleanup_fault(args):
    directory = args.output / "native-memory-startup-cleanup"
    directory.mkdir()
    report = {"case": "native-memory-startup-cleanup", "status": "running",
              "scoped_execution_proven": False, "expected_failure_phase": "after_successful_module_first_event"}
    gateway = None
    try:
        selection = write_artifacts(directory / "deployment", MEMBER)
        gateway = Gateway(args, directory / "process", directory / "persistent-state",
                          selection, native=True, memory_fault=True)
        deadline = time.monotonic() + 20.0
        response = None
        while time.monotonic() < deadline:
            # Both runs must enter this actual child before the memory read.
            # The predecessor leaks it; the correction must physically reap it.
            ready, _, _ = select.select([gateway.module.server], [], [], 0.05)
            if ready and gateway.module.pid is None:
                gateway.module.entered(gateway.process)
                gateway.module.release(gateway.process)
            response = next((row for row in gateway.stdout.messages()
                             if row.get("id") == INIT_ID), None)
            if response is not None:
                break
        if response is None:
            raise FixturePrerequisiteError("corrupt memory did not produce the actual init refusal")
        message = (response.get("error") or {}).get("message", "")
        require("Runtime bootstrap failed:" in message and "MemoryBackend(InvalidStoreData(" in message,
                f"expected exact local memory decode failure, got {response}")
        report["refusal"] = response
        code = gateway.process.wait(EXIT_LIMIT)
        gateway.reaped = True
        gateway.events.add("gateway_reaped_after_memory_error", exit_code=code)
        require(code == 1, f"failed bootstrap returned unexpected exit {code}")
        report["module_entered"] = gateway.module.pid is not None
        report["leaked_module_after_gateway_exit"] = bool(gateway.module.pid and alive(gateway.module.pid))
        require(gateway.module.pid is not None,
                "fixture did not reach post-module memory failure; an earlier refusal is not this regression")
        require(not report["leaked_module_after_gateway_exit"],
                "actual successful module was leaked on later memory-read bootstrap failure")
        gateway.assert_exit(1)
        require(not gateway.ready_messages() and not gateway.http.responses,
                "failed memory bootstrap published Ready or served HTTP")
        gateway.events.add("memory_refused_and_actual_module_reaped")
        gateway.cleanup()
        report["status"] = "passed"
    except BaseException as error:
        report["status"] = "prerequisite_failed" if isinstance(error, FixturePrerequisiteError) else "failed"
        report["error"] = f"{type(error).__name__}: {error}"
        report["traceback"] = traceback.format_exc()
    finally:
        if gateway is not None:
            try:
                gateway.cleanup(failure=report["status"] != "passed")
            except BaseException as error:
                report["status"] = "failed"
                report.setdefault("cleanup_errors", []).append(repr(error))
            report["forced_cleanup"] = gateway.forced_cleanup
        save_json(directory / "report.json", report)
    return report


def run_shutdown_reader(args):
    """Actual native gateway, physical reader and unchanged SDK shutdown clock."""
    return asyncio.run(_run_shutdown_reader(args))


async def _run_shutdown_reader(args):
    directory = args.output / "native-shutdown-held-reader"
    directory.mkdir()
    events = EventLog(directory)
    report = {"case": "native-shutdown-held-reader", "status": "running",
              "fixture_policy_source": "canonical native test artifact shape; not a host compiler",
              "scoped_execution_proven": False, "production_observation_horizon_s": 312,
              "production_sdk_horizon_s": 347, "forced_cleanup": False}
    progress_event = asyncio.Event()
    wire_lock = threading.Lock()
    outgoing = []
    responses = []
    notifications = []

    class ObservedTransport(SDK.ToolkitPersistentTransport):
        # Instrumentation delegates every real transport operation unchanged.
        # It does not mint replies, shorten deadlines or replace classification.
        def _write_line(self, value, **kwargs):
            with wire_lock:
                outgoing.append(value)
            events.add("sdk_wire_out", message=value)
            return super()._write_line(value, **kwargs)

        def _handle_callback(self, value, **kwargs):
            with wire_lock:
                notifications.append(value)
            events.add("sdk_wire_notification", message=value)
            return super()._handle_callback(value, **kwargs)

        def _send_sync_running(self, request, *, timeout=None, **kwargs):
            response = super()._send_sync_running(request, timeout=timeout, **kwargs)
            with wire_lock:
                responses.append(response)
            events.add("sdk_wire_response", message=response)
            return response

    async def callback(method, params):
        if method == "meta/shutdown_progress":
            progress_event.set()
            return None
        # Never supply an application decision for an unexpected real callback.
        # The real SDK emits its ordinary error response if an ID is present.
        raise FixturePrerequisiteError(f"unexpected application callback: {method}")

    transport = None
    process = None
    original = None
    private = None
    reader_server = None
    reader_connection = None
    reader_stream = None
    reader = None
    provider = None
    stop_task = None
    progress_wait = None
    init_task = None
    spawn_task = None
    writer_released = False
    natural_reap = False
    success = False
    try:
        selection = write_artifacts(directory / "deployment", MEMBER)
        provider = LocalProviderSentinel()
        config = directory / "meerkat.toml"
        config.write_text(config_text(provider.port))
        environment = process_env(directory)
        environment["META_GATEWAY_STDERR_FILE"] = str(directory / "gateway-stderr.log")
        transport = ObservedTransport(str(args.toolkit_gateway), selection, timeout=RPC_LIMIT)
        # The SDK constructor normally merges os.environ. This isolated fixture
        # selects its complete launch environment; no transport semantics change.
        transport._env = environment
        transport.set_callback_handler(callback)
        transport.start()
        process = transport._process
        private = transport.host_control
        require(process is not None and private is not None, "actual SDK native startup missing")
        private_reader = private._reader
        public_reader = transport._reader_thread
        require(private.open_component(COMPONENT).state.value == "open",
                "actual native component did not open")
        identities = [pipe_identity(private._command_fd), pipe_identity(private._response_fd)]
        original = ModuleHandshake(events, identities)
        reader_server = socket.socket()
        reader_server.bind(("127.0.0.1", 0))
        reader_server.listen(1)
        reader_server.settimeout(20.0)
        reader_address = reader_server.getsockname()
        reader_nonce = uuid.uuid4().hex
        module = original.module_config()
        module["args"][1] = "--fixture-lifetime-child"
        module["args"] += [str(directory / "first-module-invocation"),
                           reader_address[0], str(reader_address[1]), reader_nonce]
        port = vacant_port()
        init = {"mob_config": MOB_CONFIG, "modules": [module],
                "persistent_state": str(directory / "persistent-state"), "has_session_builder": False,
                "runtime_options": {"http_listen": f"127.0.0.1:{port}",
                                    "meerkat_config_path": str(config)}}
        save_json(directory / "init-params.json", init)
        events.add("launch", pid=process.pid, binary=str(args.toolkit_gateway),
                   binary_sha256=digest(args.toolkit_gateway), http_port=port,
                   private_pipe_identities=identities)

        async def rpc(identifier, method, params, timeout=RPC_LIMIT):
            return await transport.send_async({"jsonrpc": "2.0", "id": identifier,
                                               "method": method, "params": params}, timeout=timeout)

        init_task = asyncio.create_task(rpc(INIT_ID, "meta/init", init))
        await asyncio.to_thread(original.entered, process)
        require(not init_task.done(), "constructor returned before actual module release")
        await asyncio.to_thread(original.release, process)
        ready = await init_task
        result = ready.get("result") or {}
        if result.get("http_base_url") != f"http://127.0.0.1:{port}" or MODULE not in result.get("loaded_modules", []):
            raise FixturePrerequisiteError(f"actual native positive bootstrap failed: {ready}")
        require(result.get("stdio_shutdown_handshake") is True and
                result.get("stdio_shutdown_horizon_ms") == 347000 and
                transport._shutdown_horizon_seconds == 347.0,
                "actual SDK did not negotiate the unchanged production shutdown horizon")
        await asyncio.to_thread(until, lambda: http_probe(port).startswith(b"HTTP/"), 5.0,
                                "successful Ready did not serve selected HTTP address")
        spawned = await rpc(2, "meta/spawn_member", {"profile": "worker", "meerkat_id": MEMBER})
        status = await rpc(3, "meta/member_status", {"member_id": MEMBER})
        session = (status.get("result") or {}).get("current_session_id")
        if (spawned.get("result") or {}).get("accepted") is not True or not isinstance(session, str) or not session:
            raise FixturePrerequisiteError(f"actual native registration prerequisite failed: {spawned}, {status}")
        require((status.get("result") or {}).get("error") is None, "positive member is broken")
        report["positive_member_status"] = status
        events.add("positive_native_registration_and_http_proven", session=session)

        # Init discovers every configured module. A dynamic replacement of the
        # existing module enters the real startup owner without hidden config.
        spawn_task = asyncio.create_task(rpc(4, "meta/spawn_member", {"module_id": MODULE}, timeout=75.0))
        reader_connection, _ = await asyncio.to_thread(reader_server.accept)
        reader_connection.settimeout(5.0)
        reader_stream = reader_connection.makefile("rb")
        reader = json.loads(await asyncio.to_thread(reader_stream.readline, 65536))
        require(reader.get("kind") == "reader_held" and reader.get("nonce") == reader_nonce,
                "actual descendant writer correlation missing")
        require(reader.get("gateway_pid") == process.pid and reader.get("ppid") == reader.get("direct_pid"),
                "writer is not actual replacement child descendant")
        require(all(type(reader.get(key)) is int and reader[key] > 1 for key in ("pid", "direct_pid")),
                "actual fixture process identities missing")
        require(reader["pid"] != reader["direct_pid"] and reader["direct_pid"] != original.pid and
                alive(reader["pid"]) and alive(reader["direct_pid"]), "physical replacement is absent")
        require(not reader.get("private_pipe_matches"), "private host pipe leaked to actual descendant")
        events.add("actual_replacement_descendant_holds_stdout", **reader)
        failed = await spawn_task
        require(failed.get("error") == {"code": -32602,
                    "message": "Invalid params: Runtime(Process(Timeout { timeout_ms: 30000 }))"},
                f"replacement failed before the exact actual first-event timeout: {failed}")
        await asyncio.to_thread(until, lambda: not alive(reader["direct_pid"]), 5.0,
                                "actual timed-out replacement child not reaped")
        require(alive(original.pid) and alive(reader["pid"]),
                "expected original module and descendant did not remain physical")
        events.add("replacement_timeout_with_direct_child_absent", response=failed,
                   direct_pid=reader["direct_pid"], writer_pid=reader["pid"])

        events.flush()
        print("native-shutdown-held-reader: real replacement timeout established; waiting for 312s owner observation", flush=True)
        shutdown_started = time.monotonic()
        stop_task = asyncio.create_task(asyncio.to_thread(transport.stop))
        progress_wait = asyncio.create_task(progress_event.wait())
        observed, _ = await asyncio.wait((progress_wait, stop_task), timeout=332.0,
                                         return_when=asyncio.FIRST_COMPLETED)
        require(progress_wait in observed and progress_event.is_set(),
                "actual SDK stop finished or observation deadline elapsed before physical progress")
        progress_elapsed = time.monotonic() - shutdown_started
        require(312.0 <= progress_elapsed < 337.0,
                f"progress is not the unchanged 312s observation: {progress_elapsed}")
        with wire_lock:
            progress = [value for value in notifications if value.get("method") == "meta/shutdown_progress"]
            shutdown_requests = [value for value in outgoing if value.get("method") == "meta/shutdown"]
            responses_at_progress = list(responses)
        require(len(progress) == 1 and len(shutdown_requests) == 1, "actual shutdown wire correlation ambiguous")
        notification, request = progress[0], shutdown_requests[0]
        request_id = request["id"]
        require(all(key not in notification for key in ("id", "result", "error")),
                "pending status wrongly settled the actual SDK request")
        params = notification.get("params") or {}
        observation = params.get("module_cleanup") or {}
        require(params.get("request_id") == request_id and params.get("phase") == "pending" and
                params.get("runtime_cleanup_completed") is False and observation.get("phase") == "pending",
                "actual progress did not report pending physical ownership")
        exact = [entry for entry in observation.get("outstanding", [])
                 if entry.get("module_id") == MODULE and entry.get("process_id") == reader["direct_pid"]]
        require(len(exact) == 1 and exact[0].get("child_reaped") is True and
                exact[0].get("reader_pending") is True and exact[0].get("failure") is None,
                "actual pending observation does not retain this exact reaped child and live reader")
        require(not any(value.get("id") == request_id for value in responses_at_progress),
                "actual SDK shutdown request already received a terminal response")
        require(not stop_task.done() and process.poll() is None and not process.stdin.closed and alive(reader["pid"]),
                "SDK stop abandoned its pipe/process before physical reader release")
        events.add("actual_312s_pending_owner_and_sdk_proven", elapsed_s=progress_elapsed,
                   notification=notification, shutdown_request=request)
        events.flush()
        print("native-shutdown-held-reader: actual pending observation received; releasing physical stdout writer", flush=True)
        require(not http_probe(port), "HTTP still served after shutdown admission closed")
        # Only this actual observation releases the final stdout writer. No
        # fixed sleep or private BeginClose command substitutes for its owner.
        reader_connection.sendall(canonical({"release": reader_nonce}) + b"\n")
        closed = json.loads(await asyncio.to_thread(reader_stream.readline, 4096))
        require(closed == {"kind": "writer_closed", "nonce": reader_nonce},
                "actual descendant did not close its physical stdout writer")
        writer_released = True
        events.add("actual_stdout_writer_released", writer_pid=reader["pid"])
        await asyncio.wait_for(asyncio.shield(stop_task), timeout=12.0)
        require(process.returncode == 0 and transport._process is None,
                "actual SDK did not naturally reap successful native gateway")
        natural_reap = True
        with wire_lock:
            final = [value for value in responses if value.get("id") == request_id]
        require(len(final) == 1 and all((final[0].get("result") or {}).get(key) is True
                    for key in ("shutdown", "runtime_cleanup_completed", "host_cleanup_completed")),
                f"final original-ID response lacks actual joined attestation: {final}")
        require(time.monotonic() - shutdown_started < 347.0, "fixture crossed SDK hard-stop horizon")
        await asyncio.to_thread(original.assert_reaped)
        require(await asyncio.to_thread(reader_stream.read, 1) == b"", "descendant release connection remained live")
        await asyncio.to_thread(until, lambda: not alive(reader["pid"]), 10.0,
                                "released descendant did not exit and disappear under the OS reaper")
        require(not alive(reader["direct_pid"]), "gateway-owned replacement child remains")
        await asyncio.to_thread(public_reader.join, 3.0)
        require(not public_reader.is_alive() and not private_reader.is_alive(), "actual SDK reader outlived settled owner")
        require(not transport._pending and not transport._results, "SDK retains unsettled request entries")
        with socket.socket() as probe:
            probe.settimeout(0.2)
            require(probe.connect_ex(("127.0.0.1", port)) != 0, "HTTP listener remains after natural gateway exit")
        require(not provider.connections, "fixture attempted model execution")
        with wire_lock:
            require(not any("id" in value for value in notifications),
                    "fixture unexpectedly received an application decision callback")
        report.update(status="passed", progress_elapsed_s=progress_elapsed,
                      shutdown_elapsed_s=time.monotonic() - shutdown_started,
                      actual_progress=notification, final_response=final[0],
                      gateway_exit_code=process.returncode, direct_child_reaped=True,
                      reader_physically_joined=True, descendant_absent=True,
                      sdk_shutdown_succeeded=True, private_control_readers_joined=True)
        success = True
    except BaseException as error:
        report["status"] = "prerequisite_failed" if isinstance(error, FixturePrerequisiteError) else "failed"
        report["error"] = repr(error)
        report["traceback"] = traceback.format_exc()
    finally:
        if progress_wait is not None and not progress_wait.done():
            progress_wait.cancel()
        # Failure containment never contributes to a passing cleanup claim.
        # Release a known writer before stopping/killing its real gateway.
        if reader_connection is not None and not writer_released:
            try:
                reader_connection.sendall(canonical({"release": reader_nonce}) + b"\n")
            except OSError:
                pass
        if transport is not None and process is None:
            # start() can retain a child before private startup fails. This
            # direct caller still owes composed stop on that same transport.
            process = transport._process
            private = transport.host_control
            stop_task = asyncio.create_task(asyncio.to_thread(transport.stop))
            try:
                await asyncio.shield(stop_task)
            except BaseException as error:
                report["unwind_stop_error"] = repr(error)
        if not success and process is not None and process.poll() is None:
            report["forced_cleanup"] = True
            events.add("unwind_only_gateway_kill", pid=process.pid)
            process.kill()
            await asyncio.to_thread(process.wait, 10.0)
        if stop_task is not None and not stop_task.done():
            try:
                await asyncio.wait_for(asyncio.shield(stop_task), timeout=12.0)
            except BaseException as error:
                report["unwind_stop_error"] = repr(error)
        for pending in (init_task, spawn_task, stop_task):
            if pending is not None and pending.done():
                try:
                    pending.result()
                except BaseException:
                    pass
        # Only exact fixture processes whose command still includes its nonce
        # may be signaled on failure. Never signal a reused arbitrary PID/group.
        if not success:
            owned = []
            if original is not None and original.pid is not None:
                owned.append((original.pid, original.nonce))
            if reader is not None:
                owned.extend((reader[key], reader_nonce) for key in ("pid", "direct_pid"))
            for pid, nonce in owned:
                if not alive(pid):
                    continue
                observed = subprocess.run(["/bin/ps", "-p", str(pid), "-o", "command="],
                                          capture_output=True, text=True, check=False, timeout=3.0)
                if observed.returncode == 0 and nonce in observed.stdout:
                    report["forced_cleanup"] = True
                    events.add("unwind_only_exact_fixture_kill", pid=pid)
                    try:
                        os.kill(pid, signal.SIGKILL)
                    except ProcessLookupError:
                        pass
        if private is not None:
            private.close()
        if process is not None:
            for stream in (process.stdin, process.stdout):
                if stream is not None and not stream.closed:
                    stream.close()
        if transport is not None and transport._reader_thread is not None:
            await asyncio.to_thread(transport._reader_thread.join, 3.0)
        if transport is not None and transport._stderr_file is not None:
            transport._stderr_file.close()
            transport._stderr_file = None
        if original is not None:
            original.close()
        if reader_stream is not None:
            reader_stream.close()
        if reader_connection is not None:
            reader_connection.close()
        if reader_server is not None:
            reader_server.close()
        if provider is not None:
            provider.close()
        report["natural_gateway_reap"] = natural_reap
        events.flush()
        save_json(directory / "result.json", report)
    return report


# Controlled models exercise the installed acquisition owner and one real
# public service read. Model feedback alone is not native business completion.
INSTALLED_LIMIT = 900.0
ACQUISITION_LIMIT = 900.0
ACQUISITION_ROUTE = "fixture-original-route"
ACQUISITION_CHANNEL = "fixture-authenticated-events"
ACQUISITION_OPERATION = "get_top_hackernews_stories"
ACQUISITION_REQUEST = ("Install the supplied Agno Hacker News integration for "
    "get_top_hackernews_stories for this original task. Prepare and check its "
    "configuration, install it, reconcile the actual job, and request public "
    "task access for original. Once access is ready, original must call "
    "get_top_hackernews_stories exactly once with num_stories set to 1, then "
    "report that returned story's id and title. The setup helper must not call "
    "the service or report completion on original's behalf.")
ORIGINAL_A_FINISH = "The setup worker owns acquisition. The original task is waiting for service access."
ORIGINAL_STORY_CALL = "original-public-story"
ACQUISITION_MCP_TOOLS = ("setup_prepare", "setup_help", "setup_check", "setup_install",
    "setup_job", "setup_reconcile", "setup_inspect_schema")
ACQUISITION_HELPER_TOOLS = ACQUISITION_MCP_TOOLS + ("setup_report", "toolkit_request_public_service")
RESOURCE_REFUSAL = ("Installed Connect resources are unavailable or changed; "
                    "reinstall the complete product. No setup command was started.")


def exact_bytes(path, expected=None, *, limit=4 * 1024 * 1024, minimum=1):
    """Read one bounded regular file, then validate and parse those same bytes."""
    path = Path(path)
    require(path.is_absolute() and not path.is_symlink(), "absolute regular artifact required")
    fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    with os.fdopen(fd, "rb") as source:
        info = os.fstat(source.fileno())
        require(stat.S_ISREG(info.st_mode) and minimum <= info.st_size <= limit, "artifact size/type")
        raw = source.read(limit + 1)
    require(minimum <= len(raw) <= limit, "artifact read limit")
    if expected is not None:
        require(hashlib.sha256(raw).hexdigest() == expected, "artifact digest changed")
    return raw


def unique_json(raw):
    def pairs(items):
        value = {}
        for key, item in items:
            require(key not in value, "duplicate artifact field")
            value[key] = item
        return value
    return json.loads(raw, object_pairs_hook=pairs)


def installed_public_story(content):
    """Same text/structured result oracle as cli_paired_host/public_service.rs."""
    require(isinstance(content, str), "public story result is not text")
    decoder = json.JSONDecoder()
    values, offset = [], 0
    while offset < len(content):
        while offset < len(content) and content[offset] in " \t\r\n":
            offset += 1
        if offset == len(content):
            break
        _, end = decoder.raw_decode(content, offset)
        values.append(unique_json(content[offset:end].encode()))
        require(len(values) <= 2, "unexpected extra public service result block")
        offset = end
    require(len(values) == 2 and isinstance(values[0], list)
            and isinstance(values[1], dict) and set(values[1]) == {"value"}
            and isinstance(values[1]["value"], str), "expected actual Agno text and structured result pair")
    stories = unique_json(values[1]["value"].encode())
    # JSON booleans, integers and floating values must retain their types;
    # Python value equality alone would treat true, 1 and 1.0 as equal.
    require(canonical(stories) == canonical(values[0]) and isinstance(stories, list) and len(stories) == 1,
            "public service results disagree or did not return exactly one story")
    story = stories[0]
    require(isinstance(story, dict) and type(story.get("id")) is int
            and 0 < story["id"] <= (1 << 64) - 1
            and isinstance(story.get("title"), str) and story["title"].strip(),
            "public service did not return a typed nonempty story")
    return {"id": story["id"], "title": story["title"]}


def bytes_pin(path, raw):
    return {"path": str(path), "sha256": hashlib.sha256(raw).hexdigest(), "bytes": len(raw)}


def artifact_pin(path):
    path = Path(path)
    return {"path": str(path), "sha256": digest(path), "bytes": path.stat().st_size}


def new_artifact(path, raw):
    with Path(path).open("xb") as output:
        output.write(raw)
        output.flush()
        os.fsync(output.fileno())
    Path(path).chmod(0o600)
    return {"path": str(path), "sha256": hashlib.sha256(raw).hexdigest()}


def installed_candidate(args):
    selected = args.installed_staging
    raw = exact_bytes(selected, args.installed_staging_sha256)
    staging = unique_json(raw)
    require(staging["schema_version"] == 1
            and staging["scope"] == "unqualified_product_prefix_for_verification"
            and staging["binary_kind"] == "toolkit_gateway", "not a selected gateway staging record")
    target = staging["target"]
    require(target in ("macos-arm64", "linux-x86_64"), "unsupported installed target")
    require((target == "macos-arm64" and sys.platform == "darwin" and platform.machine() == "arm64")
            or (target == "linux-x86_64" and sys.platform.startswith("linux")
                and platform.machine() in ("x86_64", "AMD64")), "installed target differs from host")
    require(staging["prefix"] == "meerkat-connect-" + target, "staging prefix mismatch")
    package = selected.parent / staging["prefix"]
    require(package.resolve(strict=True) == package and package.is_dir(), "canonical installed prefix required")
    require(args.toolkit_gateway == package / "bin/toolkit_gateway", "select the staged product gateway")
    resources = package / "libexec/meerkat-connect"
    candidate = staging["candidate"]
    require(set(candidate) == {"target", "source_manifest_sha256", "resources_sha256",
                               "binary_kind", "binary", "resource_build_record_sha256"},
            "unexpected candidate identity")
    require(candidate["target"] == target and candidate["binary_kind"] == "toolkit_gateway"
            and candidate["binary"] == {"sha256": digest(args.toolkit_gateway),
                                        "size": args.toolkit_gateway.stat().st_size}
            and candidate["resource_build_record_sha256"] == staging["resource_build_record_sha256"],
            "staged binary/candidate mismatch")
    build_path = selected.parent / "build-record.json"
    build_raw = exact_bytes(build_path, staging["build_record_sha256"])
    build = unique_json(build_raw)
    resources_path = selected.parent / "resource-build-record.json"
    resource_build_raw = exact_bytes(resources_path, staging["resource_build_record_sha256"])
    resource_build = unique_json(resource_build_raw)
    require(build["schema_version"] == 1 and build["target"] == target
            and build["binary_kind"] == "toolkit_gateway"
            and {key: build["binary"][key] for key in ("sha256", "size")} == candidate["binary"]
            and build["resources_sha256"] == candidate["resources_sha256"]
            and build["resource_build_record_sha256"] == staging["resource_build_record_sha256"],
            "selected native build differs from staged candidate")
    cargo = build["cargo"]
    require(cargo["exit_code"] == 0 and cargo["env"]["MEERKAT_CONNECT_RESOURCES_SHA256"]
            == candidate["resources_sha256"] and cargo["argv"].count("--bin") == 1
            and cargo["argv"][cargo["argv"].index("--bin") + 1] == "toolkit_gateway",
            "gateway resource-bound build record required")
    source = build["source_manifest"]
    source_raw = exact_bytes(Path(source["path"]), source["sha256"], limit=32 * 1024 * 1024)
    require(source["sha256"] == candidate["source_manifest_sha256"]
            and resource_build["target"] == target
            and resource_build["resources_sha256"] == candidate["resources_sha256"]
            and resource_build["archive_sha256"] == staging["resource_component_sha256"],
            "resource/source identity mismatch")
    resource_raw = exact_bytes(resources / "resources.json", candidate["resources_sha256"], limit=32 * 1024 * 1024)
    manifest = unique_json(resource_raw)
    require(manifest["kind"] == "meerkat_connect_resources" and manifest["target"] == target
            and manifest["prepare"] == "prepare.py", "unexpected resource manifest")
    # Native locate/prepare verifies the full resource tree in each actual case.
    # These selected records bind that runtime proof, not a Python substitute.
    evidence = {"staging": bytes_pin(selected, raw), "native_build": bytes_pin(build_path, build_raw),
                "resource_build": bytes_pin(resources_path, resource_build_raw),
                "source_manifest": bytes_pin(Path(source["path"]), source_raw),
                "resource_manifest": bytes_pin(resources / "resources.json", resource_raw)}
    return candidate, package, manifest, evidence


def require_installed_resource_refusal(finalizer, plan, corruption, settlements):
    """Check the same negative observations before and after joined settlement."""
    require(not finalizer.entered and not os.path.lexists(plan["output"])
            and not os.path.lexists(corruption["marker"]),
            "bundled preparation or finalizer entered despite corrupt resource")
    require(not any(row["method"] == "meta/init_settled"
                    and row["params"].get("outcome") == "ready" for row in settlements),
            "corrupt resource reported native Ready")
    original = corruption["before"]
    path = Path(original["path"])
    require(bytes_pin(path, exact_bytes(path, original["sha256"])) == original,
            "original selected prepare resource changed")


def child_snapshot(pid):
    """Observe descendants of this exact child without collecting unrelated commands."""
    result = subprocess.run(["/bin/ps", "-axo", "pid=,ppid=,pgid="], capture_output=True,
                            text=True, check=True, timeout=5.0)
    rows = [tuple(map(int, line.split())) for line in result.stdout.splitlines() if line.strip()]
    selected = {pid}
    found = []
    while True:
        new = [row for row in rows if row[1] in selected and row[0] not in selected]
        if not new:
            return found
        found.extend({"pid": row[0], "ppid": row[1], "pgid": row[2]} for row in new)
        selected.update(row[0] for row in new)


def descendant_process_stat(pid):
    """Bounded diagnostic only; PID liveness remains the separate kill(0) check."""
    if not sys.platform.startswith("linux"):
        return {"status": "unavailable", "reason": "unsupported_platform"}
    try:
        with Path(f"/proc/{pid}/stat").open("rb") as stream:
            raw = stream.read(4097)
        prefix, separator, tail = raw.rpartition(b")")
        fields = tail.split()
        require(len(raw) <= 4096 and separator and prefix.startswith(str(pid).encode() + b" (")
                and len(fields) >= 20 and fields[0] in (b"R", b"S", b"D", b"Z", b"T", b"t",
                    b"X", b"x", b"K", b"W", b"P", b"I"), "invalid diagnostic process stat")
        parent, group, session, start = (int(fields[index]) for index in (1, 2, 3, 19))
        require(min(parent, group, session, start) >= 0, "invalid diagnostic process number")
        return {"status": "observed", "state": fields[0].decode("ascii"), "ppid": parent,
                "pgid": group, "session_id": session, "start_ticks": start}
    except FileNotFoundError:
        return {"status": "unavailable", "reason": "not_found"}
    except PermissionError:
        return {"status": "unavailable", "reason": "permission_denied"}
    except OSError:
        return {"status": "unavailable", "reason": "read_error"}
    except (AssertionError, ValueError):
        return {"status": "unavailable", "reason": "invalid_stat"}


def require_descendants_gone(report, observed_children):
    # Retain evidence before validation, including when the first live PID fails.
    # State/start observations are not atomic with kill(0) and never excuse it.
    report["observed_children"] = list(observed_children.values())
    checks = report["descendant_shutdown_checks"] = []

    def gone(pid):
        row = {"pid": pid, "monotonic_ns": time.monotonic_ns()}
        checks.append(row)
        try:
            row["alive"] = alive(pid)
        except BaseException as error:
            row["liveness_error"] = ("permission_denied" if isinstance(error, PermissionError)
                                     else "liveness_check_failed")
            raise
        row["process_stat"] = descendant_process_stat(pid)
        return not row["alive"]

    # Sample each recorded child before a live PID makes the all-gone check fail.
    gone_observations = [gone(pid) for pid in observed_children]
    require(all(gone_observations), "observed native descendant survives shutdown")


class InstalledFixtureFinalizer:
    """One real native artifact transaction; no mocked policy or member outputs."""
    def __init__(self, gateway, directory, plan, plan_pin, source_pin, env, events, observe):
        self.gateway, self.directory, self.plan = gateway, directory, plan
        self.plan_pin, self.source_pin, self.env = plan_pin, source_pin, env
        self.events, self.observe = events, observe
        self.entered = False
        self.selection = None
        self.evidence = {}

    def check_inputs(self):
        exact_bytes(Path(self.plan_pin["path"]), self.plan_pin["sha256"])
        exact_bytes(Path(self.source_pin["path"]), self.source_pin["sha256"])

    async def command(self, name, argv):
        # These two separate native subprocesses only prepare/validate artifacts.
        # The application runtime remains on its original gateway child.
        stdout_path, stderr_path = self.directory / (name + ".stdout"), self.directory / (name + ".stderr")
        with stdout_path.open("xb") as stdout, stderr_path.open("xb") as stderr:
            child = await asyncio.create_subprocess_exec(str(self.gateway), *argv,
                stdin=asyncio.subprocess.DEVNULL, stdout=stdout, stderr=stderr,
                env=self.env, cwd=self.directory)
            self.events.add("native_artifact_command", name=name, pid=child.pid, argv=argv)
            wait = asyncio.create_task(child.wait())
            cancelled = False
            try:
                while True:
                    try:
                        code = await asyncio.wait_for(asyncio.shield(wait), 60.0)
                        break
                    except asyncio.CancelledError:
                        cancelled = True
                        continue
            except BaseException:
                if child.returncode is None:
                    child.kill()
                await wait
                self.evidence["forced_artifact_cleanup"] = True
                raise
        stdout_raw = exact_bytes(stdout_path, minimum=0)
        stderr_raw = exact_bytes(stderr_path, minimum=0)
        record = {"argv": [str(self.gateway), *argv], "pid": child.pid, "exit_code": code,
                  "stdout": bytes_pin(stdout_path, stdout_raw), "stderr": bytes_pin(stderr_path, stderr_raw)}
        self.evidence[name] = record
        if cancelled:
            raise asyncio.CancelledError
        require(code == 0, name + " native command failed; original outcome retained")
        return unique_json(stdout_raw)

    async def __call__(self, manifest_pin):
        require(not self.entered and isinstance(manifest_pin, SDK.SelectedHostArtifact),
                "finalizer must enter once with the real completed manifest")
        self.entered = True
        self.observe()
        self.check_inputs()
        root = Path(self.plan["deployment_root"])
        expected_manifest = Path(self.plan["output"]) / "manifest.json"
        require(Path(manifest_pin.path) == expected_manifest, "native completed manifest owner differs")
        manifest_raw = exact_bytes(expected_manifest, manifest_pin.sha256)
        manifest = unique_json(manifest_raw)
        require(manifest["schema"] == 1 and manifest["kind"] == "application_setup_preparation",
                "not a completed application preparation")
        retained = {expected_manifest: manifest_raw}
        files = {}
        for item in manifest["artifacts"]:
            require(set(item) == {"path", "sha256"} and Path(item["path"]).name == item["path"]
                    and item["path"] not in files, "unexpected completed artifact name")
            path = expected_manifest.parent / item["path"]
            raw = exact_bytes(path, item["sha256"])
            retained[path] = raw
            files[item["path"]] = raw
        require("application-setup.json" in files, "completed setup missing")
        setup = unique_json(files["application-setup.json"])
        original = self.plan["application"]
        require(setup["schema"] == 4 and "helper_source_surface" not in setup
                and setup["realm"] == original["realm"]
                and setup["definition_source"] == original["definition"]
                and setup["source_helper_member"] == original["helper"]["member"]
                and all(setup["source_helper_roster"].get(key) == original["roster"][0][key]
                        for key in ("identity", "profile", "runtime_mode_override"))
                and setup["helper_tools"] == original["helper_tools"]
                and setup["helper_profile"] == original["helper_profile"],
                "completed setup differs from this original preparation")
        owner = setup["native_owner"]
        require(owner["deployment"] == self.plan["owner"]["deployment"]
                and owner["components"] == self.plan["owner"]["components"]
                and owner["channels"] == self.plan["owner"]["channels"]
                and owner["native_host"]["realm"] == original["realm"]["realm_id"]
                and len(setup["native_members"]) == len(original["members"]), "native owner/application mismatch")
        for key in ("grant_administrator", "remote_operator", "ingress_domain"):
            require(owner["native_host"][key] == self.plan["owner"]["native_host"][key],
                    "native owner changed selected " + key)
        require(owner["native_host"].get("service_parent_administration")
                == self.plan["owner"]["native_host"].get("service_parent_administration"),
                "native owner changed selected administration rule")
        expected_routes = self.plan["owner"]["native_host"]["routes"]
        require(len(owner["native_host"]["routes"]) == len(expected_routes), "native route count changed")
        for actual, expected in zip(owner["native_host"]["routes"], expected_routes):
            require({key: value for key, value in actual.items() if key != "controller_parent"}
                    == {key: value for key, value in expected.items() if key != "controller_parent"}
                    and actual["controller_parent"], "native completion changed source facts or omitted its controller")
        helper = setup["helper"]
        require(helper["controller_parent"] and helper["report_capability"]["parent"],
                "native completion did not issue the selected fresh parents")
        policy_ref = setup["application_tool_policy"]
        require(policy_ref["name"] in files and hashlib.sha256(files[policy_ref["name"]]).hexdigest()
                == policy_ref["sha256"], "returned tool policy mismatch")
        self.application_tool_policy = files[policy_ref["name"]]
        for member in setup["native_members"]:
            ref = member["initial_surface"]
            require(ref["name"] in files and hashlib.sha256(files[ref["name"]]).hexdigest()
                    == ref["sha256"], "returned native surface mismatch")
        handoff_pin = setup["resources"]["handoff"]
        resource_dir = Path(setup["resources"]["artifact_directory"])
        require(resource_dir.is_absolute() and Path(handoff_pin["path"]) == resource_dir / "prepare.stdout",
                "preparer handoff owner mismatch")
        handoff_raw = exact_bytes(Path(handoff_pin["path"]), handoff_pin["sha256"])
        handoff = unique_json(handoff_raw)
        require(set(handoff) == {"mcpServers"} and set(handoff["mcpServers"]) == {"toolkit-setup"},
                "actual preparer did not return the selected setup descriptor")
        retained[Path(handoff_pin["path"])] = handoff_raw
        self.evidence.update(completed_manifest=dataclasses.asdict(manifest_pin),
            completed_setup={"path": str(expected_manifest.parent / "application-setup.json"),
                             "sha256": hashlib.sha256(files["application-setup.json"]).hexdigest()},
            preparer_handoff=handoff_pin, completed_server=setup["server"],
            controller_authority={"request": self.plan["authorities"]["controllers"][0],
                "parent": helper["controller_parent"],
                "basis": "packaged Driver joins the actual held controller report before issuing this parent"})
        self.completed_setup = setup
        # Native prepare_application_policy independently validates the real
        # setup-to-handoff descriptor equality. This projection never supplies a
        # schema, policy digest, parent or surface in place of a native output.
        artifacts = self.directory / "policy-inputs"
        artifacts.mkdir(mode=0o700)
        new_artifact(artifacts / "policy.json", exact_bytes(Path(self.source_pin["path"]), self.source_pin["sha256"]))
        for name, raw in files.items():
            new_artifact(artifacts / name, raw)
        setup_ref = {"name": "application-setup.json", "sha256": hashlib.sha256(files["application-setup.json"]).hexdigest()}
        acquisition_raw = canonical({"schema": 1, **self.plan["acquisition"], "prepared_application_setup": setup_ref})
        acquisition_pin = new_artifact(artifacts / "native-setup.json", acquisition_raw)
        policy = {"schema": 7, "deployment": owner["deployment"],
            "source_policy_sha256": self.source_pin["sha256"],
            "components": owner["components"], "channels": owner["channels"],
            "native_host": {**owner["native_host"], "members": setup["native_members"],
                "application_tool_policy": policy_ref,
                "setup": {"name": "native-setup.json", "sha256": acquisition_pin["sha256"]}}}
        policy_pin = new_artifact(self.directory / "local-policy-input.json", canonical(policy))
        template_pin = new_artifact(self.directory / "activation-template.json", canonical({"activation_id": "installed-fixture"}))
        bundle, activation = root / "policy-bundle", root / "activation.json"
        require(not bundle.exists() and not activation.exists() and not os.path.lexists(root / "current"),
                "fixture publication destination already exists")
        prepared = await self.command("prepare-policy", ["--prepare-application-policy", "--policy", policy_pin["path"],
            "--artifacts", str(artifacts), "--deployment-root", str(root), "--bundle", str(bundle),
            "--activation-template", template_pin["path"], "--activation", str(activation)])
        require(set(prepared) == {"kind", "selection"} and prepared["kind"] == "application_policy_prepared",
                "native policy preparation receipt missing")
        selected = prepared["selection"]
        require(set(selected) == {"deployment_root", "activation", "policy_manifest", "local_policy"}
                and selected["deployment_root"] == str(root), "native final selection shape/owner mismatch")
        expected = {"activation": activation, "policy_manifest": bundle / "manifest.json",
                    "local_policy": bundle / "toolkit-local-policy.json"}
        for key, path in expected.items():
            require(Path(selected[key]["path"]) == path, "native final selection path mismatch")
            retained[path] = exact_bytes(path, selected[key]["sha256"])
        selection_pin = new_artifact(self.directory / "selection.json", canonical(selected))
        validated = await self.command("validate-policy", ["--validate-application-policy", "--selection", selection_pin["path"]])
        require(validated == {"kind": "application_policy_validated", "selection": selected},
                "native final candidate validation differs")
        self.check_inputs()
        for path, raw in retained.items():
            require(exact_bytes(path) == raw, "retained finalization artifact changed")
        require(not os.path.lexists(root / "current"), "fixture current selection changed")
        next_pointer = root / "next-current"
        next_pointer.symlink_to(activation.name)
        # Retain actual selection before publication; a later error never
        # authorizes another prepare/validate transaction or replacement owner.
        self.selection = selected
        self.evidence["selection"] = selected
        self.events.add("fixture_publish_entered", selection=selected)
        os.replace(next_pointer, root / "current")
        fd = os.open(root, os.O_RDONLY | os.O_DIRECTORY)
        try:
            os.fsync(fd)
        finally:
            os.close(fd)
        require((root / "current").resolve(strict=True) == activation, "published pointer mismatch")
        self.events.add("fixture_published")
        return SDK.HostBootstrapSelection(deployment_root=str(root), **{
            key: SDK.SelectedHostArtifact(**selected[key]) for key in expected})


def installed_fixture_principal(name):
    # Current PrincipalRef::in_domain wire shape. These are explicit fixture
    # administrator inputs; native owners issue all actual grant references.
    return {"kind": "service_account", "id": name,
            "qualification": {"kind": "qualified", "trust_domain_id": "installed-gateway-fixture"}}


def installed_fixture_policy_digest(raw):
    # Current PolicyDigest::from_canonical_bytes representation.
    return "sha256:" + hashlib.sha256(raw).hexdigest()


def acquisition_helper_targets():
    # Public GrantTarget declaration at Core 4adc3d45b: serde kind tag and
    # derived Ord put MCP before bundle operations, then bundle/operation text.
    # Toolkit's optional target puts the existing catalog base (None) first.
    # These are requests to the native preparation owner, never grant refs.
    return [
        {"target": None, "issuance_id": "original-helper-catalog-ceiling"},
        {"target": {"kind": "mcp_server", "server": "toolkit-setup"},
         "issuance_id": "original-helper-mcp-ceiling"},
        {"target": {"kind": "registered_tool_bundle_operation", "bundle": "meerkat_toolkit.setup_accounts",
                    "operation": "toolkit_request_public_service"}, "issuance_id": "original-helper-public-ceiling"},
        {"target": {"kind": "registered_tool_bundle_operation", "bundle": "meerkat_toolkit.setup_results",
                    "operation": "setup_report"}, "issuance_id": "original-helper-report-ceiling"},
    ]


def reviewed_original_catalog_policy(proposal, catalog, session_id, prior_raw, *, expected_selection=None):
    """Validate the exact complete successor before either native approval path."""
    selection = {"tools": [], "catalog": "r2"} if expected_selection is None else expected_selection
    prior = unique_json(prior_raw)
    candidate_raw = proposal["candidate_canonical_json"].encode()
    candidate = unique_json(candidate_raw)
    require(set(proposal) == {"provider_id", "policy_id", "member", "selection", "catalog_session",
                             "prior", "candidate", "candidate_canonical_json", "lifetime"}
            and proposal["provider_id"] == prior["provider_id"] and proposal["policy_id"] == prior["policy_id"]
            and proposal["member"] == "original" and proposal["selection"] == selection
            and proposal["catalog_session"] == session_id
            and proposal["lifetime"] == "until_superseded" and catalog == "original_session"
            and proposal["prior"] == {"revision": prior["revision"],
                "digest": installed_fixture_policy_digest(prior_raw)}
            and proposal["candidate"] == {"revision": prior["revision"] + 1,
                "digest": installed_fixture_policy_digest(candidate_raw)},
            "catalog review changed the original session, selection or native policy version")
    expected = json.loads(json.dumps(prior))
    expected["revision"] += 1
    original = [row for row in expected["members"] if row["member_identity"] == "original"]
    require(len(original) == 1 and not any(row["tool_name"] in ("tool_catalog_search", "tool_catalog_load")
            for row in original[0]["grants"]), "bootstrap original catalog selection was not absent")
    selected_grants = [{"tool_name": row["name"], "action": "invoke", "consequence": row["consequence"]}
                       for row in selection["tools"]]
    selected_grants.extend({"tool_name": name, "action": "invoke", "consequence": selection["catalog"]}
                           for name in ("tool_catalog_search", "tool_catalog_load"))
    for grant in selected_grants:
        existing = [row for row in original[0]["grants"] if row["tool_name"] == grant["tool_name"]]
        require(not existing or existing == [grant], "review changed an existing exact tool grant")
        if not existing:
            original[0]["grants"].append(grant)
    original[0]["grants"].sort(key=lambda row: row["tool_name"])
    source = candidate["source"]
    require(set(source) == {"source_id", "source_digest"}
            and source["source_id"] == "toolkit-application-service-decision"
            and isinstance(source["source_digest"], str) and len(source["source_digest"]) == 71
            and source["source_digest"].startswith("sha256:")
            and all(c in "0123456789abcdef" for c in source["source_digest"][7:]),
            "candidate source provenance is not the native service decision")
    # Private source-tuple digest is native-owned; the field order is fixed.
    expected["source"] = {"source_id": source["source_id"], "source_digest": source["source_digest"]}
    # Core's internal policy_digest covers this ordered payload without a LF;
    # the proposal version above covers the full canonical artifact with its LF.
    payload = {key: expected[key] for key in ("schema_version", "provider_id", "policy_id",
               "revision", "source", "default_deny", "members")}
    expected["policy_digest"] = installed_fixture_policy_digest(json.dumps(
        payload, separators=(",", ":"), ensure_ascii=False).encode())
    require(candidate == expected and candidate_raw == json.dumps(expected,
                separators=(",", ":"), ensure_ascii=False).encode() + b"\n",
            "catalog approval changed permissions or native canonical bytes")


def original_tool_notice(content, operation):
    """Recognize only the selected Core's complete two-line display forms.

    Display recognition supplies no authority. Actual reviewed catalog scope,
    original-session receipts and the offered operation remain separate checks.
    """
    if not isinstance(content, str):
        return None
    match = re.fullmatch(r"Tool configuration changed at turn boundary: "
        r"(boundary_applied\(base_changed=(true|false),visible_changed=(true|false),revision=(0|[1-9][0-9]*)\))"
        r"\nTool configuration changed: \1", content)
    if match and len(match[4]) <= 20 and int(match[4]) <= (1 << 64) - 1 and "true" in match.group(2, 3):
        return "boundary"
    for kind, text, added, removed in (
            ("added", "new deferred tools available: ", 1, 0),
            ("removed", "deferred tools removed: ", 0, 1)):
        expected = ("Deferred catalog changed at turn boundary: " + text + operation
            + "\nTool configuration changed: deferred_catalog_delta(added_hidden=" + str(added)
            + ",removed_hidden=" + str(removed) + ",pending_sources=0)")
        if content == expected:
            return kind
    # Published Core 0.9 omits hidden-set counters from model text and names
    # visibility explicitly. These complete display forms confer no authority.
    for kind, text in (("added", "new deferred tools available: "),
                       ("removed", "deferred tools now available to call: ")):
        expected = ("Deferred catalog changed at turn boundary: " + text + operation
            + "\nTool configuration changed: deferred catalog updated")
        if content == expected:
            return kind
    return None


ORIGINAL_CONTROLLER_FEEDBACK = (
    "The requested model operation is unavailable under current authorization. "
    "Continue with the authorized controller without provider-hosted capabilities, "
    "or choose another permitted action."
)


def installed_approved_service_context(proposal):
    """Expected display facts from the fixture's independently checked proposal.

    This controls the synthetic provider only. Native readiness, currentness,
    permission and B's completed input remain separate acceptance predicates.
    """
    decision = proposal["decision"]
    permission = decision["permission"]
    return {"schema": "toolkit.approved-service-context.v1",
        "operation": proposal["acquisition"]["id"], "original": decision["decision"],
        "connection": permission["connection"], "resource": permission["resource"],
        "allowed_operations": list(permission["operations"]),
        "expires_at_ms": permission["lifetime"]["expires_at_ms"],
        "configuration_sha256": decision["connection_revision"],
        "attachment": "observed_usable_before_input_admission",
        "meaning": "The host completed the approved integration attachment checks for this continuation. This context accompanies the unchanged reviewed user instruction. Native authorization is still checked on each use. This observation does not guarantee future readiness, extend permission, establish filesystem isolation or complete the user's task."}


def original_catalog_history(messages, request, operation, fresh, *, allow_controller_recovery=False,
                             service_context=None):
    """Keep A/B, one exact approved context before B, and bounded native notices."""
    context_index = None
    if service_context is not None:
        context_index = fresh - 1
        require(fresh > 0 and messages[context_index] == {
            "role": "user", "content": canonical(service_context).decode()},
            "approved host context is missing, changed or not immediately before B")
    require(all(row["role"] != "user" or row.get("content") == request or i == context_index
                for i, row in enumerate(messages[:fresh])), "unexpected original input before B")
    load_positions = [i for i, row in enumerate(messages) if row["role"] == "tool"
                      and row["tool_call_id"] == "original-catalog-load"]
    require(len(load_positions) <= 1, "duplicate original catalog load result")
    loaded_at = load_positions[0] if load_positions else len(messages)
    boundaries, deferred, recovery = set(), set(), None
    first_tool = next((i for i, row in enumerate(messages) if i > fresh
                       and (row.get("tool_calls") or row["role"] == "tool")), len(messages))
    for i, row in enumerate(messages[fresh + 1:], fresh + 1):
        if row["role"] != "user":
            continue
        kind = original_tool_notice(row.get("content"), operation)
        after_load = i > loaded_at
        if row.get("content") == ORIGINAL_CONTROLLER_FEEDBACK:
            # This schedules a new controller choice, not a replay of a tool.
            # Text cannot establish the reason: final acceptance additionally
            # requires this exact B's persisted ReprepareRequired audit.
            before = messages[fresh + 1:i]
            calls = [call.get("id") if isinstance(call, dict) else None
                for row in before for call in row.get("tool_calls", [])]
            results = [row.get("tool_call_id") for row in before if row.get("role") == "tool"]
            completed_boundary = (bool(calls) and all(isinstance(value, str) and value for value in calls)
                and len(calls) == len(set(calls))
                and calls == results)
            require(allow_controller_recovery and recovery is None
                    and (i < first_tool or completed_boundary),
                    "unexpected original controller feedback stage or count")
            recovery = i
        elif kind == "boundary":
            require(after_load not in boundaries, "repeated original tool boundary notice")
            boundaries.add(after_load)
        elif kind == "added":
            require(not after_load and kind not in deferred, "unexpected deferred addition")
            deferred.add(kind)
        elif kind == "removed":
            require(after_load and "added" in deferred and kind not in deferred,
                    "deferred removal lacks the original load")
            deferred.add(kind)
        else:
            raise AssertionError("another user input followed the selected B request")
    return recovery


def original_reprepare_projection(state, session_id, input_id, run_id):
    """Read one completed input's audit; historical evidence grants nothing."""
    require(state["input_id"] == input_id and state["last_run_id"] == run_id,
            "controller recovery audit belongs to another B input or run")
    records = state["authorization_audit"]
    require(records and all(row["observation"]["run_id"] == run_id
            and [item["input_id"] for item in row["contributors"]] == [input_id]
            for row in records), "controller recovery audit has foreign contributors or run")
    observations = [row["observation"] for row in records]
    refusal_indices = [i for i, row in enumerate(observations)
                       if row["observation"]["kind"] == "refused"]
    require(refusal_indices and not any(row["observation"]["kind"] == "authorization_unavailable"
            for row in observations), "controller recovery lacks a refusal or includes unavailable authorization")
    refused_at, prepared_at = refusal_indices[0], refusal_indices[-1] + 1
    require(refusal_indices == list(range(refused_at, prepared_at))
            and prepared_at < len(observations), "controller recovery lacks one contiguous refusal boundary")
    refused, prepared = observations[refused_at], observations[prepared_at]
    target = refused["observation"]["target"]
    require(target["kind"] == "model" and target["model"] == "fixture-original-model"
            and target["provider"] == "self_hosted" and target["usage"] == "controller_inference"
            and target["hosted_capabilities"] == [], "controller recovery target is not the selected controller")
    require(all(row["operation_id"] == refused["operation_id"]
                and row["observation"] == refused["observation"]
                for row in observations[refused_at:prepared_at])
            and refused["observation"]["reason"] == "reprepare_required"
            and all(row["operation_id"] != refused["operation_id"]
                    for row in observations[:refused_at] + observations[prepared_at:]),
            "controller recovery includes a denial, malformed facts or reused refused operation")
    require(prepared["operation_id"] != refused["operation_id"]
            and prepared["observation"]["kind"] == "prepared"
            and prepared["observation"]["target"] == target,
            "controller recovery did not prepare a fresh operation on the same exact target")
    entered = [row["observation"] for row in observations[prepared_at + 1:]
               if row["operation_id"] == prepared["operation_id"]]
    require(entered == [{"kind": "entry"}, {"kind": "outcome",
                "outcome": {"kind": "http_response", "status": 200}}],
            "controller recovery lacks its successful model entry and response")
    policy = prepared["observation"]["policy"]
    return {"session_id": session_id, "input_id": input_id, "run_id": run_id,
        "refused_operation_id": refused["operation_id"], "refusal_count": prepared_at - refused_at,
        "reason": "reprepare_required", "model": target["model"], "provider": target["provider"],
        "usage": target["usage"], "hosted_capabilities": [],
        "recovery_operation_id": prepared["operation_id"], "recovered_http_status": 200,
        "policy_publication_sequence": policy["publication_sequence"],
        "policy_observed_at_ms": policy["observed_at_ms"]}


def retained_original_reprepare(directory, session_id, input_id, run_id):
    path = directory / "state/runtime.sqlite"
    with sqlite3.connect(path.resolve(strict=True).as_uri() + "?mode=ro", uri=True) as connection:
        rows = connection.execute("SELECT state_json FROM runtime_input_states "
            "WHERE runtime_id = ? AND input_id = ?", ("rt:session:" + session_id, input_id)).fetchall()
    require(len(rows) == 1, "exact native B authorization audit is unavailable")
    raw = rows[0][0]
    raw = raw.encode() if isinstance(raw, str) else raw
    projection = original_reprepare_projection(unique_json(raw), session_id, input_id, run_id)
    projection["row_sha256"] = hashlib.sha256(raw).hexdigest()
    return new_artifact(directory / "original-b-controller-recovery.json", canonical(projection))


def exact_catalog_load(value, expected):
    require(value["catalog_exact"] is True, "native load did not use exact catalog")
    accepted, noops = set(value["accepted_names"]), set(value["noop_names"])
    require(len(accepted) == len(value["accepted_names"]) and len(noops) == len(value["noop_names"])
            and not accepted.intersection(noops) and accepted.union(noops) == set(expected),
            "native load did not accept exactly the selected names")
    resolutions = value["resolutions"]
    require(len(resolutions) == len(expected), "native load resolution count differs")
    for name in expected:
        matching = [row for row in resolutions if row["name"] == name]
        require(len(matching) == 1 and matching[0]["accepted"] is True
                and matching[0].get("rejected_reason") is None
                and matching[0].get("accepted_noop", False) == (name in noops),
                "native load refused or contradicted the selected name")


def pending_setup_search(value):
    return (isinstance(value, dict)
            and set(value) == {"catalog_exact", "total_matches", "results", "pending_sources", "empty_result_status"}
            and value["catalog_exact"] is True
            and type(value["total_matches"]) is int and value["total_matches"] == 0
            and value["results"] == [] and value["pending_sources"] == ["toolkit-setup"]
            and value["empty_result_status"] == "pending_source")


def helper_search_not_dispatched(value):
    """Core 0.9.4's closed model-visible proof, never inferred from display text."""
    data = value.get("data")
    return (value.get("error") == "operation_refused" and isinstance(data, dict)
            and set(data) == {"kind", "tool_invoked"}
            and data["kind"] == "reprepare_required" and data["tool_invoked"] is False)


class InstalledAcquisitionProvider:
    """Controlled model responses; every tool result comes from the real child."""
    request_text = ACQUISITION_REQUEST
    operation = ACQUISITION_OPERATION
    profile = "hackernews"
    selection_values = {"timeout": 7}
    access_tool = "toolkit_request_public_service"
    access_call_id = "public-task-access"
    original_catalog = False
    controller_reprepare = False
    allow_fresh_helper_search = False
    helper_search_refusal = None

    def checked_original_catalog_history(self, messages, fresh):
        return original_catalog_history(messages, self.request_text, self.operation, fresh,
            allow_controller_recovery=self.controller_reprepare,
            service_context=getattr(self, "expected_service_context", None))

    def __init__(self, directory, events):
        from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
        self.directory, self.events = directory, events
        self.lock = threading.RLock()
        self.enabled = False
        self.requests, self.errors, self.results = [], [], {}
        self.issued = set()
        self.finished = threading.Event()
        self.business_response_prepared = threading.Event()
        self.business_observation = None
        self.expected_service_context = None
        self.deadline = time.monotonic() + INSTALLED_LIMIT + ACQUISITION_LIMIT
        owner = self

        class Handler(BaseHTTPRequestHandler):
            def setup(self):
                super().setup()
                self.connection.settimeout(10.0)

            def log_message(self, *_):
                pass

            def do_POST(self):
                try:
                    require(self.path == "/v1/chat/completions", "unexpected model endpoint")
                    size = int(self.headers.get("Content-Length", "0"))
                    require(0 < size <= 8 * 1024 * 1024, "model request exceeds bound")
                    body = unique_json(self.rfile.read(size))
                    with owner.lock:
                        require(owner.enabled, "model work began before native Ready")
                        require(len(owner.requests) < 512 and time.monotonic() < owner.deadline,
                                "controlled model schedule exceeded bound")
                        owner.check_model_request(body)
                        owner.requests.append(body)
                        reply = owner.response(body)
                    raw = ("data: " + json.dumps({"choices": [reply]})
                           + "\n\ndata: [DONE]\n\n").encode()
                    self.send_response(200)
                    self.send_header("Content-Type", "text/event-stream")
                    self.send_header("Content-Length", str(len(raw)))
                    self.end_headers()
                    self.wfile.write(raw)
                except BaseException as error:
                    with owner.lock:
                        owner.errors.append(owner.error_observation(error))
                    self.send_error(500, "controlled acquisition schedule refused")

        self.server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.server.daemon_threads = False
        self.port = self.server.server_address[1]
        self.thread = threading.Thread(target=self.server.serve_forever, name="installed-model", daemon=False)
        self.thread.start()

    def check_model_request(self, body):
        pass

    def error_observation(self, error):
        return repr(error)

    def checked_configuration(self, checked):
        require(checked["scope"] == "configuration_only" and checked["credential_environment_references"] == [],
                "actual package check is not the selected public configuration")

    def check_authentication_metadata(self, metadata):
        require(metadata["authentication"] == {"kind": "none"}, "publisher did not declare public authentication")

    def access_arguments(self, job_id, raw):
        return {"job_id": job_id, "operations": raw, "agent": "original", "expires_in_ms": None}

    def check_access_result(self, result):
        require(result == self.public_reply(), "actual public request did not retain fresh authority/source")

    def public_reply(self):
        return {"state": "authorized", "grant_issued": True,
                "fresh_instruction_retained": True, "ready": False}

    def helper_finish(self):
        return "The native public request retained its task grant and fresh instruction. Service readiness and original-task completion remain unobserved."

    def response(self, body):
        lane = body["model"]
        require(lane in ("fixture-original-model", "fixture-helper-model"), "unknown model owner")
        names = {row["function"]["name"] for row in body["tools"]}
        results = {}
        refused_searches = []
        for message in body["messages"]:
            if message["role"] == "tool":
                identifier = message["tool_call_id"]
                if identifier == ORIGINAL_STORY_CALL:
                    require(lane == "fixture-original-model", "helper received the original's public service result")
                    value = {"content": message["content"], "story": installed_public_story(message["content"])}
                else:
                    value = unique_json(message["content"].encode())
                    if isinstance(value, dict) and "error" in value:
                        if not (self.allow_fresh_helper_search and lane == "fixture-helper-model"
                                and identifier in self.issued
                                and identifier.startswith("helper-catalog-search-")
                                and helper_search_not_dispatched(value)):
                            raise AssertionError("actual tool result " + identifier + " refused: " + str(value["error"]))
                        refused_searches.append(identifier)
                require(identifier not in results, "duplicate tool result in actual model history")
                results[identifier] = value
                if identifier in self.results:
                    require(self.results[identifier] == value, "retained tool result changed")
                self.results[identifier] = value
                if identifier.startswith("helper-catalog-load-"):
                    # Existing packaged_setup.rs checks the actual native load
                    # receipt separately from the next visible model surface.
                    require(value["catalog_exact"] is True, "native load did not use exact catalog")
                    accepted, noops = set(value["accepted_names"]), set(value["noop_names"])
                    require(len(accepted) == len(value["accepted_names"]) and len(noops) == len(value["noop_names"])
                            and not accepted.intersection(noops)
                            and accepted.union(noops) == set(ACQUISITION_MCP_TOOLS),
                            "native load did not accept exactly the observed setup names")
                    resolutions = value["resolutions"]
                    require(len(resolutions) == len(ACQUISITION_MCP_TOOLS), "native load resolution count differs")
                    for name in ACQUISITION_MCP_TOOLS:
                        matching = [row for row in resolutions if row["name"] == name]
                        require(len(matching) == 1 and matching[0]["accepted"] is True
                                and matching[0].get("rejected_reason") is None
                                and matching[0].get("accepted_noop", False) == (name in noops),
                                "native load refused or contradicted " + name)
                if identifier in ("original-catalog-search", "original-catalog-load"):
                    require(self.original_catalog and lane == "fixture-original-model"
                            and identifier in self.issued, "unissued or foreign original catalog result")
                    if identifier == "original-catalog-load":
                        exact_catalog_load(value, [self.operation])

        def call(identifier, name, args):
            require(name in names, "actual native surface does not expose " + name)
            require(identifier not in self.issued, "controlled fixture does not replay tool effects")
            self.issued.add(identifier)
            self.events.add("controlled_model_tool", model=lane, call_id=identifier, tool=name, args=args)
            return {"delta": {"tool_calls": [{"index": 0, "id": identifier, "type": "function",
                "function": {"name": name, "arguments": json.dumps(args)}}]}, "finish_reason": "tool_calls"}

        def finished(text):
            return {"delta": {"content": text}, "finish_reason": "stop"}

        if lane == "fixture-original-model":
            messages = body["messages"]
            originals = [index for index, message in enumerate(messages)
                         if message["role"] == "user" and message.get("content") == self.request_text]
            require(len(originals) in (1, 2), "expected exact original A input and at most one fresh B input")
            if "original-request" not in results:
                require(len(originals) == 1, "fresh B input cannot start another acquisition")
                return call("original-request", "find_and_install", {"request": self.request_text,
                    "user_context": [], "caller_observations": []})
            require(isinstance(results["original-request"].get("operation"), str)
                    and results["original-request"].get("created") is True
                    and results["original-request"].get("worker") == {"state": "input_admitted",
                        "deduplicated": False, "source_settlement": "fresh_acceptance_promoted"},
                    "original did not start the actual setup worker")
            if len(originals) == 1:
                require(ORIGINAL_STORY_CALL not in self.issued, "A cannot invoke the service")
                return finished(ORIGINAL_A_FINISH)
            # Observe the actual second user input after A's final response in
            # this same model history. This is not the native B admission or
            # terminal oracle; that separate source/input join remains required.
            first, fresh = originals
            require(any(message["role"] == "assistant" and message.get("content") == ORIGINAL_A_FINISH
                        for message in messages[first + 1:fresh]), "fresh B input lacks A's actual final history")
            if self.original_catalog:
                recovery_index = self.checked_original_catalog_history(messages, fresh)
                search, loaded = "original-catalog-search", "original-catalog-load"
                if search not in results:
                    return call(search, "tool_catalog_search", {"query": self.operation, "limit": 50})
                search_index = next(i for i, row in enumerate(messages)
                    if row["role"] == "tool" and row["tool_call_id"] == search)
                found = results[search]
                matches = [row for row in found["results"] if row["name"] == self.operation]
                require(search_index > fresh and found["catalog_exact"] is True
                        and not found.get("pending_sources", []) and len(matches) == 1
                        and matches[0]["visibility_status"] in ("loaded", "deferred"),
                        "original catalog did not observe the exact available operation")
                if loaded not in results:
                    return call(loaded, "tool_catalog_load", {"names": [matches[0]["name"]]})
                load_index = next(i for i, row in enumerate(messages)
                    if row["role"] == "tool" and row["tool_call_id"] == loaded)
                require(load_index > search_index, "original catalog load preceded its search")
            else:
                require(all(message["role"] != "user" for message in messages[fresh + 1:]),
                        "another user input followed the selected B request")
            if ORIGINAL_STORY_CALL not in results:
                offered = [row for row in body["tools"] if row["function"]["name"] == self.operation]
                if len(offered) != 1:
                    self.events.add("original_public_tool_unavailable", offered_names=sorted(names),
                        original_catalog_authority="reviewed_original_session" if self.original_catalog else "not_selected",
                        helper_catalog_scope_reused=False)
                require(len(offered) == 1, "original's exact public tool is not exposed after selected discovery")
                # Native recovery may admit B before the helper's next model
                # request observes its own RequestTool result. Do not impose an
                # ordering between those independent observation consumers.
                return call(ORIGINAL_STORY_CALL, offered[0]["function"]["name"], {"num_stories": 1})
            require(ORIGINAL_STORY_CALL in self.issued, "original received an unissued service result")
            result_index = next(index for index, message in enumerate(messages)
                if message["role"] == "tool" and message["tool_call_id"] == ORIGINAL_STORY_CALL)
            require(result_index > (load_index if self.original_catalog else fresh),
                    "public result belongs to a different original input or precedes discovery")
            story = results[ORIGINAL_STORY_CALL]["story"]
            answer = "Hacker News story " + str(story["id"]) + ": " + story["title"]
            require(self.business_observation is None, "original final response must not be replayed")
            self.business_observation = {"story": story, "answer": answer,
                "actual_tool_result": results[ORIGINAL_STORY_CALL]["content"],
                "a_input_history_index": first, "b_input_history_index": fresh,
                "matching_result_history_index": result_index,
                "model_request_index": len(self.requests) - 1,
                "model_request_sha256": hashlib.sha256(canonical(body)).hexdigest(),
                "native_b_admission": "not_observed", "native_b_terminal": "not_observed"}
            if self.original_catalog and recovery_index is not None:
                self.business_observation["controller_recovery_history_index"] = recovery_index
            if self.original_catalog:
                self.business_observation["original_catalog"] = {
                    "search_call_id": search, "load_call_id": loaded,
                    "search_result_history_index": search_index, "load_result_history_index": load_index,
                    "operation": self.operation, "helper_catalog_scope_reused": False,
                    "search_result": results[search], "load_result": results[loaded]}
            self.events.add("original_public_answer_prepared", **self.business_observation)
            self.business_response_prepared.set()
            return finished(answer)

        require(self.request_text in canonical(body["messages"]).decode(), "helper did not receive original setup request")
        if self.allow_fresh_helper_search:
            prior_refusal = self.helper_search_refusal
            require(len(refused_searches) <= 1
                    and (prior_refusal is None or refused_searches == [prior_refusal]),
                    "fresh helper search exceeded its one-decision bound or lost its refusal")
            if refused_searches:
                refused = refused_searches[0]
                fresh = refused + "-fresh"
                if fresh not in results:
                    # This controlled model proposes a new tool call. Core owns
                    # fresh admission; no same-operation replay or audit oracle.
                    reply = call(fresh, "tool_catalog_search", {"query": "setup", "limit": 50})
                    self.helper_search_refusal = refused
                    self.events.add("controlled_model_fresh_helper_search",
                        refused_call_id=refused, fresh_call_id=fresh,
                        proof=results[refused]["data"], bound=1)
                    return reply
                require(fresh in self.issued
                        and list(results).index(fresh) > list(results).index(refused),
                        "fresh helper search result is unissued or precedes its refusal")
        selected = {"family": "agno", "profile": self.profile,
                    "operations": [self.operation], "values": self.selection_values}
        phase = len(self.requests)
        searches = [value for identifier, value in results.items()
                    if identifier.startswith("helper-catalog-search-") and identifier not in refused_searches]
        loads = [value for identifier, value in results.items()
                 if identifier.startswith("helper-catalog-load-")]
        needs_fresh_load = False
        if refused_searches:
            following = list(results)[list(results).index(fresh) + 1:]
            needs_fresh_load = not any(identifier.startswith("helper-catalog-load-")
                                       for identifier in following)
        if any(name not in names for name in ACQUISITION_MCP_TOOLS) or needs_fresh_load:
            require(len(loads) < 10, "native catalog did not expose selected setup tools")
            require(all(pending_setup_search(value) for value in searches[:-1]),
                    "another Search followed a result other than the exact pending setup source")
            found = searches[-1] if searches else None
            observed_names = {}
            if found is not None:
                require(found["catalog_exact"] is True, "native catalog inventory is not exact")
                for row in found["results"]:
                    if row["name"] in ACQUISITION_MCP_TOOLS and row["visibility_status"] in ("loaded", "deferred"):
                        require(row["name"] not in observed_names, "native catalog repeated selected tool")
                        observed_names[row["name"]] = row["name"]
            if found is not None and len(observed_names) == len(ACQUISITION_MCP_TOOLS) and not found.get("pending_sources", []):
                return call("helper-catalog-load-" + str(phase), "tool_catalog_load",
                            {"names": [observed_names[name] for name in ACQUISITION_MCP_TOOLS]})
            if searches:
                require(pending_setup_search(found),
                        "native catalog did not resolve selected setup tools or report the exact pending setup source")
                threading.Event().wait(0.25)
            return call("helper-catalog-search-" + str(phase), "tool_catalog_search", {"query": "setup", "limit": 50})

        def pending_job(result, label):
            job_id = result["job"]["id"]
            require(isinstance(job_id, str) and job_id, "actual job id missing")
            current = result
            for value in results.values():
                if isinstance(value, dict) and value.get("job", {}).get("id") == job_id:
                    current = value
            status = current["job"]["status"]
            require(status in ("running", "succeeded"), "actual " + label + " job failed: " + str(current))
            if status == "running":
                threading.Event().wait(2.0)
                return job_id, call(label + "-poll-" + str(phase), "setup_job", {"job_id": job_id})
            return job_id, None

        if "schema-evidence" not in results:
            require("prepare-family" not in self.issued,
                    "schema evidence call must precede package preparation")
            return call("schema-evidence", "setup_inspect_schema", {"schema_json": "{}"})
        schema = results["schema-evidence"]
        expected_schema = {"schema_version": 1, "scope": "supplied_schema_evidence_only",
            "schema_digest": "44136fa355b3678a1146ad16f7e8649e94fb4fc21fe77e8310c060f61caaff8a",
            "bytes": 2, "hash_algorithm": "sha256", "byte_convention": "exact_supplied_utf8",
            "runtime_observation": "not_established", "schema_validation": "not_performed"}
        require("schema-evidence" in self.issued and isinstance(schema, dict)
                and set(schema) == set(expected_schema) | {"notice"}
                and all(schema[key] == value for key, value in expected_schema.items())
                and isinstance(schema["notice"], str) and bool(schema["notice"]),
                "installed helper did not return exact supplied-schema evidence and limits")

        if "prepare-family" not in results:
            require(searches and loads,
                    "fixture did not observe actual deferred catalog search and load")
            return call("prepare-family", "setup_prepare", {"family": "agno"})
        _, pending = pending_job(results["prepare-family"], "prepare")
        if pending is not None:
            return pending
        if "profile-help" not in results:
            return call("profile-help", "setup_help", {"family": "agno", "profile": self.profile})
        require(any(row["id"] == self.operation for row in results["profile-help"]["operations"]),
                "actual publisher did not expose requested operation")
        if "checked-install" not in results:
            return call("checked-install", "setup_check", selected)
        checked = results["checked-install"]
        self.checked_configuration(checked)
        if "install-once" not in results:
            return call("install-once", "setup_install", {**selected,
                "validation_id": checked["validation_id"], "configuration_sha256": checked["configuration_sha256"]})
        job_id, pending = pending_job(results["install-once"], "install")
        if pending is not None:
            return pending
        if "installed-job" not in results:
            return call("installed-job", "setup_job", {"job_id": job_id})
        if "reconciled-install" not in results:
            return call("reconciled-install", "setup_reconcile", {"job_id": job_id})
        observation = results["reconciled-install"]["observation"]
        require(observation["job_id"] == job_id and observation["status"] == "matched",
                "actual installed job did not reconcile")
        if "current-operation-names" not in results:
            return call("current-operation-names", "setup_help", {"family": "agno", "profile": self.profile})
        metadata = results["current-operation-names"]
        self.check_authentication_metadata(metadata)
        raw = [row["tool"] for row in metadata["operations"] if row["id"] == self.operation]
        require(raw == [self.operation], "actual raw operation identity differs")
        if self.access_call_id not in results:
            return call(self.access_call_id, self.access_tool, self.access_arguments(job_id, raw))
        self.check_access_result(results[self.access_call_id])
        self.finished.set()
        return finished(self.helper_finish())

    def close(self, *, drain):
        self.server.shutdown()
        self.server.server_close()
        self.thread.join(5.0)
        require(not self.thread.is_alive(), "controlled provider did not join")
        save_json(self.directory / "model-requests.json", self.requests)


class InstalledPublicCatalogProvider(InstalledAcquisitionProvider):
    """The public package case explicitly selects Original catalog review."""
    original_catalog = True
    controller_reprepare = True

    def access_arguments(self, job_id, raw):
        return {**super().access_arguments(job_id, raw),
                "tool_policy": {"tools": [], "catalog": "r2"}}


def installed_plan(directory, roster, *, acquisition=False, console=None, catalog_policy=False):
    """Explicit fixture application inputs; native alone issues parent refs."""
    require(not catalog_policy or (acquisition and console is None),
            "standing catalog selection belongs only to the public package case")
    root = directory / "deployment"
    root.mkdir(mode=0o700)
    # The deployment owner supplies parents for native stores and the entry fence.
    (root / "shared").mkdir(mode=0o700)
    (root / "shared/data").mkdir(mode=0o700)
    (root / "shared/control").mkdir(mode=0o700)
    principal = installed_fixture_principal("helper")
    administrator = installed_fixture_principal("application")
    operator = installed_fixture_principal("operator")
    member = {"mob_id": "fixture", "role": "research", "member": "helper"}
    selected_member = {"member": member, "logical_owner": principal, "context_generation": 1,
                       "audience": {"audience": "principal", "principal": principal}}
    definition = {"format": "toml", "text": '''[mob]
id = "fixture"
[profiles.research]
model = "fixture-model"
provider = "self_hosted"
self_hosted_server_id = "local"
runtime_mode = "turn_driven"
[profiles.research.tools]
comms = true
'''}
    owner = {"deployment": "installed-fixture", "components": [], "channels": [],
        "native_host": {"realm": "mob.fixture", "grant_administrator": administrator,
            "remote_operator": operator,
            "ingress_domain": {"authority": administrator, "namespace": "installed-fixture"},
            "members": [selected_member], "routes": []}}
    if acquisition:
        definition["text"] += '''
[profiles.original]
external_addressable = true
model = "fixture-original-model"
provider = "self_hosted"
self_hosted_server_id = "local"
runtime_mode = "turn_driven"
[profiles.original.tools]
comms = true
rust_bundles = ["meerkat_toolkit.setup"]
'''
        definition["text"] = definition["text"].replace('model = "fixture-model"', 'model = "fixture-helper-model"')
        # Account requests are composed on the exact helper's MCP adapter,
        # not selected from the mob-wide Rust bundle registry.
        original_member = {"mob_id": "fixture", "role": "original", "member": "original"}
        original_owner = installed_fixture_principal("original-agent")
        source_owner = installed_fixture_principal("event-source")
        requester = installed_fixture_principal("fixture-requester")
        owner["components"] = ["fixture-event-source"]
        owner["channels"] = [{"component": "fixture-event-source", "channel_id": ACQUISITION_CHANNEL,
            "service_subject": "fixture-event-source", "provider_account": "fixture-local-account",
            "sender_id": "fixture-requester", "requester": requester, "ingress_actor": source_owner,
            "target_identity": "original", "member": original_member, "realm": "mob.fixture",
            "required_surfaces": ["channel:" + ACQUISITION_CHANNEL]}]
        owner["native_host"]["members"].append({"member": original_member, "logical_owner": original_owner,
            "context_generation": 1, "audience": {"audience": "principal", "principal": requester}})
        owner["native_host"]["routes"] = [{"id": ACQUISITION_ROUTE,
            "origin": {"kind": "provider_event", "channel_id": ACQUISITION_CHANNEL,
                "sender_id": "fixture-requester", "target_identity": "original"},
            "application_parent": None, "controller_parent": None, "logical_executor": original_owner,
            "represented_subject": None, "source_authority": source_owner,
            "authentication_domain": {"authority": source_owner, "namespace": "fixture-authentication"},
            "event_domain": {"authority": source_owner, "namespace": "fixture-events"}}]
        owner["native_host"]["service_parent_administration"] = {
            "kind": "application_policy_tasks", "routes": [ACQUISITION_ROUTE]}
    source = {"purpose": ("controlled installed SDK public acquisition" if acquisition else
                          "installed gateway resource/startup fixture, no model turn"),
              "definition": definition, "roster": roster,
              "controller_route_basis": "request the actual held member report; no preselected key or model turn"}
    if acquisition:
        source["finite_service_rule"] = {"route": ACQUISITION_ROUTE, "request": ACQUISITION_REQUEST,
            "family": "agno", "profile": "hackernews", "operation": ACQUISITION_OPERATION,
            "authentication": {"kind": "none"}, "maximum_active_reservations": 1,
            "original_tool_policy": {"tools": [], "catalog": "r2"},
            "public_read": {"operation": ACQUISITION_OPERATION, "arguments": {"num_stories": 1},
                "fixture_maximum_calls": 1, "answer_fields": ["id", "title"]},
            "scope": "one original task, its retained fresh source and one public Hacker News read"}
    if console is not None:
        require(acquisition, "Console acceptance requires the complete acquisition plan")
        console.configure_owner(owner, source)
    source_pin = new_artifact(directory / "policy.json", canonical(source))
    application = {"schema": 4,
        "realm": {"realm_id": "mob.fixture", "context_root": str(directory / "contexts"),
                  "state_root": str(directory / "state"), "user_config_root": str(directory / "user")},
        "definition": definition, "roster": roster, "members": owner["native_host"]["members"],
        "helper": {"member": member, "ingress_actor": installed_fixture_principal("helper-ingress"),
            "logical_executor": principal, "represented_subject": None, "controller_parent": [],
            "operation_basis": {"kind": "targeted_grant_lineages", "base": None, "targets": []}},
        "helper_tools": "checked_installation" if acquisition else "preparation", "helper_profile": "dedicated",
        "tool_policy": {"provider_id": "installed-fixture", "policy_id": "helper-tools", "revision": 1,
            "source": {"source_id": "installed-fixture-input", "source_digest": installed_fixture_policy_digest(canonical(source))},
            "members": [{"member_identity": "helper", "grants": [
                {"tool_name": "setup_report", "action": "invoke", "consequence": "r2"}]}]}}
    plan = {"schema": 1, "deployment_root": str(root), "owner": owner,
        "startup": {"kind": "initialize", "namespace": "installed-fixture", "generation": 1},
        "authorities": {"controllers": [{"target": {"kind": "helper"},
            "issuance_id": "helper-controller-parent"}],
            "agent_setup": [], "helper_report": {"member": member, "issuance_id": "helper-report-parent"}},
        "application": application,
        "acquisition": {"console_setup": None, "backend_generation": "installed-fixture-workgraph",
            "proposal_ceiling": {"actions": [], "services": [], "frontends": [], "output_audiences": ["fixture-output"]},
            "output_audience": ["fixture-output"],
            "limits": {"setup_timeout_ms": 60000, "max_workers": 1, "max_effect_attempts": 1,
                       "admission_timeout_ms": 10000}},
        "output": str(directory / "application-preparation"), "hold_timeout_ms": int(INSTALLED_LIMIT * 1000)}
    if acquisition:
        application["tool_policy"]["members"][0]["grants"] = [
            {"tool_name": name, "action": "invoke", "consequence": "r2"}
            for name in ACQUISITION_HELPER_TOOLS]
        # Match the fixture's existing r2 observability classification for setup
        # controls, including metadata reads. Native requires these exact Invoke
        # grants and the separate catalog BASE; the class confers no authority.
        application["tool_policy"]["members"][0]["grants"].extend([
            {"tool_name": "tool_catalog_search", "action": "invoke", "consequence": "r2"},
            {"tool_name": "tool_catalog_load", "action": "invoke", "consequence": "r2"}])
        application["tool_policy"]["members"].append({"member_identity": "original", "grants": [
            {"tool_name": "find_and_install", "action": "invoke", "consequence": "r2"},
            # Trusted fixed-package expectation from cli_paired_host/public_service.rs;
            # observed model/tool output cannot add or change this exact allow.
            {"tool_name": ACQUISITION_OPERATION, "action": "invoke", "consequence": "r2"}]})
        plan["authorities"]["controllers"].append({"target": {"kind": "original_route", "route": ACQUISITION_ROUTE},
            "issuance_id": "original-controller-parent"})
        plan["authorities"]["helper_mcp"] = {"issuance_id": "helper-setup-mcp-parent"}
        plan["authorities"]["helper_catalog"] = {"issuance_id": "helper-setup-catalog-parent"}
        plan["authorities"]["helper_report"]["public_issuance_id"] = "helper-public-request-parent"
        plan["authorities"]["agent_setup"] = [{"member": original_member, "route": ACQUISITION_ROUTE,
            "issuance_id": "original-setup-request-parent", "helper_targets": acquisition_helper_targets()}]
        plan["acquisition"]["standing_policy"] = {"installation": "checked_software", "access": "provision_task_parent"}
        if catalog_policy:
            # Explicitly select the existing full-candidate owner. Its separate
            # SDK reservation still checks and approves the exact successor.
            plan["acquisition"]["standing_policy"]["tool_policy"] = "until_superseded"
        plan["acquisition"]["proposal_ceiling"]["actions"] = [
            {"action": action, "resource": "original"} for action in ("find_and_install", "delegate")]
        plan["acquisition"]["limits"]["setup_timeout_ms"] = int(ACQUISITION_LIMIT * 1000)
        plan["acquisition"]["limits"]["max_effect_attempts"] = 3
    if console is not None:
        console.configure_plan(plan)
    plan_pin = new_artifact(directory / "application-plan.json", canonical(plan))
    return plan, plan_pin, source_pin


class InstalledFixtureRule:
    """Actual finite fixture policy owner, with one held exclusion until Joined.

    The lock excludes another decision or policy replacement while the SDK owns
    the reservation. Native code still derives and checks all grant restrictions.
    This owner authenticates only its local fixture event, never a human login.
    """
    def __init__(self, finalizer, provider, original, admission, process, deadline, events):
        self.finalizer, self.provider, self.original = finalizer, provider, original
        self.admission, self.process, self.deadline, self.events = admission, process, deadline, events
        self.latest_expiry_ms = int((time.time() + ACQUISITION_LIMIT + 60.0) * 1000)
        self.exclusion = threading.Lock()
        self.reservation = None
        self.transferred = False
        self.releases = 0

    def check(self, proposal):
        f = self.finalizer
        f.check_inputs()
        source = unique_json(exact_bytes(Path(f.source_pin["path"]), f.source_pin["sha256"]))
        rule = source["finite_service_rule"]
        require(rule["request"] == ACQUISITION_REQUEST and rule["route"] == ACQUISITION_ROUTE
                and rule["family"] == "agno" and rule["profile"] == "hackernews"
                and rule["operation"] == ACQUISITION_OPERATION and rule["authentication"] == {"kind": "none"}
                and rule["maximum_active_reservations"] == 1
                and rule["original_tool_policy"] == {"tools": [], "catalog": "r2"}
                and rule["public_read"] == {"operation": ACQUISITION_OPERATION, "arguments": {"num_stories": 1},
                    "fixture_maximum_calls": 1, "answer_fields": ["id", "title"]},
                "trusted fixture policy does not select this one public read")
        require(self.process.poll() is None and time.monotonic() < self.deadline, "fixture rule is no longer current")
        selection = f.selection
        require((Path(selection["deployment_root"]) / "current").resolve(strict=True)
                == Path(selection["activation"]["path"]), "fixture selected policy changed")
        for key in ("activation", "policy_manifest", "local_policy"):
            exact_bytes(Path(selection[key]["path"]), selection[key]["sha256"])
        owner = f.plan["owner"]["native_host"]
        require(proposal["operator"] == owner["remote_operator"]
                and proposal["application"] == owner["grant_administrator"]
                and proposal["represented_subject"] is None
                and proposal["rule"] == owner["service_parent_administration"]
                and proposal["deployment"] == f.plan["owner"]["deployment"]
                and proposal["source_policy_sha256"] == f.source_pin["sha256"]
                and proposal["original"] == dataclasses.asdict(self.original)
                and proposal["instruction_sha256"] == hashlib.sha256(ACQUISITION_REQUEST.encode()).hexdigest()
                and proposal["expires_in_ms"] is None, "proposal differs from the finite fixture rule")
        with self.provider.lock:
            results = self.provider.results
            acquisition = results["original-request"]["operation"]
            checked = results["checked-install"]
            job = results["installed-job"]["job"]
            reconciled = results["reconciled-install"]["observation"]
            metadata = results["current-operation-names"]
            require(job["status"] == "succeeded" and reconciled["job_id"] == job["id"]
                    and reconciled["status"] == "matched" and checked["scope"] == "configuration_only"
                    and checked["credential_environment_references"] == []
                    and metadata["authentication"] == {"kind": "none"}, "actual public package evidence no longer matches")
            installed = reconciled["result"]
            require(installed["family"] == "agno" and installed["profile"] == "hackernews",
                    "rule does not permit another package")
            servers = installed["mcp_configuration"]["mcpServers"]
            require(len(servers) == 1, "rule requires one actual installed service")
            server = next(iter(servers))
        require(proposal["acquisition"] == {"realm": "mob.fixture", "owner": "toolkit/acquisition/installed-fixture", "id": acquisition}
                and proposal["connector_selection"] == {"server": server, "connector": None},
                "proposal selects another acquisition or service")
        decision = proposal["decision"]
        # The fixture owner approves this complete successor until superseded.
        # Its catalog visibility covers Original's conversation; native service
        # and catalog grants remain separately bounded to this task.
        reviewed_original_catalog_policy(proposal["tool_policy"], decision["catalog"],
            self.admission["session_id"], f.application_tool_policy)
        permission = decision["permission"]
        destination = decision["live_attachment"]["destination"]
        require(decision["route"] == ACQUISITION_ROUTE and decision["choice"] == "approve"
                and permission["target"] == "original" and permission["connection"] == server
                and permission["resource"] == server and permission["operations"] == [ACQUISITION_OPERATION]
                and permission["lifetime"]["scope"] == {"kind": "task", "task": decision["decision"]}
                and type(permission["lifetime"]["expires_at_ms"]) is int
                and int(time.time() * 1000) < permission["lifetime"]["expires_at_ms"] <= self.latest_expiry_ms
                and destination["context"] == self.admission["session_id"]
                and destination["logical_owner"] == installed_fixture_principal("original-agent")
                and destination["audience"] == {"audience": "principal", "principal": installed_fixture_principal("fixture-requester")},
                "rule refuses expanded operation, destination, audience or lifetime")

    def reserve(self, request):
        require(self.reservation is None and self.exclusion.acquire(blocking=False),
                "fixture rule already has an active or completed reservation")
        try:
            proposal = request.proposal
            require(hashlib.sha256(canonical(proposal)).hexdigest() == request.proposal_sha256,
                    "actual proposal digest mismatch")
            self.check(proposal)
            # Bind the finite display expectation before the SDK can submit the
            # reviewed answer. It is never derived from a model message.
            with self.provider.lock:
                require(self.provider.expected_service_context is None,
                        "approved context expectation was already selected")
                self.provider.expected_service_context = installed_approved_service_context(proposal)
        except BaseException:
            self.release_untransferred()
            raise
        owner = self

        class Reservation:
            proposal_sha256 = request.proposal_sha256
            authorization_ref = "fixture-rule-" + request.proposal_sha256
            provenance = {"owner": "installed-fixture", "policy": owner.finalizer.source_pin,
                          "original": dataclasses.asdict(owner.original), "scope": "one public task proposal"}

            def check_current(self):
                try:
                    require(owner.reservation is self and owner.exclusion.locked(), "rule exclusion lost")
                    require(request.proposal == proposal, "retained proposal changed")
                    owner.check(proposal)
                except BaseException as error:
                    raise SDK.HostReplyError(SDK.HostReplyCode.DENIED) from error
                owner.events.add("fixture_rule_current", proposal_sha256=self.proposal_sha256)

            def release(self):
                require(owner.transferred and owner.reservation is self
                        and owner.exclusion.locked() and owner.releases == 0,
                        "rule reservation released more than once")
                owner.releases += 1
                owner.exclusion.release()
                owner.events.add("fixture_rule_released_after_sdk_joined", proposal_sha256=self.proposal_sha256)

        self.reservation = Reservation()
        try:
            self.reservation.check_current()
        except BaseException:
            self.release_untransferred()
            raise
        self.events.add("fixture_rule_reserved", proposal_sha256=request.proposal_sha256)
        return self.reservation

    def release_untransferred(self):
        require(not self.transferred and self.exclusion.locked() and self.releases == 0,
                "cannot release a reservation already transferred to SDK")
        self.exclusion.release()
        self.events.add("fixture_rule_released_before_sdk_transfer")


async def observe_installed_service_reply(observe, provider, process, deadline):
    """Observe one retained SDK exchange; a timeout never resubmits its command."""
    while True:
        with provider.lock:
            errors = list(provider.errors)
        require(not errors, "controlled model schedule failed: " + repr(errors))
        remaining = deadline - time.monotonic()
        require(remaining > 0 and process.poll() is None,
                "native service exchange did not complete before its bound")
        try:
            # The public SDK retains Next/result correlation across timeout.
            # Complete each observer before inspecting failure or shutting down.
            return await asyncio.to_thread(observe, timeout=min(0.25, remaining))
        except TimeoutError:
            continue


async def run_installed_acquisition(private, finalizer, provider, directory, events, process):
    # Authenticate one actual local occurrence before the source crosses the
    # SDK. The fresh key is private to this fixture source and never sent to a
    # model, native host or evidence file. This is not provider/human login.
    deadline = time.monotonic() + ACQUISITION_LIMIT
    # This fixture owns the selected provider-event source. Native bootstrap
    # stages its component; open that exact owner before submitting its event.
    component = "fixture-event-source"
    require(finalizer.plan["owner"]["components"] == [component],
            "fixture event component differs from the selected owner")
    observed = await asyncio.to_thread(private.open_component, component)
    require(observed.component == component and observed.state.value == "open",
            "actual fixture event component did not open")
    events.add("fixture_event_component_opened", observation={
        "incarnation": str(observed.incarnation), "component": observed.component,
        "revision": observed.revision, "state": observed.state.value})
    key = os.urandom(32)
    occurrence = "fixture-event-" + uuid.uuid4().hex
    source = {"route": ACQUISITION_ROUTE, "channel": ACQUISITION_CHANNEL,
        "sender": "fixture-requester", "occurrence": occurrence, "content": ACQUISITION_REQUEST}
    source_bytes = canonical(source)
    signature = hmac.digest(key, source_bytes, "sha256")
    require(hmac.compare_digest(signature, hmac.digest(key, canonical(source), "sha256")), "fixture event authentication failed")
    authenticated = {"event": source, "signature_sha256": hashlib.sha256(signature).hexdigest(),
                     "authority": installed_fixture_principal("event-source")}
    evidence = SDK.PrivateSourceEvidence(occurrence, "fixture-authentication-v1",
        hashlib.sha256(canonical(authenticated)).hexdigest())
    event = SDK.PrivateAuthenticatedEvent(ACQUISITION_ROUTE, occurrence, occurrence,
        evidence, SDK.PrivateSourceEvidence(occurrence, "fixture-event-v1", hashlib.sha256(source_bytes).hexdigest()),
        (), hashlib.sha256(ACQUISITION_REQUEST.encode()).hexdigest())
    event_pin = new_artifact(directory / "authenticated-fixture-event.json", canonical(authenticated))
    retained = await asyncio.to_thread(private.retain_authenticated_event, event, timeout=30.0)
    with provider.lock:
        require(not provider.requests and not provider.errors, "model activity preceded original admission")
        provider.enabled = True
    admission = await asyncio.to_thread(private.submit_authenticated_original,
        retained.original, ACQUISITION_REQUEST, evidence, timeout=60.0)
    events.add("authenticated_original_submission_observed",
        original=dataclasses.asdict(retained.original), outcome=admission)
    require(admission["state"] == "admitted" and admission["source_settlement"] == "fresh_acceptance_promoted"
            and admission["deduplicated"] is False and admission["turn_ticket"], "original native admission not established")
    events.add("authenticated_original_admitted", original=dataclasses.asdict(retained.original), outcome=admission)
    rule = None
    custody = {"transfer": "not_transferred", "retirement": "not_observed"}
    request = await observe_installed_service_reply(
        private.next_service_parent_administration, provider, process, deadline)
    try:
        # Custody starts on receipt, before local observation or preparation can
        # fail. Retirement also covers a request with no rule or proposal pin.
        custody["exchange_id"] = request.exchange_id
        with provider.lock:
            require(not provider.errors, "controlled model schedule failed: " + repr(provider.errors))
        # The original's real tool result is independent of the helper's scheduling.
        await asyncio.to_thread(until, lambda: provider.results.get("original-request"),
                                30.0, "original acquisition observation not returned")
        rule = InstalledFixtureRule(finalizer, provider, retained.original, admission, process, deadline, events)
        proposal_pin = new_artifact(directory / "service-parent-proposal.json", canonical(request.proposal))
        custody["proposal"] = proposal_pin
        reservation = rule.reserve(request)
        try:
            policy_sha256 = request.tool_policy_sha256
            require(isinstance(policy_sha256, str), "reviewed catalog policy digest missing")
            attempt = request.authorize_tool_policy(SDK.StandingRuleDecision(reservation), policy_sha256)
        except BaseException:
            # An interruption can arrive after SDK custody transfer but before
            # return. The retained public attempt, not return timing, decides.
            if request.attempt is None:
                rule.release_untransferred()
            else:
                rule.transferred = True
            raise
        rule.transferred = True
        custody["transfer"] = "sdk_owned"
        ack = await asyncio.to_thread(attempt.submit, timeout=30.0)
        custody["ack"] = ack
        require(ack == {"state": "answer_forwarded"}, "service-parent answer was not correlated")
        observed = await observe_installed_service_reply(attempt.result, provider, process, deadline)
        custody["operation_observation"] = observed
        fresh = request.proposal["decision"]["decision"]
        require(observed["outcome"] == "succeeded" and observed["custody"] == "joined"
                and observed["reason"] is None and observed["parent_issued"] is True
                and observed["parent_issuance_id"] == request.proposal["issuance_id"]
                and observed["child"]["state"] == "grant_issued"
                and observed["child"]["grant"]["route"] == ACQUISITION_ROUTE
                and observed["child"]["grant"]["decision"] == fresh
                and observed["source"]["route"] == ACQUISITION_ROUTE
                and observed["source"]["original"] == fresh
                and fresh != retained.original.original,
                "actual parent/child/fresh source did not join successfully for this proposal")
        require(observed.get("tool_policy") == {
            "proposal_sha256": policy_sha256, "candidate": request.proposal["tool_policy"]["candidate"],
            "native": "accepted", "retained_as_current": True},
            "reviewed Original catalog policy was not natively accepted and retained")
        require(rule.releases == 1 and not rule.exclusion.locked(), "SDK did not release actual rule exclusion after Joined")
        while not provider.finished.is_set():
            require(time.monotonic() < deadline and not provider.errors,
                    "helper did not observe the real public request result")
            await asyncio.sleep(0.05)
        require(not provider.errors, "controlled model schedule failed")
        while not provider.business_response_prepared.is_set():
            require(time.monotonic() < deadline and process.poll() is None and not provider.errors,
                    "original did not observe its actual public result after fresh input")
            await asyncio.sleep(0.05)
        with provider.lock:
            require(not provider.errors and provider.business_observation is not None,
                    "original public response evidence is unavailable")
            business = dict(provider.business_observation)
        # Read the exact fresh source returned by this RequestTool operation.
        # Recovery owns B's admission. This observation never resubmits B or
        # treats the provider's prepared answer as a native terminal fact.
        b_original = SDK.PrivateOriginalRef(**observed["source"])
        b_admission = None
        receipt_reads = 0
        while True:
            remaining = deadline - time.monotonic()
            require(remaining > 0 and process.poll() is None and not provider.errors,
                    "fresh B input did not reach a finalized native receipt")
            terminal_observation = await asyncio.to_thread(private.observe_service_input_receipt,
                b_original, timeout=min(30.0, remaining))
            receipt_reads += 1
            events.add("original_native_receipt_observed", original=dataclasses.asdict(b_original),
                       observation=terminal_observation)
            state = terminal_observation["state"]
            require(state in ("admission_unresolved", "admitted"), "unknown B admission observation")
            if state == "admitted":
                actual = {key: terminal_observation[key] for key in ("session_id", "input_id")}
                require(actual["session_id"] == admission["session_id"]
                        and actual["input_id"] != admission["input_id"],
                        "fresh B was not admitted as a new input in the original session")
                require(b_admission is None or actual == b_admission, "retained B admission changed")
                b_admission = actual
                sourced = terminal_observation["receipt"]
                if sourced is not None:
                    require(sourced["source"] in ("live_runtime", "durable_store"),
                            "native receipt lacks a supported witness source")
                    receipt = sourced["report"]
                    require(receipt["input_id"] == actual["input_id"], "native receipt names another input")
                    status = receipt["status"]
                    require(status in ("pending", "finalized"),
                            "B terminated without its own finalized result receipt")
                    if status == "finalized":
                        run_id = receipt["run_id"]
                        require(isinstance(run_id, str) and bool(run_id)
                                and receipt["scope"] == {"kind": "run", "run_id": run_id}
                                and receipt["owner_input_id"] == actual["input_id"]
                                and receipt["recipient_input_ids"] == [actual["input_id"]]
                                and receipt["terminal"] == {"outcome_type": "consumed"},
                                "B does not own the exact single-recipient completed run receipt")
                        outcome = receipt["outcome"]
                        require(outcome["completion_type"] == "completed"
                                and outcome["session_id"] == admission["session_id"]
                                and type(outcome["tool_calls"]) is int and outcome["tool_calls"] == 3
                                and outcome["text"] == business["answer"],
                                "native B result did not complete exact search, load, one public read and answer")
                        break
            else:
                require(b_admission is None, "retained B admission became unresolved")
            # Pending may already carry a terminal outcome. It still cannot
            # satisfy completion until the native owner finalizes the receipt.
            await asyncio.sleep(0.1)
        completion = {"scope": "one actual installed SDK public read with an exact finalized native B result",
            "authenticated_event": event_pin, "original": dataclasses.asdict(retained.original), "admission": admission,
            "proposal": proposal_pin, "ack": ack, "observation": observed, "custody": custody,
            "rule_releases_after_joined": rule.releases, "helper_result": provider.results["public-task-access"],
            "schema_evidence": provider.results["schema-evidence"],
            "business": business, "fresh_original": dataclasses.asdict(b_original),
            "native_b": terminal_observation, "native_receipt_reads": receipt_reads,
            "service_readiness": "not_independently_observed",
            "original_task_completion": "native_finalized_completed_for_exact_b_input"}
        if "controller_recovery_history_index" in business:
            completion["controller_recovery_audit"] = retained_original_reprepare(
                directory, b_admission["session_id"], b_admission["input_id"], receipt["run_id"])
        new_artifact(directory / "original-business-observation.json", canonical(completion))
        return completion
    finally:
        original_error = sys.exc_info()[1]
        if original_error is not None:
            custody["original_error"] = repr(original_error)
        attempt = request.attempt
        if attempt is not None:
            if rule is not None:
                rule.transferred = True
            custody["transfer"] = "sdk_owned"
        # Keep this exact request and private channel alive through its bounded
        # SDK retirement. Cancellation of this observer cannot drop that call.
        retirement = asyncio.create_task(asyncio.to_thread(
            (attempt if attempt is not None else request).retire, timeout=30.0))
        interrupted = None
        cleanup_error = None
        try:
            while not retirement.done():
                try:
                    await asyncio.shield(retirement)
                except asyncio.CancelledError as error:
                    interrupted = interrupted or error
            retired = retirement.result()
            custody["retirement"] = "joined"
            custody["retirement_observation"] = retired
            if "operation_observation" in custody:
                require(retired == custody["operation_observation"], "retirement changed the original operation observation")
            if attempt is not None:
                require(rule is not None and rule.releases == 1 and not rule.exclusion.locked(),
                        "joined retirement did not release SDK custody")
        except BaseException as error:
            cleanup_error = error
            custody["retirement"] = "retained_uncertain"
            custody["retirement_error"] = repr(error)
        custody["rule_owner_created"] = rule is not None
        custody["rule_releases_after_joined"] = rule.releases if rule is not None else None
        custody["rule_exclusion_still_held"] = rule.exclusion.locked() if rule is not None else None
        if interrupted is not None:
            custody["retirement_observer_interruption"] = repr(interrupted)
        try:
            new_artifact(directory / "service-parent-custody.json", canonical(custody))
        except BaseException as error:
            custody["evidence_write_error"] = repr(error)
            cleanup_error = cleanup_error or error
        events.add("service_parent_custody_observed", **custody)
        if original_error is None:
            if cleanup_error is not None:
                raise cleanup_error
            if interrupted is not None:
                raise interrupted


def run_installed_product(args, candidate, package, resource_manifest, *, corrupt):
    return asyncio.run(_run_installed_product(args, candidate, package, resource_manifest, corrupt=corrupt))


async def _run_installed_product(args, candidate, package, resource_manifest, *, corrupt, console=None):
    require(console is None or not corrupt, "Console journey is a separate positive case")
    name = console.name if console is not None else ("installed-product-resource-corruption" if corrupt else "installed-product-success")
    directory = args.output / name
    directory.mkdir(mode=0o700)
    events = EventLog(directory)
    report = {"case": name, "status": "running", "candidate": candidate,
              "scope": ("installed resource corruption; zero model turns" if corrupt else
                        "installed SDK acquisition and one public read with exact native input completion"),
              "forced_cleanup": False, "scoped_execution_proven": False}
    if console is not None:
        report["scope"] = console.scope
    provider = (console.provider(directory, events) if console is not None else
                LocalProviderSentinel() if corrupt else InstalledPublicCatalogProvider(directory, events))
    environment = process_env(directory)
    empty_path = directory / "empty-path"
    empty_path.mkdir(mode=0o700)
    environment["PATH"] = str(empty_path)
    stderr = directory / "gateway.stderr"
    environment["META_GATEWAY_STDERR_FILE"] = str(stderr)
    gateway = args.toolkit_gateway
    runtime = transport = process = private = public_reader = private_reader = finalizer = None
    outgoing, responses, settlements, observed_children = [], [], [], {}
    http_port = None
    previous_directory = Path.cwd()
    try:
        if corrupt:
            # The same selected binary and resource manifest, with exactly one
            # declared executable resource changed. The payload would leave a
            # marker if native integrity admission ever let bundled code run.
            import shutil
            clone = directory / package.name
            shutil.copytree(package, clone, symlinks=True)
            gateway = clone / "bin/toolkit_gateway"
            resource = clone / "libexec/meerkat-connect" / resource_manifest["prepare"]
            require(resource.name == "prepare.py" and not resource.is_symlink(), "negative resource target")
            original_resource = package / "libexec/meerkat-connect" / resource_manifest["prepare"]
            original_raw = exact_bytes(original_resource)
            require(exact_bytes(resource) == original_raw, "negative clone changed before deliberate corruption")
            before = bytes_pin(original_resource, original_raw)
            marker = directory / "bundled-entry-entered"
            mode = stat.S_IMODE(resource.stat().st_mode)
            resource.chmod(mode | 0o200)
            resource.write_bytes(("from pathlib import Path\nPath(" + repr(str(marker))
                                  + ").write_text('unexpected bundled execution')\n").encode())
            resource.chmod(mode)
            require(digest(gateway) == candidate["binary"]["sha256"]
                    and digest(clone / "libexec/meerkat-connect/resources.json") == candidate["resources_sha256"],
                    "negative changed a candidate identity")
            report["resource_corruption"] = {"before": before, "after": artifact_pin(resource), "marker": str(marker)}
        config_path = directory / "meerkat.toml"
        config_source = config_text(provider.port, acquisition=not corrupt)
        report["model_configuration"] = new_artifact(config_path, config_source.encode())
        specs = [META_IDENTITY.DurableAgentSpec(identity="helper", profile="research", runtime_mode_override="turn_driven")]
        if not corrupt:
            specs.append(META_IDENTITY.DurableAgentSpec(identity="original", profile="original", runtime_mode_override="turn_driven"))
        plan, plan_pin, source_pin = installed_plan(directory, [spec.to_dict() for spec in specs], acquisition=not corrupt, console=console,
            catalog_policy=console is None and not corrupt)
        if console is not None and hasattr(console, "prepare_plan"):
            plan, plan_pin, source_pin = console.prepare_plan(directory, plan, plan_pin, source_pin)
        report["preparation_plan"] = plan_pin
        callbacks = []

        class Providers:
            async def roster(self, context):
                require(context.mob_definition is not None, "native roster context has no definition")
                callbacks.append({"kind": "roster", "context": context.to_dict()})
                return specs

            async def compute_edges(self, target_identities, context):
                callbacks.append({"kind": "topology", "target_identities": list(target_identities)})
                return []

        def observe():
            if process is not None:
                require(process.poll() is None, "main gateway left before finalization")
                snapshot = [{**row, "process_stat": descendant_process_stat(row["pid"])}
                            for row in child_snapshot(process.pid)]
                report.setdefault("descendant_snapshots", []).append({"main_pid": process.pid,
                    "monotonic_ns": time.monotonic_ns(), "children": snapshot})
                for row in snapshot:
                    observed_children[row["pid"]] = row
                report["observed_children"] = list(observed_children.values())

        finalizer_directory = directory / "finalization"
        finalizer_directory.mkdir(mode=0o700)
        finalizer = InstalledFixtureFinalizer(gateway, finalizer_directory, plan, plan_pin,
                                             source_pin, environment, events, observe)

        class ObservedApplication(APP_RUNTIME.ToolkitApplicationRuntime):
            def _make_gateway_transport(self):
                nonlocal transport, process, private, public_reader, private_reader
                # Invoke the actual one-shot application transport constructor.
                transport = super()._make_gateway_transport()
                # Parent's constructor merges env; replace it with the complete
                # isolated fixture env before its single Popen, as existing scope does.
                transport._env = dict(environment)
                write, send, deliver, start = (transport._write_line, transport._send_sync_running,
                                               transport._deliver_init_event, transport.start)

                def observed_write(value, **kwargs):
                    observed = console.wire_value(value) if console is not None else value
                    outgoing.append(observed)
                    events.add("sdk_wire_out", message=observed)
                    return write(value, **kwargs)

                def observed_send(request, *, timeout=None, **kwargs):
                    value = send(request, timeout=timeout, **kwargs)
                    observed = console.wire_value(value) if console is not None else value
                    responses.append(observed)
                    events.add("sdk_wire_response", message=observed)
                    return value

                def observed_deliver(method, params, **kwargs):
                    observed = console.wire_value(params) if console is not None else params
                    settlements.append({"method": method, "params": observed})
                    events.add("sdk_init_event", method=method, params=observed)
                    return deliver(method, params, **kwargs)

                def observed_start():
                    nonlocal process, private, public_reader, private_reader
                    try:
                        return start()
                    finally:
                        process, private = transport._process, transport.host_control
                        public_reader = transport._reader_thread
                        private_reader = private._reader if private is not None else None
                        if process is not None:
                            events.add("main_gateway_started", pid=process.pid, binary=str(gateway))

                transport._write_line, transport._send_sync_running = observed_write, observed_send
                transport._deliver_init_event, transport.start = observed_deliver, observed_start
                return transport

        providers = Providers()
        # Fresh Connect prepares its resources before native init can be accepted.
        config = META_BUILDER.MetaBuilderConfig(gateway_bin=str(gateway),
            init_deadline=INSTALLED_LIMIT, gateway_init_timeout=INSTALLED_LIMIT,
            roster_provider=providers, topology_provider=providers,
            persistent_state=plan["application"]["realm"]["state_root"],
            meerkat_config_path=str(config_path), http_listen="127.0.0.1:0",
            console_read_only=True, console_require_app_auth=False)
        if console is not None:
            console.configure_runtime(config, directory)
        preparation = SDK.ApplicationPreparation(SDK.SelectedHostArtifact(**plan_pin), timeout=INSTALLED_LIMIT)
        runtime = ObservedApplication(config, preparation, finalizer)
        # Native transport has no cwd override; these sequential cases launch
        # outside the checkout and without an ambient Python/Node fallback.
        os.chdir(directory)
        connect_error = None
        try:
            await runtime.connect()
        except BaseException as error:
            connect_error = error
            report["connect_error"] = {"type": type(error).__name__, "message": str(error)}
        if corrupt:
            require(connect_error is not None, "corrupt resource unexpectedly reached Ready")
            diagnostic_raw = exact_bytes(stderr, minimum=0)
            report["gateway_stderr"] = bytes_pin(stderr, diagnostic_raw)
            require(RESOURCE_REFUSAL in diagnostic_raw.decode(errors="replace"),
                    "failure did not discriminate native resource integrity")
            require_installed_resource_refusal(finalizer, plan, report["resource_corruption"], settlements)
            report["resource_refusal"] = RESOURCE_REFUSAL
        else:
            if connect_error is not None:
                raise connect_error
            require(process is not None and process.poll() is None and runtime.is_running,
                    "actual native runtime not running")
            observe()
            attempt = runtime.preparation_attempt
            require(attempt is not None and attempt.manifest is not None
                    and attempt.selection is not None and attempt.settlement is not None
                    and not isinstance(attempt.settlement.native, BaseException)
                    and not isinstance(attempt.settlement.finalization, BaseException)
                    and dataclasses.asdict(attempt.selection) == finalizer.selection,
                    "same-child native completion/adoption not established")
            init = [value for value in outgoing if value.get("method") == "meta/init"]
            require(len(init) == 1, "expected exactly one native init")
            ready = [row for row in settlements if row["method"] == "meta/init_settled"
                     and row["params"].get("init_id") == init[0]["params"]["init_id"]
                     and row["params"].get("outcome") == "ready"]
            require(len(ready) == 1, "missing actual correlated native Ready settlement")
            with events.lock:
                published = next(i for i, row in enumerate(events.rows) if row["event"] == "fixture_published")
                settled = next(i for i, row in enumerate(events.rows) if row["event"] == "sdk_init_event"
                    and row["method"] == "meta/init_settled" and row["params"].get("outcome") == "ready")
            require(published < settled, "native Ready preceded final publication")
            catalog = await runtime._rpc("meta/models/catalog", {}, transport_timeout=30.0)
            require(isinstance(catalog, (dict, list)) and bool(catalog), "native model catalog result missing")
            require(runtime._transport is transport and transport._process is process and process.poll() is None,
                    "catalog used another gateway child")
            from urllib.parse import urlsplit
            url = urlsplit(runtime.rust_http_base_url or "")
            require(url.scheme == "http" and url.hostname == "127.0.0.1" and url.port,
                    "native Ready did not return the selected loopback HTTP listener")
            http_port = url.port
            require(http_probe(http_port), "ready gateway never served HTTP")
            report.update(main_gateway_pid=process.pid, native_ready=ready[0]["params"],
                          model_catalog=new_artifact(directory / "model-catalog.json", canonical(catalog)),
                          callbacks=callbacks, finalization=finalizer.evidence,
                          application_selection=dataclasses.asdict(attempt.selection))
            if console is None:
                report["acquisition"] = await run_installed_acquisition(
                    private, finalizer, provider, directory, events, process)
            else:
                console.application_tool_policy = finalizer.application_tool_policy
                if hasattr(console, "observe_preparation"):
                    console.observe_preparation(attempt, finalizer, gateway, process, observe)
                if hasattr(console, "observe_completed_setup"):
                    console.observe_completed_setup(
                        finalizer.completed_setup, finalizer.evidence["completed_setup"])
                report["console_journey"] = await console.run(
                    private, provider, directory, events, process, runtime.rust_http_base_url)
            observe()
        # Runtime owns its exact existing stop. Never repair by starting another
        # gateway or signal a process group to manufacture successful cleanup.
        try:
            await runtime.shutdown()
        except BaseException as error:
            report["shutdown_error"] = {"type": type(error).__name__, "message": str(error)}
            if not corrupt:
                raise
        termination = runtime.last_gateway_termination
        require(termination is not None and termination.state == "exited"
                and termination.signal is None and termination.escalation == "none",
                "native child cleanup did not settle without signals")
        require(process is not None and process.returncode is not None, "main child not reaped")
        if not corrupt:
            require(termination.exit_code == 0 and process.returncode == 0,
                    "positive gateway did not exit normally")
            shutdown = [value for value in outgoing if value.get("method") == "meta/shutdown"]
            require(len(shutdown) == 1, "expected one actual shutdown request")
            replies = [value for value in responses if str(value.get("id")) == str(shutdown[0]["id"])]
            require(len(replies) == 1 and "error" not in replies[0], "shutdown response correlation missing")
            require(all(replies[0].get("result", {}).get(key) is True for key in
                        ("shutdown", "runtime_cleanup_completed", "host_cleanup_completed")),
                    "shutdown did not attest runtime and host cleanup")
            report["shutdown_response"] = replies[0]
            with socket.socket() as probe:
                probe.settimeout(0.25)
                require(probe.connect_ex(("127.0.0.1", http_port)) != 0, "gateway listener remains open")
            report["http_listener_closed"] = True
        for reader in (public_reader, private_reader):
            require(reader is not None and not reader.is_alive(), "SDK reader not joined")
        require(transport._process is None and not transport._pending and not transport._results,
                "transport retained active process/pending responses")
        require_descendants_gone(report, observed_children)
        if corrupt:
            # Observe again only after the native child and both SDK readers
            # have joined, before retaining a successful negative result.
            require_installed_resource_refusal(finalizer, plan, report["resource_corruption"], settlements)
        report.update(status="passed", termination=dataclasses.asdict(termination),
            natural_gateway_reap=True, sdk_readers_joined=True,
            observed_children=list(observed_children.values()), observed_children_gone=True)
    except BaseException as error:
        report.update(status="failed", error=repr(error), traceback=traceback.format_exc())
    finally:
        if runtime is not None and transport is not None and transport._process is not None:
            try:
                await runtime.shutdown()
            except BaseException as error:
                report["cleanup_error"] = repr(error)
        if transport is not None:
            record = transport.last_termination
            if record is not None:
                report["termination"] = dataclasses.asdict(record)
                if record.escalation != "none":
                    report["forced_cleanup"] = True
                    report["status"] = "failed"
        if finalizer is not None:
            report["finalization"] = finalizer.evidence
            if finalizer.evidence.get("forced_artifact_cleanup"):
                report["forced_cleanup"] = True
                report["status"] = "failed"
        # Join the existing loopback provider after the actual child settles.
        try:
            provider.close(drain=True)
            report["provider_joined"] = True
        except BaseException as error:
            report["provider_cleanup_error"] = repr(error)
            report["provider_joined"] = False
            report["status"] = "failed"
        if corrupt:
            report["sentinel_connections"] = list(provider.connections)
            if report["sentinel_connections"]:
                report["status"] = "failed"
                report["sentinel_refusal"] = "zero-model fixture contacted provider sentinel"
        else:
            report["model_requests"] = artifact_pin(directory / "model-requests.json")
            report["model_request_count"] = len(provider.requests)
            report["model_schedule_errors"] = list(provider.errors)
            if provider.errors:
                report["status"] = "failed"
            custody_path = directory / "service-parent-custody.json"
            if custody_path.exists():
                report["service_parent_custody"] = artifact_pin(custody_path)
            business_path = directory / "original-business-observation.json"
            if business_path.exists():
                report["original_business_observation"] = artifact_pin(business_path)
        if console is not None and (directory / "console-journey.json").exists():
            report["console_evidence"] = artifact_pin(directory / "console-journey.json")
        if stderr.exists():
            settled_stderr = bytes_pin(stderr, exact_bytes(stderr, minimum=0))
            if "gateway_stderr" in report and report["gateway_stderr"] != settled_stderr:
                report["status"] = "failed"
                report["stderr_refusal"] = "native diagnostic changed after its checked observation"
            report["gateway_stderr"] = settled_stderr
        os.chdir(previous_directory)
        events.flush()
        report["events"] = artifact_pin(directory / "events.json")
        if console is not None:
            report = console.wire_value(report)
            try:
                console.check_final_artifacts(directory, report)
            except BaseException:
                report = console.report_value(report)
                report.update(status="failed", evidence_refusal="Console final evidence check failed")
        save_json(directory / "result.json", report)
    return report


def main():
    global SDK, INBOX, APP_RUNTIME, META_BUILDER, META_IDENTITY
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--toolkit-gateway", required=True, type=Path)
    parser.add_argument("--raw-gateway", type=Path)
    parser.add_argument("--toolkit-python", required=True, type=Path)
    parser.add_argument("--meta-python", "--mobkit-python", dest="mobkit_python", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--case", choices=("core", "all", "memory-cleanup", "shutdown-reader", "private-protocol", "installed-product", "installed-console", "installed-console-credentials", "installed-console-browser", "installed-console-warm", "installed-console-source-build", "installed-console-helper-reset"), default="core")
    parser.add_argument("--installed-staging", type=Path)
    parser.add_argument("--installed-staging-sha256")
    parser.add_argument("--browser-forge-source", type=Path)
    parser.add_argument("--browser-forge-source-sha256")
    parser.add_argument("--helper-reset-forge-source", type=Path)
    parser.add_argument("--helper-reset-forge-source-sha256")
    args = parser.parse_args()
    helper_reset = args.case == "installed-console-helper-reset"
    if helper_reset:
        if args.helper_reset_forge_source is None or not args.helper_reset_forge_source_sha256:
            parser.error("Helper reset needs exact selected Forge adapter source receipt/hash")
    elif args.helper_reset_forge_source is not None or args.helper_reset_forge_source_sha256 is not None:
        parser.error("Helper-reset Forge inputs apply only to installed-console-helper-reset")
    browser = args.case == "installed-console-browser"
    if browser:
        if args.browser_forge_source is None or not args.browser_forge_source_sha256:
            parser.error("Browser mode needs an exact selected shipping Forge source receipt and hash")
    elif args.browser_forge_source is not None or args.browser_forge_source_sha256 is not None:
        parser.error("Forge Browser source inputs apply only to installed-console-browser")
    if sys.platform != "darwin" and not sys.platform.startswith("linux"):
        parser.error("verifier supports explicit macOS /dev/fd or Linux /proc/self/fd inventory only")
    installed = args.case in ("installed-product", "installed-console", "installed-console-credentials", "installed-console-browser", "installed-console-warm", "installed-console-source-build", "installed-console-helper-reset")
    if installed:
        if args.raw_gateway is not None or args.installed_staging is None or args.installed_staging_sha256 is None:
            parser.error("installed-product needs the exact staging path/hash and no unrelated raw gateway")
    elif args.raw_gateway is None or args.installed_staging is not None or args.installed_staging_sha256 is not None:
        parser.error("legacy scopes require raw gateway; installed staging applies only to installed-product")
    path_names = ["toolkit_gateway", "toolkit_python", "mobkit_python", "output"]
    path_names += ["installed_staging"] if installed else ["raw_gateway"]
    if browser:
        path_names.append("browser_forge_source")
    if helper_reset:
        path_names.append("helper_reset_forge_source")
    for name in path_names:
        value = getattr(args, name)
        if not value.is_absolute():
            parser.error(f"--{name.replace('_', '-')} must be absolute")
        setattr(args, name, value.resolve())
    for binary in ([args.toolkit_gateway] if installed else [args.toolkit_gateway, args.raw_gateway]):
        if not binary.is_file() or not os.access(binary, os.X_OK):
            parser.error(f"explicit built executable is unavailable: {binary}")
    installed_selection = None
    if installed:
        try:
            installed_selection = installed_candidate(args)
        except (AssertionError, KeyError, TypeError, ValueError, OSError) as error:
            parser.error(f"invalid installed selection: {error}")
        if args.output.is_relative_to(installed_selection[1]):
            parser.error("output directory must be outside the selected installed package")
    if args.output.exists():
        parser.error("output directory must not exist; retained evidence is never overwritten")
    args.output.mkdir(parents=True)
    report = {"schema": 1, "platform": platform.platform(), "python": sys.version,
              "script_sha256": digest(__file__), "toolkit_gateway": str(args.toolkit_gateway),
              "toolkit_gateway_sha256": digest(args.toolkit_gateway),
              "raw_gateway": str(args.raw_gateway) if args.raw_gateway is not None else None,
              "raw_gateway_sha256": digest(args.raw_gateway) if args.raw_gateway is not None else None,
              "execution_claim": "local subprocess composition only; no public scoped or paired host support claim",
              "cases": [], "status": "running"}
    try:
        sys.path[0:0] = [str(args.toolkit_python), str(args.mobkit_python)]
        SDK = importlib.import_module("meerkat_toolkit.host_control")
        modules = [(SDK, args.toolkit_python)]
        if installed:
            APP_RUNTIME = importlib.import_module("meerkat_toolkit.application_runtime")
            META_BUILDER = importlib.import_module("meerkat_meta.builder")
            META_IDENTITY = importlib.import_module("meerkat_meta.identity_first_models")
            modules.append((APP_RUNTIME, args.toolkit_python))
            modules += [(importlib.import_module(name), args.mobkit_python) for name in (
                "meerkat_meta", "meerkat_meta.builder", "meerkat_meta.runtime", "meerkat_meta._transport",
                "meerkat_meta.agent_builder", "meerkat_meta.identity_first_models",
                "meerkat_meta.identity_first_providers", "meerkat_meta.types")]
        else:
            INBOX = importlib.import_module("meerkat_toolkit.frontend.inbox")
            modules += [(INBOX, args.toolkit_python),
                        (importlib.import_module("meerkat_meta._transport"), args.mobkit_python)]
        for module, root in modules:
            path = Path(module.__file__).resolve()
            require(path.is_relative_to(root), f"SDK import escaped explicit root: {path}")
            report.setdefault("sdk_sources", []).append({"path": str(path), "sha256": digest(path)})
        if installed:
            candidate, package, resource_manifest, inputs = installed_selection
            report.update(installed_candidate=candidate, installed_inputs=inputs,
                execution_claim="installed SDK acquisition and one public read; success requires exact native input completion; no live authentication or general service support claim")
            console = None
            if args.case in ("installed-console", "installed-console-credentials", "installed-console-browser", "installed-console-warm", "installed-console-source-build", "installed-console-helper-reset"):
                module_path = Path(__file__).with_name("verify_installed_console.py")
                spec = importlib.util.spec_from_file_location("connect_installed_console", module_path)
                module = importlib.util.module_from_spec(spec)
                spec.loader.exec_module(module)
                report["console_verifier"] = artifact_pin(module_path)
                if helper_reset:
                    reset_path = Path(__file__).with_name("verify_installed_helper_reset.py")
                    reset_spec = importlib.util.spec_from_file_location("connect_installed_helper_reset", reset_path)
                    reset_module = importlib.util.module_from_spec(reset_spec)
                    reset_spec.loader.exec_module(reset_module)
                    report["helper_reset_verifier"] = artifact_pin(reset_path)
                    console = reset_module.create_scenario(sys.modules[__name__], module, module.loopback_origin(),
                        args.helper_reset_forge_source, args.helper_reset_forge_source_sha256)
                elif args.case == "installed-console-source-build":
                    warm_path = Path(__file__).with_name("verify_installed_warm.py")
                    warm_spec = importlib.util.spec_from_file_location("connect_installed_warm", warm_path)
                    warm_module = importlib.util.module_from_spec(warm_spec)
                    warm_spec.loader.exec_module(warm_module)
                    source_path = Path(__file__).with_name("verify_installed_source_build.py")
                    source_spec = importlib.util.spec_from_file_location("connect_installed_source_build", source_path)
                    source_module = importlib.util.module_from_spec(source_spec)
                    source_spec.loader.exec_module(source_module)
                    report["warm_observation_helpers"] = artifact_pin(warm_path)
                    report["source_build_verifier"] = artifact_pin(source_path)
                    console = source_module.create_scenario(sys.modules[__name__], module, warm_module, module.loopback_origin())
                elif args.case == "installed-console-warm":
                    warm_path = Path(__file__).with_name("verify_installed_warm.py")
                    warm_spec = importlib.util.spec_from_file_location("connect_installed_warm", warm_path)
                    warm_module = importlib.util.module_from_spec(warm_spec)
                    warm_spec.loader.exec_module(warm_module)
                    report["warm_verifier"] = artifact_pin(warm_path)
                    console = warm_module.create_scenario(sys.modules[__name__], module, module.loopback_origin())
                else:
                    scenario = (module.TrelloConsoleScenario if args.case == "installed-console-credentials"
                                else module.ConsoleScenario)
                    console = (module.BrowserConsoleScenario(sys.modules[__name__], module.loopback_origin(),
                        args.browser_forge_source, args.browser_forge_source_sha256) if browser else
                        scenario(sys.modules[__name__], module.loopback_origin()))
                report["execution_claim"] = console.scope
            for corrupt in ((False,) if console is not None else (False, True)):
                result = (asyncio.run(_run_installed_product(args, candidate, package, resource_manifest,
                    corrupt=False, console=console)) if console is not None else
                    run_installed_product(args, candidate, package, resource_manifest, corrupt=corrupt))
                report["cases"].append(result)
                print(f"{result['case']}: {result['status']}", flush=True)
                if result["status"] != "passed":
                    break
            if console is not None:
                require(digest(module_path) == report["console_verifier"]["sha256"], "Console verifier helper changed during verification")
            if args.case == "installed-console-warm":
                require(digest(warm_path) == report["warm_verifier"]["sha256"], "Warm verifier changed during verification")
            if args.case == "installed-console-source-build":
                require(digest(warm_path) == report["warm_observation_helpers"]["sha256"], "Observation helpers changed during verification")
                require(digest(source_path) == report["source_build_verifier"]["sha256"], "Source-build verifier changed during verification")
            if helper_reset:
                require(digest(reset_path) == report["helper_reset_verifier"]["sha256"], "Reset verifier changed during verification")
                console.check_sources()
            require(installed_candidate(args)[0] == candidate, "selected staged candidate changed during verification")
            for row in report["sdk_sources"]:
                require(digest(Path(row["path"])) == row["sha256"], "selected SDK source changed during verification")
            report["status"] = "passed" if len(report["cases"]) == (1 if console is not None else 2) and all(
                item["status"] == "passed" for item in report["cases"]) else "failed"
            return 0 if report["status"] == "passed" else 1
        cases = [("native-success", True, False, None),
                 ("native-member-mismatch", True, True, None),
                 ("raw-success", False, False, None),
                 ("native-public-stdin-eof", True, False, "stdin_eof"),
                 ("native-public-stdout-loss", True, False, "stdout_loss"),
                 ("native-private-eof", True, False, "private_eof"),
                 ("native-private-protocol", True, False, "private_protocol")]
        if args.case == "private-protocol":
            cases = [case for case in cases if case[0] == "native-private-protocol"]
        elif args.case in ("memory-cleanup", "shutdown-reader"):
            cases = []
        for name, native, mismatch, loss in cases:
            result = run_case(args, name, native, mismatch, loss)
            report["cases"].append(result)
            print(f"{name}: {result['status']}", flush=True)
            # Failure of the canonical positive fixture is not negative evidence.
            if name == "native-success" and result["status"] != "passed":
                report["remaining_cases"] = "not run: canonical native positive prerequisite failed"
                break
        if args.case in ("all", "memory-cleanup") and (len(report["cases"]) == len(cases) and
                                               all(case["status"] == "passed" for case in report["cases"])):
            report["cases"].append(run_memory_cleanup_fault(args))
        if args.case == "shutdown-reader":
            report["cases"].append(run_shutdown_reader(args))
        expected_cases = len(cases) + int(args.case in ("all", "memory-cleanup", "shutdown-reader"))
        report["status"] = "passed" if len(report["cases"]) == expected_cases and all(
            case["status"] == "passed" for case in report["cases"]) else "failed"
    except BaseException as error:
        report["status"] = "prerequisite_failed"
        report["error"] = repr(error)
        report["traceback"] = traceback.format_exc()
    finally:
        save_json(args.output / "manifest.json", report)
    return 0 if report["status"] == "passed" else 1


if __name__ == "__main__":
    if len(sys.argv) > 1 and sys.argv[1] == "--fixture-lifetime-child":
        raise SystemExit(fixture_lifetime_child(sys.argv[2:]))
    if len(sys.argv) > 1 and sys.argv[1] == "--fixture-child":
        raise SystemExit(fixture_child(sys.argv[2:]))
    raise SystemExit(main())
