"""Host startup recovery with fake SDK owners and no native processes."""
import ast
import asyncio
import copy
import hashlib
import json
import sqlite3
from contextlib import contextmanager
from pathlib import Path
import sys
import tomllib
from types import SimpleNamespace

from fastapi import HTTPException
import pytest

from launchkit import architect as module
from launchkit.architect import ArchitectService, ArchitectUnavailable
from meerkat_meta import IdentityBootstrapMode
from meerkat_meta.errors import RpcError, TransportError
from launchkit.blueprint import starter_blueprint
from launchkit.guide_models import starter_state
from launchkit.guide_store import GuideStore
from launchkit.guide_timeline import GuideTimeline

SERVER_SOURCE = Path(__import__("launchkit").__path__[0]) / "server.py"


class FakeRuntime:
    rust_http_base_url = "http://native.invalid"

    def __init__(self, *, ready=True, broken=0, error=None):
        self.ready = ready
        self.broken = broken
        self.error = error
        self.shutdown_calls = 0
        self.status_calls = 0
        self.wait_calls = []
        self.wait_entered = asyncio.Event()
        self.wait_release = None
        self.wait_error = None
        self.wait_status = None
        self.wait_outcomes = []
        self.member_calls = []
        self.members = {}
        self.member_error = None
        self.identity_calls = []
        self.identity_sessions = {}

    async def status(self, identity):
        self.identity_calls.append(identity)
        return SimpleNamespace(identity=identity,
            session_id=self.identity_sessions.get(identity, "mock-session-" + identity))

    def mob_handle(self):
        return self

    async def member_status(self, identity):
        self.member_calls.append(identity)
        if self.member_error is not None:
            raise self.member_error
        return self.members.get(identity, SimpleNamespace(
            status="active", error=None, current_session_id="mock-session-" + identity))

    async def wait_identity_bootstrap(self, **kwargs):
        self.wait_calls.append(kwargs)
        self.wait_entered.set()
        if self.wait_release is not None:
            await self.wait_release.wait()
        if self.wait_outcomes:
            outcome = self.wait_outcomes.pop(0)
            if isinstance(outcome, BaseException):
                raise outcome
            if outcome is not None:
                return outcome
        if self.wait_error:
            raise self.wait_error
        return self.wait_status or SimpleNamespace(complete=True, ready=self.ready,
            counts=SimpleNamespace(broken=self.broken), timed_out=False,
            startup_ready=self.ready if kwargs["target"] == "startup_ready" else None, error=None)

    async def identity_bootstrap_status(self):
        self.status_calls += 1
        return SimpleNamespace(complete=True, ready=self.ready,
                               counts=SimpleNamespace(broken=self.broken), error=self.error)

    async def shutdown(self):
        self.shutdown_calls += 1


@pytest.fixture
def startup(tmp_path, monkeypatch):
    from launchkit import bootstrap
    from launchkit.templates import storage_setup

    controller = SimpleNamespace(outcomes=[], builds=0, builders=0, available=True,
                                 entered=asyncio.Event(), release=None, lock_events=[], modes=[], mob_configs=[], callback_runner=None,
                                 gateway_init_timeouts=[], gateways=[], spec_updates=[])

    class FakeBuilder:
        def __getattr__(self, name):
            if name == "build":
                raise AttributeError(name)
            # Composition stays in ArchitectService. This double only accepts
            # the chain and controls its one native bootstrap boundary.
            def chained(*args, **kwargs):
                if name in {"roster", "topology_provider", "session_service"}:
                    setattr(self, name + "_callback", args[0])
                if name == "mob_inline":
                    controller.mob_configs.append(args[0])
                if name == "declare_spec_update":
                    controller.spec_updates.append(args[0])
                if name == "gateway":
                    controller.gateways.append(args[0])
                return self
            return chained

        async def build(self):
            controller.builds += 1
            controller.entered.set()
            if controller.release is not None:
                await controller.release.wait()
            if controller.callback_runner is not None:
                await controller.callback_runner(self)
            outcome = controller.outcomes.pop(0)
            if isinstance(outcome, BaseException):
                raise outcome
            return outcome

        def identity_bootstrap_mode(self, mode):
            controller.modes.append(mode.to_dict())
            return self

        def gateway_init_timeout(self, seconds):
            controller.gateway_init_timeouts.append(seconds)
            return self

    def builder():
        controller.builders += 1
        return FakeBuilder()

    @contextmanager
    def project_lock(root):
        (root / ".mobkit").mkdir(mode=0o700, exist_ok=True)
        controller.lock_events.append("acquired")
        try:
            yield
        finally:
            controller.lock_events.append("released")

    async def prepare_storage(root, build):
        # These tests exercise host lifecycle, not native store preparation.
        return None

    def official_mock_binary(destination, name):
        assert destination.is_relative_to(tmp_path)
        destination.parent.mkdir(parents=True, mode=0o700, exist_ok=True)
        destination.write_bytes(("Mock official bytes for " + name).encode())
        destination.chmod(0o700)
        prefix = {"rpc_gateway": "mobkit-rpc-gateway", "mobkit_gateway": "mobkit-gateway"}[name]
        receipt = destination.with_name(name + ".receipt.json")
        receipt.write_text(json.dumps({"version": bootstrap.VERSION, "core_version": bootstrap.CORE_VERSION,
            "archive": f"{prefix}-{bootstrap.VERSION}-{bootstrap.platform_target()}.tar.gz",
            "release_url": bootstrap.RELEASE_URL,
            "archive_sha256": "a" * 64, "binary_sha256": hashlib.sha256(destination.read_bytes()).hexdigest()}))
        receipt.chmod(0o600)
        # Receipt validation is real; these test payloads are never executed.
        return destination

    monkeypatch.setitem(sys.modules, "meerkat_meta",
                        SimpleNamespace(Meta=SimpleNamespace(builder=builder),
                                        IdentityBootstrapMode=IdentityBootstrapMode))
    monkeypatch.setitem(sys.modules, "meerkat_meta.identity_first_models", SimpleNamespace(
        DurableAgentSpec=lambda **kwargs: SimpleNamespace(**kwargs),
        ManagedPeerEdge=lambda **kwargs: SimpleNamespace(**kwargs)))
    monkeypatch.setattr(module, "planner_capability", lambda: {
        "available": controller.available, "provider": "openai", "reason": "Fake provider availability"})
    monkeypatch.setenv("OPENAI_API_KEY", "isolated-startup-test-provider")
    rpc_gateway = bootstrap.package_tool_path(tmp_path / "owned-source", "rpc_gateway")
    official_mock_binary(rpc_gateway, "rpc_gateway")
    from launchkit import guide_sdk
    from launchkit.connect_installer_caller import ExactFile
    selected = bootstrap.SelectedGuideRuntime(
        ExactFile(rpc_gateway, hashlib.sha256(rpc_gateway.read_bytes()).hexdigest()),
        guide_sdk.SELECTED_VERSION, guide_sdk.SELECTED_VERSION, "a" * 40, "b" * 40)
    monkeypatch.setattr(bootstrap, "_selected_guide_runtime", selected)
    monkeypatch.setattr(module, "__file__", str(tmp_path / "owned-source/launchkit/architect.py"))
    monkeypatch.setattr(storage_setup, "project_lock", project_lock)
    monkeypatch.setattr(storage_setup, "prepare_storage", prepare_storage)
    monkeypatch.delenv("MOBKIT_ARCHITECT_MODEL", raising=False)
    monkeypatch.delenv("MOBKIT_VISUALIZER_MODEL", raising=False)
    monkeypatch.delenv("MOBKIT_VISUALIZER_REASONING", raising=False)
    monkeypatch.delenv("MOBKIT_GATEWAY_BIN", raising=False)
    monkeypatch.delenv("MOBKIT_RPC_GATEWAY_BIN", raising=False)
    # ArchitectService sets this existing mitigation. Isolate that mutation to
    # this test even when other tests in the process use a different setting.
    monkeypatch.setenv("MOBKIT_IDENTITY_RESTORE_CONCURRENCY", "1")
    service = ArchitectService(tmp_path / "owned-service")
    service.set_context(starter_blueprint().model_dump(mode="json"))
    return service, controller


