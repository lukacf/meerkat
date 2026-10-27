import asyncio
import copy
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
from types import SimpleNamespace

import pytest

from launchkit.blueprint import starter_blueprint
from launchkit.compiler import generate_files
from console_source_fixture import write_console_sources


def load_host(monkeypatch, root):
    templates = Path(__file__).parents[1] / "launchkit"
    monkeypatch.syspath_prepend(str(templates))
    monkeypatch.syspath_prepend(str(templates / "templates"))
    monkeypatch.syspath_prepend(str(templates / "templates_v2"))
    definition = importlib.util.spec_from_file_location("application_test_host", templates / "templates_v2/app.py")
    module = importlib.util.module_from_spec(definition)
    definition.loader.exec_module(module)
    monkeypatch.setattr(module, "ROOT", root)
    return module


def write_application(root, adapter):
    directory = root / "application"
    directory.mkdir()
    (directory / "__init__.py").write_text("from .adapters import Integration\ndef build(context):\n    return Integration(context)\n")
    (directory / "adapters.py").write_text(adapter)


APPLICATION = '''class Integration:
    def __init__(self, context):
        self.context = context

    async def tools_for_agent(self, agent):
        if agent["classId"] != "lead":
            return []
        return [{"name": "custom_read", "description": "Read application state",
                 "schema": {"type": "object", "properties": {}}, "handler": self.read}]

    async def read(self, arguments):
        return {"identity": self.context.deployment["entrypoint"], "arguments": arguments}

    def schedule_runnables(self):
        return [{"name": "application.review", "handler": self.review}]

    async def review(self, occurrence):
        return {"reviewed": occurrence["occurrence_id"], "payload": occurrence.get("payload")}

    async def start(self):
        self.context.events.append("application_start")
        assert self.context.runtime is not None
        self.context.stop_event.set()
        if self.context.fail_start:
            raise RuntimeError("adapter start failed")

    async def stop(self):
        if hasattr(self.context, "events"):
            self.context.events.append("application_stop")
'''


async def supported_host_carriers(runtime, method, params):
    assert method == "mobkit/capabilities"
    return {"host_system_prompt": {"send": True, "dispatch": True}}


async def test_launcher_readiness_attests_native_bootstrap_over_private_pipe(tmp_path, monkeypatch):
    module = load_host(monkeypatch, tmp_path)
    reader, writer = os.pipe()
    monkeypatch.setenv("MOBKIT_LAUNCH_READY_FD", str(writer))
    observed = {"complete": True, "ready": True, "counts": {"broken": 0},
                "identities": {"lead:main": {"state": "ready"}}}
    class Runtime:
        rust_http_base_url = "http://127.0.0.1:61337"
        async def identity_bootstrap_status(self):
            return SimpleNamespace(complete=True, ready=True, counts=SimpleNamespace(broken=0),
                                   to_dict=lambda: observed)
    try:
        await module.report_host_readiness(Runtime())
        with os.fdopen(reader) as channel:
            payload = json.loads(channel.readline())
        assert payload == {"pid": os.getpid(), "bootstrap": observed,
                           "console_url": "http://127.0.0.1:61337/console"}
        assert "MOBKIT_LAUNCH_READY_FD" not in os.environ
    finally:
        for descriptor in (reader, writer):
            try:
                os.close(descriptor)
            except OSError:
                pass


@pytest.mark.parametrize("advertised", [392_000, None], ids=["advertised", "sdk-without-accessor"])
async def test_launcher_readiness_reports_the_gateway_shutdown_horizon(tmp_path, monkeypatch, advertised):
    # The worker's stop waits this long for the gateway's own cleanup. An SDK
    # without the accessor reports nothing and the worker uses its fallback.
    module = load_host(monkeypatch, tmp_path)
    reader, writer = os.pipe()
    monkeypatch.setenv("MOBKIT_LAUNCH_READY_FD", str(writer))
    observed = {"complete": True, "ready": True, "counts": {"broken": 0},
                "identities": {"lead:main": {"state": "ready"}}}
    class Runtime:
        rust_http_base_url = "http://127.0.0.1:61337"
        async def identity_bootstrap_status(self):
            return SimpleNamespace(complete=True, ready=True, counts=SimpleNamespace(broken=0),
                                   to_dict=lambda: observed)
    if advertised is not None:
        Runtime.gateway_shutdown_horizon_ms = advertised
    try:
        await module.report_host_readiness(Runtime())
        with os.fdopen(reader) as channel:
            payload = json.loads(channel.readline())
        expected = {"pid": os.getpid(), "bootstrap": observed, "console_url": "http://127.0.0.1:61337/console"}
        if advertised is not None:
            expected["gateway_shutdown_horizon_ms"] = advertised
        assert payload == expected
    finally:
        for descriptor in (reader, writer):
            try:
                os.close(descriptor)
            except OSError:
                pass


async def test_launcher_never_attests_broken_native_identities(tmp_path, monkeypatch):
    module = load_host(monkeypatch, tmp_path)
    reader, writer = os.pipe()
    monkeypatch.setenv("MOBKIT_LAUNCH_READY_FD", str(writer))
    class Runtime:
        async def identity_bootstrap_status(self):
            return SimpleNamespace(complete=True, ready=False, counts=SimpleNamespace(broken=1))
    try:
        with pytest.raises(RuntimeError, match="not ready"):
            await module.report_host_readiness(Runtime())
        os.set_blocking(reader, False)
        with pytest.raises(BlockingIOError):
            os.read(reader, 1)
    finally:
        os.close(reader)
        os.close(writer)


async def test_launcher_startup_stop_closes_claimed_pipe_without_status_request(tmp_path, monkeypatch):
    module = load_host(monkeypatch, tmp_path)
    stop = asyncio.Event()
    stop.set()
    reader, writer = os.pipe()
    monkeypatch.setenv("MOBKIT_LAUNCH_READY_FD", str(writer))

    class Runtime:
        async def identity_bootstrap_status(self):
            pytest.fail("An observed stop must not start another readiness status request")

    try:
        await module.report_host_readiness(Runtime(), stop=stop)
        assert "MOBKIT_LAUNCH_READY_FD" not in os.environ
        with pytest.raises(OSError):
            os.fstat(writer)
        os.set_blocking(reader, False)
        assert os.read(reader, 1) == b""
    finally:
        for descriptor in (reader, writer):
            try:
                os.close(descriptor)
            except OSError:
                pass


