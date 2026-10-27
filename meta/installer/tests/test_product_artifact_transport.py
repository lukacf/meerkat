"""Offline artifact streams over existing SSH owners, never a native runtime."""
import asyncio
import base64
from dataclasses import replace
import hashlib
import json
import os
from pathlib import Path
import shlex
import signal
import struct
import threading

import pytest

from launchkit import ssh_transport
from launchkit.gcp_transport import GcpIapTransport, checked_host_key
from launchkit.runtime_candidates import ArtifactFile
from launchkit.ssh_profiles import SshProfiles
from launchkit.ssh_transport import FRAME_PREFIX, SshTransport, SshTransportError


ENDPOINT = {"host": "synthetic.example", "user": "fixture", "ssh_port": 2222}
RAW_KEY = struct.pack(">I", 11) + b"ssh-ed25519" + struct.pack(">I", 32) + bytes(range(32))
FINGERPRINT = "SHA256:" + base64.b64encode(hashlib.sha256(RAW_KEY).digest()).decode().rstrip("=")
HOSTS = b"[synthetic.example]:2222 ssh-ed25519 " + base64.b64encode(RAW_KEY) + b"\n"
PRIVATE_KEY = b"OFFLINE FIXTURE ONLY - NOT AN SSH IDENTITY\n"
GCP_ENDPOINT = {"transport": "gcp-iap", "operation_id": "11111111-1111-4111-8111-111111111111",
    "account": "fixture@example.test", "project": "fixture-project", "zone": "europe-west1-b",
    "instance": "fixture-app", "instance_id": "1234567890123456789",
    "ownership_token": "11111111111141118111111111111111", "host": "compute.1234567890123456789",
    "user": "mobkit", "ssh_port": 22}
GCP_KEY = {"key_type": "ssh-ed25519", "key": base64.b64encode(RAW_KEY).decode()}


def private(path, content):
    path.write_bytes(content)
    path.chmod(0o600)
    return path


def artifact(tmp_path, content=b"complete product bytes\x00\xff\n", name="product.tar.gz"):
    path = private(tmp_path.resolve() / name, content)
    return ArtifactFile(path, len(content), hashlib.sha256(content).hexdigest())


def diagnostic(host=ENDPOINT["host"], port=ENDPOINT["ssh_port"], fingerprint=FINGERPRINT):
    alias = host if port == 22 else f"[{host}]:{port}"
    return (f"debug1: Server host key: ssh-ed25519 {fingerprint}\n"
        f"debug1: Host '{alias}' is known and matches the ED25519 host key.\n").encode()


def response(value=None):
    return (FRAME_PREFIX + json.dumps(value or {"stored": True}, separators=(",", ":")) + "\n").encode()


class RecordingSink:
    """Exercise short writes without retaining a real process or pipe."""
    def __init__(self, maximum=65531, on_write=None):
        self.data, self.requests, self.closed = bytearray(), [], False
        self.maximum, self.on_write = maximum, on_write

    def write(self, value):
        assert not self.closed
        value = bytes(value)
        self.requests.append(len(value))
        count = min(len(value), self.maximum)
        self.data.extend(value[:count])
        if self.on_write:
            self.on_write(self)
        return count

    def flush(self):
        pass

    def close(self):
        self.closed = True


def parse_stream(data):
    assert len(data) >= 4
    header_size, = struct.unpack(">I", bytes(data[:4]))
    assert 0 < header_size <= ssh_transport.MAX_TRANSFER_HEADER_BYTES
    header_bytes = bytes(data[4:4 + header_size])
    assert len(header_bytes) == header_size and header_bytes.isascii()
    return json.loads(header_bytes), bytes(data[4 + header_size:])


