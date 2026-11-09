"""Software setup integration with fictional MCP owners, never subprocesses."""
import asyncio
import copy
import hashlib
import json
from pathlib import Path
from contextlib import asynccontextmanager
from types import SimpleNamespace

import pytest

from launchkit import toolkit_setup as module
from launchkit.toolkit_setup import ToolkitSetup, ToolkitSetupUnavailable


IDENTITY = {"publisher": "fixture", "name": "calendar-read", "version": "1.0.0"}
JOB = "a" * 32


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def object_schema(properties=None, required=None):
    properties = properties or {}
    return {"type": "object", "properties": properties,
            "required": list(properties) if required is None else required,
            "additionalProperties": False}


@pytest.fixture
def host(tmp_path, monkeypatch):
    bundle = tmp_path / "bundle"
    bundle.mkdir()
    entries = [{"identity": IDENTITY, "display_name": "Fixture calendar reader", "description": "Catalog data only"}]
    (bundle / "catalog.json").write_text(json.dumps({"entries": entries}))
    (bundle / "CONTENTS.json").write_text(json.dumps({"files": {"catalog.json": {"sha256": digest(bundle / "catalog.json")}}}))
    interpreter = tmp_path / "runtime/bin/python"
    interpreter.parent.mkdir(parents=True)
    interpreter.write_text("Fictional interpreter - never executed")
    interpreter.chmod(0o700)
    lexical = interpreter.parent / "python3.11"
    lexical.symlink_to(interpreter)
    adapter = tmp_path / "setup/mcp_adapter.py"
    adapter.parent.mkdir()
    for name in ("mcp_adapter.py", "backend.py", "form_support.py"):
        (adapter.parent / name).write_text("# Fictional pinned setup source - never imported\n")
    parent = tmp_path / "sessions"
    parent.mkdir(mode=0o700)
    binding = {"scope": module.SCOPE, "command": str(lexical),
        "args": ["-I", "-B", str(adapter), "--families", "agno,services", "--python", str(lexical),
                 "--bundle", str(bundle), "--contents-sha256", digest(bundle / "CONTENTS.json")],
        "workspace_parent": str(parent), "contents_sha256": digest(bundle / "CONTENTS.json"),
        "catalog_sha256": digest(bundle / "catalog.json"), "catalog_count": 1,
        "setup_files": {path.name: digest(path) for path in adapter.parent.glob("*.py")}}
    config = tmp_path / "selected-setup.json"
    config.write_text(json.dumps(binding))
    config.chmod(0o600)
    monkeypatch.setenv(module.CONFIG_ENV, str(config))
    catalog = {"scope": "catalog_metadata_only", "entries": entries,
        **{key: binding[key] for key in ("contents_sha256", "catalog_sha256")}}
    selection = {"family": "services", "profile": "fixture-calendar", "operations": ["read_events"],
                 "values": {"token_env": "FIXTURE_CALENDAR_TOKEN"},
                 "validation_id": "b" * 32, "digest": "c" * 64}
    text = {"type": "string", "minLength": 1}
    schemas = {name: object_schema() for name in module.TOOLS}
    schemas["setup_catalog_entry"] = object_schema({key: text for key in IDENTITY})
    schemas["setup_prepare"] = object_schema({"family": {"enum": ["agno", "services"]}})
    schemas["setup_install"] = object_schema({"family": {"enum": ["agno", "services"]}, "profile": text,
        "operations": {"type": "array", "items": text, "minItems": 1, "uniqueItems": True},
        "values": {"type": "object"}, "validation_id": text, "digest": {"type": "string", "pattern": "^[a-f0-9]{64}$"}})
    schemas["setup_job"] = object_schema({"job_id": text})
    schemas["setup_stop"] = object_schema({"job_id": text})
    schemas["setup_jobs"] = object_schema({"before": {"type": ["string", "null"]}})
    schemas["setup_check"] = object_schema({"family": {"enum": ["agno", "services"]}, "profile": text,
        "operations": {"type": "array", "items": text, "minItems": 1, "uniqueItems": True}, "values": {"type": "object"}})
    schemas["setup_help"] = object_schema({"family": {"enum": ["agno", "services"]}, "profile": text})
    schemas["setup_recovery_manifest"] = object_schema({"job_ids": {"type": "array",
        "items": {"type": "string", "pattern": "^[a-f0-9]{32}$"}, "minItems": 1,
        "maxItems": 256, "uniqueItems": True}})
    check_arguments = {key: copy.deepcopy(selection[key]) for key in ("family", "profile", "operations", "values")}
    instances = []

    class FakeConnection:
        def __init__(self, value):
            self.config = copy.deepcopy(value)
            self.tools = {name: SimpleNamespace(inputSchema=copy.deepcopy(schema), description="Fictional owner contract")
                          for name, schema in schemas.items()}
            self.tools["execute_shell"] = SimpleNamespace(inputSchema=object_schema({"command": text}), description="Not granted")
            self.catalog = copy.deepcopy(catalog)
            self.state = {"mode": "setup", "families": [{"id": family, "status": "unprepared", "profiles": []}
                          for family in ("agno", "services")], "job": None, "scope": "Software configuration only"}
            self.calls, self.jobs = [], {}
            self.effects = 0
            self.responses = {}
            self.closed = 0
            self.lose_next_install = False
            self.start_error = None
            self.close_error = None
            self.close_entered = asyncio.Event()
            self.close_release = None
            instances.append(self)

        async def start(self):
            if self.start_error:
                raise self.start_error

        async def call(self, name, arguments):
            self.calls.append((name, copy.deepcopy(arguments)))
            if name in self.responses:
                response = self.responses[name]
                if callable(response):
                    response = response(arguments)
                    if asyncio.iscoroutine(response):
                        response = await response
                return copy.deepcopy(response)
            if name == "setup_catalog":
                result = self.catalog
            elif name == "setup_state":
                result = self.state
            elif name == "setup_catalog_entry":
                result = {**{key: self.catalog[key] for key in ("scope", "contents_sha256", "catalog_sha256")},
                          "entry": {"identity": copy.deepcopy(arguments), "strategies": [{"kind": "library"}]}}
            elif name == "setup_install":
                key = json.dumps(arguments, sort_keys=True)
                if key not in self.jobs:
                    self.effects += 1
                    self.jobs[key] = {"id": JOB, "kind": "install", "status": "running", "selection": copy.deepcopy(arguments)}
                result = {"job": self.jobs[key]}
                if self.lose_next_install:
                    self.lose_next_install = False
                    raise RuntimeError("PRIVATE_REPLY_CANARY /private/fixture")
            elif name == "setup_job":
                job = next(job for job in self.jobs.values() if job["id"] == arguments["job_id"])
                result = {"job": job}
            elif name == "setup_jobs":
                result = {"jobs": [{"id": job["id"], "status": job["status"]} for job in self.jobs.values()], "next_before": None}
            elif name == "setup_stop":
                job = next(job for job in self.jobs.values() if job["id"] == arguments["job_id"])
                job["status"] = "unconfirmed"
                result = {"stop": {"status": "requested"}}
            elif name == "setup_check":
                result = {"scope": "configuration_only", "family": arguments["family"], "profile": arguments["profile"],
                    "configuration_sha256": "c" * 64, "contents_sha256": binding["contents_sha256"]}
            elif name == "setup_help":
                result = {"family": arguments["family"], "profile": arguments["profile"],
                    "operations": [{"id": name} for name in ("read_article", "search_article", "write_admin", "read_events")]}
            else:
                result = {"observed": name}
            return {"structuredContent": copy.deepcopy(result)}

        async def close(self):
            self.closed += 1
            self.close_entered.set()
            if self.close_release is not None:
                await self.close_release.wait()
            if self.close_error:
                raise self.close_error

    monkeypatch.setattr(module, "MCPConnection", FakeConnection)
    return SimpleNamespace(binding=binding, config=config, adapter=adapter, bundle=bundle,
                           instances=instances, factory=FakeConnection, selection=selection, check_arguments=check_arguments)


