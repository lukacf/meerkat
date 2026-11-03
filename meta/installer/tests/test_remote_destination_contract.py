"""Join the real generated package, host transfer and standalone receiver."""
import base64
import hashlib
import importlib.util
import json
from pathlib import Path
import subprocess
import sys
import uuid

import pytest

from launchkit.blueprint import starter_blueprint
from launchkit.compiler import generate_files
from launchkit.deployment import package_configuration_fields
from launchkit.remote_deployment import controller_program, source_archive
from launchkit import remote_destination as destination
from launchkit import bootstrap


def accepted_package(provider="openai", local_knowledge=False):
    spec = starter_blueprint().model_dump(mode="json")
    spec["target"] = "linux"
    if provider == "anthropic":
        spec.update(provider=provider, model="claude-sonnet-4-6")
    if local_knowledge:
        spec["knowledge"]["mode"] = "local"
    generated = generate_files(spec)
    build_id = str(uuid.uuid4())
    generated["implementation-report.json"] = json.dumps({"build_id": build_id, "guide_version": 4,
        "verification": {"digest": "a" * 64}})
    files = {name: (content.encode(), 0o755 if name.endswith(".sh") else 0o644)
             for name, content in generated.items()}
    archive, transferred = source_archive(files)
    binding = {"guide_id": str(uuid.uuid4()), "guide_version": 4, "build_id": build_id,
        "accepted_source_digest": "a" * 64, "package_sha256": "b" * 64,
        "transferred_source_digest": transferred,
        "controller_sha256": hashlib.sha256(Path(destination.__file__).read_bytes()).hexdigest()}
    return files, archive, binding


@pytest.mark.parametrize("provider,local", [("openai", False), ("openai", True), ("anthropic", True)])
def test_generated_package_host_manifest_and_receiver_private_fields_agree(provider, local):
    files, archive, binding = accepted_package(provider, local)
    recovered, source, definition = destination._archive(base64.b64encode(archive).decode(), binding)
    assert recovered == archive
    host = {field["key"]: field for field in package_configuration_fields(definition, files)}
    receiver = {field["key"]: field for field in destination._fields(source, definition)}
    assert host.keys() == receiver.keys()
    for key in host:
        assert host[key]["secret"] == receiver[key]["secret"], key
        assert host[key]["required"] == receiver[key]["required"], key
    if local:
        assert receiver["OPENAI_API_KEY"]["required"] is True


def request_for(tmp_path, monkeypatch, action="status"):
    home = tmp_path / "owned-home"
    home.mkdir(mode=0o700)
    monkeypatch.setattr(destination, "_home", lambda: home)
    _, _, binding = accepted_package()
    root = home / "src/team"
    home.joinpath("src").mkdir(mode=0o700)
    _, host = destination._host(str(root))
    return root, {"action": action, "deployment_id": str(uuid.uuid4()), "binding": binding,
        "destination": str(root), "expected_host": {key: host[key] for key in ("uid", "home", "destination")}}


def test_actual_empty_destination_status_returns_exact_nonadmission_without_creating_project(tmp_path, monkeypatch):
    root, request = request_for(tmp_path, monkeypatch)
    value = destination.handle(request)
    assert value["protocol"] == 1 and value["binding"] == request["binding"]
    assert value["deployment_id"] == request["deployment_id"]
    assert value["status"] == "not-installed" and value["admitted"] is False
    assert value["worker_live"] is False and not value["service_owned"] and not root.exists()


@pytest.mark.parametrize("custody", [".mobkit/launch-remote/operation.json", ".mobkit/remote-service.json",
                                     ".mobkit/remote-service-health.json"])
def test_missing_joining_owner_never_turns_existing_custody_into_nonadmission(tmp_path, monkeypatch, custody):
    root, request = request_for(tmp_path, monkeypatch)
    record = root / custody
    record.parent.mkdir(parents=True, mode=0o700)
    record.write_text('{"owned-fixture":true}')
    record.chmod(0o600)
    value = destination.handle(request)
    assert value["error"]["code"] == "recovery-needed"
    assert value.get("admitted") is not False and value.get("status") != "not-installed"
    assert record.read_text() == '{"owned-fixture":true}'