@pytest.mark.parametrize("stop_stage,startup_fails,front_terminal", [
    (stage, fails, False)
    for stage in ("load", "selection", "connect", "bootstrap", "application", "front", "readiness")
    for fails in (False, True)
] + [("pool", False, False), ("front", False, True)])
async def test_service_startup_stop_preserves_entered_stage_and_cleanup_without_readiness(
        tmp_path, monkeypatch, stop_stage, startup_fails, front_terminal):
    import meerkat_mobkit

    module = load_host(monkeypatch, tmp_path)
    files = generate_files(starter_blueprint())
    spec = json.loads(files["deployment.json"])
    for name in ("config/mob.toml", "console_shell.mjs", "console_shell.css"):
        path = tmp_path / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(files[name])
    source = APPLICATION.replace("self.context.stop_event.set()",
                                 "await self.context.pause_startup('application')")
    source = source.replace("    async def tools_for_agent(self, agent):", '''    async def select_host_tools(self, agent, available_names):
        self.context.selection_calls.append(agent["identity"])
        if len(self.context.selection_calls) == 1:
            await self.context.pause_startup("selection")
        return list(available_names)

    async def tools_for_agent(self, agent):
        if not self.context.load_observed:
            self.context.load_observed = True
            await self.context.pause_startup("load")''')
    write_application(tmp_path, source)
    write_console_sources(tmp_path, spec)
    binary = tmp_path / "gateway"
    binary.touch()
    monkeypatch.setattr(module, "gateway_path", lambda: binary)
    monkeypatch.setattr(module, "console_configuration", lambda root, fields: True)
    # This inert lifecycle test never creates credentials or opens a login.
    monkeypatch.setattr(module, "validate_console_configuration_values", lambda *args: None)
    monkeypatch.setattr(module, "validate_configuration_values", lambda *args: None)
    monkeypatch.setenv("OPENAI_API_KEY", "isolated-test-value")
    monkeypatch.delenv("MOBKIT_CONNECT_BIN", raising=False)
    events, contexts = [], []
    entered, released = asyncio.Event(), asyncio.Event()
    failure = OSError(f"{stop_stage} failed while stop was requested")

    async def pause_startup(stage):
        events.append(stage)
        if stage == stop_stage:
            entered.set()
            await released.wait()
            events.append(f"joined_{stage}")
            if startup_fails:
                raise failure

    context_type = module.ApplicationContext
    def context(root, deployment, stop_event):
        value = context_type(root, deployment, stop_event)
        value.events, value.fail_start = events, False
        value.pause_startup = pause_startup
        value.selection_calls = []
        value.load_observed = False
        contexts.append(value)
        return value
    monkeypatch.setattr(module, "ApplicationContext", context)

    class LocalTools:
        def __init__(self, *args):
            pass
        def for_agent(self, agent):
            return []
        async def close(self):
            events.append("tools_close")

    class Pool:
        def __init__(self, spec):
            pass
        def for_class(self, item):
            return []
        async def start(self):
            await pause_startup("pool")
        async def close(self):
            events.append("pool_close")

    class Runtime:
        rust_http_base_url = "http://127.0.0.1:61337"
        def __init__(self, config):
            self.status_calls = 0
        def on_schedule_fire(self, name, handler):
            pass
        async def connect(self):
            await pause_startup("connect")
        async def identity_bootstrap_status(self):
            self.status_calls += 1
            await pause_startup("bootstrap" if self.status_calls == 1 else "readiness")
            return SimpleNamespace(complete=True, ready=True, counts=SimpleNamespace(broken=0),
                to_dict=lambda: {"complete": True, "ready": True, "counts": {"broken": 0}})
        async def shutdown(self):
            events.append("runtime_shutdown")
    Runtime._rpc = supported_host_carriers

    class Front:
        native_auth = {"mode": "fixture-auth-contract"}
        def __init__(self, *args, runtime_namespace):
            assert runtime_namespace == "mobkit"
            self.task = None
        async def start(self, url):
            assert url == Runtime.rust_http_base_url
            if not front_terminal:
                self.task = asyncio.create_task(asyncio.Event().wait())
            await pause_startup("front")
            if front_terminal:
                async def failed_server():
                    raise failure
                self.task = asyncio.create_task(failed_server())
                await asyncio.wait((self.task,))
                assert self.task.done()
        async def close(self):
            events.append("front_close")
            if self.task is not None:
                self.task.cancel()
                await asyncio.gather(self.task, return_exceptions=True)

    async def prepare_storage(root, factory):
        events.append("storage_prepare")
    original_service_wait = module.await_service_shutdown
    async def service_wait(stop, front):
        events.append("service_wait")
        assert stop.is_set()
        await original_service_wait(stop, front)

    monkeypatch.setattr(module, "ConnectorTools", LocalTools)
    monkeypatch.setattr(module, "MCPPool", Pool)
    monkeypatch.setattr(module, "ConsoleFront", Front)
    monkeypatch.setattr(module, "await_service_shutdown", service_wait)
    monkeypatch.setattr(meerkat_mobkit, "MobKitRuntime", Runtime)
    monkeypatch.setitem(sys.modules, "storage_setup", SimpleNamespace(prepare_storage=prepare_storage))
    monkeypatch.setattr(asyncio.get_running_loop(), "add_signal_handler", lambda *args: None)
    reader, writer = os.pipe()
    monkeypatch.setenv("MOBKIT_LAUNCH_READY_FD", str(writer))
    args = SimpleNamespace(connections_check=False, activate_revision=None, definition_revision=False,
                           list_flows=False, run_flow=None, smoke=False)
    running = asyncio.create_task(module.run(spec, args))
    try:
        try:
            await asyncio.wait_for(entered.wait(), 2)
        except TimeoutError:
            if running.done():
                await running
            raise
        assert not running.done(), "The entered startup stage is retained until it settles"
        contexts[0].stop_event.set()
        await asyncio.sleep(0)
        assert not running.done(), "Stop must not abandon the still-withheld startup stage"
        assert not any(event in events for event in
                       {"front_close", "application_stop", "runtime_shutdown", "pool_close", "tools_close"})
        released.set()
        if startup_fails or front_terminal:
            with pytest.raises(OSError) as raised:
                await running
            assert raised.value is failure
        else:
            await running
        stages = ("load", "pool", "selection", "connect", "bootstrap", "application", "front", "readiness")
        assert [event for event in events if event in stages] == list(stages[:stages.index(stop_stage) + 1])
        assert events.count(f"joined_{stop_stage}") == 1
        assert events.count("service_wait") == int(stop_stage in {"front", "readiness"} and not startup_fails)
        if stop_stage in {"load", "pool", "selection"}:
            assert "storage_prepare" not in events
        if stop_stage in {"load", "pool"}:
            assert contexts[0].selection_calls == [], "Stopped setup must not enter application host selection"
        elif stop_stage == "selection":
            assert contexts[0].selection_calls == [spec["agents"][0]["identity"]], "Stop must prevent selection for subsequent agents"
        else:
            assert contexts[0].selection_calls == [agent["identity"] for agent in spec["agents"]]
        cleanup = [event for event in events if event in
                   {"front_close", "application_stop", "runtime_shutdown", "pool_close", "tools_close"}]
        expected = ["front_close", "application_stop", "pool_close", "tools_close"]
        if stop_stage not in {"load", "pool", "selection"}:
            expected.insert(0 if stop_stage == "connect" and startup_fails else 2, "runtime_shutdown")
        assert cleanup == expected
        if stop_stage == "readiness" and not startup_fails:
            assert "MOBKIT_LAUNCH_READY_FD" not in os.environ
            with pytest.raises(OSError):
                os.fstat(writer)
        else:
            # Before readiness, the call has not claimed the inherited pipe.
            # A failed status request retains its existing descriptor behavior.
            os.fstat(writer)
            os.close(writer)
        os.set_blocking(reader, False)
        assert os.read(reader, 1) == b"", "A stopped startup must publish no readiness payload"
    finally:
        released.set()
        if not running.done():
            running.cancel()
        await asyncio.gather(running, return_exceptions=True)
        for descriptor in (reader, writer):
            try:
                os.close(descriptor)
            except OSError:
                pass


async def test_service_front_failure_wakes_host_shutdown_instead_of_leaving_agents_unreachable(tmp_path, monkeypatch):
    module = load_host(monkeypatch, tmp_path)
    stop = asyncio.Event()
    async def failed_front():
        await asyncio.sleep(0)
        raise RuntimeError("front failed")
    task = asyncio.create_task(failed_front())
    with pytest.raises(RuntimeError, match="front failed"):
        await module.await_service_shutdown(stop, SimpleNamespace(task=task))
    assert task.done() and not stop.is_set()


async def test_service_normal_shutdown_does_not_cancel_owned_front_before_cleanup(tmp_path, monkeypatch):
    module = load_host(monkeypatch, tmp_path)
    stop = asyncio.Event()
    task = asyncio.create_task(asyncio.Event().wait())
    stop.set()
    try:
        await module.await_service_shutdown(stop, SimpleNamespace(task=task))
        assert not task.done()
    finally:
        task.cancel()
        await asyncio.gather(task, return_exceptions=True)


async def test_service_completed_front_failure_wins_over_concurrent_requested_stop(tmp_path, monkeypatch):
    module = load_host(monkeypatch, tmp_path)
    stop = asyncio.Event()
    failure = OSError("Console listener failed while shutdown was requested")

    async def failed_front():
        stop.set()
        raise failure

    task = asyncio.create_task(failed_front())
    await asyncio.wait((task,))
    assert task.done() and stop.is_set()
    try:
        with pytest.raises(OSError) as raised:
            await module.await_service_shutdown(stop, SimpleNamespace(task=task))
        assert raised.value is failure
    finally:
        await asyncio.gather(task, return_exceptions=True)


async def test_service_completed_clean_front_allows_concurrent_requested_stop(tmp_path, monkeypatch):
    module = load_host(monkeypatch, tmp_path)
    stop = asyncio.Event()

    async def clean_front():
        stop.set()

    task = asyncio.create_task(clean_front())
    await asyncio.wait((task,))
    assert task.done() and stop.is_set()
    await module.await_service_shutdown(stop, SimpleNamespace(task=task))
    assert task.result() is None and not task.cancelled()


async def test_service_completed_clean_front_without_requested_stop_is_unexpected(tmp_path, monkeypatch):
    module = load_host(monkeypatch, tmp_path)
    stop = asyncio.Event()

    async def clean_front():
        return None

    task = asyncio.create_task(clean_front())
    await asyncio.wait((task,))
    assert task.done() and not stop.is_set()
    with pytest.raises(RuntimeError, match=r"The Console login front stopped unexpectedly\."):
        await module.await_service_shutdown(stop, SimpleNamespace(task=task))
    assert task.result() is None and not task.cancelled()


