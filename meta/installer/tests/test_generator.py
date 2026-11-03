"""Deployment artifact boundaries: valid config, safe paths and executable tools."""
from __future__ import annotations

import ast
import hashlib
import importlib.util
import io
import json
from pathlib import Path
import subprocess
import sys
import tomllib
import zipfile

import pytest

from launchkit.generator import MOBKIT_VERSION, build_archive, generate_files


@pytest.fixture
def spec():
    return {
        "project": {"name": 'My "helpful" team', "slug": "my-team", "description": "Organize work and retrieve the relevant context from connected sources."},
        "agents": [
            {"id": "lead", "name": "Coordinator", "role": "Coordinate the work", "instructions": "Ask your specialist for research and synthesize clear answers."},
            {"id": "research", "name": "Researcher", "role": "Find useful evidence", "instructions": "Inspect connected documents and cite your evidence honestly."},
        ],
        "connections": ["workspace", "github", "telegram"],
        "provider": "openai", "model": "gpt-5.5", "target": "mac",
    }


def extract_to(tmp_path, spec):
    files = generate_files(spec)
    for name, content in files.items():
        destination = tmp_path / name
        destination.parent.mkdir(parents=True, exist_ok=True)
        destination.write_text(content)
    return files


def load_connectors(tmp_path, spec):
    extract_to(tmp_path, spec)
    module_spec = importlib.util.spec_from_file_location("generated_connectors", tmp_path / "connectors.py")
    module = importlib.util.module_from_spec(module_spec)
    module_spec.loader.exec_module(module)
    return module, module.ConnectorTools(tmp_path, json.loads((tmp_path / "deployment.json").read_text()))


@pytest.mark.parametrize("target", ["mac", "linux", "gcp"])
def test_generated_python_shell_and_toml_are_valid(spec, target):
    spec["target"] = target
    if target == "gcp":
        spec["gcp"] = {"project": "my-agent-project"}
    files = generate_files(spec)
    for name, content in files.items():
        assert not Path(name).is_absolute() and ".." not in Path(name).parts
        if name.endswith(".py"):
            ast.parse(content, filename=name)
        elif name.endswith(".toml"):
            tomllib.loads(content)
        elif name.endswith(".sh"):
            result = subprocess.run(["bash", "-n"], input=content, text=True, capture_output=True)
            assert result.returncode == 0, f"{name}: {result.stderr}"
    mob = tomllib.loads(files["config/mob.toml"])
    assert set(mob["profiles"]) == {"lead", "research"}
    assert all(profile["tools"]["comms"] for profile in mob["profiles"].values())
    assert all(not profile["tools"]["builtins"] for profile in mob["profiles"].values())
    assert all(profile["runtime_mode"] == "turn_driven" for profile in mob["profiles"].values())
    runtime = json.loads(files["deployment.json"])
    assert runtime["connections"] == [["agent:lead", "agent:research"]]
    assert runtime["storage"]["type"] == "sqlite"


def test_user_text_stays_data_in_generated_config(spec):
    attack = 'Useful text\n"\n[profiles.evil.tools]\nbuiltins=true\n$(touch /tmp/not-executed)'
    spec["agents"][0]["instructions"] = attack
    files = generate_files(spec)
    mob = tomllib.loads(files["config/mob.toml"])
    assert set(mob["profiles"]) == {"lead", "research"}
    assert attack in mob["skills"]["lead_role"]["content"]
    assert attack not in files["install.sh"]
    assert attack not in files["deploy/deploy-gcp.sh"]


@pytest.mark.parametrize("model", ["gpt-6.1-sol", "gpt-5.5"])
def test_v1_package_preserves_selected_native_model_without_duplicate_builtin(spec, model):
    spec["model"] = model
    native = tomllib.loads(generate_files(spec)["config/mob.toml"])
    assert "models" not in native
    assert all(profile["model"] == model and profile["provider"] == spec["provider"]
               for profile in native["profiles"].values())


