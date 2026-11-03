"""Offline IAP routing fixtures, with no cloud, key generation or real SSH."""
import asyncio
import base64
import hashlib
import json
from pathlib import Path
import shlex

import pytest

from launchkit import gcp_transport, ssh_transport, ssh_browser_access
from launchkit.gcp_transport import GcpIapTransport, checked_host_key, validate_gcp_endpoint
from launchkit.ssh_transport import SshTransportError, FRAME_PREFIX
from launchkit.ssh_browser_access import BrowserAccessError


ENDPOINT = {"transport": "gcp-iap", "operation_id": "11111111-1111-4111-8111-111111111111",
    "account": "fixture@example.test", "project": "fixture-project", "zone": "europe-west1-b",
    "instance": "fixture-app", "instance_id": "1234567890123456789",
    "ownership_token": "11111111111141118111111111111111", "host": "compute.1234567890123456789",
    "user": "mobkit", "ssh_port": 22}
RAW_KEY = b"\x00\x00\x00\x0bssh-ed25519\x00\x00\x00\x20" + bytes(range(32))
KEY = {"key_type": "ssh-ed25519", "key": base64.b64encode(RAW_KEY).decode()}
PIN, FINGERPRINT = checked_host_key(KEY, ENDPOINT["host"])


def fixture(tmp_path, *, verify=None, keys=None, identity=None):
    private = tmp_path / "private"
    private.mkdir(mode=0o700)
    path = private / "identity"
    path.write_bytes(b"offline fixture only")
    path.chmod(0o600)
    calls = []
    async def checked(endpoint):
        calls.append(("verify", dict(endpoint)))
        return dict(endpoint)
    async def host_keys(endpoint):
        calls.append(("keys", dict(endpoint)))
        return dict(KEY)
    async def key_path(endpoint):
        calls.append(("identity", dict(endpoint)))
        return path
    transport = GcpIapTransport(verify or checked, keys or host_keys, identity or key_path,
        gcloud_executable="/fixture path/gcloud", cli_home="/fixture home", cli_config_directory="/fixture config")
    return transport, calls, path


def inert_execute(monkeypatch):
    records = []
    async def execute(argv, body, timeout):
        records.append((list(argv), body, timeout))
        if argv[0] == "/usr/bin/ssh-keygen":
            pin = Path(argv[-1])
            assert pin.stat().st_mode & 0o777 == 0o600
            assert pin.read_bytes() == PIN
            return PIN, b"", 0
        diagnostic = (f"debug1: Server host key: ssh-ed25519 {FINGERPRINT}\n"
            f"debug1: Host '{ENDPOINT['host']}' is known and matches the ED25519 host key.\n").encode()
        return (FRAME_PREFIX + '{"ready":true}\n').encode(), diagnostic, 0
    monkeypatch.setattr(ssh_transport, "_execute", execute)
    monkeypatch.setattr(ssh_transport.subprocess, "Popen", lambda *a, **k: pytest.fail("Real process forbidden."))
    return records


def test_actual_rpc_builder_pins_first_request_and_keeps_secrets_in_stdin(tmp_path, monkeypatch):
    transport, calls, identity = fixture(tmp_path)
    records = inert_execute(monkeypatch)
    payload = {"action": "install", "settings": {"OPENAI_API_KEY": "offline-secret-value"}}
    result = asyncio.run(transport.request(dict(ENDPOINT), "print('host-owned RPC')", payload))
    assert result == {"ready": True, "_transport": {"host_key_fingerprint": FINGERPRINT}}
    assert [item[0] for item in calls] == ["verify", "keys", "identity", "verify"]
    assert all(item[1] == ENDPOINT for item in calls)
    argv, body, _ = records[-1]
    assert argv[0] == "/usr/bin/ssh"
    assert json.loads(body) == payload
    assert "offline-secret-value" not in " ".join(argv)
    assert argv.count("StrictHostKeyChecking=yes") == 1
    assert argv.count("GlobalKnownHostsFile=/dev/null") == 1
    assert sum(arg.startswith("UserKnownHostsFile=") for arg in argv) == 1
    assert "IdentityFile=" + str(identity) in argv
    proxy = next(arg.removeprefix("ProxyCommand=") for arg in argv if arg.startswith("ProxyCommand="))
    parts = shlex.split(proxy)
    assert parts[:2] == ["/usr/bin/env", "-i"]
    assert "HOME=/fixture home" in parts and "CLOUDSDK_CONFIG=/fixture config" in parts
    assert "/fixture path/gcloud" in parts and "start-iap-tunnel" in parts
    assert "--account=fixture@example.test" in parts and "--project=fixture-project" in parts
    assert "--zone=europe-west1-b" in parts and "--listen-on-stdin" in parts
    assert not any("StrictHostKeyChecking=no" in arg or "UserKnownHostsFile=" in arg for arg in parts)
    assert argv[-3:] == ["--", ENDPOINT["host"], "python3 -I -c " + shlex.quote("print('host-owned RPC')")]
    assert all(not Path(record[0][-1]).exists() for record in records if record[0][0] == "/usr/bin/ssh-keygen")


