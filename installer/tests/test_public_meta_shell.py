"""A shell-started public Meta package runs on the Meta SDK and never imports MobKit."""
import asyncio
import importlib.util
import json
from pathlib import Path
import sys
from types import ModuleType, SimpleNamespace

import pytest

from launchkit.blueprint import starter_blueprint
from launchkit.compiler import generate_package
from launchkit.deployment_sources import compile_native_source, read_package_source
from launchkit.public_meta_runtime import VERSION
from test_deployment_native_team import accepted_guide

MOBKIT_MODULES = ("meerkat_mobkit", "meerkat_mobkit.builder", "meerkat_mobkit.errors",
    "meerkat_mobkit.identity_first_models")


def load_host(monkeypatch, root):
    templates = Path(__file__).parents[1] / "launchkit"
    monkeypatch.syspath_prepend(str(templates))
    monkeypatch.syspath_prepend(str(templates / "templates"))
    monkeypatch.syspath_prepend(str(templates / "templates_v2"))
    definition = importlib.util.spec_from_file_location("public_meta_shell_host", templates / "templates_v2/app.py")
    module = importlib.util.module_from_spec(definition)
    definition.loader.exec_module(module)
    monkeypatch.setattr(module, "ROOT", root)
    return module


def package(root, version_package=True):
    plan = starter_blueprint().model_dump(mode="json")
    files = {item.path: item for item in generate_package(plan, public_runtime=version_package)}
    for name in ("deployment.json", "config/mob.toml", "config/console.toml", "config/connections.json"):
        path = root / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(files[name].content)
    return json.loads(files["deployment.json"].content)


class RpcError(Exception):
    pass


def meta_sdk(monkeypatch, events, configs):
    """Fake meerkat_meta modules; every MobKit import fails."""
    for name in MOBKIT_MODULES:
        monkeypatch.setitem(sys.modules, name, None)

    class MetaBuilderConfig:
        def __init__(self, **options):
            configs.append(options)
            self.__dict__.update(options)

    class MetaRuntime:
        rust_http_base_url = "http://127.0.0.1:61337"

        def __init__(self, config):
            self.config = config
            events.append("runtime")

        def on_schedule_fire(self, name, handler):
            events.append("schedule:" + name)

        async def connect(self):
            events.append("connect")

        async def identity_bootstrap_status(self):
            return SimpleNamespace(complete=True, ready=True, counts=SimpleNamespace(broken=0), identities={})

        def mob_handle(self):
            async def query_mob_events(query):
                events.append("definition_events")
                return [SimpleNamespace(kind="mob_created", data={"definition": {}, "type": "mob_created"})]
            return SimpleNamespace(query_mob_events=query_mob_events)

        async def shutdown(self):
            events.append("shutdown")

    root = ModuleType("meerkat_meta")
    builder, runtime, errors = ModuleType("meerkat_meta.builder"), ModuleType("meerkat_meta.runtime"), ModuleType("meerkat_meta.errors")
    models = ModuleType("meerkat_meta.identity_first_models")
    builder.MetaBuilderConfig = MetaBuilderConfig
    runtime.MetaRuntime = MetaRuntime
    errors.RpcError = RpcError
    models.DurableAgentSpec = models.ManagedPeerEdge = lambda **kwargs: SimpleNamespace(**kwargs)
    root.builder, root.runtime, root.errors, root.identity_first_models = builder, runtime, errors, models
    root.MetaRuntime = MetaRuntime
    for name, value in (("meerkat_meta", root), ("meerkat_meta.builder", builder), ("meerkat_meta.runtime", runtime),
            ("meerkat_meta.errors", errors), ("meerkat_meta.identity_first_models", models)):
        monkeypatch.setitem(sys.modules, name, value)


@pytest.fixture
def shell(tmp_path, monkeypatch):
    module = load_host(monkeypatch, tmp_path)
    spec = package(tmp_path)
    (tmp_path / ".bin").mkdir()
    (tmp_path / ".bin/rpc_gateway").write_bytes(b"gateway fixture, never executed")
    events, configs = [], []
    meta_sdk(monkeypatch, events, configs)

    class Tools:
        def __init__(self, *args):
            pass
        def for_agent(self, agent):
            return []
        async def close(self):
            pass

    class Pool:
        def __init__(self, spec):
            pass
        def for_class(self, item):
            return []
        async def start(self):
            pass
        async def close(self):
            pass

    async def prepare_storage(root, factory):
        events.append("prepare_storage")

    monkeypatch.setattr(module, "ConnectorTools", Tools)
    monkeypatch.setattr(module, "MCPPool", Pool)
    monkeypatch.setitem(sys.modules, "storage_setup", SimpleNamespace(prepare_storage=prepare_storage))
    monkeypatch.setenv("OPENAI_API_KEY", "isolated-test-value")
    monkeypatch.delenv("MOBKIT_RPC_GATEWAY_BIN", raising=False)
    monkeypatch.delenv("MOBKIT_PORT", raising=False)
    monkeypatch.delenv("META_IDENTITY_RESTORE_CONCURRENCY", raising=False)
    monkeypatch.delenv("MOBKIT_IDENTITY_RESTORE_CONCURRENCY", raising=False)
    return SimpleNamespace(module=module, spec=spec, root=tmp_path, events=events, configs=configs)


