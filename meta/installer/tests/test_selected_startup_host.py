"""Selected generated-host wiring with inert runtime settlement.

The controls use genuinely installed SDK builder, roster, topology and callback
types. Toolkit checks require the selected product's installed Toolkit wheel.
No gateway, provider, native owner or Console starts.
"""
import asyncio
import builtins
from contextlib import asynccontextmanager
from copy import deepcopy
import hashlib
import importlib.util
import json
from pathlib import Path
import sys
from types import SimpleNamespace
import tomllib

import pytest

from launchkit.blueprint import starter_blueprint
from launchkit.compiler import generate_files
from launchkit.connect_application_callbacks import SelectedApplicationCallbacks
from launchkit.connect_installer_caller import ExactFile


def generated_host(root, monkeypatch):
    files = generate_files(starter_blueprint())
    for name, content in files.items():
        output = root / name
        output.parent.mkdir(parents=True, exist_ok=True)
        output.write_text(content)
    application = root / "application"
    application.mkdir()
    (application / "__init__.py").write_text('''
class Integration:
    def __init__(self, context): self.context = context
    def tools_for_agent(self, agent):
        if agent["identity"] != self.context.deployment["entrypoint"]: return []
        async def read_owned_case(case_id=None): return {"case_id": case_id}
        return [{"name": "read_owned_case", "handler": read_owned_case,
            "description": "Read a selected case", "schema": {"type": "object"}}]
def build(context): return Integration(context)
''')
    monkeypatch.syspath_prepend(str(root))
    module_names = ("connectors", "mcp_connections", "application_configuration", "console_configuration",
        "console_auth", "console_connect", "console_binding", "console_presentation", "console_shell",
        "native_controller_review", "native_console_login", "native_definition_continuity", "storage_setup")
    for name in module_names:
        monkeypatch.delitem(sys.modules, name, raising=False)
    loader = importlib.util.spec_from_file_location("selected_startup_generated_host", root / "app.py")
    host = importlib.util.module_from_spec(loader)
    loader.loader.exec_module(host)
    for name in module_names:
        if name in sys.modules:
            loaded = sys.modules.pop(name)
            monkeypatch.setitem(sys.modules, name, loaded)
    return host, json.loads(files["deployment.json"]), files


def genuine_meta():
    from meerkat_meta import agent_builder, builder, identity_first_models
    # The test environment installs the reviewed client wheel. Preserve its
    # real module/type identities rather than importing a private source tree.
    return builder.MetaBuilderConfig, agent_builder.CallbackDispatcher, identity_first_models