@pytest.mark.parametrize("change", [
    {"host": "other.test"}, {"user": "root"}, {"ssh_port": True}, {"ssh_port": 23},
    {"instance_id": "0123"}, {"instance_id": str(2**64)}, {"operation_id": "ABC"},
    {"ownership_token": "a" * 33}, {"account": "fixture\n@example.test"},
    {"project": "fixture;touch"}, {"zone": "-other"}, {"instance": "$(bad)"},
    {"ProxyCommand": "unowned"}, {"identity_file": "/private/key"},
])
def test_endpoint_rejects_unowned_or_malformed_transport_fields(change):
    with pytest.raises(SshTransportError) as failed:
        validate_gcp_endpoint({**ENDPOINT, **change})
    assert failed.value.code == "invalid_endpoint"


@pytest.mark.parametrize("key", [None, {}, [], {**KEY, "comment": "extra"},
    {"key_type": "ssh-ed25519-cert-v01@openssh.com", "key": KEY["key"]},
    {**KEY, "key": KEY["key"] + "\n"}, {**KEY, "key": KEY["key"][:-1]},
    {**KEY, "key": base64.b64encode(b"x" * 51).decode()},
    {**KEY, "key": base64.b64encode(RAW_KEY + b"x").decode()},
    {**KEY, "key": "@revoked " + KEY["key"]}, {"ssh-ed25519": KEY["key"]}])
def test_one_exact_guest_key_rejects_ambiguous_noncanonical_or_foreign_types(key):
    with pytest.raises(SshTransportError) as failed:
        checked_host_key(key, ENDPOINT["host"])
    assert failed.value.code == "host_key_unavailable"


@pytest.mark.parametrize("stage", ["first", "second"])
def test_vm_or_account_identity_change_blocks_before_ssh(tmp_path, monkeypatch, stage):
    seen = []
    async def verify(endpoint):
        seen.append(endpoint)
        return {**endpoint, "instance_id": "987"} if len(seen) == (1 if stage == "first" else 2) else endpoint
    transport, _, _ = fixture(tmp_path, verify=verify)
    records = inert_execute(monkeypatch)
    with pytest.raises(SshTransportError) as failed:
        asyncio.run(transport.request(ENDPOINT, "pass", {}))
    assert failed.value.code == "connection_unavailable"
    assert records == []


def test_changed_guest_key_blocks_rpc_and_browser_before_identity_or_ssh(tmp_path, monkeypatch):
    transport, calls, _ = fixture(tmp_path)
    records = inert_execute(monkeypatch)
    wrong = "SHA256:" + "A" * 43
    with pytest.raises(SshTransportError) as failed:
        asyncio.run(transport.request(ENDPOINT, "pass", {}, expected_fingerprint=wrong))
    assert failed.value.code == "host_key_unavailable"
    with pytest.raises(BrowserAccessError) as browser:
        asyncio.run(transport.open(ENDPOINT, expected_fingerprint=wrong, port=4110))
    assert browser.value.code == "host-key-unavailable"
    assert not any(item[0] == "identity" for item in calls) and records == []