async def test_host_rejects_untrusted_console_hook_and_keeps_authenticated_hook_out_of_agent_tools(tmp_path, monkeypatch):
    module = load_host(monkeypatch, tmp_path)
    spec = json.loads(generate_files(starter_blueprint())["deployment.json"])
    write_application(tmp_path, APPLICATION + '\n    async def on_console_input(self, event):\n        self.context.received = event\n')
    context = module.ApplicationContext(tmp_path, spec, asyncio.Event())
    application = module.Application(context)
    with pytest.raises(ValueError, match="requires local-password"):
        await application.load()
    write_console_sources(tmp_path, spec)
    await application.load()
    assert all(tool["name"] != "on_console_input" for tools in application.tools.values() for tool in tools)
    evidence = SimpleNamespace(principal="host-user", interaction_id="native-interaction")
    await application.on_console_input(evidence)
    assert context.received is evidence


async def test_application_tools_are_class_scoped_without_starting_adapters(tmp_path, monkeypatch):
    module = load_host(monkeypatch, tmp_path)
    spec = json.loads(generate_files(starter_blueprint())["deployment.json"])
    write_application(tmp_path, APPLICATION)
    context = module.ApplicationContext(tmp_path, spec, asyncio.Event())
    context.events = []
    application = module.Application(context)
    try:
        await application.load()
        assert [tool["name"] for tool in application.tools["lead"]] == ["custom_read"]
        assert all(not tools for profile, tools in application.tools.items() if profile != "lead")
        assert context.runtime is None and context.events == []
        assert list(application.runnables) == ["application.review"]
        result = await application.tools["lead"][0]["handler"]({"selected": "scope"})
        assert result == {"identity": "lead:main", "arguments": {"selected": "scope"}}
    finally:
        await application.stop()
    assert context.events == ["application_stop"]


@pytest.mark.parametrize("definition,error", [
    ('{}', "must return a list"),
    ('[{"name": " ", "handler": self.review}]', "Malformed application schedule runnable"),
    ('[{"name": "application.review", "handler": None}]', "Malformed application schedule runnable"),
    ('[{"name": "application.review", "handler": self.review}] * 2', "Duplicate application schedule runnable"),
])
async def test_application_rejects_invalid_schedule_runnables(tmp_path, monkeypatch, definition, error):
    module = load_host(monkeypatch, tmp_path)
    spec = json.loads(generate_files(starter_blueprint())["deployment.json"])
    source = APPLICATION.replace('[{"name": "application.review", "handler": self.review}]', definition)
    write_application(tmp_path, source)
    application = module.Application(module.ApplicationContext(tmp_path, spec, asyncio.Event()))
    try:
        with pytest.raises(ValueError, match=error):
            await application.load()
    finally:
        await application.stop()


async def test_application_runnable_uses_native_occurrence_callback_contract(tmp_path, monkeypatch):
    from meerkat_mobkit.builder import MobKitBuilder
    from meerkat_mobkit.runtime import MobKitRuntime

    module = load_host(monkeypatch, tmp_path)
    spec = json.loads(generate_files(starter_blueprint())["deployment.json"])
    original_grants = [agent["tools"]["schedule"] for agent in spec["agents"]]
    write_application(tmp_path, APPLICATION)
    application = module.Application(module.ApplicationContext(tmp_path, spec, asyncio.Event()))
    try:
        await application.load()
        builder = MobKitBuilder().host_runnables(list(application.runnables))
        runtime = MobKitRuntime(builder._config)
        assert runtime._build_init_params()["runtime_options"]["host_runnables"] == ["application.review"]
        application.bind_schedule_runnables(runtime)
        occurrence = {"schedule_id": "schedule-1", "occurrence_id": "occurrence-2",
                      "due_at": "2026-10-01T07:00:00+00:00", "payload": {"scope": "local"}}
        result = await runtime._dispatcher.handle_callback(
            "callback/schedule_fire", {"runnable": "application.review", "occurrence": occurrence})
        assert result == {"reviewed": "occurrence-2", "payload": {"scope": "local"}}
        assert [agent["tools"]["schedule"] for agent in spec["agents"]] == original_grants
        assert application.context.runtime is None
    finally:
        await application.stop()


async def test_preconnect_binding_handles_restored_due_callback_with_shipping_sdk(tmp_path, monkeypatch):
    from meerkat_mobkit import MobKitRuntime
    from meerkat_mobkit.builder import MobKitBuilderConfig

    module = load_host(monkeypatch, tmp_path)
    context = module.ApplicationContext(tmp_path, {"agents": []}, asyncio.Event())
    application = module.Application(context)
    (tmp_path / "config").mkdir()
    (tmp_path / "config/mob.toml").write_text("[mob]\nid='fixture'\n")
    occurrences = []

    async def handler(occurrence):
        assert context.runtime is not None
        occurrences.append(occurrence)
        return {"completed": occurrence["occurrence_id"]}

    application.runnables["owned.review"] = handler
    config = MobKitBuilderConfig(mob_config_path=str(tmp_path / "config/mob.toml"), host_runnables=["owned.review"])
    occurrence = {"schedule_id": "owned-schedule", "occurrence_id": "restored-occurrence",
                  "due_at": "2026-10-01T07:00:00Z", "payload": {"source": "restore"}}

    async def bootstrap(runtime):
        assert runtime is context.runtime
        assert runtime._build_init_params()["runtime_options"]["host_runnables"] == ["owned.review"]
        result = await runtime._dispatcher.handle_callback(
            "callback/schedule_fire", {"runnable": "owned.review", "occurrence": occurrence})
        assert result == {"completed": "restored-occurrence"}
        runtime._running = True

    monkeypatch.setattr(MobKitRuntime, "_bootstrap", bootstrap)
    monkeypatch.setattr(MobKitRuntime, "_rpc", supported_host_carriers)
    runtime = await module.connect_application_runtime(config, application)
    assert runtime.is_running and occurrences == [occurrence]
    await runtime.shutdown()


@pytest.mark.parametrize("failure", [RuntimeError("native init failed"), asyncio.CancelledError()])
async def test_failed_preconnect_runtime_is_reaped_and_context_cleared(tmp_path, monkeypatch, failure):
    import meerkat_mobkit

    module = load_host(monkeypatch, tmp_path)
    application = module.Application(module.ApplicationContext(tmp_path, {}, asyncio.Event()))
    (tmp_path / "config").mkdir()
    (tmp_path / "config/mob.toml").write_text("[mob]\nid='fixture'\n")
    events = []
    application.runnables["owned.review"] = lambda occurrence: None

    class Runtime:
        def __init__(self, config):
            pass
        def on_schedule_fire(self, name, handler):
            events.append("bound")
        async def connect(self):
            assert application.context.runtime is self
            events.append("connect")
            raise failure
        async def shutdown(self):
            events.append("shutdown")

    monkeypatch.setattr(meerkat_mobkit, "MobKitRuntime", Runtime)
    with pytest.raises(type(failure)):
        await module.connect_application_runtime(SimpleNamespace(mob_config_path=str(tmp_path / "config/mob.toml")), application)
    assert application.context.runtime is None
    assert events == ["bound", "connect", "shutdown"]


@pytest.mark.parametrize("new_sdk", [False, True])
async def test_host_turns_author_canonical_role_without_changing_keyed_admission_or_private_identity(tmp_path, monkeypatch, new_sdk):
    import meerkat_mobkit
    from meerkat_mobkit.identity_first_models import DispatchInput
    from meerkat_mobkit.runtime import IdentityAgentHandle

    module = load_host(monkeypatch, tmp_path)
    files = generate_files(starter_blueprint())
    native_path = tmp_path / "config/mob.toml"
    native_path.parent.mkdir()
    native_path.write_text(files["config/mob.toml"])
    spec = json.loads(files["deployment.json"])
    application = module.Application(module.ApplicationContext(tmp_path, spec, asyncio.Event()))
    calls, delivered = [], []

    class Runtime:
        def __init__(self, config):
            pass
        async def connect(self):
            pass
        async def shutdown(self):
            pass
        def agent(self, identity):
            return IdentityAgentHandle(self, identity)
        async def _rpc(self, method, params):
            calls.append((method, copy.deepcopy(params)))
            if method == "mobkit/capabilities":
                return {"host_system_prompt": {"send": True, "dispatch": True}}
            return "actual typed-admission stand-in"
        async def send(self, identity, content):
            pytest.fail("Legacy SDK send would lose native System content")
        async def dispatch(self, identity, dispatch_input):
            pytest.fail("Legacy SDK dispatch would lose native System content")

    if new_sdk:
        async def send(self, identity, content, *, system_prompt=None):
            delivered.append((identity, content, system_prompt))
            return "new SDK send admission"
        async def dispatch(self, identity, dispatch_input, *, system_prompt=None):
            delivered.append((identity, dispatch_input.to_dict(), system_prompt))
            return "new SDK keyed admission"
        Runtime.send, Runtime.dispatch = send, dispatch
    monkeypatch.setattr(meerkat_mobkit, "MobKitRuntime", Runtime)
    runtime = await module.connect_application_runtime(SimpleNamespace(mob_config_path=str(native_path)), application)
    identity = spec["agents"][0]["identity"]
    message = DispatchInput(content="New case, private preference unchanged", origin="system",
                            correlation_id="same-correlation", idempotency_key="same-key")
    handle = runtime.agent(identity)
    await handle.dispatch(message)
    await handle.send("Ordinary host question")
    if new_sdk:
        assert delivered[0][:2] == (identity, message.to_dict())
        system = delivered[0][2]
        assert delivered[1][:2] == (identity, "Ordinary host question")
    else:
        assert calls[1][0] == "mobkit/dispatch"
        assert calls[1][1]["identity"] == identity
        assert calls[1][1]["dispatch_input"] == message.to_dict()
        system = calls[1][1]["system_prompt"]
        assert calls[2][0] == "mobkit/send" and calls[2][1]["content"] == "Ordinary host question"
    assert sum(method == "mobkit/capabilities" for method, _ in calls) == 1
    assert "supersede earlier role directions" in system
    assert spec["classes"][0]["instructions"] in system
    assert "Preserve historical case facts, permissions, conversation and private context" in system
    before = len(calls)
    with pytest.raises(ValueError, match="canonical"):
        await runtime.send(identity, "ordinary", system_prompt="An application-selected role")
    assert len(calls) == before
    native_path.write_text(native_path.read_text() + "# external edit\n")
    with pytest.raises(RuntimeError, match="changed while running"):
        await handle.dispatch(message)
    assert len(calls) == before


