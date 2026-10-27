"""Isolated local process fixtures, never a real SSH host or user key lookup."""
import asyncio
import base64
import hashlib
import hmac
import json
import os
from pathlib import Path
import shlex
import signal
import subprocess
import sys
import time

import pytest

from launchkit import ssh_transport
from launchkit.ssh_transport import FRAME_PREFIX, SshTransport, SshTransportError, validate_endpoint


ENDPOINT = {"host": "fixture.example", "user": "fixture", "ssh_port": 2222}
FINGERPRINT = "SHA256:" + base64.b64encode(bytes(range(32))).decode().rstrip("=")
VERIFIED = (f"debug1: Server host key: ssh-ed25519 {FINGERPRINT}\n"
            "debug1: Host '[fixture.example]:2222' is known and matches the ED25519 host key.\n")


def fixture_transport(tmp_path, script):
    path = tmp_path / "fake-ssh.py"
    path.write_text(script)
    return SshTransport(executable=sys.executable, argv_prefix=(str(path),))


def reply_script(value, stderr="", exit_code=0):
    return ("import sys\n"
            "sys.stdin.buffer.read()\n"
            f"sys.stderr.write({stderr!r})\n"
            f"sys.stdout.buffer.write({value!r}); sys.stdout.buffer.flush()\n"
            f"raise SystemExit({exit_code})\n")


def frame(value):
    return (FRAME_PREFIX + json.dumps(value, separators=(",", ":")) + "\n").encode()


async def wait_record(path, timeout=30):
    async def ready():
        while not path.exists():
            await asyncio.sleep(.01)
        return json.loads(path.read_text())
    return await asyncio.wait_for(ready(), timeout)


def alive(pid):
    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        return False
    return True


async def assert_absent(pid):
    async def absent():
        while alive(pid):
            await asyncio.sleep(.01)
    await asyncio.wait_for(absent(), 3)


@pytest.mark.parametrize("host", ["localhost", "host.example", "host.example.", "work_alias-1", "127.0.0.1", "::1", "fe80::1%lo0"])
def test_endpoint_accepts_only_normalized_fixed_fields(host):
    given = {"host": host, "user": "dev.user-1"}
    assert validate_endpoint(given) == {"host": host, "user": "dev.user-1", "ssh_port": 22}
    result = validate_endpoint(given)
    result["host"] = "other.example"
    assert given["host"] == host


@pytest.mark.parametrize("patch", [
    {"host": "-oProxyCommand=bad"}, {"host": "host;echo bad"}, {"host": "$(bad)"},
    {"host": "user@host"}, {"host": "host\n-o bad"}, {"host": "host\0bad"},
    {"host": "host name"}, {"host": "h\u00f6st"}, {"host": ".."}, {"host": "host/path"},
    {"host": "[::1]"}, {"host": "fe80::1%lo;bad"}, {"host": "127.0.0.1%lo0"},
    {"user": "-option"}, {"user": "bad user"}, {"user": "$(bad)"}, {"user": "user\n"},
    {"ssh_port": True}, {"ssh_port": "22"}, {"ssh_port": 0}, {"ssh_port": 65536},
    {"executable": "/bin/sh"},
])
async def test_invalid_endpoint_never_spawns(monkeypatch, patch):
    def forbidden(*args, **kwargs):
        raise AssertionError("Invalid destination created a process.")
    monkeypatch.setattr(ssh_transport.subprocess, "Popen", forbidden)
    with pytest.raises(SshTransportError) as failure:
        await SshTransport().request({**ENDPOINT, **patch}, "pass", {})
    assert failure.value.code == "invalid_endpoint"
    assert "Remote observation is unavailable" in str(failure.value)


@pytest.mark.parametrize("endpoint", [None, [], {}, {"host": "host"}, {"user": "user"}])
def test_missing_or_nonmapping_endpoint_rejected(endpoint):
    with pytest.raises(SshTransportError, match="destination is invalid"):
        validate_endpoint(endpoint)