@pytest.fixture
def prepared_host(host, tmp_path, monkeypatch, request):
    """Fictional owner-emitted configuration; no trusted source is executed."""
    target = getattr(request, "param", "macos-arm64")
    root = tmp_path
    bundle = root / "consumers"
    host.bundle.rename(bundle)
    entries = json.loads((bundle / "catalog.json").read_bytes())["entries"]
    (bundle / "catalog.json").write_text(json.dumps(entries))
    (bundle / "CONTENTS.json").write_text(json.dumps({"files": {
        "catalog.json": {"sha256": digest(bundle / "catalog.json")}}}))
    verifier = bundle / "install-library-consumer.py"
    verifier.write_text("# Fictional consumer verifier - never executed\n")
    setup_installer = host.adapter.parent / "install-library-consumer.py"
    setup_installer.write_text("# Fictional setup installer - never executed\n")
    selected = root / "selected"
    (selected / "bin").mkdir(parents=True)
    (selected / "provenance").mkdir()
    selected_files = {}
    for name in ("install-library-consumer.py", "toolkit-python-plan"):
        path = selected / "bin" / name
        path.write_text("# Fictional selected owner - never executed\n")
        selected_files["bin/" + name] = digest(path)
    selection = selected / "provenance/selection.json"
    selection.write_text(json.dumps({"platform": target, "files": selected_files}))
    output = root / "configuration"
    output.mkdir(mode=0o700)
    staging = root / "staging"
    staging.mkdir(mode=0o700)
    profile = {"schema_version": 1, "scope": module.SCOPE, "staging_root": str(staging),
        "installer": {"path": str(selected / "bin/install-library-consumer.py"),
            "sha256": selected_files["bin/install-library-consumer.py"]},
        "validators": {"python": {"path": str(selected / "bin/toolkit-python-plan"),
            "sha256": selected_files["bin/toolkit-python-plan"]}}}
    if target == "linux-x86_64":
        profile["python_target"] = target
    profile_path = output / "selected-install.json"
    profile_path.write_text(json.dumps(profile))
    python = Path(host.binding["command"]).resolve()
    mcp_python = Path(host.binding["command"])
    workspace = root / "prepared-workspace"
    launch = {"command": "/usr/bin/env", "args": ["-i", "PATH=/usr/bin:/bin", "PYTHONDONTWRITEBYTECODE=1",
        str(mcp_python), "-I", "-B", str(host.adapter), "--bundle", str(bundle),
        "--contents-sha256", digest(bundle / "CONTENTS.json"), "--workspace", str(workspace),
        "--families", "agno,services", "--verifier", str(verifier), "--verifier-sha256", digest(verifier),
        "--python", str(python), "--selected-install-profile", str(profile_path),
        "--selected-install-profile-sha256", digest(profile_path)]}
    config = output / "mcp.json"
    config.write_text(json.dumps({"mcpServers": {"toolkit-setup": launch}}))
    receipt = {"schema_version": 1, "status": "prepared", "scope": "host_setup_configuration_only",
        "release": {"archive_sha256": "e" * 64, "archive_root": root.name, "directory": str(root)},
        "target": target, "workspace": str(workspace),
        "runtimes": {name: {"path": str(path), "resolved_path": str(python), "sha256": digest(python),
            "executable": True} for name, path in (("python", python), ("mcp_python", mcp_python))},
        "public_pypi_enabled": False, "public_npm_enabled": False,
        "files": {"mcp.json": digest(config), "selected-install.json": digest(profile_path)}}
    receipt_path = output / "prepared.json"
    receipt_path.write_text(json.dumps(receipt))
    monkeypatch.setattr(module, "_PREPARED_RELEASES", {"e" * 64: {
        "target": target, "root": root.name, "contents_sha256": digest(bundle / "CONTENTS.json"),
        "selection_sha256": digest(selection), "setup_installer_sha256": digest(setup_installer)}})
    monkeypatch.setattr(module, "_PREPARED_SETUP_FILES", {
        name: digest(host.adapter.parent / name) for name in ("mcp_adapter.py", "backend.py", "form_support.py")})
    monkeypatch.setattr(module, "_PREPARED_CATALOG_SHA256", digest(bundle / "catalog.json"))
    monkeypatch.setattr(module, "_PREPARED_VERIFIER_SHA256", digest(verifier))
    host.binding.update(contents_sha256=digest(bundle / "CONTENTS.json"), catalog_sha256=digest(bundle / "catalog.json"))

    def connection(value):
        result = host.factory(value)
        result.catalog.update({key: host.binding[key] for key in ("contents_sha256", "catalog_sha256")})
        return result

    monkeypatch.setattr(module, "MCPConnection", connection)
    monkeypatch.setenv(module.CONFIG_ENV, str(config))
    return SimpleNamespace(**{**vars(host), "config": config, "bundle": bundle,
        "receipt": receipt, "receipt_path": receipt_path, "launch": launch,
        "workspace": workspace, "profile": profile, "profile_path": profile_path,
        "staging": staging, "python": python, "selected_manifest": selection})


@pytest.fixture
def saved_setup_host(prepared_host, monkeypatch):
    """One fictional request selected by the host's packaged bootstrap."""
    host = prepared_host
    prior_path = host.config.parent / "prior-request.json"
    prior_path.write_text(json.dumps({"family": "services", "profile": "gmail",
        "operations": ["retired_operation"], "values": {"token_env": "SAVED_TOKEN_REFERENCE"},
        "opaque_owner_binding": "Preserved for the Toolkit owner, never interpreted by Launch"}))
    prior = {"source": "recovery_manifest", "manifest_sha256": "a" * 64,
        "request_id": JOB, "request_sha256": digest(prior_path)}
    receipt, launch = copy.deepcopy(host.receipt), copy.deepcopy(host.launch)
    receipt["prior_selection"] = prior
    receipt["files"]["prior-request.json"] = prior["request_sha256"]
    launch["args"] += ["--prior-request", str(prior_path), "--prior-request-sha256", prior["request_sha256"]]
    host.config.write_text(json.dumps({"mcpServers": {"toolkit-setup": launch}}))
    receipt["files"]["mcp.json"] = digest(host.config)
    host.receipt_path.write_text(json.dumps(receipt))
    module._PREPARED_RELEASES[receipt["release"]["archive_sha256"]]["saved_setup"] = True
    parent_connection = module.MCPConnection

    def connection(config):
        value = parent_connection(config)
        value.tools["setup_successor"].inputSchema = object_schema({
            "prior_request_sha256": {"type": "string", "pattern": "^[a-f0-9]{64}$"}})
        value.state["prior_selection"] = {"family": "services", "profile": "gmail",
            "request_sha256": prior["request_sha256"]}
        value.responses["setup_successor"] = {"structuredContent": {
            "status": "review_required", "scope": "successor_configuration_candidate",
            "family": "services", "profile": "gmail", "prior_request_sha256": prior["request_sha256"],
            "values": {"token_env": "SAVED_TOKEN_REFERENCE"}, "prior_operations": ["retired_operation"],
            "current_help": {"operations": [{"id": "create_draft_message", "tool": "gmail_create_draft_message"}]},
            "unapplied": [{"name": "retired_field", "type": "string", "reason": "Not in current metadata"}]}}
        return value

    monkeypatch.setattr(module, "MCPConnection", connection)
    return SimpleNamespace(**{**vars(host), "receipt": receipt, "launch": launch,
        "prior": prior, "prior_path": prior_path, "connection_factory": connection})


@pytest.mark.asyncio
@pytest.mark.parametrize("prepared_host", ["macos-arm64", "linux-x86_64"], indirect=True)
async def test_saved_setup_reaches_current_settings_review_only_on_explicit_selected_digest(saved_setup_host):
    host = saved_setup_host
    before = host.prior_path.read_bytes()
    setup = ToolkitSetup()
    await setup.start()
    owner = host.instances[0]
    assert setup.status()["available"] and setup.status()["mode"] == "setup"
    assert setup.status()["prior_selection"] == host.prior
    assert owner.config["command"] == host.launch["command"] and owner.config["args"] == host.launch["args"]
    assert owner.config["args"].count("--workspace") == 1 and setup.workspace == host.workspace
    assert owner.calls == [("setup_catalog", {}), ("setup_state", {})]
    assert owner.jobs == {} and not owner.effects and setup.checked_selections == {}
    registered = {}
    setup.register_tools(SimpleNamespace(profile_name="connectors",
        register_tool=lambda name, callback, **kwargs: registered.update({name: (callback, kwargs)})))
    callback, options = registered["setup_successor"]
    assert host.prior["request_sha256"] in options["description"]
    assert "select current operations explicitly" in options["description"]
    assert "SAVED_TOKEN_REFERENCE" not in options["description"] and str(host.prior_path) not in options["description"]
    result = await callback({"prior_request_sha256": host.prior["request_sha256"]})
    assert result == owner.responses["setup_successor"]["structuredContent"]
    assert result["prior_operations"] == ["retired_operation"] and result["status"] == "review_required"
    assert owner.calls[-1] == ("setup_successor", {"prior_request_sha256": host.prior["request_sha256"]})
    assert owner.jobs == {} and not owner.effects and setup.checked_selections == {}
    assert host.prior_path.read_bytes() == before
    await setup.shutdown()
    assert owner.closed == 1 and host.prior_path.read_bytes() == before


@pytest.mark.asyncio
@pytest.mark.parametrize("change", ["unsupported-release", "raw-prior-without-selection", "recovery-mode",
    "missing-copy", "changed-copy", "missing-pin", "selection-source", "manifest-digest", "request-id",
    "request-digest", "extra-authority", "command-path", "command-digest", "missing-pair"])
async def test_saved_setup_rejects_unbound_or_changed_request_before_connection(saved_setup_host, change):
    host = saved_setup_host
    receipt, launch = copy.deepcopy(host.receipt), copy.deepcopy(host.launch)
    if change == "unsupported-release":
        module._PREPARED_RELEASES[receipt["release"]["archive_sha256"]].pop("saved_setup")
    elif change == "raw-prior-without-selection": receipt.pop("prior_selection")
    elif change == "recovery-mode": receipt["recovery"] = {"mode": "readback_only"}
    elif change == "missing-copy": host.prior_path.unlink()
    elif change == "changed-copy": host.prior_path.write_text("Different saved request")
    elif change == "missing-pin": receipt["files"].pop("prior-request.json")
    elif change == "selection-source": receipt["prior_selection"]["source"] = "automatic_selection"
    elif change == "manifest-digest": receipt["prior_selection"]["manifest_sha256"] = "not-a-digest"
    elif change == "request-id": receipt["prior_selection"]["request_id"] = "not-a-selected-id"
    elif change == "request-digest": receipt["prior_selection"]["request_sha256"] = "0" * 64
    elif change == "extra-authority": receipt["prior_selection"]["grants"] = ["gmail_write"]
    elif change == "command-path": launch["args"][-3] = str(host.staging / "different-request.json")
    elif change == "command-digest": launch["args"][-1] = "0" * 64
    else: launch["args"] = launch["args"][:-4]
    host.config.write_text(json.dumps({"mcpServers": {"toolkit-setup": launch}}))
    receipt["files"]["mcp.json"] = digest(host.config)
    host.receipt_path.write_text(json.dumps(receipt))
    setup = ToolkitSetup()
    await setup.start()
    assert not setup.status()["available"] and not host.instances and not host.workspace.exists()
    assert "prior_selection" not in setup.status()


@pytest.mark.asyncio
@pytest.mark.parametrize("failure", ["missing-tool", "missing-selection", "wrong-digest", "wrong-family", "empty-profile"])
async def test_saved_setup_refuses_changed_current_owner_selection_without_dispatch(saved_setup_host, monkeypatch, failure):
    host = saved_setup_host

    def connection(config):
        value = host.connection_factory(config)
        if failure == "missing-tool": value.tools.pop("setup_successor")
        elif failure == "missing-selection": value.state.pop("prior_selection")
        elif failure == "wrong-digest": value.state["prior_selection"]["request_sha256"] = "0" * 64
        elif failure == "wrong-family": value.state["prior_selection"]["family"] = "activepieces"
        else: value.state["prior_selection"]["profile"] = ""
        return value

    monkeypatch.setattr(module, "MCPConnection", connection)
    setup = ToolkitSetup()
    await setup.start()
    assert not setup.status()["available"] and not setup.schemas and setup.connection is None
    owner = host.instances[0]
    assert owner.closed == 1 and not owner.effects and not owner.jobs
    assert all(name in {"setup_catalog", "setup_state"} for name, _ in owner.calls)


