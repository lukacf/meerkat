"""Installed restart caller controls using explicitly inert SDK specimens.

These controls prove byte custody, provider sequencing and cancellation joins.
They do not qualify native restoration, permission, readiness or useful work.
No SDK specimen launches a gateway or implements native policy semantics.
"""
import asyncio
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import threading
from types import ModuleType, SimpleNamespace

import pytest

from launchkit import connect_application_startup as module
from launchkit.connect_installer_caller import CustodyError, ExactFile
from test_application_callbacks import Fixture as CallbackFixture


def pin(path, content, *, executable=False):
    path.write_bytes(content)
    path.chmod(0o700 if executable else 0o600)
    return ExactFile(path, hashlib.sha256(content).hexdigest())


class RestartFixture:
    def __init__(self, root, monkeypatch, *, accepted_source=None, project_lease=None):
        root.chmod(0o700)
        self.root, self.current, self.events = root, True, []
        # This inert Restore specimen includes the parents from its prior
        # completed initialization. The Restore code must only verify them.
        for name in ("shared", "shared/data", "shared/control"):
            (root / name).mkdir(mode=0o700, exist_ok=True)
        self.app = SimpleNamespace(context=SimpleNamespace(root=root, runtime=None),
            dynamic=False, _application_callbacks=None, bind_schedule_runnables=self.bind)
        source = CallbackFixture(root, native=True)
        value = {**source.value, 'deployment_root': str(root),
            'application': {**source.value['application'], 'realm': {
                'realm_id': 'selected-realm', 'context_root': str(root),
                'state_root': str(root / '.mobkit/state')}}}
        self.plan = pin(source.path, json.dumps(value).encode())
        self.gateway = pin(root / 'gateway', b'inert gateway bytes, never executed\n', executable=True)
        self.activation = pin(root / 'activation.json', b'{"retained":"activation specimen"}\n')
        bundle = root / 'bundle'
        bundle.mkdir(mode=0o700)
        # Literal formatting matters: the carrier must retain these exact bytes.
        self.setup = pin(bundle / 'application-setup.json', b'{ "completed" : true }\n')
        self.policy = pin(bundle / 'toolkit-local-policy.json', b'{"policy":"specimen"}\n')
        self.manifest = pin(bundle / 'manifest.json', json.dumps({'artifacts': [
            {'path': self.setup.path.name, 'sha256': self.setup.sha256},
            {'path': self.policy.path.name, 'sha256': self.policy.sha256}]}).encode())
        self.selection = {'deployment_root': str(root), 'activation': self.activation.pin(),
            'policy_manifest': self.manifest.pin(), 'local_policy': self.policy.pin()}
        self.selection_file = pin(root / 'selection.json', json.dumps(self.selection).encode())
        os.symlink(str(self.activation.path), root / 'current')
        self.lease = project_lease or SimpleNamespace(root=root, require_current=self.check)
        self.source = accepted_source if accepted_source is not None else SimpleNamespace(receipt={'destination': str(root)},
            require_owner_current=self.check, require_filesystem_current=self.check,
            require_current_async=self.check_source_async, require_current=self.check)
        self.owner = self.make_owner()
        self.fail_connect = False
        self.modules = {}
        for name in ('meerkat_meta', 'meerkat_meta.builder', 'meerkat_toolkit',
                     'meerkat_toolkit.host_control'):
            self.modules[name] = ModuleType(name)
            monkeypatch.setitem(sys.modules, name, self.modules[name])

        class ConfigSpecimen:
            def __init__(self, **options):
                self.__dict__.update(options)

        class ArtifactSpecimen:
            def __init__(self, path, sha256):
                self.path, self.sha256 = path, sha256

        class SelectionSpecimen:
            def __init__(self, **fields):
                self.__dict__.update(fields)

        fixture = self

        class RuntimeSpecimen:
            def __init__(self, config, selection, **options):
                assert not options, 'Restart must retain default native Restore mode.'
                fixture.events.append('construct')
                self.config, self.selection = config, selection

            async def connect(self):
                assert fixture.app.context.runtime is self
                fixture.events.append('connect')
                if fixture.fail_connect:
                    raise RuntimeError('SDK connection failure specimen')

            async def shutdown(self):
                fixture.events.append('shutdown')

        self.modules['meerkat_meta.builder'].MetaBuilderConfig = ConfigSpecimen
        host = self.modules['meerkat_toolkit.host_control']
        host.SelectedHostArtifact = ArtifactSpecimen
        host.HostBootstrapSelection = SelectionSpecimen
        host.ToolkitMobKitRuntime = RuntimeSpecimen

    def make_owner(self, **overrides):
        return module.RestoredApplicationStartup(**{**dict(project_lease=self.lease, accepted_source=self.source,
            gateway=self.gateway, selection_file=self.selection_file, preparation_plan=self.plan,
            completed_setup=self.setup, startup_timeout=60), **overrides})

    def check(self):
        if not self.current:
            raise RuntimeError('lost project owner specimen')

    async def check_source_async(self):
        self.check()

    def bind(self, runtime):
        assert self.app.context.runtime is runtime
        self.events.append('bind schedules')

    def config(self, **overrides):
        return self.owner.builder_config(**{**dict(roster_provider=self.app,
            topology_provider=self.app, session_builder=object(), host_runnables=['saved-handler'],
            persistent_state=str(self.root / '.mobkit/state')), **overrides})