def intercept_stream(monkeypatch, *, sink=None, fingerprint=FINGERPRINT, host=None, port=None):
    records = []
    sink = sink or RecordingSink()

    async def execute(argv, writer, timeout, *, require_current=None):
        records.append((list(argv), timeout, sink))
        ssh_transport._transfer_current(require_current)
        await writer(sink)
        ssh_transport._transfer_current(require_current)
        return response(), diagnostic(host or ENDPOINT["host"], port or ENDPOINT["ssh_port"], fingerprint), 0

    async def pin(self, endpoint, expected, deadline):
        assert expected == hashlib.sha256(RAW_KEY).digest()
        return HOSTS

    monkeypatch.setattr(SshTransport, "_pin", pin)
    monkeypatch.setattr(ssh_transport, "_execute_stream", execute)
    monkeypatch.setattr(ssh_transport.subprocess, "Popen", lambda *a, **k: pytest.fail("Real process forbidden."))
    return records, sink


async def test_exact_binary_stream_handles_short_writes_without_json_or_argv_file_bytes(tmp_path, monkeypatch):
    content = bytes(range(256)) * 8193
    selected = artifact(tmp_path, content)
    records, sink = intercept_stream(monkeypatch)
    payload = {"action": "store", "note": "\u00e5\U0001f30c", "private": "fixture-secret"}
    program = "print('host-owned receiver')\n# $() is inert"
    result = await SshTransport().transfer_file(ENDPOINT, program, payload, selected,
        expected_fingerprint=FINGERPRINT)
    header, transferred = parse_stream(sink.data)
    assert header == {**payload, "transfer_file": {"size": len(content), "sha256": selected.sha256}}
    assert transferred == content and sink.closed
    assert payload == {"action": "store", "note": "\u00e5\U0001f30c", "private": "fixture-secret"}
    assert result == {"stored": True, "_transport": {"host_key_fingerprint": FINGERPRINT}}
    argv, timeout, _ = records[0]
    assert argv[0] == "/usr/bin/ssh" and 0 < timeout <= 30
    assert argv[-7:-1] == ["-p", "2222", "-l", "fixture", "--", "synthetic.example"]
    assert shlex.split(argv[-1]) == ["python3", "-I", "-c", program]
    for option in ("StrictHostKeyChecking=yes", "BatchMode=yes", "UpdateHostKeys=no",
                   "ForwardAgent=no", "PermitLocalCommand=no", "ClearAllForwardings=yes",
                   "ControlMaster=no", "ControlPath=none", "FingerprintHash=sha256"):
        assert option in argv and argv[argv.index(option) - 1] == "-o"
    assert "fixture-secret" not in " ".join(argv) and str(selected.path) not in " ".join(argv)
    # The artifact is read in bounded chunks; it is never passed as one write.
    assert max(sink.requests) <= 1024 * 1024 and max(sink.requests) < len(content)


@pytest.mark.parametrize("case", ["size-bool", "size-zero", "size-negative", "size-too-large",
    "size-wrong", "hash-malformed", "hash-wrong", "relative", "parent-traversal", "symlink",
    "parent-link", "hardlink", "directory", "fifo", "foreign-owner"])
