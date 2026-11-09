"""Fresh generated-host crossing controls, without native qualification.

The SDK specimens below check caller identity and sequencing only. They cannot
establish native finish, policy semantics, same-child adoption or Ready.
"""
import ast
import asyncio
from contextlib import asynccontextmanager
import hashlib
import importlib.util
import json
from pathlib import Path
import sys
import threading
from types import ModuleType, SimpleNamespace

import pytest

from launchkit import connect_application_startup as module
from launchkit.blueprint import starter_blueprint
from launchkit.compiler import generate_files, generate_package
from launchkit.connect_installer_caller import CustodyError, ExactFile
from test_application_callbacks import Fixture as CallbackFixture
from console_source_fixture import write_console_sources


def pin(path, content):
    path.write_bytes(content)
    path.chmod(0o700 if path.name == 'gateway' else 0o600)
    return ExactFile(path, hashlib.sha256(content).hexdigest())


class Fixture:
    def __init__(self, root, monkeypatch, *, accepted_source=None, project_lease=None):
        root.chmod(0o700)
        self.root, self.current = root, True
        self.app = SimpleNamespace(context=SimpleNamespace(root=root, runtime=None), dynamic=False,
            _application_callbacks=None, bind_schedule_runnables=self.bind)
        self.events = []
        self.callbacks = CallbackFixture(root, native=True)
        value = {**self.callbacks.value, 'deployment_root': str(root), 'hold_timeout_ms': 30_000,
            'startup': {'kind': 'initialize'},
            'application': {**self.callbacks.value['application'], 'realm': {
                'realm_id': 'selected-realm', 'context_root': str(root),
                'state_root': str(root / '.mobkit/state')}}}
        self.plan = pin(self.callbacks.path, json.dumps(value).encode())
        self.gateway = pin(root / 'gateway', b'not an executable trial\n')
        self.policy = pin(root / 'policy.json', b'{"original":true}\n')
        self.template = pin(root / 'template.json', b'{"original":true}\n')
        self.workspace = root / 'finalizer'
        self.workspace.mkdir(mode=0o700)
        self.lease = project_lease or SimpleNamespace(root=root, require_current=self.check)
        self.source = accepted_source if accepted_source is not None else SimpleNamespace(receipt={'destination': str(root)},
            require_owner_current=self.check, require_filesystem_current=self.check,
            require_current_async=self.check_source_async, require_current=self.check)
        self.owner = module.FreshApplicationStartup(project_lease=self.lease, accepted_source=self.source,
            gateway=self.gateway, preparation_plan=self.plan, source_policy=self.policy,
            activation_template=self.template, command=lambda *a, **kw: pytest.fail('unexpected command'),
            command_timeout=5, startup_timeout=60, provision_attempt_path=root / 'provision.json',
            working_directory=self.workspace, bundle_path=root / 'bundle', activation_path=root / 'activation.json')
        self.modules = {}
        for name in ('meerkat_meta', 'meerkat_meta.builder', 'meerkat_toolkit',
                     'meerkat_toolkit.application_runtime', 'meerkat_toolkit.host_control'):
            self.modules[name] = ModuleType(name)
            monkeypatch.setitem(sys.modules, name, self.modules[name])
        class Config:
            def __init__(self, **options):
                self.__dict__.update(options)
        class Artifact:
            def __init__(self, path, sha256):
                self.path, self.sha256 = path, sha256
        class Preparation:
            def __init__(self, plan, timeout):
                self.plan, self.timeout = plan, timeout
        fixture = self
        class Runtime:
            def __init__(self, config, preparation, finalizer):
                fixture.events.append('construct')
                self.config, self.preparation, self.finalizer = config, preparation, finalizer
            async def connect(self):
                fixture.events.append('connect')
                if fixture.fail_connect:
                    raise RuntimeError('SDK settlement failure specimen')
            async def shutdown(self):
                fixture.events.append('shutdown')
        self.fail_connect = False
        self.modules['meerkat_meta.builder'].MetaBuilderConfig = Config
        self.modules['meerkat_toolkit.application_runtime'].ToolkitApplicationRuntime = Runtime
        self.modules['meerkat_toolkit.host_control'].ApplicationPreparation = Preparation
        self.modules['meerkat_toolkit.host_control'].SelectedHostArtifact = Artifact

    def check(self):
        if not self.current:
            raise RuntimeError('lost project owner')

    async def check_source_async(self):
        self.check()

    def bind(self, runtime):
        assert self.app.context.runtime is runtime
        self.events.append('bind schedules')

    def config(self):
        return self.owner.builder_config(roster_provider=self.app, topology_provider=self.app,
            session_builder=object(), host_runnables=['actual-handler'],
            persistent_state=str(self.root / '.mobkit/state'))


@pytest.mark.asyncio
async def test_one_runtime_with_original_pins_and_bound_host_providers(tmp_path, monkeypatch):
    f = Fixture(tmp_path, monkeypatch)
    await f.owner.prepare(f.app)
    config = f.config()
    runtime = await f.owner.connect(config, f.app)
    assert runtime is f.owner.runtime is f.app.context.runtime
    assert f.events == ['construct', 'bind schedules', 'connect']
    assert config.roster_provider is f.app and config.topology_provider is f.app
    assert runtime.finalizer is f.owner.finalizer
    assert runtime.preparation.plan.path == str(f.plan.path)
    assert runtime.preparation.plan.sha256 == f.plan.sha256
    assert runtime.preparation.timeout == 30
    assert config.init_deadline == 60
    assert not any(getattr(config, key, None) for key in ('mob_config_path', 'application_tool_policies'))
    with pytest.raises(CustodyError, match='already entered'):
        await f.owner.connect(config, f.app)
    assert f.events == ['construct', 'bind schedules', 'connect']
    assert not (tmp_path / 'current').exists()
    await f.owner.shutdown(runtime)
    assert f.events[-1] == 'shutdown'
    with pytest.raises(CustodyError, match='own retained runtime'):
        await f.owner.shutdown(object())


@pytest.mark.asyncio
@pytest.mark.parametrize('changed', ['plan', 'gateway', 'owner', 'roster', 'callbacks', 'definition', 'deadline', 'state'])
async def test_changed_selection_never_constructs_runtime(tmp_path, monkeypatch, changed):
    f = Fixture(tmp_path, monkeypatch)
    await f.owner.prepare(f.app)
    config = f.config()
    if changed in ('plan', 'gateway'):
        getattr(f, changed).path.write_bytes(b'changed')
    elif changed == 'owner':
        f.current = False
    elif changed == 'roster':
        config.roster_provider = object()
    elif changed == 'callbacks':
        f.app._application_callbacks = object()
    elif changed == 'definition':
        config.mob_config_path = 'competing.toml'
    elif changed == 'state':
        config.persistent_state = str(tmp_path / 'other-state')
    else:
        config.init_deadline = 900
    with pytest.raises((CustodyError, RuntimeError)):
        await f.owner.connect(config, f.app)
    assert not f.events


