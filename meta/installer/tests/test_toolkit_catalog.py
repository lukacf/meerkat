"""Host-pinned catalog discovery with fake MCP/SDK owners, no subprocesses."""
import asyncio
import copy
import hashlib
import json
import os
from contextlib import asynccontextmanager
from pathlib import Path
from types import SimpleNamespace

import pytest

from launchkit import toolkit_catalog as module
from launchkit.toolkit_catalog import ToolkitCatalog, ToolkitCatalogUnavailable
from launchkit.templates_v2.mcp_connections import MCPConnection
from launchkit.architect import ArchitectUnavailable
from test_architect_startup_recovery import FakeRuntime, startup


IDENTITY = {"publisher": "example", "name": "calendar-reader", "version": "1.0.0"}
OTHER = {"publisher": "example", "name": "public-search", "version": "2.0.0"}


@pytest.fixture
def host(tmp_path, monkeypatch):
    bundle = tmp_path / "bundle"
    bundle.mkdir()
    entries = [{"identity": identity, "display_name": identity["name"], "description": "Public catalog data"}
               for identity in (IDENTITY, OTHER)]
    (bundle / "catalog.json").write_text(json.dumps({"entries": entries}))
    (bundle / "CONTENTS.json").write_text(json.dumps({"files": {"catalog.json": {"sha256":
        hashlib.sha256((bundle / "catalog.json").read_bytes()).hexdigest()}}}))
    python = tmp_path / "isolated/runtime/bin/python"
    python.parent.mkdir(parents=True)
    python.write_text("never executed by these tests")
    python.chmod(0o700)
    # Venv interpreter symlinks must retain their lexical path at launch.
    lexical = python.parent / "python3.11"
    lexical.symlink_to(python)
    adapter = tmp_path / "kit/setup/mcp_adapter.py"
    adapter.parent.mkdir(parents=True)
    adapter.write_text("never executed by these tests")
    parent = tmp_path / "sessions"
    parent.mkdir(mode=0o700)
    binding = {"command": str(lexical), "args": ["-I", "-B", str(adapter), "--families", "services",
        "--python", str(lexical), "--bundle", str(bundle), "--contents-sha256",
        hashlib.sha256((bundle / "CONTENTS.json").read_bytes()).hexdigest()],
        "workspace_parent": str(parent), "contents_sha256": hashlib.sha256((bundle / "CONTENTS.json").read_bytes()).hexdigest(),
        "catalog_sha256": hashlib.sha256((bundle / "catalog.json").read_bytes()).hexdigest(), "catalog_count": 2}
    path = tmp_path / "host-selected.json"
    path.write_text(json.dumps(binding))
    path.chmod(0o600)
    monkeypatch.setenv(module.CONFIG_ENV, str(path))
    value = {"scope": "catalog_metadata_only", **{key: binding[key] for key in ("contents_sha256", "catalog_sha256")},
             "entries": entries}
    instances = []

    class FakeConnection:
        def __init__(self, config):
            self.config = config
            self.tools = dict.fromkeys([*module.TOOLS, "setup_install", "setup_prepare"])
            self.calls = []
            self.closed = 0
            self.start_error = None
            self.reply_error = None
            self.start_entered = asyncio.Event()
            self.start_release = None
            self.catalog = copy.deepcopy(value)
            self.detail = {key: copy.deepcopy(value[key]) for key in ("scope", "contents_sha256", "catalog_sha256")}
            self.detail["entry"] = {"identity": copy.deepcopy(IDENTITY), "strategies": [
                {"kind": "mcp", "assessment": "requires separate setup"},
                {"kind": "library", "assessment": "unresolved"}], "provenance": {"source": "fixture"}}
            instances.append(self)

        async def start(self):
            self.start_entered.set()
            if self.start_release is not None:
                await self.start_release.wait()
            if self.start_error:
                raise self.start_error

        async def call(self, name, args):
            self.calls.append((name, copy.deepcopy(args)))
            if self.reply_error:
                raise self.reply_error
            return {"structuredContent": copy.deepcopy(self.catalog if name == "setup_catalog" else self.detail)}

        async def close(self):
            self.closed += 1

    monkeypatch.setattr(module, "MCPConnection", FakeConnection)
    return SimpleNamespace(binding=binding, path=path, bundle=bundle, value=value,
                           instances=instances, factory=FakeConnection)