@pytest.mark.parametrize("program,payload,timeout", [
    ("", {}, 30), ("bad\0program", {}, 30), (None, {}, 30), ("pass", [], 30),
    ("pass", {"bad": float("nan")}, 30), ("pass", {"bad": object()}, 30),
    ("pass", {}, 0), ("pass", {}, -1), ("pass", {}, True), ("pass", {}, "30"),
    ("pass", {}, float("inf")), ("pass", {}, float("nan")),
    ("pass", {}, 10 ** 500),
])
async def test_invalid_request_never_spawns(monkeypatch, program, payload, timeout):
    monkeypatch.setattr(ssh_transport.subprocess, "Popen", lambda *a, **k: pytest.fail("Unexpected process."))
    with pytest.raises(SshTransportError) as failure:
        await SshTransport().request(ENDPOINT, program, payload, timeout)
    assert failure.value.code == "invalid_request"


async def test_actual_fixture_observes_fixed_argv_clean_env_quoted_program_and_stdin(tmp_path, monkeypatch):
    monkeypatch.setenv("OPENAI_API_KEY", "fixture-provider-secret")
    monkeypatch.setenv("AWS_SESSION_TOKEN", "fixture-cloud-secret")
    monkeypatch.setenv("SSH_AUTH_SOCK", "/fixture/agent.sock")
    transport = fixture_transport(tmp_path,
        "import json,os,sys\n"
        "body=json.load(sys.stdin)\n"
        f"print({FRAME_PREFIX!r}+json.dumps({{'argv':sys.argv[1:],'environment':dict(os.environ),'payload':body}},separators=(',',':')))\n")
    program = "print('a quote: \\\" and $() and ;')\n# $(must-not-execute)"
    payload = {"unicode": "\u00e5\U0001f30c", "large": "x" * 70000, "quoted": "$(not-a-command)"}
    result = await transport.request(ENDPOINT, program, payload)
    argv = result["argv"]
    assert argv[:5] == ["-F", "/dev/null", "-v", "-T", "-o"]
    for option in ["StrictHostKeyChecking=yes", "BatchMode=yes", "UpdateHostKeys=no", "ForwardAgent=no",
                   "PermitLocalCommand=no", "ClearAllForwardings=yes", "ControlMaster=no", "ControlPath=none",
                   "ConnectTimeout=10", "FingerprintHash=sha256"]:
        assert option in argv and argv[argv.index(option)-1] == "-o"
    assert argv[-7:-1] == ["-p", "2222", "-l", "fixture", "--", "fixture.example"]
    assert shlex.split(argv[-1]) == ["python3", "-I", "-c", program]
    assert result["payload"] == payload
    assert result["environment"]["SSH_AUTH_SOCK"] == "/fixture/agent.sock"
    assert "OPENAI_API_KEY" not in result["environment"] and "AWS_SESSION_TOKEN" not in result["environment"]
    # CPython/macOS may add locale keys when the fixture starts; Popen receives
    # only the transport's allowlist, checked separately below.
    assert set(result["environment"]) <= {"HOME", "USER", "PATH", "SSH_AUTH_SOCK", "LC_CTYPE", "__CF_USER_TEXT_ENCODING"}
    assert "_transport" not in result


def test_production_executable_and_local_process_ownership_are_fixed(monkeypatch):
    seen = {}
    def unavailable(argv, **kwargs):
        seen.update(argv=argv, **kwargs)
        raise FileNotFoundError("Private path should never be published.")
    monkeypatch.setattr(ssh_transport.subprocess, "Popen", unavailable)
    async def request():
        with pytest.raises(SshTransportError) as failure:
            await SshTransport().request(ENDPOINT, "pass", {})
        assert failure.value.code == "connection_unavailable"
        assert "Private" not in str(failure.value)
    asyncio.run(request())
    assert seen["argv"][0] == "/usr/bin/ssh"
    assert seen["start_new_session"] is True and "shell" not in seen
    assert set(seen["env"]) <= {"HOME", "USER", "PATH", "SSH_AUTH_SOCK"}


