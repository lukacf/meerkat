"""Generated-host policy source controls, without native or provider execution."""
import asyncio
from copy import deepcopy
import importlib.util
import json
import os
from pathlib import Path
import sys
import threading
from types import SimpleNamespace

import pytest

from launchkit.blueprint import starter_blueprint
from launchkit.compiler import generate_files


APPLICATION = '''
class Integration:
    def __init__(self, context): self.context = context
    def tools_for_agent(self, agent):
        if agent["identity"] != self.context.deployment["entrypoint"]: return []
        return [{"name": "custom_read", "description": "Read application state",
            "schema": {"type": "object"}, "handler": lambda arguments: arguments},
            {"name": "unselected_callback", "description": "Not a policy grant",
            "schema": {"type": "object"}, "handler": lambda arguments: arguments}]
def build(context): return Integration(context)
'''


def generated_host(root, monkeypatch, *, blueprint=None):
    files = generate_files(starter_blueprint() if blueprint is None else blueprint)
    for name, content in files.items():
        output = root / name
        output.parent.mkdir(parents=True, exist_ok=True)
        output.write_text(content)
    application = root / "application"
    application.mkdir()
    (application / "__init__.py").write_text(APPLICATION)
    monkeypatch.syspath_prepend(str(root))
    names = ("connectors", "mcp_connections", "application_configuration", "console_configuration",
        "console_auth", "console_connect", "console_binding", "console_presentation", "console_shell",
        "native_controller_review", "native_console_login", "native_definition_continuity", "storage_setup")
    for name in names:
        monkeypatch.delitem(sys.modules, name, raising=False)
    loader = importlib.util.spec_from_file_location("tool_policy_generated_host", root / "app.py")
    host = importlib.util.module_from_spec(loader)
    loader.loader.exec_module(host)
    for name in names:
        if name in sys.modules:
            loaded = sys.modules.pop(name)
            monkeypatch.setitem(sys.modules, name, loaded)
    return host, json.loads(files["deployment.json"])


def selection(spec, names=None):
    names = names or {}
    return {"provider_id": "application-source", "policy_id": "accepted-tool-intent", "revision": 1,
        "members": [{"member_identity": agent["identity"], "grants": [
            {"tool_name": name, "action": "invoke", "consequence": "r2"}
            for name in names.get(agent["identity"], ())]} for agent in spec["agents"]]}


def write_selection(root, document):
    source = (json.dumps(document, indent=2) + "\n\n").encode()
    (root / "application/tool-policy.json").write_bytes(source)
    return source


def application(host, spec):
    return host.Application(host.ApplicationContext(host.ROOT, spec, asyncio.Event()))


async def test_missing_ordinary_source_remains_optional(tmp_path, monkeypatch, capsys):
    host, spec = generated_host(tmp_path, monkeypatch)
    await host.application_check(spec)
    loaded = application(host, spec)
    await loaded.load()
    assert loaded.tool_policy_source_bytes is loaded.tool_policy_selection is None
    assert loaded.tool_policy_matches == loaded.tool_policy_unresolved == {}
    assert "tool policy" not in capsys.readouterr().out
    await loaded.stop()


@pytest.mark.parametrize("fault", [None, "missing", "changed", "redirected"])
async def test_helperless_source_uses_exact_seed_policy_without_baseline_fallback(tmp_path, monkeypatch, fault):
    from launchkit.connect_policy_input import APPLICATION_INVENTORY_PATH, APPLICATION_NATIVE_POLICY_PATH
    host, spec = generated_host(tmp_path, monkeypatch)
    baseline = write_selection(tmp_path, selection(spec))
    original = selection(spec, {spec["entrypoint"]: ["custom_read"]})
    selected = json.dumps(original, indent=3).encode() + b"\n"
    path = tmp_path / APPLICATION_NATIVE_POLICY_PATH
    if fault != "missing":
        path.write_bytes(selected)
    spec["applicationInventory"] = APPLICATION_INVENTORY_PATH
    if fault == "redirected":
        spec["applicationInventory"] = "../foreign.json"
    loaded = application(host, spec)
    if fault in {"missing", "redirected"}:
        with pytest.raises(ValueError):
            await loaded.load_tool_policy(required=True)
    else:
        await loaded.load_tool_policy(required=True)
        assert loaded.tool_policy_source_bytes == selected
        assert loaded.tool_policy_selection == original
        if fault == "changed":
            path.write_bytes(baseline)
            with pytest.raises(ValueError, match="changed after loading"):
                await loaded.require_tool_policy_current()
        else:
            await loaded.require_tool_policy_current()
    assert (tmp_path / "application/tool-policy.json").read_bytes() == baseline