@pytest.mark.asyncio
async def test_disabled_default_does_not_construct_or_launch(monkeypatch):
    monkeypatch.delenv(module.CONFIG_ENV, raising=False)
    monkeypatch.setattr(module, "MCPConnection", lambda value: pytest.fail("No opt-in may launch a connection"))
    catalog = ToolkitCatalog()
    await catalog.start()
    assert catalog.status()["status"] == "disabled" and not catalog.status()["available"]
    with pytest.raises(ToolkitCatalogUnavailable):
        await catalog.catalog({})
    await catalog.shutdown()


@pytest.mark.asyncio
async def test_fixed_lexical_command_fresh_workspaces_and_isolated_environment(host, monkeypatch):
    monkeypatch.setenv("HOME", "/unrelated-private-home")
    monkeypatch.setenv("TMPDIR", "/unrelated-private-tmp")
    catalog = ToolkitCatalog()
    await catalog.start()
    first = host.instances[0].config
    assert first["command"] == host.binding["command"]
    assert first["args"][:-2] == host.binding["args"] and first["args"][-2] == "--workspace"
    workspace = Path(first["args"][-1])
    assert workspace.parent.parent == Path(host.binding["workspace_parent"]) and not workspace.exists()
    assert first["env"] == {"PATH": "/usr/bin:/bin", "LANG": "C.UTF-8",
        "HOME": str(workspace.parent / "home"), "TMPDIR": str(workspace.parent / "tmp"),
        "TEMP": str(workspace.parent / "tmp"), "PYTHONDONTWRITEBYTECODE": "1"}
    assert Path(first["env"]["HOME"]).stat().st_mode & 0o077 == 0
    assert "unrelated-private" not in json.dumps(first)
    await catalog.shutdown()
    assert host.instances[0].closed == 1
    await catalog.start()
    second = host.instances[1].config
    assert first["args"][-1] != second["args"][-1] and workspace.parent.exists()
    await catalog.shutdown()


@pytest.mark.asyncio
@pytest.mark.parametrize("change", ["unsafe-mode", "extra-env", "relative-command", "wrong-python", "workspace-argument",
                                 "substitution", "wrong-pin", "wrong-catalog", "wrong-count"])
async def test_invalid_host_selection_never_constructs_connection(host, change):
    binding = copy.deepcopy(host.binding)
    if change == "unsafe-mode":
        host.path.chmod(0o666)
    elif change == "extra-env":
        binding["env"] = {"SECRET": "PRIVATE_CANARY"}
    elif change == "relative-command":
        binding["command"] = "python"
    elif change == "wrong-python":
        binding["args"][6] = "/some/other/python"
    elif change == "workspace-argument":
        binding["args"] += ["--workspace", "/fixed/reused/path"]
    elif change == "substitution":
        binding["args"][2] = "${HOME}/mcp_adapter.py"
    elif change == "wrong-pin":
        binding["contents_sha256"] = "0" * 64
    elif change == "wrong-catalog":
        (host.bundle / "catalog.json").write_text("changed catalog")
    else:
        binding["catalog_count"] = True
    host.path.write_text(json.dumps(binding))
    catalog = ToolkitCatalog()
    await catalog.start()
    assert not host.instances and catalog.status()["status"] == "unavailable"
    assert "PRIVATE_CANARY" not in json.dumps(catalog.status()) and str(host.path) not in json.dumps(catalog.status())


@pytest.mark.asyncio
async def test_exact_catalog_identity_dispatch_and_all_strategy_preservation(host):
    catalog = ToolkitCatalog()
    await catalog.start()
    result = await catalog.entry(copy.deepcopy(IDENTITY))
    assert result["entry"]["strategies"] == host.instances[0].detail["entry"]["strategies"]
    assert result["catalog_sha256"] == host.binding["catalog_sha256"]
    before = copy.deepcopy(host.instances[0].calls)
    for name, args in [("setup_install", {}), ("setup_prepare", {}),
                       ("setup_catalog", {"command": "/invented"}),
                       ("setup_catalog_entry", {**IDENTITY, "version": "invented"}),
                       ("setup_catalog_entry", {**IDENTITY, "args": []})]:
        with pytest.raises(ValueError):
            await catalog.call(name, args)
    assert host.instances[0].calls == before and catalog.status()["available"]
    await catalog.shutdown()