def public_host_key(seed=0, kind=b"ssh-ed25519"):
    raw = bytes((index+seed) % 256 for index in range(32))
    blob = len(kind).to_bytes(4, "big") + kind + len(raw).to_bytes(4, "big") + raw
    return base64.b64encode(blob).decode(), "SHA256:" + base64.b64encode(hashlib.sha256(blob).digest()).decode().rstrip("=")


def pinned_transport(tmp_path, known_hosts_paths, fingerprint, marker=None):
    path = tmp_path / "pinned-fake-ssh.py"
    verification = VERIFIED.replace(FINGERPRINT, fingerprint)
    marker_write = f"Path({str(marker)!r}).write_text('admitted')\n" if marker else ""
    path.write_text("import json,os,stat,sys\nfrom pathlib import Path\n"
        "body=json.load(sys.stdin)\n"
        "args=sys.argv[1:]\n"
        "held=next(value.split('=',1)[1] for value in args if value.startswith('UserKnownHostsFile='))\n"
        "metadata=os.stat(held)\n"
        + marker_write +
        f"sys.stderr.write({verification!r})\n"
        f"print({FRAME_PREFIX!r}+json.dumps({{'argv':args,'known_hosts_path':held,'known_hosts':Path(held).read_text(),'mode':stat.S_IMODE(metadata.st_mode),'owner':metadata.st_uid}},separators=(',',':')))\n")
    return SshTransport(executable=sys.executable, argv_prefix=(str(path),), known_hosts_paths=known_hosts_paths)


def hashed_host(lookup):
    salt = bytes(range(20))
    hashed = hmac.new(salt, lookup.encode(), hashlib.sha1).digest()
    return "|1|" + base64.b64encode(salt).decode() + "|" + base64.b64encode(hashed).decode()


@pytest.mark.parametrize("expected", ["", "SHA256:bad", "MD5:ab:cd", "SHA256:" + "-"*43, 7, {}, "SHA256:" + "A"*500])
async def test_invalid_pin_cannot_spawn_or_read_known_hosts(monkeypatch, expected):
    monkeypatch.setattr(ssh_transport.subprocess, "Popen", lambda *a, **k: pytest.fail("Invalid pin created a process."))
    with pytest.raises(SshTransportError) as failure:
        await SshTransport().request(ENDPOINT, "pass", {}, expected_fingerprint=expected)
    assert failure.value.code == "invalid_request"


@pytest.mark.parametrize("pattern", ["[fixture.example]:2222", "[fixture.example]:2222,[other.example]:2222",
                                    "[*.example]:2222", hashed_host("[fixture.example]:2222")])
async def test_actual_keygen_represents_exact_public_pin_in_private_file(tmp_path, pattern):
    key, fingerprint = public_host_key()
    other_key, _ = public_host_key(1)
    trust = tmp_path / "fixture_known_hosts"
    original = f"{pattern} ssh-ed25519 {key} private-comment\n[fixture.example]:2222 ssh-ed25519 {other_key}\n"
    trust.write_text(original)
    result = await pinned_transport(tmp_path, (trust,), fingerprint).request(ENDPOINT, "pass", {}, expected_fingerprint=fingerprint)
    assert result["known_hosts"] == f"[fixture.example]:2222 ssh-ed25519 {key}\n"
    assert result["mode"] == 0o600 and result["owner"] == os.getuid()
    assert "GlobalKnownHostsFile=/dev/null" in result["argv"]
    assert result["_transport"] == {"host_key_fingerprint": fingerprint}
    assert not Path(result["known_hosts_path"]).exists()
    assert trust.read_text() == original