async def test_original_intent_preserves_empty_grants_and_pending_native_names(
        tmp_path, monkeypatch, capsys):
    host, spec = generated_host(tmp_path, monkeypatch)
    lead, other = (row["identity"] for row in spec["agents"][:2])
    original = selection(spec, {lead: ["custom_read", "host_docs", "setup_report"],
                                other: ["custom_read"]})
    source = write_selection(tmp_path, original)
    loaded = application(host, spec)
    await loaded.load()
    assert loaded.tool_policy_source_bytes == source
    assert loaded.tool_policy_selection == original
    assert loaded.tool_policy_matches[lead] == ("custom_read",)
    assert loaded.tool_policy_matches[other] == ()
    assert loaded.tool_policy_unresolved[lead] == ("host_docs", "setup_report")
    assert loaded.tool_policy_unresolved[other] == ("custom_read",)
    # Registered availability neither grants omitted callbacks nor expands empty members.
    assert "unselected_callback" in [tool["name"] for tool in loaded.tools[spec["agents"][0]["profile"]]]
    assert loaded.tool_policy_selection["members"][-1]["grants"] == []
    copied = loaded.tool_policy_selection
    copied["members"][0]["grants"].clear()
    assert loaded.tool_policy_selection == original
    matches = loaded.tool_policy_matches
    matches[lead] = ("forged",)
    assert loaded.tool_policy_matches[lead] == ("custom_read",)
    actual_catalog = {row["identity"]: loaded.tools[row["profile"]] for row in spec["agents"]}
    actual_catalog[lead] = actual_catalog[lead] + [{"name": "host_docs"}]
    await loaded.validate_tool_policy_catalog(actual_catalog)
    assert loaded.tool_policy_matches[lead] == ("custom_read", "host_docs")
    assert loaded.tool_policy_unresolved[lead] == ("setup_report",)
    assert loaded.tool_policy_unresolved[other] == ("custom_read",)
    loaded.report_tool_policy()
    assert "2 tool selections await native surface validation" in capsys.readouterr().out
    assert loaded.tool_policy_source_bytes == source
    await loaded.stop()


@pytest.mark.parametrize("mode", ["missing", "wrong-member", "invalid-enum"])
async def test_selected_preflight_refuses_before_connectors_state_or_application(
        tmp_path, monkeypatch, mode):
    host, spec = generated_host(tmp_path, monkeypatch)
    if mode != "missing":
        document = selection(spec, {spec["entrypoint"]: ["setup_report"]})
        if mode == "wrong-member":
            document["members"][0]["member_identity"] = "foreign-durable-identity"
        else:
            document["members"][0]["grants"][0]["consequence"] = "approved"
        write_selection(tmp_path, document)
    gateway = tmp_path / "not-executed-gateway"
    gateway.write_text("An inert existence check only.\n")
    monkeypatch.setattr(host, "console_configuration", lambda *args: False)
    monkeypatch.setattr(host, "validate_configuration_values", lambda *args: None)
    events = []
    def forbidden(*args, **kwargs):
        events.append("resource or application effect")
        pytest.fail("Invalid selected source reached effects.")
    monkeypatch.setattr(host, "ConnectorTools", forbidden)
    monkeypatch.setattr(host, "MCPPool", forbidden)
    monkeypatch.setattr(host.Application, "load", forbidden)
    startup = SimpleNamespace(gateway=SimpleNamespace(path=gateway), prepare=forbidden)
    with pytest.raises(ValueError):
        await host.run(spec, SimpleNamespace(), application_startup=startup)
    assert events == []
    assert not (tmp_path / "data").exists()
    assert not (tmp_path / ".mobkit/state").exists()


@pytest.mark.parametrize("mutation", ["replace", "remove", "add-after-absence"])
async def test_loaded_source_change_refuses_before_catalog_acceptance(tmp_path, monkeypatch, mutation):
    host, spec = generated_host(tmp_path, monkeypatch)
    document = selection(spec)
    if mutation != "add-after-absence":
        write_selection(tmp_path, document)
    loaded = application(host, spec)
    await loaded.load()
    if mutation == "replace":
        changed = deepcopy(document)
        changed["revision"] = 2
        write_selection(tmp_path, changed)
    elif mutation == "remove":
        (tmp_path / "application/tool-policy.json").unlink()
    else:
        write_selection(tmp_path, document)
    with pytest.raises(ValueError, match="source changed"):
        await loaded.validate_tool_policy_catalog({row["identity"]: [] for row in spec["agents"]})
    await loaded.stop()