@pytest.mark.asyncio
async def test_failed_sdk_connection_retains_same_runtime_and_shuts_it_down(tmp_path, monkeypatch):
    f = Fixture(tmp_path, monkeypatch)
    await f.owner.prepare(f.app)
    f.fail_connect = True
    with pytest.raises(RuntimeError, match='settlement failure'):
        await f.owner.connect(f.config(), f.app)
    assert f.events == ['construct', 'bind schedules', 'connect', 'shutdown']
    assert f.owner.runtime is not None and f.app.context.runtime is None


@pytest.mark.asyncio
async def test_repeated_cancel_joins_sdk_shutdown_before_clearing_application_runtime(tmp_path, monkeypatch):
    f = Fixture(tmp_path, monkeypatch)
    await f.owner.prepare(f.app)
    entered, release = asyncio.Event(), asyncio.Event()
    runtime_type = f.modules['meerkat_toolkit.application_runtime'].ToolkitApplicationRuntime
    async def wait_cleanup(runtime):
        entered.set()
        await release.wait()
        f.events.append('shutdown')
    monkeypatch.setattr(runtime_type, 'shutdown', wait_cleanup)
    f.fail_connect = True
    task = asyncio.create_task(f.owner.connect(f.config(), f.app))
    await entered.wait()
    task.cancel()
    await asyncio.sleep(0.02)
    task.cancel()
    await asyncio.sleep(0.02)
    assert not task.done() and f.app.context.runtime is f.owner.runtime
    release.set()
    with pytest.raises(RuntimeError, match='settlement failure'):
        await task
    assert f.events == ['construct', 'bind schedules', 'connect', 'shutdown']
    assert f.app.context.runtime is None


@pytest.mark.asyncio
async def test_connected_runtime_cleanup_keeps_original_owner_through_repeated_cancel(tmp_path, monkeypatch):
    f = Fixture(tmp_path, monkeypatch)
    await f.owner.prepare(f.app)
    runtime = await f.owner.connect(f.config(), f.app)
    entered, release = asyncio.Event(), asyncio.Event()
    async def delayed():
        entered.set()
        await release.wait()
        f.events.append('shutdown')
    monkeypatch.setattr(runtime, 'shutdown', delayed)
    task = asyncio.create_task(f.owner.shutdown(runtime))
    await entered.wait()
    task.cancel()
    await asyncio.sleep(0.02)
    task.cancel()
    await asyncio.sleep(0.02)
    assert not task.done() and f.app.context.runtime is runtime
    release.set()
    await task
    assert f.events[-1] == 'shutdown'


@pytest.mark.asyncio
@pytest.mark.parametrize('condition', ['dynamic', 'root', 'runtime', 'callbacks',
                                     'plan-root', 'deadline', 'context-root', 'state-root'])
async def test_fresh_selection_refuses_incompatible_host_or_plan_before_connect(tmp_path, monkeypatch, condition):
    f = Fixture(tmp_path, monkeypatch)
    if condition == 'dynamic':
        f.app.dynamic = True
    elif condition == 'root':
        f.app.context.root = tmp_path / 'other'
    elif condition == 'runtime':
        f.app.context.runtime = object()
    elif condition == 'callbacks':
        f.app._application_callbacks = object()
    elif condition in ('context-root', 'state-root'):
        value = json.loads(f.plan.read())
        value['application']['realm'][condition.replace('-', '_')] = str(tmp_path / 'other')
        f.owner.plan = pin(f.plan.path, json.dumps(value).encode())
        f.owner._finalizer_inputs['preparation_plan'] = f.owner.plan
    else:
        value = json.loads(f.plan.read())
        value['deployment_root' if condition == 'plan-root' else 'hold_timeout_ms'] = (
            str(tmp_path / 'other') if condition == 'plan-root' else 60_000)
        f.owner.plan = pin(f.plan.path, json.dumps(value).encode())
        f.owner._finalizer_inputs['preparation_plan'] = f.owner.plan
    with pytest.raises(CustodyError) as refused:
        await f.owner.prepare(f.app)
    if condition == 'dynamic':
        # A dynamic roster is named, not refused as a generic fixed-application change.
        text = str(refused.value)
        assert 'dynamic agent roster' in text and 'fixed roster' in text, text
        assert text == module.DYNAMIC_ROSTER_UNSUPPORTED
    assert not f.events and f.owner.callbacks is None
    if condition != 'callbacks':
        assert f.app._application_callbacks is None


def block_input_worker(fixture, monkeypatch, operation):
    """Block a real file observation, not the async joining helper or SDK."""
    entered, release = threading.Event(), threading.Event()
    method = 'verify' if operation == 'gateway' else 'read'
    selected = fixture.gateway if operation == 'gateway' else fixture.plan
    original = getattr(ExactFile, method)

    def delayed(pin, *args, **kwargs):
        if pin is selected:
            entered.set()
            assert release.wait(3), 'input worker was not released'
        return original(pin, *args, **kwargs)

    monkeypatch.setattr(ExactFile, method, delayed)
    return entered, release


async def wait_for_input_worker(task, entered):
    async with asyncio.timeout(30):
        while not entered.is_set():
            if task.done():
                await task
                pytest.fail('startup completed before the selected input worker')
            await asyncio.sleep(0.001)


@pytest.mark.asyncio
@pytest.mark.parametrize('operation', ['gateway', 'plan'])
@pytest.mark.parametrize('changed', ['owner', 'owner-root', 'deadline', 'root',
                                    'dynamic', 'runtime', 'callbacks'])
async def test_prepare_rechecks_fixed_host_after_blocked_input_worker(tmp_path, monkeypatch, operation, changed):
    f = Fixture(tmp_path, monkeypatch)
    entered, release = block_input_worker(f, monkeypatch, operation)
    task = asyncio.create_task(f.owner.prepare(f.app))
    try:
        await wait_for_input_worker(task, entered)
        if changed == 'owner':
            f.current = False
        elif changed == 'owner-root':
            f.lease.root = tmp_path / 'other-owner'
        elif changed == 'deadline':
            f.owner.startup_timeout = 900
        elif changed == 'root':
            f.app.context.root = tmp_path / 'other-application'
        elif changed == 'dynamic':
            f.app.dynamic = True
        elif changed == 'runtime':
            f.app.context.runtime = object()
        else:
            f.app._application_callbacks = object()
    finally:
        release.set()
    with pytest.raises((CustodyError, RuntimeError)):
        await task
    assert f.owner.application is f.owner.callbacks is f.owner.finalizer is None
    assert f.owner.runtime is None and f.events == []
    if changed != 'callbacks':
        assert f.app._application_callbacks is None


@pytest.mark.asyncio
@pytest.mark.parametrize('operation', ['gateway', 'plan'])
@pytest.mark.parametrize('changed', ['owner', 'owner-root', 'owner-deadline', 'root',
    'dynamic', 'runtime', 'callbacks', 'roster', 'topology', 'gateway', 'deadline',
    'state', 'definition', 'inline-definition', 'policy', 'carrier', 'digest', 'revision'])