@pytest.mark.parametrize("entry", ["missing", "foreign", "wrong-port", "negated", "authority-only", "certificate", "malformed"])
async def test_unrepresentable_pin_refuses_before_remote_program_admission(tmp_path, entry):
    key, fingerprint = public_host_key()
    foreign, _ = public_host_key(1)
    certificate, _ = public_host_key(kind=b"ssh-ed25519-cert-v01@openssh.com")
    content = {
        "missing": "other.example ssh-ed25519 " + key + "\n",
        "foreign": "[fixture.example]:2222 ssh-ed25519 " + foreign + "\n",
        "wrong-port": "fixture.example ssh-ed25519 " + key + "\n",
        "negated": "![fixture.example]:2222,[*.example]:2222 ssh-ed25519 " + key + "\n",
        "authority-only": "@cert-authority [fixture.example]:2222 ssh-ed25519 " + key + "\n",
        "certificate": "[fixture.example]:2222 ssh-ed25519-cert-v01@openssh.com " + certificate + "\n",
        "malformed": "[fixture.example]:2222 ssh-ed25519 malformed-base64\n",
    }[entry]
    trust = tmp_path / "fixture_known_hosts"
    trust.write_text(content)
    marker = tmp_path / "remote-admitted"
    with pytest.raises(SshTransportError) as failure:
        await pinned_transport(tmp_path, (trust,), fingerprint, marker).request(
            ENDPOINT, "pass", {"settings": {"FIXTURE_SECRET": "not-for-a-new-host"}}, expected_fingerprint=fingerprint)
    assert failure.value.code == "host_key_unavailable" and not marker.exists()
    assert trust.read_text() == content
    assert "not-for-a-new-host" not in str(failure.value)


@pytest.mark.parametrize("revoked_first", [True, False])
async def test_global_or_user_revocation_overrides_same_pinned_regular_key(tmp_path, revoked_first):
    key, fingerprint = public_host_key()
    ordinary, revoked = tmp_path / "ordinary_hosts", tmp_path / "revoked_hosts"
    ordinary.write_text(f"[fixture.example]:2222 ssh-ed25519 {key}\n")
    revoked.write_text(f"@revoked {hashed_host('[fixture.example]:2222')} ssh-ed25519 {key}\n")
    paths = (revoked, ordinary) if revoked_first else (ordinary, revoked)
    marker = tmp_path / "remote-admitted"
    with pytest.raises(SshTransportError) as failure:
        await pinned_transport(tmp_path, paths, fingerprint, marker).request(ENDPOINT, "pass", {}, expected_fingerprint=fingerprint)
    assert failure.value.code == "host_key_unavailable" and not marker.exists()


async def test_nonmatching_revoked_records_remain_in_pinned_trust_file(tmp_path):
    key, fingerprint = public_host_key()
    revoked, _ = public_host_key(1)
    trust = tmp_path / "fixture_known_hosts"
    trust.write_text(f"[fixture.example]:2222 ssh-ed25519 {key}\n@revoked [fixture.example]:2222 ssh-ed25519 {revoked}\n")
    result = await pinned_transport(tmp_path, (trust,), fingerprint).request(ENDPOINT, "pass", {}, expected_fingerprint=fingerprint)
    assert set(result["known_hosts"].splitlines()) == {
        f"[fixture.example]:2222 ssh-ed25519 {key}", f"@revoked [fixture.example]:2222 ssh-ed25519 {revoked}"}


async def test_missing_trust_files_are_skipped_but_no_ambient_fallback_exists(tmp_path):
    _, fingerprint = public_host_key()
    marker = tmp_path / "remote-admitted"
    with pytest.raises(SshTransportError) as failure:
        await pinned_transport(tmp_path, (tmp_path / "absent",), fingerprint, marker).request(
            ENDPOINT, "pass", {}, expected_fingerprint=fingerprint)
    assert failure.value.code == "host_key_unavailable" and not marker.exists()


async def test_public_pin_query_has_bounded_output_and_no_admission_on_overflow(tmp_path):
    key, fingerprint = public_host_key()
    trust = tmp_path / "fixture_known_hosts"
    trust.write_text(f"[fixture.example]:2222 ssh-ed25519 {key}\n" * 3500)
    marker = tmp_path / "remote-admitted"
    with pytest.raises(SshTransportError) as failure:
        await pinned_transport(tmp_path, (trust,), fingerprint, marker).request(
            ENDPOINT, "pass", {}, timeout=3, expected_fingerprint=fingerprint)
    assert failure.value.code == "host_key_unavailable" and not marker.exists()