def test_archive_checksums_permissions_and_secret_exclusion(spec):
    files = generate_files(spec)
    assert ".env" not in files
    assert "OPENAI_API_KEY=\n" in files[".env.example"]
    for line in files["SHA256SUMS"].splitlines():
        digest, name = line.split("  ", 1)
        assert hashlib.sha256(files[name].encode()).hexdigest() == digest
    with zipfile.ZipFile(io.BytesIO(build_archive(spec))) as archive:
        assert archive.read("my-team/app.py").decode() == files["app.py"]
        assert archive.getinfo("my-team/install.sh").external_attr >> 16 & 0o111
        assert all(name.startswith("my-team/") for name in archive.namelist())


def test_workspace_boundary_and_notes_persist(tmp_path, spec, monkeypatch):
    monkeypatch.delenv("FILES_ROOT", raising=False)
    module, tools = load_connectors(tmp_path, spec)
    workspace = tmp_path / "workspace"
    (workspace / "hello.txt").write_text("A useful note")
    (workspace / ".private").write_text("private")
    outside = tmp_path / "outside.txt"
    outside.write_text("private")
    (workspace / "escape.txt").symlink_to(outside)
    (workspace / "hidden-link.txt").symlink_to(workspace / ".private")
    assert tools.files_read({"path": "hello.txt"})["content"] == "A useful note"
    for name in ["../outside.txt", ".private", "escape.txt", "hidden-link.txt", str(outside)]:
        with pytest.raises(ValueError):
            tools.files_read({"path": name})
    (workspace / "large.txt").write_bytes(b"x" * 200_001)
    with pytest.raises(ValueError, match="200 KB"):
        tools.files_read({"path": "large.txt"})
    content = "Robert'); DROP TABLE notes;--"
    tools.notes_save({"content": content})
    reopened = module.ConnectorTools(tmp_path, json.loads((tmp_path / "deployment.json").read_text()))
    assert reopened.notes_search({"query": "Robert"})["notes"][0]["content"] == content


def test_dotenv_is_data_not_shell(tmp_path, spec, monkeypatch):
    module, _ = load_connectors(tmp_path, spec)
    monkeypatch.delenv("TEST_VALUE", raising=False)
    marker = tmp_path / "should-not-exist"
    env_file = tmp_path / ".env"
    value = f"$(touch {marker}) `touch {marker}`"
    env_file.write_text("TEST_VALUE=" + json.dumps(value) + "\n")
    module.load_environment(env_file)
    import os
    assert os.environ["TEST_VALUE"] == value
    assert not marker.exists()


def test_missing_connector_credentials_fail_loudly(tmp_path, spec, monkeypatch):
    _, tools = load_connectors(tmp_path, spec)
    monkeypatch.delenv("GITHUB_TOKEN", raising=False)
    with pytest.raises(ValueError, match="GITHUB_TOKEN"):
        tools.required("GITHUB_TOKEN")


def test_gcp_dry_run_executes_no_cloud_commands(tmp_path, spec):
    spec.update(target="gcp", gcp={"project": "my-agent-project"})
    extract_to(tmp_path, spec)
    result = subprocess.run(["bash", str(tmp_path / "deploy/deploy-gcp.sh"), "--dry-run"], text=True, capture_output=True)
    assert result.returncode == 0, result.stderr
    assert "No cloud resources have been changed" in result.stdout
    assert "my-agent-project" in result.stdout


def load_installer(tmp_path, spec):
    extract_to(tmp_path, spec)
    module_spec = importlib.util.spec_from_file_location("generated_install_gateway", tmp_path / "install_gateway.py")
    module = importlib.util.module_from_spec(module_spec)
    module_spec.loader.exec_module(module)
    return module


def gateway_archive(name="rpc_gateway", *, symlink=False):
    import tarfile
    buffer = io.BytesIO()
    with tarfile.open(fileobj=buffer, mode="w:gz") as archive:
        info = tarfile.TarInfo(name)
        if symlink:
            info.type = tarfile.SYMTYPE
            info.linkname = "/tmp/elsewhere"
            archive.addfile(info)
        else:
            payload = b"verified binary fixture"
            info.size = len(payload)
            archive.addfile(info, io.BytesIO(payload))
    return buffer.getvalue()