async def test_connect_rechecks_selection_before_constructor_after_blocked_input_worker(
        tmp_path, monkeypatch, operation, changed):
    f = Fixture(tmp_path, monkeypatch)
    await f.owner.prepare(f.app)
    config = f.config()
    entered, release = block_input_worker(f, monkeypatch, operation)
    task = asyncio.create_task(f.owner.connect(config, f.app))
    try:
        await wait_for_input_worker(task, entered)
        if changed == 'owner':
            f.current = False
        elif changed == 'owner-root':
            f.lease.root = tmp_path / 'other-owner'
        elif changed == 'owner-deadline':
            f.owner.startup_timeout = 900
        elif changed == 'root':
            f.app.context.root = tmp_path / 'other-application'
        elif changed == 'dynamic':
            f.app.dynamic = True
        elif changed == 'runtime':
            f.app.context.runtime = object()
        elif changed == 'callbacks':
            f.app._application_callbacks = object()
        else:
            key, value = {
                'roster': ('roster_provider', object()),
                'topology': ('topology_provider', object()),
                'gateway': ('gateway_bin', str(tmp_path / 'unverified-gateway')),
                'deadline': ('init_deadline', 900),
                'state': ('persistent_state', str(tmp_path / 'other-state')),
                'definition': ('mob_config_path', 'competing.toml'),
                'inline-definition': ('mob_config_inline', 'competing'),
                'policy': ('application_tool_policies', []),
                'carrier': ('prepared_application_bundle', b'competing'),
                'digest': ('prepared_application_digest', '0' * 64),
                'revision': ('declare_spec_update_expected_revision', 0),
            }[changed]
            setattr(config, key, value)
    finally:
        release.set()
    with pytest.raises((CustodyError, RuntimeError)):
        await task
    assert f.owner.runtime is None and f.events == []


@pytest.mark.asyncio
async def test_same_shaped_foreign_application_never_replaces_selected_host(tmp_path, monkeypatch):
    f = Fixture(tmp_path, monkeypatch)
    await f.owner.prepare(f.app)
    config = f.config()
    foreign = SimpleNamespace(**vars(f.app))
    config.roster_provider = config.topology_provider = foreign
    with pytest.raises(CustodyError, match='selected application provider'):
        await f.owner.connect(config, foreign)
    assert f.owner.runtime is None and f.events == []


@pytest.mark.asyncio
async def test_builder_refuses_foreign_storage_before_runtime_construction(tmp_path, monkeypatch):
    f = Fixture(tmp_path, monkeypatch)
    await f.owner.prepare(f.app)
    with pytest.raises(CustodyError, match='storage'):
        f.owner.builder_config(roster_provider=f.app, topology_provider=f.app,
            persistent_state=str(tmp_path / 'other-state'))
    assert not f.events


@pytest.mark.asyncio
async def test_cancelled_input_worker_is_joined_before_lease_can_leave(tmp_path, monkeypatch):
    f = Fixture(tmp_path, monkeypatch)
    entered, release = threading.Event(), threading.Event()
    original = module.ApplicationPolicyFinalizer
    def delayed(*args, **kwargs):
        entered.set()
        assert release.wait(3)
        return original(*args, **kwargs)
    monkeypatch.setattr(module, 'ApplicationPolicyFinalizer', delayed)
    task = asyncio.create_task(f.owner.prepare(f.app))
    while not entered.is_set():
        await asyncio.sleep(0.001)
    try:
        task.cancel()
        await asyncio.sleep(0.02)
        task.cancel()
        await asyncio.sleep(0.02)
        assert not task.done() and f.app._application_callbacks is None
    finally:
        release.set()
    with pytest.raises(asyncio.CancelledError):
        await task
    assert not f.events


def test_generated_package_contains_exact_importable_startup_closure_and_checksums(tmp_path):
    files = generate_package(starter_blueprint())
    by_name = {item.path: item for item in files}
    for name in ('connect_application_startup.py', 'connect_application_callbacks.py',
                 'connect_application_finalizer.py', 'connect_installer_caller.py'):
        item = by_name['launchkit/' + name]
        assert item.as_bytes() == (Path(module.__file__).parent / name).read_bytes()
        assert f'{hashlib.sha256(item.as_bytes()).hexdigest()}  {item.path}\n' in by_name['SHA256SUMS'].content
    app = by_name['app.py'].content
    assert 'await application_startup.prepare(application)' in app
    assert 'if application_startup is None:\n            await prepare_storage' in app
    assert 'application_startup=application_startup' in app
    compile(app, 'app.py', 'exec')


@pytest.mark.asyncio
@pytest.mark.parametrize("hosted_origin", [None, "https://console.example.test"])
async def test_actual_generated_run_prepares_once_and_bypasses_old_storage_runtime(tmp_path, monkeypatch, hosted_origin):
    files = generate_files(starter_blueprint())
    for name, content in files.items():
        destination = tmp_path / name
        destination.parent.mkdir(parents=True, exist_ok=True)
        destination.write_text(content)
    monkeypatch.syspath_prepend(str(tmp_path))
    definition = importlib.util.spec_from_file_location('fresh_generated_app', tmp_path / 'app.py')
    host = importlib.util.module_from_spec(definition)
    definition.loader.exec_module(host)
    spec = json.loads(files['deployment.json'])
    # Selected startup requires explicit source intent for every fixed identity.
    # Empty grants here exercise host sequencing without native permission.
    (tmp_path / 'application').mkdir(exist_ok=True)
    (tmp_path / 'application/__init__.py').write_text(
        'class Integration: pass\ndef build(context): return Integration()\n')
    (tmp_path / 'application/tool-policy.json').write_text(json.dumps({
        'provider_id': 'startup-sequencing-specimen', 'policy_id': 'explicit-empty-intent',
        'revision': 1, 'members': [
            {'member_identity': row['identity'], 'grants': []} for row in spec['agents']]}))
    monkeypatch.setattr(host, 'console_configuration', lambda *a: True)
    monkeypatch.setattr(host, 'validate_configuration_values', lambda *a: None)
    monkeypatch.setattr(host, 'validate_console_configuration_values', lambda *a: None)
    write_console_sources(tmp_path, spec)
    monkeypatch.setattr(host, 'local_tools', lambda *a: [])
    events = []
    class Console:
        native_auth = {'selected_console': True}
        def __init__(self, root, port, principal, password, on_input, *, runtime_namespace, **options):
            assert runtime_namespace == 'meta'
            assert options == ({} if hosted_origin is None else {
                "public_origin": hosted_origin, "hosted_mcp_targets": ()})
            events.append('selected Meta Console')
        async def close(self): events.append('console close')
    monkeypatch.setattr(host, 'ConsoleFront', Console)
    class Pool:
        def __init__(self, spec): self.connections = {}
        async def start(self): events.append('pool start')
        async def close(self): events.append('pool close')
        def for_class(self, item): return []
    monkeypatch.setattr(host, 'MCPPool', Pool)
    class Runtime:
        async def identity_bootstrap_status(self):
            return SimpleNamespace(complete=True, ready=True, counts=SimpleNamespace(broken=0))
        async def shutdown(self): events.append('runtime shutdown')
    runtime = Runtime()
    class Startup:
        console_public_origin = hosted_origin
        hosted_mcp_targets = ()
        gateway = SimpleNamespace(path=tmp_path / 'app.py')
        async def prepare(self, application): events.append('prepare')
        def builder_config(self, **options):
            assert 'mob_config_path' not in options
            return SimpleNamespace(**options)
        async def connect(self, config, application):
            events.append('connect')
            if hosted_origin is not None:
                assert application.context.console_url == hosted_origin + "/console"
            assert config.roster_provider is application and config.topology_provider is application
            application.context.stop_event.set()
            return runtime
        async def shutdown(self, selected):
            assert selected is runtime
            await selected.shutdown()
    import storage_setup
    async def forbidden(*args): pytest.fail('old storage bootstrap constructed another runtime')
    monkeypatch.setattr(storage_setup, 'prepare_storage', forbidden)
    args = SimpleNamespace(activate_revision=None, connections_check=False)
    await host.run(spec, args, application_startup=Startup())
    assert events == ['selected Meta Console', 'prepare', 'pool start', 'connect',
        'console close', 'runtime shutdown', 'pool close']