def start_endpoint(service):
    """Run the current route body without constructing app-owned private stores."""
    source = SERVER_SOURCE
    parsed = ast.parse(source.read_text(), filename=str(source))
    declaration = next(node for node in parsed.body
                       if isinstance(node, ast.AsyncFunctionDef) and node.name == "architect_start")
    declaration = copy.deepcopy(declaration)
    declaration.decorator_list = []
    namespace = {
        "architect": service, "ArchitectUnavailable": ArchitectUnavailable,
        "HTTPException": HTTPException, "BlueprintBody": object,
        "blueprint_or_error": lambda value: SimpleNamespace(model_dump=lambda **kwargs: value),
        "LOG": SimpleNamespace(warning=lambda *args: None),
    }
    exec(compile(ast.Module(body=[declaration], type_ignores=[]), str(source), "exec"), namespace)
    return namespace["architect_start"]


def autonomous_setup_profile(monkeypatch):
    select_target = module._setup_bootstrap_target
    def select_with_autonomous_profile(definition):
        assert select_target(definition) == "materialized"
        autonomous = definition.replace('runtime_mode = "turn_driven"',
                                        'runtime_mode = "autonomous_host"', 1)
        assert tomllib.loads(autonomous)["profiles"]["architect"]["runtime_mode"] == "autonomous_host"
        return select_target(autonomous)
    monkeypatch.setattr(module, "_setup_bootstrap_target", select_with_autonomous_profile)


@pytest.mark.parametrize("autonomous", [None, "architect", "connectors", "reviewer", "visualizer"])
def test_setup_barrier_follows_every_explicit_compiler_profile_mode(autonomous):
    definition = "\n".join(f'[profiles.{name}]\nruntime_mode = "{mode}"' for name, mode in
        ((name, "autonomous_host" if name == autonomous else "turn_driven")
         for name in ("architect", "connectors", "reviewer", "visualizer")))
    assert module._setup_bootstrap_target(definition) == ("startup_ready" if autonomous else "materialized")
    for unknown in ('[mob]\nid = "setup"', '[profiles.architect]\nmodel = "gpt-6.1-sol"',
                    '[profiles.architect]\nruntime_mode = "unknown"'):
        assert module._setup_bootstrap_target(unknown) == "startup_ready"


@pytest.mark.asyncio
async def test_turn_driven_setup_waits_for_native_materialization_once_and_reuses_its_owner(startup):
    service, controller = startup
    runtime = FakeRuntime()
    runtime.wait_release = asyncio.Event()
    controller.outcomes = [runtime]
    starting = asyncio.create_task(service.start())
    await runtime.wait_entered.wait()
    assert runtime.wait_calls == [{"timeout": 180, "target": "materialized"}]
    assert service.runtime is runtime and service.status()["status"] == "starting"
    profiles = tomllib.loads(controller.mob_configs[0])["profiles"]
    assert set(profiles) == {"architect", "connectors", "reviewer", "visualizer"}
    assert all(profile["runtime_mode"] == "turn_driven" for profile in profiles.values())
    runtime.wait_release.set()
    assert (await starting)["status"] == "ready"
    assert service.runtime is runtime and runtime.shutdown_calls == 0
    assert runtime.status_calls == 1, "A fresh native status must verify the materialized owner"
    assert (await service.start())["status"] == "ready"
    assert controller.builds == controller.builders == len(runtime.wait_calls) == 1
    await service.shutdown()
    assert runtime.shutdown_calls == 1


@pytest.mark.asyncio
@pytest.mark.parametrize("startup_ready", [True, False, None])
async def test_an_autonomous_setup_profile_keeps_the_native_startup_ready_gate(startup, monkeypatch, startup_ready):
    service, controller = startup
    autonomous_setup_profile(monkeypatch)
    runtime = FakeRuntime()
    runtime.wait_status = SimpleNamespace(complete=True, ready=True, counts=SimpleNamespace(broken=0),
        timed_out=False, startup_ready=startup_ready, error=None)
    controller.outcomes = [runtime]
    if startup_ready is True:
        assert (await service.start())["status"] == "ready"
        assert service.runtime is runtime and runtime.shutdown_calls == 0
        await service.shutdown()
    else:
        with pytest.raises(RuntimeError, match="did not become ready"):
            await service.start()
        assert service.runtime is None and service.status()["startup_phase"] == "identity_warmup"
    assert runtime.wait_calls == [{"timeout": 180, "target": "startup_ready"}]
    assert controller.builds == runtime.shutdown_calls == 1