@pytest.mark.asyncio
async def test_exact_completed_carrier_and_providers_restore_in_one_runtime(tmp_path, monkeypatch):
    f = RestartFixture(tmp_path, monkeypatch)
    before = os.lstat(tmp_path / 'current')
    await f.owner.prepare(f.app)
    config = f.config()
    runtime = await f.owner.connect(config, f.app)
    assert runtime is f.owner.runtime is f.app.context.runtime
    assert f.events == ['construct', 'bind schedules', 'connect']
    assert config.prepared_application_bundle == f.setup.read()
    assert config.prepared_application_bundle != f.plan.read()
    assert config.prepared_application_bundle != f.manifest.read()
    assert config.prepared_application_digest == f.setup.sha256
    assert config.roster_provider is f.app and config.topology_provider is f.app
    assert config.init_deadline == 60 and config.host_runnables == ['saved-handler']
    for name in ('activation', 'policy_manifest', 'local_policy'):
        artifact = getattr(runtime.selection, name)
        assert {'path': artifact.path, 'sha256': artifact.sha256} == f.selection[name]
    assert runtime.selection.deployment_root == str(tmp_path)
    assert f.app._application_callbacks is f.owner.callbacks
    assert os.lstat(tmp_path / 'current') == before
    with pytest.raises(CustodyError, match='already selected'):
        await f.owner.prepare(f.app)
    with pytest.raises(CustodyError, match='already entered'):
        await f.owner.connect(config, f.app)
    with pytest.raises(CustodyError, match='own retained runtime'):
        await f.owner.shutdown(object())
    await f.owner.shutdown(runtime)
    assert f.events[-1] == 'shutdown'


@pytest.mark.asyncio
@pytest.mark.parametrize('changed', ['owner', 'lease-root', 'deadline'])
async def test_final_input_worker_handoff_rechecks_owner_before_returning_runtime(
        tmp_path, monkeypatch, changed):
    f = RestartFixture(tmp_path, monkeypatch)
    await f.owner.prepare(f.app)
    config = f.config()
    loop = asyncio.get_running_loop()
    original = f.owner._read_inputs
    reads = 0
    pins = (f.gateway, f.plan, f.setup, f.selection_file,
            f.activation, f.manifest, f.policy)
    before = tuple(item.read() for item in pins), os.lstat(tmp_path / 'current')

    def invalidate():
        if changed == 'owner':
            f.current = False
        elif changed == 'lease-root':
            f.lease.root = tmp_path / 'other-root'
        else:
            f.owner.startup_timeout = 900

    def observed_then_invalidated():
        nonlocal reads
        observed = original()
        reads += 1
        if reads == 2:
            # This callback precedes delivery of the worker's completed result.
            # Its own final check has already accepted the original owner.
            loop.call_soon_threadsafe(invalidate)
        return observed

    def no_process(*args, **kwargs):
        pytest.fail('The retained restart specimen must not execute a process.')

    monkeypatch.setattr(f.owner, '_read_inputs', observed_then_invalidated)
    monkeypatch.setattr(asyncio, 'create_subprocess_exec', no_process)
    monkeypatch.setattr(subprocess, 'Popen', no_process)
    with pytest.raises((CustodyError, RuntimeError), match='owner|root|deadline'):
        await f.owner.connect(config, f.app)
    assert reads == 2
    assert f.events == ['construct', 'bind schedules', 'connect', 'shutdown']
    assert f.app.context.runtime is None and f.owner.runtime is not None
    assert before == (tuple(item.read() for item in pins), os.lstat(tmp_path / 'current'))