async def test_verified_observation_must_still_match_pre_admission_pin(tmp_path):
    key, fingerprint = public_host_key()
    trust = tmp_path / "fixture_known_hosts"
    trust.write_text(f"[fixture.example]:2222 ssh-ed25519 {key}\n")
    path = tmp_path / "fake-ssh.py"
    path.write_text(reply_script(frame({}), VERIFIED))
    transport = SshTransport(executable=sys.executable, argv_prefix=(str(path),), known_hosts_paths=(trust,))
    with pytest.raises(SshTransportError) as failure:
        await transport.request(ENDPOINT, "pass", {}, expected_fingerprint=fingerprint)
    assert failure.value.code == "invalid_response"


@pytest.mark.parametrize("cancel", [False, True])
async def test_pin_file_is_removed_on_actual_timeout_or_cancel(tmp_path, cancel):
    key, fingerprint = public_host_key()
    trust, marker = tmp_path / "fixture_known_hosts", tmp_path / "pin-path"
    trust.write_text(f"[fixture.example]:2222 ssh-ed25519 {key}\n")
    path = tmp_path / "fake-ssh.py"
    path.write_text("import json,sys,time\nfrom pathlib import Path\n"
        "held=next(value.split('=',1)[1] for value in sys.argv[1:] if value.startswith('UserKnownHostsFile='))\n"
        f"Path({str(marker)!r}).write_text(json.dumps({{'path':held}}))\n"
        "time.sleep(60)\n")
    transport = SshTransport(executable=sys.executable, argv_prefix=(str(path),), known_hosts_paths=(trust,))
    task = asyncio.create_task(transport.request(ENDPOINT, "pass", {}, timeout=2 if cancel else .2, expected_fingerprint=fingerprint))
    await wait_record(marker)
    if cancel:
        task.cancel()
        with pytest.raises(asyncio.CancelledError):
            await asyncio.wait_for(task, 2)
    else:
        with pytest.raises(SshTransportError) as failure:
            await task
        assert failure.value.code == "timeout"
    assert not Path((await wait_record(marker))["path"]).exists()


async def test_verified_host_fingerprint_is_only_trusted_separate_metadata(tmp_path):
    transport = fixture_transport(tmp_path, reply_script(frame({"ok": True}), VERIFIED + "private debug text\n"))
    result = await transport.request(ENDPOINT, "pass", {})
    assert result == {"ok": True, "_transport": {"host_key_fingerprint": FINGERPRINT}}
    assert "private debug" not in json.dumps(result)


@pytest.mark.parametrize("stderr", [
    "", f"debug1: Server host key: ssh-ed25519 {FINGERPRINT}\n",
    "debug1: Host 'fixture.example' is known and matches the ED25519 host key.\n",
    VERIFIED.replace("ED25519 host key", "RSA host key"),
    VERIFIED.replace(FINGERPRINT, "SHA256:invalid"),
    f"debug1: Server host key: ssh-ed25519 {FINGERPRINT}\nAuthenticated to host\n" + VERIFIED,
    f"debug1: Server host key: ssh-ed25519 {FINGERPRINT}\n" + VERIFIED,
    VERIFIED.replace("Server host key:", "Server host certificate:"),
])
async def test_unverified_or_unsupported_host_key_never_fabricated(tmp_path, stderr):
    result = await fixture_transport(tmp_path, reply_script(frame({"ok": True}), stderr)).request(ENDPOINT, "pass", {})
    assert result == {"ok": True}


async def test_later_remote_debug_shaped_output_cannot_replace_initial_fingerprint(tmp_path):
    foreign = "SHA256:" + base64.b64encode(b"z" * 32).decode().rstrip("=")
    result = await fixture_transport(tmp_path, reply_script(frame({}), VERIFIED + VERIFIED.replace(FINGERPRINT, foreign))).request(ENDPOINT, "pass", {})
    assert result["_transport"]["host_key_fingerprint"] == FINGERPRINT