def factory_host(tmp_path, monkeypatch, *flags):
    files = generate_files(starter_blueprint())
    for name, content in files.items():
        destination = tmp_path / name
        destination.parent.mkdir(parents=True, exist_ok=True)
        destination.write_text(content)
    tmp_path.chmod(0o700)
    return load_factory_host(tmp_path, monkeypatch, *flags)


def load_factory_host(tmp_path, monkeypatch, *flags):
    monkeypatch.syspath_prepend(str(tmp_path))
    definition = importlib.util.spec_from_file_location('fresh_entrypoint_app', tmp_path / 'app.py')
    host = importlib.util.module_from_spec(definition)
    definition.loader.exec_module(host)
    monkeypatch.setattr(sys, 'argv', ['app.py', *flags])
    monkeypatch.setattr(host.os, 'chdir', lambda path: None)
    return host


@pytest.mark.parametrize('cleanup_fails', [False, True])
def test_generated_terminal_entrypoint_reports_preserved_startup_and_cleanup_causes(
        tmp_path, monkeypatch, capsys, cleanup_fails):
    host = factory_host(tmp_path, monkeypatch)
    events, caught = [], []
    startup_failure = RuntimeError('Application adapter could not open its selected port.')
    close_failure = OSError('Application listener cleanup is unconfirmed.')
    nested_failure = ValueError('Connector teardown needs its original owner.')
    cleanup_failure = ExceptionGroup('Application teardown failures', [close_failure,
        ExceptionGroup('Connector teardown failures', [nested_failure])])
    builder = ModuleType('meerkat_mobkit.builder')
    builder.MobKitBuilderConfig = lambda **kwargs: pytest.fail('Startup refusal cannot construct a runtime.')
    monkeypatch.setitem(sys.modules, 'meerkat_mobkit.builder', builder)
    monkeypatch.setattr(host, 'gateway_path', lambda: tmp_path / 'app.py')
    monkeypatch.setattr(host, 'console_configuration', lambda *args: False)
    monkeypatch.setattr(host, 'validate_configuration_values', lambda *args: None)

    async def load(application, **kwargs):
        events.append('application load')
        raise startup_failure

    async def stop(application):
        events.append('application stop')
        if cleanup_fails:
            raise cleanup_failure

    async def pool_close():
        events.append('pool close')

    async def tools_close():
        events.append('tools close')

    monkeypatch.setattr(host.Application, 'load', load)
    monkeypatch.setattr(host.Application, 'stop', stop)
    monkeypatch.setattr(host, 'MCPPool', lambda *args: SimpleNamespace(close=pool_close))
    monkeypatch.setattr(host, 'ConnectorTools', lambda *args: SimpleNamespace(close=tools_close))
    original_run, original_exit = asyncio.run, sys.exit

    def child_loop(coroutine):
        async def inert_child():
            monkeypatch.setattr(asyncio.get_running_loop(), 'add_signal_handler', lambda *args: None)
            return await coroutine
        return original_run(inert_child())

    def terminal_exit(code):
        caught.append(sys.exception())
        original_exit(code)

    monkeypatch.setattr(host.asyncio, 'run', child_loop)
    monkeypatch.setattr(host.sys, 'exit', terminal_exit)
    monkeypatch.setattr(host, '__name__', '__main__')
    # Run the exact generated terminal block against the existing loaded host.
    # Its real main/run/finally remain active; no child process is constructed.
    terminal = ast.parse((tmp_path / 'app.py').read_text()).body[-1]
    entrypoint = compile(ast.Module(body=[terminal], type_ignores=[]), str(tmp_path / 'app.py'), 'exec')
    with pytest.raises(SystemExit) as exited:
        exec(entrypoint, host.__dict__)
    assert exited.value.code == 1
    assert events == ['application load', 'application stop', 'pool close', 'tools close']
    output = capsys.readouterr()
    assert not output.out
    if cleanup_fails:
        assert type(caught[0]) is ExceptionGroup
        assert caught[0].exceptions == (startup_failure, cleanup_failure)
        assert cleanup_failure.exceptions[0] is close_failure
        assert cleanup_failure.exceptions[1].exceptions == (nested_failure,)
        assert output.err == (
            'Deployment needs attention: Application run and cleanup failed (2 sub-exceptions)\n'
            '  RuntimeError: Application adapter could not open its selected port.\n'
            '  OSError: Application listener cleanup is unconfirmed.\n'
            '  ValueError: Connector teardown needs its original owner.\n')
    else:
        assert caught == [startup_failure]
        assert output.err == 'Deployment needs attention: Application adapter could not open its selected port.\n'


def test_generated_trusted_entrypoint_passes_same_held_project_lease(tmp_path, monkeypatch):
    host = factory_host(tmp_path, monkeypatch)
    observed, loops, events = [], [], []
    selected = object()

    @asynccontextmanager
    async def owner(lease):
        assert asyncio.get_running_loop() is loops[0]
        lease.require_current()
        events.append('enter')
        try:
            await asyncio.sleep(0)
            yield selected
        finally:
            lease.require_current()
            assert asyncio.get_running_loop() is loops[0]
            events.append('close')

    def factory(lease):
        loops.append(asyncio.get_running_loop())
        lease.require_current()
        assert lease.root == tmp_path
        observed.append(lease)
        events.append('factory')
        return owner(lease)

    async def run(spec, args, *, application_startup):
        observed[0].require_current()
        assert application_startup is selected
        assert asyncio.get_running_loop() is loops[0]
        events.append('run')
        await asyncio.sleep(0)
        observed[0].require_current()
        events.append('cleanup')

    monkeypatch.setattr(host, 'run', run)
    host.main(application_startup_factory=factory)
    assert len(observed) == 1
    assert events == ['factory', 'enter', 'run', 'cleanup', 'close']
    with pytest.raises(RuntimeError, match='no longer current'):
        observed[0].require_current()