@pytest.mark.asyncio
@pytest.mark.parametrize('changed', ['gateway', 'plan', 'setup', 'selection_file',
    'activation', 'manifest', 'policy', 'owner', 'lease-root', 'current-missing',
    'current-replaced', 'current-target', 'current-regular'])
async def test_changed_retained_pin_or_current_never_constructs_runtime(tmp_path, monkeypatch, changed):
    f = RestartFixture(tmp_path, monkeypatch)
    await f.owner.prepare(f.app)
    config = f.config()
    if changed in ('gateway', 'plan', 'setup', 'selection_file', 'activation', 'manifest', 'policy'):
        getattr(f, changed).path.write_bytes(b'changed bytes')
    elif changed == 'owner':
        f.current = False
    elif changed == 'lease-root':
        f.lease.root = tmp_path / 'other-root'
    else:
        (tmp_path / 'current').unlink()
        if changed == 'current-replaced':
            os.symlink(str(f.activation.path), tmp_path / 'current')
        elif changed == 'current-target':
            other = pin(tmp_path / 'other.json', f.activation.read())
            os.symlink(str(other.path), tmp_path / 'current')
        elif changed == 'current-regular':
            pin(tmp_path / 'current', f.activation.read())
    with pytest.raises((CustodyError, RuntimeError)):
        await f.owner.connect(config, f.app)
    assert f.events == [] and f.owner.runtime is None


@pytest.mark.asyncio
@pytest.mark.parametrize('changed', ['roster', 'topology', 'callbacks', 'application',
    'application-root', 'dynamic', 'connected', 'gateway', 'deadline', 'state',
    'carrier', 'carrier-type', 'digest', 'definition', 'tool-policy', 'revision', 'config-type'])
async def test_mismatched_config_or_host_never_constructs_runtime(tmp_path, monkeypatch, changed):
    f = RestartFixture(tmp_path, monkeypatch)
    await f.owner.prepare(f.app)
    config, application = f.config(), f.app
    if changed in ('roster', 'topology'):
        setattr(config, changed + '_provider', object())
    elif changed == 'callbacks':
        f.app._application_callbacks = object()
    elif changed == 'application':
        application = SimpleNamespace(**f.app.__dict__)
    elif changed == 'application-root':
        f.app.context.root = tmp_path / 'other'
    elif changed == 'dynamic':
        f.app.dynamic = True
    elif changed == 'connected':
        f.app.context.runtime = object()
    elif changed == 'config-type':
        config = SimpleNamespace(**config.__dict__)
    else:
        field, value = {
            'gateway': ('gateway_bin', '/foreign/gateway'),
            'deadline': ('init_deadline', 900),
            'state': ('persistent_state', '/other/state'),
            'carrier': ('prepared_application_bundle', f.plan.read()),
            'carrier-type': ('prepared_application_bundle', bytearray(f.setup.read())),
            'digest': ('prepared_application_digest', '0' * 64),
            'definition': ('mob_config_path', 'config/mob.toml'),
            'tool-policy': ('application_tool_policies', ['foreign']),
            'revision': ('declare_spec_update_expected_revision', 1),
        }[changed]
        setattr(config, field, value)
    with pytest.raises(CustodyError):
        await f.owner.connect(config, application)
    assert f.events == []


@pytest.mark.asyncio
@pytest.mark.parametrize('changed', ['root', 'realm-context', 'realm-state', 'membership',
    'manifest-duplicate', 'setup-root', 'selection-root', 'missing-current', 'dynamic', 'connected'])
