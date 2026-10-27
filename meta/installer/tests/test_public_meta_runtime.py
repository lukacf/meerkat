"""Compiled agent teams can install the public Meta release instead of MobKit 0.8."""
import hashlib
import importlib.util
import io
import json
from pathlib import Path
import sys
import tarfile
from types import SimpleNamespace
import urllib.error

import pytest

from launchkit import compiler, public_meta_runtime
from launchkit.blueprint import starter_blueprint
from launchkit.schema import DEFAULT_MODELS

VERSION = public_meta_runtime.VERSION
FIXTURES = Path(__file__).parent / "fixtures/meta_release_0_9_4"
TEMPLATES = Path(__file__).resolve().parents[1] / "launchkit/templates"
UNIX_TARGETS = ("x86_64-unknown-linux-gnu", "aarch64-unknown-linux-gnu", "aarch64-apple-darwin",
    "x86_64-apple-darwin")


def plan(**classes):
    value = starter_blueprint().model_dump(mode="json")
    for key, item in classes.items():
        value["agent_classes"][0][key] = item
    return value


def public_files(spec=None):
    return {item.path: item for item in compiler.generate_package(spec or plan(), public_runtime=True)}


def manifest_rows(name):
    return [line.split() for line in (FIXTURES / name).read_text().splitlines() if line.strip()]