@pytest.mark.parametrize("kind", ["file-symlink", "fifo", "directory-symlink"])
async def test_nonordinary_policy_source_refuses_without_blocking(tmp_path, monkeypatch, kind):
    host, spec = generated_host(tmp_path, monkeypatch)
    target = tmp_path / "original-policy.json"
    target.write_text(json.dumps(selection(spec)))
    path = tmp_path / "application/tool-policy.json"
    if kind == "file-symlink":
        path.symlink_to(target)
    elif kind == "fifo":
        os.mkfifo(path)
    else:
        directory = tmp_path / "application"
        directory.rename(tmp_path / "original-application")
        directory.symlink_to(tmp_path / "original-application", target_is_directory=True)
    loaded = application(host, spec)
    with pytest.raises(ValueError, match="ordinary"):
        await asyncio.wait_for(loaded.load_tool_policy(), 1)
    assert loaded.tool_policy_source_bytes is None


async def test_policy_read_runs_off_loop_and_settles_on_cancellation(tmp_path, monkeypatch):
    host, spec = generated_host(tmp_path, monkeypatch)
    write_selection(tmp_path, selection(spec))
    loaded = application(host, spec)
    original = host._read_tool_policy_source
    entered, release, settled = threading.Event(), threading.Event(), threading.Event()
    loop_thread = threading.get_ident()
    def delayed(root, *, deployment=None):
        assert threading.get_ident() != loop_thread
        entered.set()
        assert release.wait(2)
        try:
            return original(root, deployment=deployment)
        finally:
            settled.set()
    monkeypatch.setattr(host, "_read_tool_policy_source", delayed)
    task = asyncio.create_task(loaded.load_tool_policy())
    try:
        assert await asyncio.to_thread(entered.wait, 30)
        task.cancel()
        await asyncio.sleep(0)
        assert not task.done()
        release.set()
        with pytest.raises(asyncio.CancelledError):
            await task
        assert settled.is_set()
        assert not loaded._tool_policy_loaded
        assert loaded.tool_policy_source_bytes is None
    finally:
        release.set()
        if not task.done():
            await asyncio.gather(task, return_exceptions=True)


async def test_fixed_policy_does_not_admit_dynamic_roster(tmp_path, monkeypatch):
    host, spec = generated_host(tmp_path, monkeypatch)
    write_selection(tmp_path, selection(spec))
    path = tmp_path / "application/__init__.py"
    path.write_text(APPLICATION.replace("    def tools_for_agent(self, agent):", '''    def desired_roster(self):
        raise AssertionError("Fixed selection must refuse before dynamic snapshot effects.")
    def desired_edges(self, identities): return []
    def tools_for_agent(self, agent):'''))
    loaded = application(host, spec)
    with pytest.raises(ValueError, match="fixed durable roster"):
        await loaded.load()
    await loaded.stop()


async def test_application_check_reports_unresolved_names_without_native_acceptance(
        tmp_path, monkeypatch, capsys):
    host, spec = generated_host(tmp_path, monkeypatch)
    write_selection(tmp_path, selection(spec, {spec["entrypoint"]: ["custom_read", "setup_report"]}))
    await host.application_check(spec)
    output = capsys.readouterr().out
    assert "Application tool policy source validated: 1 selected names match available callbacks" in output
    assert "1 tool selections await native surface validation" in output
    assert "Source intent does not authorize tools" in output


async def test_schedule_hook_cannot_return_with_changed_policy_source(tmp_path, monkeypatch):
    host, spec = generated_host(tmp_path, monkeypatch)
    original = write_selection(tmp_path, selection(spec))
    path = tmp_path / "application/__init__.py"
    path.write_text(APPLICATION.replace("    def tools_for_agent(self, agent):", '''    async def schedule_runnables(self):
        path = self.context.root / "application/tool-policy.json"
        path.write_bytes(path.read_bytes() + b" ")
        return []
    def tools_for_agent(self, agent):'''))
    loaded = application(host, spec)
    with pytest.raises(ValueError, match="source changed"):
        await loaded.load()
    assert loaded.tool_policy_source_bytes == original
    await loaded.stop()


