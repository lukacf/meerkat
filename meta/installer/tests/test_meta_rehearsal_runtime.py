"""Selected Meta trial controls; no provider calls or native executable runs."""
import asyncio
import ast
import copy
import hashlib
import json
import tomllib
from types import SimpleNamespace

import pytest
import meerkat_meta
from meerkat_meta.errors import GatewayStartupCleanupError

from meta_trial_fixtures import selected, meta_trial_runtime
from launchkit.blueprint import Blueprint, starter_blueprint
from launchkit.compiler import resolved_blueprint
from launchkit.implementation import read_receipt
from launchkit.rehearsal import missing_trial_providers, worker_environment, RehearsalService
from launchkit import rehearsal_worker as worker
from launchkit.templates.storage_setup import write_private
from launchkit import application_trial
from launchkit.compiler import _runtime


def mixed_spec():
    value = starter_blueprint().model_dump(mode="json")
    value["agent_classes"][1]["provider"] = "anthropic"
    value["agent_classes"][1]["model"] = "claude-fixture-other-model"
    return Blueprint.model_validate(value)


def test_trial_retains_proposed_models_and_peers_without_old_registry_or_execution_grants(selected):
    spec = mixed_spec()
    definition = tomllib.loads(worker.rehearsal_mob_toml(spec))
    assert "models" not in definition, "selected Meta resolves its own actual model registry"
    for profile in definition["profiles"].values():
        if profile["provider"] == "openai":
            assert profile["model"] == "gpt-6.1-sol"
            assert profile["auth_binding"] == "fixture-host-route"
        else:
            assert (profile["provider"], profile["model"]) == ("anthropic", "claude-fixture-other-model")
            assert "auth_binding" not in profile, "the guide host binding cannot be reused for another model route"
        tools = profile["tools"]
        assert tools["comms"] is True
        assert all(tools[name] is False for name in ["mob", "shell", "builtins", "workgraph", "schedule"])
    assert resolved_blueprint(spec)["edges"], "this proposal still has its intended peer collaboration"


def test_trial_host_credentials_cover_only_original_model_and_environment_remains_scoped(selected, tmp_path, monkeypatch):
    for key in ["OPENAI_API_KEY", "ANTHROPIC_API_KEY", "GEMINI_API_KEY"]:
        monkeypatch.delenv(key, raising=False)
    spec = mixed_spec()
    assert missing_trial_providers(spec) == ["anthropic"]
    monkeypatch.setenv("ANTHROPIC_API_KEY", "fixture-selected-provider")
    assert missing_trial_providers(spec) == []
    monkeypatch.setenv("GEMINI_API_KEY", "fixture-unselected-provider")
    monkeypatch.setenv("FORGE_PRIVATE_SETTING", "fixture-not-for-worker")
    monkeypatch.setenv("MOBKIT_GATEWAY_BIN", "/untrusted/gateway")
    environment = worker_environment(tmp_path, spec)
    assert environment["ANTHROPIC_API_KEY"] == "fixture-selected-provider"
    assert all(name not in environment for name in ["GEMINI_API_KEY", "FORGE_PRIVATE_SETTING", "MOBKIT_GATEWAY_BIN"])
    value = starter_blueprint().model_dump(mode="json")
    value["agent_classes"][0]["model"] = "gpt-fixture-different-model"
    assert missing_trial_providers(Blueprint.model_validate(value)) == ["openai"]


@pytest.mark.asyncio
async def test_parent_rejects_changed_gateway_before_worker_spawn(selected, tmp_path, monkeypatch):
    spec = starter_blueprint()
    directory = tmp_path / "trial"
    directory.mkdir(mode=0o700)
    write_private(directory / "input.json", {"blueprint": spec.model_dump(mode="json")})
    write_private(directory / "receipt.json", {"status": "queued"})
    spawns = []

    async def forbidden(*args, **kwargs):
        spawns.append(args)
        raise AssertionError("A changed selection cannot reach worker admission")

    monkeypatch.setattr("launchkit.rehearsal.spawn_owned_worker", forbidden)
    selected.gateway.path.write_bytes(b"changed inert gateway\n")
    service = RehearsalService(tmp_path, SimpleNamespace())
    await service._run(directory)
    assert not spawns
    assert read_receipt(directory)["status"] == "failed"