@pytest.mark.parametrize("output", [
    b'{"ok":true}\n', b'banner\n' + frame({}), frame({}) + frame({}), frame({}) + b'junk',
    frame({})[:-1], frame({}).replace(b"\n", b"\r\n"), frame([]), frame("text"),
    (FRAME_PREFIX + '{"ok":NaN}\n').encode(), (FRAME_PREFIX + '{"ok":1,"ok":2}\n').encode(),
    (FRAME_PREFIX + '{"ok":1e999}\n').encode(),
    (FRAME_PREFIX + '{broken}\n').encode(), FRAME_PREFIX.encode() + b'{"ok":"\xff"}\n',
    frame({"_transport": {"host_key_fingerprint": FINGERPRINT}}),
])
async def test_rejects_unframed_multiple_malformed_and_foreign_metadata(tmp_path, output):
    with pytest.raises(SshTransportError) as failure:
        await fixture_transport(tmp_path, reply_script(output, VERIFIED)).request(ENDPOINT, "pass", {})
    assert failure.value.code == "invalid_response"
    assert "remote work may still be running" in str(failure.value)


async def test_nonzero_ssh_exit_is_observation_unavailable_even_with_valid_frame(tmp_path):
    with pytest.raises(SshTransportError) as failure:
        await fixture_transport(tmp_path, reply_script(frame({"remote_status": "running"}), "secret error\n", 255)).request(ENDPOINT, "pass", {})
    assert failure.value.code == "connection_unavailable"
    assert "secret error" not in str(failure.value)
    assert "remote work may still be running" in str(failure.value)


async def test_stdout_exact_limit_accepted_and_actual_overflow_force_reaped(tmp_path):
    overhead = len(frame({"value": ""}))
    exact = frame({"value": "x" * (ssh_transport.MAX_STDOUT_BYTES-overhead)})
    assert len(exact) == ssh_transport.MAX_STDOUT_BYTES
    result = await fixture_transport(tmp_path, reply_script(exact)).request(ENDPOINT, "pass", {})
    assert len(result["value"]) == ssh_transport.MAX_STDOUT_BYTES-overhead
    marker = tmp_path / "overflow-pid"
    script = ("import json,os,signal,sys,time\nfrom pathlib import Path\n"
              f"Path({str(marker)!r}).write_text(json.dumps({{'pid':os.getpid()}}))\n"
              "signal.signal(signal.SIGTERM,signal.SIG_IGN)\n"
              f"sys.stdout.buffer.write(b'x'*{ssh_transport.MAX_STDOUT_BYTES+1}); sys.stdout.buffer.flush()\n"
              "time.sleep(60)\n")
    started = time.monotonic()
    with pytest.raises(SshTransportError) as failure:
        await fixture_transport(tmp_path, script).request(ENDPOINT, "pass", {}, timeout=3)
    assert failure.value.code == "output_limit" and time.monotonic()-started < 3
    await assert_absent((await wait_record(marker))["pid"])


async def test_stderr_is_bounded_and_drained_without_exposing_raw_logs(tmp_path):
    transport = fixture_transport(tmp_path,
        "import sys\n"
        "sys.stdin.buffer.read()\n"
        f"sys.stderr.write({VERIFIED!r}); sys.stderr.flush()\n"
        f"sys.stderr.buffer.write(b'private-debug-'*{ssh_transport.MAX_STDERR_BYTES}); sys.stderr.buffer.flush()\n"
        f"sys.stdout.buffer.write({frame({'ok':True})!r})\n")
    result = await transport.request(ENDPOINT, "pass", {}, timeout=3)
    assert result == {"ok": True, "_transport": {"host_key_fingerprint": FINGERPRINT}}


def owned_tree_script(marker, *, parent_exit=False):
    return ("import json,os,signal,time\nfrom pathlib import Path\n"
            "signal.signal(signal.SIGTERM,signal.SIG_IGN)\n"
            "child=os.fork()\n"
            "if child == 0:\n while True: time.sleep(1)\n"
            f"marker=Path({str(marker)!r})\n"
            "pending=marker.with_name(marker.name+'.pending')\n"
            "pending.write_text(json.dumps({'parent':os.getpid(),'child':child,'group':os.getpgrp()}))\n"
            "pending.replace(marker)\n"
            + ("os._exit(0)\n" if parent_exit else "while True: time.sleep(1)\n"))