async def test_unbound_or_unsafe_source_never_creates_transfer_process(tmp_path, monkeypatch, case):
    selected = artifact(tmp_path)
    if case == "size-bool": selected = replace(selected, size=True)
    elif case == "size-zero": selected = replace(selected, size=0)
    elif case == "size-negative": selected = replace(selected, size=-1)
    elif case == "size-too-large": selected = replace(selected, size=ssh_transport.MAX_TRANSFER_FILE_BYTES + 1)
    elif case == "size-wrong": selected = replace(selected, size=selected.size + 1)
    elif case == "hash-malformed": selected = replace(selected, sha256="SENSITIVE-INVALID-HASH")
    elif case == "hash-wrong": selected = replace(selected, sha256="0" * 64)
    elif case == "relative": selected = replace(selected, path=Path(selected.path.name))
    elif case == "parent-traversal": selected = replace(selected, path=selected.path.parent / ".." / selected.path.parent.name / selected.path.name)
    elif case == "symlink":
        link = selected.path.with_name("source-link"); link.symlink_to(selected.path)
        selected = replace(selected, path=link)
    elif case == "parent-link":
        link = selected.path.parent / "parent-link"; link.symlink_to(selected.path.parent, target_is_directory=True)
        selected = replace(selected, path=link / selected.path.name)
    elif case == "hardlink": os.link(selected.path, selected.path.with_name("second-link"))
    elif case == "directory":
        selected.path.unlink(); selected.path.mkdir()
    elif case == "fifo":
        selected.path.unlink(); os.mkfifo(selected.path, 0o600)
    else:
        uid = os.getuid()
        monkeypatch.setattr(ssh_transport.os, "getuid", lambda: uid + 1)
    async def forbidden(*a, **k): pytest.fail("Unsafe source reached process creation.")
    monkeypatch.setattr(ssh_transport, "_execute_stream", forbidden)
    monkeypatch.setattr(ssh_transport.subprocess, "Popen", lambda *a, **k: pytest.fail("Unsafe source spawned."))
    with pytest.raises(SshTransportError) as failed:
        await SshTransport().transfer_file(ENDPOINT, "pass", {}, selected, expected_fingerprint=FINGERPRINT)
    assert failed.value.code == "invalid_request"
    assert str(selected.path) not in str(failed.value) and "SENSITIVE" not in str(failed.value)


@pytest.mark.parametrize("payload", [[], {"transfer_file": {}}, {"bad": float("nan")}, {"bad": object()}])
async def test_reserved_or_non_json_header_is_rejected_before_process(tmp_path, monkeypatch, payload):
    async def forbidden(*a, **k): pytest.fail("Invalid header created a transfer.")
    monkeypatch.setattr(ssh_transport, "_execute_stream", forbidden)
    with pytest.raises(SshTransportError) as failed:
        await SshTransport().transfer_file(ENDPOINT, "pass", payload, artifact(tmp_path),
            expected_fingerprint=FINGERPRINT)
    assert failed.value.code == "invalid_request"


async def test_header_bound_is_checked_before_process_creation(tmp_path, monkeypatch):
    monkeypatch.setattr(ssh_transport, "MAX_TRANSFER_HEADER_BYTES", 128)
    async def forbidden(*a, **k): pytest.fail("Oversized header created a transfer.")
    monkeypatch.setattr(ssh_transport, "_execute_stream", forbidden)
    with pytest.raises(SshTransportError) as failed:
        await SshTransport().transfer_file(ENDPOINT, "pass", {"oversized": "x" * 128}, artifact(tmp_path),
            expected_fingerprint=FINGERPRINT)
    assert failed.value.code == "invalid_request"


async def test_pin_and_stream_share_original_endpoint_and_absolute_deadline(tmp_path, monkeypatch):
    selected, original = artifact(tmp_path), dict(ENDPOINT)
    loop = asyncio.get_running_loop()
    clock, elapsed, observations = loop.time, [0], []
    deadline = clock() + 6
    records, _ = intercept_stream(monkeypatch)
    async def pin(self, endpoint, expected, given_deadline):
        observations.append((dict(endpoint), expected, given_deadline))
        original["host"] = "replacement.example"
        elapsed[0] = 4
        return HOSTS
    monkeypatch.setattr(SshTransport, "_pin", pin)
    with monkeypatch.context() as patch:
        patch.setattr(loop, "time", lambda: clock() + elapsed[0])
        await SshTransport().transfer_file(original, "pass", {}, selected, 20,
            expected_fingerprint=FINGERPRINT, deadline=deadline)
    assert observations == [(ENDPOINT, hashlib.sha256(RAW_KEY).digest(), deadline)]
    argv, remaining, _ = records[0]
    assert argv[-2] == ENDPOINT["host"] and 0 < remaining <= 2
    pin_options = [arg for arg in argv if arg.startswith("UserKnownHostsFile=")]
    assert len(pin_options) == 1 and "GlobalKnownHostsFile=/dev/null" in argv
    assert not Path(pin_options[0].split("=", 1)[1]).exists()