@pytest.mark.asyncio
async def test_saved_review_digest_and_lost_reply_cannot_select_other_request_or_repeat_effects(saved_setup_host):
    host = saved_setup_host
    setup = ToolkitSetup()
    await setup.start()
    owner = host.instances[0]
    before = copy.deepcopy(owner.calls)
    with pytest.raises(ValueError, match="exact host-selected"):
        await setup.call("setup_successor", {"prior_request_sha256": "0" * 64})
    assert owner.calls == before

    def lost(_):
        raise RuntimeError("PRIVATE_SAVED_REVIEW_CANARY")

    owner.responses["setup_successor"] = lost
    with pytest.raises(ToolkitSetupUnavailable) as caught:
        await setup.call("setup_successor", {"prior_request_sha256": host.prior["request_sha256"]})
    assert "PRIVATE_SAVED_REVIEW_CANARY" not in str(caught.value)
    await setup.start()
    await setup.call("setup_state", {})
    assert setup.connection is owner and len(host.instances) == 1 and not owner.effects and not owner.jobs
    assert sum(name == "setup_successor" for name, _ in owner.calls) == 1 and setup.checked_selections == {}
    await setup.shutdown()


@pytest.mark.asyncio
async def test_advertised_saved_manifest_export_is_read_only_and_never_written_by_launch(host):
    setup = ToolkitSetup()
    await setup.start()
    owner = host.instances[0]
    result = {"manifest_json": '{"schema_version":1}', "manifest_sha256": "d" * 64, "manifest_bytes": 20}
    owner.responses["setup_recovery_manifest"] = {"structuredContent": result}
    assert await setup.call("setup_recovery_manifest", {"job_ids": [JOB]}) == result
    assert owner.calls[-1] == ("setup_recovery_manifest", {"job_ids": [JOB]})
    assert not owner.effects and not owner.jobs and not list(setup.workspace.parent.rglob("*manifest*"))
    with pytest.raises(ValueError):
        await setup.call("setup_successor", {"prior_request_sha256": "d" * 64})
    await setup.shutdown()


@pytest.fixture
def recovery_host(prepared_host, monkeypatch):
    """Fictional configured-library recovery emitted by the current owner."""
    host = prepared_host
    original = host.config.parent.parent / "original-workspace"
    original.mkdir(mode=0o700)
    marker = original / "retained-state"
    marker.write_text("Original owner state remains untouched")
    manifest = {"schema_version": 1, "workspace": str(original), "requests": [
        {"id": JOB, "sha256": "b" * 64}, {"id": "d" * 32, "sha256": "c" * 64}]}
    manifest_path = host.config.parent / "recover-manifest.json"
    manifest_path.write_text(json.dumps(manifest))
    host.profile_path.unlink()
    launch = copy.deepcopy(host.launch)
    launch["args"][-4:] = ["--recover-manifest", str(manifest_path),
        "--recover-manifest-sha256", digest(manifest_path)]
    host.config.write_text(json.dumps({"mcpServers": {"toolkit-setup": launch}}))
    receipt = copy.deepcopy(host.receipt)
    receipt["files"] = {"mcp.json": digest(host.config), "recover-manifest.json": digest(manifest_path)}
    receipt["recovery"] = {"mode": "readback_only", "workspace": str(original),
        "requests": len(manifest["requests"]), "manifest_sha256": digest(manifest_path)}
    host.receipt_path.write_text(json.dumps(receipt))
    known = module._PREPARED_RELEASES[receipt["release"]["archive_sha256"]]
    known.update(recovery=True, setup_files=copy.deepcopy(module._PREPARED_SETUP_FILES))

    def connection(value):
        result = host.factory(value)
        result.tools = {name: tool for name, tool in result.tools.items() if name in module.RECOVERY_TOOLS}
        result.state.update(mode="recovery", job_history={"recovery_selection": True,
            "count": len(manifest["requests"])}, **{name: {"available": False}
                for name in ("catalog", "stop", "preparation_retry", "selected_retry", "export_retry")})
        result.jobs = {row["id"]: {"id": row["id"], "kind": "install", "status": "unconfirmed",
            "recovered": True, "error": "historical_outcome_unknown", "recovery_request": copy.deepcopy(row)}
            for row in manifest["requests"]}
        return result

    monkeypatch.setattr(module, "MCPConnection", connection)
    return SimpleNamespace(**{**vars(host), "launch": launch, "receipt": receipt,
        "original": original, "marker": marker, "manifest": manifest, "manifest_path": manifest_path,
        "connection_factory": connection})


@pytest.mark.asyncio
@pytest.mark.parametrize("prepared_host", ["macos-arm64", "linux-x86_64"], indirect=True)
async def test_prepared_recovery_consumes_literal_owner_command_without_installation_profile(recovery_host):
    host = recovery_host
    setup = ToolkitSetup()
    await setup.start()
    connection = host.instances[0]
    assert setup.status()["available"] and setup.status()["mode"] == "recovery"
    assert setup.status()["scope"] == module.RECOVERY_SCOPE
    assert "readback" in setup.status()["detail"]
    assert connection.config["command"] == host.launch["command"]
    assert connection.config["args"] == host.launch["args"]
    assert connection.config["args"].count("--workspace") == 1 and setup.workspace == host.workspace
    assert connection.calls == [("setup_state", {})] and not host.profile_path.exists()
    assert set(setup.schemas) == module.RECOVERY_TOOLS
    assert (await setup.call("setup_job", {"job_id": JOB}))["job"]["status"] == "unconfirmed"
    await setup.shutdown()
    assert connection.calls[-1] == ("setup_jobs", {"before": None}) and connection.closed == 1
    assert not connection.effects and host.marker.read_text() == "Original owner state remains untouched"


@pytest.mark.asyncio
async def test_recovery_never_registers_or_dispatches_effects_or_installation_handoff(recovery_host, monkeypatch):
    host = recovery_host

    def connection(config):
        value = host.connection_factory(config)
        # An accidentally over-advertised endpoint cannot widen the host binding.
        for name in module.TOOLS - module.RECOVERY_TOOLS:
            value.tools[name] = SimpleNamespace(inputSchema=object_schema(), description="Not granted")
        return value

    monkeypatch.setattr(module, "MCPConnection", connection)
    setup = ToolkitSetup()
    await setup.start()
    registered = {}
    setup.register_tools(SimpleNamespace(profile_name="connectors",
        register_tool=lambda name, callback, **kwargs: registered.update({name: (callback, kwargs)})))
    assert set(registered) == module.RECOVERY_TOOLS and "inspect_toolkit_installation" not in registered
    assert all("retained setup readback only" in options["description"] for _, options in registered.values())
    owner = host.instances[0]
    before = copy.deepcopy(owner.calls)
    for name in module.TOOLS - module.RECOVERY_TOOLS:
        with pytest.raises(ValueError):
            await setup.call(name, {})
    with pytest.raises(ValueError, match="cannot authorize"):
        await setup.handoff({"job_id": JOB, "connection_id": "reader", "name": "Reader"})
    assert owner.calls == before and not owner.effects
    await registered["setup_job"][0]({"job_id": JOB})
    assert owner.calls[-1] == ("setup_job", {"job_id": JOB})
    await setup.shutdown()


@pytest.mark.asyncio
async def test_recovery_lost_observation_retains_same_owner_without_replaying_effects(recovery_host):
    host = recovery_host
    setup = ToolkitSetup()
    await setup.start()
    owner = host.instances[0]

    def lost(_):
        raise RuntimeError("PRIVATE_RECOVERY_CANARY")

    owner.responses["setup_reconcile"] = lost
    with pytest.raises(ToolkitSetupUnavailable) as caught:
        await setup.call("setup_reconcile", {})
    assert "PRIVATE_RECOVERY_CANARY" not in str(caught.value)
    assert setup.connection is owner and setup.status()["available"] and not owner.closed
    await setup.start()
    await setup.call("setup_job", {"job_id": JOB})
    assert len(host.instances) == 1 and not owner.effects
    assert sum(name == "setup_reconcile" for name, _ in owner.calls) == 1
    await setup.shutdown()


@pytest.mark.asyncio
async def test_recovery_new_workspace_is_single_use_and_preserves_original_state(recovery_host):
    host = recovery_host
    setup = ToolkitSetup()
    await setup.start()
    host.workspace.mkdir(mode=0o700)
    new_marker = host.workspace / "readback-state"
    new_marker.write_text("The admitted recovery owner's observations")
    await setup.shutdown()
    await setup.start()
    assert not setup.status()["available"] and len(host.instances) == 1
    assert new_marker.read_text() == "The admitted recovery owner's observations"
    assert host.marker.read_text() == "Original owner state remains untouched"
    await setup.shutdown()


@pytest.mark.asyncio
@pytest.mark.parametrize("change", ["unsupported-recovery-release", "mode", "missing-manifest", "manifest-bytes",
    "manifest-schema", "empty-selection", "duplicate-selection", "invalid-id", "invalid-hash", "extra-anchor",
    "count", "boolean-count", "receipt-workspace", "manifest-workspace", "overlap", "extra-profile",
    "install-command", "source-bytes", "unlisted-source"])