async def test_prepare_refuses_incompatible_retained_selection(tmp_path, monkeypatch, changed):
    f = RestartFixture(tmp_path, monkeypatch)
    if changed in ('root', 'realm-context', 'realm-state'):
        value = json.loads(f.plan.read())
        if changed == 'root':
            value['deployment_root'] = '/foreign-root'
        else:
            value['application']['realm']['context_root' if changed == 'realm-context' else 'state_root'] = '/foreign-root'
        f.owner.plan = pin(f.plan.path, json.dumps(value).encode())
    elif changed in ('membership', 'manifest-duplicate'):
        value = json.loads(f.manifest.read())
        if changed == 'membership':
            value['artifacts'][0]['sha256'] = '0' * 64
        else:
            value['artifacts'].append(value['artifacts'][0])
        f.owner = f.make_owner()
        f.owner.selection_file = pin(f.selection_file.path,
            json.dumps({**f.selection, 'policy_manifest': pin(f.manifest.path, json.dumps(value).encode()).pin()}).encode())
    elif changed == 'setup-root':
        f.owner.setup = pin(tmp_path / 'application-setup.json', f.setup.read())
    elif changed == 'selection-root':
        f.owner.selection_file = pin(f.selection_file.path,
            json.dumps({**f.selection, 'deployment_root': '/foreign-root'}).encode())
    elif changed == 'missing-current':
        (tmp_path / 'current').unlink()
    elif changed == 'dynamic':
        f.app.dynamic = True
        read, entered = f.owner._read_inputs, []
        monkeypatch.setattr(f.owner, '_read_inputs', lambda: entered.append(True) or read())
    else:
        f.app.context.runtime = object()
    with pytest.raises(CustodyError) as refused:
        await f.owner.prepare(f.app)
    if changed == 'dynamic':
        # A dynamic roster is named, not refused as a generic fixed-application change,
        # and at entry: no retained input is read.
        text = str(refused.value)
        assert 'dynamic agent roster' in text and 'fixed roster' in text, text
        assert text == module.DYNAMIC_ROSTER_UNSUPPORTED
        assert entered == []
    assert f.events == [] and f.app._application_callbacks is None


@pytest.mark.parametrize('deadline', [True, 0, -1, float('inf'), float('nan'), '60'])
def test_constructor_requires_finite_startup_owner_deadline(tmp_path, monkeypatch, deadline):
    f = RestartFixture(tmp_path, monkeypatch)
    with pytest.raises(CustodyError, match='finite startup deadline'):
        f.make_owner(startup_timeout=deadline)


@pytest.mark.asyncio
@pytest.mark.parametrize('changed', ['root', 'dynamic', 'runtime', 'callbacks'])
async def test_host_change_during_observation_does_not_bind_callbacks(tmp_path, monkeypatch, changed):
    f = RestartFixture(tmp_path, monkeypatch)
    original = f.owner._read_inputs

    def changed_read():
        observed = original()
        if changed == 'root':
            f.app.context.root = tmp_path / 'foreign-root'
        elif changed == 'dynamic':
            f.app.dynamic = True
        elif changed == 'runtime':
            f.app.context.runtime = object()
        else:
            f.app._application_callbacks = object()
        return observed

    monkeypatch.setattr(f.owner, '_read_inputs', changed_read)
    with pytest.raises(CustodyError) as refused:
        await f.owner.prepare(f.app)
    # A roster that turned dynamic is named first; the other changes keep their text.
    expected = 'dynamic agent roster' if changed == 'dynamic' else 'changed during restart observation'
    assert expected in str(refused.value), str(refused.value)
    assert f.owner.callbacks is None and f.events == []


@pytest.mark.asyncio
@pytest.mark.parametrize('option,value', [('mob_config_path', 'config/mob.toml'),
    ('mob_config_inline', '[mob]'), ('application_tool_policies', []),
    ('prepared_application_bundle', b'foreign'), ('prepared_application_digest', 'foreign'),
    ('declare_spec_update_expected_revision', 0), ('persistent_state', '/other/state')])
async def test_builder_refuses_competing_or_foreign_inputs(tmp_path, monkeypatch, option, value):
    f = RestartFixture(tmp_path, monkeypatch)
    await f.owner.prepare(f.app)
    with pytest.raises(CustodyError):
        f.config(**{option: value})
    assert f.events == []


@pytest.mark.asyncio
async def test_restart_builder_refuses_changed_original_deadline(tmp_path, monkeypatch):
    f = RestartFixture(tmp_path, monkeypatch)
    await f.owner.prepare(f.app)
    f.owner.startup_timeout = 900
    with pytest.raises(CustodyError, match='selected startup deadline'):
        f.config()
    assert f.events == [] and f.owner.runtime is None