@pytest.mark.asyncio
@pytest.mark.parametrize("selected", [False, True], ids=["ordinary", "selected-Meta"])
async def test_run_keeps_selected_gateway_and_sdk_types_through_host_callbacks(
        tmp_path, monkeypatch, selected):
    host, spec, files = generated_host(tmp_path, monkeypatch)
    if selected:
        (tmp_path / "application/tool-policy.json").write_text(json.dumps({
            "provider_id": "host-wiring-specimen", "policy_id": "explicit-empty-intent",
            "revision": 1, "members": [{"member_identity": row["identity"], "grants": []}
                for row in spec["agents"]]}))
        config_type, dispatcher_type, models = genuine_meta()
    else:
        from meerkat_mobkit import agent_builder, builder, identity_first_models
        config_type, dispatcher_type, models = (
            builder.MobKitBuilderConfig, agent_builder.CallbackDispatcher, identity_first_models)
    events, source_types = [], []
    monkeypatch.setenv("MOBKIT_IDENTITY_RESTORE_CONCURRENCY", "7")
    monkeypatch.setenv("META_IDENTITY_RESTORE_CONCURRENCY", "9")
    ordinary = tmp_path / "ordinary-gateway"
    selected_path = tmp_path / "selected-gateway"
    for path in (ordinary, selected_path):
        path.write_bytes(b"Never executed by these host wiring controls.\n")

    def ordinary_gateway():
        if selected:
            pytest.fail("Selected startup consulted the ordinary gateway.")
        events.append("ordinary gateway")
        return ordinary
    monkeypatch.setattr(host, "gateway_path", ordinary_gateway)
    monkeypatch.setattr(host, "console_configuration", lambda *args: False)
    monkeypatch.setattr(host, "validate_configuration_values", lambda *args: None)
    monkeypatch.setattr(host, "local_tools", lambda *args: [])
    monkeypatch.setattr(asyncio.get_running_loop(), "add_signal_handler", lambda *args: None)

    class Tools:
        def __init__(self, *args): pass
        async def close(self): events.append("tools close")
    class Pool:
        def __init__(self, spec): self.connections = {}
        def for_class(self, item): return []
        async def start(self): events.append("pool start")
        async def close(self): events.append("pool close")
    monkeypatch.setattr(host, "ConnectorTools", Tools)
    monkeypatch.setattr(host, "MCPPool", Pool)

    class Runtime:
        async def shutdown(self): events.append("runtime shutdown")
    runtime = Runtime()

    async def exercise(config, application):
        events.append("connect")
        assert host.os.environ["META_IDENTITY_RESTORE_CONCURRENCY"] == ("1" if selected else "9")
        assert host.os.environ["MOBKIT_IDENTITY_RESTORE_CONCURRENCY"] == ("7" if selected else "1")
        assert type(config) is config_type
        assert config.gateway_bin == str(selected_path if selected else ordinary)
        assert config.roster_provider is config.topology_provider is application
        assert config.persistent_state == str(tmp_path / ".mobkit/state")
        assert config.mob_config_path == (None if selected else str(tmp_path / "config/mob.toml"))
        context = models.RosterContext(mob_definition=observed) if selected else None
        roster = await application.roster(context)
        edges = await application.compute_edges([row.identity for row in roster], context)
        assert roster and edges
        assert all(type(row) is models.DurableAgentSpec for row in roster)
        assert all(type(edge) is models.ManagedPeerEdge for edge in edges)
        source_types.extend((type(roster[0]).__module__, type(edges[0]).__module__))
        dispatcher = dispatcher_type()
        dispatcher.register_builder(config.session_builder)
        helper = spec["agents"][0]
        options = {"scope_id": "selected-case-scope", "profile_name":
            projected_profile if selected else helper["profile"], "resume_session_id": "retained-session",
            "labels": {"agent_identity": helper["identity"]}}
        if selected:
            member = members[0]["member"]
            options["labels"] = {"mob_id": member["mob_id"], "role": projected_profile,
                "profile_name": projected_profile, "agent_identity": member["member"],
                "meerkat_id": member["member"]}
        built = await dispatcher.handle_callback("callback/build_agent", {"options": options})
        assert "read_owned_case" in [row["name"] for row in built["tools"]]
        result = await dispatcher.handle_callback("callback/call_tool", {
            "scope_id": "selected-case-scope", "tool": "read_owned_case", "arguments": {"case_id": "kept"}})
        assert result == {"content": {"case_id": "kept"}}
        application.context.runtime = runtime
        application.context.stop_event.set()
        return runtime

    original = tomllib.loads(files["config/mob.toml"])
    mob = original.pop("mob")
    original["id"] = mob["id"]
    observed = deepcopy(original)
    projected_profile = "explicit-observed-helper-profile-specimen"
    projected = deepcopy(original["profiles"][spec["agents"][0]["profile"]])
    projected["tools"].setdefault("mcp_servers", []).append({
        "name": "toolkit-setup", "command": "never-executed-specimen"})
    projected["tools"].setdefault("rust_bundles", []).append("meerkat_toolkit.setup_results")
    observed["profiles"][projected_profile] = projected
    rows = [{"identity": row["identity"], "profile": row["profile"]} for row in spec["agents"]]
    members = [{"member": {"mob_id": spec["slug"], "role": row["profile"],
                "member": "explicit-binding-specimen-" + str(index)}} for index, row in enumerate(rows)]
    plan = {"schema": 1, "application": {"schema": 4,
        "definition": {"format": "native", "definition": original}, "roster": rows,
        "members": members, "helper": {"member": members[0]["member"]}, "helper_profile": "dedicated"}}
    plan_path = tmp_path / "callback-plan.json"
    content = json.dumps(plan).encode()
    plan_path.write_bytes(content)
    plan_path.chmod(0o600)
    callbacks = SelectedApplicationCallbacks(ExactFile(plan_path, hashlib.sha256(content).hexdigest()))

    class Startup:
        gateway = SimpleNamespace(path=selected_path)
        async def prepare(self, application):
            events.append("prepare")
            application._application_callbacks = callbacks
        def builder_config(self, **options):
            assert options["gateway_bin"] == str(selected_path)
            assert "mob_config_path" not in options
            return config_type(**options)
        async def connect(self, config, application): return await exercise(config, application)
        async def shutdown(self, selected_runtime):
            assert selected_runtime is runtime
            await selected_runtime.shutdown()

    async def storage(root, build):
        if selected:
            pytest.fail("Selected startup constructed the ordinary storage runtime.")
        assert root == tmp_path
        events.append("ordinary storage")
    original_storage = sys.modules.get("storage_setup")
    import storage_setup
    if original_storage is None:
        sys.modules.pop("storage_setup")
        monkeypatch.setitem(sys.modules, "storage_setup", storage_setup)
    monkeypatch.setattr(storage_setup, "prepare_storage", storage)
    if not selected:
        async def ordinary_connect(config, application, *, application_startup):
            assert application_startup is None
            return await exercise(config, application)
        monkeypatch.setattr(host, "connect_application_runtime", ordinary_connect)
    original_import = builtins.__import__
    def selected_import(name, *args, **kwargs):
        if selected and (name == "meerkat_mobkit" or name.startswith("meerkat_mobkit.")):
            pytest.fail("Selected startup imported an ordinary MobKit SDK type: " + name)
        return original_import(name, *args, **kwargs)
    monkeypatch.setattr(builtins, "__import__", selected_import)
    args = SimpleNamespace(activate_revision=None, connections_check=False)
    await host.run(spec, args, application_startup=Startup() if selected else None)
    assert events == (["prepare", "pool start", "connect", "runtime shutdown", "pool close", "tools close"]
        if selected else ["ordinary gateway", "pool start", "ordinary storage", "connect",
                          "runtime shutdown", "pool close", "tools close"])
    assert source_types == ["meerkat_meta.identity_first_models" if selected else
                           "meerkat_mobkit.identity_first_models"] * 2