async def test_changed_source_during_pin_is_rejected_before_stream(tmp_path, monkeypatch):
    selected = artifact(tmp_path)
    async def pin(self, *a):
        selected.path.write_bytes(b"x" * selected.size)
        return HOSTS
    async def forbidden(*a, **k): pytest.fail("Changed source reached SSH child.")
    monkeypatch.setattr(SshTransport, "_pin", pin)
    monkeypatch.setattr(ssh_transport, "_execute_stream", forbidden)
    with pytest.raises(SshTransportError) as failed:
        await SshTransport().transfer_file(ENDPOINT, "pass", {}, selected, expected_fingerprint=FINGERPRINT)
    assert failed.value.code == "invalid_request"


@pytest.mark.parametrize("change", ["bytes", "inode"])
async def test_source_change_during_stream_cannot_return_stored_success(tmp_path, monkeypatch, change):
    selected = artifact(tmp_path, b"x" * (2 * 1024 * 1024 + 1))
    changed = []
    def mutate(sink):
        if changed or len(sink.data) < 4096:
            return
        changed.append(True)
        if change == "bytes": selected.path.write_bytes(b"y" * selected.size)
        else:
            replacement = private(selected.path.with_name("replacement"), b"x" * selected.size)
            replacement.replace(selected.path)
    _, sink = intercept_stream(monkeypatch, sink=RecordingSink(on_write=mutate))
    with pytest.raises(SshTransportError) as failed:
        await SshTransport().transfer_file(ENDPOINT, "pass", {}, selected, expected_fingerprint=FINGERPRINT)
    assert changed and failed.value.code == "invalid_request" and sink.closed


@pytest.mark.parametrize("currentness", ["false", "awaitable"])
async def test_false_or_async_currentness_is_rejected_without_transfer(tmp_path, monkeypatch, currentness):
    selected = artifact(tmp_path)
    def current():
        return False if currentness == "false" else asyncio.get_running_loop().create_future()
    async def forbidden(*a, **k): pytest.fail("Stale selection reached process creation.")
    monkeypatch.setattr(ssh_transport, "_execute_stream", forbidden)
    with pytest.raises(SshTransportError) as failed:
        await SshTransport().transfer_file(ENDPOINT, "pass", {}, selected,
            expected_fingerprint=FINGERPRINT, require_current=current)
    assert failed.value.code == "invalid_request"


async def test_currentness_is_rechecked_after_pin_before_child(tmp_path, monkeypatch):
    selected, current = artifact(tmp_path), [True]
    async def pin(self, *a):
        current[0] = False
        return HOSTS
    async def forbidden(*a, **k): pytest.fail("Stale selection created SSH child.")
    monkeypatch.setattr(SshTransport, "_pin", pin)
    monkeypatch.setattr(ssh_transport, "_execute_stream", forbidden)
    with pytest.raises(SshTransportError) as failed:
        await SshTransport().transfer_file(ENDPOINT, "pass", {}, selected,
            expected_fingerprint=FINGERPRINT, require_current=lambda: current[0])
    assert failed.value.code == "invalid_request"


async def test_currentness_change_during_stream_stops_transfer(tmp_path, monkeypatch):
    selected, current = artifact(tmp_path, b"x" * (2 * 1024 * 1024 + 1)), [True]
    def stale(sink):
        if len(sink.data) > 4096: current[0] = False
    _, sink = intercept_stream(monkeypatch, sink=RecordingSink(on_write=stale))
    with pytest.raises(SshTransportError) as failed:
        await SshTransport().transfer_file(ENDPOINT, "pass", {}, selected,
            expected_fingerprint=FINGERPRINT, require_current=lambda: current[0])
    assert failed.value.code == "invalid_request" and sink.closed
    # Fast bytes may already reach the private receiver before the loop notices
    # a changed owner. That cannot become a successful transfer observation.


