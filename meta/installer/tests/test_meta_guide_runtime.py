"""Exercise Forge's selected guide binding without launching native processes."""
import asyncio
from dataclasses import replace
import hashlib
import json
import tomllib
from types import SimpleNamespace

import pytest

from launchkit import bootstrap, guide_runtime, guide_sdk, planner, visual_review_runtime
from launchkit.connect_installer_caller import CustodyError, ExactFile
from launchkit.implementation_worker import implementation_mob_toml
from launchkit.schema import DesignRequest
from meerkat_meta.errors import GatewayStartupCleanupError
from test_architect_startup_recovery import FakeRuntime, startup


@pytest.fixture
def selected(tmp_path, monkeypatch):
    tmp_path.chmod(0o700)
    gateway = tmp_path / "inert-rpc-gateway"
    gateway.write_bytes(b"inert test fixture, never execute\n")
    gateway.chmod(0o700)
    selection = bootstrap.SelectedGuideRuntime(
        ExactFile(gateway, hashlib.sha256(gateway.read_bytes()).hexdigest()),
        guide_sdk.SELECTED_VERSION, guide_sdk.SELECTED_VERSION, "a" * 40, "b" * 40,
        {"provider": "openai", "model": "gpt-6.1-sol"},
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
    ]
    route["model"] = "changed-fixture"
    assert selected.model_configuration["model"] == "gpt-6.1-sol"
    with pytest.raises(RuntimeError, match="changed its selected host route"):
        guide_runtime.model_profile_lines("openai", "changed-fixture", selected.model_configuration)
    selected.gateway.path.write_bytes(b"changed fixture\n")
    with pytest.raises(CustodyError, match="^The retained caller file no longer matches its selected bytes\\.$"):
        guide_runtime.configure_builder(builder, selected)
    assert len(builder.calls) == 1, "changed gateway bytes must be rejected before builder admission"


BINDINGS = [{"realm": "global", "binding": "copilot_openai"},
            {"realm": "fixture", "binding": "host", "profile": "work"}]
INVALID_BINDINGS = ["fixture-host-route", {"realm": "fixture"}, {"binding": "host"},
    {"realm": "fixture", "binding": ""}, {"realm": "fixture", "binding": "host", "origin": "configured"},
    {"realm": "fixture", "binding": 7}, {"realm": "fixture", "binding": "host\x00"}, None]


@pytest.mark.parametrize("binding", BINDINGS)
def test_fresh_guide_applies_a_profile_auth_binding(selected, binding):
    route = {**selected.model_configuration, "auth_binding": binding}
    accepted = replace(selected, _model_route=route)
    assert accepted.model_configuration == route
    assert bootstrap.selected_guide_runtime() is selected


@pytest.mark.parametrize("binding", INVALID_BINDINGS)
def test_fresh_guide_refuses_an_invalid_auth_binding(selected, binding):
    route = {**selected.model_configuration, "auth_binding": binding}
    with pytest.raises(bootstrap.GatewayInstallError, match="original trusted model route"):
        replace(selected, _model_route=route)
    assert bootstrap.selected_guide_runtime() is selected


@pytest.mark.parametrize("binding", BINDINGS)
def test_worker_restore_keeps_the_auth_binding_and_refuses_an_invalid_one(selected, monkeypatch, binding):
    route = {**selected.model_configuration, "auth_binding": binding}
    monkeypatch.setattr(bootstrap, "_selected_guide_runtime", replace(selected, _model_route=route))
    control = bootstrap.export_selected_guide_context()
    assert control["model_configuration"]["auth_binding"] == binding
    monkeypatch.setattr(bootstrap, "_selected_guide_runtime", None)
    assert bootstrap.restore_selected_guide_context(control).model_configuration == route
    for invalid in INVALID_BINDINGS:
        monkeypatch.setattr(bootstrap, "_selected_guide_runtime", None)
        changed = json.loads(json.dumps(control))
        changed["model_configuration"]["auth_binding"] = invalid
        with pytest.raises(bootstrap.GatewayInstallError, match="original trusted model route"):
            bootstrap.restore_selected_guide_context(changed)
        assert bootstrap.selected_guide_runtime() is None


@pytest.mark.parametrize("binding", BINDINGS)
def test_profile_authoring_emits_the_auth_binding_as_profile_toml(binding):
    import tomllib
    route = {"provider": "openai", "model": "gpt-6.1-sol", "auth_binding": binding}
    lines = guide_runtime.model_profile_lines(route["provider"], route["model"], route)
    assert lines[:2] == ['model = "gpt-6.1-sol"', 'provider = "openai"'] and len(lines) == 3
    parsed = tomllib.loads("[profiles.guide]\n" + "\n".join(lines) + "\n")["profiles"]["guide"]
    assert parsed == {"model": "gpt-6.1-sol", "provider": "openai", "auth_binding": binding}
    for invalid in INVALID_BINDINGS:
        with pytest.raises(RuntimeError, match="invalid auth binding"):
            guide_runtime.model_profile_lines(route["provider"], route["model"], {**route, "auth_binding": invalid})


@pytest.mark.parametrize("composition", ["setup team", "designer", "builder", "visual critic"])
async def test_every_profile_forge_composes_names_the_selected_auth_binding(startup, monkeypatch, tmp_path, composition):
    """Each mob Forge composes for its own agents names the launcher-selected
    route's auth binding in every profile: the setup team, the planner's
    designer, the builder's implementation and acceptance agents, and the
    visual critic. The SDK is an inert double; no gateway, provider or native
    process runs, and each start stops once its definition is composed."""
    service, controller = startup
    binding = {"realm": "fixture", "binding": "host", "profile": "work"}
    route = {"provider": "openai", "model": "gpt-6.1-sol", "auth_binding": binding}
    routed = bootstrap.SelectedGuideRuntime(bootstrap.selected_guide_runtime().gateway, guide_sdk.SELECTED_VERSION,
        guide_sdk.declared_core_version(), "a" * 40, "b" * 40, route)
    monkeypatch.setattr(bootstrap, "_selected_guide_runtime", routed)

    class Composed(Exception):
        """Stops a start once its mob definition is composed."""

    if composition == "setup team":
        controller.outcomes = [FakeRuntime()]
        assert (await service.start())["status"] == "ready"
        await service.shutdown()
        definition = controller.mob_configs[-1]
    elif composition == "designer":
        controller.outcomes = [Composed()]
        with pytest.raises(Composed):
            await planner.design_team(DesignRequest(description="Plan a small research team."))
        definition = controller.mob_configs[-1]
    elif composition == "builder":
        definition = implementation_mob_toml("openai", "gpt-6.1-sol",
            model_configuration=routed.model_configuration, guide_runtime=routed)
    else:
        monkeypatch.setattr(visual_review_runtime, "planner_capability",
            lambda: {"available": True, "provider": "openai"})
        critic = visual_review_runtime.VisualReviewRuntime(tmp_path / "visual-critic")
        critic.reviews = SimpleNamespace()
        controller.outcomes = [Composed()]
        with pytest.raises(Composed):
            await critic.get_runtime()
        definition = controller.mob_configs[-1]
    profiles = tomllib.loads(definition)["profiles"]
    assert set(profiles) == {"setup team": {"architect", "connectors", "reviewer", "visualizer"},
        "designer": {"designer"}, "builder": {"implementation", "acceptance"},
        "visual critic": {visual_review_runtime.REVIEW_PROFILE}}[composition]
    for name, profile in profiles.items():
        assert (profile["provider"], profile["model"], profile.get("auth_binding")) == (
            "openai", "gpt-6.1-sol", binding), name


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