@pytest.mark.asyncio
async def test_parent_passes_private_runtime_seal_to_its_original_worker(selected, tmp_path, monkeypatch):
    directory = tmp_path / "trial"
    directory.mkdir(mode=0o700)
    write_private(directory / "input.json", {"blueprint": starter_blueprint().model_dump(mode="json")})
    write_private(directory / "receipt.json", {"status": "queued"})
    admissions, stops = [], []

    class Child:
        returncode = 0

        async def wait(self):
            return 0

    child = Child()

    async def spawn(*args, **kwargs):
        admissions.append((args, kwargs))
        return child, False

    async def stop(process):
        assert process is child
        stops.append(process)

    monkeypatch.setattr("launchkit.rehearsal.spawn_owned_worker", spawn)
    service = RehearsalService(tmp_path, SimpleNamespace())
    monkeypatch.setattr(service, "_stop_process", stop)
    await service._run(directory)
    assert len(admissions) == 1 and stops == [child]
    args, options = admissions[0]
    control = directory / "forge-agent-runtime.json"
    assert args[args.index("--agent-runtime-sha256") + 1] == hashlib.sha256(control.read_bytes()).hexdigest()
    assert json.loads(control.read_text())["gateway"] == selected.gateway.pin()
    assert control.stat().st_mode & 0o777 == 0o600
    assert "MOBKIT_GATEWAY_BIN" not in options["env"]
    assert not service.processes


@pytest.mark.asyncio
@pytest.mark.parametrize("cancelled", [False, True])
async def test_trial_drains_failed_sdk_startup_cleanup_without_predecessor_maintenance(
        selected, tmp_path, monkeypatch, cancelled):
    directory = tmp_path / "trial"
    directory.mkdir(mode=0o700)
    spec = starter_blueprint()
    write_private(directory / "input.json", {"blueprint": spec.model_dump(mode="json"),
        "text": "Inspect fixture evidence", "flow_id": None, "params": None,
        "fixtures": {"files": {}, "notes": []}})
    write_private(directory / "receipt.json", {"status": "queued"})
    stops, settings = [], {}

    class Cleanup:
        async def shutdown(self):
            stops.append("stopped")

    cleanup = Cleanup()
    failure = asyncio.CancelledError("fixture cancelled") if cancelled else GatewayStartupCleanupError(
        ValueError("fixture init"), OSError("fixture stop"), cleanup)
    if cancelled:
        failure.gateway_cleanup = cleanup

    class Builder:
        def __getattr__(self, name):
            def configure(*args, **kwargs):
                settings[name] = args
                return self
            return configure

        async def build(self):
            raise failure

    monkeypatch.setattr(meerkat_meta.Meta, "builder", Builder)
    with pytest.raises(type(failure)) as caught:
        await worker.execute(directory, asyncio.Event())
    assert caught.value is failure
    assert stops == ["stopped"]
    assert settings["gateway"] == (str(selected.gateway.path),)
    assert settings["persistent_state"] == (str(directory / "native/.meta/state"),)
    assert not (directory / "native/.bin").exists()
    assert read_receipt(directory)["native_console_url"] is None


@pytest.mark.asyncio
async def test_offline_host_policy_uses_actual_meta_dtos_and_keeps_native_grants():
    # Execute only this harness helper. No authored application, filesystem,
    # sandbox process or native runtime is initialized by this source check.
    helper = next(node for node in ast.parse(application_trial._HARNESS).body
        if isinstance(node, ast.AsyncFunctionDef) and node.name == "native_policy")
    deployment = _runtime(starter_blueprint(), native_version="0.9.0")
    scope = {"production": SimpleNamespace(integration=SimpleNamespace()), "deployment": deployment}
    exec(compile(ast.Module(body=[helper], type_ignores=[]), "trial-policy-helper", "exec"), scope)
    policy = await scope["native_policy"]()
    assert application_trial.validate_application_native_policy(policy, deployment) == policy
    assert [row["identity"] for row in policy["roster"]] == [row["identity"] for row in deployment["agents"]]
    assert [(row["a"], row["b"]) for row in policy["edges"]] == [tuple(row) for row in deployment["connections"]]
    for mutation in ["grant", "lineage", "extra_identity"]:
        changed = copy.deepcopy(policy)
        if mutation == "grant":
            changed["roster"][0]["tools"] = {"shell": True}
        elif mutation == "lineage":
            changed["roster"][0]["labels"]["spawned_by"] = "invented-parent"
        else:
            changed["roster"].append({**changed["roster"][0], "identity": "invented-extra-agent"})
        with pytest.raises(application_trial.ApplicationTrialError):
            application_trial.validate_application_native_policy(changed, deployment)