def gateway_installer(tmp_path, monkeypatch, target, version=VERSION):
    (tmp_path / "install_gateway.py").write_text((TEMPLATES / "install_gateway.py").read_text())
    (tmp_path / "deployment.json").write_text(json.dumps({"mobkitVersion": version}))
    spec = importlib.util.spec_from_file_location("isolated_meta_gateway_installer", tmp_path / "install_gateway.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    monkeypatch.delenv("MOBKIT_RPC_GATEWAY_BIN", raising=False)
    monkeypatch.setattr(module, "platform_target", lambda: target)
    return module


def test_recorded_catalog_is_the_pinned_release_catalog():
    catalog = public_meta_runtime.model_catalog()
    assert catalog["version"] == VERSION == public_meta_runtime.VERSION
    source = catalog["source"]
    archive = f"meta-rpc-gateway-{VERSION}-x86_64-unknown-linux-gnu.tar.gz"
    listed = [digest for digest, name in manifest_rows("checksums.sha256") if name.lstrip("*") == archive]
    # Recorded from this release's own checksum-listed gateway and the pinned wheel.
    assert (source["archive"], [source["archive_sha256"]]) == (archive, listed)
    assert source["sdk_wheel_sha256"] == public_meta_runtime.SDK_WHEEL_SHA256
    assert source["release"] == public_meta_runtime.RELEASE_URL
    assert public_meta_runtime.unlisted_models(DEFAULT_MODELS.items()) == []


def test_agent_team_requirements_pin_one_hashed_wheel_beside_the_host_pins():
    files = public_files()
    assert files["requirements.sdk.txt"].content == (
        f"meerkat-meta=={VERSION} --hash=sha256:{public_meta_runtime.SDK_WHEEL_SHA256}\n")
    assert files["requirements.txt"].content == compiler.HOST_REQUIREMENTS
    steps = [line for line in files["install.sh"].content.splitlines() if "-r requirements" in line]
    assert steps[:2] == [
        'prepare_application_dependency_step "installing the pinned Meta SDK" .venv/bin/python -m pip install '
        "--disable-pip-version-check --no-deps --require-hashes -r requirements.sdk.txt",
        'prepare_application_dependency_step "installing pinned dependencies" .venv/bin/python -m pip install '
        "--disable-pip-version-check -r requirements.txt"]
    assert json.loads(files["deployment.json"].content)["mobkitVersion"] == VERSION
    assert "[models." not in files["config/mob.toml"].content
    legacy = {item.path: item for item in compiler.generate_package(plan())}
    assert "requirements.sdk.txt" not in legacy
    assert legacy["requirements.txt"].content.splitlines()[0] == f"meerkat-mobkit=={compiler.MOBKIT_VERSION}"


def test_agent_team_dependencies_urls_and_receipts_name_no_mobkit_runtime(tmp_path, monkeypatch):
    files = public_files()
    for name in ("requirements.txt", "requirements.sdk.txt"):
        assert "mobkit" not in files[name].content.lower() and "0.8.47" not in files[name].content, name
    # deployment.json keeps its mobkitVersion key and .mobkit/ paths; its values name the Meta pin.
    deployment = files["deployment.json"].content
    assert json.loads(deployment)["mobkitVersion"] == VERSION
    assert all(text not in deployment for text in ("meerkat-mobkit", "mobkit-rpc-gateway", "0.8.47"))
    install = files["install.sh"].content
    assert "meerkat-mobkit" not in install and "mobkit_gateway" not in install
    module = gateway_installer(tmp_path, monkeypatch, "x86_64-unknown-linux-gnu")
    assert module.RELEASE_URL == f"https://github.com/lukacf/meerkat-meta/releases/download/v{VERSION}"
    assert all("mobkit" not in prefix for prefix in module.BINARIES.values())
    assert set(module.BINARIES) == {"rpc_gateway", "meta_gateway"}


def test_a_model_the_pinned_catalog_does_not_list_holds_the_package():
    unlisted = plan(provider="anthropic", model="claude-opus-4-6")
    held = compiler.package_availability(unlisted, public_runtime=True)
    reason = public_meta_runtime.models_unlisted_reason([("anthropic", "claude-opus-4-6")])
    assert held == {"available": False, "status": "pending-configuration", "reason": reason}
    assert "claude-opus-4-6" in reason and f"Meta {VERSION}" in reason
    # The reason names models the pinned catalog does list for that provider.
    catalog = public_meta_runtime.model_catalog()
    default = next(row["default_model_id"] for row in catalog["provider_defaults"] if row["provider"] == "anthropic")
    assert f"Listed anthropic models include {default}" in reason
    assert public_meta_runtime.unlisted_models([("anthropic", default)]) == []
    with pytest.raises(ValueError) as refused:
        compiler.generate_package(unlisted, public_runtime=True)
    assert str(refused.value) == reason
    listed = plan(provider="anthropic", model="claude-sonnet-4-6")
    assert compiler.package_availability(listed, public_runtime=True)["available"] is True
    # The legacy compile keeps its own registry; the public catalog does not apply to it.
    assert compiler.package_availability(unlisted)["available"] is True


def test_the_public_runtime_takes_no_selected_product_or_candidate():
    with pytest.raises(ValueError, match="no selected product or candidate"):
        compiler.generate_package(plan(), public_runtime=True, runtime_target=object())
    with pytest.raises(ValueError, match="no selected product or candidate"):
        compiler.package_availability(plan(), public_runtime=True, verified_artifacts=object())


def test_agent_team_download_and_preview_are_the_meta_package():
    # The download (POST /api/blueprint/package) and its preview (POST
    # /api/blueprint/validate) give the package Install agent team installs.
    import zipfile
    spec = plan()
    prefix = spec["project"]["slug"] + "/"
    with zipfile.ZipFile(io.BytesIO(compiler.build_archive(spec))) as archive:
        files = {name.removeprefix(prefix): archive.read(name) for name in archive.namelist()}
    expected = public_files(spec)
    assert json.loads(files["deployment.json"])["mobkitVersion"] == VERSION
    assert b"meerkat-mobkit" not in files["requirements.txt"] and "requirements.sdk.txt" in files
    digests = lambda values: {name: hashlib.sha256(value).hexdigest() for name, value in values.items()}
    assert digests(files) == digests({name: item.as_bytes() for name, item in expected.items()})
    preview = compiler.package_preview(spec)
    assert preview["mobkit_version"] == VERSION
    assert {item["path"] for item in preview["files"]} == set(expected)
    unlisted = plan(provider="anthropic", model="claude-opus-4-6")
    reason = public_meta_runtime.models_unlisted_reason([("anthropic", "claude-opus-4-6")])
    held = compiler.package_preview(unlisted)
    assert held["package_availability"]["reason"] == reason and held["files"] == []
    assert held["checks"] == [{"label": "Model catalog", "status": "pending-configuration", "detail": reason}]
    with pytest.raises(ValueError) as refused:
        compiler.build_archive(unlisted)
    assert str(refused.value) == reason


def test_meta_release_assets_follow_the_recorded_manifest_tiers(tmp_path, monkeypatch):
    tiers = {name: [row[1].lstrip("*") for row in manifest_rows(name)]
        for name in ("checksums.sha256", "checksums-trailing.sha256")}
    reached = set()
    for target in UNIX_TARGETS:
        (tmp_path / target).mkdir()
        module = gateway_installer(tmp_path / target, monkeypatch, target)
        listed = tiers[module.checksum_manifest(target)]
        for prefix in module.BINARIES.values():
            archive = f"{prefix}-{VERSION}-{target}.tar.gz"
            assert listed.count(archive) == 1, (target, archive)
            reached.add(archive)
    assert module.checksum_manifest("x86_64-apple-darwin") == "checksums-trailing.sha256"
    published = {name for names in tiers.values() for name in names
        if name.startswith(("meta-rpc-gateway-", "meta-gateway-")) and name.endswith(".tar.gz")}
    assert published == reached


def official_meta_assets(module, target, monkeypatch):
    assets = {}
    for name, prefix in module.BINARIES.items():
        stream = io.BytesIO()
        with tarfile.open(fileobj=stream, mode="w:gz") as archive:
            binary = b"recorded release layout fixture " + name.encode()
            member = tarfile.TarInfo(name)
            member.size = len(binary)
            archive.addfile(member, io.BytesIO(binary))
        assets[f"{prefix}-{VERSION}-{target}.tar.gz"] = stream.getvalue()
    assets[module.checksum_manifest(target)] = "".join(
        hashlib.sha256(payload).hexdigest() + "  " + name + "\n" for name, payload in assets.items()).encode()
    calls = []
    def download(url, limit, missing=None):
        calls.append(url)
        assert url.startswith(module.RELEASE_URL + "/")
        return assets[url.rsplit("/", 1)[-1]]
    monkeypatch.setattr(module, "download", download)
    return calls


@pytest.mark.parametrize("target", ["x86_64-unknown-linux-gnu", "x86_64-apple-darwin"])
def test_meta_install_reads_the_meta_release_and_writes_its_receipts(tmp_path, monkeypatch, target):
    module = gateway_installer(tmp_path, monkeypatch, target)
    calls = official_meta_assets(module, target, monkeypatch)
    module.main()
    release = f"https://github.com/lukacf/meerkat-meta/releases/download/v{VERSION}/"
    manifest = "checksums-trailing.sha256" if target == "x86_64-apple-darwin" else "checksums.sha256"
    assert calls == [release + manifest, release + f"meta-rpc-gateway-{VERSION}-{target}.tar.gz",
        release + f"meta-gateway-{VERSION}-{target}.tar.gz"]
    assert sorted(path.name for path in (tmp_path / ".bin").iterdir()) == [
        "meta_gateway", "meta_gateway.receipt.json", "rpc_gateway", "rpc_gateway.receipt.json"]
    for name, prefix in module.BINARIES.items():
        receipt = json.loads((tmp_path / ".bin" / (name + ".receipt.json")).read_text())
        assert receipt["version"] == VERSION and receipt["release_url"] == release.rstrip("/")
        assert receipt["archive"] == f"{prefix}-{VERSION}-{target}.tar.gz"
    # The pinned 0.9.4 binaries fit the Meta bound (rpc_gateway is about 471 MiB).
    sizes = json.loads((FIXTURES / "SOURCE.json").read_text())["binaries"]["x86_64-unknown-linux-gnu"]
    assert max(sizes.values()) < module.MAX_BINARY_BYTES


@pytest.mark.parametrize("version,meta", [("0.8.47", False), ("0.9.4", True), ("0.10.0", True), ("1.0.0", True)])
def test_later_meta_versions_keep_the_meta_release(tmp_path, monkeypatch, version, meta):
    module = gateway_installer(tmp_path, monkeypatch, "x86_64-unknown-linux-gnu", version)
    repository = "meerkat-meta" if meta else "meerkat-mobkit"
    assert module.RELEASE_URL == f"https://github.com/lukacf/{repository}/releases/download/v{version}"
    assert set(module.BINARIES) == ({"rpc_gateway", "meta_gateway"} if meta else {"rpc_gateway", "mobkit_gateway"})


def test_intel_mac_names_a_missing_trailing_tier(tmp_path, monkeypatch):
    module = gateway_installer(tmp_path, monkeypatch, "x86_64-apple-darwin")
    requested = []
    def unpublished(request, timeout):
        requested.append(request.full_url)
        raise urllib.error.HTTPError(request.full_url, 404, "Not Found", {}, None)
    monkeypatch.setattr(module.urllib.request, "urlopen", unpublished)
    with pytest.raises(SystemExit) as refused:
        module.main()
    assert requested == [f"{module.RELEASE_URL}/checksums-trailing.sha256"]
    assert "has not published its macOS Intel tools" in str(refused.value.code)
    assert not (tmp_path / ".bin").exists()


@pytest.fixture
def storage():
    spec = importlib.util.spec_from_file_location("generated_meta_storage_setup", TEMPLATES / "storage_setup.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def meta_project(root, plan):
    """A package whose .bin/meta_gateway replays reports recorded from the 0.9.4 tool."""
    (root / "deployment.json").write_text(json.dumps({"slug": "test-team", "mobkitVersion": VERSION}))
    (root / ".bin").mkdir()
    tool = root / ".bin/meta_gateway"
    tool.write_text("\n".join([
        f"#!{sys.executable}", "import json, pathlib, sys",
        "pathlib.Path(__file__).with_name('calls.jsonl').open('a').write(json.dumps(sys.argv[1:]) + '\\n')",
        f"plan = {json.dumps(plan)}", f"fixtures = pathlib.Path({str(FIXTURES / 'storage-reports')!r})",
        "name, code = plan['apply' if '--apply' in sys.argv else 'read']",
        "state = sys.argv[sys.argv.index('--state-dir') + 1]",
        "sys.stdout.write((fixtures / (name + '.json')).read_text().replace('{state_dir}', state))",
        "sys.exit(code)", ""]))
    tool.chmod(0o700)
    def calls():
        log = root / ".bin/calls.jsonl"
        return [json.loads(line) for line in log.read_text().splitlines()] if log.exists() else []
    return calls


def bootable(root, events):
    class Runtime:
        async def identity_bootstrap_status(self):
            return SimpleNamespace(complete=True, ready=True, counts=SimpleNamespace(broken=0))
        async def shutdown(self):
            events.append("stopped")
    async def build():
        events.append("started")
        (root / ".mobkit/state/continuity.sqlite3").write_text("first boot")
        return Runtime()
    return build


def test_storage_tool_follows_the_package_runtime(tmp_path, storage):
    for version, tool, name in ((VERSION, "meta_gateway", "Meta"), ("0.10.0", "meta_gateway", "Meta"),
            ("1.0.0", "meta_gateway", "Meta"), ("0.8.47", "mobkit_gateway", "MobKit")):
        (tmp_path / "deployment.json").write_text(json.dumps({"mobkitVersion": version}))
        native = storage.native_storage(tmp_path)
        assert (native["tool"], native["name"]) == (tool, name)


@pytest.mark.asyncio
async def test_meta_storage_prepares_a_new_project_through_meta_gateway(tmp_path, storage):
    calls = meta_project(tmp_path, {"apply": ["boot-apply", 0], "read": ["boot-dry2", 0]})
    events = []
    await storage.prepare_storage(tmp_path, bootable(tmp_path, events))
    state = str(tmp_path / ".mobkit/state")
    assert events == ["started", "stopped"]
    assert calls() == [["storage-migrate", "--state-dir", state, "--json", "--apply"]]
    marker = json.loads((tmp_path / ".mobkit/launch-storage.json").read_text())
    assert marker == {"format": 1, "project": "test-team", "version": VERSION, "phase": "ready"}
    await storage.prepare_storage(tmp_path, None)
    assert calls()[-1] == ["storage-migrate", "--state-dir", state, "--json"], "Ready projects only read"


@pytest.mark.asyncio
async def test_meta_storage_refuses_unstamped_existing_state_with_the_meta_command(tmp_path, storage):
    calls = meta_project(tmp_path, {"apply": ["boot-apply", 0], "read": ["boot-dry", 0]})
    (tmp_path / ".mobkit/state").mkdir(parents=True)
    (tmp_path / ".mobkit/state/continuity.sqlite3").write_text("restored")
    with pytest.raises(RuntimeError) as refused:
        await storage.prepare_storage(tmp_path, None)
    assert "explicit Meta migration" in str(refused.value)
    assert ".bin/meta_gateway storage-migrate --state-dir .mobkit/state --apply" in str(refused.value)
    assert [call[-1] for call in calls()] == ["--json"]
    assert not (tmp_path / ".mobkit/launch-storage.json").exists()


@pytest.mark.asyncio
async def test_meta_storage_refuses_mobkit_state_and_names_adoption(tmp_path, storage):
    calls = meta_project(tmp_path, {"apply": ["boot-apply", 0], "read": ["predecessor-dry", 1]})
    (tmp_path / ".mobkit/state").mkdir(parents=True)
    (tmp_path / ".mobkit/state/mobkit_metadata.sqlite3").write_text("0.8 state")
    with pytest.raises(RuntimeError) as refused:
        await storage.prepare_storage(tmp_path, None)
    message = str(refused.value)
    assert message == storage.PREDECESSOR_STATE.format(version=VERSION)
    assert "storage-adopt-predecessor" in message and "No data was changed" in message
    assert [call[-1] for call in calls()] == ["--json"]
    assert (tmp_path / ".mobkit/state/mobkit_metadata.sqlite3").read_text() == "0.8 state"
    assert not (tmp_path / ".mobkit/launch-storage.json").exists()
    retained = json.loads((tmp_path / ".mobkit/storage-initialization.json").read_text())
    assert "predecessor (pre-0.9) state" in retained["errors"][0]


def test_readme_names_the_meta_pin():
    readme = public_files()["README.md"].content
    assert f"the pinned Meta {VERSION} SDK, checked against its published SHA-256," in readme
    assert "MobKit" not in readme and "Meerkat/MobKit" not in readme
    assert f"Pinned Meta {VERSION} advertises session, identity, mob and `host_runnable` targets" in readme
