"""Exercise Forge's selected guide binding without launching native processes."""
import asyncio
import hashlib
import json

import pytest

from launchkit import bootstrap, guide_runtime, guide_sdk
from launchkit.connect_installer_caller import CustodyError, ExactFile
from meerkat_meta.errors import GatewayStartupCleanupError


@pytest.fixture
def selected(tmp_path, monkeypatch):
    tmp_path.chmod(0o700)
    gateway = tmp_path / "inert-rpc-gateway"
    gateway.write_bytes(b"inert test fixture, never execute\n")
    gateway.chmod(0o700)
    selection = bootstrap.SelectedGuideRuntime(
        ExactFile(gateway, hashlib.sha256(gateway.read_bytes()).hexdigest()),
        guide_sdk.SELECTED_VERSION, guide_sdk.SELECTED_VERSION, "a" * 40, "b" * 40,
        {"provider": "openai", "model": "gpt-6.1-sol", "auth_binding": "fixture-host-route"},
    )
    monkeypatch.setattr(bootstrap, "_selected_guide_runtime", selection)
    return selection


def test_agent_builder_uses_original_gateway_and_model_binding(selected):
    class Builder:
        def __init__(self):
            self.calls = []

        def gateway(self, path):
            self.calls.append(path)

    builder = Builder()
    assert guide_runtime.configure_builder(builder, selected) is builder
    assert builder.calls == [str(selected.gateway.path)]
    route = guide_runtime.selected_model_configuration()
    assert guide_runtime.model_profile_lines("openai", "gpt-6.1-sol", route) == [
        'model = "gpt-6.1-sol"', 'provider = "openai"',
        'auth_binding = "fixture-host-route"',
    ]
    route["model"] = "changed-fixture"
    assert selected.model_configuration["model"] == "gpt-6.1-sol"
    with pytest.raises(RuntimeError, match="changed its selected host route"):
        guide_runtime.model_profile_lines("openai", "changed-fixture", selected.model_configuration)
    selected.gateway.path.write_bytes(b"changed fixture\n")
    with pytest.raises(CustodyError, match="^The retained caller file no longer matches its selected bytes\\.$"):
        guide_runtime.configure_builder(builder, selected)
    assert len(builder.calls) == 1, "changed gateway bytes must be rejected before builder admission"


def test_no_implicit_predecessor_runtime(monkeypatch):
    monkeypatch.setattr(bootstrap, "_selected_guide_runtime", None)
    with pytest.raises(RuntimeError, match="selected Meta runtime"):
        guide_runtime.selected_runtime()


def test_worker_restores_original_private_selection_and_rejects_changed_control(selected, tmp_path, monkeypatch):
    directory = tmp_path / "worker"
    directory.mkdir(mode=0o700)
    digest = guide_runtime.capture_worker_runtime(directory)
    assert guide_runtime.capture_worker_runtime(directory) == digest
    path = directory / "forge-agent-runtime.json"
    assert path.stat().st_mode & 0o777 == 0o600
    monkeypatch.setattr(bootstrap, "_selected_guide_runtime", None)
    restored = guide_runtime.restore_worker_runtime(directory, digest)
    assert restored.gateway.pin() == selected.gateway.pin()
    assert restored.model_configuration == selected.model_configuration
    value = json.loads(path.read_text())
    value["model_configuration"]["model"] = "changed-fixture"
    path.write_text(json.dumps(value))
    with pytest.raises(RuntimeError, match="changed before startup"):
        guide_runtime.restore_worker_runtime(directory, digest)
    assert bootstrap.selected_guide_runtime() is restored
    with pytest.raises(RuntimeError, match="changed its original"):
        guide_runtime.capture_worker_runtime(directory)


def test_failed_start_keeps_sdk_cleanup_through_wrapping_and_cancellation():
    cleanup = object()
    sdk_error = GatewayStartupCleanupError(ValueError("fixture startup"), OSError("fixture stop"), cleanup)
    cancelled = asyncio.CancelledError("fixture cancellation")
    cancelled.gateway_cleanup = cleanup
    failure = BaseExceptionGroup("fixture group", [sdk_error, cancelled])

    class Builder:
        async def build(self):
            raise failure

    retained = []
    for _ in range(2):
        with pytest.raises(BaseExceptionGroup) as caught:
            asyncio.run(guide_runtime.build_with_cleanup(Builder(), retained))
        assert caught.value is failure
    assert retained == [cleanup], "cleanup stays owned once, never inferred stopped from failed startup"