async def test_invalid_prepared_recovery_refuses_before_connection_without_effects(recovery_host, change):
    host = recovery_host
    receipt, launch, manifest = copy.deepcopy(host.receipt), copy.deepcopy(host.launch), copy.deepcopy(host.manifest)
    if change == "unsupported-recovery-release":
        module._PREPARED_RELEASES[receipt["release"]["archive_sha256"]].pop("recovery")
    elif change == "mode": receipt["recovery"]["mode"] = "setup"
    elif change == "missing-manifest": host.manifest_path.unlink()
    elif change == "manifest-bytes": host.manifest_path.write_text("changed")
    elif change == "manifest-schema": manifest["schema_version"] = True
    elif change == "empty-selection": manifest["requests"] = []
    elif change == "duplicate-selection": manifest["requests"][1] = copy.deepcopy(manifest["requests"][0])
    elif change == "invalid-id": manifest["requests"][0]["id"] = "not-an-anchor"
    elif change == "invalid-hash": manifest["requests"][0]["sha256"] = "not-a-digest"
    elif change == "extra-anchor": manifest["requests"][0]["install"] = True
    elif change == "count": receipt["recovery"]["requests"] = 1
    elif change == "boolean-count": receipt["recovery"]["requests"] = True
    elif change == "receipt-workspace": receipt["recovery"]["workspace"] = str(host.staging)
    elif change == "manifest-workspace": manifest["workspace"] = str(host.staging)
    elif change == "overlap":
        receipt["recovery"]["workspace"] = manifest["workspace"] = str(host.config.parent.parent)
    elif change == "extra-profile": receipt["files"]["selected-install.json"] = "0" * 64
    elif change == "install-command": launch["args"][-4] = "--selected-install-profile"
    elif change == "source-bytes": host.adapter.write_text("# Changed recovery owner\n")
    else: (host.adapter.parent / "unlisted.py").write_text("# Not part of the frozen archive\n")
    if change in {"manifest-schema", "empty-selection", "duplicate-selection", "invalid-id", "invalid-hash",
            "extra-anchor", "manifest-workspace", "overlap"}:
        host.manifest_path.write_text(json.dumps(manifest))
        receipt["recovery"]["manifest_sha256"] = receipt["files"]["recover-manifest.json"] = digest(host.manifest_path)
        launch["args"][-1] = digest(host.manifest_path)
    if change == "install-command" or launch != host.launch:
        host.config.write_text(json.dumps({"mcpServers": {"toolkit-setup": launch}}))
        receipt["files"]["mcp.json"] = digest(host.config)
    host.receipt_path.write_text(json.dumps(receipt))
    setup = ToolkitSetup()
    await setup.start()
    assert not setup.status()["available"] and not host.instances
    assert not host.workspace.exists() and host.marker.read_text() == "Original owner state remains untouched"
    await setup.shutdown()


@pytest.mark.asyncio
@pytest.mark.parametrize("failure", ["normal-mode", "missing-readback-tool", "wrong-count", "boolean-count",
    "missing-selection", "stop-authority", "retry-authority", "catalog-authority"])
async def test_recovery_start_refuses_changed_owner_contract_without_install_or_retry(recovery_host, monkeypatch, failure):
    host = recovery_host

    def connection(config):
        value = host.connection_factory(config)
        if failure == "normal-mode": value.state["mode"] = "setup"
        elif failure == "missing-readback-tool": value.tools.pop("setup_reconcile")
        elif failure == "wrong-count": value.state["job_history"]["count"] = 1
        elif failure == "boolean-count": value.state["job_history"]["count"] = True
        elif failure == "missing-selection": value.state["job_history"].pop("recovery_selection")
        elif failure == "stop-authority": value.state["stop"]["available"] = True
        elif failure == "retry-authority": value.state["selected_retry"]["available"] = True
        else: value.state["catalog"]["available"] = True
        return value

    monkeypatch.setattr(module, "MCPConnection", connection)
    setup = ToolkitSetup()
    await setup.start()
    assert not setup.status()["available"] and setup.connection is None and not setup.schemas
    assert host.instances[0].closed == 1 and not host.instances[0].effects
    assert all(name == "setup_state" for name, _ in host.instances[0].calls)
    await setup.shutdown()


@pytest.mark.asyncio
async def test_recovery_cleanup_never_stops_a_misreported_worker_or_replaces_owner(recovery_host):
    host = recovery_host
    setup = ToolkitSetup()
    await setup.start()
    owner = host.instances[0]
    owner.jobs[JOB]["status"] = "running"
    with pytest.raises(ToolkitSetupUnavailable, match="existing owner is retained"):
        await setup.shutdown()
    assert setup.connection is owner and not owner.closed and not owner.effects
    assert not any(name == "setup_stop" for name, _ in owner.calls)
    await setup.start()
    assert len(host.instances) == 1
    owner.jobs[JOB]["status"] = "unconfirmed"
    await setup.shutdown()
    assert owner.closed == 1


@pytest.mark.asyncio
@pytest.mark.parametrize("prepared_host", ["macos-arm64", "linux-x86_64"], indirect=True)
async def test_current_release_normal_preparation_keeps_installation_mode_and_its_own_source_inventory(prepared_host):
    host = prepared_host
    additional = host.adapter.parent / "compose_runtime.py"
    additional.write_text("# Fictional newer owner module - never imported\n")
    known = module._PREPARED_RELEASES[host.receipt["release"]["archive_sha256"]]
    known.update(recovery=True, setup_files={**module._PREPARED_SETUP_FILES, additional.name: digest(additional)})
    setup = ToolkitSetup()
    await setup.start()
    assert setup.status()["available"] and setup.status()["mode"] == "setup"
    assert setup.status()["scope"] == module.SCOPE
    assert host.instances[0].config["args"] == host.launch["args"]
    assert {name for name, _ in host.instances[0].calls} == {"setup_catalog", "setup_state"}
    registered = {}
    setup.register_tools(SimpleNamespace(profile_name="connectors",
        register_tool=lambda name, callback, **kwargs: registered.update({name: callback})))
    assert "setup_install" in registered and "inspect_toolkit_installation" in registered
    await setup.shutdown()


@pytest.mark.asyncio
@pytest.mark.parametrize("prepared_host", ["macos-arm64", "linux-x86_64"], indirect=True)
async def test_release_specific_catalog_and_verifier_pins_preserve_the_emitted_command(prepared_host):
    host = prepared_host
    known = module._PREPARED_RELEASES[host.receipt["release"]["archive_sha256"]]
    (host.bundle / "catalog.json").write_text(json.dumps([{
        "identity": IDENTITY, "display_name": "New frozen catalog", "description": "Metadata only"}]))
    verifier = host.bundle / "install-library-consumer.py"
    verifier.write_text("# Fictional new verifier - never executed\n")
    known.update(catalog_sha256=digest(host.bundle / "catalog.json"), verifier_sha256=digest(verifier))
    host.binding["catalog_sha256"] = known["catalog_sha256"]
    host.launch["args"][18] = known["verifier_sha256"]
    host.config.write_text(json.dumps({"mcpServers": {"toolkit-setup": host.launch}}))
    host.receipt["files"]["mcp.json"] = digest(host.config)
    host.receipt_path.write_text(json.dumps(host.receipt))
    setup = ToolkitSetup()
    await setup.start()
    assert setup.status()["available"]
    assert setup.binding["catalog_sha256"] == known["catalog_sha256"]
    assert host.instances[0].config["args"] == host.launch["args"]
    assert {name for name, _ in host.instances[0].calls} == {"setup_catalog", "setup_state"}
    await setup.shutdown()


@pytest.mark.asyncio
@pytest.mark.parametrize("change", ["catalog-bytes", "verifier-bytes", "verifier-argument"])
async def test_release_specific_pins_refuse_a_different_catalog_verifier_or_launch_before_start(prepared_host, change):
    host = prepared_host
    known = module._PREPARED_RELEASES[host.receipt["release"]["archive_sha256"]]
    known.update(catalog_sha256=digest(host.bundle / "catalog.json"),
        verifier_sha256=digest(host.bundle / "install-library-consumer.py"))
    if change == "catalog-bytes":
        (host.bundle / "catalog.json").write_text("[]")
    elif change == "verifier-bytes":
        (host.bundle / "install-library-consumer.py").write_text("# Different verifier\n")
    else:
        host.launch["args"][18] = "a" * 64
        host.config.write_text(json.dumps({"mcpServers": {"toolkit-setup": host.launch}}))
        host.receipt["files"]["mcp.json"] = digest(host.config)
        host.receipt_path.write_text(json.dumps(host.receipt))
    setup = ToolkitSetup()
    await setup.start()
    assert not setup.status()["available"] and not host.instances
    assert not host.workspace.exists()
    await setup.shutdown()


@pytest.mark.asyncio
async def test_prepared_owner_launch_retains_exact_arguments_workspace_and_minimal_environment(prepared_host, monkeypatch):
    host = prepared_host
    monkeypatch.setenv("OPENAI_API_KEY", "PRIVATE_ENV_CANARY")
    setup = ToolkitSetup()
    await setup.start()
    assert setup.status()["available"]
    config = host.instances[0].config
    assert config["command"] == host.launch["command"]
    assert config["args"] == host.launch["args"]
    assert config["args"].count("--workspace") == 1
    assert setup.workspace == host.workspace and not host.workspace.exists()
    assert setup.binding["families"] == ["agno", "services"]
    assert setup.binding["catalog_count"] == 1
    assert set(config["env"]) == {"PATH", "HOME", "TMPDIR", "TEMP", "LANG", "PYTHONDONTWRITEBYTECODE"}
    assert "PRIVATE_ENV_CANARY" not in json.dumps(config)
    assert {name for name, _ in host.instances[0].calls} == {"setup_catalog", "setup_state"}
    await setup.start()
    assert len(host.instances) == 1
    await setup.shutdown()


@pytest.mark.asyncio
@pytest.mark.parametrize("prepared_host", ["linux-x86_64"], indirect=True)
async def test_linux_prepared_profile_accepts_its_required_owner_emitted_python_target(prepared_host):
    host = prepared_host
    assert host.profile["python_target"] == host.receipt["target"] == "linux-x86_64"
    setup = ToolkitSetup()
    await setup.start()
    assert setup.status()["available"]
    assert host.instances[0].config["args"] == host.launch["args"]
    assert setup.workspace == host.workspace
    await setup.shutdown()


@pytest.mark.asyncio
@pytest.mark.parametrize("prepared_host,target", [
    ("linux-x86_64", None), ("linux-x86_64", "macos-arm64"), ("macos-arm64", "linux-x86_64")],
    indirect=["prepared_host"])