@pytest.mark.parametrize("mode", [0o644, 0o660, 0o777])
def test_nonprivate_identity_is_refused_without_reading_its_content(tmp_path, monkeypatch, mode):
    transport, _, identity = fixture(tmp_path)
    identity.chmod(mode)
    records = inert_execute(monkeypatch)
    with pytest.raises(SshTransportError):
        asyncio.run(transport.request(ENDPOINT, "pass", {}))
    assert records == []


def test_identity_link_is_refused(tmp_path, monkeypatch):
    transport, _, identity = fixture(tmp_path)
    linked = identity.parent / "link"
    linked.symlink_to(identity)
    transport.identity_file = linked
    records = inert_execute(monkeypatch)
    with pytest.raises(SshTransportError):
        asyncio.run(transport.request(ENDPOINT, "pass", {}))
    assert records == []


def test_exact_account_percent_is_escaped_for_openssh_token_expansion(tmp_path):
    transport, _, identity = fixture(tmp_path)
    proxy = transport._prefix({**ENDPOINT, "account": "fixture%name@example.test"}, str(identity))[1]
    assert "fixture%%name@example.test" in proxy


def test_actual_browser_builder_reuses_guardian_and_same_pin(tmp_path, monkeypatch):
    transport, calls, _ = fixture(tmp_path)
    records = inert_execute(monkeypatch)
    guardian_calls, handles = [], []
    def popen(argv, **kwargs):
        guardian_calls.append((argv, kwargs))
        return object()
    class Handle:
        def __init__(self, process):
            self.process, self.closed = process, False
            handles.append(self)
        async def _send(self, body):
            self.body = json.loads(body)
        async def _frame(self):
            diagnostics = (f"debug1: Server host key: ssh-ed25519 {FINGERPRINT}\n"
                f"debug1: Host '{ENDPOINT['host']}' is known and matches the ED25519 host key.\n").encode()
            return {"event": "ready", "diagnostics": base64.b64encode(diagnostics).decode()}
        async def _close(self):
            self.closed = True
            return True
        async def close(self):
            return await self._close()
    monkeypatch.setattr(ssh_browser_access.subprocess, "Popen", popen)
    monkeypatch.setattr(ssh_browser_access, "_Handle", Handle)
    handle = asyncio.run(transport.open(ENDPOINT, expected_fingerprint=FINGERPRINT, port=4110))
    assert handle is handles[0] and not handle.closed
    assert guardian_calls[0][0][-1] == ssh_browser_access.GUARDIAN
    assert guardian_calls[0][1]["start_new_session"] is True
    assert [item[0] for item in calls] == ["verify", "keys", "identity", "verify"]
    assert base64.b64decode(handle.body["pin"]) == PIN
    argv = handle.body["argv"]
    assert argv.count("UserKnownHostsFile=@PIN@") == 1
    assert argv.count("StrictHostKeyChecking=yes") == 1
    assert "127.0.0.1:4110:127.0.0.1:4110" in argv
    assert any(item.startswith("ProxyCommand=") for item in argv)
    assert asyncio.run(handle.close()) is True and handle.closed
    assert len(records) == 1 and records[0][0][0] == "/usr/bin/ssh-keygen"


@pytest.mark.parametrize("cleanup", [True, False])
def test_post_open_local_cleanup_failure_closes_or_retains_exact_guardian(tmp_path, monkeypatch, cleanup):
    transport, _, _ = fixture(tmp_path)
    class Handle:
        closed = False
        async def _close(self):
            self.closed = True
            return cleanup
    handle = Handle()
    class Browser:
        def __init__(self, **kwargs):
            pass
        async def open(self, *args, **kwargs):
            return handle
    original = gcp_transport.tempfile.TemporaryDirectory
    class Temporary:
        def __enter__(self):
            self.temporary = original(prefix="offline-iap-test-")
            return self.temporary.__enter__()
        def __exit__(self, *args):
            self.temporary.__exit__(*args)
            raise OSError("offline cleanup fixture")
        def __init__(self, **kwargs):
            pass
    monkeypatch.setattr(gcp_transport, "SshBrowserAccessTransport", Browser)
    monkeypatch.setattr(gcp_transport.tempfile, "TemporaryDirectory", Temporary)
    with pytest.raises(BrowserAccessError) as failed:
        asyncio.run(transport.open(ENDPOINT, expected_fingerprint=FINGERPRINT, port=4110))
    assert handle.closed
    assert failed.value.code == ("connection-unavailable" if cleanup else "cleanup-unavailable")
    assert (getattr(failed.value, "handle", None) is handle) is (not cleanup)