@pytest.mark.parametrize('phase', ['factory', 'constructor', 'prepare', 'cancel', 'early'])
def test_generated_factory_lifetime_closes_under_lease_after_failure_or_early_exit(tmp_path, monkeypatch, phase):
    host = factory_host(tmp_path, monkeypatch)
    observed, loops, events = [], [], []
    selected, failure = object(), RuntimeError('original selected startup failure')

    def current(lease):
        lease.require_current()
        assert asyncio.get_running_loop() is loops[0]

    @asynccontextmanager
    async def owner(lease):
        current(lease)
        events.append('enter')
        try:
            if phase == 'constructor':
                raise failure
            yield selected
        finally:
            current(lease)
            await asyncio.sleep(0)
            current(lease)
            events.append('close')

    def factory(lease):
        loops.append(asyncio.get_running_loop())
        current(lease)
        observed.append(lease)
        events.append('factory')
        if phase == 'factory':
            raise failure
        return owner(lease)

    async def run(spec, args, *, application_startup):
        current(observed[0])
        assert application_startup is selected
        events.append('run')
        try:
            if phase == 'prepare':
                raise failure
            if phase == 'cancel':
                asyncio.current_task().cancel()
                await asyncio.sleep(0)
            return
        finally:
            current(observed[0])
            events.append('cleanup entered')
            await asyncio.sleep(0)
            current(observed[0])
            events.append('cleanup settled')

    monkeypatch.setattr(host, 'run', run)
    if phase == 'early':
        host.main(application_startup_factory=factory)
    elif phase == 'cancel':
        with pytest.raises(asyncio.CancelledError):
            host.main(application_startup_factory=factory)
    else:
        with pytest.raises(RuntimeError) as result:
            host.main(application_startup_factory=factory)
        assert result.value is failure
    expected = ['factory']
    if phase != 'factory':
        expected += ['enter']
        if phase != 'constructor':
            expected += ['run', 'cleanup entered', 'cleanup settled']
        expected += ['close']
    assert events == expected
    assert len(observed) == 1
    with pytest.raises(RuntimeError, match='no longer current'):
        observed[0].require_current()


@pytest.mark.parametrize('ready', [True, False])
def test_generated_factory_doctor_exits_context_before_releasing_lease(tmp_path, monkeypatch, ready):
    host = factory_host(tmp_path, monkeypatch, '--doctor')
    observed, loops, events = [], [], []
    selected = object()

    @asynccontextmanager
    async def owner(lease):
        events.append('enter')
        try:
            yield selected
        finally:
            lease.require_current()
            assert asyncio.get_running_loop() is loops[0]
            events.append('close')

    def factory(lease):
        loops.append(asyncio.get_running_loop())
        lease.require_current()
        observed.append(lease)
        return owner(lease)

    def doctor(spec, *, application_startup):
        assert application_startup is selected
        assert asyncio.get_running_loop() is loops[0]
        observed[0].require_current()
        events.append('doctor')
        return ready

    monkeypatch.setattr(host, 'doctor', doctor)
    monkeypatch.setattr(host, 'run', lambda *a, **kw: pytest.fail('Doctor started an application.'))
    with pytest.raises(SystemExit) as result:
        host.main(application_startup_factory=factory)
    assert result.value.code == (0 if ready else 1)
    assert events == ['enter', 'doctor', 'close']
    with pytest.raises(RuntimeError, match='no longer current'):
        observed[0].require_current()


@pytest.mark.parametrize('invalid', ['plain-object', 'not-callable', 'empty-context'])
def test_generated_explicit_factory_refuses_malformed_context_without_ordinary_startup(tmp_path, monkeypatch, invalid):
    host = factory_host(tmp_path, monkeypatch)
    events = []

    @asynccontextmanager
    async def empty(lease):
        try:
            yield None
        finally:
            lease.require_current()
            events.append('close')

    factory = (0 if invalid == 'not-callable' else
        (empty if invalid == 'empty-context' else lambda lease: object()))
    monkeypatch.setattr(host, 'run', lambda *a, **kw: pytest.fail('Malformed selection entered ordinary startup.'))
    with pytest.raises(TypeError):
        host.main(application_startup_factory=factory)
    assert events == (['close'] if invalid == 'empty-context' else [])


def test_generated_main_without_factory_preserves_ordinary_child_loop(tmp_path, monkeypatch):
    host = factory_host(tmp_path, monkeypatch, '--list-flows')
    calls = []

    async def run(spec, args, *, application_startup):
        assert application_startup is None
        assert args.list_flows
        calls.append(asyncio.get_running_loop())

    monkeypatch.setattr(host, 'run', run)
    host.main()
    assert len(calls) == 1 and calls[0].is_closed()


@pytest.mark.parametrize('blocked', ['run-cleanup', 'owner-exit'])
def test_generated_factory_repeated_cancellation_keeps_actual_source_until_cleanup_settles(
        tmp_path, monkeypatch, blocked):
    from test_accepted_source_custody import admitted, retain

    directory, receipt, destination = asyncio.run(admitted(tmp_path))
    host = factory_host(tmp_path / 'entrypoint', monkeypatch)
    spec = host.load_spec()
    monkeypatch.setattr(host, 'ROOT', destination)
    monkeypatch.setattr(host, 'load_spec', lambda: spec)
    original_run = asyncio.run
    sources, leases, callers, loops, events = [], [], [], [], []

    def child_loop(coroutine):
        async def caller():
            callers.append(asyncio.current_task())
            loops.append(asyncio.get_running_loop())
            return await coroutine
        return original_run(caller())

    monkeypatch.setattr(host.asyncio, 'run', child_loop)

    def current(lease):
        lease.require_current()
        assert asyncio.get_running_loop() is loops[0]

    @asynccontextmanager
    async def owner(lease):
        current(lease)
        leases.append(lease)
        source = await retain(directory, receipt, lease, current=lambda: current(lease))
        sources.append(source)
        try:
            yield SimpleNamespace(source=source)
        finally:
            current(lease)
            if blocked == 'owner-exit':
                cleanup_started.set()
                await release.wait()
                current(lease)
            source.close()
            events.append('source closed')

    async def run(spec, args, *, application_startup):
        nonlocal cleanup_started, release
        cleanup_started, release = asyncio.Event(), asyncio.Event()
        source = application_startup.source
        current(leases[0])
        source.require_owner_current()

        async def interrupt():
            callers[0].cancel('first caller cancellation')
            await cleanup_started.wait()
            current(leases[0])
            source.require_owner_current()
            callers[0].cancel('second caller cancellation')
            await asyncio.sleep(0)
            release.set()

        interrupter = asyncio.create_task(interrupt())
        try:
            await asyncio.Event().wait()
        finally:
            current(leases[0])
            events.append('run cleanup entered')
            if blocked == 'run-cleanup':
                cleanup_started.set()
                await release.wait()
                current(leases[0])
            source.require_owner_current()
            events.append('run cleanup settled')
            # Retain the exact control task through the owner's exit as well.
            if blocked == 'run-cleanup':
                await interrupter

    cleanup_started = release = None
    monkeypatch.setattr(host, 'run', run)
    try:
        with pytest.raises(asyncio.CancelledError) as result:
            host.main(application_startup_factory=owner)
        assert str(result.value) == 'first caller cancellation'
        assert events == ['run cleanup entered', 'run cleanup settled', 'source closed']
        with pytest.raises(CustodyError, match='no longer current'):
            sources[0].require_owner_current()
        with pytest.raises(RuntimeError, match='no longer current'):
            leases[0].require_current()
    finally:
        for source in sources:
            source.close()