async def test_prepared_python_target_matches_the_exact_platform_profile_shape(prepared_host, target):
    host = prepared_host
    profile = copy.deepcopy(host.profile)
    if target is None:
        profile.pop("python_target")
    else:
        profile["python_target"] = target
    host.profile_path.write_text(json.dumps(profile))
    launch = copy.deepcopy(host.launch)
    launch["args"][-1] = digest(host.profile_path)
    host.config.write_text(json.dumps({"mcpServers": {"toolkit-setup": launch}}))
    receipt = copy.deepcopy(host.receipt)
    receipt["files"].update({"selected-install.json": digest(host.profile_path), "mcp.json": digest(host.config)})
    host.receipt_path.write_text(json.dumps(receipt))
    setup = ToolkitSetup()
    await setup.start()
    assert not setup.status()["available"] and not host.instances
    assert not host.workspace.exists()
    await setup.shutdown()


@pytest.mark.asyncio
async def test_prepared_workspace_belongs_to_one_owner_and_is_never_replaced(prepared_host):
    host = prepared_host
    setup = ToolkitSetup()
    await setup.start()
    host.workspace.mkdir(mode=0o700)  # The fictional backend admits its fixed workspace.
    marker = host.workspace / "owned-session"
    marker.write_text("Existing setup state")
    await setup.shutdown()
    await setup.start()
    assert not setup.status()["available"] and len(host.instances) == 1
    assert marker.read_text() == "Existing setup state"
    await setup.shutdown()


@pytest.mark.asyncio
@pytest.mark.parametrize("change", ["scope", "unknown-release", "wrong-target", "public-acquisition", "unsafe-receipt",
    "missing-receipt", "mcp-bytes", "profile-bytes", "runtime-bytes", "runtime-link", "source-bytes", "unlisted-source",
    "catalog-bytes", "contents-bytes", "selected-manifest", "selected-tool", "arbitrary-selected-tool", "extra-server",
    "extra-env", "extra-argument", "duplicate-family", "unsupported-family", "workspace-mismatch", "occupied-workspace"])
async def test_invalid_prepared_binding_refuses_before_connection_without_replacing_workspace(prepared_host, change):
    host = prepared_host
    receipt, launch = copy.deepcopy(host.receipt), copy.deepcopy(host.launch)
    if change == "scope": receipt["scope"] = "account_access_and_native_attachment"
    elif change == "unknown-release": receipt["release"]["archive_sha256"] = "0" * 64
    elif change == "wrong-target": receipt["target"] = "linux-x86_64"
    elif change == "public-acquisition": receipt["public_pypi_enabled"] = True
    elif change == "unsafe-receipt": host.receipt_path.chmod(0o666)
    elif change == "missing-receipt": host.receipt_path.unlink()
    elif change == "mcp-bytes": host.config.write_text("changed")
    elif change == "profile-bytes": host.profile_path.write_text("changed")
    elif change == "runtime-bytes": host.python.write_text("Changed interpreter")
    elif change == "runtime-link": receipt["runtimes"]["mcp_python"]["resolved_path"] = "/other/interpreter"
    elif change == "source-bytes": host.adapter.write_text("# Changed adapter\n")
    elif change == "unlisted-source": (host.adapter.parent / "foreign.py").write_text("# Foreign source\n")
    elif change == "catalog-bytes": (host.bundle / "catalog.json").write_text("changed")
    elif change == "contents-bytes": (host.bundle / "CONTENTS.json").write_text("changed")
    elif change == "selected-manifest": host.selected_manifest.write_text("changed")
    elif change == "selected-tool": Path(host.profile["validators"]["python"]["path"]).write_text("changed")
    elif change == "arbitrary-selected-tool":
        profile = copy.deepcopy(host.profile)
        profile["installer"]["path"] = str(host.python)
        profile["installer"]["sha256"] = digest(host.python)
        host.profile_path.write_text(json.dumps(profile))
        receipt["files"]["selected-install.json"] = digest(host.profile_path)
        launch["args"][-1] = digest(host.profile_path)
    elif change == "extra-server": pass
    elif change == "extra-env": launch["env"] = {"TOKEN": "PRIVATE_ENV_CANARY"}
    elif change == "extra-argument": launch["args"] += ["--allow-public-pypi"]
    elif change == "duplicate-family": launch["args"][14] = "agno,agno"
    elif change == "unsupported-family": launch["args"][14] = "activepieces"
    elif change == "workspace-mismatch": launch["args"][12] = str(host.workspace.parent / "different")
    else: host.workspace.mkdir(mode=0o700)
    if change in {"arbitrary-selected-tool", "extra-server", "extra-env", "extra-argument", "duplicate-family",
            "unsupported-family", "workspace-mismatch"}:
        servers = {"toolkit-setup": launch}
        if change == "extra-server": servers["foreign"] = copy.deepcopy(launch)
        host.config.write_text(json.dumps({"mcpServers": servers}))
        receipt["files"]["mcp.json"] = digest(host.config)
    if change != "missing-receipt": host.receipt_path.write_text(json.dumps(receipt))
    setup = ToolkitSetup()
    await setup.start()
    assert not host.instances and not setup.status()["available"]
    assert "PRIVATE_ENV_CANARY" not in json.dumps(setup.status())
    assert host.workspace.exists() == (change == "occupied-workspace")
    await setup.shutdown()


@pytest.mark.asyncio
async def test_disabled_default_never_constructs_or_launches(monkeypatch):
    monkeypatch.delenv(module.CONFIG_ENV, raising=False)
    monkeypatch.setattr(module, "MCPConnection", lambda value: pytest.fail("Opt-in is required before constructing a connection"))
    setup = ToolkitSetup()
    await setup.start()
    assert setup.status()["status"] == "disabled" and setup.status()["available"] is False
    draft = SimpleNamespace(profile_name="connectors", register_tool=lambda *args, **kwargs: pytest.fail("No disabled tools"))
    setup.register_tools(draft)
    with pytest.raises(ValueError):
        await setup.call("setup_install", {})
    await setup.shutdown()


@pytest.mark.asyncio
async def test_host_selected_lexical_interpreter_minimal_environment_and_fresh_workspace(host, monkeypatch):
    monkeypatch.setenv("HOME", "/unrelated-private-home")
    monkeypatch.setenv("TMPDIR", "/unrelated-private-tmp")
    monkeypatch.setenv("OPENAI_API_KEY", "PRIVATE_ENV_CANARY")
    monkeypatch.setenv(module.CONFIG_ENV, "/not/the/explicit/config")
    setup = ToolkitSetup(config_path=host.config)
    await setup.start()
    assert setup.status()["available"] is True
    config = host.instances[0].config
    assert config["preserve_tool_errors"] is True
    assert config["command"] == host.binding["command"]
    assert config["args"][:-2] == host.binding["args"]
    assert config["args"][-2:] == ["--workspace", str(setup.workspace)]
    assert setup.workspace.parent.parent == Path(host.binding["workspace_parent"])
    assert not setup.workspace.exists()
    assert config["env"] == {"PATH": "/usr/bin:/bin", "LANG": "C.UTF-8",
        "HOME": str(setup.workspace.parent / "home"), "TMPDIR": str(setup.workspace.parent / "tmp"),
        "TEMP": str(setup.workspace.parent / "tmp"), "PYTHONDONTWRITEBYTECODE": "1"}
    assert "PRIVATE_ENV_CANARY" not in json.dumps(config) and "unrelated-private" not in json.dumps(config)
    for name in ("home", "tmp"):
        assert (setup.workspace.parent / name).stat().st_mode & 0o077 == 0
    old_workspace = setup.workspace
    await setup.start()
    assert len(host.instances) == 1
    await setup.shutdown()
    assert host.instances[0].closed == 1 and setup.connection is None
    await setup.start()
    assert len(host.instances) == 2 and setup.workspace != old_workspace and old_workspace.parent.exists()
    await setup.shutdown()


@pytest.mark.asyncio
@pytest.mark.parametrize("change", ["scope", "extra-secret", "relative-command", "different-interpreter", "arbitrary-workspace",
    "unsupported-family", "duplicate-family", "adapter-changed", "unlisted-source", "missing-source", "source-pin",
    "contents-pin", "catalog-changed", "boolean-count", "unsafe-config", "unsafe-parent"])
async def test_invalid_host_selection_refuses_before_connection_construction(host, change):
    binding = copy.deepcopy(host.binding)
    if change == "scope": binding["scope"] = "account_access_and_native_attachment"
    elif change == "extra-secret": binding["env"] = {"TOKEN": "PRIVATE_ENV_CANARY"}
    elif change == "relative-command": binding["command"] = "python"
    elif change == "different-interpreter": binding["args"][6] = "/other/python"
    elif change == "arbitrary-workspace": binding["args"] += ["--workspace", "/caller/path"]
    elif change == "unsupported-family": binding["args"][4] = "activepieces"
    elif change == "duplicate-family": binding["args"][4] = "services,services"
    elif change == "adapter-changed": host.adapter.write_text("# Changed selected executable input\n")
    elif change == "unlisted-source": (host.adapter.parent / "unlisted.py").write_text("# Additional source\n")
    elif change == "missing-source": (host.adapter.parent / "form_support.py").unlink()
    elif change == "source-pin": binding["setup_files"]["backend.py"] = "0" * 64
    elif change == "contents-pin": binding["contents_sha256"] = binding["args"][10] = "0" * 64
    elif change == "catalog-changed": (host.bundle / "catalog.json").write_text("changed catalog")
    elif change == "boolean-count": binding["catalog_count"] = True
    elif change == "unsafe-config": host.config.chmod(0o666)
    else: Path(binding["workspace_parent"]).chmod(0o777)
    host.config.write_text(json.dumps(binding))
    setup = ToolkitSetup()
    await setup.start()
    assert not host.instances and setup.status()["available"] is False
    assert setup.status()["status"] == "unavailable"
    assert "PRIVATE_ENV_CANARY" not in json.dumps(setup.status()) and str(host.config) not in json.dumps(setup.status())


@pytest.mark.asyncio
@pytest.mark.parametrize("failure", ["missing-tool", "malformed-schema", "wrong-mode", "missing-family", "foreign-family",
    "duplicate-state-family", "wrong-catalog-pin", "catalog-owner-error", "state-owner-error", "start-error"])
