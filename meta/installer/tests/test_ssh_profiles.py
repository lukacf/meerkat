"""Exact host selection using private synthetic files, never a real SSH target."""
import asyncio
import base64
from copy import deepcopy
import hashlib
import json
import os
from pathlib import Path
import struct

import pytest

from launchkit import ssh_profiles, ssh_transport, ssh_browser_access
from launchkit.ssh_profiles import SshProfiles, SshProfilesError, configured_ssh_transport
from launchkit.ssh_transport import SshTransport, SshTransportError, FRAME_PREFIX
from launchkit.ssh_browser_access import SshBrowserAccessTransport, BrowserAccessError
from test_remote_deployment import service, connect


ENDPOINT = {"host": "synthetic.example", "user": "fixture", "ssh_port": 2222}
BLOB = struct.pack(">I", 11) + b"ssh-ed25519" + struct.pack(">I", 32) + bytes(range(32))
FINGERPRINT = "SHA256:" + base64.b64encode(hashlib.sha256(BLOB).digest()).decode().rstrip("=")
HOSTS = b"[synthetic.example]:2222 ssh-ed25519 " + base64.b64encode(BLOB) + b"\n"
KEY = b"SYNTHETIC PRIVATE KEY BYTES - NOT AN SSH IDENTITY\n"


def private(path, content):
    path.write_bytes(content)
    path.chmod(0o600)
    return path


@pytest.fixture
def files(tmp_path):
    root = tmp_path.resolve()
    identity = private(root / "private identity % literal", KEY)
    hosts = private(root / "verified-hosts", HOSTS)
    document = root / "profiles.json"
    value = {"version": 1, "profiles": [{"endpoint": deepcopy(ENDPOINT),
        "identity_file": str(identity), "known_hosts_file": str(hosts)}]}
    private(document, json.dumps(value).encode())
    return document, identity, hosts, value


def rewrite(files, mutate):
    document, _, _, value = files
    changed = deepcopy(value)
    mutate(changed)
    private(document, json.dumps(changed).encode())


def assert_private_prefix(prefix, hosts=None):
    assert prefix[:1] == ("-i",)
    identity = Path(prefix[1])
    options = {
        item.split("=", 1)[0]: item.split("=", 1)[1]
        for index, item in enumerate(prefix) if index > 1 and prefix[index - 1] == "-o"
    }
    if hosts is None:
        hosts = Path(options["UserKnownHostsFile"])
    else:
        assert "UserKnownHostsFile" not in options
    assert identity.read_bytes() == KEY and hosts.read_bytes() == HOSTS
    for path in (identity, hosts):
        assert path.stat().st_mode & 0o777 == 0o600
    assert identity.parent.stat().st_mode & 0o777 == 0o700
    assert options["IdentitiesOnly"] == "yes" and options["IdentityAgent"] == "none"
    assert options["PreferredAuthentications"] == "publickey"
    assert options["AddKeysToAgent"] == "no" and options["GlobalKnownHostsFile"] == "/dev/null"
    return identity, hosts


async def test_initial_and_pinned_requests_use_actual_fixed_ssh_argv_and_same_trust(files, monkeypatch):
    profiles = SshProfiles(str(files[0]))
    calls = []; copies = []
    async def execute(argv, body, timeout):
        calls.append(list(argv))
        if argv[0] == "/usr/bin/ssh-keygen":
            assert argv[1:3] == ["-F", "[synthetic.example]:2222"]
            assert Path(argv[-1]).read_bytes() == HOSTS
            return HOSTS, b"", 0
        assert argv[0] == "/usr/bin/ssh"
        split = argv.index("-F")
        identity, hosts = assert_private_prefix(tuple(argv[1:split]))
        copies.append((identity, hosts))
        assert argv[split:split + 2] == ["-F", "/dev/null"]
        for option in ("StrictHostKeyChecking=yes", "BatchMode=yes", "ForwardAgent=no",
                       "PermitLocalCommand=no", "ClearAllForwardings=yes", "ControlMaster=no"):
            assert option in argv
        assert argv[-2] == ENDPOINT["host"] and argv[-1].startswith("python3 -I -c ")
        assert json.loads(body) == {"request": "ordinary fixture"}
        stderr = ("debug1: Server host key: ssh-ed25519 " + FINGERPRINT + "\n"
                  "debug1: Host '[synthetic.example]:2222' is known and matches the ED25519 host key.\n").encode()
        return (FRAME_PREFIX + '{"ok":true}\n').encode(), stderr, 0
    monkeypatch.setattr(ssh_transport, "_execute", execute)
    assert (await profiles.request(ENDPOINT, "pass", {"request": "ordinary fixture"}))["ok"]
    assert (await profiles.request(ENDPOINT, "pass", {"request": "ordinary fixture"},
        expected_fingerprint=FINGERPRINT))["ok"]
    assert len(calls) == 3 and calls[1][0] == "/usr/bin/ssh-keygen"
    assert all(not path.exists() for pair in copies for path in pair)
    with pytest.raises(SshTransportError) as refusal:
        await profiles.request(ENDPOINT, "pass", {}, expected_fingerprint="SHA256:" + "A" * 43)
    assert refusal.value.code == "host_key_unavailable" and len(calls) == 4