def doctor_selection(root, monkeypatch):
    host, spec, _ = generated_host(root, monkeypatch)
    genuine_meta()
    if importlib.util.find_spec("meerkat_toolkit") is None:
        pytest.skip("Install the exact Toolkit wheel from the selected product to run its SDK wiring checks.")
    from meerkat_toolkit import application_runtime, host_control
    assert callable(application_runtime.ToolkitApplicationRuntime)
    assert callable(host_control.ToolkitMobKitRuntime)
    root.chmod(0o700)
    gateway = root / "selected-doctor-gateway"
    content = b"Inert selected executable specimen; never run.\n"
    gateway.write_bytes(content)
    gateway.chmod(0o700)
    selected = SimpleNamespace(gateway=ExactFile(gateway, hashlib.sha256(content).hexdigest()),
        lease=SimpleNamespace(root=root, require_current=lambda: None))
    for name in ("prepare", "connect", "provision"):
        setattr(selected, name, lambda *args: pytest.fail("Doctor entered native startup."))
    monkeypatch.setattr(host, "gateway_path", lambda: pytest.fail("Selected doctor consulted ordinary gateway."))
    monkeypatch.setenv("OPENAI_API_KEY", "inert-doctor-provider-value")
    return host, spec, selected


def test_selected_main_doctor_uses_actual_sdk_and_exact_gateway_under_current_lease(
        tmp_path, monkeypatch, capsys):
    host, spec, selected = doctor_selection(tmp_path, monkeypatch)
    monkeypatch.chdir(tmp_path)
    monkeypatch.setattr(sys, "argv", ["app.py", "--doctor"])
    original_storage = sys.modules.get("storage_setup")
    import storage_setup
    if original_storage is None:
        sys.modules.pop("storage_setup")
        monkeypatch.setitem(sys.modules, "storage_setup", storage_setup)
    project_lock = storage_setup.project_lock
    leases = []
    factory_loops = []
    factory_exits = []
    @asynccontextmanager
    async def factory(lease):
        lease.require_current()
        leases.append(lease)
        factory_loops.append(asyncio.get_running_loop())
        selected.lease = lease
        try:
            yield selected
        finally:
            lease.require_current()
            assert asyncio.get_running_loop() is factory_loops[0]
            factory_exits.append(selected)
    original = host.doctor
    def checked_doctor(value, *, application_startup):
        assert value == spec and application_startup is selected
        assert asyncio.get_running_loop() is factory_loops[0]
        application_startup.lease.require_current()
        # The same real flock excludes a second owner during diagnostics.
        with pytest.raises(RuntimeError, match="already running"):
            with project_lock(tmp_path):
                pytest.fail("Selected doctor released its project owner.")
        return original(value, application_startup=application_startup)
    monkeypatch.setattr(host, "doctor", checked_doctor)
    with pytest.raises(SystemExit) as result:
        host.main(application_startup_factory=factory)
    assert result.value.code == 0 and len(leases) == 1
    assert factory_exits == [selected]
    with pytest.raises(RuntimeError, match="no longer current"):
        leases[0].require_current()
    output = capsys.readouterr().out
    assert "[ready] Meta SDK" in output and "[ready] Toolkit SDK" in output
    assert "[ready] Selected application gateway" in output
    assert "MobKit SDK" not in output and "Storage maintenance tool" not in output
    assert "inert-doctor-provider-value" not in output
    assert not (tmp_path / "current").exists()