@pytest.mark.asyncio
@pytest.mark.parametrize("failure", ["scope", "contents", "catalog", "count", "duplicate", "missing-tool", "startup"])
async def test_failed_start_closes_owned_connection_and_sanitizes_status(host, monkeypatch, failure):
    def connection(config):
        result = host.factory(config)
        if failure == "scope": result.catalog["scope"] = "installed_ready"
        elif failure == "contents": result.catalog["contents_sha256"] = "0" * 64
        elif failure == "catalog": result.catalog["catalog_sha256"] = "0" * 64
        elif failure == "count": result.catalog["entries"].pop()
        elif failure == "duplicate": result.catalog["entries"][1] = result.catalog["entries"][0]
        elif failure == "missing-tool": result.tools.pop("setup_catalog_entry")
        else: result.start_error = RuntimeError("PRIVATE_CREDENTIAL_CANARY /private/executable")
        return result
    monkeypatch.setattr(module, "MCPConnection", connection)
    catalog = ToolkitCatalog()
    await catalog.start()
    assert catalog.status()["status"] == "unavailable" and host.instances[0].closed == 1
    assert catalog.connection is None and not catalog.identities
    assert "PRIVATE_CREDENTIAL_CANARY" not in json.dumps(catalog.status())
    await catalog.shutdown()
    assert host.instances[0].closed == 1


@pytest.mark.asyncio
@pytest.mark.parametrize("failure", ["foreign-identity", "wrong-pin", "raw-error"])
async def test_detail_failure_retires_connection_without_raw_error_projection(host, failure):
    catalog = ToolkitCatalog()
    await catalog.start()
    connection = host.instances[0]
    if failure == "foreign-identity": connection.detail["entry"]["identity"] = OTHER
    elif failure == "wrong-pin": connection.detail["catalog_sha256"] = "0" * 64
    else: connection.reply_error = RuntimeError("PRIVATE_CREDENTIAL_CANARY")
    with pytest.raises(ToolkitCatalogUnavailable) as error:
        await catalog.entry(IDENTITY)
    assert "PRIVATE_CREDENTIAL_CANARY" not in str(error.value)
    assert catalog.status()["status"] == "unavailable" and connection.closed == 1
    before = copy.deepcopy(connection.calls)
    with pytest.raises(ToolkitCatalogUnavailable): await catalog.catalog({})
    assert connection.calls == before


@pytest.mark.asyncio
async def test_start_cancellation_closes_the_already_retained_connection(host, monkeypatch):
    release = asyncio.Event()
    def connection(config):
        result = host.factory(config)
        result.start_release = release
        return result
    monkeypatch.setattr(module, "MCPConnection", connection)
    catalog = ToolkitCatalog()
    task = asyncio.create_task(catalog.start())
    while not host.instances: await asyncio.sleep(0)
    await host.instances[0].start_entered.wait()
    assert catalog.connection is host.instances[0]
    task.cancel()
    with pytest.raises(asyncio.CancelledError): await task
    assert host.instances[0].closed == 1 and catalog.connection is None


@pytest.mark.asyncio
async def test_actual_mcp_client_keeps_transport_contexts_on_owned_task(host, monkeypatch):
    import mcp
    import mcp.client.stdio
    observed, owners = {}, []
    @asynccontextmanager
    async def stdio(params):
        observed["command"], observed["args"], observed["env"] = params.command, params.args, params.env
        owners.append(asyncio.current_task())
        try: yield object(), object()
        finally: owners.append(asyncio.current_task())
    class Session:
        def __init__(self, *args, **kwargs): pass
        async def __aenter__(self):
            owners.append(asyncio.current_task())
            return self
        async def __aexit__(self, *args): owners.append(asyncio.current_task())
        async def initialize(self): pass
        async def list_tools(self, **kwargs):
            return SimpleNamespace(tools=[SimpleNamespace(name=name) for name in [*module.TOOLS, "setup_install"]], nextCursor=None)
        async def call_tool(self, name, args, **kwargs):
            assert name == "setup_catalog" and args == {}
            return SimpleNamespace(isError=False, model_dump=lambda **kwargs: {"structuredContent": host.value})
    monkeypatch.setattr(mcp.client.stdio, "stdio_client", stdio)
    monkeypatch.setattr(mcp, "ClientSession", Session)
    monkeypatch.setattr(module, "MCPConnection", MCPConnection)
    monkeypatch.setenv("SECRET_TOKEN", "PRIVATE_CANARY")
    monkeypatch.setenv("HOME", "/unrelated-private-home")
    catalog = ToolkitCatalog()
    await catalog.start()
    assert catalog.status()["available"]
    await catalog.shutdown()
    assert len(owners) == 4 and len(set(owners)) == 1 and owners[0] is not asyncio.current_task()
    assert observed["command"] == host.binding["command"]
    assert "SECRET_TOKEN" not in observed["env"] and "unrelated-private" not in json.dumps(observed["env"])