@pytest.mark.parametrize("epoch", [1, 4])
async def test_shipping_legacy_gateway_allows_initial_use_but_never_claims_later_role_adoption(tmp_path, monkeypatch, epoch):
    import meerkat_mobkit
    from meerkat_mobkit.identity_first_models import DispatchInput
    from meerkat_mobkit.runtime import IdentityAgentHandle

    module = load_host(monkeypatch, tmp_path)
    files = generate_files(starter_blueprint())
    native_path = tmp_path / "config/mob.toml"
    native_path.parent.mkdir()
    native_path.write_text(files["config/mob.toml"])
    spec = json.loads(files["deployment.json"])
    application = module.Application(module.ApplicationContext(tmp_path, spec, asyncio.Event()))
    calls, delivered = [], []

    class Runtime:
        def __init__(self, config):
            pass
        async def connect(self):
            pass
        async def shutdown(self):
            pass
        async def _rpc(self, method, params):
            calls.append(method)
            return {}
        def mob_handle(self):
            return self
        async def query_mob_events(self, options):
            calls.append("native ledger")
            assert options == {"event_types": ["mob_created", "mob_definition_updated"], "limit": 1000}
            return [SimpleNamespace(kind="mob_created", data={})] + ([
                SimpleNamespace(kind="mob_definition_updated", data={"epoch": epoch})] if epoch > 1 else [])
        def agent(self, identity):
            return IdentityAgentHandle(self, identity)
        async def send(self, identity, content):
            delivered.append(("send", identity, content))
            return "shipping SDK send admission"
        async def dispatch(self, identity, dispatch_input):
            delivered.append(("dispatch", identity, dispatch_input.to_dict()))
            return "shipping SDK keyed admission"

    monkeypatch.setattr(meerkat_mobkit, "MobKitRuntime", Runtime)
    monkeypatch.setattr(module, "verify_continuity_runtime", lambda *args, **kwargs: None)
    runtime = await module.connect_application_runtime(SimpleNamespace(mob_config_path=str(native_path), gateway_bin="fixture"), application)
    identity = spec["agents"][0]["identity"]
    dispatch = DispatchInput(content="Do useful work", origin="system", correlation_id="private-case", idempotency_key="same-key")
    if epoch == 1:
        assert await runtime.agent(identity).dispatch(dispatch) == "shipping SDK keyed admission"
        assert await runtime.agent(identity).send("A useful follow-up") == "shipping SDK send admission"
        assert delivered == [("dispatch", identity, dispatch.to_dict()), ("send", identity, "A useful follow-up")]
    else:
        with pytest.raises(RuntimeError, match="no turn was sent"):
            await runtime.agent(identity).dispatch(dispatch)
        with pytest.raises(RuntimeError, match="no turn was sent"):
            await runtime.agent(identity).send("A useful follow-up")
        assert delivered == []
    assert calls == ["mobkit/capabilities", "native ledger"] + (["mobkit/mob_events/query"] * 2 if epoch > 1 else [])
    await runtime.shutdown()


@pytest.mark.parametrize("change,error", [
    ('"handler": self.read', "valid"),
    ('"handler": None', "Malformed application callback"),
    ('"name": "bad.tool"', "Malformed application callback"),
])
async def test_application_checks_callback_contract(tmp_path, monkeypatch, change, error):
    module = load_host(monkeypatch, tmp_path)
    spec = json.loads(generate_files(starter_blueprint())["deployment.json"])
    source = APPLICATION
    if "handler" in change:
        source = source.replace('"handler": self.read', change)
    else:
        source = source.replace('"name": "custom_read"', change)
    write_application(tmp_path, source)
    application = module.Application(module.ApplicationContext(tmp_path, spec, asyncio.Event()))
    try:
        if error == "valid":
            await application.load()
        else:
            with pytest.raises(ValueError, match=error):
                await application.load()
    finally:
        await application.stop()


def test_generated_application_diagnostics_do_not_import_on_syntax_check(tmp_path):
    for name, text in generate_files(starter_blueprint()).items():
        path = tmp_path / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text)
    write_application(tmp_path, APPLICATION)
    init = tmp_path / "application/__init__.py"
    init.write_text("raise RuntimeError('import was executed')\n" + init.read_text())
    checked = subprocess.run([sys.executable, str(tmp_path / "app.py"), "--check"], capture_output=True, text=True)
    assert checked.returncode == 0, checked.stderr
    imported = subprocess.run([sys.executable, str(tmp_path / "app.py"), "--application-check"], capture_output=True, text=True)
    assert imported.returncode != 0 and "import was executed" in imported.stderr
    init.write_text("from .adapters import Integration\ndef build(context):\n    return Integration(context)\n")
    loaded = subprocess.run([sys.executable, str(tmp_path / "app.py"), "--application-check"], capture_output=True, text=True)
    assert loaded.returncode == 0, loaded.stderr
    assert "1 scoped callbacks" in loaded.stdout