@pytest.mark.parametrize("handoff", ["selected-prepare", "ordinary-storage", "connect"])
async def test_awaited_owner_source_change_refuses_before_following_runtime_work(
        tmp_path, monkeypatch, handoff):
    host, spec = generated_host(tmp_path, monkeypatch)
    source = write_selection(tmp_path, selection(spec))
    gateway = tmp_path / "not-executed-gateway"
    gateway.write_text("Not executed.\n")
    events = []
    monkeypatch.setattr(host, "gateway_path", lambda: gateway)
    monkeypatch.setattr(host, "console_configuration", lambda *args: False)
    monkeypatch.setattr(host, "validate_configuration_values", lambda *args: None)
    monkeypatch.setattr(host, "local_tools", lambda *args: [])
    monkeypatch.setattr(asyncio.get_running_loop(), "add_signal_handler", lambda *args: None)
    class Tools:
        def __init__(self, *args): events.append("tools")
        async def close(self): events.append("tools close")
    class Pool:
        def __init__(self, spec): self.connections = {}
        def for_class(self, item): return []
        async def start(self): events.append("pool start")
        async def close(self): events.append("pool close")
    monkeypatch.setattr(host, "ConnectorTools", Tools)
    monkeypatch.setattr(host, "MCPPool", Pool)
    def change_source():
        (tmp_path / "application/tool-policy.json").write_bytes(source + b" ")
    class Startup:
        def __init__(self): self.gateway = SimpleNamespace(path=gateway)
        async def prepare(self, loaded):
            events.append("prepare")
            assert loaded.tool_policy_source_bytes == source
            change_source()
        def builder_config(self, **options): pytest.fail("Stale prepare reached builder.")
    import storage_setup
    async def storage(root, build):
        events.append("storage")
        if handoff == "ordinary-storage": change_source()
    monkeypatch.setattr(storage_setup, "prepare_storage", storage)
    class Runtime:
        async def identity_bootstrap_status(self): pytest.fail("Stale connection reached bootstrap.")
        async def shutdown(self): events.append("runtime shutdown")
    runtime = Runtime()
    async def connect(config, loaded, *, application_startup):
        assert handoff == "connect"
        events.append("connect")
        change_source()
        return runtime
    monkeypatch.setattr(host, "connect_application_runtime", connect)
    args = SimpleNamespace(activate_revision=None, connections_check=False)
    startup = Startup() if handoff == "selected-prepare" else None
    with pytest.raises(ValueError, match="source changed"):
        await host.run(spec, args, application_startup=startup)
    assert events[-2:] == ["pool close", "tools close"]
    if handoff == "selected-prepare":
        assert "pool start" not in events
    elif handoff == "ordinary-storage":
        assert "connect" not in events
    else:
        assert "runtime shutdown" in events


async def test_direct_selected_connect_refuses_stale_intent_before_owner_call(tmp_path, monkeypatch):
    host, spec = generated_host(tmp_path, monkeypatch)
    source = write_selection(tmp_path, selection(spec))
    loaded = application(host, spec)
    await loaded.load()
    (tmp_path / "application/tool-policy.json").write_bytes(source + b" ")
    class Startup:
        async def connect(self, *args): pytest.fail("Stale direct connection reached native owner.")
    with pytest.raises(ValueError, match="source changed"):
        await host.connect_application_runtime(SimpleNamespace(), loaded, application_startup=Startup())
    await loaded.stop()


async def test_direct_selected_connect_settles_stale_runtime_cleanup_through_cancellation(
        tmp_path, monkeypatch):
    host, spec = generated_host(tmp_path, monkeypatch)
    source = write_selection(tmp_path, selection(spec))
    loaded = application(host, spec)
    await loaded.load()
    entered, release, finished = asyncio.Event(), asyncio.Event(), asyncio.Event()
    runtime = SimpleNamespace()
    class Startup:
        async def connect(self, config, selected_application):
            selected_application.context.runtime = runtime
            (tmp_path / "application/tool-policy.json").write_bytes(source + b" ")
            return runtime
        async def shutdown(self, selected_runtime):
            assert selected_runtime is runtime
            entered.set()
            await release.wait()
            finished.set()
    task = asyncio.create_task(host.connect_application_runtime(
        SimpleNamespace(), loaded, application_startup=Startup()))
    try:
        await asyncio.wait_for(entered.wait(), 30)
        task.cancel()
        await asyncio.sleep(0)
        task.cancel()
        await asyncio.sleep(0)
        assert not task.done()
        release.set()
        with pytest.raises(ValueError, match="source changed"):
            await task
        assert finished.is_set()
        assert loaded.context.runtime is None
    finally:
        release.set()
        if not task.done(): await asyncio.gather(task, return_exceptions=True)
        await loaded.stop()