def test_actual_nonlinux_preflight_never_admits_private_setup_or_creates_destination(tmp_path, monkeypatch):
    root, request = request_for(tmp_path, monkeypatch, "preflight")
    monkeypatch.setattr(destination.platform, "system", lambda: "Darwin")
    def forbidden(*args): raise AssertionError("Non-Linux preflight must not probe a user service manager")
    monkeypatch.setattr(destination, "_probe", forbidden)
    monkeypatch.setattr(destination.platform, "machine", forbidden)
    monkeypatch.setattr(destination.platform, "libc_ver", forbidden)
    value = destination.handle(request)
    assert value["host"]["platform"] == "darwin" and value["ready"] is False
    assert value["status"] == "preflight" and not root.exists()
    assert next(check for check in value["checks"] if check["name"] == "linux")["status"] == "failed"


@pytest.mark.parametrize("machine,libc,expected", [
    ("x86_64", ("glibc", "2.31"), "x86_64-unknown-linux-gnu"),
    ("AMD64", ("glibc", "2.36"), "x86_64-unknown-linux-gnu"),
    ("aarch64", ("glibc", "2.31"), "aarch64-unknown-linux-gnu"),
    ("arm64", ("glibc", "2.31"), "aarch64-unknown-linux-gnu"),
    ("x86_64", ("glibc", "2.31.0"), "x86_64-unknown-linux-gnu"),
    ("x86_64", ("glibc", "3.0"), "x86_64-unknown-linux-gnu"),
    ("x86_64", ("glibc", "2.30"), None),
    ("x86_64", ("glibc", "2"), None),
    ("x86_64", ("glibc", ""), None),
    ("x86_64", ("glibc", "not-a-version"), None),
    ("x86_64", ("musl", "1.2.5"), None),
    ("x86_64", ("", ""), None),
    ("x86_64", ("unknown", "2.36"), None),
    ("i686", ("glibc", "2.36"), None),
    ("riscv64", ("glibc", "2.36"), None),
    ("", ("glibc", "2.36"), None),
])
def test_remote_preflight_matches_actual_bootstrap_and_generated_gateway_installer(tmp_path, monkeypatch, machine, libc, expected):
    root, request = request_for(tmp_path, monkeypatch, "preflight")
    files, _, _ = accepted_package()
    package = tmp_path / "gateway-source"
    package.mkdir()
    for name in ("deployment.json", "install_gateway.py"):
        (package / name).write_bytes(files[name][0])
    spec = importlib.util.spec_from_file_location("preflight_gateway_parity", package / "install_gateway.py")
    gateway = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(gateway)
    monkeypatch.setattr(destination.platform, "system", lambda: "Linux")
    monkeypatch.setattr(destination.platform, "machine", lambda: machine)
    monkeypatch.setattr(destination.platform, "libc_ver", lambda: libc)
    monkeypatch.setattr(destination, "_probe", lambda *args: True)
    monkeypatch.setattr(destination, "_lingering", lambda *args: None)
    value = destination.handle(request)
    assert value["status"] == "preflight" and value["ready"] is (expected is not None)
    if expected is None:
        with pytest.raises(bootstrap.GatewayInstallError):
            bootstrap.platform_target()
        with pytest.raises(SystemExit):
            gateway.platform_target()
        assert any(check["status"] == "failed" for check in value["checks"]
                   if check["name"] in {"gateway-architecture", "gateway-libc"})
    else:
        assert bootstrap.platform_target() == gateway.platform_target() == expected
        assert all(check["status"] == "passed" for check in value["checks"]
                   if check["name"] in {"gateway-architecture", "gateway-libc"})
    assert not root.exists() and not (root.parents[1] / ".local").exists()


def test_inline_controller_bootstrap_has_real_framing_and_exact_source_binding(tmp_path):
    # This is an actual isolated local Python process, never SSH or a Linux
    # deployment. Preflight is read-only; no credentials or native jobs enter it.
    _, _, binding = accepted_package()
    root = tmp_path / "new-project"
    request = {"action": "preflight", "deployment_id": str(uuid.uuid4()), "binding": binding,
               "destination": str(root)}
    content = Path(destination.__file__).read_bytes()
    value = subprocess.run([sys.executable, "-I", "-c", controller_program(content)],
        input=json.dumps(request).encode(), stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=10, check=True)
    assert value.stderr == b"" and value.stdout.count(b"\n") == 1
    prefix = destination.FRAME_PREFIX.encode()
    assert value.stdout.startswith(prefix)
    frame = json.loads(value.stdout[len(prefix):])
    assert frame["protocol"] == 1 and frame["binding"] == binding
    assert frame["deployment_id"] == request["deployment_id"]
    assert not root.exists() and "settings" not in frame