@pytest.mark.parametrize("kind", ["rpc", "browser"])
async def test_retained_fingerprint_uses_only_native_reduced_pin_file(files, monkeypatch, kind):
    profiles = SshProfiles(str(files[0])); seen = []
    diagnostics = ("debug1: Server host key: ssh-ed25519 " + FINGERPRINT + "\n"
        "debug1: Host '[synthetic.example]:2222' is known and matches the ED25519 host key.\n").encode()
    async def execute(argv, *args):
        if argv[0] == "/usr/bin/ssh-keygen": return HOSTS, b"", 0
        seen.append(argv)
        return (FRAME_PREFIX + '{"ok":true}\n').encode(), diagnostics, 0
    monkeypatch.setattr(ssh_transport, "_execute", execute)
    if kind == "rpc":
        await profiles.request(ENDPOINT, "pass", {}, expected_fingerprint=FINGERPRINT)
    else:
        class Handle:
            def __init__(self, process): pass
            async def _send(self, body): seen.append(json.loads(body)["argv"])
            async def _frame(self):
                return {"event": "ready", "diagnostics": base64.b64encode(diagnostics).decode()}
            async def observe(self): return True
            async def close(self): return True
        monkeypatch.setattr(ssh_browser_access.subprocess, "Popen", lambda *args, **kwargs: object())
        monkeypatch.setattr(ssh_browser_access, "_Handle", Handle)
        handle = await profiles.open(ENDPOINT, expected_fingerprint=FINGERPRINT, port=18088)
        assert await handle.close() is True
    # OpenSSH uses the first scalar option, so an earlier broad trust file
    # must never override the existing transport's exact fingerprint pin.
    assert len(seen) == 1
    trust = [arg for arg in seen[0] if arg.startswith("UserKnownHostsFile=")]
    assert len(trust) == 1
    if kind == "browser": assert trust == ["UserKnownHostsFile=@PIN@"]


@pytest.mark.parametrize("change", [{"host": "different.example"}, {"user": "different"}, {"ssh_port": 22}])
async def test_unspecified_exact_endpoint_keeps_default_transport(files, monkeypatch, change):
    profiles = SshProfiles(str(files[0])); calls = []
    async def request(self, endpoint, program, payload, timeout, **kwargs):
        calls.append((self, deepcopy(endpoint), kwargs))
        return {"fixture": True}
    monkeypatch.setattr(SshTransport, "request", request)
    endpoint = {**ENDPOINT, **change}
    assert profiles.browser_transport(endpoint) is None
    assert await profiles.request(endpoint, "pass", {}) == {"fixture": True}
    assert len(calls) == 1 and calls[0][0]._argv_prefix == ()
    assert calls[0][0]._known_hosts_paths is None and calls[0][1] == endpoint


async def test_browser_selection_reuses_copies_and_exact_guardian_until_join(files, monkeypatch, service):
    profiles = SshProfiles(str(files[0])); observed = []
    class Handle:
        joined = False
        async def observe(self): return True
        async def close(self): return self.joined
    raw = Handle()
    async def open_transport(self, endpoint, **kwargs):
        identity, hosts = assert_private_prefix(self._argv_prefix, self._ssh._known_hosts_paths[0])
        assert self._ssh._known_hosts_paths == (hosts,)
        observed.append((identity, hosts, deepcopy(endpoint), kwargs))
        return raw
    monkeypatch.setattr(SshBrowserAccessTransport, "open", open_transport)
    service.transport = profiles
    selected = service.browser_transport({"target": "linux", "endpoint": ENDPOINT})
    assert selected is profiles
    handle = await selected.open(ENDPOINT, expected_fingerprint=FINGERPRINT, port=18088)
    identity, hosts, endpoint, args = observed[0]
    assert endpoint == ENDPOINT and args["expected_fingerprint"] == FINGERPRINT and args["port"] == 18088
    assert 0 < args["timeout"] <= 15 and args["deadline"] > asyncio.get_running_loop().time()
    assert await handle.observe() is True
    assert await handle.close() is False and identity.exists() and hosts.exists()
    raw.joined = True
    assert await handle.close() is True and not identity.exists() and not hosts.exists()
    assert await handle.close() is True
    cloud = object(); service.cloud_transport = cloud
    assert service.browser_transport({"target": "gcp", "endpoint": ENDPOINT}) is cloud
    assert service.browser_transport({"target": "linux", "endpoint": {**ENDPOINT, "user": "other"}}) is None