@pytest.mark.parametrize("timeout", [True, 0, -1, float("inf"), float("nan"), 10**400])
def test_invalid_timeout_performs_no_cloud_observation(tmp_path, timeout):
    transport, calls, _ = fixture(tmp_path)
    with pytest.raises(SshTransportError):
        asyncio.run(transport.request(ENDPOINT, "pass", {}, timeout))
    with pytest.raises(BrowserAccessError):
        asyncio.run(transport.open(ENDPOINT, expected_fingerprint=FINGERPRINT, port=4110, timeout=timeout))
    assert calls == []


@pytest.mark.parametrize("fingerprint", ["", "bad", {}, "SHA256:" + "-" * 43])
def test_invalid_pin_performs_no_cloud_observation(tmp_path, fingerprint):
    transport, calls, _ = fixture(tmp_path)
    with pytest.raises(SshTransportError) as failed:
        asyncio.run(transport.request(ENDPOINT, "pass", {}, expected_fingerprint=fingerprint))
    assert failed.value.code == "invalid_request"
    with pytest.raises(BrowserAccessError) as failed:
        asyncio.run(transport.open(ENDPOINT, expected_fingerprint=fingerprint, port=4110))
    assert failed.value.code == "invalid-request" and calls == []


def operation(transport, kind, timeout):
    if kind == "rpc":
        return transport.request(ENDPOINT, "pass", {}, timeout, expected_fingerprint=FINGERPRINT)
    return transport.open(ENDPOINT, expected_fingerprint=FINGERPRINT, port=4110, timeout=timeout)


@pytest.mark.parametrize("kind", ["rpc", "browser"])
@pytest.mark.parametrize("stage", ["first_verify", "keys", "identity", "second_verify"])
def test_cloud_preparation_is_in_operation_budget_and_finishes_cancellation_cleanup(
        tmp_path, monkeypatch, kind, stage):
    transport, calls, _ = fixture(tmp_path)
    records = inert_execute(monkeypatch)
    finished, seen = [], []
    original_verify = transport.verify
    original_keys, original_identity = transport.host_keys, transport.identity_file

    async def observe(callback, endpoint, name):
        seen.append(name)
        if name == stage:
            try:
                await asyncio.Future()
            finally:
                # Production cloud callbacks own their command cleanup. The
                # deadline must wait for it rather than abandoning the task.
                await asyncio.sleep(.01)
                finished.append(name)
        return await callback(endpoint)

    async def verify(endpoint):
        name = "second_verify" if "first_verify" in seen else "first_verify"
        return await observe(original_verify, endpoint, name)

    async def keys(endpoint):
        return await observe(original_keys, endpoint, "keys")

    async def identity(endpoint):
        return await observe(original_identity, endpoint, "identity")

    transport.verify, transport.host_keys, transport.identity_file = verify, keys, identity
    error_type = SshTransportError if kind == "rpc" else BrowserAccessError
    async def run():
        with pytest.raises(error_type) as failed:
            # The outer bound is a harness guard, not the transport deadline.
            await asyncio.wait_for(operation(transport, kind, .03), 1)
        assert failed.value.code == "timeout"
    asyncio.run(run())
    ordered = ["first_verify", "keys", "identity", "second_verify"]
    assert seen == ordered[:ordered.index(stage) + 1]
    assert finished == [stage] and len(calls) == ordered.index(stage)
    assert records == []