@pytest.mark.asyncio
@pytest.mark.parametrize("configured", [None, "low", "medium", "high"])
async def test_animation_reasoning_matches_exact_captured_profile_and_preserves_other_roles(startup, monkeypatch, configured):
    service, controller = startup
    if configured is not None:
        monkeypatch.setenv("MOBKIT_VISUALIZER_REASONING", configured)
    expected = configured or "medium"
    controller.outcomes = [FakeRuntime()]
    async def change_environment_after_composition(builder):
        monkeypatch.setenv("MOBKIT_VISUALIZER_REASONING", "low" if expected == "high" else "high")
    controller.callback_runner = change_environment_after_composition
    status = await service.start()
    configured_mob = tomllib.loads(controller.mob_configs[0])
    assert controller.builders == controller.builds == 1
    assert "models" not in configured_mob
    for name in ("architect", "connectors", "reviewer", "visualizer"):
        profile = configured_mob["profiles"][name]
        effort = expected if name == "visualizer" else "low"
        assert profile["model"] == "gpt-6.1-sol" and profile["provider"] == "openai"
        assert profile["provider_params"] == {"provider_tag": {
            "provider": "open_ai", "reasoning_effort": effort, "supports_reasoning_override": True}}
        assert profile["resume_overrides"] == ["model", "provider", "provider_params"]
        if name in {"architect", "visualizer"}:
            assert profile["auto_compact_threshold"] == 200000
        else:
            assert "auto_compact_threshold" not in profile
        assert status["profile_models"][name] == {"model": profile["model"], "reasoning": effort}
    assert (await service.start())["profile_models"] == status["profile_models"]
    assert service.status()["profile_models"] == status["profile_models"]
    assert controller.builds == 1
    await service.shutdown()


@pytest.mark.asyncio
@pytest.mark.parametrize("architect_model, visualizer_model, compacted", [
    ("gpt-6.1-sol", "gpt-6-sol", {"architect"}),
    ("gpt-6-sol", "gpt-6.1-sol", {"visualizer"}),
    ("gpt-6-sol", "gpt-6-sol", set()),
])
async def test_native_compaction_policy_follows_each_selected_model_without_changing_other_profiles(
        startup, monkeypatch, architect_model, visualizer_model, compacted):
    service, controller = startup
    monkeypatch.setenv("MOBKIT_ARCHITECT_MODEL", architect_model)
    monkeypatch.setenv("MOBKIT_VISUALIZER_MODEL", visualizer_model)
    controller.outcomes = [FakeRuntime()]
    status = await service.start()
    profiles = tomllib.loads(controller.mob_configs[0])["profiles"]
    for name, profile in profiles.items():
        expected_model = visualizer_model if name == "visualizer" else architect_model
        assert profile["model"] == expected_model
        assert status["profile_models"][name]["model"] == expected_model
        if name in compacted:
            assert profile["auto_compact_threshold"] == 200000
        else:
            assert "auto_compact_threshold" not in profile
        assert profile["resume_overrides"] == ["model", "provider", "provider_params"]
    # An already owned runtime is reused; this policy does not reset its owner.
    owned = service.runtime
    assert await service.start() == status
    assert service.runtime is owned and owned.shutdown_calls == 0
    assert controller.builders == controller.builds == len(controller.mob_configs) == 1
    await service.shutdown()


@pytest.mark.asyncio
@pytest.mark.parametrize("expected_revision", [None, 2])
async def test_ordinary_restore_keeps_canonical_tuning_but_explicit_upgrade_uses_current_defaults(
        startup, expected_revision):
    service, controller = startup
    thresholds = {"architect": None, "connectors": 65000, "reviewer": None, "visualizer": 200000}
    database = module.setup_runtime_root(service.root) / ".mobkit/state/event_log.sqlite3"
    database.parent.mkdir(parents=True)
    runtime_root = module.setup_runtime_root(service.root)
    for directory in (service.root, runtime_root, runtime_root / ".mobkit", database.parent):
        directory.chmod(0o700)
    from launchkit import bootstrap
    from launchkit.templates import storage_setup
    storage_setup.write_private(runtime_root / "deployment.json", {"slug": "launch-architect", "mobkitVersion": bootstrap.VERSION})
    storage_setup.write_private(runtime_root / ".mobkit/launch-storage.json",
        {"format": 1, "project": "launch-architect", "version": bootstrap.VERSION, "phase": "ready"})
    with sqlite3.connect(database) as connection:
        connection.execute("CREATE TABLE mob_specs (mob_id TEXT PRIMARY KEY, spec_json BLOB NOT NULL)")
        spec = {"revision": 2, "definition": {"id": "launch-architect", "profiles": {
            name: {"provider": "openai", "auto_compact_threshold": threshold}
            for name, threshold in thresholds.items()}}}
        connection.execute("INSERT INTO mob_specs VALUES (?, ?)", ("launch-architect", json.dumps(spec).encode()))
    before = database.read_bytes()
    controller.outcomes = [FakeRuntime()]
    assert (await service.start(expected_revision=expected_revision))["status"] == "ready"
    profiles = tomllib.loads(controller.mob_configs[0])["profiles"]
    for name, profile in profiles.items():
        expected = thresholds[name] if expected_revision is None else (200000 if name in {"architect", "visualizer"} else None)
        assert profile.get("auto_compact_threshold") == expected
        assert profile["model"] == "gpt-6.1-sol" and profile["provider"] == "openai"
    assert controller.spec_updates == ([] if expected_revision is None else [expected_revision])
    assert database.read_bytes() == before
    assert controller.builds == 1
    await service.shutdown()


@pytest.mark.asyncio
@pytest.mark.parametrize("generation", ["lent", "current"])
async def test_lent_setup_provider_yields_to_the_selected_model_host(startup, monkeypatch, generation):
    # On Meta 0.9.4 a fresh setup generation borrows the previous one's saved
    # settings. A lent provider must not bind it against the selected host route.
    service, controller = startup
    from launchkit import bootstrap, guide_sdk
    monkeypatch.setattr(guide_sdk, "SELECTED_VERSION", "0.9.4")
    routed = bootstrap.SelectedGuideRuntime(bootstrap.selected_guide_runtime().gateway, "0.9.4", "0.9.4",
        "a" * 40, "b" * 40, {"provider": "openai", "model": "gpt-6.1-sol", "auth_binding": "fixture-host-route"})
    monkeypatch.setattr(bootstrap, "_selected_guide_runtime", routed)
    runtime_root = service.root / ("architect-runtime-meta-v1" if generation == "lent" else "architect-runtime-meta-v2")
    database = runtime_root / ".mobkit/state/event_log.sqlite3"
    database.parent.mkdir(parents=True)
    for directory in (service.root, runtime_root, runtime_root / ".mobkit", database.parent):
        directory.chmod(0o700)
    with sqlite3.connect(database) as connection:
        connection.execute("CREATE TABLE mob_specs (mob_id TEXT PRIMARY KEY, spec_json BLOB NOT NULL)")
        spec = {"revision": 1, "definition": {"id": "launch-architect", "profiles": {
            name: {"provider": "anthropic", "auto_compact_threshold": 65000}
            for name in ("architect", "connectors", "reviewer", "visualizer")}}}
        connection.execute("INSERT INTO mob_specs VALUES (?, ?)", ("launch-architect", json.dumps(spec).encode()))
    before = database.read_bytes()
    controller.outcomes = [FakeRuntime()]
    if generation == "current":
        # The fresh generation's own definition still refuses a conflicting host.
        with pytest.raises(ArchitectUnavailable, match="differs from this selected model host"):
            await service.start()
        assert controller.builds == 0
    else:
        assert (await service.start())["status"] == "ready"
        profiles = tomllib.loads(controller.mob_configs[0])["profiles"]
        assert {profile["provider"] for profile in profiles.values()} == {"openai"}
        assert all(profile.get("auto_compact_threshold") != 65000 for profile in profiles.values())
        assert controller.builds == 1
        await service.shutdown()
    assert database.read_bytes() == before