@pytest.mark.parametrize("changed", ["bytes", "execute", "owner", "Meta", "Toolkit"])
def test_selected_doctor_fails_cleanly_for_changed_gateway_owner_or_unavailable_sdk(
        tmp_path, monkeypatch, capsys, changed):
    host, spec, selected = doctor_selection(tmp_path, monkeypatch)
    if changed == "bytes":
        selected.gateway.path.write_bytes(b"Changed selected gateway.\n")
    elif changed == "execute":
        selected.gateway.path.chmod(0o600)
    elif changed == "owner":
        def lost(): raise RuntimeError("lost project owner")
        selected.lease.require_current = lost
    else:
        original = importlib.import_module
        unavailable = "meerkat_meta.builder" if changed == "Meta" else "meerkat_toolkit.application_runtime"
        def unavailable_sdk(name, *args, **kwargs):
            if name == unavailable:
                raise ImportError("Diagnostic import failure; no private exception text rendered.")
            return original(name, *args, **kwargs)
        monkeypatch.setattr(importlib, "import_module", unavailable_sdk)
    assert host.doctor(spec, application_startup=selected) is False
    output = capsys.readouterr().out
    label = changed + " SDK" if changed in {"Meta", "Toolkit"} else "Selected application gateway"
    assert "[needs setup] " + label in output
    assert "Diagnostic import failure" not in output
    assert not (tmp_path / "current").exists()


@pytest.mark.parametrize("action", ["--check", "--application-check"])
def test_source_only_checks_do_not_select_trusted_startup(tmp_path, monkeypatch, action):
    host, _, _ = generated_host(tmp_path, monkeypatch)
    monkeypatch.chdir(tmp_path)
    monkeypatch.setattr(sys, "argv", ["app.py", action])
    host.main(application_startup_factory=lambda lease: pytest.fail("Source check selected startup."))
    assert not (tmp_path / ".mobkit").exists()


def test_ordinary_doctor_keeps_old_sdk_gateway_and_storage_checks(tmp_path, monkeypatch, capsys):
    host, spec, _ = generated_host(tmp_path, monkeypatch)
    monkeypatch.setenv("OPENAI_API_KEY", "inert-doctor-provider-value")
    ordinary = tmp_path / ".bin"
    ordinary.mkdir()
    for name in ("rpc_gateway", "mobkit_gateway"):
        (ordinary / name).touch()
    monkeypatch.setenv("MOBKIT_RPC_GATEWAY_BIN", str(ordinary / "rpc_gateway"))
    assert host.doctor(spec) is True
    output = capsys.readouterr().out
    assert "[ready] MobKit SDK" in output and "[ready] Storage maintenance tool" in output
    (ordinary / "mobkit_gateway").unlink()
    assert host.doctor(spec) is False
    assert "[needs setup] Storage maintenance tool" in capsys.readouterr().out
    monkeypatch.chdir(tmp_path)
    monkeypatch.setattr(sys, "argv", ["app.py", "--doctor"])
    monkeypatch.setattr(host, "doctor", lambda value: value == spec)
    with pytest.raises(SystemExit) as result:
        host.main()
    assert result.value.code == 0 and not (tmp_path / ".mobkit").exists()