@pytest.mark.asyncio
@pytest.mark.parametrize('operation', ['gateway', 'plan'])
async def test_restart_rejects_owner_and_config_deadline_changed_during_input_worker(
        tmp_path, monkeypatch, operation):
    f = RestartFixture(tmp_path, monkeypatch)
    await f.owner.prepare(f.app)
    config = f.config()
    entered, release = threading.Event(), threading.Event()
    method = 'verify' if operation == 'gateway' else 'read'
    selected = f.gateway if operation == 'gateway' else f.plan
    original = getattr(ExactFile, method)

    def delayed(pin, **options):
        if pin is selected:
            entered.set()
            assert release.wait(3), 'deadline counterexample worker was not released'
        return original(pin, **options)

    monkeypatch.setattr(ExactFile, method, delayed)
    task = asyncio.create_task(f.owner.connect(config, f.app))
    try:
        async with asyncio.timeout(30):
            while not entered.is_set():
                if task.done():
                    await task
                    pytest.fail('restart completed before the selected input worker')
                await asyncio.sleep(0.001)
        # Matching mutable fields must not redefine the owner's original limit.
        f.owner.startup_timeout = config.init_deadline = 900
    finally:
        release.set()
    with pytest.raises(CustodyError, match='selected startup deadline'):
        await task
    assert f.events == [] and f.owner.runtime is None and f.app.context.runtime is None


@pytest.mark.asyncio
async def test_failed_connection_joins_same_runtime_before_clearing_context(tmp_path, monkeypatch):
    f = RestartFixture(tmp_path, monkeypatch)
    await f.owner.prepare(f.app)
    f.fail_connect = True
    with pytest.raises(RuntimeError, match='connection failure specimen'):
        await f.owner.connect(f.config(), f.app)
    assert f.events == ['construct', 'bind schedules', 'connect', 'shutdown']
    assert f.owner.runtime is not None and f.app.context.runtime is None


@pytest.mark.asyncio
@pytest.mark.parametrize('changed', ['owner', 'current'])
async def test_loss_during_sdk_connection_closes_same_runtime_without_returning_it(tmp_path, monkeypatch, changed):
    f = RestartFixture(tmp_path, monkeypatch)
    await f.owner.prepare(f.app)

    async def changed_connection(runtime):
        f.events.append('connect')
        if changed == 'owner':
            f.current = False
        else:
            (tmp_path / 'current').unlink()

    monkeypatch.setattr(f.modules['meerkat_toolkit.host_control'].ToolkitMobKitRuntime,
        'connect', changed_connection)
    with pytest.raises((CustodyError, RuntimeError)):
        await f.owner.connect(f.config(), f.app)
    assert f.events == ['construct', 'bind schedules', 'connect', 'shutdown']
    assert f.app.context.runtime is None and f.owner.runtime is not None


@pytest.mark.asyncio
async def test_cancelled_sdk_connection_joins_repeatedly_cancelled_cleanup(tmp_path, monkeypatch):
    f = RestartFixture(tmp_path, monkeypatch)
    await f.owner.prepare(f.app)
    connecting, cleaning, release = asyncio.Event(), asyncio.Event(), asyncio.Event()

    async def waiting_connection(runtime):
        f.events.append('connect')
        connecting.set()
        await asyncio.Future()

    async def waiting_cleanup(runtime):
        cleaning.set()
        await release.wait()
        f.events.append('shutdown')

    runtime_type = f.modules['meerkat_toolkit.host_control'].ToolkitMobKitRuntime
    monkeypatch.setattr(runtime_type, 'connect', waiting_connection)
    monkeypatch.setattr(runtime_type, 'shutdown', waiting_cleanup)
    task = asyncio.create_task(f.owner.connect(f.config(), f.app))
    await connecting.wait()
    task.cancel()
    await cleaning.wait()
    task.cancel()
    await asyncio.sleep(0)
    task.cancel()
    await asyncio.sleep(0)
    assert not task.done() and f.app.context.runtime is f.owner.runtime
    release.set()
    with pytest.raises(asyncio.CancelledError):
        await task
    assert f.app.context.runtime is None and f.events[-1] == 'shutdown'