@pytest.mark.parametrize('phase', ['enter', 'exit'])
@pytest.mark.parametrize('late_failure', [False, True])
def test_generated_factory_first_cancellation_during_entry_or_exit_settles_original_owner(
        tmp_path, monkeypatch, phase, late_failure):
    host = factory_host(tmp_path, monkeypatch)
    original_run = asyncio.run
    callers, loops, leases, events = [], [], [], []
    selected, failure = object(), OSError('original owner failure')

    def child_loop(coroutine):
        async def caller():
            callers.append(asyncio.current_task())
            loops.append(asyncio.get_running_loop())
            return await coroutine
        return original_run(caller())

    monkeypatch.setattr(host.asyncio, 'run', child_loop)

    def current():
        leases[0].require_current()
        assert asyncio.get_running_loop() is loops[0]

    async def interrupt():
        await blocked.wait()
        current()
        callers[0].cancel('first caller cancellation')
        await asyncio.sleep(0)
        callers[0].cancel('second caller cancellation')
        await asyncio.sleep(0)
        release.set()

    async def settle():
        blocked.set()
        await release.wait()
        current()
        events.append(phase + ' settled')

    @asynccontextmanager
    async def owner(lease):
        current()
        try:
            if phase == 'enter':
                await settle()
                if late_failure:
                    raise failure
            yield selected
        finally:
            if phase == 'exit':
                await settle()
            current()
            events.append('owner closed')
            if phase == 'exit' and late_failure:
                raise failure

    def factory(lease):
        nonlocal blocked, release
        current_loop = asyncio.get_running_loop()
        assert current_loop is loops[0]
        lease.require_current()
        leases.append(lease)
        blocked, release = asyncio.Event(), asyncio.Event()
        asyncio.create_task(interrupt())
        return owner(lease)

    async def run(spec, args, *, application_startup):
        assert phase == 'exit', 'A cancelled acquisition must not start the application.'
        assert application_startup is selected
        current()
        events.append('run settled')

    blocked = release = None
    monkeypatch.setattr(host, 'run', run)
    if late_failure:
        with pytest.raises(BaseExceptionGroup) as result:
            host.main(application_startup_factory=factory)
        assert len(result.value.exceptions) == 2
        cancellation, actual_failure = result.value.exceptions
        assert isinstance(cancellation, asyncio.CancelledError)
        assert str(cancellation) == 'first caller cancellation'
        assert actual_failure is failure
    else:
        with pytest.raises(asyncio.CancelledError) as result:
            host.main(application_startup_factory=factory)
        assert str(result.value) == 'first caller cancellation'
    assert events == (['run settled'] if phase == 'exit' else []) + [phase + ' settled', 'owner closed']
    with pytest.raises(RuntimeError, match='no longer current'):
        leases[0].require_current()


@pytest.mark.parametrize('boundary', ['console', 'integration'])
@pytest.mark.parametrize('body_failure', [False, True])
@pytest.mark.parametrize('cleanup_failure', [False, True])
def test_generated_factory_first_cancel_during_actual_run_cleanup_settles_before_owner_exit(
        tmp_path, monkeypatch, boundary, body_failure, cleanup_failure):
    host = factory_host(tmp_path, monkeypatch)
    spec = host.load_spec()
    (tmp_path / 'application').mkdir(exist_ok=True)
    (tmp_path / 'application/__init__.py').write_text(
        'class Integration: pass\ndef build(context): return Integration()\n')
    (tmp_path / 'application/tool-policy.json').write_text(json.dumps({
        'provider_id': 'cleanup-sequencing-specimen', 'policy_id': 'explicit-empty-intent',
        'revision': 1, 'members': [
            {'member_identity': row['identity'], 'grants': []} for row in spec['agents']]}))
    monkeypatch.setattr(host, 'console_configuration', lambda *a: True)
    monkeypatch.setattr(host, 'validate_configuration_values', lambda *a: None)
    monkeypatch.setattr(host, 'validate_console_configuration_values', lambda *a: None)
    write_console_sources(tmp_path, spec)
    monkeypatch.setattr(host, 'local_tools', lambda *a: [])
    original_run = asyncio.run
    callers, leases, events = [], [], []
    failure = RuntimeError('original running application failure')
    cleanup_error = OSError('original cleanup failure')

    def child_loop(coroutine):
        async def caller():
            callers.append(asyncio.current_task())
            return await coroutine
        return original_run(caller())

    monkeypatch.setattr(host.asyncio, 'run', child_loop)

    async def cleanup(name):
        leases[0].require_current()
        events.append(name + ' entered')
        if name == boundary:
            blocked.set()
            await release.wait()
            leases[0].require_current()
        events.append(name + ' settled')
        if name == boundary and cleanup_failure:
            raise cleanup_error

    class Console:
        native_auth = {'selected_console': True}
        def __init__(self, *args, runtime_namespace):
            assert runtime_namespace == 'meta'
        async def close(self): await cleanup('console')

    class Pool:
        def __init__(self, spec): self.connections = {}
        async def start(self): pass
        async def close(self): await cleanup('pool')
        def for_class(self, item): return []

    class Tools:
        def __init__(self, *args): pass
        async def close(self): await cleanup('tools')

    class Runtime:
        async def identity_bootstrap_status(self):
            if body_failure:
                raise failure
            application.context.stop_event.set()
            return SimpleNamespace(complete=True, ready=True, counts=SimpleNamespace(broken=0))
        async def shutdown(self): await cleanup('runtime')

    runtime = Runtime()

    class Startup:
        gateway = SimpleNamespace(path=tmp_path / 'app.py')
        async def prepare(self, selected):
            nonlocal application
            application = selected
            selected.integration.stop = lambda: cleanup('integration')
        def builder_config(self, **options): return SimpleNamespace(**options)
        async def connect(self, config, selected): return runtime
        async def shutdown(self, selected):
            assert selected is runtime
            await selected.shutdown()

    monkeypatch.setattr(host, 'ConsoleFront', Console)
    monkeypatch.setattr(host, 'MCPPool', Pool)
    monkeypatch.setattr(host, 'ConnectorTools', Tools)

    async def interrupt():
        await blocked.wait()
        callers[0].cancel('first caller cancellation')
        await asyncio.sleep(0)
        callers[0].cancel('second caller cancellation')
        await asyncio.sleep(0)
        release.set()

    @asynccontextmanager
    async def owner(lease):
        nonlocal blocked, release
        leases.append(lease)
        blocked, release = asyncio.Event(), asyncio.Event()
        asyncio.create_task(interrupt())
        try:
            yield Startup()
        finally:
            lease.require_current()
            events.append('owner closed')

    application = blocked = release = None
    if body_failure or cleanup_failure:
        with pytest.raises(BaseExceptionGroup) as result:
            host.main(application_startup_factory=owner)
        assert isinstance(result.value.exceptions[0], asyncio.CancelledError)
        assert str(result.value.exceptions[0]) == 'first caller cancellation'
        def leaves(error):
            return ([leaf for child in error.exceptions for leaf in leaves(child)]
                    if isinstance(error, BaseExceptionGroup) else [error])
        actual = leaves(result.value)
        if body_failure:
            assert failure in actual
        if cleanup_failure:
            assert cleanup_error in actual
    else:
        with pytest.raises(asyncio.CancelledError) as result:
            host.main(application_startup_factory=owner)
        assert str(result.value) == 'first caller cancellation'
    assert events == [event for name in ('console', 'integration', 'runtime', 'pool', 'tools')
                      for event in (name + ' entered', name + ' settled')] + ['owner closed']
    with pytest.raises(RuntimeError, match='no longer current'):
        leases[0].require_current()