@pytest.mark.parametrize("kind", ["rpc", "browser"])
def test_preparation_and_ssh_share_remaining_budget(tmp_path, monkeypatch, kind):
    transport, calls, _ = fixture(tmp_path)
    original = transport.verify
    observed = []
    class Handle:
        pass
    handle = Handle()
    class Ssh:
        def __init__(self, **kwargs):
            pass
        async def request(self, endpoint, program, payload, timeout, **kwargs):
            observed.append(timeout)
            return {"ready": True}
        async def open(self, endpoint, **kwargs):
            observed.append(kwargs["timeout"])
            return handle
    monkeypatch.setattr(gcp_transport, "SshTransport", Ssh)
    monkeypatch.setattr(gcp_transport, "SshBrowserAccessTransport", Ssh)
    async def run():
        loop = asyncio.get_running_loop()
        clock, elapsed = loop.time, [0]
        async def verify(endpoint):
            if not elapsed[0]:
                elapsed[0] = 4
            return await original(endpoint)
        transport.verify = verify
        with monkeypatch.context() as patch:
            patch.setattr(loop, "time", lambda: clock() + elapsed[0])
            result = await operation(transport, kind, 10)
        if kind == "rpc":
            assert result == {"ready": True}
        else:
            assert result is handle
    asyncio.run(run())
    assert [item[0] for item in calls] == ["verify", "keys", "identity", "verify"]
    assert len(observed) == 1 and 0 < observed[0] <= 6


@pytest.mark.parametrize("kind", ["rpc", "browser"])
def test_late_cloud_result_admits_no_further_observation_or_ssh(tmp_path, monkeypatch, kind):
    transport, calls, _ = fixture(tmp_path)
    original = transport.verify
    records = inert_execute(monkeypatch)
    async def run():
        loop = asyncio.get_running_loop()
        clock, elapsed = loop.time, [0]
        async def verify(endpoint):
            result = await original(endpoint)
            elapsed[0] = 11
            return result
        transport.verify = verify
        error_type = SshTransportError if kind == "rpc" else BrowserAccessError
        with monkeypatch.context() as patch:
            patch.setattr(loop, "time", lambda: clock() + elapsed[0])
            with pytest.raises(error_type) as failed:
                await operation(transport, kind, 10)
        assert failed.value.code == "timeout"
    asyncio.run(run())
    assert [item[0] for item in calls] == ["verify"] and records == []


@pytest.mark.parametrize("kind", ["rpc", "browser"])
def test_external_cancellation_during_cloud_check_remains_cancellation(tmp_path, monkeypatch, kind):
    transport, calls, _ = fixture(tmp_path)
    records = inert_execute(monkeypatch)
    async def run():
        entered, cleaned = asyncio.Event(), asyncio.Event()
        async def verify(endpoint):
            entered.set()
            try:
                await asyncio.Future()
            finally:
                await asyncio.sleep(.01)
                cleaned.set()
        transport.verify = verify
        task = asyncio.create_task(operation(transport, kind, 10))
        await asyncio.wait_for(entered.wait(), 30)
        task.cancel()
        with pytest.raises(asyncio.CancelledError):
            await asyncio.wait_for(task, 1)
        assert cleaned.is_set()
    asyncio.run(run())
    assert calls == [] and records == []


@pytest.mark.parametrize("cleanup", [True, False])
def test_late_browser_result_closes_or_retains_exact_guardian(tmp_path, monkeypatch, cleanup):
    transport, _, _ = fixture(tmp_path)
    class Handle:
        closed = False
        async def _close(self):
            self.closed = True
            return cleanup
    handle = Handle()
    async def run():
        loop = asyncio.get_running_loop()
        clock, elapsed = loop.time, [0]
        class Browser:
            def __init__(self, **kwargs):
                pass
            async def open(self, *args, **kwargs):
                elapsed[0] = 11
                return handle
        with monkeypatch.context() as patch:
            patch.setattr(gcp_transport, "SshBrowserAccessTransport", Browser)
            patch.setattr(loop, "time", lambda: clock() + elapsed[0])
            with pytest.raises(BrowserAccessError) as failed:
                await operation(transport, "browser", 10)
        assert failed.value.code == ("timeout" if cleanup else "cleanup-unavailable")
        assert (getattr(failed.value, "handle", None) is handle) is (not cleanup)
    asyncio.run(run())
    assert handle.closed


def test_late_rpc_result_is_not_reported_as_current_success(tmp_path, monkeypatch):
    transport, _, _ = fixture(tmp_path)
    async def run():
        loop = asyncio.get_running_loop()
        clock, elapsed = loop.time, [0]
        class Ssh:
            def __init__(self, **kwargs):
                pass
            async def request(self, *args, **kwargs):
                elapsed[0] = 11
                return {"ready": True}
        with monkeypatch.context() as patch:
            patch.setattr(gcp_transport, "SshTransport", Ssh)
            patch.setattr(loop, "time", lambda: clock() + elapsed[0])
            with pytest.raises(SshTransportError) as failed:
                await operation(transport, "rpc", 10)
        assert failed.value.code == "timeout"
    asyncio.run(run())