async def test_loop_affine_owner_stays_on_original_calling_task_during_hash_and_stream(tmp_path, monkeypatch):
    selected = artifact(tmp_path, b"x" * (2 * 1024 * 1024 + 1))
    loop, owner, observations = asyncio.get_running_loop(), asyncio.current_task(), []
    def current():
        assert asyncio.get_running_loop() is loop
        assert asyncio.current_task() is owner
        observations.append(True)
    _, sink = intercept_stream(monkeypatch)
    result = await SshTransport().transfer_file(ENDPOINT, "pass", {}, selected,
        expected_fingerprint=FINGERPRINT, require_current=current)
    assert result["stored"] is True and sink.closed and len(observations) >= 4


async def test_verified_transfer_response_must_match_retained_fingerprint(tmp_path, monkeypatch):
    records, _ = intercept_stream(monkeypatch, fingerprint="SHA256:" + "A" * 43)
    async def pin(self, *a): return HOSTS
    monkeypatch.setattr(SshTransport, "_pin", pin)
    with pytest.raises(SshTransportError) as failed:
        await SshTransport().transfer_file(ENDPOINT, "pass", {}, artifact(tmp_path), expected_fingerprint=FINGERPRINT)
    assert failed.value.code == "invalid_response" and len(records) == 1
    pin = next(arg.split("=", 1)[1] for arg in records[0][0] if arg.startswith("UserKnownHostsFile="))
    assert not Path(pin).exists()


async def test_binary_transfer_requires_existing_fingerprint_before_read_or_spawn(tmp_path, monkeypatch):
    async def forbidden(*a, **k): pytest.fail("Missing pin reached source admission or process creation.")
    monkeypatch.setattr(ssh_transport, "_observe_transfer_source", forbidden)
    monkeypatch.setattr(ssh_transport, "_execute_stream", forbidden)
    with pytest.raises(SshTransportError) as failed:
        await SshTransport().transfer_file(ENDPOINT, "pass", {}, artifact(tmp_path))
    assert failed.value.code == "invalid_request"


async def test_stream_cancellation_reaps_only_owned_process_and_closes_every_pipe(monkeypatch):
    entered, drained, written, signals, created = asyncio.Event(), [], [], [], []
    class Pipe:
        closed = False
        def close(self): self.closed = True
    class Process:
        pid, returncode = 765432, None
        def __init__(self): self.stdin, self.stdout, self.stderr = Pipe(), Pipe(), Pipe()
        def wait(self, timeout):
            assert timeout == ssh_transport.FORCE_REAP_TIMEOUT
            self.returncode = -signal.SIGKILL
            return self.returncode
    process = Process()
    def popen(argv, **kwargs):
        created.append((argv, kwargs))
        return process
    async def read(stream, *, stdout):
        try: await asyncio.Future()
        finally: drained.append(stream)
    async def writer(stream):
        entered.set()
        try: await asyncio.Future()
        finally: written.append(stream)
    monkeypatch.setattr(ssh_transport.subprocess, "Popen", popen)
    monkeypatch.setattr(ssh_transport.os, "killpg", lambda pid, sig: signals.append((pid, sig)))
    monkeypatch.setattr(ssh_transport, "_read", read)
    task = asyncio.create_task(ssh_transport._execute_stream(["offline-ssh"], writer, 30))
    await asyncio.wait_for(entered.wait(), 30)
    task.cancel()
    with pytest.raises(asyncio.CancelledError): await asyncio.wait_for(task, 1)
    assert signals == [(process.pid, signal.SIGKILL)]
    assert set(drained) == {process.stdout, process.stderr} and written == [process.stdin]
    assert all(pipe.closed for pipe in (process.stdin, process.stdout, process.stderr))
    kwargs = created[0][1]
    assert kwargs["start_new_session"] is True and "shell" not in kwargs
    assert set(kwargs["env"]) <= {"HOME", "USER", "PATH", "SSH_AUTH_SOCK"}