@pytest.mark.asyncio
@pytest.mark.parametrize("configured", ["", "LOW", " medium", "low\n", "none", "private-canary"])
async def test_invalid_animation_reasoning_refuses_before_any_startup_work(startup, monkeypatch, configured):
    service, controller = startup
    monkeypatch.setenv("MOBKIT_VISUALIZER_REASONING", configured)
    def forbidden():
        raise AssertionError("Invalid configuration must precede provider or model work.")
    monkeypatch.setattr(module, "planner_capability", forbidden)
    with pytest.raises(ArchitectUnavailable) as raised:
        await service.start()
    assert str(raised.value) == "Set MOBKIT_VISUALIZER_REASONING to low, medium or high."
    assert not service.root.exists()
    assert controller.builders == controller.builds == 0 and controller.lock_events == []
    assert controller.mob_configs == [] and service.runtime is None
    assert service.state == "stopped" and not service.start_lock.locked()


@pytest.mark.asyncio
async def test_initial_automatic_start_ready_fast_path_and_clean_stopped_restart(startup):
    service, controller = startup
    first, second = FakeRuntime(), FakeRuntime()
    controller.outcomes = [first, second]
    initial = await service.start()
    assert initial["status"] == "ready" and initial["startup_retry_required"] is False
    assert await service.start() == initial
    assert controller.builds == controller.builders == 1
    assert controller.modes == [{"mode": "lazy_with_background_warm", "concurrency": 1}]
    assert first.wait_calls == [{"timeout": 180, "target": "materialized"}]

    await service.shutdown()
    assert first.shutdown_calls == 1
    assert service.state == "stopped" and service.runtime is None
    assert service.status()["startup_retry_required"] is False
    assert (await service.start())["status"] == "ready"
    assert service.runtime is second and controller.builds == 2
    await service.shutdown()


@pytest.mark.asyncio
async def test_failed_start_blocks_already_queued_automatic_owners_and_retains_history(startup, tmp_path):
    service, controller = startup
    controller.release = asyncio.Event()
    controller.outcomes = [RuntimeError("Fake native bootstrap failure")]
    first = asyncio.create_task(service.start())
    await controller.entered.wait()

    store = GuideStore(tmp_path / "owned-guides")
    saved = store.create(starter_state({}))
    saved["messages"].append({"id": "saved-user", "role": "user", "text": "Keep my proposed team"})
    saved = store.save(saved, saved["version"])
    saved_before = copy.deepcopy(saved)
    timeline = GuideTimeline(store, service)
    service.visualizer.pending = {"key": "owned-animation"}
    animation_before = copy.deepcopy(service.visualizer.pending)
    callers = [asyncio.create_task(service.start()) for _ in range(3)]
    callers += [
        asyncio.create_task(service.message("Follow up", service.context)),
        asyncio.create_task(timeline.read(saved["id"])),
        asyncio.create_task(service.visualizer.observe_generation("owned-animation")),
    ]
    await asyncio.sleep(0)
    controller.release.set()
    results = await asyncio.gather(first, *callers, return_exceptions=True)

    assert isinstance(results[0], RuntimeError)
    assert all(isinstance(result, ArchitectUnavailable) for result in results[1:])
    assert all("Automatic startup is paused" in str(result) for result in results[1:])
    assert controller.builds == controller.builders == 1
    assert controller.lock_events == ["acquired", "released"]
    assert service.state == "failed" and service.runtime is None
    assert service.status()["startup_retry_required"] is True
    assert not service.start_lock.locked() and not service.message_lock.locked()
    assert service.lock_context is None
    assert store.get(saved["id"]) == saved_before
    assert service.visualizer.pending == animation_before
    await service.shutdown()
    assert service.state == "failed" and service.status()["startup_retry_required"] is True


@pytest.mark.asyncio
async def test_explicit_start_endpoint_retries_once_and_restores_automatic_ready_behavior(startup):
    service, controller = startup
    recovered = FakeRuntime()
    controller.outcomes = [RuntimeError("Fake first bootstrap failure"), recovered]
    with pytest.raises(RuntimeError):
        await service.start()
    with pytest.raises(ArchitectUnavailable):
        await service.start()

    observed_keywords = []
    original_start = service.start
    async def observed_start(**kwargs):
        observed_keywords.append(kwargs)
        return await original_start(**kwargs)
    service.start = observed_start
    result = await start_endpoint(service)()
    assert observed_keywords == [{"retry_failed": True}]
    assert result["status"] == "ready" and result["startup_retry_required"] is False
    assert service.last_error is None
    assert controller.builds == 2 and service.runtime is recovered
    await original_start()
    assert controller.builds == 2
    await service.shutdown()


@pytest.mark.asyncio
async def test_failed_explicit_retry_stays_latched_and_returns_safe_existing_error(startup):
    service, controller = startup
    controller.outcomes = [RuntimeError("Fake first failure"), RuntimeError("private-error-canary")]
    with pytest.raises(RuntimeError):
        await service.start()
    with pytest.raises(HTTPException) as error:
        await start_endpoint(service)()
    assert error.value.status_code == 502
    assert "private-error-canary" not in str(error.value.detail)
    assert service.status()["startup_retry_required"] is True
    for _ in range(3):
        with pytest.raises(ArchitectUnavailable):
            await service.start()
    assert controller.builds == 2
    assert controller.lock_events == ["acquired", "released", "acquired", "released"]


@pytest.mark.asyncio
async def test_returned_but_unready_runtime_is_cleaned_and_latched(startup):
    service, controller = startup
    unready = FakeRuntime(ready=False, broken=1)
    controller.outcomes = [unready]
    with pytest.raises(RuntimeError, match="did not become ready"):
        await service.message("First automatic request", service.context)
    assert unready.shutdown_calls == 1
    assert service.runtime is None and service.state == "failed"
    with pytest.raises(ArchitectUnavailable):
        await service.start()
    assert controller.builds == 1 and service.status()["startup_retry_required"] is True