@pytest.mark.parametrize("kind", ["rpc", "browser"])
def test_actual_iap_transports_do_not_admit_connection_after_request_encoding_exhausts_budget(
        tmp_path, monkeypatch, kind):
    transport, _, _ = fixture(tmp_path)
    records = inert_execute(monkeypatch)
    original_verify, original_dumps = transport.verify, json.dumps
    payload = {"fixture": "ordinary request"}
    async def run():
        loop = asyncio.get_running_loop()
        clock, elapsed = loop.time, [0]
        async def verify(endpoint):
            if not elapsed[0]:
                elapsed[0] = 4
            return await original_verify(endpoint)
        def dumps(value, *args, **kwargs):
            result = original_dumps(value, *args, **kwargs)
            if value is payload or isinstance(value, dict) and set(value) == {"argv", "pin", "port", "timeout", "deadline"}:
                # Simulate expensive encoding without allocating a huge payload.
                elapsed[0] += 7
            return result
        transport.verify = verify
        with monkeypatch.context() as patch:
            patch.setattr(loop, "time", lambda: clock() + elapsed[0])
            patch.setattr(json, "dumps", dumps)
            error_type = SshTransportError if kind == "rpc" else BrowserAccessError
            with pytest.raises(error_type) as failed:
                if kind == "rpc":
                    await transport.request(ENDPOINT, "pass", payload, 10, expected_fingerprint=FINGERPRINT)
                else:
                    await operation(transport, kind, 10)
        assert failed.value.code == "timeout"
    asyncio.run(run())
    # Browser pin lookup is allowed while time remains; no guardian or SSH
    # connection is created after encoding consumes the original budget.
    assert all(record[0][0] == "/usr/bin/ssh-keygen" for record in records)
    assert len(records) == (0 if kind == "rpc" else 1)


@pytest.mark.parametrize("kind", ["rpc", "browser"])
@pytest.mark.parametrize("caller_seconds,encoding_seconds", [(2, 3), (100, 11)])
def test_child_absolute_deadline_can_only_shorten_budget_and_covers_encoding(
        tmp_path, monkeypatch, kind, caller_seconds, encoding_seconds):
    records = inert_execute(monkeypatch)
    original_dumps = json.dumps
    payload = {"fixture": "child request"}
    pin = tmp_path / "known_hosts"
    pin.write_bytes(PIN)
    pin.chmod(0o600)
    async def run():
        loop = asyncio.get_running_loop()
        clock, elapsed = loop.time, [0]
        def dumps(value, *args, **kwargs):
            result = original_dumps(value, *args, **kwargs)
            if value is payload or isinstance(value, dict) and set(value) == {"argv", "pin", "port", "timeout", "deadline"}:
                elapsed[0] += encoding_seconds
            return result
        deadline = clock() + caller_seconds
        base = {name: ENDPOINT[name] for name in ("host", "user", "ssh_port")}
        with monkeypatch.context() as patch:
            patch.setattr(loop, "time", lambda: clock() + elapsed[0])
            patch.setattr(json, "dumps", dumps)
            error_type = SshTransportError if kind == "rpc" else BrowserAccessError
            with pytest.raises(error_type) as failed:
                if kind == "rpc":
                    await ssh_transport.SshTransport(known_hosts_paths=(pin,)).request(base, "pass", payload,
                        10, expected_fingerprint=FINGERPRINT, deadline=deadline)
                else:
                    await ssh_browser_access.SshBrowserAccessTransport(known_hosts_paths=(pin,)).open(base,
                        expected_fingerprint=FINGERPRINT, port=4110, timeout=10, deadline=deadline)
        assert failed.value.code == "timeout"
    asyncio.run(run())
    assert all(record[0][0] == "/usr/bin/ssh-keygen" for record in records)
    assert len(records) == (0 if kind == "rpc" else 1)