def profile(tmp_path):
    identity = private(tmp_path.resolve() / "identity", PRIVATE_KEY)
    hosts = private(tmp_path.resolve() / "known_hosts", HOSTS)
    document = private(tmp_path.resolve() / "profiles.json", json.dumps({"version": 1,
        "profiles": [{"endpoint": ENDPOINT, "identity_file": str(identity), "known_hosts_file": str(hosts)}]}).encode())
    return SshProfiles(str(document)), (document, identity, hosts)


@pytest.mark.parametrize("kind", ["binary", "json"])
async def test_cancelled_blocked_writer_retains_owned_inputs_until_original_thread_finishes(tmp_path, monkeypatch, kind):
    profiles, originals = profile(tmp_path)
    selected = artifact(tmp_path)
    entered, release = threading.Event(), threading.Event()
    pipe_closed, reaped, worker_finished = threading.Event(), threading.Event(), threading.Event()
    copies, sources, signals = [], [], []
    original_copies, original_source = profiles._copies, ssh_transport._observe_transfer_source
    class Pipe:
        closed = False
        def close(self): self.closed = True
    class BlockedPipe(Pipe):
        writer_thread = None
        def write(self, value):
            self.writer_thread = threading.get_ident()
            entered.set()
            release.wait()
            if self.closed:
                raise BrokenPipeError("Offline closed fixture pipe.")
            return len(value)
        def close(self):
            self.closed = True
            pipe_closed.set()
            if self.writer_thread == threading.get_ident():
                worker_finished.set()
    class Process:
        pid, returncode = 765431, None
        def __init__(self): self.stdin, self.stdout, self.stderr = BlockedPipe(), Pipe(), Pipe()
        def wait(self, timeout):
            assert timeout == ssh_transport.FORCE_REAP_TIMEOUT
            self.returncode = -signal.SIGKILL
            reaped.set()
            return self.returncode
    process = Process()
    def copy(selected_bytes):
        temporary, prefix, hosts = original_copies(selected_bytes)
        copies.append((Path(prefix[1]), hosts))
        return temporary, prefix, hosts
    async def source(value, require_current):
        handle, identity = await original_source(value, require_current)
        sources.append(handle)
        return handle, identity
    async def pin(self, endpoint, expected, deadline):
        assert endpoint == ENDPOINT and expected == hashlib.sha256(RAW_KEY).digest()
        assert self._known_hosts_paths[0].read_bytes() == HOSTS
        return HOSTS
    async def read(stream, *, stdout):
        await asyncio.Future()
    monkeypatch.setattr(profiles, "_copies", copy)
    monkeypatch.setattr(ssh_transport, "_observe_transfer_source", source)
    monkeypatch.setattr(SshTransport, "_pin", pin)
    monkeypatch.setattr(ssh_transport, "_read", read)
    monkeypatch.setattr(ssh_transport.subprocess, "Popen", lambda *a, **k: process)
    monkeypatch.setattr(ssh_transport.os, "killpg", lambda pid, sig: signals.append((pid, sig)))
    payload = {"request": "same owned fixture"}
    operation = (profiles.transfer_file(ENDPOINT, "pass", payload, selected,
        expected_fingerprint=FINGERPRINT) if kind == "binary" else
        profiles.request(ENDPOINT, "pass", payload, expected_fingerprint=FINGERPRINT))
    task = asyncio.create_task(operation)
    try:
        assert await asyncio.to_thread(entered.wait, 30)
        task.cancel()
        assert await asyncio.to_thread(reaped.wait, 1)
        assert await asyncio.to_thread(pipe_closed.wait, 1)
        assert signals == [(process.pid, signal.SIGKILL)]
        assert not task.done() and not worker_finished.is_set()
        if kind == "binary":
            assert sources and all(not handle.closed for handle in sources)
        else:
            assert sources == []
        assert copies and all(path.exists() for pair in copies for path in pair)
        release.set()
        with pytest.raises(asyncio.CancelledError): await asyncio.wait_for(task, 2)
        assert worker_finished.is_set()
        if kind == "binary":
            assert all(handle.closed for handle in sources)
        else:
            assert sources == []
        assert all(not path.exists() for pair in copies for path in pair)
        assert all(pipe.closed for pipe in (process.stdin, process.stdout, process.stderr))
        assert all(path.exists() for path in originals)
    finally:
        release.set()
        if not task.done():
            task.cancel()
        await asyncio.wait_for(asyncio.gather(task, return_exceptions=True), 2)