@pytest.mark.asyncio
async def test_missing_initial_provider_does_not_turn_a_precondition_into_failed_bootstrap(startup):
    service, controller = startup
    controller.available = False
    with pytest.raises(ArchitectUnavailable, match="Fake provider availability"):
        await service.start()
    assert controller.builds == 0 and service.state == "stopped"
    assert service.status()["startup_retry_required"] is False
    controller.available = True
    controller.outcomes = [FakeRuntime()]
    assert (await service.start())["status"] == "ready"
    assert controller.builds == 1
    await service.shutdown()


@pytest.mark.asyncio
async def test_explicit_start_preserves_existing_context_conflict_before_retry(startup):
    service, controller = startup
    await service.message_lock.acquire()
    try:
        with pytest.raises(HTTPException) as error:
            await start_endpoint(service)(SimpleNamespace(blueprint=service.context))
        assert error.value.status_code == 409
        assert controller.builds == controller.builders == 0
    finally:
        service.message_lock.release()


@pytest.mark.asyncio
@pytest.mark.parametrize(("error", "kind", "code"), [
    (TransportError("meta/init failed: persistent transport: timeout after 60.0s waiting for response"),
     "startup_timeout", None),
    (TransportError("gateway process died during bootstrap: persistent transport: timeout after 60s waiting for response"),
     "startup_timeout", None),
    (TransportError("meta/init failed: private-canary timed out reading /private/canary"),
     "startup_failure", None),
    (TransportError("meta/init failed: persistent transport: timeout after 60.0s waiting for response private-canary"),
     "startup_failure", None),
    (RuntimeError("meta/init failed: persistent transport: timeout after 60.0s waiting for response"),
     "startup_failure", None),
    (RpcError(-32014, "private-canary", request_id="private-canary", data={"secret": "private-canary"}),
     "native_refusal", -32014),
    (RpcError(True, "private-canary"), "startup_failure", None),
    (RpcError("private-canary", "private-canary"), "startup_failure", None),
    (RuntimeError("private-canary"), "startup_failure", None),
    (TimeoutError("private-canary"), "startup_timeout", None),
])
async def test_failed_native_build_has_bounded_typed_diagnostics(startup, error, kind, code):
    service, controller = startup
    controller.outcomes = [error]
    with pytest.raises(type(error)):
        await service.start()
    status = service.status()
    assert status["status"] == "failed"
    assert status["startup_phase"] == "native_bootstrap"
    assert status["startup_error_kind"] == kind
    assert status["startup_error_code"] == code
    assert "private-canary" not in json.dumps(status)
    assert controller.builds == controller.builders == 1
    assert service.runtime is None and service.lock_context is None
    with pytest.raises(ArchitectUnavailable):
        await service.start()
    assert service.status() == status and controller.builds == 1


@pytest.mark.asyncio
@pytest.mark.parametrize("failure", ["state_directory", "runtime_directory", "console_write", "console_directory"])
async def test_directory_preparation_failure_latches_before_native_and_recovers_once(startup, monkeypatch, failure):
    service, controller = startup
    runtime_root = module.setup_runtime_root(service.root)
    console_path = runtime_root / "console.toml"
    original_mkdir, original_write = Path.mkdir, Path.write_text
    context_before = copy.deepcopy(service.context)
    console_failure = failure.startswith("console_")
    if failure == "console_directory":
        runtime_root.mkdir(parents=True, mode=0o700)
        console_path.mkdir()
    elif not console_failure:
        failed_directory = {
            "state_directory": service.root,
            "runtime_directory": runtime_root,
        }[failure]

    with monkeypatch.context() as fault:
        if failure == "console_write":
            def denied_write(path, *args, **kwargs):
                if path == console_path:
                    raise PermissionError("private-directory-canary")
                return original_write(path, *args, **kwargs)
            fault.setattr(Path, "write_text", denied_write)
        elif not console_failure:
            def denied_mkdir(path, *args, **kwargs):
                if path == failed_directory:
                    raise PermissionError("private-directory-canary")
                return original_mkdir(path, *args, **kwargs)
            fault.setattr(Path, "mkdir", denied_mkdir)
        with pytest.raises(OSError):
            await service.start()

    status = service.status()
    assert status["status"] == "failed"
    assert status["startup_retry_required"] is True
    assert status["startup_phase"] == ("gateway_preparation" if console_failure else "storage_preparation")
    assert status["startup_error_kind"] == ("startup_failure" if console_failure else "storage_preparation")
    assert status["startup_error_code"] is None and status["detail"]
    assert "private-directory-canary" not in json.dumps(status)
    assert str(service.root) not in json.dumps(status)
    before_locks = ["acquired", "released"] if console_failure else []
    assert controller.builders == controller.builds == 0 and controller.lock_events == before_locks
    assert service.runtime is None and service.lock_context is None
    assert not service.start_lock.locked()
    assert service.context == context_before
    if failure == "console_directory":
        assert console_path.is_dir() and not list(console_path.iterdir())

    # Fixing the local cause does not authorize an automatic replacement owner.
    if failure == "console_directory":
        console_path.rmdir()
    for _ in range(2):
        with pytest.raises(ArchitectUnavailable):
            await service.start()
    assert controller.builders == controller.builds == 0
    assert service.status() == status

    recovered = FakeRuntime()
    controller.outcomes = [recovered]
    result = await start_endpoint(service)()
    assert result["status"] == "ready" and result["startup_retry_required"] is False
    assert result["startup_phase"] is None and result["startup_error_kind"] is None
    assert controller.builders == controller.builds == 1
    assert controller.lock_events == before_locks + ["acquired"]
    assert service.context == context_before
    await service.start()
    assert controller.builders == controller.builds == 1
    await service.shutdown()
    assert recovered.shutdown_calls == 1 and controller.lock_events == before_locks + ["acquired", "released"]


@pytest.mark.asyncio
async def test_storage_error_reports_phase_without_native_payload_or_claiming_timeout(startup, monkeypatch):
    from launchkit import runtime_dependencies
    service, controller = startup
    def failed_storage(*args, **kwargs):
        raise RpcError(-32014, "private-canary", data={"secret": "private-canary"})
    monkeypatch.setattr(runtime_dependencies, "_directory", failed_storage)
    with pytest.raises(RpcError):
        await service.start()
    status = service.status()
    assert status["startup_phase"] == status["startup_error_kind"] == "storage_preparation"
    assert status["startup_error_code"] is None
    assert controller.builds == 0 and service.lock_context is None
    assert "private-canary" not in json.dumps(status)