def arguments(**changes):
    return SimpleNamespace(**{"connections_check": False, "activate_revision": None, "definition_revision": False,
        "list_flows": False, "run_flow": None, "smoke": False, **changes})


async def test_shell_route_runs_a_meta_package_on_the_meta_sdk(shell, monkeypatch, capsys):
    import os
    monkeypatch.setattr(asyncio.get_running_loop(), "add_signal_handler", lambda *args: None)
    assert shell.spec["mobkitVersion"] == VERSION
    await shell.module.run(shell.spec, arguments(definition_revision=True))
    assert shell.events == ["prepare_storage", "runtime", "connect", "definition_events", "shutdown"]
    [config] = shell.configs
    assert config["gateway_bin"] == str(shell.root / ".bin/rpc_gateway")
    assert config["persistent_state"] == str(shell.root / ".mobkit/state")
    assert config["mob_config_path"] == str(shell.root / "config/mob.toml")
    assert config["http_listen"] == f"127.0.0.1:{shell.spec['port']}"
    # Meta refuses every console request when app auth is required without a
    # signing key; a shell package's loopback console is explicitly open.
    assert config["console_require_app_auth"] is False and config["auth_config"] is None
    assert os.environ.get("META_IDENTITY_RESTORE_CONCURRENCY") == "1"
    assert "MOBKIT_IDENTITY_RESTORE_CONCURRENCY" not in os.environ
    assert json.loads(capsys.readouterr().out.strip().splitlines()[-1])["definition_revision"] == 1


async def test_meta_package_with_an_authenticated_console_requires_app_auth(shell, monkeypatch):
    # A package that declares the local-password Console keeps app auth on the
    # Meta route, with the front's own native auth configuration.
    monkeypatch.setattr(asyncio.get_running_loop(), "add_signal_handler", lambda *args: None)
    application = shell.root / "application"
    application.mkdir(exist_ok=True)
    (application / "__init__.py").write_text("class Integration:\n    pass\n\n\ndef build(context):\n    return Integration()\n")
    (application / "console.json").write_text(json.dumps({"authentication": "local-password"}))
    (application / "configuration.json").write_text(json.dumps({"fields": [
        {"key": "CONSOLE_USER_ID", "label": "Console user", "secret": False, "required": True},
        {"key": "CONSOLE_LOGIN_PASSWORD", "label": "Console password", "secret": True, "required": True,
         "min_length": 12}]}))
    monkeypatch.setenv("CONSOLE_USER_ID", "local-user")
    monkeypatch.setenv("CONSOLE_LOGIN_PASSWORD", "local-password-123")
    fronts = []

    class Front:
        def __init__(self, root, port, user, password, on_input, *, runtime_namespace):
            self.native_auth = {"fixture": "front auth configuration", "namespace": runtime_namespace}
            self.closed = False
            fronts.append(self)

        async def close(self):
            self.closed = True

        def __getattr__(self, name):
            # The definition read never starts or serves the Console front.
            raise AssertionError("The definition read must not use the Console front: " + name)

    monkeypatch.setattr(shell.module, "ConsoleFront", Front)
    await shell.module.run(shell.spec, arguments(definition_revision=True))
    [front] = fronts
    [config] = shell.configs
    assert config["console_require_app_auth"] is True and config["auth_config"] is front.native_auth
    assert front.native_auth["namespace"] == "meta" and front.closed


async def test_meta_package_refuses_revision_activation_before_any_runtime(shell, monkeypatch):
    monkeypatch.setattr(asyncio.get_running_loop(), "add_signal_handler", lambda *args: None)
    with pytest.raises(ValueError, match="not available for this Meta package"):
        await shell.module.run(shell.spec, arguments(activate_revision=2))
    assert shell.events == [] and shell.configs == []


def test_doctor_names_the_package_runtime(tmp_path, monkeypatch, capsys):
    module = load_host(monkeypatch, tmp_path)
    monkeypatch.setenv("OPENAI_API_KEY", "isolated-test-value")
    monkeypatch.delenv("MOBKIT_RPC_GATEWAY_BIN", raising=False)
    (tmp_path / ".bin").mkdir()
    for name in ("rpc_gateway", "meta_gateway"):
        (tmp_path / ".bin" / name).write_bytes(b"fixture")
    module.doctor(package(tmp_path))
    lines = capsys.readouterr().out.splitlines()
    assert "[ready] Meta SDK" in lines and "[ready] Storage maintenance tool" in lines
    assert not any("MobKit" in line for line in lines)
    legacy = package(tmp_path, version_package=False)
    (tmp_path / ".bin/meta_gateway").unlink()
    module.doctor(legacy)
    lines = capsys.readouterr().out.splitlines()
    assert any(line.endswith("] MobKit SDK") for line in lines)
    assert "[needs setup] Storage maintenance tool" in lines