@pytest.mark.asyncio
@pytest.mark.parametrize('mode', ['fresh', 'restore'])
@pytest.mark.parametrize('phase', ['prepare', 'connect', 'settled'])
async def test_real_accepted_source_change_with_unchanged_native_pins_withholds_startup(
        tmp_path, monkeypatch, mode, phase):
    from test_accepted_source_custody import admitted, retain
    from test_application_restart import RestartFixture
    from launchkit.templates.storage_setup import project_lock

    directory, receipt, destination = await admitted(tmp_path,
        extra_files={'assets/task-context.txt': b'original accepted context\n'})
    owner_loop = threading.get_ident()

    def owner_current():
        assert threading.get_ident() == owner_loop, 'Source owner callbacks stay on their event loop.'

    with project_lock(destination) as lease:
        source = await retain(directory, receipt, lease, current=owner_current)
        try:
            fixture_type = Fixture if mode == 'fresh' else RestartFixture
            f = fixture_type(destination, monkeypatch, accepted_source=source, project_lease=lease)
            original_plan = f.plan.read()

            def change_context():
                (destination / 'assets/task-context.txt').write_bytes(b'unreviewed replacement context\n')
                assert f.plan.read() == original_plan

            if phase == 'prepare':
                change_context()
                with pytest.raises(CustodyError):
                    await f.owner.prepare(f.app)
            else:
                await f.owner.prepare(f.app)
                config = f.config()
                if phase == 'connect':
                    change_context()
                else:
                    runtime_module = (f.modules['meerkat_toolkit.application_runtime']
                        if mode == 'fresh' else f.modules['meerkat_toolkit.host_control'])
                    runtime_class = (runtime_module.ToolkitApplicationRuntime if mode == 'fresh'
                        else runtime_module.ToolkitMobKitRuntime)
                    original_connect = runtime_class.connect

                    async def connect_then_change(runtime):
                        await original_connect(runtime)
                        change_context()

                    monkeypatch.setattr(runtime_class, 'connect', connect_then_change)
                with pytest.raises(CustodyError):
                    await f.owner.connect(config, f.app)
            assert f.events == ([] if phase != 'settled'
                else ['construct', 'bind schedules', 'connect', 'shutdown'])
            assert f.app.context.runtime is None
            assert source._invalidated
        finally:
            source.close()


@pytest.mark.asyncio
@pytest.mark.parametrize('mode', ['fresh', 'restore'])
@pytest.mark.parametrize('boundary', ['prepare', 'connect', 'return'])
async def test_real_source_change_on_worker_handoff_refuses_next_startup_action(
        tmp_path, monkeypatch, mode, boundary):
    from test_accepted_source_custody import admitted, retain
    from test_application_restart import RestartFixture
    from launchkit.templates.storage_setup import project_lock

    directory, receipt, destination = await admitted(tmp_path,
        extra_files={'assets/task-context.txt': b'original accepted context\n'})
    owner_loop = threading.get_ident()

    def owner_current():
        assert threading.get_ident() == owner_loop

    with project_lock(destination) as lease:
        source = await retain(directory, receipt, lease, current=owner_current)
        try:
            fixture_type = Fixture if mode == 'fresh' else RestartFixture
            f = fixture_type(destination, monkeypatch, accepted_source=source, project_lease=lease)
            original_plan = f.plan.read()
            changed = False

            def change_context():
                nonlocal changed
                (destination / 'assets/task-context.txt').write_bytes(b'changed on worker handoff\n')
                assert f.plan.read() == original_plan
                changed = True

            if boundary != 'prepare':
                await f.owner.prepare(f.app)
                config = f.config()
            if mode == 'fresh' and boundary != 'prepare':
                validate = f.owner._require_source_current_async
                calls = 0

                async def validate_then_change():
                    nonlocal calls
                    await validate()
                    calls += 1
                    if calls == (2 if boundary == 'connect' else 3):
                        change_context()

                monkeypatch.setattr(f.owner, '_require_source_current_async', validate_then_change)
            else:
                joined = module._joined
                calls = 0

                async def joined_then_change(function, *args, **kwargs):
                    nonlocal calls
                    result = await joined(function, *args, **kwargs)
                    calls += 1
                    if calls == (2 if boundary == 'return' else 1):
                        change_context()
                    return result

                monkeypatch.setattr(module, '_joined', joined_then_change)
            with pytest.raises(CustodyError):
                if boundary == 'prepare':
                    await f.owner.prepare(f.app)
                else:
                    await f.owner.connect(config, f.app)
            assert changed
            assert f.events == ([] if boundary != 'return'
                else ['construct', 'bind schedules', 'connect', 'shutdown'])
            assert f.app.context.runtime is None
            assert source._invalidated
        finally:
            source.close()


@pytest.mark.asyncio
async def test_real_source_change_after_native_attempt_claim_prevents_command_entry(
        tmp_path, monkeypatch):
    from test_accepted_source_custody import admitted, retain
    from launchkit.templates.storage_setup import project_lock
    from launchkit import connect_installer_caller as caller_module

    directory, receipt, destination = await admitted(tmp_path,
        extra_files={'assets/task-context.txt': b'original accepted context\n'})
    with project_lock(destination) as lease:
        source = await retain(directory, receipt, lease)
        try:
            f = Fixture(destination, monkeypatch, accepted_source=source, project_lease=lease)
            selected = {'deployment_root': str(destination), 'activation': f.template.pin(),
                'policy_manifest': f.plan.pin(), 'local_policy': f.policy.pin()}
            selection_file = pin(destination / 'selected.json', json.dumps(selected).encode())
            attempt = destination / 'validation-attempt.json'
            calls = []

            async def execute(argv):
                calls.append(argv)
                return caller_module.ProcessOutcome(0,
                    json.dumps({'kind': 'application_policy_validated', 'selection': selected}).encode(), b'')

            monkeypatch.setattr(f.owner.caller, 'execute', execute)
            joined = caller_module._joined_filesystem
            changed = False

            async def claim_then_change(function, *args, **kwargs):
                nonlocal changed
                result = await joined(function, *args, **kwargs)
                if function.__name__ == 'claim_attempt':
                    assert attempt.is_file()
                    (destination / 'assets/task-context.txt').write_bytes(b'changed after claim\n')
                    changed = True
                return result

            monkeypatch.setattr(caller_module, '_joined_filesystem', claim_then_change)
            outcome = await f.owner.caller.validate_application_policy(selection_file, attempt_path=attempt)
            assert changed and calls == []
            assert outcome.status == 'source_changed' and outcome.artifact is None
            completion = json.loads(Path(str(attempt) + '.outcome.json').read_text())
            assert completion['status'] == 'outcome_unknown' and completion['process'] is None
            assert source._invalidated
        finally:
            source.close()