@pytest.mark.parametrize("cancel,parent_exit", [(False, False), (True, False), (False, True), (True, True)])
async def test_timeout_and_cancel_reap_owned_leader_and_descendant_only(tmp_path, monkeypatch, cancel, parent_exit):
    marker = tmp_path / "owned-tree"
    loop = asyncio.get_running_loop()
    clock, elapsed = loop.time, [0]
    monkeypatch.setattr(loop, "time", lambda: clock() + elapsed[0])
    unrelated, task, record = None, None, None
    try:
        unrelated = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(60)"], start_new_session=True)
        task = asyncio.create_task(fixture_transport(tmp_path, owned_tree_script(marker, parent_exit=parent_exit)).request(
            ENDPOINT, "pass", {}, timeout=30))
        record = await wait_record(marker, timeout=10)
        assert record["group"] == record["parent"] != os.getpgrp()
        if cancel:
            task.cancel()
            with pytest.raises(asyncio.CancelledError):
                await asyncio.wait_for(task, ssh_transport.FORCE_REAP_TIMEOUT + 2)
        else:
            # Expire the actual transport deadline after the owned hierarchy
            # exists. Process startup speed must not decide test coverage.
            elapsed[0] = 31
            with pytest.raises(SshTransportError) as failure:
                await asyncio.wait_for(task, ssh_transport.FORCE_REAP_TIMEOUT + 2)
            assert failure.value.code == "timeout"
            assert "remote work may still be running" in str(failure.value)
        await assert_absent(record["parent"])
        await assert_absent(record["child"])
        assert unrelated.poll() is None
    finally:
        if task is not None and not task.done():
            task.cancel()
            await asyncio.gather(task, return_exceptions=True)
        if record is not None and alive(record["parent"]):
            os.killpg(record["group"], signal.SIGKILL)
        if unrelated is not None:
            unrelated.kill()
            unrelated.wait(timeout=2)
        monkeypatch.setattr(loop, "time", clock)


async def test_cleanup_budget_failure_preserves_observation_unavailable(monkeypatch):
    signals = []
    class FakeProcess:
        pid, returncode = 876543, None
        stdin = stdout = stderr = type("Stream", (), {"close": lambda self: None})()
        def wait(self, timeout):
            assert timeout == ssh_transport.FORCE_REAP_TIMEOUT
            raise subprocess.TimeoutExpired("private executable", timeout)
    monkeypatch.setattr(ssh_transport.os, "killpg", lambda pid, signum: signals.append((pid, signum)))
    with pytest.raises(SshTransportError) as failure:
        await ssh_transport._cleanup(FakeProcess(), [])
    assert signals == [(876543, signal.SIGKILL)]
    assert failure.value.code == "cleanup_unavailable" and "private executable" not in str(failure.value)


async def test_cached_returncode_cleanup_never_signals_reaped_group(monkeypatch):
    class FakeProcess:
        pid, returncode = 876543, 0
        stdin = stdout = stderr = type("Stream", (), {"close": lambda self: None})()
        def wait(self, timeout):
            assert timeout == ssh_transport.FORCE_REAP_TIMEOUT
            return self.returncode
    monkeypatch.setattr(ssh_transport.os, "killpg", lambda *args: pytest.fail("Reaped group was signaled."))
    await ssh_transport._cleanup(FakeProcess(), [])


async def test_cleanup_signal_error_is_fixed_and_still_attempts_bounded_reap(monkeypatch):
    waited = []
    class FakeProcess:
        pid, returncode = 876543, None
        stdin = stdout = stderr = type("Stream", (), {"close": lambda self: None})()
        def wait(self, timeout):
            waited.append(timeout)
            return 0
    def denied(*args):
        raise PermissionError("private diagnostic")
    monkeypatch.setattr(ssh_transport.os, "killpg", denied)
    with pytest.raises(SshTransportError) as failure:
        await ssh_transport._cleanup(FakeProcess(), [])
    assert waited == [ssh_transport.FORCE_REAP_TIMEOUT]
    assert failure.value.code == "cleanup_unavailable" and "private diagnostic" not in str(failure.value)
