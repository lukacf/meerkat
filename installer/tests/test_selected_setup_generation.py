"""Selected receiver storage stays separate from retained Guide state."""
from unittest.mock import Mock

import pytest

from launchkit import architect, bootstrap, guide_sdk, receiver_sdk, role_runtime
from launchkit.guide_transport import SelectedGuideTransport
from launchkit.provider_settings import canonical_setup_settings
from test_architect_startup_recovery import startup
from test_provider_settings import native_spec


def select_receiver(monkeypatch):
    selected = SelectedGuideTransport(Mock(name="unused_transport"),
        receiver_sdk.SELECTED_VERSION, receiver_sdk.declared_core_version(),
        "a" * 40, "b" * 40, {"provider": "openai", "model": "gpt-6.1-sol"})
    monkeypatch.setattr(bootstrap, "_selected_guide_runtime", None)
    bootstrap.configure_selected_guide_runtime(selected)
    return selected


def test_selected_receiver_controls_state_and_settings_without_changing_legacy(tmp_path, monkeypatch):
    monkeypatch.setattr(guide_sdk, "SELECTED_VERSION", "0.9.3")
    selected = select_receiver(monkeypatch)
    previous = tmp_path / "architect-runtime-meta-v1"
    current = tmp_path / "architect-runtime-meta-v2"
    database = native_spec(previous, provider="anthropic")
    before, inode = database.read_bytes(), database.stat().st_ino
    assert architect.setup_runtime_root(tmp_path) == current
    assert architect.setup_settings_root(tmp_path) == previous
    assert canonical_setup_settings(architect.setup_settings_root(tmp_path))["provider"] == "anthropic"
    native_spec(current, provider="openai")
    assert architect.setup_settings_root(tmp_path) == current
    assert canonical_setup_settings(architect.setup_settings_root(tmp_path))["provider"] == "openai"
    assert database.read_bytes() == before and database.stat().st_ino == inode
    selected.factory.assert_not_called()


@pytest.mark.asyncio
async def test_architect_passes_selected_generation_to_admission_before_any_native_build(startup, monkeypatch):
    service, controller = startup
    monkeypatch.setattr(guide_sdk, "SELECTED_VERSION", "0.9.3")
    selected = select_receiver(monkeypatch)
    previous = service.root / "architect-runtime-meta-v1"
    database = native_spec(previous, provider="anthropic")
    before, inode = database.read_bytes(), database.stat().st_ino
    observed = []

    async def inspect_admission(role, original, state_path, cleanup):
        assert role == "architect" and original is selected
        observed.append(state_path)
        assert state_path == service.root / "architect-runtime-meta-v2/.mobkit/state"
        raise RuntimeError("inert admission boundary reached")

    monkeypatch.setattr(role_runtime, "prepare_runtime", inspect_admission)
    with pytest.raises(RuntimeError, match="inert admission boundary reached"):
        await service.start()
    assert len(observed) == 1 and controller.builders == controller.builds == 0
    assert database.read_bytes() == before and database.stat().st_ino == inode
    selected.factory.assert_not_called()


def test_invalid_selected_runtime_is_not_replaced_by_bundled_guide(tmp_path, monkeypatch):
    selected = select_receiver(monkeypatch)
    object.__setattr__(selected, "factory", Mock(name="changed_transport"))
    with pytest.raises(bootstrap.GatewayInstallError, match="original guide transport"):
        architect.setup_runtime_root(tmp_path)
    assert not list(tmp_path.iterdir())
