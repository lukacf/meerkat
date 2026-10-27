"""Shared typed model routing with inert modules, no selected SDK execution."""
import asyncio
import builtins
import copy
import json
import sys
from types import ModuleType, SimpleNamespace

import pytest

from launchkit.blueprint import starter_blueprint
from launchkit.compiler import generate_files
from test_application_extension import DYNAMIC_APPLICATION, load_host, write_application


class InertModel(SimpleNamespace):
    def to_dict(self):
        return copy.deepcopy(vars(self))


@pytest.fixture
def models(monkeypatch):
    namespace = ModuleType("meerkat_meta")
    namespace.identity_first_models = SimpleNamespace(
        DurableAgentSpec=InertModel, ManagedPeerEdge=InertModel)
    monkeypatch.setitem(sys.modules, "meerkat_meta", namespace)
    original = builtins.__import__
    seen = []

    def import_models(name, *args, **kwargs):
        if name == "meerkat_mobkit" or name.startswith("meerkat_mobkit."):
            seen.append(name)
            raise ModuleNotFoundError("Legacy SDK intentionally absent in inert product environment",
                name="meerkat_mobkit")
        return original(name, *args, **kwargs)

    monkeypatch.setattr(builtins, "__import__", import_models)
    return namespace.identity_first_models, seen, original


async def test_dynamic_application_load_and_application_check_without_legacy_sdk(
        tmp_path, monkeypatch, capsys, models):
    module = load_host(monkeypatch, tmp_path)
    spec = json.loads(generate_files(starter_blueprint())["deployment.json"])
    write_application(tmp_path, DYNAMIC_APPLICATION)
    application = module.Application(module.ApplicationContext(tmp_path, spec, asyncio.Event()))
    await application.load()
    assert application.dynamic and application._application_callbacks is None
    assert all(type(row) is InertModel for row in application._snapshot[1])
    assert application.context.runtime is None and not application.integration.started
    await module.application_check(spec)
    assert "Native materialization and user authorization were not exercised" in capsys.readouterr().out
    assert models[1] == ["meerkat_mobkit", "meerkat_mobkit"]
    await application.stop()


def test_missing_internal_legacy_dependency_is_not_product_fallback(tmp_path, monkeypatch, models):
    module = load_host(monkeypatch, tmp_path)

    def broken(name, *args, **kwargs):
        if name == "meerkat_mobkit":
            raise ModuleNotFoundError("An installed legacy SDK dependency is broken", name="internal_dependency")
        return models[2](name, *args, **kwargs)

    monkeypatch.setattr(builtins, "__import__", broken)
    with pytest.raises(ModuleNotFoundError) as failure:
        module.identity_models()
    assert failure.value.name == "internal_dependency"
    assert module.identity_models(native=True) is models[0]


async def test_selected_meta_fixed_roster_uses_meta_models_without_helper_callbacks(
        tmp_path, monkeypatch, models):
    module = load_host(monkeypatch, tmp_path)
    spec = json.loads(generate_files(starter_blueprint())["deployment.json"])
    application = module.Application(module.ApplicationContext(tmp_path, spec, asyncio.Event()))
    application._native_identity_models = True
    roster = await application.roster(SimpleNamespace(mob_definition=None, previous_identities=[]))
    assert roster and all(type(row) is InertModel for row in roster)
    assert application._application_callbacks is None and models[1] == []
    assert application.context.runtime is None


@pytest.mark.parametrize("requires_policy", [False, True])
async def test_generated_run_checks_selected_policy_requirement_before_startup(
        tmp_path, monkeypatch, requires_policy):
    """Run the generated entry to the source owner's preparation boundary.

    No runtime, provider or Console executes. Missing mandatory Toolkit policy
    must fail before that boundary; ordinary startup must reach it without
    synthesizing the absent policy file.
    """
    module = load_host(monkeypatch, tmp_path)
    spec = json.loads(generate_files(starter_blueprint())["deployment.json"])
    spec["environment"] = []
    gateway = tmp_path / "inert-rpc"
    gateway.write_text("Not an executable runtime.\n")
    reached = []

    async def prepare(application):
        reached.append(application)
        raise RuntimeError("inert original preparation boundary")

    startup = SimpleNamespace(gateway=SimpleNamespace(path=gateway),
        require_tool_policy_source=requires_policy, prepare=prepare)
    monkeypatch.setitem(sys.modules, "storage_setup", SimpleNamespace(prepare_storage=None))
    args = SimpleNamespace(activate_revision=None)
    if requires_policy:
        with pytest.raises(ValueError, match="requires application/tool-policy.json"):
            await module.run(spec, args, application_startup=startup)
        assert reached == []
    else:
        with pytest.raises(RuntimeError, match="inert original preparation boundary"):
            await module.run(spec, args, application_startup=startup)
        assert len(reached) == 1 and reached[0].context.runtime is None
    assert not (tmp_path / "application/tool-policy.json").exists()