@pytest.mark.parametrize("fail_start,collision,select_host_tools", [(False, False, False), (True, False, False), (False, True, False), (False, False, True)])
async def test_application_lifecycle_uses_native_builder_and_cleans_up_failed_start(tmp_path, monkeypatch, fail_start, collision, select_host_tools):
    from meerkat_mobkit import MobKit, MobKitRuntime
    from meerkat_mobkit.builder import MobKitBuilderConfig

    module = load_host(monkeypatch, tmp_path)
    files = generate_files(starter_blueprint())
    spec = json.loads(files["deployment.json"])
    (tmp_path / "config").mkdir()
    (tmp_path / "config/mob.toml").write_text(files["config/mob.toml"])
    source = APPLICATION
    if select_host_tools:
        source = source.replace("    async def tools_for_agent(self, agent):", '''    def select_host_tools(self, agent, available_names):
        if "mcp_start" not in self.context.events:
            raise RuntimeError("Host selection ran before connector startup")
        self.context.selection_calls.append((agent["identity"], available_names))
        return []

    async def tools_for_agent(self, agent):''')
    write_application(tmp_path, source)
    binary = tmp_path / "gateway"
    binary.touch()
    monkeypatch.setattr(module, "gateway_path", lambda: binary)
    monkeypatch.setenv("OPENAI_API_KEY", "isolated-test-value")
    events, registered, trust, selection_calls = [], {}, {}, []
    context_type = module.ApplicationContext

    def context(root, deployment, stop_event):
        value = context_type(root, deployment, stop_event)
        value.events, value.fail_start = events, fail_start
        value.selection_calls = selection_calls
        return value

    monkeypatch.setattr(module, "ApplicationContext", context)

    class LocalTools:
        def __init__(self, *args):
            pass
        def for_agent(self, agent):
            if collision:
                return [{"name": "custom_read", "handler": lambda args: {}, "description": "Existing tool", "schema": {}}]
            if select_host_tools:
                return [{"name": "notes_search", "handler": lambda args: "unselected synthetic shared note", "description": "Existing notes callback", "schema": {}}]
            return []
        async def close(self):
            events.append("local_close")

    class Pool:
        def __init__(self, spec):
            pass
        def for_class(self, item):
            return []
        async def start(self):
            events.append("mcp_start")
        async def close(self):
            events.append("mcp_close")

    class Runtime:
        rust_http_base_url = "http://127.0.0.1:0"
        def __init__(self, config):
            assert isinstance(config, MobKitBuilderConfig)
            assert config.host_runnables == ["application.review"]
            assert config.persistent_state == str(tmp_path / ".mobkit/state")
            assert config.console_require_app_auth is False
            previous = (MobKit.builder().mob(config.mob_config_path).gateway(config.gateway_bin)
                        .persistent_state(config.persistent_state).roster(config.roster_provider)
                        .topology_provider(config.topology_provider).session_service(config.session_builder)
                        .agent_memory(enabled=True, distiller={"enabled": False}, steward={"enabled": False},
                                      operator_scope="off", content_trust={"untrusted_tools": ["custom_read"]})
                        .console_config(config.console_config_path).http_listen(config.http_listen)
                        .console_auth_required(False).host_runnables(["application.review"]))
            assert MobKitRuntime(config)._build_init_params() == MobKitRuntime(previous._config)._build_init_params()
            assert MobKitRuntime(config)._build_init_params()["runtime_options"]["agent_memory"] == {
                "enabled": True, "distiller": {"enabled": False}, "steward": {"enabled": False},
                "operator_scope": "off", "content_trust": {"untrusted_tools": ["custom_read"]}}
            self.config, self.handlers = config, {}
            trust.update(config.agent_memory_config["content_trust"])
            events.append("native_construct")
        async def connect(self):
            # Restore may deliver during native initialization, before connect returns.
            assert self.handlers.keys() == {"application.review"}
            result = await self.handlers["application.review"]({"occurrence_id": "restored-due"})
            assert result["reviewed"] == "restored-due"
            for agent in spec["agents"]:
                def register(name, handler, **kwargs):
                    registered.setdefault(agent["identity"], []).append(name)
                draft = SimpleNamespace(profile_name=agent["profile"], resume_session_id=None,
                                        additional_instructions=[], register_tool=register)
                await self.config.session_builder.build_agent(draft)
            events.append("native_connect")
        async def identity_bootstrap_status(self):
            events.append("native_bootstrap")
            return SimpleNamespace(complete=True, ready=True, counts=SimpleNamespace(broken=0))
        async def shutdown(self):
            events.append("native_shutdown")
        def on_schedule_fire(self, name, handler):
            assert name == "application.review" and callable(handler)
            self.handlers[name] = handler
            events.append("schedule_bind")

    async def prepare_storage(root, factory):
        pass

    monkeypatch.setattr(module, "ConnectorTools", LocalTools)
    monkeypatch.setattr(module, "MCPPool", Pool)
    Runtime._rpc = supported_host_carriers
    monkeypatch.setitem(sys.modules, "meerkat_mobkit", SimpleNamespace(MobKitRuntime=Runtime))
    monkeypatch.setitem(sys.modules, "meerkat_mobkit.identity_first_models",
                        SimpleNamespace(DurableAgentSpec=SimpleNamespace, ManagedPeerEdge=SimpleNamespace))
    monkeypatch.setitem(sys.modules, "storage_setup", SimpleNamespace(prepare_storage=prepare_storage))
    args = SimpleNamespace(connections_check=False, activate_revision=None, definition_revision=False,
                           list_flows=False, run_flow=None, smoke=False)
    if collision:
        with pytest.raises(ValueError, match="collides with another tool"):
            await module.run(spec, args)
    elif fail_start:
        with pytest.raises(RuntimeError, match="adapter start failed"):
            await module.run(spec, args)
    else:
        await module.run(spec, args)
    if collision:
        assert registered == {} and trust == {}
        assert events == ["mcp_start", "application_stop", "mcp_close", "local_close"]
    else:
        assert registered == {"lead:main": ["custom_read"]}
        assert trust["untrusted_tools"] == ["custom_read"]
        assert events == ["mcp_start", "native_construct", "schedule_bind", "native_connect", "native_bootstrap", "application_start",
                          "application_stop", "native_shutdown", "mcp_close", "local_close"]
    if select_host_tools:
        assert selection_calls == [(agent["identity"], ("notes_search",)) for agent in spec["agents"]]


async def test_public_runtime_config_preserves_authenticated_revision_and_policy_files(tmp_path, monkeypatch):
    import meerkat_mobkit
    from meerkat_mobkit import MobKit, MobKitRuntime

    module = load_host(monkeypatch, tmp_path)
    files = generate_files(starter_blueprint())
    spec = json.loads(files["deployment.json"])
    for name in ("config/mob.toml", "config/gating.toml", "config/access.toml", "deployment/routing.toml"):
        path = tmp_path / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(files["config/mob.toml"] if name == "config/mob.toml" else "")
    binary = tmp_path / "gateway"
    binary.touch()
    monkeypatch.setattr(module, "gateway_path", lambda: binary)
    monkeypatch.setenv("OPENAI_API_KEY", "isolated-test-value")
    monkeypatch.setattr(module, "console_configuration", lambda root, fields: True)
    configs, lifecycle = [], []

    class Front:
        native_auth = {"mode": "fixture-auth-contract"}
        def __init__(self, *args, runtime_namespace):
            assert runtime_namespace == "mobkit"
            self.on_input = None
        async def close(self):
            lifecycle.append("front_close")

    class LocalTools:
        def __init__(self, *args):
            pass
        def for_agent(self, agent):
            return []
        async def close(self):
            lifecycle.append("tools_close")

    class Pool:
        def __init__(self, spec):
            pass
        def for_class(self, item):
            return []
        async def start(self):
            pass
        async def close(self):
            lifecycle.append("pool_close")

    class Runtime:
        def __init__(self, config):
            configs.append(config)
            previous = (MobKit.builder().mob(config.mob_config_path).gateway(str(binary))
                        .persistent_state(config.persistent_state).roster(config.roster_provider)
                        .topology_provider(config.topology_provider).session_service(config.session_builder)
                        .agent_memory(enabled=True, distiller={"enabled": False}, steward={"enabled": False},
                                      operator_scope="off", content_trust={"untrusted_tools": []})
                        .console_config(config.console_config_path).http_listen("127.0.0.1:0")
                        .console_auth_required(True).auth(Front.native_auth).declare_spec_update(7)
                        .gating(str(tmp_path / "config/gating.toml"))
                        .access_control(str(tmp_path / "config/access.toml"))
                        .routing(str(tmp_path / "deployment/routing.toml")))
            assert MobKitRuntime(config)._build_init_params() == MobKitRuntime(previous._config)._build_init_params()
        async def connect(self):
            lifecycle.append("connect")
        async def identity_bootstrap_status(self):
            return SimpleNamespace(complete=True, ready=True, counts=SimpleNamespace(broken=0))
        async def shutdown(self):
            lifecycle.append("shutdown")

    async def prepare_storage(root, factory):
        pass

    monkeypatch.setattr(module, "ConsoleFront", Front)
    monkeypatch.setattr(module, "ConnectorTools", LocalTools)
    monkeypatch.setattr(module, "MCPPool", Pool)
    Runtime._rpc = supported_host_carriers
    monkeypatch.setattr(meerkat_mobkit, "MobKitRuntime", Runtime)
    monkeypatch.setitem(sys.modules, "storage_setup", SimpleNamespace(prepare_storage=prepare_storage))
    args = SimpleNamespace(connections_check=False, activate_revision=7, definition_revision=False,
                           list_flows=False, run_flow=None, smoke=False)
    await module.run(spec, args)
    assert len(configs) == 1 and configs[0].host_runnables == []
    assert lifecycle == ["connect", "front_close", "shutdown", "pool_close", "tools_close"]


DYNAMIC_APPLICATION = '''import copy
import json
class Integration:
    def __init__(self, context):
        self.context = context
        registry = context.root / "data/desired.json"
        self.rows = (json.loads(registry.read_text()) if registry.exists() else
                     [{"identity": agent["identity"], "classId": agent["classId"]}
                      for agent in context.deployment["agents"]])
        self.edges = [{"a": a, "b": b} for a, b in context.deployment["connections"]]
        self.callback_name = "record_state"
        self.schema = {"type": "object", "properties": {"version": {"const": 1}}}
        self.started = False
    def desired_roster(self):
        return self.rows
    def desired_edges(self, target_identities):
        self.targets = list(target_identities)
        return self.edges
    def tools_for_agent(self, agent):
        owned = copy.deepcopy(agent)
        async def read(arguments):
            return {"identity": owned["identity"], "context": owned.get("context"),
                    "classId": owned["classId"], "tools": owned["tools"], "arguments": arguments}
        # Mutating the supplied copy must not change host authority.
        agent["tools"]["workspace"] = True
        return [{"name": self.callback_name, "description": "Read own synthetic record",
                 "schema": copy.deepcopy(self.schema), "handler": read}]
    def start(self):
        self.started = True
    def stop(self):
        pass
'''