async def test_start_contract_failure_closes_the_connection_and_preserves_safe_status(host, monkeypatch, failure):
    def connection(config):
        value = host.factory(config)
        if failure == "missing-tool": value.tools.pop("setup_install")
        elif failure == "malformed-schema": value.tools["setup_install"].inputSchema = {"type": "not-a-json-schema-type"}
        elif failure == "wrong-mode": value.state["mode"] = "recovery"
        elif failure == "missing-family": value.state["families"].pop()
        elif failure == "foreign-family": value.state["families"][0]["id"] = "activepieces"
        elif failure == "duplicate-state-family": value.state["families"].append(copy.deepcopy(value.state["families"][0]))
        elif failure == "wrong-catalog-pin": value.catalog["contents_sha256"] = "0" * 64
        elif failure == "catalog-owner-error": value.responses["setup_catalog"] = {"isError": True, "structuredContent": {"error": "catalog_unavailable"}}
        elif failure == "state-owner-error": value.responses["setup_state"] = {"isError": True, "structuredContent": {"error": "host_closing"}}
        else: value.start_error = RuntimeError("PRIVATE_START_CANARY /private/executable")
        return value
    monkeypatch.setattr(module, "MCPConnection", connection)
    setup = ToolkitSetup()
    await setup.start()
    assert setup.status()["available"] is False and setup.connection is None
    assert host.instances[0].closed == 1 and not setup.schemas
    assert "PRIVATE_START_CANARY" not in json.dumps(setup.status())
    await setup.shutdown()
    assert host.instances[0].closed == 1


@pytest.mark.asyncio
async def test_advertised_schema_and_host_tool_allowlist_prevent_arbitrary_dispatch(host):
    setup = ToolkitSetup()
    await setup.start()
    connection = host.instances[0]
    before = copy.deepcopy(connection.calls)
    for name, args in [("execute_shell", {"command": "not permitted"}), ("setup_acquire_node", {}),
                       ("setup_prepare", {"family": "activepieces"}), ("setup_state", {"path": "/caller"}),
                       ("setup_install", {**host.selection, "destination": "/caller"}),
                       ("setup_install", {**host.selection, "digest": "invented"})]:
        with pytest.raises(ValueError):
            await setup.call(name, args)
    assert connection.calls == before and connection.effects == 0 and setup.status()["available"]
    await setup.call("setup_prepare", {"family": "services"})
    assert connection.calls[-1] == ("setup_prepare", {"family": "services"})
    await setup.shutdown()


@pytest.mark.asyncio
async def test_lost_install_reply_uses_same_connection_for_exact_job_readback_without_host_retry(host):
    setup = ToolkitSetup()
    await setup.start()
    connection = host.instances[0]
    connection.lose_next_install = True
    with pytest.raises(ToolkitSetupUnavailable) as error:
        await setup.call("setup_install", copy.deepcopy(host.selection))
    assert "PRIVATE_REPLY_CANARY" not in str(error.value)
    assert setup.connection is connection and setup.status()["available"] and connection.closed == 0
    assert connection.effects == 1 and sum(name == "setup_install" for name, _ in connection.calls) == 1
    readback = await setup.call("setup_job", {"job_id": JOB})
    assert readback["job"]["id"] == JOB and readback["job"]["status"] == "running"
    assert sum(name == "setup_install" for name, _ in connection.calls) == 1
    explicit_replay = await setup.call("setup_install", copy.deepcopy(host.selection))
    assert explicit_replay == readback and connection.effects == 1 and len(host.instances) == 1
    assert sum(name == "setup_install" for name, _ in connection.calls) == 2
    readback["job"]["selection"]["operations"].append("invented_write")
    assert (await setup.call("setup_job", {"job_id": JOB}))["job"]["selection"] == host.selection
    assert "authorized" not in explicit_replay and "attached" not in explicit_replay
    await setup.shutdown()


@pytest.mark.asyncio
async def test_only_connectors_register_exact_advertised_tools_and_callback_targets(host):
    setup = ToolkitSetup()
    await setup.start()
    registered = {}
    for profile in ("architect", "connectors", "reviewer", "visualizer"):
        tools = {}
        setup.register_tools(SimpleNamespace(profile_name=profile,
            register_tool=lambda name, callback, **kwargs: tools.update({name: (callback, kwargs)})))
        registered[profile] = tools
    assert all(not registered[profile] for profile in ("architect", "reviewer", "visualizer"))
    assert set(registered["connectors"]) == (module.TOOLS - {"setup_successor"}) | {"inspect_toolkit_installation"}
    assert "execute_shell" not in registered["connectors"]
    for name in module.TOOLS - {"setup_successor"}:
        _, options = registered["connectors"][name]
        assert options["input_schema"] == host.instances[0].tools[name].inputSchema
    await registered["connectors"]["setup_prepare"][0]({"family": "agno"})
    await registered["connectors"]["setup_state"][0]({})
    assert host.instances[0].calls[-2:] == [("setup_prepare", {"family": "agno"}), ("setup_state", {})]
    registered["connectors"]["setup_prepare"][1]["input_schema"]["properties"]["family"]["enum"].append("activepieces")
    with pytest.raises(ValueError):
        await registered["connectors"]["setup_prepare"][0]({"family": "activepieces"})
    await setup.shutdown()


@pytest.mark.asyncio
async def test_external_schema_reference_cannot_trigger_remote_resolution_or_dispatch(host, monkeypatch):
    def connection(config):
        value = host.factory(config)
        value.tools["setup_prepare"].inputSchema = {"$ref": "https://fixture.invalid/remote-schema.json"}
        return value
    monkeypatch.setattr(module, "MCPConnection", connection)
    setup = ToolkitSetup()
    await setup.start()
    assert setup.status()["available"]
    before = copy.deepcopy(host.instances[0].calls)
    with pytest.raises(ValueError, match="advertised Toolkit setup arguments"):
        await setup.call("setup_prepare", {"family": "services"})
    assert host.instances[0].calls == before
    await setup.shutdown()


@pytest.mark.asyncio
async def test_owner_configuration_refusal_preserves_fixed_guidance_and_same_session_for_correction(host):
    setup = ToolkitSetup()
    await setup.start()
    connection = host.instances[0]
    refusal = {"error": "invalid_configuration", "diagnostic": {"version": 1,
        "phase": "configuration_validation", "code": "channel_list_conflict", "field_path": ["operations"],
        "message": "Keep the channel restriction and deselect list_conversations_page.",
        "family": "services", "profile": "slack", "configuration_sha256": "d" * 64,
        "contents_sha256": host.binding["contents_sha256"]}}
    connection.responses["setup_check"] = {"isError": True, "structuredContent": copy.deepcopy(refusal)}
    result = await setup.call("setup_check", host.check_arguments)
    assert result == refusal and setup.connection is connection and connection.effects == 0
    result["diagnostic"]["field_path"].append("changed")
    assert connection.responses["setup_check"]["structuredContent"] == refusal
    connection.responses.pop("setup_check")
    assert (await setup.call("setup_check", host.check_arguments))["scope"] == "configuration_only"
    assert sum(name == "setup_check" for name, _ in connection.calls) == 2
    await setup.shutdown()


@pytest.mark.asyncio
@pytest.mark.parametrize("response", [
    {"isError": True, "structuredContent": {"error": "PRIVATE_CANARY /private/path"}},
    {"isError": True, "structuredContent": {"error": "invalid_arguments", "raw_output": "PRIVATE_CANARY"}},
    {"isError": True, "structuredContent": {"error": 7}},
    {"isError": True, "content": [{"type": "text", "text": "PRIVATE_CANARY"}]},
])
async def test_malformed_owner_error_is_sanitized_without_retry_or_losing_readback_owner(host, response):
    setup = ToolkitSetup()
    await setup.start()
    connection = host.instances[0]
    connection.responses["setup_check"] = response
    with pytest.raises(ToolkitSetupUnavailable) as caught:
        await setup.call("setup_check", host.check_arguments)
    assert "PRIVATE_CANARY" not in str(caught.value)
    assert sum(name == "setup_check" for name, _ in connection.calls) == 1
    assert setup.connection is connection and setup.status()["available"]
    assert (await setup.call("setup_state", {}))["mode"] == "setup"
    await setup.shutdown()


@pytest.mark.asyncio
@pytest.mark.parametrize("fault", ["owner-refusal", "wrong-job"])
async def test_handoff_requires_exact_job_readback_from_the_current_owner(host, fault):
    setup = ToolkitSetup()
    await setup.start()
    connection = host.instances[0]
    if fault == "owner-refusal":
        connection.responses["setup_job"] = {"isError": True, "structuredContent": {"error": "unknown_job"}}
    else:
        connection.responses["setup_job"] = {"structuredContent": {"job": {"id": "e" * 32, "kind": "install", "status": "succeeded"}}}
    before = copy.deepcopy(connection.calls)
    with pytest.raises(ToolkitSetupUnavailable, match="could not be confirmed"):
        await setup.handoff({"job_id": JOB, "connection_id": "calendar", "name": "Calendar"})
    assert connection.calls[len(before):] == [("setup_job", {"job_id": JOB})]
    assert connection.effects == 0 and setup.connection is connection
    before = copy.deepcopy(connection.calls)
    with pytest.raises(ValueError):
        await setup.handoff({"job_id": "invented", "connection_id": "calendar", "name": "Calendar"})
    assert connection.calls == before
    await setup.shutdown()


@pytest.mark.asyncio
async def test_registered_handoff_returns_succeeded_owned_launch_and_private_slots_without_grants(host):
    from launchkit.blueprint import Connection, starter_blueprint

    setup = ToolkitSetup()
    await setup.start()
    connection = host.instances[0]
    await setup.call("setup_install", host.selection)
    installed = next(iter(connection.jobs.values()))
    installed.update(status="succeeded", result={"scope": "observed_software_installation_only",
        "credential_environment_references": [{"field_path": ["config", "token_env"], "environment_variable": "FIXTURE_CALENDAR_TOKEN"}],
        "mcp_configuration": {"mcpServers": {"fixture-calendar": {"command": "/fictional/installed/runtime/bin/python",
            "args": ["-I", "-B", "-m", "calendar_reader", "--config", "/fictional/installed/configuration/selected.json"]}}}})
    before = copy.deepcopy(installed)
    registered = {}
    setup.register_tools(SimpleNamespace(profile_name="connectors",
        register_tool=lambda name, callback, **kwargs: registered.update({name: callback})))
    proposal = await registered["inspect_toolkit_installation"]({"job_id": JOB, "connection_id": "calendar", "name": "Calendar"})
    launch = before["result"]["mcp_configuration"]["mcpServers"]["fixture-calendar"]
    assert proposal["connection"]["command"] == launch["command"] and proposal["connection"]["args"] == launch["args"]
    assert proposal["connection"]["env"] == {"FIXTURE_CALENDAR_TOKEN": "${FIXTURE_CALENDAR_TOKEN}"}
    assert proposal["provenance"]["job_id"] == JOB and proposal["provenance"]["portability"] == "current_host_only"
    assert not {"authorized", "authenticated", "attached", "grants", "ready"}.intersection(proposal)
    blueprint = starter_blueprint()
    classes = copy.deepcopy(blueprint.agent_classes)
    blueprint.connections.append(Connection.model_validate(proposal["connection"]))
    assert blueprint.agent_classes == classes and all(not agent.tools.mcp for agent in blueprint.agent_classes)
    assert installed == before and connection.effects == 1
    assert connection.calls[-1] == ("setup_job", {"job_id": JOB})
    await setup.shutdown()