def fake_downloads(monkeypatch, module, payload, checksum=None, maintenance=None, maintenance_checksum=None):
    monkeypatch.delenv("MOBKIT_RPC_GATEWAY_BIN", raising=False)
    monkeypatch.setattr(module.platform, "system", lambda: "Darwin")
    monkeypatch.setattr(module.platform, "machine", lambda: "arm64")
    checksum = checksum or hashlib.sha256(payload).hexdigest()
    name = f"mobkit-rpc-gateway-{MOBKIT_VERSION}-aarch64-apple-darwin.tar.gz"
    maintenance_name = f"mobkit-gateway-{MOBKIT_VERSION}-aarch64-apple-darwin.tar.gz"
    maintenance = maintenance if maintenance is not None else gateway_archive("mobkit_gateway")
    maintenance_checksum = maintenance_checksum or hashlib.sha256(maintenance).hexdigest()
    manifest = (checksum + "  " + name + "\n" + maintenance_checksum + "  " + maintenance_name + "\n").encode()
    calls = []
    def download(url, limit):
        calls.append((url, limit))
        if url.endswith("checksums.sha256"):
            return manifest
        return maintenance if url.endswith("/" + maintenance_name) else payload
    monkeypatch.setattr(module, "download", download)
    return calls


def test_gateway_installer_verifies_bytes_before_install(tmp_path, spec, monkeypatch):
    module = load_installer(tmp_path, spec)
    calls = fake_downloads(monkeypatch, module, gateway_archive())
    module.main()
    installed = tmp_path / ".bin/rpc_gateway"
    assert installed.read_bytes() == b"verified binary fixture"
    assert installed.stat().st_mode & 0o777 == 0o700
    maintenance = tmp_path / ".bin/mobkit_gateway"
    assert maintenance.read_bytes() == b"verified binary fixture"
    assert maintenance.stat().st_mode & 0o777 == 0o700
    assert len(calls) == 3
    assert calls[0][1] == module.MAX_MANIFEST_BYTES
    for name in ["rpc_gateway", "mobkit_gateway"]:
        receipt_path = tmp_path / ".bin" / (name + ".receipt.json")
        receipt = json.loads(receipt_path.read_text())
        assert receipt_path.stat().st_mode & 0o777 == 0o600
        assert receipt["version"] == MOBKIT_VERSION
        assert receipt["binary_sha256"] == hashlib.sha256(b"verified binary fixture").hexdigest()
        assert len(receipt["archive_sha256"]) == 64


@pytest.mark.parametrize("name,symlink", [("../rpc_gateway", False), ("rpc_gateway", True)])
def test_gateway_installer_rejects_archive_traversal_and_links(tmp_path, spec, monkeypatch, name, symlink):
    module = load_installer(tmp_path, spec)
    fake_downloads(monkeypatch, module, gateway_archive(name, symlink=symlink))
    with pytest.raises(SystemExit, match="structure"):
        module.main()
    assert not (tmp_path / ".bin/rpc_gateway").exists()


def test_gateway_installer_rejects_checksum_mismatch(tmp_path, spec, monkeypatch):
    module = load_installer(tmp_path, spec)
    fake_downloads(monkeypatch, module, gateway_archive(), checksum="0" * 64)
    with pytest.raises(SystemExit, match="checksum mismatch"):
        module.main()
    assert not (tmp_path / ".bin/rpc_gateway").exists()


@pytest.mark.parametrize("maintenance,checksum,error", [
    (gateway_archive("../mobkit_gateway"), None, "structure"),
    (gateway_archive("mobkit_gateway", symlink=True), None, "structure"),
    (gateway_archive("rpc_gateway"), None, "structure"),
    (gateway_archive("mobkit_gateway"), "0" * 64, "checksum mismatch"),
    (b"not a gzip archive", None, "could not be read"),
])
def test_maintenance_failure_preserves_both_existing_binaries(tmp_path, spec, monkeypatch, maintenance, checksum, error):
    module = load_installer(tmp_path, spec)
    directory = tmp_path / ".bin"
    directory.mkdir()
    for name in ["rpc_gateway", "mobkit_gateway"]:
        (directory / name).write_bytes(b"previous verified installation")
    fake_downloads(monkeypatch, module, gateway_archive(), maintenance=maintenance, maintenance_checksum=checksum)
    with pytest.raises(SystemExit, match=error):
        module.main()
    assert all((directory / name).read_bytes() == b"previous verified installation" for name in ["rpc_gateway", "mobkit_gateway"])