async def dynamic_host(tmp_path, monkeypatch, *, load=True):
    module = load_host(monkeypatch, tmp_path)
    spec = json.loads(generate_files(starter_blueprint())["deployment.json"])
    write_application(tmp_path, DYNAMIC_APPLICATION)
    application = module.Application(module.ApplicationContext(tmp_path, spec, asyncio.Event()))
    if load:
        await application.load()
    return module, application, spec


async def test_dynamic_roster_derives_class_authority_and_copies_complete_snapshot(tmp_path, monkeypatch):
    _, application, spec = await dynamic_host(tmp_path, monkeypatch)
    original = copy.deepcopy(spec)
    application.integration.rows.append({"identity": "lead:synthetic-one", "classId": "lead",
        "display_name": "Synthetic one", "labels": {"record": "one"},
        "context": {"record": "one", "role": "data only", "tools": {"workspace": True}}})
    application.integration.edges = [{"a": "lead:main", "b": "lead:synthetic-one"}]
    await application._prepare_snapshot()
    agents, roster, edges = application._snapshot[:3]
    created = next(row for row in roster if row.identity == "lead:synthetic-one")
    lead = next(row for row in spec["classes"] if row["id"] == "lead")
    assert created.profile == spec["applicationProfiles"]["lead"]
    assert created.profile != agents["lead:main"]["profile"]
    assert created.runtime_mode_override == "turn_driven" and created.addressability == "addressable"
    assert created.labels == {"record": "one", "class": "lead", "role": lead["description"], "console_group": lead["name"]}
    assert agents[created.identity]["tools"] == lead["tools"]
    assert agents[created.identity]["tools"]["workspace"] is False
    assert created.context["role"] == "data only"
    assert edges[0].to_dict() == {"a": "lead:main", "b": "lead:synthetic-one"}
    assert application.integration.targets == list(agents)
    application.context.deployment["classes"][0]["tools"]["workspace"] = True
    application.integration.rows[-1]["context"]["record"] = "modified"
    returned = await application.roster(None)
    returned[-1].context["record"] = "caller mutation"
    assert created.context["record"] == "one" and application.deployment == original and spec == original
    assert not application.integration.started


@pytest.mark.parametrize("case", ["no-map", "half-hooks", "missing-seed", "seed-class", "duplicate",
    "unknown-class", "profile", "model", "placement", "runtime_mode_override", "grants", "reserved-label",
    "lineage-label", "nonfinite", "unknown-edge", "self-edge", "duplicate-edge"])
async def test_invalid_desired_roster_never_publishes_or_reconciles(tmp_path, monkeypatch, case):
    _, application, _ = await dynamic_host(tmp_path, monkeypatch)
    snapshot = application._snapshot
    if case == "no-map":
        application.deployment.pop("applicationProfiles")
    elif case == "half-hooks":
        path = tmp_path / "application/adapters.py"
        path.write_text(path.read_text().replace("    def desired_edges(self, target_identities):\n        self.targets = list(target_identities)\n        return self.edges\n", ""))
        with pytest.raises(ValueError, match="together"):
            await application.load()
        assert application._snapshot is snapshot
        return
    elif case == "missing-seed":
        application.integration.rows.pop()
    elif case == "seed-class":
        application.integration.rows[0]["classId"] = "review"
    elif case == "duplicate":
        application.integration.rows.append(copy.deepcopy(application.integration.rows[0]))
    elif case == "unknown-class":
        application.integration.rows[0]["classId"] = "missing"
    elif case in {"profile", "model", "placement", "runtime_mode_override", "grants"}:
        application.integration.rows[0][case] = "authored override"
    elif case in {"reserved-label", "lineage-label"}:
        application.integration.rows[0]["labels"] = {"role" if case == "reserved-label" else "spawned_by": "operator"}
    elif case == "nonfinite":
        application.integration.rows[0]["context"] = {"number": float("nan")}
    elif case == "unknown-edge":
        application.integration.edges.append({"a": "lead:main", "b": "missing:one"})
    elif case == "self-edge":
        application.integration.edges.append({"a": "lead:main", "b": "lead:main"})
    else:
        application.integration.edges += [{"a": "lead:main", "b": "research:1"}, {"a": "research:1", "b": "lead:main"}]
    calls = []
    async def reconcile():
        calls.append("native reconcile")
    application.context.runtime = SimpleNamespace(is_running=True, reconcile=reconcile)
    with pytest.raises(ValueError):
        await application.context.reconcile_agents()
    assert application._snapshot is snapshot and calls == []


async def test_dynamic_identity_class_binding_survives_removal_within_host(tmp_path, monkeypatch):
    _, application, _ = await dynamic_host(tmp_path, monkeypatch)
    application.integration.rows.append({"identity": "lead:synthetic-one", "classId": "lead"})
    await application._prepare_snapshot()
    application.integration.rows.pop()
    await application._prepare_snapshot()
    snapshot = application._snapshot
    application.integration.rows.append({"identity": "lead:synthetic-one", "classId": "review"})
    with pytest.raises(ValueError, match="including after removal"):
        await application._prepare_snapshot()
    assert application._snapshot is snapshot


@pytest.mark.parametrize("changed", [True, "1", 2])
async def test_dynamic_callback_catalog_rejects_type_sensitive_schema_changes(tmp_path, monkeypatch, changed):
    _, application, _ = await dynamic_host(tmp_path, monkeypatch)
    snapshot = application._snapshot
    application.integration.schema["properties"]["version"]["const"] = changed
    with pytest.raises(ValueError, match="schemas must agree"):
        await application._prepare_snapshot()
    assert application._snapshot is snapshot
    application.integration.schema["properties"]["version"]["const"] = 1
    application.integration.callback_name = "new_callback"
    with pytest.raises(ValueError, match="rebuild"):
        await application._prepare_snapshot()
    assert application._snapshot is snapshot


async def test_actual_sdk_provider_customizer_scopes_same_class_on_create_and_resume(tmp_path, monkeypatch):
    from meerkat_mobkit.agent_builder import CallbackDispatcher
    _, application, spec = await dynamic_host(tmp_path, monkeypatch)
    for key in ("one", "two"):
        application.integration.rows.append({"identity": f"lead:{key}", "classId": "lead", "context": {"record": key}})
    await application._prepare_snapshot()
    dispatcher = CallbackDispatcher()
    dispatcher.register_roster_provider(application)
    dispatcher.register_topology_provider(application)
    dispatcher.register_agent_customizer(application)
    roster = await dispatcher.handle_callback("callback/roster_provider/roster", {"context": {"previous_identities": []}})
    assert len(roster) == len(spec["agents"]) + 2
    assert all(row["profile"] == spec["applicationProfiles"]["lead"] for row in roster if row["identity"] in {"lead:one", "lead:two"})
    projected = await dispatcher.handle_callback("callback/topology_provider/compute_edges", {"target_identities": ["lead:main"], "context": {}})
    assert projected == []
    async def build(identity, scope):
        row = next(value for value in roster if value["identity"] == identity)
        draft = await dispatcher.handle_callback("callback/agent_customizer/customize_build", {
            "scope_id": scope, "context": {"identity": identity}, "spec": row,
            "draft": {"labels": {"agent_identity": identity}}})
        assert "model" not in draft and "system_prompt" not in draft
        assert draft["labels"] == {"agent_identity": identity}
        assert [tool["name"] for tool in draft["external_tools"]] == ["record_state"]
        return await dispatcher.handle_callback("callback/call_tool", {"scope_id": scope, "tool": "record_state", "arguments": {}})
    one = await build("lead:one", "fresh-one")
    two = await build("lead:two", "fresh-two")
    assert one["content"]["identity"] == "lead:one" and one["content"]["context"] == {"record": "one"}
    assert two["content"]["identity"] == "lead:two" and two["content"]["context"] == {"record": "two"}
    assert one["content"]["tools"] == spec["classes"][0]["tools"]
    assert (await build("lead:one", "restored-one")) == one
    with pytest.raises(ValueError, match="no handler"):
        await dispatcher.handle_callback("callback/call_tool", {"scope_id": "fresh-one", "tool": "record_state", "arguments": {}})
    assert await dispatcher.handle_callback("callback/call_tool", {"scope_id": "fresh-two", "tool": "record_state", "arguments": {}}) == two