@pytest.mark.asyncio
async def test_repeated_cancellation_keeps_failed_runtime_cleanup_joined(tmp_path, monkeypatch):
    f = RestartFixture(tmp_path, monkeypatch)
    await f.owner.prepare(f.app)
    entered, release = asyncio.Event(), asyncio.Event()

    async def delayed_cleanup(runtime):
        entered.set()
        await release.wait()
        f.events.append('shutdown')

    monkeypatch.setattr(f.modules['meerkat_toolkit.host_control'].ToolkitMobKitRuntime,
        'shutdown', delayed_cleanup)
    f.fail_connect = True
    task = asyncio.create_task(f.owner.connect(f.config(), f.app))
    await entered.wait()
    task.cancel()
    await asyncio.sleep(0)
    task.cancel()
    await asyncio.sleep(0)
    assert not task.done() and f.app.context.runtime is f.owner.runtime
    release.set()
    with pytest.raises(RuntimeError, match='connection failure specimen'):
        await task
    assert f.app.context.runtime is None and f.events[-1] == 'shutdown'


@pytest.mark.asyncio
async def test_repeated_cancellation_keeps_connected_runtime_shutdown_joined(tmp_path, monkeypatch):
    f = RestartFixture(tmp_path, monkeypatch)
    await f.owner.prepare(f.app)
    runtime = await f.owner.connect(f.config(), f.app)
    entered, release = asyncio.Event(), asyncio.Event()

    async def delayed_cleanup():
        entered.set()
        await release.wait()
        f.events.append('shutdown')

    monkeypatch.setattr(runtime, 'shutdown', delayed_cleanup)
    task = asyncio.create_task(f.owner.shutdown(runtime))
    await entered.wait()
    task.cancel()
    await asyncio.sleep(0)
    task.cancel()
    await asyncio.sleep(0)
    assert not task.done() and f.app.context.runtime is runtime
    release.set()
    await task
    assert f.events[-1] == 'shutdown'


@pytest.mark.asyncio
async def test_cancelled_observation_joins_original_read_without_binding_runtime(tmp_path, monkeypatch):
    f = RestartFixture(tmp_path, monkeypatch)
    entered, release = threading.Event(), threading.Event()
    original = ExactFile.read

    def delayed_read(selected, **options):
        if selected is f.plan:
            entered.set()
            assert release.wait(5), 'test read release was not delivered'
        return original(selected, **options)

    monkeypatch.setattr(ExactFile, 'read', delayed_read)
    task = asyncio.create_task(f.owner.prepare(f.app))
    assert await asyncio.to_thread(entered.wait, 5)
    task.cancel()
    await asyncio.sleep(0)
    task.cancel()
    await asyncio.sleep(0)
    assert not task.done()
    release.set()
    with pytest.raises(asyncio.CancelledError):
        await task
    assert f.owner.callbacks is None and f.app._application_callbacks is None and f.events == []


@pytest.mark.asyncio
async def test_restore_does_not_launch_commands_or_mutate_retained_files(tmp_path, monkeypatch):
    f = RestartFixture(tmp_path, monkeypatch)
    before = {path: path.lstat() for path in tmp_path.rglob('*')}

    def forbidden(*args, **kwargs):
        pytest.fail('Restart attempted an installer, subprocess or filesystem mutation.')

    original_open = os.open

    def read_only_open(path, flags, *args, **kwargs):
        if flags & (os.O_WRONLY | os.O_RDWR | os.O_CREAT | os.O_TRUNC):
            forbidden()
        return original_open(path, flags, *args, **kwargs)

    for name in ('ApplicationPolicyFinalizer', 'NativeInstallerCaller', 'application_command_executor'):
        monkeypatch.setattr(module, name, forbidden)
    monkeypatch.setattr(subprocess, 'Popen', forbidden)
    monkeypatch.setattr(asyncio, 'create_subprocess_exec', forbidden)
    for name in ('symlink', 'replace', 'mkdir', 'unlink'):
        monkeypatch.setattr(os, name, forbidden)
    monkeypatch.setattr(os, 'open', read_only_open)
    monkeypatch.setattr(Path, 'write_bytes', forbidden)
    monkeypatch.setattr(Path, 'write_text', forbidden)
    await f.owner.prepare(f.app)
    runtime = await f.owner.connect(f.config(), f.app)
    await f.owner.shutdown(runtime)
    assert {path: path.lstat() for path in tmp_path.rglob('*')} == before