def test_rpc_override_still_installs_verified_official_maintenance(tmp_path, spec, monkeypatch):
    module = load_installer(tmp_path, spec)
    calls = fake_downloads(monkeypatch, module, gateway_archive())
    selected = tmp_path / "selected-gateway"
    selected.write_bytes(b"explicitly selected rpc binary")
    selected.chmod(0o700)
    monkeypatch.setenv("MOBKIT_RPC_GATEWAY_BIN", str(selected))
    module.main()
    assert (tmp_path / ".bin/rpc_gateway").read_bytes() == selected.read_bytes()
    assert (tmp_path / ".bin/mobkit_gateway").read_bytes() == b"verified binary fixture"
    assert len(calls) == 2 and all("mobkit-rpc-gateway" not in url for url, _ in calls)
    receipt = json.loads((tmp_path / ".bin/rpc_gateway.receipt.json").read_text())
    assert receipt["source"] == "explicit_override"
    assert "archive_sha256" not in receipt
    maintenance = json.loads((tmp_path / ".bin/mobkit_gateway.receipt.json").read_text())
    assert maintenance["archive"].startswith(f"mobkit-gateway-{MOBKIT_VERSION}-")
    assert len(maintenance["archive_sha256"]) == 64


@pytest.mark.parametrize("libc,version,compatible", [("glibc", "2.31", True), ("glibc", "2.40", True), ("glibc", "2.30", False), ("musl", "1.2", False), ("glibc", "unknown", False), ("glibc", "", False)])
def test_generated_installer_checks_minimum_linux_glibc(tmp_path, spec, monkeypatch, libc, version, compatible):
    module = load_installer(tmp_path, spec)
    monkeypatch.setattr(module.platform, "system", lambda: "Linux")
    monkeypatch.setattr(module.platform, "machine", lambda: "x86_64")
    monkeypatch.setattr(module.platform, "libc_ver", lambda: (libc, version))
    if compatible:
        assert module.platform_target() == "x86_64-unknown-linux-gnu"
    else:
        with pytest.raises(SystemExit, match="glibc 2.31"):
            module.platform_target()


def test_download_bounds_response_and_rejects_insecure_redirect(tmp_path, spec, monkeypatch):
    module = load_installer(tmp_path, spec)
    class Response(io.BytesIO):
        url = "https://release.example/archive"
        def geturl(self):
            return self.url
    monkeypatch.setattr(module.urllib.request, "urlopen", lambda *args, **kwargs: Response(b"123456"))
    with pytest.raises(SystemExit, match="size limit"):
        module.download("https://release.example/archive", 5)
    assert module.download("https://release.example/archive", 6) == b"123456"
    Response.url = "http://release.example/archive"
    with pytest.raises(SystemExit, match="insecure"):
        module.download("https://release.example/archive", 6)


@pytest.mark.parametrize("limit_name,limit,error", [
    ("MAX_MANIFEST_BYTES", 10, "manifest exceeded"),
    ("MAX_ARCHIVE_BYTES", 10, "archive exceeded"),
    ("MAX_BINARY_BYTES", 10, "binary size"),
    ("MAX_TAR_OVERHEAD", 100, "decompressed size limit"),
])
def test_generated_installer_bounds_every_artifact_stage(tmp_path, spec, monkeypatch, limit_name, limit, error):
    module = load_installer(tmp_path, spec)
    fake_downloads(monkeypatch, module, gateway_archive())
    monkeypatch.setattr(module, limit_name, limit)
    if limit_name == "MAX_TAR_OVERHEAD":
        monkeypatch.setattr(module, "MAX_BINARY_BYTES", 100)
    with pytest.raises(SystemExit, match=error):
        module.main()
    assert not (tmp_path / ".bin/rpc_gateway").exists()
    assert not (tmp_path / ".bin/mobkit_gateway").exists()


