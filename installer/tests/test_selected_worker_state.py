"""Explicit state selection in the actual worker builder paths, without native I/O."""
import asyncio
import json
from pathlib import Path
from unittest.mock import Mock
from types import SimpleNamespace

import pytest

from launchkit import bootstrap, guide_runtime, receiver_sdk
from launchkit.blueprint import starter_blueprint
from launchkit.guide_transport import SelectedGuideTransport
from test_implementation_worker import team



class InertTransportBuilder:
    """State/transport interface specimen; not an implementation of either SDK."""
    def __init__(self):
        self._config = SimpleNamespace(persistent_state=None)
        self.factory = None

    def persistent_state(self, path):
        self._config.persistent_state = path
        return self

    def gateway_transport(self, factory):
        self.factory = factory
        return self

    def mob_inline(self, value): return self
    def roster(self, value): return self
    def topology_provider(self, value): return self
    def agent_memory(self, value): return self
    def session_service(self, value): return self
    def http_listen(self, value): return self
    def console_auth_required(self, value): return self

    async def build(self):
        raise AssertionError("A state-order specimen must stop before runtime startup.")


def selection(**kwargs):
    return SelectedGuideTransport(Mock(name="original_factory"), receiver_sdk.SELECTED_VERSION,
        receiver_sdk.declared_core_version(), "a" * 40, "b" * 40,
        {"provider": "openai", "model": "gpt-6.1-sol"}, **kwargs)


@pytest.mark.parametrize("value", [None, False, "", "/", "relative", "/a/../b", "/a/./b",
                                   "/a//b", "//a/b", "/a/b/", "/a/\n/b", "/a/\x00b", "/" + "a" * 4096])
def test_explicit_state_cannot_be_ephemeral_or_noncanonical(value):
    with pytest.raises(bootstrap.GatewayInstallError, match="canonical absolute"):
        selection(persistent_state=value)


def test_legacy_omission_is_distinct_from_explicit_state_and_rechecked(tmp_path):
    legacy = tmp_path / "native/.meta/state"
    old = selection()
    assert guide_runtime.worker_persistent_state(old, legacy) == str(legacy)
    selected = selection(persistent_state=str(tmp_path / "protected/native/state"))
    assert guide_runtime.worker_persistent_state(selected, legacy) == selected.persistent_state
    assert not list(tmp_path.iterdir()), "selection cannot create or migrate state"
    object.__setattr__(selected, "persistent_state", str(tmp_path / "replacement"))
    with pytest.raises(bootstrap.GatewayInstallError, match="selection changed"):
        guide_runtime.worker_persistent_state(selected, legacy)
    assert not list(tmp_path.iterdir())


@pytest.mark.parametrize("kind,suffix", [("implementation", ".mobkit"), ("rehearsal", ".meta")])
@pytest.mark.parametrize("existing", ["directory", "file", "dangling_symlink"])
def test_conflicting_legacy_state_is_preserved_and_refused(tmp_path, kind, suffix, existing):
    legacy = tmp_path / "native" / suffix / "state"
    legacy.parent.mkdir(parents=True)
    if existing == "directory":
        legacy.mkdir()
        (legacy / "original").write_bytes(b"original saved state")
    elif existing == "file":
        legacy.write_bytes(b"original saved state")
    else:
        legacy.symlink_to(tmp_path / "missing")
    original = legacy.lstat()
    selected = selection(persistent_state=str(tmp_path / "protected/native/state"))
    with pytest.raises(bootstrap.GatewayInstallError, match="saved state is preserved"):
        guide_runtime.worker_persistent_state(selected, legacy)
    assert legacy.lstat() == original
    if existing == "directory":
        assert (legacy / "original").read_bytes() == b"original saved state"
    elif existing == "file":
        assert legacy.read_bytes() == b"original saved state"
    else:
        assert legacy.is_symlink() and legacy.readlink() == tmp_path / "missing"
    selected.factory.assert_not_called()
    assert not (tmp_path / "protected").exists()


def test_transport_configuration_does_not_override_builder_state(tmp_path):
    selected = selection(persistent_state=str(tmp_path / "protected/native/state"))
    builder = InertTransportBuilder().persistent_state("/already-selected/by-caller")
    guide_runtime.configure_builder(builder, selected)
    assert builder._config.persistent_state == "/already-selected/by-caller"
    selected.factory.assert_not_called()
    with pytest.raises(bootstrap.GatewayInstallError, match="one builder"):
        guide_runtime.configure_builder(InertTransportBuilder(), selected)


def test_unreadable_legacy_state_refuses_instead_of_assuming_absence(tmp_path, monkeypatch):
    selected = selection(persistent_state=str(tmp_path / "protected/native/state"))
    def unreadable(path):
        raise PermissionError("fixture inaccessible legacy state")
    monkeypatch.setattr(Path, "lstat", unreadable)
    with pytest.raises(PermissionError):
        guide_runtime.worker_persistent_state(selected, tmp_path / "native/.meta/state")
    selected.factory.assert_not_called()


@pytest.mark.asyncio
@pytest.mark.parametrize("kind", ["implementation", "rehearsal"])
@pytest.mark.parametrize("explicit", [False, True])
async def test_actual_workers_select_exact_state_path_before_transport_binding(tmp_path, monkeypatch, request, kind, explicit):
    from meerkat_meta import Meta
    from launchkit import implementation_worker, rehearsal_worker

    selected = selection(**({"persistent_state": str(tmp_path / "protected/native/state")} if explicit else {}))
    monkeypatch.setattr(bootstrap, "_selected_guide_runtime", None)
    bootstrap.configure_selected_guide_runtime(selected)
    if kind == "implementation":
        tools, _ = request.getfixturevalue("team")
        directory = tools.directory
        worker = implementation_worker
        suffix = ".mobkit/state"
    else:
        directory = tmp_path
        worker = rehearsal_worker
        suffix = ".meta/state"
        (directory / "input.json").write_text(json.dumps({
            "blueprint": starter_blueprint().model_dump(mode="json"), "text": "Review these notes.",
            "flow_id": None, "params": None, "fixtures": {"files": {"sample.txt": "Evidence"}, "notes": []}}))
        (directory / "receipt.json").write_text(json.dumps({"status": "queued"}))
    expected = selected.persistent_state if explicit else str(directory / "native" / suffix)
    observed = []
    async def reached_builder(builder):
        observed.append(builder._config.persistent_state)
        assert selected._bound, "the original transport was configured on this builder"
        raise RuntimeError("fixture stopped before native startup")
    monkeypatch.setattr(InertTransportBuilder, "build", reached_builder)
    monkeypatch.setattr(Meta, "builder", staticmethod(InertTransportBuilder))
    if kind == "rehearsal":
        # Rehearsal's outer run() owns the failure receipt; execute propagates.
        with pytest.raises(RuntimeError, match="fixture stopped before native startup"):
            await worker.execute(directory, asyncio.Event())
    else:
        await worker.execute(directory, asyncio.Event())
    assert observed == [expected]
    selected.factory.assert_not_called()
    assert not Path(expected).exists()
    if kind == "implementation":
        assert json.loads((directory / "receipt.json").read_text())["status"] == "failed"