@pytest.mark.asyncio
async def test_shutdown_waits_for_owned_close_and_refuses_new_dispatch_without_relaunch(host):
    setup = ToolkitSetup()
    await setup.start()
    connection = host.instances[0]
    connection.close_release = asyncio.Event()
    shutdown = asyncio.create_task(setup.shutdown())
    await connection.close_entered.wait()
    assert not shutdown.done() and setup.status()["available"] is False
    waiting = asyncio.create_task(setup.call("setup_state", {}))
    await asyncio.sleep(0)
    assert not waiting.done()
    connection.close_release.set()
    await shutdown
    with pytest.raises(ToolkitSetupUnavailable):
        await waiting
    assert connection.closed == 1 and setup.connection is None and not setup.schemas
    assert len(host.instances) == 1
    await setup.shutdown()
    assert connection.closed == 1


@pytest.mark.asyncio
async def test_failed_close_retains_owner_for_explicit_cleanup_without_new_connection(host):
    setup = ToolkitSetup()
    await setup.start()
    connection = host.instances[0]
    connection.close_error = RuntimeError("CLOSE_FAILED")
    with pytest.raises(RuntimeError, match="CLOSE_FAILED"):
        await setup.shutdown()
    assert setup.connection is connection and setup.status()["available"] is False
    await setup.start()
    assert len(host.instances) == 1
    connection.close_error = None
    await setup.shutdown()
    assert connection.closed == 2 and setup.connection is None


@pytest.mark.asyncio
@pytest.mark.parametrize("preserve", [None, False, True], ids=["default", "explicit-false", "explicit-true"])
async def test_production_mcp_client_preserves_structured_errors_only_when_explicitly_enabled(monkeypatch, preserve):
    import mcp
    import mcp.client.stdio
    from mcp.types import CallToolResult, TextContent, Tool
    from launchkit.templates_v2.mcp_connections import MCPConnection

    owner_error = CallToolResult(isError=True,
        content=[TextContent(type="text", text="Configuration refused.")],
        structuredContent={"error": "invalid_configuration", "diagnostic": {
            "code": "channel_list_conflict", "field_path": ["operations"],
            "message": "Keep the channel restriction and deselect list_conversations_page."}})
    owner_success = CallToolResult(isError=False,
        content=[TextContent(type="text", text="Configuration checked.")],
        structuredContent={"validation_id": "b" * 32, "digest": "c" * 64})
    replies = [owner_error, owner_success]
    calls, contexts = [], []

    @asynccontextmanager
    async def fictional_stdio(parameters):
        assert parameters.command == "/fictional/never-executed/python"
        contexts.append(asyncio.current_task())
        try:
            yield object(), object()
        finally:
            contexts.append(asyncio.current_task())

    class FictionalSession:
        def __init__(self, *args, **kwargs):
            pass

        async def __aenter__(self):
            contexts.append(asyncio.current_task())
            return self

        async def __aexit__(self, *args):
            contexts.append(asyncio.current_task())

        async def initialize(self):
            pass

        async def list_tools(self, **kwargs):
            return SimpleNamespace(tools=[Tool(name="setup_check", inputSchema=object_schema())], nextCursor=None)

        async def call_tool(self, name, arguments, **kwargs):
            calls.append((name, copy.deepcopy(arguments)))
            return replies.pop(0)

    monkeypatch.setattr(mcp.client.stdio, "stdio_client", fictional_stdio)
    monkeypatch.setattr(mcp, "ClientSession", FictionalSession)
    config = {"name": "Fictional setup", "transport": "stdio", "command": "/fictional/never-executed/python", "args": []}
    if preserve is not None:
        config["preserve_tool_errors"] = preserve
    connection = MCPConnection(config)
    try:
        await connection.start()
        if preserve is True:
            received = await connection.call("setup_check", {})
            assert received == owner_error.model_dump(mode="json", by_alias=True, exclude_none=True)
            assert received["isError"] is True
            received["structuredContent"]["diagnostic"]["field_path"].append("changed")
            assert owner_error.structuredContent["diagnostic"]["field_path"] == ["operations"]
        else:
            with pytest.raises(RuntimeError, match="MCP tool setup_check reported an error"):
                await connection.call("setup_check", {})
        success = await connection.call("setup_check", {})
        assert success == owner_success.model_dump(mode="json", by_alias=True, exclude_none=True)
        assert success["isError"] is False
        assert calls == [("setup_check", {}), ("setup_check", {})] and not replies
    finally:
        await connection.close()
    assert connection.task is None
    assert len(contexts) == 4 and len(set(contexts)) == 1 and contexts[0] is not asyncio.current_task()


def installed_job(host, *, family="agno", profile="fixture-reader"):
    return {"id": JOB, "kind": "install", "status": "succeeded", "result": {
        "scope": "observed_software_installation_only", "family": family, "profile": profile,
        "configuration_sha256": "c" * 64, "contents_sha256": host.binding["contents_sha256"],
        "credential_environment_references": [], "mcp_configuration": {"mcpServers": {"fixture-reader": {
            "command": "/fictional/installed/runtime/bin/python", "args": ["-I", "-B", "-m", "fixture_reader"]}}}}}


def selected_read():
    return {"family": "agno", "profile": "fixture-reader", "operations": ["read_article"], "values": {}}


@pytest.mark.asyncio
async def test_matching_checked_agno_selection_evidences_only_selected_documented_names_and_cleanup_clears(host):
    setup = ToolkitSetup()
    await setup.start()
    connection = host.instances[0]
    arguments = selected_read()
    await setup.call("setup_check", arguments)
    arguments["operations"].append("write_admin")
    connection.jobs["selected"] = installed_job(host)
    result = await setup.handoff({"job_id": JOB, "connection_id": "reader", "name": "Reader"})
    assert result["connection"]["tool_evidence"]["tool_names"] == ["read_article"]
    assert "write_admin" not in result["connection"]["tool_evidence"]["tool_names"]
    assert result["provenance"]["tool_evidence_scope"] == "selected_maintained_operation_metadata"
    assert connection.calls[-2:] == [("setup_job", {"job_id": JOB}),
                                    ("setup_help", {"family": "agno", "profile": "fixture-reader"})]
    assert not {"grants", "authenticated", "authorized", "attached"}.intersection(result)
    assert setup.checked_selections == {("agno", "fixture-reader", "c" * 64): ["read_article"]}
    await setup.shutdown()
    assert not setup.checked_selections and connection.closed == 1


@pytest.mark.asyncio
@pytest.mark.parametrize("family", ["agno", "services"])
@pytest.mark.parametrize("mismatch", ["check-family", "check-profile", "check-digest", "check-contents",
    "installed-family", "installed-profile", "installed-digest", "installed-contents", "no-check"])
async def test_selection_binding_mismatch_leaves_handoff_without_inferred_tool_evidence(host, mismatch, family):
    setup = ToolkitSetup()
    await setup.start()
    connection = host.instances[0]
    arguments = selected_read()
    arguments['family'] = family
    other_family = 'services' if family == 'agno' else 'agno'
    checked = {"scope": "configuration_only", "family": family, "profile": "fixture-reader",
               "configuration_sha256": "c" * 64, "contents_sha256": host.binding["contents_sha256"]}
    if mismatch == "check-family": checked["family"] = other_family
    elif mismatch == "check-profile": checked["profile"] = "other-reader"
    elif mismatch == "check-digest": checked["configuration_sha256"] = "not-a-digest"
    elif mismatch == "check-contents": checked["contents_sha256"] = "d" * 64
    connection.responses["setup_check"] = {"structuredContent": checked}
    if mismatch != "no-check":
        await setup.call("setup_check", arguments)
    job = installed_job(host, family=family)
    if mismatch == "installed-family": job["result"]["family"] = other_family
    elif mismatch == "installed-profile": job["result"]["profile"] = "other-reader"
    elif mismatch == "installed-digest": job["result"]["configuration_sha256"] = "d" * 64
    elif mismatch == "installed-contents": job["result"]["contents_sha256"] = "d" * 64
    connection.jobs["selected"] = job
    result = await setup.handoff({"job_id": JOB, "connection_id": "reader", "name": "Reader"})
    assert result["connection"].get("tool_evidence") is None
    assert "tool_evidence_scope" not in result["provenance"]
    assert all(name != "setup_help" for name, _ in connection.calls)
    await setup.shutdown()


@pytest.mark.asyncio
async def test_checked_services_operation_ids_do_not_become_unproven_bare_mcp_tool_names(host):
    setup = ToolkitSetup()
    await setup.start()
    connection = host.instances[0]
    await setup.call("setup_check", {"family": "services", "profile": "fixture-reader", "operations": ["read_article"], "values": {}})
    connection.jobs["selected"] = installed_job(host, family="services")
    result = await setup.handoff({"job_id": JOB, "connection_id": "reader", "name": "Reader"})
    assert result["connection"].get("tool_evidence") is None
    assert connection.calls[-1] == ("setup_help", {"family": "services", "profile": "fixture-reader"})
    await setup.shutdown()


