"""Exact tracking survives the existing host instruction adapter, offline."""
import asyncio
from copy import deepcopy
import json
from types import SimpleNamespace

import pytest

from launchkit.blueprint import starter_blueprint
from launchkit.compiler import generate_files
from test_application_extension import load_host
from test_application_callback_bridge import load_package
from test_console_auth import metadata


async def tracked_host(tmp_path, monkeypatch, *, carrier=True, system_parameter=False,
                       tracking_parameter=True, failure=None):
    import meerkat_mobkit
    from meerkat_mobkit import identity_first_models

    host = load_host(monkeypatch, tmp_path)
    files = generate_files(starter_blueprint())
    native = tmp_path / "config/mob.toml"
    native.parent.mkdir()
    native.write_text(files["config/mob.toml"])
    spec = json.loads(files["deployment.json"])
    app = host.Application(host.ApplicationContext(tmp_path, spec, asyncio.Event()))
    admissions = []
    result = {"identity": spec["entrypoint"], "dispatched": True,
              "turn_ticket": "opaque-native-ticket", "turn_unavailable": None}

    class Runtime:
        def __init__(self, config):
            pass

        async def connect(self):
            pass

        async def shutdown(self):
            pass

        async def _rpc(self, method, params):
            if method == "mobkit/capabilities":
                return {"host_system_prompt": {"send": carrier, "dispatch": carrier}}
            assert method == "mobkit/dispatch"
            admissions.append(("wire", deepcopy(params)))
            if failure is not None:
                raise failure
            return deepcopy(result)

        def mob_handle(self):
            return self

        async def query_mob_events(self, query):
            # Exact initial native definition already carries canonical roles.
            return [SimpleNamespace(kind="mob_created", data={})]

    async def dispatch(self, identity, message, *, track_turn=False):
        admissions.append(("sdk", {"identity": identity,
            "dispatch_input": message.to_dict(), "track_turn": track_turn}))
        if failure is not None:
            raise failure
        return deepcopy(result)

    async def dispatch_system(self, identity, message, *, track_turn=False, system_prompt=None):
        admissions.append(("sdk", {"identity": identity,
            "dispatch_input": message.to_dict(), "track_turn": track_turn,
            "system_prompt": system_prompt}))
        if failure is not None:
            raise failure
        return deepcopy(result)

    async def dispatch_legacy(self, identity, message):
        admissions.append(("legacy", {"identity": identity,
            "dispatch_input": message.to_dict()}))
        return deepcopy(result)

    Runtime.dispatch = (dispatch_legacy if not tracking_parameter
                        else dispatch_system if system_parameter else dispatch)
    monkeypatch.setattr(meerkat_mobkit, "MobKitRuntime", Runtime)
    # Preserve the raw native admission in this boundary fixture. Exact ticket
    # parsing and completion are owned by the shipping SDK, not this host.
    monkeypatch.setattr(identity_first_models.DispatchResult, "from_dict", staticmethod(deepcopy))
    runtime = await host.connect_application_runtime(SimpleNamespace(mob_config_path=str(native)), app)
    return runtime, spec, admissions, result


@pytest.mark.parametrize("carrier,system_parameter", [(True, False), (True, True), (False, False)])
async def test_tracked_admission_keeps_native_key_ticket_and_canonical_instructions(
        tmp_path, monkeypatch, carrier, system_parameter):
    from meerkat_mobkit.identity_first_models import DispatchInput
    runtime, spec, admissions, expected = await tracked_host(
        tmp_path, monkeypatch, carrier=carrier, system_parameter=system_parameter)
    message = DispatchInput(content="An admitted Reading question", origin="connector",
                            correlation_id="telegram:bot:42", idempotency_key="telegram:bot:42")
    result = await runtime.dispatch(spec["entrypoint"], message, track_turn=True)
    assert result == expected
    assert len(admissions) == 1
    lane, params = admissions[0]
    assert lane == ("wire" if carrier and not system_parameter else "sdk")
    assert params["track_turn"] is True
    assert params["identity"] == spec["entrypoint"]
    assert params["dispatch_input"] == message.to_dict()
    if carrier:
        assert spec["classes"][0]["instructions"] in params["system_prompt"]
    else:
        assert "system_prompt" not in params


@pytest.mark.parametrize("carrier", [True, False])
async def test_lost_tracked_admission_reply_is_not_retried(tmp_path, monkeypatch, carrier):
    from meerkat_mobkit.identity_first_models import DispatchInput
    runtime, spec, admissions, _ = await tracked_host(
        tmp_path, monkeypatch, carrier=carrier,
        failure=TimeoutError("synthetic lost admission reply"))
    with pytest.raises(TimeoutError, match="lost admission"):
        await runtime.dispatch(spec["entrypoint"], DispatchInput("Question", "connector"), track_turn=True)
    assert len(admissions) == 1


async def test_sdk_without_tracking_refuses_before_admission_and_keeps_untracked_compatibility(tmp_path, monkeypatch):
    from meerkat_mobkit.identity_first_models import DispatchInput
    runtime, spec, admissions, _ = await tracked_host(
        tmp_path, monkeypatch, carrier=False, tracking_parameter=False)
    message = DispatchInput("Question", "connector")
    with pytest.raises(RuntimeError, match="No input was sent"):
        await runtime.dispatch(spec["entrypoint"], message, track_turn=True)
    assert admissions == []
    await runtime.dispatch(spec["entrypoint"], message)
    assert admissions == [("legacy", {"identity": spec["entrypoint"], "dispatch_input": message.to_dict()})]


@pytest.mark.parametrize("flag", [1, "true", None])
async def test_tracking_flag_must_be_boolean_before_admission(tmp_path, monkeypatch, flag):
    from meerkat_mobkit.identity_first_models import DispatchInput
    runtime, spec, admissions, _ = await tracked_host(tmp_path, monkeypatch)
    with pytest.raises(TypeError, match="boolean"):
        await runtime.dispatch(spec["entrypoint"], DispatchInput("Question", "connector"), track_turn=flag)
    assert admissions == []


async def test_context_frontend_identity_delegates_to_loaded_source_owner_and_refuses_drift(tmp_path, monkeypatch):
    source = '''
class Integration:
    def tools_for_agent(self, agent): return []
    def frontend_management(self, request): return {"status": "unknown"}
def build(context): return Integration()
'''
    host, app, _ = load_package(tmp_path, monkeypatch, source)
    unloaded = host.ApplicationContext(tmp_path, app.deployment, asyncio.Event())
    with pytest.raises(RuntimeError, match="before application loading"):
        unloaded.frontend_host()
    metadata(tmp_path)
    try:
        await app.load()
        selected = app.context.frontend_host()
        assert selected == app.frontend_host()
        selected["source_digest"] = "forged"
        assert app.context.frontend_host()["source_digest"] != "forged"
        path = tmp_path / "application/__init__.py"
        path.write_text(path.read_text() + "\n# later public source\n")
        with pytest.raises(ValueError, match="source changed"):
            app.context.frontend_host()
        assert app.context.runtime is None
    finally:
        await app.stop()