@pytest.mark.parametrize("cancel", [False, True])
async def test_profile_transfer_retains_private_copies_and_original_deadline_until_child_returns(tmp_path, monkeypatch, cancel):
    profiles, originals = profile(tmp_path)
    selected, captured = artifact(tmp_path), []
    entered = asyncio.Event()
    def current(): return True
    deadline = asyncio.get_running_loop().time() + 10
    async def transfer(self, endpoint, program, payload, value, timeout, **kwargs):
        identity = Path(self._argv_prefix[1])
        hosts, = self._known_hosts_paths
        assert identity.read_bytes() == PRIVATE_KEY and hosts.read_bytes() == HOSTS
        assert identity.stat().st_mode & 0o777 == hosts.stat().st_mode & 0o777 == 0o600
        assert not any(arg.startswith("UserKnownHostsFile=") for arg in self._argv_prefix)
        assert endpoint == ENDPOINT and value is selected and kwargs["deadline"] == deadline
        assert kwargs["expected_fingerprint"] == FINGERPRINT and kwargs["require_current"] is current
        assert 0 < timeout <= 10
        captured.append((identity, hosts))
        entered.set()
        if cancel:
            try: await asyncio.Future()
            finally: assert identity.exists() and hosts.exists()
        return {"stored": True}
    monkeypatch.setattr(SshTransport, "transfer_file", transfer)
    task = asyncio.create_task(profiles.transfer_file(ENDPOINT, "pass", {}, selected,
        expected_fingerprint=FINGERPRINT, deadline=deadline, require_current=current))
    await asyncio.wait_for(entered.wait(), 30)
    if cancel:
        task.cancel()
        with pytest.raises(asyncio.CancelledError): await task
    else: assert await task == {"stored": True}
    assert all(not path.exists() for pair in captured for path in pair)
    assert all(path.exists() for path in originals)


@pytest.mark.parametrize("index", [0, 1, 2])
async def test_changed_selected_profile_has_no_default_transfer_fallback(tmp_path, monkeypatch, index):
    profiles, originals = profile(tmp_path)
    private(originals[index], originals[index].read_bytes() + b"changed")
    async def forbidden(*a, **k): pytest.fail("Changed profile reached SSH transfer.")
    monkeypatch.setattr(SshTransport, "transfer_file", forbidden)
    with pytest.raises(SshTransportError) as failed:
        await profiles.transfer_file(ENDPOINT, "pass", {}, artifact(tmp_path), expected_fingerprint=FINGERPRINT)
    assert failed.value.code == "configuration_unavailable"
    assert str(originals[index]) not in str(failed.value) and PRIVATE_KEY.decode().strip() not in str(failed.value)


def iap(tmp_path):
    root = tmp_path.resolve() / "private"
    root.mkdir(mode=0o700)
    identity, calls = private(root / "identity", PRIVATE_KEY), []
    async def verify(endpoint):
        calls.append(("verify", dict(endpoint)))
        return dict(endpoint)
    async def keys(endpoint):
        calls.append(("keys", dict(endpoint)))
        return dict(GCP_KEY)
    async def key_path(endpoint):
        calls.append(("identity", dict(endpoint)))
        return identity
    return GcpIapTransport(verify, keys, key_path, gcloud_executable="/fixture path/gcloud",
        cli_home="/fixture home", cli_config_directory="/fixture config"), calls, identity