@pytest.mark.asyncio
async def test_preparation_error_does_not_invent_native_cause(startup, monkeypatch):
    service, controller = startup
    async def failed_preparation():
        raise RuntimeError("private-canary timeout /private/canary")
    monkeypatch.setattr(service.toolkit_catalog, "start", failed_preparation)
    with pytest.raises(RuntimeError):
        await service.start()
    status = service.status()
    assert status["startup_phase"] == "gateway_preparation"
    assert status["startup_error_kind"] == "startup_failure"
    assert status["startup_error_code"] is None
    assert controller.builds == 0 and controller.lock_events == ["acquired", "released"]
    assert "private-canary" not in json.dumps(status)


@pytest.mark.asyncio
async def test_readiness_failure_records_diagnostic_before_runtime_cleanup(startup):
    service, controller = startup
    runtime = FakeRuntime(ready=False, broken=1)
    runtime.wait_status = SimpleNamespace(complete=True, ready=True,
        counts=SimpleNamespace(broken=0), timed_out=False, startup_ready=True, error=None)
    captured = []
    async def shutdown():
        captured.append(service.status())
        runtime.shutdown_calls += 1
    runtime.shutdown = shutdown
    controller.outcomes = [runtime]
    with pytest.raises(RuntimeError, match="did not become ready"):
        await service.start()
    assert len(captured) == runtime.shutdown_calls == 1
    assert captured[0]["status"] == "failed"
    assert captured[0]["startup_phase"] == "readiness_check"
    assert captured[0]["startup_error_kind"] == "startup_failure"
    assert service.runtime is None and service.lock_context is None


@pytest.mark.asyncio
async def test_retry_clears_old_diagnostic_while_pending_and_when_ready(startup):
    service, controller = startup
    controller.outcomes = [RpcError(-32014, "private-canary"), FakeRuntime()]
    with pytest.raises(RpcError):
        await service.start()
    controller.entered.clear()
    controller.release = asyncio.Event()
    retry = asyncio.create_task(service.start(retry_failed=True))
    await controller.entered.wait()
    pending = service.status()
    assert pending["status"] == "starting" and pending["startup_phase"] == "native_bootstrap"
    assert pending["startup_error_kind"] is pending["startup_error_code"] is None
    assert service.last_error is None
    controller.release.set()
    ready = await retry
    assert ready["status"] == "ready"
    assert ready["startup_phase"] is ready["startup_error_kind"] is ready["startup_error_code"] is None
    assert controller.builds == 2
    await service.shutdown()


@pytest.mark.asyncio
@pytest.mark.parametrize(("changes", "kind"), [
    ({"timed_out": True}, "startup_timeout"),
    ({"timed_out": None}, "startup_failure"),
    ({"error": "private-canary"}, "startup_failure"),
    ({"complete": False}, "startup_failure"),
    ({"ready": False}, "startup_failure"),
    ({"counts": SimpleNamespace(broken=1)}, "startup_failure"),
])
async def test_warmup_requires_full_ready_result_and_cleans_local_owner(startup, changes, kind):
    service, controller = startup
    runtime = FakeRuntime()
    status = dict(complete=True, ready=True, counts=SimpleNamespace(broken=0),
                  timed_out=False, startup_ready=True, error=None)
    status.update(changes)
    runtime.wait_status = SimpleNamespace(**status)
    controller.outcomes = [runtime]
    with pytest.raises((RuntimeError, TimeoutError)):
        await service.start()
    assert runtime.wait_calls == [{"timeout": 180, "target": "materialized"}]
    assert runtime.shutdown_calls == 1 and service.runtime is None
    assert service.lock_context is None and service.status()["startup_retry_required"] is True
    assert service.status()["startup_phase"] == "identity_warmup"
    assert service.status()["startup_error_kind"] == kind
    assert "private-canary" not in json.dumps(service.status())
    with pytest.raises(ArchitectUnavailable):
        await service.start()
    assert controller.builds == 1


@pytest.mark.asyncio
async def test_warmup_typed_timeout_cleans_owner_and_does_not_project_message(startup):
    service, controller = startup
    runtime = FakeRuntime()
    runtime.wait_error = TimeoutError("private-canary /private/canary")
    controller.outcomes = [runtime]
    with pytest.raises(TimeoutError):
        await service.start()
    assert runtime.shutdown_calls == 1 and service.runtime is None
    assert service.status()["startup_error_kind"] == "startup_timeout"
    assert service.status()["startup_phase"] == "identity_warmup"
    assert "private-canary" not in json.dumps(service.status())


@pytest.mark.asyncio
async def test_fresh_meta_uses_one_owner_and_native_materialization_barrier(startup, monkeypatch):
    from launchkit.templates import storage_setup
    service, controller = startup
    final = FakeRuntime()
    controller.outcomes = [final]
    async def initialize(*args):
        pytest.fail("Fresh Meta startup must not construct a predecessor storage initializer")
    monkeypatch.setattr(storage_setup, "prepare_storage", initialize)
    result = await service.start()
    assert result["status"] == "ready" and service.runtime is final
    assert final.shutdown_calls == 0
    assert controller.gateway_init_timeouts == [300]
    assert final.wait_calls == [{"timeout": 180, "target": "materialized"}]
    assert controller.modes == [{"mode": "lazy_with_background_warm", "concurrency": 1}]
    assert controller.builds == 1
    await service.shutdown()
    assert final.shutdown_calls == 1


@pytest.mark.asyncio
async def test_cancelled_local_warmup_cleans_once_releases_lock_and_latches_failure(startup):
    service, controller = startup
    runtime = FakeRuntime()
    runtime.wait_release = asyncio.Event()
    controller.outcomes = [runtime]
    task = asyncio.create_task(service.start())
    await runtime.wait_entered.wait()
    assert service.runtime is runtime and service.startup_phase == "identity_warmup"
    task.cancel()
    with pytest.raises(asyncio.CancelledError):
        await task
    assert runtime.shutdown_calls == 1
    assert service.runtime is None and service.lock_context is None
    assert controller.lock_events == ["acquired", "released"]
    assert not service.start_lock.locked()
    assert service.status()["status"] == "failed"
    assert service.status()["startup_error_kind"] == "startup_failure"
    assert service.status()["startup_retry_required"] is True
    with pytest.raises(ArchitectUnavailable):
        await service.start()
    assert controller.builds == 1


@pytest.mark.asyncio
async def test_fresh_status_error_after_successful_wait_still_fails_readiness(startup):
    service, controller = startup
    runtime = FakeRuntime(error="private-canary")
    controller.outcomes = [runtime]
    with pytest.raises(RuntimeError, match="did not become ready"):
        await service.start()
    assert runtime.wait_calls == [{"timeout": 180, "target": "materialized"}]
    assert runtime.shutdown_calls == 1 and service.runtime is None
    assert service.status()["status"] == "failed"
    assert service.status()["startup_phase"] == "readiness_check"
    assert service.status()["startup_error_kind"] == "startup_failure"
    assert "private-canary" not in json.dumps(service.status())