def test_shell_runtime_follows_the_package_release_pin(tmp_path, monkeypatch):
    module = load_host(monkeypatch, tmp_path)
    assert [module.public_meta_package({"mobkitVersion": version}) for version in ("0.8.47", "0.9.4", "0.10.0")] \
        == [False, True, True]
    # Only a synthetic spec lacks the pin; it keeps the MobKit route it always had.
    assert module.public_meta_package({}) is False
    with pytest.raises(ValueError, match="invalid runtime version"):
        module.public_meta_package({"mobkitVersion": "next"})


async def test_meta_smoke_retries_a_saturated_lane_with_meta_errors(shell):
    replies = [RpcError("observation_lane_saturated"), "ready"]

    class Agent:
        async def send(self, message):
            return SimpleNamespace(completion_baseline="cursor")
        async def wait_for_completion(self, baseline, timeout):
            reply = replies.pop(0)
            if isinstance(reply, Exception):
                raise reply
            return reply

    assert await shell.module.request(Agent(), "status", timeout=5, meta=True) == "ready"


def test_native_team_source_compiles_for_the_public_meta_release():
    guide = accepted_guide()
    payload, source, _ = compile_native_source(guide.state, guide.state["version"])
    import io
    import zipfile
    with zipfile.ZipFile(io.BytesIO(payload)) as archive:
        files = {name: (archive.read(name), archive.getinfo(name).external_attr >> 16 & 0o777)
            for name in archive.namelist()}
    assert json.loads(files["deployment.json"][0])["mobkitVersion"] == VERSION
    assert files["requirements.sdk.txt"][0].decode().startswith(f"meerkat-meta=={VERSION} --hash=sha256:")
    assert b"meerkat-mobkit" not in files["requirements.txt"][0]
    helper = "launchkit/connect_member_class_requests.py"
    assert files[helper] == ((Path(__file__).parents[1] / helper).read_bytes(), 0o644)
    assert read_package_source(files) == source
    assert source["source_kind"] == "native-team"


def test_meta_package_readme_matches_the_shell_route():
    plan = starter_blueprint().model_dump(mode="json")
    meta = {item.path: item.content for item in generate_package(plan, public_runtime=True)}["README.md"]
    legacy = {item.path: item.content for item in generate_package(plan)}["README.md"]
    activation = "To activate a changed `config/mob.toml`, run `bash start.sh --activate-revision N`"
    carrier = "This host projects the canonical native profile skills onto each ordinary application-authored `send`"
    # The 0.8 shell host activates revisions and authors role instructions; its text stays.
    assert activation in legacy and carrier in legacy
    # The Meta shell host refuses activation before startup and adds no instruction carrier.
    assert activation not in meta and carrier not in meta
    assert "`bash start.sh --activate-revision N` refuses before anything starts" in meta
    assert "build the changed team in Forge and install it into a new folder" in meta
    assert "`bash start.sh --definition-revision` reads the revision from the native structural event ledger" in meta
    assert "adds no System content to application-authored `send` or keyed `dispatch` turns" in meta


def test_installed_gateway_above_512_mib_keeps_its_provenance(tmp_path):
    import hashlib
    from launchkit import deployment_worker
    destination = tmp_path / "team"
    (destination / ".bin").mkdir(parents=True)
    (destination / ".env").write_text("")
    binary = destination / ".bin/rpc_gateway"
    # Sparse: above the former 512 MiB bound, as the 0.9.5 gateway may be.
    size = 520 * 1024 * 1024
    with binary.open("wb") as handle:
        handle.truncate(size)
    binary.chmod(0o700)
    digest, zeros = hashlib.sha256(), bytes(1024 * 1024)
    for _ in range(size // len(zeros)):
        digest.update(zeros)
    receipt = destination / ".bin/rpc_gateway.receipt.json"
    receipt.write_text(json.dumps({"version": VERSION, "binary_sha256": digest.hexdigest()}))
    receipt.chmod(0o600)
    supervisor = deployment_worker.Supervisor.__new__(deployment_worker.Supervisor)
    supervisor.destination = destination
    supervisor._candidate_runtime_target = lambda files: None
    files = {"deployment.json": (json.dumps({"mobkitVersion": VERSION}).encode(), 0o644)}
    assert supervisor.native_binary_provenance(files) == {"native_binary_sha256": digest.hexdigest(),
        "native_binary_version": VERSION, "native_binary_source": "official-release"}
    with binary.open("r+b") as handle:
        handle.truncate(deployment_worker.MAX_NATIVE_BINARY_BYTES + 1)
    with pytest.raises(RuntimeError, match="not owned and bounded"):
        supervisor.native_binary_provenance(files)