async def test_browser_failed_start_retains_exact_handle_and_copies_until_join(files, monkeypatch):
    profiles = SshProfiles(str(files[0])); captured = []
    class Handle:
        joined = False
        async def observe(self): return False
        async def close(self): return self.joined
    raw = Handle()
    async def fail(self, *args, **kwargs):
        captured.append(assert_private_prefix(self._argv_prefix, self._ssh._known_hosts_paths[0]))
        raise BrowserAccessError("cleanup-unavailable", handle=raw)
    monkeypatch.setattr(SshBrowserAccessTransport, "open", fail)
    with pytest.raises(BrowserAccessError) as failed:
        await profiles.open(ENDPOINT, expected_fingerprint=FINGERPRINT, port=18088)
    assert failed.value.code == "cleanup-unavailable"
    assert await failed.value.handle.close() is False
    assert all(path.exists() for path in captured[0])
    raw.joined = True
    assert await failed.value.handle.close() is True
    assert all(not path.exists() for path in captured[0])


@pytest.mark.parametrize("mutate", [
    lambda x: x.update(version=True), lambda x: x.update(version=2),
    lambda x: x.update(extra="SENSITIVE_UNKNOWN_FIELD"), lambda x: x.update(profiles={}),
    lambda x: x["profiles"].append(deepcopy(x["profiles"][0])),
    lambda x: x.update(profiles=x["profiles"] * 33),
    lambda x: x["profiles"][0].update(argv=["-o", "ProxyCommand=secret"]),
    lambda x: x["profiles"][0]["endpoint"].update(host="-oUnsafe"),
    lambda x: x["profiles"][0]["endpoint"].update(ssh_port=True),
    lambda x: x["profiles"][0]["endpoint"].pop("ssh_port"),
    lambda x: x["profiles"][0].update(identity_file="relative/private"),
    lambda x: x["profiles"][0].update(known_hosts_file="/tmp/../private"),
    lambda x: x["profiles"][0].update(identity_file="/tmp//private"),
])
def test_closed_malformed_configuration_refuses_without_disclosure(files, mutate):
    rewrite(files, mutate)
    with pytest.raises(SshProfilesError) as error:
        SshProfiles(str(files[0]))
    assert str(files[0]) not in str(error.value) and "SENSITIVE" not in str(error.value)


@pytest.mark.parametrize("document", [b'{"version":1,"version":1,"profiles":[]}',
    b'{"version":1,"profiles":[],"profiles":[]}', b'{"version":NaN,"profiles":[]}', b'\xff'])
def test_duplicate_json_members_and_invalid_encoding_fail_closed(files, document):
    private(files[0], document)
    with pytest.raises(SshProfilesError): SshProfiles(str(files[0]))


@pytest.mark.parametrize("index", [0, 1, 2])
@pytest.mark.parametrize("unsafe", ["permissions", "symlink", "hardlink", "fifo"])
def test_every_selected_file_requires_private_ordinary_ownership(files, index, unsafe):
    path = files[index]
    if unsafe == "permissions": path.chmod(0o640)
    elif unsafe == "hardlink": os.link(path, path.with_name(path.name + ".linked"))
    elif unsafe == "symlink":
        old = path.with_name(path.name + ".original"); path.rename(old); path.symlink_to(old)
    else:
        path.unlink(); os.mkfifo(path, 0o600)
    with pytest.raises(SshProfilesError): SshProfiles(str(files[0]))


def test_symlink_parent_is_refused_even_for_owned_regular_leaf(files):
    link = files[0].parent / "parent-link"; link.symlink_to(files[0].parent, target_is_directory=True)
    with pytest.raises(SshProfilesError): SshProfiles(str(link / files[0].name))


def test_foreign_owner_is_refused_before_parsing_private_contents(files, monkeypatch):
    uid = os.getuid()
    monkeypatch.setattr(ssh_profiles.os, "getuid", lambda: uid + 1)
    with pytest.raises(SshProfilesError): SshProfiles(str(files[0]))


@pytest.mark.parametrize("index,limit", [(0, ssh_profiles.MAX_DOCUMENT),
    (1, ssh_profiles.MAX_IDENTITY), (2, ssh_profiles.MAX_KNOWN_HOSTS)])