@pytest.mark.asyncio
async def test_selected_guide_change_after_warmup_cleans_owner_and_latches_failure(startup, monkeypatch):
    from launchkit import bootstrap
    from launchkit.connect_installer_caller import CustodyError
    service, controller = startup
    runtime = FakeRuntime()
    controller.outcomes = [runtime]
    selected = bootstrap.selected_guide_runtime()
    original_status = runtime.status
    async def changed_after_warmup(identity):
        status = await original_status(identity)
        if identity == "visualizer:main":
            selected.gateway.path.write_bytes(b"Changed after owned native warmup")
        return status
    runtime.status = changed_after_warmup
    with pytest.raises(CustodyError):
        await service.start()
    assert runtime.wait_calls == [{"timeout": 180, "target": "materialized"}]
    assert runtime.shutdown_calls == 1
    assert service.runtime is None and service.lock_context is None
    assert controller.lock_events == ["acquired", "released"]
    assert not service.start_lock.locked()
    status = service.status()
    assert status["status"] == "failed" and status["startup_retry_required"] is True
    assert status["startup_phase"] == "identity_warmup"
    assert status["startup_error_kind"] == "startup_failure" and status["startup_error_code"] is None
    with pytest.raises(ArchitectUnavailable):
        await service.start()
    assert controller.builds == 1 and service.status() == status


CALLBACK_INITIAL = {"roster": "not_entered", "topology": "not_entered",
    "build_profiles": {name: "not_entered" for name in
        ("architect", "connectors", "reviewer", "visualizer")}}


SATURATED_OBSERVATION = "identity bootstrap readiness failed: mob lifecycle operation admission is still pending at observation_lane_saturated: member_status_observation"


def saturated_observation():
    return RpcError(-32000, SATURATED_OBSERVATION,
                    method="meta/wait_identity_bootstrap")


def fake_readiness_clock(monkeypatch):
    clock = SimpleNamespace(value=0, sleeps=[])
    async def sleep(delay):
        clock.sleeps.append(delay)
        clock.value += delay
    monkeypatch.setattr(module, "time", SimpleNamespace(monotonic=lambda: clock.value))
    monkeypatch.setattr(module, "asyncio", SimpleNamespace(**{**vars(asyncio), "sleep": sleep}))
    return clock


@pytest.mark.asyncio
async def test_materialization_does_not_hide_or_retry_a_startup_observation_error(startup, monkeypatch):
    service, controller = startup
    runtime = FakeRuntime()
    runtime.wait_error = saturated_observation()
    controller.outcomes = [runtime]
    clock = fake_readiness_clock(monkeypatch)
    with pytest.raises(RpcError) as failure:
        await service.start()
    assert failure.value is runtime.wait_error
    assert runtime.wait_calls == [{"timeout": 180, "target": "materialized"}]
    assert clock.sleeps == [] and runtime.status_calls == 0
    assert controller.builds == runtime.shutdown_calls == 1
    assert service.runtime is None and service.status()["status"] == "failed"


@pytest.mark.asyncio
async def test_saturated_readiness_observation_retries_same_native_owner_with_one_deadline(startup, monkeypatch):
    service, controller = startup
    autonomous_setup_profile(monkeypatch)
    runtime = FakeRuntime()
    runtime.wait_outcomes = [saturated_observation(), saturated_observation(), None]
    controller.outcomes = [runtime]
    clock = fake_readiness_clock(monkeypatch)
    assert (await service.start())["status"] == "ready"
    assert controller.builds == controller.builders == 1
    assert service.runtime is runtime and runtime.shutdown_calls == 0
    assert clock.sleeps == [0.25, 0.5]
    assert runtime.wait_calls == [
        {"timeout": 180, "target": "startup_ready"},
        {"timeout": 179.75, "target": "startup_ready"},
        {"timeout": 179.25, "target": "startup_ready"}]
    await service.shutdown()
    assert runtime.shutdown_calls == 1


@pytest.mark.asyncio
async def test_saturated_readiness_exhaustion_is_bounded_and_cleans_same_owner(startup, monkeypatch):
    service, controller = startup
    autonomous_setup_profile(monkeypatch)
    runtime = FakeRuntime()
    runtime.wait_error = saturated_observation()
    controller.outcomes = [runtime]
    clock = fake_readiness_clock(monkeypatch)
    with pytest.raises(TimeoutError):
        await service.start()
    assert clock.value == 180 and max(clock.sleeps) == 2
    assert len(runtime.wait_calls) == len(clock.sleeps)
    assert all(0 < call["timeout"] <= 180 for call in runtime.wait_calls)
    assert controller.builds == controller.builders == runtime.shutdown_calls == 1
    assert service.runtime is None and service.lock_context is None
    assert service.status()["startup_error_kind"] == "startup_timeout"
    assert SATURATED_OBSERVATION not in json.dumps(service.status())


@pytest.mark.asyncio
@pytest.mark.parametrize("error", [
    RpcError(-32001, SATURATED_OBSERVATION, method="meta/wait_identity_bootstrap"),
    RpcError(True, SATURATED_OBSERVATION, method="meta/wait_identity_bootstrap"),
    RpcError(-32000, SATURATED_OBSERVATION, method="mobkit/init"),
    RpcError(-32000, SATURATED_OBSERVATION),
    RpcError(-32000, "identity bootstrap readiness mapping is incomplete", method="meta/wait_identity_bootstrap"),
    RpcError(-32000, SATURATED_OBSERVATION + " private-canary", method="meta/wait_identity_bootstrap"),
    RuntimeError(SATURATED_OBSERVATION),
    TransportError(SATURATED_OBSERVATION),
])
async def test_other_readiness_failures_never_retry_or_project_native_payload(startup, monkeypatch, error):
    service, controller = startup
    runtime = FakeRuntime()
    runtime.wait_error = error
    controller.outcomes = [runtime]
    clock = fake_readiness_clock(monkeypatch)
    with pytest.raises(type(error)) as raised:
        await service.start()
    assert raised.value is error
    assert len(runtime.wait_calls) == 1 and clock.sleeps == []
    assert controller.builds == runtime.shutdown_calls == 1
    assert service.runtime is None and service.lock_context is None
    assert "private-canary" not in json.dumps(service.status())