async def test_admitting_identity_is_buildable_but_not_projected(tmp_path, monkeypatch):
    from meerkat_mobkit.agent_builder import CallbackDispatcher
    _, application, spec = await dynamic_host(tmp_path, monkeypatch)
    pending = {"identity": "lead:pending", "classId": "lead", "context": {"record": "private-pending"}}
    application.integration.admission_roster = lambda: {"listed": application.integration.rows, "admitting": [pending]}
    await application._prepare_snapshot()
    dispatcher = CallbackDispatcher()
    dispatcher.register_agent_customizer(application)
    assert pending["identity"] not in {row.identity for row in await application.roster(None)}
    assert pending["identity"] not in application.integration.targets
    buildable = next(row for row in application._snapshot.buildable_specs if row.identity == pending["identity"])
    assert buildable.profile == spec["applicationProfiles"]["lead"]
    assert buildable.profile != application._snapshot[0]["lead:main"]["profile"]
    await dispatcher.handle_callback("callback/agent_customizer/customize_build", {
        "scope_id": "held-pending", "context": {"identity": pending["identity"]}, "spec": buildable.to_dict(),
        "draft": {"labels": {"agent_identity": pending["identity"]}}})
    result = await dispatcher.handle_callback("callback/call_tool", {
        "scope_id": "held-pending", "tool": "record_state", "arguments": {}})
    assert result["content"]["identity"] == pending["identity"]
    assert result["content"]["context"] == {"record": "private-pending"}
    assert result["content"]["tools"] == spec["classes"][0]["tools"]
    pending["context"]["record"] = "mutated"
    assert buildable.context == {"record": "private-pending"}
    assert await application.compute_edges(["lead:main", "lead:pending"], None) == []


async def test_admission_snapshot_uses_one_read_for_both_partitions(tmp_path, monkeypatch):
    _, application, _ = await dynamic_host(tmp_path, monkeypatch)
    reads = []
    def legacy_read():
        raise AssertionError("Do not mix a separate listed read with the admission snapshot.")
    async def snapshot():
        reads.append("one app transaction")
        await asyncio.sleep(0)
        return {"listed": application.integration.rows,
                "admitting": [{"identity": "lead:pending", "classId": "lead"}]}
    application.integration.desired_roster = legacy_read
    application.integration.admission_roster = snapshot
    await application._prepare_snapshot()
    assert reads == ["one app transaction"]
    assert "lead:pending" not in {row.identity for row in await application.roster(None)}
    assert "lead:pending" in application._snapshot.buildable_agents


async def test_projection_refresh_completes_while_native_reconcile_is_parked(tmp_path, monkeypatch):
    _, application, _ = await dynamic_host(tmp_path, monkeypatch)
    pending = [{"identity": "lead:pending", "classId": "lead"}]
    application.integration.admission_roster = lambda: {"listed": application.integration.rows, "admitting": pending}
    entered, release = asyncio.Event(), asyncio.Event()
    async def reconcile():
        entered.set()
        await release.wait()
        return {"source": "test runtime"}
    runtime = SimpleNamespace(is_running=True, reconcile=reconcile)
    application.context.runtime = runtime
    task = asyncio.create_task(application.reconcile_agents())
    try:
        await asyncio.wait_for(entered.wait(), 1)
        assert application._reconcile_lock.locked()
        assert "lead:pending" not in {row.identity for row in await asyncio.wait_for(application.roster(None), 1)}
        application.integration.rows.append(pending.pop())
        await asyncio.wait_for(application._prepare_snapshot(runtime=runtime), 1)
        assert application._reconcile_lock.locked()
        assert "lead:pending" in {row.identity for row in await asyncio.wait_for(application.roster(None), 1)}
    finally:
        release.set()
        await asyncio.wait_for(task, 1)


@pytest.mark.parametrize("case", ["seed", "duplicate", "profile", "unknown-class", "edge", "not-list", "schema"])
async def test_admitting_rows_cannot_weaken_roster_or_tool_validation(tmp_path, monkeypatch, case):
    _, application, _ = await dynamic_host(tmp_path, monkeypatch)
    pending = {"identity": "lead:pending", "classId": "lead"}
    rows = [pending]
    if case == "seed":
        rows = [copy.deepcopy(application.integration.rows[0])]
        application.integration.rows = application.integration.rows[1:]
    elif case == "duplicate":
        application.integration.rows.append(copy.deepcopy(pending))
    elif case == "profile":
        pending["profile"] = "lead__main"
    elif case == "unknown-class":
        pending["classId"] = "uncompiled"
    elif case == "edge":
        application.integration.edges = [{"a": "lead:main", "b": "lead:pending"}]
    elif case == "not-list":
        rows = {"pending": pending}
    elif case == "schema":
        hook = application.integration.tools_for_agent
        def tools(agent):
            result = hook(agent)
            if agent["identity"] == "lead:pending":
                result[0]["schema"] = {"type": "string"}
            return result
        application.integration.tools_for_agent = tools
    application.integration.admission_roster = lambda: {"listed": application.integration.rows, "admitting": rows}
    snapshot = application._snapshot
    with pytest.raises(ValueError):
        await application._prepare_snapshot()
    assert application._snapshot is snapshot
    assert "lead:pending" not in application._identity_classes


async def test_customizer_refuses_wrong_adopted_spec_and_stale_hook_completion(tmp_path, monkeypatch):
    from meerkat_mobkit.identity_first_models import AgentBuildContext, AgentBuildDraft
    _, application, _ = await dynamic_host(tmp_path, monkeypatch)
    original = copy.deepcopy(application._snapshot[1][0])
    wrong = copy.deepcopy(original)
    wrong.profile = "unknown"
    with pytest.raises(ValueError, match="validated desired roster"):
        await application.customize_build(AgentBuildContext(original.identity, [], []), wrong, AgentBuildDraft())
    entered, release = asyncio.Event(), asyncio.Event()
    hook = application.integration.tools_for_agent
    async def paused(agent):
        entered.set()
        await release.wait()
        return hook(agent)
    application.integration.tools_for_agent = paused
    draft = AgentBuildDraft()
    task = asyncio.create_task(application.customize_build(AgentBuildContext(original.identity, [], []), original, draft))
    await asyncio.wait_for(entered.wait(), 30)
    application._snapshot = copy.deepcopy(application._snapshot)
    release.set()
    with pytest.raises(ValueError, match="changed during"):
        await task
    assert draft.external_tools == []


@pytest.mark.parametrize("new_context", [True, 1.0, "1"])
async def test_actual_customizer_refuses_equal_python_values_from_an_older_native_spec(tmp_path, monkeypatch, new_context):
    from meerkat_mobkit.agent_builder import CallbackDispatcher
    _, application, _ = await dynamic_host(tmp_path, monkeypatch)
    application.integration.rows[0]["context"] = {"record": 1}
    await application._prepare_snapshot()
    adopted = application._snapshot[1][0].to_dict()
    application.integration.rows[0]["context"] = {"record": new_context}
    await application._prepare_snapshot()
    dispatcher = CallbackDispatcher()
    dispatcher.register_agent_customizer(application)
    with pytest.raises(ValueError, match="validated desired roster"):
        await dispatcher.handle_callback("callback/agent_customizer/customize_build", {
            "scope_id": "stale-adoption", "context": {"identity": adopted["identity"]},
            "spec": adopted, "draft": {}})
    with pytest.raises(ValueError, match="no handler"):
        await dispatcher.handle_callback("callback/call_tool", {
            "scope_id": "stale-adoption", "tool": "record_state", "arguments": {}})


@pytest.mark.parametrize("failure", [RuntimeError, asyncio.CancelledError])
async def test_reconcile_uses_native_provider_snapshot_and_preserves_uncertain_desire(tmp_path, monkeypatch, failure):
    _, application, _ = await dynamic_host(tmp_path, monkeypatch)
    with pytest.raises(RuntimeError, match="offline trial lifecycle"):
        await application.context.reconcile_agents()
    calls = []
    async def reconcile():
        # Native calls back into providers during reconcile. They must not
        # acquire the host validation lock a second time.
        calls.append([row.identity for row in await application.roster(None)])
        await application.compute_edges(calls[-1], {})
        raise failure("native uncertain")
    application.context.runtime = SimpleNamespace(is_running=True, reconcile=reconcile)
    application.integration.rows.append({"identity": "lead:later", "classId": "lead"})
    with pytest.raises(failure):
        await asyncio.wait_for(application.context.reconcile_agents(), 1)
    assert "lead:later" in calls[0] and "lead:later" in application._snapshot[0]