def test_file_bytes_are_bounded_before_any_connection(files, index, limit):
    private(files[index], b"x" * (limit + 1))
    with pytest.raises(SshProfilesError): SshProfiles(str(files[0]))


@pytest.mark.parametrize("index", [0, 1, 2])
async def test_selected_file_change_blocks_rpc_and_browser_without_fallback(files, monkeypatch, index):
    profiles = SshProfiles(str(files[0]))
    private(files[index], files[index].read_bytes() + b"changed")
    async def forbidden(*args, **kwargs): pytest.fail("Unsafe selection reached SSH transport.")
    monkeypatch.setattr(SshTransport, "request", forbidden)
    monkeypatch.setattr(SshBrowserAccessTransport, "open", forbidden)
    with pytest.raises(SshTransportError) as rpc:
        await profiles.request(ENDPOINT, "pass", {})
    with pytest.raises(BrowserAccessError) as browser:
        await profiles.open(ENDPOINT, expected_fingerprint=FINGERPRINT, port=18088)
    assert rpc.value.code == "configuration_unavailable" and browser.value.code == "configuration-unavailable"
    for message in (str(rpc.value), str(browser.value)):
        assert str(files[index]) not in message and KEY.decode().strip() not in message


async def test_admitted_rpc_uses_checked_bytes_after_original_path_changes(files, monkeypatch):
    profiles = SshProfiles(str(files[0])); copy = profiles._copies; seen = []
    def change(selected):
        private(files[1], b"REPLACEMENT KEY - MUST NOT BE USED")
        return copy(selected)
    async def request(self, *args, **kwargs):
        seen.append(assert_private_prefix(self._argv_prefix))
        return {"fixture": True}
    monkeypatch.setattr(profiles, "_copies", change)
    monkeypatch.setattr(SshTransport, "request", request)
    assert await profiles.request(ENDPOINT, "pass", {}) == {"fixture": True}
    assert all(not path.exists() for path in seen[0])
    with pytest.raises(SshTransportError): await profiles.request(ENDPOINT, "pass", {})
    assert len(seen) == 1


async def test_invalid_endpoint_or_expired_budget_never_copies_private_files(files, monkeypatch):
    profiles = SshProfiles(str(files[0]))
    monkeypatch.setattr(profiles, "_copies", lambda _: pytest.fail("Invalid request copied credentials."))
    with pytest.raises(SshTransportError) as invalid:
        await profiles.request({**ENDPOINT, "argv": []}, "pass", {})
    assert invalid.value.code == "invalid_endpoint"
    with pytest.raises(BrowserAccessError) as expired:
        await profiles.open(ENDPOINT, expected_fingerprint=FINGERPRINT, port=18088,
            deadline=asyncio.get_running_loop().time() - 1)
    assert expired.value.code == "timeout"


def test_optional_environment_selection_is_explicit_and_invalid_never_defaults(files, monkeypatch):
    monkeypatch.delenv("META_FORGE_SSH_PROFILES", raising=False)
    assert configured_ssh_transport() is None
    monkeypatch.setenv("META_FORGE_SSH_PROFILES", str(files[0]))
    assert configured_ssh_transport().browser_transport(ENDPOINT) is not None
    monkeypatch.setenv("META_FORGE_SSH_PROFILES", "")
    with pytest.raises(SshProfilesError): configured_ssh_transport()


async def test_profile_transport_preserves_build_admission_and_configuration_failure_custody(files, monkeypatch, service):
    receipt = await connect(service)
    # Bind the synthetic service's exact endpoint to the private profile.
    value = files[3]; value["profiles"][0]["endpoint"] = deepcopy(receipt["endpoint"])
    private(files[0], json.dumps(value).encode())
    service.transport = SshProfiles(str(files[0]))
    async def forbidden(*args, **kwargs): pytest.fail("Guard/configuration failure reached SSH.")
    monkeypatch.setattr(SshTransport, "request", forbidden)
    service.implementation.accepted = False
    from launchkit.projects import DraftConflict
    with pytest.raises((ValueError, DraftConflict)):
        await service.connect(service.guide.identity, service.guide.state["version"],
            service.implementation.identity, **receipt["endpoint"], destination="~/src/team")
    service.implementation.accepted = True
    private(files[1], b"CHANGED PRIVATE IDENTITY")
    observed = await service.status(service.guide.identity, force=True)
    assert observed["status"] == "observation-unavailable" and observed["last_known_status"] == "configuration-required"
    assert "META_FORGE_SSH_PROFILES" in observed["detail"] and not observed["can_install"]
    assert str(files[1]) not in json.dumps(observed) and "CHANGED PRIVATE IDENTITY" not in json.dumps(observed)