def cleanup_failures(error):
    if isinstance(error, BaseExceptionGroup):
        return [item for group in error.exceptions for item in cleanup_failures(group)]
    return [error]


@pytest.mark.asyncio
@pytest.mark.parametrize('mode', ['fresh', 'restore'])
@pytest.mark.parametrize('cleanup_fails', [False, True])
async def test_failed_connect_and_factory_exit_keep_one_exact_shutdown_with_real_source(
        tmp_path, monkeypatch, mode, cleanup_fails):
    from test_accepted_source_custody import admitted, retain
    from test_application_restart import RestartFixture
    from launchkit.deployment_application_bootstrap import BootstrapFactory
    from launchkit.templates.storage_setup import project_lock

    directory, receipt, destination = await admitted(tmp_path)
    connection_failure = RuntimeError('original inert connection failure')
    cleanup_failure = RuntimeError('original inert cleanup failure')
    with project_lock(destination) as lease:
        source = await retain(directory, receipt, lease)
        try:
            fixture_type = Fixture if mode == 'fresh' else RestartFixture
            f = fixture_type(destination, monkeypatch, accepted_source=source, project_lease=lease)
            await f.owner.prepare(f.app)
            runtime_type = (f.modules['meerkat_toolkit.application_runtime'].ToolkitApplicationRuntime
                if mode == 'fresh' else f.modules['meerkat_toolkit.host_control'].ToolkitMobKitRuntime)
            calls = []

            async def failed_connect(runtime):
                raise connection_failure

            async def cleanup(runtime):
                source.require_owner_current()
                lease.require_current()
                calls.append(runtime)
                if cleanup_fails:
                    raise cleanup_failure

            monkeypatch.setattr(runtime_type, 'connect', failed_connect)
            monkeypatch.setattr(runtime_type, 'shutdown', cleanup)
            # Use the actual current bootstrap method-retention join, without
            # admitting or starting an application child or SDK.
            factory = object.__new__(BootstrapFactory)
            factory._shutdown_task = factory._shutdown_runtime = None
            factory._retain_shutdown(f.owner)
            with pytest.raises(BaseException) as outcome:
                await f.owner.connect(f.config(), f.app)
            runtime = f.owner.runtime
            assert runtime is not None and calls == [runtime]
            if cleanup_fails:
                with pytest.raises(BaseException) as repeated:
                    await f.owner.shutdown(runtime)
                assert cleanup_failure in cleanup_failures(repeated.value)
            else:
                await f.owner.shutdown(runtime)
            assert calls == [runtime], 'Factory exit must join the failed-connect cleanup, never start another call'
            assert connection_failure in cleanup_failures(outcome.value)
            if cleanup_fails:
                assert cleanup_failure in cleanup_failures(outcome.value)
                assert f.app.context.runtime is runtime
                assert not source._closed
                source.require_owner_current()
                lease.require_current()
            else:
                assert f.app.context.runtime is None
                source.close()
                assert source._closed
        finally:
            # Test-only descriptor release after failed cleanup does not assert
            # runtime settlement or permit releasing a production owner early.
            source.close()


@pytest.mark.asyncio
@pytest.mark.parametrize('mode', ['fresh', 'restore'])
async def test_failed_connect_concurrent_cleanup_waiters_join_one_original_task(
        tmp_path, monkeypatch, mode):
    from test_application_restart import RestartFixture

    fixture_type = Fixture if mode == 'fresh' else RestartFixture
    f = fixture_type(tmp_path, monkeypatch)
    await f.owner.prepare(f.app)
    runtime_type = (f.modules['meerkat_toolkit.application_runtime'].ToolkitApplicationRuntime
        if mode == 'fresh' else f.modules['meerkat_toolkit.host_control'].ToolkitMobKitRuntime)
    entered, release = asyncio.Event(), asyncio.Event()
    failure = RuntimeError('original delayed inert connection failure')
    calls = []

    async def connect(runtime):
        raise failure

    async def cleanup(runtime):
        calls.append(runtime)
        entered.set()
        await release.wait()

    monkeypatch.setattr(runtime_type, 'connect', connect)
    monkeypatch.setattr(runtime_type, 'shutdown', cleanup)
    connecting = asyncio.create_task(f.owner.connect(f.config(), f.app))
    await entered.wait()
    runtime = f.owner.runtime
    other_waiter = asyncio.create_task(f.owner.shutdown(runtime))
    try:
        await asyncio.sleep(0)
        await asyncio.sleep(0)
        assert calls == [runtime], 'Failed connect and another waiter must retain the same task'
        connecting.cancel()
        other_waiter.cancel()
        await asyncio.sleep(0)
        connecting.cancel()
        other_waiter.cancel()
        await asyncio.sleep(0)
        assert not connecting.done() and not other_waiter.done()
        assert f.app.context.runtime is runtime
    finally:
        release.set()
        outcomes = await asyncio.gather(connecting, other_waiter, return_exceptions=True)
    assert outcomes[0] is failure and outcomes[1] is None
    assert calls == [runtime] and f.app.context.runtime is None


@pytest.mark.asyncio
@pytest.mark.parametrize('mode', ['fresh', 'restore'])
async def test_original_connect_cancellation_and_failed_cleanup_both_survive_repeated_cancel(
        tmp_path, monkeypatch, mode):
    from test_application_restart import RestartFixture

    fixture_type = Fixture if mode == 'fresh' else RestartFixture
    f = fixture_type(tmp_path, monkeypatch)
    await f.owner.prepare(f.app)
    runtime_type = (f.modules['meerkat_toolkit.application_runtime'].ToolkitApplicationRuntime
        if mode == 'fresh' else f.modules['meerkat_toolkit.host_control'].ToolkitMobKitRuntime)
    connecting, cleaning, release = asyncio.Event(), asyncio.Event(), asyncio.Event()
    cleanup_failure = RuntimeError('original inert cancellation cleanup failure')
    calls = []

    async def connect(runtime):
        connecting.set()
        await asyncio.Future()

    async def cleanup(runtime):
        calls.append(runtime)
        cleaning.set()
        await release.wait()
        raise cleanup_failure

    monkeypatch.setattr(runtime_type, 'connect', connect)
    monkeypatch.setattr(runtime_type, 'shutdown', cleanup)
    task = asyncio.create_task(f.owner.connect(f.config(), f.app))
    await connecting.wait()
    task.cancel('original connect cancellation')
    await cleaning.wait()
    task.cancel('later cleanup waiter cancellation')
    await asyncio.sleep(0)
    task.cancel('last cleanup waiter cancellation')
    await asyncio.sleep(0)
    runtime = f.owner.runtime
    assert not task.done() and f.app.context.runtime is runtime
    release.set()
    with pytest.raises(BaseException) as result:
        await task
    failures = cleanup_failures(result.value)
    assert cleanup_failure in failures
    assert any(isinstance(error, asyncio.CancelledError)
        and str(error) == 'original connect cancellation' for error in failures)
    with pytest.raises(RuntimeError) as repeated:
        await f.owner.shutdown(runtime)
    assert repeated.value is cleanup_failure and calls == [runtime]
    assert f.app.context.runtime is runtime