@pytest.mark.asyncio
async def test_actual_architect_registration_is_connectors_only_and_shutdown_cleans(host, startup):
    service, controller = startup
    registered = {}
    async def callbacks(builder):
        for profile in ("architect", "connectors", "reviewer", "visualizer"):
            tools = {}
            await builder.session_service_callback.build_agent(SimpleNamespace(profile_name=profile,
                register_tool=lambda name, callback, **kwargs: tools.update({name: (callback, kwargs)})))
            registered[profile] = tools
    controller.callback_runner = callbacks
    controller.outcomes = [FakeRuntime()]
    assert (await service.start())["toolkit_catalog"]["available"]
    for profile, tools in registered.items():
        assert set(tools) & module.TOOLS == (module.TOOLS if profile == "connectors" else set())
    catalog = await registered["connectors"]["setup_catalog"][0]({})
    assert catalog["scope"] == "catalog_metadata_only"
    entry = await registered["connectors"]["setup_catalog_entry"][0](IDENTITY)
    assert entry["entry"]["identity"] == IDENTITY
    assert registered["connectors"]["setup_catalog_entry"][1]["input_schema"] == module.ENTRY_SCHEMA
    assert "search_mcp_registry" in registered["connectors"]
    await service.shutdown()
    assert host.instances[0].closed == 1 and controller.lock_events == ["acquired", "released"]


@pytest.mark.asyncio
async def test_native_start_failure_closes_catalog_and_releases_existing_lock(host, startup):
    service, controller = startup
    controller.outcomes = [RuntimeError("FAKE_NATIVE_START_FAILURE")]
    with pytest.raises(RuntimeError, match="FAKE_NATIVE_START_FAILURE"): await service.start()
    assert host.instances[0].closed == 1 and controller.lock_events == ["acquired", "released"]
    assert service.state == "failed" and service.toolkit_catalog.connection is None


@pytest.mark.asyncio
async def test_catalog_cleanup_error_preserves_both_failures_and_lock_cleanup(startup, monkeypatch):
    service, controller = startup
    runtime = FakeRuntime()
    original_native_shutdown = runtime.shutdown
    async def native_shutdown(): raise RuntimeError("NATIVE_CLEANUP_ERROR")
    runtime.shutdown = native_shutdown
    controller.outcomes = [runtime]
    monkeypatch.delenv(module.CONFIG_ENV, raising=False)
    await service.start()
    original_catalog_shutdown = service.toolkit_catalog.shutdown
    original_lease = service.lock_context
    async def catalog_shutdown(): raise RuntimeError("PRIVATE_CATALOG_CLEANUP_ERROR")
    monkeypatch.setattr(service.toolkit_catalog, "shutdown", catalog_shutdown)
    with pytest.raises(BaseExceptionGroup) as error: await service.shutdown()
    assert [str(item) for item in error.value.exceptions] == ["NATIVE_CLEANUP_ERROR", "PRIVATE_CATALOG_CLEANUP_ERROR"]
    assert service.runtime is runtime and service.lock_context is original_lease
    assert controller.lock_events == ["acquired"] and service.state == "failed"
    assert service.provider_stop_failed
    with pytest.raises(ArchitectUnavailable, match="cleanup is unconfirmed"):
        await service.start(retry_failed=True)
    assert controller.builds == 1
    runtime.shutdown = original_native_shutdown
    monkeypatch.setattr(service.toolkit_catalog, "shutdown", original_catalog_shutdown)
    await service.shutdown()
    assert service.runtime is None and service.lock_context is None
    assert controller.lock_events == ["acquired", "released"] and not service.provider_stop_failed


@pytest.mark.asyncio
async def test_catalog_close_failure_is_unavailable_and_retains_owned_connection(host, startup):
    service, controller = startup
    runtime = FakeRuntime()
    controller.outcomes = [runtime]
    await service.start()
    connection = host.instances[0]
    original_lease = service.lock_context
    async def failed_close(): raise RuntimeError("CATALOG_CLOSE_FAILED")
    original_close = connection.close
    connection.close = failed_close
    with pytest.raises(RuntimeError, match="CATALOG_CLOSE_FAILED"): await service.shutdown()
    assert runtime.shutdown_calls == 1 and service.lock_context is original_lease and service.state == "failed"
    assert service.toolkit_catalog.connection is connection
    assert not service.toolkit_catalog.status()["available"]
    assert controller.lock_events == ["acquired"] and service.provider_stop_failed
    connection.close = original_close
    await service.shutdown()
    assert connection.closed == 1 and service.toolkit_catalog.connection is None
    assert runtime.shutdown_calls == 1 and service.lock_context is None
    assert controller.lock_events == ["acquired", "released"] and not service.provider_stop_failed