async def test_reconciliation_serializes_and_runtime_replacement_does_not_call_new_runtime(tmp_path, monkeypatch):
    _, application, _ = await dynamic_host(tmp_path, monkeypatch)
    entered, release = asyncio.Event(), asyncio.Event()
    calls = []
    async def reconcile():
        calls.append("old")
        entered.set()
        await release.wait()
        return {"pending": True}
    application.context.runtime = SimpleNamespace(is_running=True, reconcile=reconcile)
    first = asyncio.create_task(application.context.reconcile_agents())
    await asyncio.wait_for(entered.wait(), 30)
    second = asyncio.create_task(application.context.reconcile_agents())
    await asyncio.sleep(0)
    assert calls == ["old"]
    application.context.runtime = SimpleNamespace(is_running=True, reconcile=lambda: calls.append("new"))
    release.set()
    assert await first == {"pending": True}
    with pytest.raises(RuntimeError, match="changed before"):
        await second
    assert calls == ["old"]


async def test_runtime_changed_during_validation_never_publishes_unadmitted_snapshot(tmp_path, monkeypatch):
    _, application, _ = await dynamic_host(tmp_path, monkeypatch)
    snapshot = application._snapshot
    entered, release = asyncio.Event(), asyncio.Event()
    hook = application.integration.tools_for_agent
    async def paused(agent):
        entered.set()
        await release.wait()
        return hook(agent)
    application.integration.tools_for_agent = paused
    calls = []
    async def reconcile():
        calls.append("native")
    application.context.runtime = SimpleNamespace(is_running=True, reconcile=reconcile)
    application.integration.rows.append({"identity": "lead:unadmitted", "classId": "lead"})
    task = asyncio.create_task(application.context.reconcile_agents())
    await asyncio.wait_for(entered.wait(), 30)
    application.context.runtime = SimpleNamespace(is_running=True, reconcile=reconcile)
    release.set()
    with pytest.raises(RuntimeError, match="changed during"):
        await task
    assert application._snapshot is snapshot and "lead:unadmitted" not in application._identity_classes
    assert calls == []


@pytest.mark.parametrize("select_host_tools", [False, True])
async def test_dynamic_startup_wires_exact_native_customizer_and_identity_grants(tmp_path, monkeypatch, select_host_tools):
    import meerkat_mobkit
    from meerkat_mobkit import MobKitRuntime
    from meerkat_mobkit.agent_builder import CallbackDispatcher
    module = load_host(monkeypatch, tmp_path)
    files = generate_files(starter_blueprint())
    spec = json.loads(files["deployment.json"])
    (tmp_path / "config").mkdir()
    (tmp_path / "config/mob.toml").write_text(files["config/mob.toml"])
    (tmp_path / "data").mkdir()
    rows = [{"identity": agent["identity"], "classId": agent["classId"]} for agent in spec["agents"]]
    rows += [{"identity": "lead:extra", "classId": "lead", "context": {"record": "extra"}}]
    (tmp_path / "data/desired.json").write_text(json.dumps(rows))
    source = DYNAMIC_APPLICATION.replace("self.started = True", "self.started = True\n        self.context.stop_event.set()")
    if select_host_tools:
        source = source.replace("    def tools_for_agent(self, agent):", '''    def select_host_tools(self, agent, available_names):
        if not (self.context.root / "data/catalog-ready").exists():
            raise RuntimeError("Host selection ran before connector startup")
        self.context.selected_identities = getattr(self.context, "selected_identities", []) + [agent["identity"]]
        return []

    def tools_for_agent(self, agent):''')
    write_application(tmp_path, source)
    binary = tmp_path / "gateway"
    binary.touch()
    monkeypatch.setattr(module, "gateway_path", lambda: binary)
    monkeypatch.setenv("OPENAI_API_KEY", "isolated-test-value")
    events, observed = [], {}
    class LocalTools:
        def __init__(self, *args):
            pass
        def for_agent(self, agent):
            if "files" not in agent["connectors"]:
                return []
            return [{"name": "file_read", "description": "Declared workspace callback", "schema": {"type": "object"}, "handler": lambda args: "synthetic"}]
        async def close(self):
            events.append("tools_close")
    class Pool:
        def __init__(self, spec):
            pass
        def for_class(self, item):
            return []
        async def start(self):
            (tmp_path / "data/catalog-ready").write_text("synthetic ready")
        async def close(self):
            events.append("pool_close")
    class Runtime:
        rust_http_base_url = "http://127.0.0.1:0"
        is_running = True
        def __init__(self, config):
            self.config = config
            wire = MobKitRuntime(config)._build_init_params()
            assert wire["has_roster_provider"] is True and wire["has_topology_provider"] is True and wire["has_agent_customizer"] is True
            assert wire["runtime_options"]["agent_memory"] == {
                "enabled": True, "distiller": {"enabled": False}, "steward": {"enabled": False},
                "operator_scope": "off", "content_trust": {"untrusted_tools": ["record_state"]}}
            assert config.agent_customizer is config.roster_provider is config.topology_provider
        async def connect(self):
            dispatcher = CallbackDispatcher()
            dispatcher.register_agent_customizer(self.config.agent_customizer)
            dispatcher.register_builder(self.config.session_builder)
            for index, spec_row in enumerate(await self.config.roster_provider.roster(None)):
                scope = f"native-test-{index}"
                preliminary = await dispatcher.handle_callback("callback/build_agent", {"options": {
                    "scope_id": scope, "profile_name": spec_row.profile, "resume_session_id": "existing"}})
                names = [tool["name"] for tool in preliminary.get("tools", [])]
                seed = next((agent for agent in spec["agents"] if agent["profile"] == spec_row.profile), None)
                expected = (["file_read", "record_state"] if seed and seed["tools"]["workspace"] and not select_host_tools
                            else ["record_state"] if seed else [])
                assert names == expected
                if seed:
                    initial = await dispatcher.handle_callback("callback/call_tool", {
                        "scope_id": scope, "tool": "record_state", "arguments": {}})
                    assert initial["content"]["identity"] == seed["identity"]
                rendered = await dispatcher.handle_callback("callback/agent_customizer/customize_build", {
                    "scope_id": scope, "context": {"identity": spec_row.identity}, "spec": spec_row.to_dict(), "draft": {}})
                observed[spec_row.identity] = [tool["name"] for tool in rendered["external_tools"]]
                result = await dispatcher.handle_callback("callback/call_tool", {"scope_id": scope, "tool": "record_state", "arguments": {}})
                assert result["content"]["identity"] == spec_row.identity
                if spec_row.identity == "lead:extra":
                    assert result["content"]["context"] == {"record": "extra"}
            with pytest.raises(ValueError, match="unknown profile"):
                await dispatcher.handle_callback("callback/build_agent", {"options": {"scope_id": "invalid", "profile_name": "invented"}})
        async def identity_bootstrap_status(self):
            return SimpleNamespace(complete=True, ready=True, counts=SimpleNamespace(broken=0))
        async def shutdown(self):
            events.append("native_shutdown")
    async def prepare_storage(root, factory):
        pass
    monkeypatch.setattr(module, "ConnectorTools", LocalTools)
    monkeypatch.setattr(module, "MCPPool", Pool)
    Runtime._rpc = supported_host_carriers
    monkeypatch.setattr(meerkat_mobkit, "MobKitRuntime", Runtime)
    monkeypatch.setitem(sys.modules, "storage_setup", SimpleNamespace(prepare_storage=prepare_storage))
    args = SimpleNamespace(connections_check=False, activate_revision=None, definition_revision=False,
                           list_flows=False, run_flow=None, smoke=False)
    await module.run(spec, args)
    assert observed["lead:extra"] == observed["lead:main"] == observed["review:main"] == ["record_state"]
    assert observed["research:1"] == observed["research:2"] == (["record_state"] if select_host_tools else ["file_read", "record_state"])
    assert events == ["native_shutdown", "pool_close", "tools_close"]


async def test_dynamic_restart_validation_reads_application_registry_without_native_claim(tmp_path, monkeypatch, capsys):
    module, application, spec = await dynamic_host(tmp_path, monkeypatch)
    rows = application.integration.rows + [{"identity": "lead:persisted", "classId": "lead", "context": {"record": "durable"}}]
    (tmp_path / "data").mkdir()
    (tmp_path / "data/desired.json").write_text(json.dumps(rows))
    before = (tmp_path / "data/desired.json").read_bytes()
    restarted = module.Application(module.ApplicationContext(tmp_path, spec, asyncio.Event()))
    await restarted.load()
    assert set(restarted._snapshot[0]) == {row["identity"] for row in rows}
    assert restarted._snapshot[0]["lead:persisted"]["context"] == {"record": "durable"}
    await module.application_check(spec)
    output = capsys.readouterr().out
    assert "5 desired identities" in output and "Native materialization and user authorization were not exercised" in output
    assert not restarted.integration.started and restarted.context.runtime is None
    assert (tmp_path / "data/desired.json").read_bytes() == before