@pytest.mark.asyncio
async def test_cancelling_saturated_readiness_backoff_cleans_without_another_wait(startup, monkeypatch):
    service, controller = startup
    autonomous_setup_profile(monkeypatch)
    runtime = FakeRuntime()
    runtime.wait_error = saturated_observation()
    controller.outcomes = [runtime]
    sleeping = asyncio.Event()
    async def sleep(delay):
        assert delay == 0.25
        sleeping.set()
        await asyncio.Event().wait()
    monkeypatch.setattr(module, "asyncio", SimpleNamespace(**{**vars(asyncio), "sleep": sleep}))
    task = asyncio.create_task(service.start())
    await asyncio.wait_for(sleeping.wait(), timeout=30)
    task.cancel()
    with pytest.raises(asyncio.CancelledError):
        await task
    assert len(runtime.wait_calls) == 1
    assert controller.builds == runtime.shutdown_calls == 1
    assert service.runtime is None and service.lock_context is None
    assert not service.start_lock.locked()


@pytest.mark.asyncio
async def test_actual_startup_callbacks_mark_entered_then_returned_without_context(startup, monkeypatch):
    service, controller = startup
    canary = "PRIVATE_CALLBACK_CONTEXT_SHOULD_NEVER_APPEAR"
    entered = []
    def spec(**kwargs):
        assert service.status()["startup_callback_progress"]["roster"] == "entered"
        entered.append("roster")
        return SimpleNamespace(**kwargs)
    def edge(**kwargs):
        assert service.status()["startup_callback_progress"]["topology"] == "entered"
        entered.append("topology")
        return SimpleNamespace(**kwargs)
    monkeypatch.setitem(sys.modules, "meerkat_meta.identity_first_models", SimpleNamespace(
        DurableAgentSpec=spec, ManagedPeerEdge=edge))
    async def callbacks(builder):
        await builder.roster_callback.roster({"private_context": canary})
        assert service.status()["startup_callback_progress"]["roster"] == "returned"
        for profile in CALLBACK_INITIAL["build_profiles"]:
            def register(*args, **kwargs):
                assert service.status()["startup_callback_progress"]["build_profiles"][profile] == "entered"
                entered.append(profile)
            await builder.session_service_callback.build_agent(SimpleNamespace(
                profile_name=profile, private_draft=canary, register_tool=register))
            assert service.status()["startup_callback_progress"]["build_profiles"][profile] == "returned"
        await builder.topology_provider_callback.compute_edges([canary], {"private_context": canary})
    controller.callback_runner = callbacks
    controller.outcomes = [FakeRuntime()]
    result = await service.start()
    progress = result["startup_callback_progress"]
    assert progress == {"roster": "returned", "topology": "returned",
        "build_profiles": dict.fromkeys(CALLBACK_INITIAL["build_profiles"], "returned")}
    assert set(entered) == {"roster", "topology", *CALLBACK_INITIAL["build_profiles"]}
    assert len(json.dumps(progress)) < 512 and canary not in json.dumps(result)
    # Public response mutation cannot contaminate the next observation.
    progress["build_profiles"]["architect"] = canary
    progress["roster"] = canary
    assert canary not in json.dumps(service.status())
    await service.shutdown()


@pytest.mark.asyncio
async def test_failed_callback_progress_is_retained_until_explicit_retry_then_cleared(startup):
    service, controller = startup
    async def failing(builder):
        await builder.roster_callback.roster({})
        def register(*args, **kwargs):
            raise RuntimeError("PRIVATE_REGISTRATION_FAILURE")
        await builder.session_service_callback.build_agent(SimpleNamespace(
            profile_name="architect", register_tool=register))
    controller.callback_runner = failing
    with pytest.raises(RuntimeError, match="PRIVATE_REGISTRATION_FAILURE"):
        await service.start()
    failed = service.status()["startup_callback_progress"]
    assert failed["roster"] == "returned" and failed["build_profiles"]["architect"] == "entered"
    assert failed["topology"] == "not_entered"
    assert "PRIVATE_REGISTRATION_FAILURE" not in json.dumps(service.status())
    with pytest.raises(ArchitectUnavailable):
        await service.start()
    assert service.status()["startup_callback_progress"] == failed
    controller.callback_runner = None
    controller.outcomes = [FakeRuntime()]
    controller.entered = asyncio.Event()
    controller.release = asyncio.Event()
    retry = asyncio.create_task(service.start(retry_failed=True))
    await controller.entered.wait()
    assert service.status()["startup_callback_progress"] == CALLBACK_INITIAL
    controller.release.set()
    assert (await retry)["status"] == "ready"
    # A mocked owner that calls no callbacks cannot manufacture callback returns.
    assert service.status()["startup_callback_progress"] == CALLBACK_INITIAL
    await service.shutdown()


@pytest.mark.asyncio
@pytest.mark.parametrize("unknown_profile", ["PRIVATE_UNKNOWN_PROFILE", ["PRIVATE_UNKNOWN_PROFILE"], None])
async def test_unknown_build_profile_never_becomes_public_callback_metadata(startup, unknown_profile):
    service, controller = startup
    async def callbacks(builder):
        await builder.session_service_callback.build_agent(SimpleNamespace(
            profile_name=unknown_profile, register_tool=lambda *args, **kwargs: None))
    controller.callback_runner = callbacks
    controller.outcomes = [FakeRuntime()]
    result = await service.start()
    assert result["startup_callback_progress"] == CALLBACK_INITIAL
    assert "PRIVATE_UNKNOWN_PROFILE" not in json.dumps(result)
    await service.shutdown()


@pytest.mark.asyncio
@pytest.mark.parametrize("callback", ["roster", "topology", "visualizer"])
async def test_callback_failure_never_reports_returned(startup, monkeypatch, callback):
    service, controller = startup
    def refused(*args, **kwargs):
        raise RuntimeError("PRIVATE_CALLBACK_FAILURE")
    if callback == "roster":
        monkeypatch.setattr(sys.modules["meerkat_meta.identity_first_models"], "DurableAgentSpec", refused)
    elif callback == "topology":
        monkeypatch.setattr(sys.modules["meerkat_meta.identity_first_models"], "ManagedPeerEdge", refused)
    else:
        monkeypatch.setattr(service.visualizer, "register_tools", refused)
    async def callbacks(builder):
        if callback == "roster":
            await builder.roster_callback.roster({})
        elif callback == "topology":
            await builder.topology_provider_callback.compute_edges([], {})
        else:
            await builder.session_service_callback.build_agent(SimpleNamespace(profile_name="visualizer"))
    controller.callback_runner = callbacks
    with pytest.raises(RuntimeError, match="PRIVATE_CALLBACK_FAILURE"):
        await service.start()
    progress = service.status()["startup_callback_progress"]
    assert (progress["build_profiles"]["visualizer"] if callback == "visualizer" else progress[callback]) == "entered"
    assert "returned" not in json.dumps(progress)
    assert "PRIVATE_CALLBACK_FAILURE" not in json.dumps(service.status())
    await service.shutdown()