async def test_iap_transfer_reuses_checked_endpoint_private_pin_and_one_absolute_budget(tmp_path, monkeypatch):
    transport, calls, identity = iap(tmp_path)
    selected, observed, elapsed = artifact(tmp_path), [], [0]
    loop, current = asyncio.get_running_loop(), lambda: True
    clock = loop.time
    deadline = clock() + 10
    original_verify = transport.verify
    async def verify(endpoint):
        elapsed[0] = 4
        return await original_verify(endpoint)
    transport.verify = verify
    async def transfer(self, endpoint, program, payload, value, timeout, **kwargs):
        pin, = self._known_hosts_paths
        expected_pin, expected_fingerprint = checked_host_key(GCP_KEY, GCP_ENDPOINT["host"])
        assert pin.read_bytes() == expected_pin and pin.stat().st_mode & 0o777 == 0o600
        assert endpoint == {name: GCP_ENDPOINT[name] for name in ("host", "user", "ssh_port")}
        assert value is selected and kwargs["deadline"] == deadline and kwargs["require_current"] is current
        assert kwargs["expected_fingerprint"] == expected_fingerprint and 0 < timeout <= 6
        assert "IdentityFile=" + str(identity) in self._argv_prefix
        proxy = next(arg.split("=", 1)[1] for arg in self._argv_prefix if arg.startswith("ProxyCommand="))
        parts = shlex.split(proxy)
        assert "start-iap-tunnel" in parts and "--listen-on-stdin" in parts
        assert "--account=fixture@example.test" in parts and "--project=fixture-project" in parts
        observed.append(pin)
        return {"stored": True}
    monkeypatch.setattr(SshTransport, "transfer_file", transfer)
    with monkeypatch.context() as patch:
        patch.setattr(loop, "time", lambda: clock() + elapsed[0])
        assert await transport.transfer_file(GCP_ENDPOINT, "pass", {}, selected, 20,
            expected_fingerprint=FINGERPRINT, deadline=deadline, require_current=current) == {"stored": True}
    assert [name for name, _ in calls] == ["verify", "keys", "identity", "verify"]
    assert all(endpoint == GCP_ENDPOINT for _, endpoint in calls)
    assert observed and all(not path.exists() for path in observed) and identity.exists()


@pytest.mark.parametrize("stage", ["first", "second", "key"])
async def test_iap_changed_vm_or_guest_key_never_reaches_transfer(tmp_path, monkeypatch, stage):
    transport, calls, _ = iap(tmp_path)
    original_verify = transport.verify
    async def verify(endpoint):
        result = await original_verify(endpoint)
        count = sum(name == "verify" for name, _ in calls)
        return {**result, "instance_id": "987"} if count == (1 if stage == "first" else 2) else result
    if stage != "key": transport.verify = verify
    async def forbidden(*a, **k): pytest.fail("Changed IAP identity reached artifact transfer.")
    monkeypatch.setattr(SshTransport, "transfer_file", forbidden)
    with pytest.raises(SshTransportError) as failed:
        await transport.transfer_file(GCP_ENDPOINT, "pass", {}, artifact(tmp_path),
            expected_fingerprint="SHA256:" + "A" * 43 if stage == "key" else FINGERPRINT)
    assert failed.value.code == ("host_key_unavailable" if stage == "key" else "connection_unavailable")


async def test_iap_cancelled_observation_joins_callback_cleanup_before_return(tmp_path, monkeypatch):
    transport, calls, _ = iap(tmp_path)
    entered, cleaned = asyncio.Event(), asyncio.Event()
    async def verify(endpoint):
        entered.set()
        try: await asyncio.Future()
        finally:
            await asyncio.sleep(0)
            cleaned.set()
    transport.verify = verify
    async def forbidden(*a, **k): pytest.fail("Cancelled IAP observation created transfer.")
    monkeypatch.setattr(SshTransport, "transfer_file", forbidden)
    task = asyncio.create_task(transport.transfer_file(GCP_ENDPOINT, "pass", {}, artifact(tmp_path),
        expected_fingerprint=FINGERPRINT))
    await asyncio.wait_for(entered.wait(), 30)
    task.cancel()
    with pytest.raises(asyncio.CancelledError): await asyncio.wait_for(task, 1)
    assert cleaned.is_set() and calls == []