@pytest.mark.parametrize("kind", ["rpc", "browser"])
@pytest.mark.parametrize("deadline", [True, "later", {}, float("inf"), float("nan"), 10**400])
def test_invalid_child_deadline_never_looks_up_pin_or_starts_process(tmp_path, monkeypatch, kind, deadline):
    records = inert_execute(monkeypatch)
    base = {name: ENDPOINT[name] for name in ("host", "user", "ssh_port")}
    error_type = SshTransportError if kind == "rpc" else BrowserAccessError
    with pytest.raises(error_type) as failed:
        if kind == "rpc":
            asyncio.run(ssh_transport.SshTransport().request(base, "pass", {}, deadline=deadline))
        else:
            asyncio.run(ssh_browser_access.SshBrowserAccessTransport().open(base,
                expected_fingerprint=FINGERPRINT, port=4110, deadline=deadline))
    assert failed.value.code == ("invalid_request" if kind == "rpc" else "invalid-request")
    assert records == []


@pytest.mark.parametrize("kind", ["rpc", "browser"])
def test_expired_child_deadline_never_constructs_pin_lookup(tmp_path, monkeypatch, kind):
    records = inert_execute(monkeypatch)
    base = {name: ENDPOINT[name] for name in ("host", "user", "ssh_port")}
    def forbidden(*args, **kwargs):
        pytest.fail("Expired request constructed a pin lookup.")
    monkeypatch.setattr(ssh_transport.SshTransport, "_pin", forbidden)
    error_type = SshTransportError if kind == "rpc" else BrowserAccessError
    with pytest.raises(error_type) as failed:
        if kind == "rpc":
            asyncio.run(ssh_transport.SshTransport().request(base, "pass", {},
                expected_fingerprint=FINGERPRINT, deadline=0))
        else:
            asyncio.run(ssh_browser_access.SshBrowserAccessTransport().open(base,
                expected_fingerprint=FINGERPRINT, port=4110, deadline=0))
    assert failed.value.code == "timeout" and records == []


@pytest.mark.parametrize("stage", ["spawn", "send", "frame"])
@pytest.mark.parametrize("cleanup", [True, False])
def test_guardian_startup_cannot_send_or_report_ready_after_budget_expiry(monkeypatch, stage, cleanup):
    base = {name: ENDPOINT[name] for name in ("host", "user", "ssh_port")}
    observed = []
    diagnostics = (f"debug1: Server host key: ssh-ed25519 {FINGERPRINT}\n"
        f"debug1: Host '{ENDPOINT['host']}' is known and matches the ED25519 host key.\n").encode()
    async def run():
        loop = asyncio.get_running_loop()
        clock, elapsed = loop.time, [0]
        transport = ssh_browser_access.SshBrowserAccessTransport()
        async def pin(*args):
            return PIN
        class Handle:
            def __init__(self, process):
                assert process is owned_process
            async def _send(self, body):
                observed.append("send")
                if stage == "send":
                    elapsed[0] = 11
            async def _frame(self):
                observed.append("frame")
                if stage == "frame":
                    elapsed[0] = 11
                return {"event": "ready", "diagnostics": base64.b64encode(diagnostics).decode()}
            async def _close(self):
                observed.append("close")
                return cleanup
        owned_process = object()
        handles = []
        def handle(process):
            value = Handle(process)
            handles.append(value)
            return value
        def spawn(*args, **kwargs):
            if stage == "spawn":
                elapsed[0] = 11
            return owned_process
        with monkeypatch.context() as patch:
            patch.setattr(loop, "time", lambda: clock() + elapsed[0])
            patch.setattr(transport._ssh, "_pin", pin)
            patch.setattr(ssh_browser_access.subprocess, "Popen", spawn)
            patch.setattr(ssh_browser_access, "_Handle", handle)
            with pytest.raises(BrowserAccessError) as failed:
                await transport.open(base, expected_fingerprint=FINGERPRINT, port=4110, timeout=10)
        assert failed.value.code == ("timeout" if cleanup else "cleanup-unavailable")
        assert (getattr(failed.value, "handle", None) is handles[0]) is (not cleanup)
    asyncio.run(run())
    assert observed == ({"spawn": ["close"], "send": ["send", "close"],
        "frame": ["send", "frame", "close"]}[stage])