@pytest.mark.parametrize("digest", ["z" * 64, "a" * 63])
def test_manifest_digest_must_be_sha256_hex(tmp_path, spec, monkeypatch, digest):
    module = load_installer(tmp_path, spec)
    fake_downloads(monkeypatch, module, gateway_archive(), checksum=digest)
    with pytest.raises(SystemExit, match="manifest must name"):
        module.main()
    assert not (tmp_path / ".bin").exists()


@pytest.mark.asyncio
async def test_github_connector_is_scoped_to_configured_repo(tmp_path, spec, monkeypatch):
    _, tools = load_connectors(tmp_path, spec)
    monkeypatch.setenv("GITHUB_REPOSITORY", "example/project")
    monkeypatch.setenv("GITHUB_TOKEN", "test-token")
    captured = []
    def request(url, headers, payload=None):
        captured.append((url, headers, payload))
        return [{"number": 3, "title": "A task", "body": "Review it", "html_url": "https://github.com/example/project/issues/3", "state": "open"}]
    monkeypatch.setattr(tools, "request", request)
    result = await tools.github_issues({"state": "closed"})
    assert result["issues"][0]["number"] == 3
    assert captured[0][0] == "https://api.github.com/repos/example/project/issues?per_page=30&state=closed"
    assert captured[0][2] is None
    monkeypatch.setenv("GITHUB_REPOSITORY", "example/project/../../escape")
    with pytest.raises(ValueError, match="owner/repository"):
        await tools.github_issues({})
    assert len(captured) == 1


@pytest.mark.asyncio
async def test_smoke_retries_observation_without_redelivering(tmp_path, spec, monkeypatch):
    from meerkat_mobkit.errors import RpcError
    from types import SimpleNamespace
    connectors, _ = load_connectors(tmp_path, spec)
    monkeypatch.setitem(sys.modules, "connectors", connectors)
    monkeypatch.syspath_prepend(str(tmp_path))
    module_spec = importlib.util.spec_from_file_location("generated_app", tmp_path / "app.py")
    app = importlib.util.module_from_spec(module_spec)
    module_spec.loader.exec_module(app)
    class Agent:
        sends = 0
        observations = 0
        async def send(self, text):
            self.sends += 1
            return SimpleNamespace(completion_baseline="exact-baseline")
        async def wait_for_completion(self, baseline, **kwargs):
            assert baseline == "exact-baseline"
            self.observations += 1
            if self.observations == 1:
                raise RpcError(-32000, "observation_lane_saturated: member_status_observation")
            return "Hello, ready."
    agent = Agent()
    assert await app.smoke_request(agent, timeout=5) == "Hello, ready."
    assert agent.sends == 1
    assert agent.observations == 2


def test_non_bmp_names_and_prompts_are_valid_toml(spec):
    spec["project"]["name"] = "My team \U0001f331"
    spec["agents"][0]["instructions"] = "Help with the garden \U0001f33b and keep clear notes."
    parsed = tomllib.loads(generate_files(spec)["config/mob.toml"])
    assert "\U0001f33b" in parsed["skills"]["lead_role"]["content"]


def test_app_reports_gateway_errors_without_raw_traceback(tmp_path, spec, monkeypatch):
    from meerkat_mobkit.errors import RpcError
    connectors, _ = load_connectors(tmp_path, spec)
    monkeypatch.setitem(sys.modules, "connectors", connectors)
    monkeypatch.syspath_prepend(str(tmp_path))
    module_spec = importlib.util.spec_from_file_location("generated_app_failure", tmp_path / "app.py")
    app = importlib.util.module_from_spec(module_spec)
    module_spec.loader.exec_module(app)
    async def failed(*args, **kwargs):
        raise RpcError(-32014, "Gateway storage needs repair")
    monkeypatch.setattr(app, "run", failed)
    monkeypatch.setattr(sys, "argv", ["app.py"])
    monkeypatch.chdir(tmp_path)
    with pytest.raises(RuntimeError, match="MobKit could not complete startup or the request: Gateway storage needs repair"):
        app.main()