async def test_same_size_in_read_mutation_with_restored_mtime_refuses(tmp_path, monkeypatch):
    host, spec = generated_host(tmp_path, monkeypatch)
    source = write_selection(tmp_path, selection(spec))
    path = tmp_path / "application/tool-policy.json"
    before = path.stat()
    original_fstat = os.fstat
    calls = 0
    def mutate_after_read(descriptor):
        nonlocal calls
        observed = original_fstat(descriptor)
        if (observed.st_dev, observed.st_ino) == (before.st_dev, before.st_ino):
            calls += 1
            if calls == 2:
                # Preserve size and mtime so change-time is the relevant custody observation.
                path.write_bytes(source[:-1] + b" ")
                assert path.stat().st_size == before.st_size
                os.utime(path, ns=(before.st_atime_ns, before.st_mtime_ns))
                return original_fstat(descriptor)
        return observed
    monkeypatch.setattr(host.os, "fstat", mutate_after_read)
    loaded = application(host, spec)
    with pytest.raises(ValueError, match="changed while"):
        await loaded.load_tool_policy()
    assert not loaded._tool_policy_loaded


@pytest.mark.parametrize("fault", [None, "future-member", "missing-seed"])
async def test_helperless_dynamic_roster_keeps_policy_coverage_on_exact_seeds(tmp_path, monkeypatch, fault):
    from launchkit.blueprint import Blueprint
    from launchkit.connect_policy_input import APPLICATION_INVENTORY_PATH, APPLICATION_NATIVE_POLICY_PATH
    from test_profile_only_classes import profile_plan
    host, spec = generated_host(tmp_path, monkeypatch, blueprint=Blueprint.model_validate(profile_plan()))
    baseline = write_selection(tmp_path, selection(spec))
    spec["applicationInventory"] = APPLICATION_INVENTORY_PATH
    policy = selection(spec, {spec["entrypoint"]: ["custom_read", "native_pending"]})
    if fault == "future-member":
        policy["members"].append({"member_identity": "resident:future", "grants": []})
    elif fault == "missing-seed":
        policy["members"] = []
    selected = json.dumps(policy, indent=3).encode() + b"\n"
    (tmp_path / APPLICATION_NATIVE_POLICY_PATH).write_bytes(selected)
    source = APPLICATION.replace("    def tools_for_agent(self, agent):", '''    def desired_roster(self):
        return [{"identity": row["identity"], "classId": row["classId"]}
                for row in self.context.deployment["agents"]]
    def desired_edges(self, identities): return []
    def tools_for_agent(self, agent):
        assert agent["identity"] in {row["identity"] for row in self.context.deployment["agents"]}
''')
    (tmp_path / "application/__init__.py").write_text(source)
    loaded = application(host, spec)
    try:
        if fault is not None:
            with pytest.raises(ValueError, match="member"):
                await loaded.load()
            return
        await loaded.load_tool_policy(required=True)
        await loaded.load()
        assert loaded.dynamic
        assert set(loaded._snapshot.agents) == {spec["entrypoint"]}
        assert loaded._tool_catalog["resident"] == {}
        assert loaded.tool_policy_source_bytes == selected
        assert loaded.tool_policy_selection == policy
        assert loaded.tool_policy_matches == {spec["entrypoint"]: ("custom_read",)}
        assert loaded.tool_policy_unresolved == {spec["entrypoint"]: ("native_pending",)}
        # A class declaration never extends the fixed-seed policy catalog.
        with pytest.raises(ValueError, match="every fixed identity"):
            await loaded.validate_tool_policy_catalog({**{row["identity"]: [] for row in spec["agents"]},
                "resident:future": []})
        await loaded.require_tool_policy_current()
        assert loaded.context.runtime is None
    finally:
        await loaded.stop()
        assert (tmp_path / "application/tool-policy.json").read_bytes() == baseline