@pytest.mark.asyncio
@pytest.mark.parametrize("profile,operation,tool", [
    ("gmail", "create_draft_message", "gmail_create_draft_message"),
    ("slack", "read_interval", "slack_read_interval"),
    ("gmail", "create_draft_message", "publisher.review-draft"),
])
async def test_checked_services_selection_uses_only_literal_publisher_mapping(host, profile, operation, tool):
    setup = ToolkitSetup()
    await setup.start()
    connection = host.instances[0]
    arguments = {"family": "services", "profile": profile, "operations": [operation], "values": {}}
    await setup.call("setup_check", arguments)
    arguments["operations"].append("send_message")
    connection.jobs["selected"] = installed_job(host, family="services", profile=profile)
    help_value = {"family": "services", "profile": profile, "operations": [
        {"id": operation, "tool": tool, "effect": "write" if profile == "gmail" else "read"},
        {"id": "send_message", "tool": "publisher_send_message", "effect": "write"}]}
    connection.responses["setup_help"] = {"structuredContent": help_value}
    result = await setup.handoff({"job_id": JOB, "connection_id": "service", "name": "Service"})
    evidence = result["connection"]["tool_evidence"]
    assert evidence["tool_names"] == [tool]
    document = json.dumps(help_value, sort_keys=True, separators=(',', ':'), ensure_ascii=True).encode()
    assert evidence["sha256"] == hashlib.sha256(document).hexdigest()
    assert evidence["source_url"] == f"toolkit-setup:services/{profile}?contents_sha256={host.binding['contents_sha256']}"
    assert result["provenance"]["tool_evidence_scope"] == "selected_maintained_operation_metadata"
    assert result["connection"]["command"] == "/fictional/installed/runtime/bin/python"
    assert result["connection"]["args"] == ["-I", "-B", "-m", "fixture_reader"]
    assert not {"grants", "authenticated", "authorized", "attached"}.intersection(result)
    assert connection.effects == 0
    await setup.shutdown()


@pytest.mark.asyncio
@pytest.mark.parametrize("failure", ["missing-selected-tool", "partial-selected-tools", "null-tool", "invalid-tool",
    "oversize-tool", "duplicate-tool", "duplicate-id", "malformed-unselected-tool"])
async def test_services_mapping_refuses_missing_or_invalid_publisher_names(host, failure):
    setup = ToolkitSetup()
    await setup.start()
    connection = host.instances[0]
    arguments = {"family": "services", "profile": "gmail", "operations": ["create_draft_message"], "values": {}}
    if failure == "partial-selected-tools":
        arguments["operations"].append("get_profile")
    await setup.call("setup_check", arguments)
    connection.jobs["selected"] = installed_job(host, family="services", profile="gmail")
    rows = [{"id": "create_draft_message", "tool": "gmail_create_draft_message", "effect": "write"},
            {"id": "get_profile", "tool": "gmail_get_profile", "effect": "read"}]
    if failure == "missing-selected-tool": rows[0].pop("tool")
    elif failure == "partial-selected-tools": rows[1].pop("tool")
    elif failure == "null-tool": rows[0]["tool"] = None
    elif failure == "invalid-tool": rows[0]["tool"] = "untrusted/name"
    elif failure == "oversize-tool": rows[0]["tool"] = "x" * 129
    elif failure == "duplicate-tool": rows[1]["tool"] = rows[0]["tool"]
    elif failure == "duplicate-id": rows[1]["id"] = rows[0]["id"]
    else: rows[1]["tool"] = 7
    connection.responses["setup_help"] = {"structuredContent": {
        "family": "services", "profile": "gmail", "operations": rows}}
    if failure in {"missing-selected-tool", "partial-selected-tools"}:
        result = await setup.handoff({"job_id": JOB, "connection_id": "mail", "name": "Mail"})
        assert result["connection"].get("tool_evidence") is None
        assert "tool_evidence_scope" not in result["provenance"]
    else:
        with pytest.raises(ToolkitSetupUnavailable, match="operation names could not be confirmed"):
            await setup.handoff({"job_id": JOB, "connection_id": "mail", "name": "Mail"})
    assert connection.effects == 0
    await setup.shutdown()


@pytest.mark.asyncio
@pytest.mark.parametrize("failure", ["wrong-family", "wrong-profile", "missing-selected-name", "malformed-operations", "owner-refusal"])
async def test_matching_selection_requires_same_owner_help_before_returning_evidence(host, failure):
    setup = ToolkitSetup()
    await setup.start()
    connection = host.instances[0]
    await setup.call("setup_check", selected_read())
    connection.jobs["selected"] = installed_job(host)
    help_value = {"family": "agno", "profile": "fixture-reader", "operations": [{"id": "read_article"}]}
    if failure == "wrong-family": help_value["family"] = "services"
    elif failure == "wrong-profile": help_value["profile"] = "other-reader"
    elif failure == "missing-selected-name": help_value["operations"] = [{"id": "write_admin"}]
    elif failure == "malformed-operations": help_value["operations"] = [{"id": 7}]
    connection.responses["setup_help"] = ({"isError": True, "structuredContent": {"error": "profile_unavailable"}}
        if failure == "owner-refusal" else {"structuredContent": help_value})
    with pytest.raises(ToolkitSetupUnavailable, match="operation names could not be confirmed"):
        await setup.handoff({"job_id": JOB, "connection_id": "reader", "name": "Reader"})
    assert connection.effects == 0 and connection.closed == 0
    assert connection.calls[-1] == ("setup_help", {"family": "agno", "profile": "fixture-reader"})
    await setup.shutdown()


@pytest.mark.asyncio
async def test_shutdown_pages_all_owned_jobs_stops_only_running_once_and_waits_exact_settlement(host):
    setup = ToolkitSetup()
    await setup.start()
    connection = host.instances[0]
    first, terminal, last = "1" * 32, "2" * 32, "3" * 32
    connection.jobs = {identifier: {"id": identifier, "status": state} for identifier, state in
                       [(first, "running"), (terminal, "succeeded"), (last, "running")]}
    connection.responses["setup_jobs"] = lambda args: {"structuredContent": {
        "jobs": ([{"id": first, "status": "running"}, {"id": terminal, "status": "succeeded"}]
                 if args["before"] is None else [{"id": last, "status": "running"}]),
        "next_before": terminal if args["before"] is None else None}}
    polls = {first: 0, last: 0}
    def settlement(args):
        identifier = args["job_id"]
        polls[identifier] += 1
        state = "running" if identifier == first and polls[identifier] == 1 else "unconfirmed" if identifier == first else "succeeded"
        return {"structuredContent": {"job": {"id": identifier, "status": state}}}
    connection.responses["setup_job"] = settlement
    before = len(connection.calls)
    await setup.shutdown()
    calls = connection.calls[before:]
    assert calls[:2] == [("setup_jobs", {"before": None}), ("setup_jobs", {"before": terminal})]
    assert [args["job_id"] for name, args in calls if name == "setup_stop"] == [first, last]
    assert polls == {first: 2, last: 1}
    assert connection.closed == 1 and setup.connection is None and not setup.schemas


@pytest.mark.asyncio
@pytest.mark.parametrize("inventory", [
    {"jobs": []},
    {"jobs": [{"id": "invented", "status": "running"}], "next_before": None},
    {"jobs": [{"id": JOB, "status": "running"}, {"id": JOB, "status": "running"}], "next_before": None},
    {"jobs": [{"id": JOB, "status": "ready"}], "next_before": None},
    {"jobs": [], "next_before": JOB},
])
async def test_uncertain_shutdown_inventory_retains_exact_connection_schemas_and_selection(host, inventory):
    setup = ToolkitSetup()
    await setup.start()
    connection = host.instances[0]
    await setup.call("setup_check", selected_read())
    schemas, selected = copy.deepcopy(setup.schemas), copy.deepcopy(setup.checked_selections)
    connection.responses["setup_jobs"] = {"structuredContent": inventory}
    with pytest.raises(ToolkitSetupUnavailable, match="settlement is unconfirmed"):
        await setup.shutdown()
    assert setup.connection is connection and connection.closed == 0
    assert setup.schemas == schemas and setup.checked_selections == selected and not setup.status()["available"]
    assert all(name != "setup_stop" for name, _ in connection.calls)
    await setup.start()
    assert len(host.instances) == 1
    connection.responses.pop("setup_jobs")
    await setup.shutdown()
    assert connection.closed == 1 and not setup.checked_selections


@pytest.mark.asyncio
@pytest.mark.parametrize("failure", ["lost-stop-reply", "wrong-settlement-job", "settlement-owner-error", "timeout"])
async def test_shutdown_uncertain_stop_or_settlement_never_closes_or_replaces_owner(host, monkeypatch, failure):
    setup = ToolkitSetup()
    await setup.start()
    connection = host.instances[0]
    connection.jobs["active"] = {"id": JOB, "status": "running"}
    if failure == "lost-stop-reply":
        def lost_reply(args):
            raise RuntimeError("PRIVATE_LOST_STOP_CANARY")
        connection.responses["setup_stop"] = lost_reply
    elif failure == "wrong-settlement-job":
        connection.responses["setup_job"] = {"structuredContent": {"job": {"id": "f" * 32, "status": "succeeded"}}}
    elif failure == "settlement-owner-error":
        connection.responses["setup_job"] = {"isError": True, "structuredContent": {"error": "unknown_job"}}
    else:
        timeout = asyncio.timeout
        def short_deadline(seconds):
            assert seconds == 30
            return timeout(.01)
        monkeypatch.setattr(module, "asyncio", SimpleNamespace(timeout=short_deadline, sleep=asyncio.sleep))
        connection.responses["setup_job"] = {"structuredContent": {"job": {"id": JOB, "status": "running"}}}
    with pytest.raises(ToolkitSetupUnavailable, match="settlement is unconfirmed") as caught:
        await setup.shutdown()
    assert "PRIVATE_LOST_STOP_CANARY" not in str(caught.value)
    assert setup.connection is connection and connection.closed == 0 and setup.schemas
    assert sum(name == "setup_stop" for name, _ in connection.calls) == 1
    await setup.start()
    assert len(host.instances) == 1
    connection.jobs["active"]["status"] = "unconfirmed"
    connection.responses.clear()
    await setup.shutdown()
    assert connection.closed == 1 and sum(name == "setup_stop" for name, _ in connection.calls) == 1
