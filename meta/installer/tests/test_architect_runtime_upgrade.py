"""A Meta setup start leaves a retained MobKit setup store untouched."""
from contextlib import contextmanager
import hashlib
from types import SimpleNamespace

import pytest

from launchkit import architect, bootstrap
from launchkit.connect_installer_caller import CustodyError
from launchkit.templates import storage_setup
from test_architect_startup_recovery import FakeRuntime, startup
from test_provider_settings import native_spec


TARGET = "aarch64-apple-darwin"
PROJECT_LOCK = storage_setup.project_lock


def mock_binary(path, version, name):
    path.parent.mkdir(parents=True, mode=0o700, exist_ok=True)
    content = ("Never executed: " + name + ":" + version).encode()
    path.write_bytes(content)
    path.chmod(0o700)
    prefix = {"rpc_gateway": "mobkit-rpc-gateway", "mobkit_gateway": "mobkit-gateway"}[name]
    value = {"version": version, "archive": f"{prefix}-{version}-{TARGET}.tar.gz",
             "release_url": f"https://github.com/lukacf/meerkat-mobkit/releases/download/v{version}",
             "archive_sha256": "a" * 64, "binary_sha256": hashlib.sha256(content).hexdigest()}
    if version in {"0.8.46", "0.8.47"}:
        value["core_version"] = "0.8.51"
    receipt = path.with_name(name + ".receipt.json")
    storage_setup.write_private(receipt, value)
    return path


@pytest.fixture
def crossing(tmp_path, monkeypatch):
    """A retained MobKit 0.8.42 setup store that a Meta setup start must leave alone."""
    monkeypatch.delenv("MOBKIT_GATEWAY_BIN", raising=False)
    monkeypatch.delenv("MOBKIT_RPC_GATEWAY_BIN", raising=False)
    root = tmp_path / "saved-state/architect-runtime-v2"
    root.mkdir(parents=True, mode=0o700)
    (root / ".bin").mkdir(mode=0o700)
    (root / ".mobkit/state").mkdir(parents=True, mode=0o700)
    (root / ".mobkit").chmod(0o700)
    history = root / ".mobkit/state/event_log.sqlite3"
    history.write_bytes(b"Fixture-owned saved native history, never opened as SQLite.")
    old = mock_binary(tmp_path / "original-package/.tools/mobkit_gateway", "0.8.42", "mobkit_gateway")
    (root / ".bin/mobkit_gateway").symlink_to(old)
    storage_setup.write_private(root / "deployment.json", {"slug": "launch-architect", "mobkitVersion": "0.8.42"})
    storage_setup.write_private(root / ".mobkit/launch-storage.json",
        {"format": 1, "project": "launch-architect", "version": "0.8.42", "phase": "ready"})
    return SimpleNamespace(root=root, old=old, history=history, events=[], leased=False,
                           original_history=history.read_bytes(), original_history_inode=history.stat().st_ino)


def retained(fixture):
    return fixture.root / ".mobkit" / architect.SETUP_UPGRADE_RECEIPT


def assert_history_unchanged(fixture):
    assert fixture.history.read_bytes() == fixture.original_history
    assert fixture.history.stat().st_ino == fixture.original_history_inode
    assert fixture.old.read_bytes().endswith(b"0.8.42")


def setup_service_for_crossing(startup, fixture, monkeypatch):
    service, controller = startup
    service.root = fixture.root.parent
    fresh_root = architect.setup_runtime_root(service.root)
    assert fresh_root != fixture.root

    @contextmanager
    def fresh_lease(root):
        assert root == fresh_root
        with PROJECT_LOCK(root):
            fixture.leased = True
            fixture.events.append("lease-acquired")
            try:
                yield
            finally:
                fixture.leased = False
                fixture.events.append("lease-released")

    monkeypatch.setattr(storage_setup, "project_lock", fresh_lease)
    async def catalog():
        assert fixture.leased
        fixture.events.append("toolkit-catalog")
    async def setup():
        assert fixture.leased
        fixture.events.append("toolkit-setup")
    async def inspect(*args):
        pytest.fail("Fresh Meta init must not inspect or upgrade the predecessor actor store")
    async def input_capability(**kwargs):
        return None
    monkeypatch.setattr(service.toolkit_catalog, "start", catalog)
    monkeypatch.setattr(service.toolkit_setup, "start", setup)
    monkeypatch.setattr(storage_setup, "prepare_storage", inspect)
    monkeypatch.setattr(service, "refresh_input_capability", input_capability)
    controller.outcomes = [FakeRuntime()]
    return service, controller


@pytest.mark.asyncio
async def test_meta_start_preserves_legacy_state_and_uses_separately_selected_gateway(startup, crossing, monkeypatch):
    service, controller = setup_service_for_crossing(startup, crossing, monkeypatch)
    selected = bootstrap.selected_guide_runtime()
    assert (await service.start())["status"] == "ready"
    assert crossing.events == ["lease-acquired", "toolkit-catalog", "toolkit-setup"]
    assert controller.builds == 1 and controller.gateways == [str(selected.gateway.path)]
    assert not retained(crossing).exists()
    assert_history_unchanged(crossing)
    await service.shutdown()
    assert crossing.events[-1] == "lease-released"


@pytest.mark.asyncio
async def test_selected_guide_drift_refuses_build_without_touching_predecessor(startup, crossing, monkeypatch):
    service, controller = setup_service_for_crossing(startup, crossing, monkeypatch)
    selected = bootstrap.selected_guide_runtime()
    async def changed_after_selection():
        selected.gateway.path.write_bytes(b"Changed after exact guide selection.")
    monkeypatch.setattr(service.toolkit_setup, "start", changed_after_selection)
    with pytest.raises((RuntimeError, CustodyError)):
        await service.start()
    assert controller.builds == 0 and not crossing.leased
    assert crossing.events == ["lease-acquired", "toolkit-catalog", "lease-released"]
    assert not retained(crossing).exists()
    assert_history_unchanged(crossing)


@pytest.mark.asyncio
async def test_live_project_lease_prevents_download_rebind_and_builder(startup, monkeypatch):
    service, controller = startup
    root = architect.setup_runtime_root(service.root)
    root.mkdir(parents=True, mode=0o700)
    monkeypatch.setattr(storage_setup, "project_lock", PROJECT_LOCK)
    with PROJECT_LOCK(root):
        with pytest.raises(RuntimeError, match="already running"):
            await service.start()
    assert controller.builders == controller.builds == 0
    assert not (root / "deployment.json").exists()
    assert not (root / ".bin/mobkit_gateway").is_symlink()
