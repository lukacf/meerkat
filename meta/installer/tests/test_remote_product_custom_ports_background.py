"""Original product custom ports and Restore service, with inert OS boundaries.

Package/source/factory readers, Configure, Prepare/finalizer/Restore, canonical
UUIDs, service definition generation/verification, the retained actor/IPC,
UserServiceManager parsing and custom producer/browser predicates remain real.
The imported interactive handle/admission fixture and the background manager
command/native receipt/proc/cgroup/tunnel/HTTP edges are explicitly inert. No
real service, runtime, authentication, useful job or cloud deployment is proven.
"""
import asyncio
from copy import deepcopy
import json
import os
from pathlib import Path
import shutil
from uuid import uuid4

import pytest

from launchkit import deployment, deployment_service as services, deployment_worker
from launchkit import remote_destination as controller, remote_product_control as control
from launchkit.application_access import selected_application_entry
from launchkit.projects import DraftConflict
from launchkit.remote_browser_access import entry_ready, native_ready
from launchkit.remote_deployment import read_receipt, update_receipt
from test_remote_browser_access import Forward
from test_product_destination_owner import package_bytes as package_source_bytes
from test_remote_product_custom_access import (
    base_protected_package, protected_owner, protected_package as child_package,
    custom_owner, assert_original, assert_no_owned_link, proc_process,
    original_control_package, original_control_destination, original_controls,
    action, control_destination, inputs_destination, finalize_original_destination,
    destination, installation, destination_source, artifact_custody, product,
    no_destination_execution, no_product_execution,
)
import test_product_destination_install as install_fixture


FIELD_PORT, LITERAL_PORT = 19501, 19502


@pytest.fixture
def port_package(base_protected_package, monkeypatch):
    # Stage declarations before the original source/package owner enters.
    # Keep viewer-selected configuration source inputs intact; the field is
    # declared in the actual deployment environment accepted by both readers.
    package = install_fixture.package_bytes
    def source_package(files):
        _, staged = package(files)
        selected = dict(staged)
        definition = json.loads(selected['deployment.json'])
        assert not any(field['key'] == 'WORKSPACE_PORT' for field in definition['environment'])
        definition['environment'].append({'key': 'WORKSPACE_PORT', 'label': 'Workspace port',
            'required': False, 'secret': False, 'type': 'integer', 'minimum': 1024,
            'maximum': 65535})
        selected['deployment.json'] = control._json(definition)
        selected['application/experience.json'] = control._json({'entries': [
            {'id': 'workspace', 'label': 'Application workspace', 'path': '/workspace',
                'port_field': 'WORKSPACE_PORT'},
            {'id': 'reports', 'label': 'Application reports', 'path': '/reports',
                'port': LITERAL_PORT}]})
        return package_source_bytes(selected)
    monkeypatch.setattr(install_fixture, 'package_bytes', source_package)


@pytest.fixture
async def port_configuration(port_package, original_controls):
    c = original_controls
    receipt = read_receipt(c.directory)
    c.port_configuration_id = str(uuid4())
    saved = await c.remote.configure(c.gid, c.state['version'], c.did, receipt['build_id'],
        receipt['accepted_source_digest'], receipt['package_sha256'],
        settings={'WORKSPACE_PORT': str(FIELD_PORT)}, remove_fields=[],
        action_request_id=c.port_configuration_id, product_owner=c.selection)
    request = saved['product_control']['action_request']
    assert request['action_request_id'] == c.port_configuration_id and request['action'] == 'configure'
    assert request['phase'] == 'settled' and request['outcome'] == 'returned'
    assert saved['configuration_saved']
    options = await c.remote.settings_options(c.gid, c.did, product_owner=c.selection)
    assert options['source_verified'] and options['supported']
    assert (options['deployment_id'], options['build_id'], options['source_digest'], options['package_sha256']) == (
        c.did, receipt['build_id'], receipt['accepted_source_digest'], receipt['package_sha256'])
    fields = [field for field in options['fields'] if field['key'] == 'WORKSPACE_PORT']
    assert len(fields) == 1 and fields[0]['configured'] is True and fields[0]['secret'] is False
    assert controller._interface_settings(c.f.root, {'WORKSPACE_PORT'}) == {'WORKSPACE_PORT': str(FIELD_PORT)}
    return c


@pytest.fixture
def protected_package(port_configuration, child_package):
    # Resolve actual stopped Configure before the original interactive fixture
    # prepares/starts. Its publication gate, Task and admission stay unchanged.
    assert port_configuration is not None


def socket_rows(proc, console_port):
    rows = []
    for port, inode in ((console_port, 9876), (FIELD_PORT, 9877), (LITERAL_PORT, 9878)):
        rows.append(f'0: 0100007F:{port:04X} 00000000:0000 0A 00000000:00000000 '
            f'00:00000000 00000000 {os.getuid()} 0 {inode} 1\n')
    (proc / 'self/net/tcp').write_text('header\n' + ''.join(rows))
    for fd, inode in ((8, 9877), (9, 9878)):
        path = proc / f'61235/fd/{fd}'
        if not path.is_symlink():
            path.symlink_to(f'socket:[{inode}]')


def custom_http(c):
    c.application_checks = []
    async def http(port, path):
        assert (port, path) in {(FIELD_PORT, '/workspace'), (LITERAL_PORT, '/reports')}
        c.application_checks.append((port, path))
        return 200
    c.browser.application_check = http


async def open_entry(c, name='workspace'):
    return await c.browser.open(c.gid, c.state['version'], c.observed['build_id'], c.did,
        'application:' + name)


@pytest.fixture
async def explicit_owner(custom_owner):
    c = custom_owner
    assert c.port not in {FIELD_PORT, LITERAL_PORT}
    # These are distinct inert listener sockets, not one inode serving two
    # ports. The actual producer resolves the declared field from saved .env.
    socket_rows(c.proc, c.port)
    custom_http(c)
    c.observed = await c.remote.status(c.gid, force=True)
    yield c


async def test_original_actor_resolves_saved_nonsecret_and_literal_custom_ports(explicit_owner):
    c = explicit_owner
    observed = c.observed
    assert native_ready(observed, True) and observed['service'] == {}
    tokens = []
    for name, port, path in (('workspace', FIELD_PORT, '/workspace'), ('reports', LITERAL_PORT, '/reports')):
        entry = selected_application_entry(observed, 'application:' + name)
        assert entry['destination_port'] == port and entry['listener_owned']
        assert len(entry['listener_token']) == 64 and entry['verified'] is False
        tokens.append(entry['listener_token'])
        result = await open_entry(c, name)
        assert result['status'] == 'ready' and result['local_url'] == f'http://127.0.0.1:{port}{path}'
        assert result['transport_reachable'] and result['http_status'] == 200
        assert result['application_verified'] is result['authentication_verified'] is result['workflow_verified'] is False
    assert len(set(tokens)) == 2 and c.application_checks == [(FIELD_PORT, '/workspace'), (LITERAL_PORT, '/reports')]
    assert [call[1]['port'] for call in c.browser.transport.calls] == [FIELD_PORT, LITERAL_PORT]
    assert not c.http_checks and c.port_configuration_id != c.finish_id
    assert_original(c)


async def test_port_file_changed_inside_actual_listener_read_cannot_retain_old_value(explicit_owner, monkeypatch):
    c = explicit_owner
    private = c.f.root / '.env'
    original = private.read_bytes()
    original_socket = controller._listener_socket
    writes, reads = [], []
    def socket(port, deadline):
        value = original_socket(port, deadline)
        if port == FIELD_PORT:
            reads.append(value)
            if not writes:
                # Keep the resolved value identical. The file observation
                # itself changed inside the real first kernel snapshot.
                private.write_bytes(original + b'# inert port file drift\n')
                writes.append(port)
        return value
    monkeypatch.setattr(controller, '_listener_socket', socket)
    try:
        observed = await c.remote.status(c.gid, force=True)
        assert writes == [FIELD_PORT] and len(reads) >= 2
        assert native_ready(observed, True)
        assert_no_owned_link(observed)
        assert not entry_ready(observed, True, 'application:workspace')
        assert not c.browser.transport.calls and not c.application_checks
    finally:
        private.write_bytes(original)
    assert_original(c)


@pytest.mark.parametrize('change', ['value', 'duplicate'])
async def test_nonsecret_port_changes_during_custom_http_revoke_owned_forward(explicit_owner, change):
    c = explicit_owner
    entered, release = asyncio.Event(), asyncio.Event()
    async def http(port, path):
        assert (port, path) == (FIELD_PORT, '/workspace')
        c.application_checks.append((port, path))
        entered.set()
        await release.wait()
        return 200
    c.browser.application_check = http
    private = c.f.root / '.env'
    original = private.read_bytes()
    pending = asyncio.create_task(open_entry(c))
    try:
        async with asyncio.timeout(20):
            await entered.wait()
        assert len(c.browser.transport.calls) == 1
        handle = c.browser.transport.handles[0]
        if change == 'value':
            lines = original.decode().splitlines()
            private.write_text('\n'.join('WORKSPACE_PORT=19503' if line.startswith('WORKSPACE_PORT=')
                else line for line in lines) + '\n')
        else:
            private.write_bytes(original + b'WORKSPACE_PORT=19501\n')
        release.set()
        async with asyncio.timeout(20):
            result = await pending
        assert result['status'] == 'unavailable' and result['local_url'] is None
        assert not result['transport_reachable'] and result['cleanup_confirmed']
        assert handle.closes == 1 and not handle.live
        assert c.browser.records[(c.gid, c.did, 'application:workspace')]['handle'] is None
        assert c.application_checks == [(FIELD_PORT, '/workspace')]
        assert 'inert-private-fixture-password' not in json.dumps(result)
    finally:
        release.set()
        await asyncio.gather(pending, return_exceptions=True)
        private.write_bytes(original)
    assert_original(c)


@pytest.fixture
async def background_owner(port_configuration, monkeypatch, tmp_path):
    c, f = port_configuration, port_configuration.f
    manager = f.service.service_manager
    assert type(manager) is services.UserServiceManager and not f.service.tasks and not f.service.processes
    c.native_commands, c.manager_commands, c.health_checks = [], [], []
    c.producer_queries, c.producer_query_index, c.producer_active = [], 0, False
    c.manager_hook = None
    c.manager_loaded, c.manager_running = False, False
    c.background_instance, c.background_attempt = str(uuid4()), str(uuid4())
    proc, groups = tmp_path / 'background-inert-proc', tmp_path / 'background-inert-cgroups'
    for relative in ('self/net', 'self/ns', 'sys/kernel/random'):
        (proc / relative).mkdir(parents=True, exist_ok=True)
    (proc / 'sys/kernel/random/boot_id').write_text('11111111-1111-4111-8111-111111111111\n')
    (proc / 'self/ns/net').symlink_to('net:[123]')
    (proc / 'self/net/tcp6').write_text('header\n')
    for pid, parent, ticks in ((61233, os.getpid(), 90), (61234, 61233, 100), (61235, 61234, 110)):
        proc_process(proc, pid, parent, ticks)
    (proc / '61235/fd/7').symlink_to('socket:[9876]')
    monkeypatch.setattr(controller, 'PROC_ROOT', proc)
    monkeypatch.setattr(controller, 'CGROUP_ROOT', groups)
    c.proc, c.groups, c.manager = proc, groups, manager
    definition = json.loads(f.selected.files['deployment.json'])
    identities = [row['identity'] for row in definition['agents']]
    c.port = deployment.installed_console_port(f.root, definition['port'])
    assert c.port not in {FIELD_PORT, LITERAL_PORT}
    bootstrap = {'complete': True, 'ready': True, 'counts': {'broken': 0, 'ready': len(identities)},
        'identities': {identity: {'state': 'ready'} for identity in identities}}

    def native_service_started():
        canonical = read_receipt(f.directory)
        service = services.verify_service(f.directory, canonical)
        assert canonical['execution_mode'] == 'background' and service['manager'] == 'systemd-user'
        assert f.factory._source is None and f.factory.host._product_source_custody._closed
        # The inert native edge preserves the real Console constructor's key
        # creation before its read-only health authorization can succeed.
        from launchkit.templates_v2.console_auth import _private_key
        _private_key(f.root, create=True)
        c.selected_service = deepcopy(services.service_binding(service))
        c.group = '/user.slice/user-' + str(os.getuid()) + '.slice/' + service['label'] + '.service'
        members = groups / c.group.lstrip('/') / 'cgroup.procs'
        members.parent.mkdir(mode=0o700, parents=True)
        members.write_text('61233\n61234\n61235\n')
        c.members = members
        for pid in (61233, 61234, 61235):
            (proc / f'{pid}/cgroup').write_text('0::' + c.group + '\n')
        socket_rows(proc, c.port)
        url = f'http://127.0.0.1:{c.port}/console'
        deployment_worker.readiness({'pid': 61234, 'bootstrap': bootstrap, 'console_url': url},
            definition, 61234, c.port)
        update_receipt(f.directory, service_supervisor_pid=61233, service_instance_id=c.background_instance,
            service_cleanup_confirmed=False, status='running', phase='run', runtime_pid=61234,
            process_live=True, bootstrap=bootstrap, console_url=url, startup_attempt_id=c.background_attempt,
            startup_console_health={'attempt_id': c.background_attempt, 'status': 'pass', 'attempts': 1},
            health='ready', health_checked_at=deployment.now())

    async def command(arguments, *, allow_failure=False, capture_output=True, timeout=None):
        assert f.service.service_manager is manager or c.manager_hook is not None
        assert arguments[0] == '/usr/bin/systemctl' and arguments[1] == '--user'
        c.manager_commands.append((tuple(arguments), asyncio.current_task()))
        if arguments[2:] == ['show', '--property=Version']:
            return 0, 'Version=255\n'
        receipt = read_receipt(f.directory)
        service = services.verify_service(f.directory, receipt)
        unit = service['label'] + '.service'
        if arguments[2:] == ['daemon-reload']:
            return 0, ''
        if arguments[2:] == ['enable', '--now', unit]:
            assert not c.manager_running and not c.manager_loaded
            c.manager_loaded = c.manager_running = True
            native_service_started()
            return 0, ''
        if arguments[2:] == ['disable', '--now', unit]:
            c.manager_running = False
            update_receipt(f.directory, service_cleanup_confirmed=True, status='stopped', phase='stop',
                runtime_pid=None, process_live=False, console_url=None, bootstrap=None,
                startup_attempt_id=None, startup_console_health=None, health='unknown', health_checked_at=None)
            return 0, ''
        assert arguments[2:] == ['show', unit,
            '--property=LoadState,ActiveState,SubState,MainPID,ControlPID,ControlGroup,FragmentPath']
        # Only the command edge is inert. The original manager's observe parser
        # validates this FragmentPath and passes its finite facts to production.
        output = ('LoadState=' + ('loaded' if c.manager_loaded else 'not-found') + '\n'
            'ActiveState=' + ('active' if c.manager_running else 'inactive') + '\n'
            'SubState=' + ('running' if c.manager_running else 'dead') + '\n'
            'MainPID=' + ('61233' if c.manager_running else '0') + '\nControlPID=0\n'
            'ControlGroup=' + (c.group if c.manager_running else '') + '\n'
            'FragmentPath=' + service['unit_path'] + '\n')
        if c.producer_active:
            assert asyncio.current_task() is c.owner and f.service.lock.locked()
            c.producer_query_index += 1
            index = c.producer_query_index
            c.producer_queries.append((index, deepcopy(services.service_binding(service))))
            if c.manager_hook is not None:
                output = await c.manager_hook(index, output)
            if index == 2:
                c.producer_active = False
        return 0, output
    monkeypatch.setattr(manager, 'command', command)

    # Record the actual producer's successful package read, then target only its
    # manager queries. Factory status has its own earlier manager/health reads.
    original_validation = deployment.await_validation
    async def validation(function, *args, **kwargs):
        value = await original_validation(function, *args, **kwargs)
        if function is deployment._observe_status_package and args[0].__name__ == 'source_observation':
            receipt = read_receipt(f.directory)
            c.producer_active = receipt.get('execution_mode') == 'background' and receipt.get('status') == 'running'
            c.producer_query_index = 0
        return value
    monkeypatch.setattr(deployment, 'await_validation', validation)

    class HealthClient:
        def __init__(self, **kwargs):
            assert kwargs == {'timeout': 3, 'trust_env': False}
        async def __aenter__(self):
            return self
        async def __aexit__(self, *args):
            return False
        async def get(self, url, *, headers):
            assert url == f'http://127.0.0.1:{c.port}/console/identities'
            assert headers and f.service.service_manager is manager
            c.health_checks.append(url)
            class Response:
                def raise_for_status(self):
                    pass
                def json(self):
                    return {'identities': [{'identity': identity} for identity in identities]}
            return Response()
    import httpx
    monkeypatch.setattr(httpx, 'AsyncClient', HealthClient)

    original_dispatch = control._dispatch_control_request
    async def completed_dispatch(service, inputs, context, root, owner, operation_id,
            record, envelope, current, *, ops):
        if envelope['request']['action'] == 'settings-options' and not c.native_commands:
            assert asyncio.current_task() is c.owner and service is f.service
            c.native_commands.extend(await finalize_original_destination(f, inputs, monkeypatch))
            async with service.lock:
                await inputs.factory.continue_application_startup(service, c.gid, c.did)
        return await original_dispatch(service, inputs, context, root, owner,
            operation_id, record, envelope, current, ops=ops)
    monkeypatch.setattr(control, '_dispatch_control_request', completed_dispatch)
    inventory = f.input_owner.host._inventory
    f.actual = {'schema': 1, 'mob_id': inventory['mob_id'], 'helper_identity': inventory['helper_identity'],
        'realm_id': 'inert-destination-realm', 'members': [{'identity': row['identity'], 'member': {
            'mob_id': inventory['mob_id'], 'role': row['profile'], 'member': 'inert-member-' + str(i)}}
            for i, row in enumerate(inventory['roster'])]}
    monkeypatch.setattr(f.held.supervisor, 'command', f.inventory_command)
    prepared = await action(c, 'prepare-setup')
    assert prepared['can_finish_setup'] and f.factory._view()['kind'] == 'fresh'
    assert len(f.inventory_calls) == 1 and not c.native_commands
    settings = await c.remote.settings_options(c.gid, c.did, product_owner=c.selection)
    assert settings['source_verified'] and settings['supported'] and settings['deployment_id'] == c.did
    assert f.factory._view()['kind'] == 'restore' and len(c.native_commands) == 2
    setup = await c.remote.application_setup(c.gid, c.did, product_owner=c.selection)
    assert setup['available'] and setup['kind'] == 'restore' and f.factory._view()['kind'] == 'restore'
    assert len(f.inventory_calls) == 1 and len(c.native_commands) == 2
    c.background_id = str(uuid4())
    started = await action(c, 'start-background', c.background_id)
    request = started['product_control']['action_request']
    assert request['action_request_id'] == c.background_id and request['action'] == 'start-background'
    assert request['phase'] == 'settled' and request['outcome'] == 'returned'
    assert not f.service.tasks and not f.service.processes and not f.service.worker_admissions
    assert f.service.service_manager is manager and f.parent is c.owner and not c.owner.done()
    assert c.manager_running and f.factory._source is None and f.factory.host._product_source_custody._closed
    browser = c.remote.browser_access
    browser.transport, browser.monitor_interval = Forward(), 999
    c.browser, c.observed = browser, await c.remote.status(c.gid, force=True)
    custom_http(c)
    assert native_ready(c.observed, True) and entry_ready(c.observed, True, 'application:workspace')
    assert c.producer_queries[-1][0] == 2 and c.health_checks
    try:
        yield c
    finally:
        c.manager_hook, c.producer_active = None, False
        f.service.service_manager = manager
        await browser.shutdown()
        stopped = await action(c, 'stop')
        assert stopped['status'] == 'stopped' and not c.manager_running
        assert not c.owner.done() and f.parent is c.owner


def assert_background_original(c):
    f = c.f
    assert f.parent is c.owner and not c.owner.done() and f.service.service_manager is c.manager
    assert not f.service.tasks and not f.service.processes and not f.service.worker_admissions
    assert len(f.inventory_calls) == 1 and len(c.native_commands) == 2
    assert f.factory._source is None and f.factory.host._product_source_custody._closed
    canonical = read_receipt(f.directory)
    assert canonical['service_instance_id'] == c.background_instance and canonical['service_supervisor_pid'] == 61233
    assert services.service_binding(services.verify_service(f.directory, canonical)) == c.selected_service


async def test_original_restore_service_manager_projects_and_opens_exact_custom_port(background_owner):
    c = background_owner
    actual = c.observed['product_control']['factory_status']
    assert actual['execution_mode'] == 'background' and actual['service_instance_id'] == c.background_instance
    assert actual['service']['manager'] == 'systemd-user' and c.observed['service'] == {}
    assert actual['service_observation']['owned'] and actual['service_observation']['pid'] == 61233
    entry = selected_application_entry(c.observed, 'application:workspace')
    assert entry['destination_port'] == FIELD_PORT and entry['listener_owned'] and len(entry['listener_token']) == 64
    result = await open_entry(c)
    assert result['status'] == 'ready' and result['local_url'] == f'http://127.0.0.1:{FIELD_PORT}/workspace'
    assert result['transport_reachable'] and result['http_status'] == 200
    assert result['application_verified'] is result['authentication_verified'] is result['workflow_verified'] is False
    assert len(c.browser.transport.calls) == 1 and c.application_checks == [(FIELD_PORT, '/workspace')]
    assert_background_original(c)


@pytest.mark.parametrize('drift', ['instance', 'unit-definition', 'installed-source', 'manager-object', 'foreign-holder'])
async def test_original_background_producer_rejoins_service_and_listener_after_manager_await(background_owner, drift):
    c, f = background_owner, background_owner.f
    entered, release = asyncio.Event(), asyncio.Event()
    async def held(index, output):
        if index == 2:
            entered.set()
            await release.wait()
        return output
    c.manager_hook = held
    unit = Path(c.selected_service['unit_path'])
    source = f.root / 'application/experience.json'
    unit_bytes, source_bytes = unit.read_bytes(), source.read_bytes()
    member_bytes = c.members.read_bytes()
    foreign = c.proc / '61236'
    pending = asyncio.create_task(c.remote.status(c.gid, force=True))
    try:
        async with asyncio.timeout(20):
            await entered.wait()
        assert f.service.lock.locked() and c.producer_query_index == 2
        assert not c.owner.done() and c.producer_queries[-1][1] == c.selected_service
        if drift == 'instance':
            update_receipt(f.directory, service_instance_id=str(uuid4()))
        elif drift == 'unit-definition':
            unit.write_bytes(unit_bytes + b'\n')
        elif drift == 'installed-source':
            source.write_bytes(source_bytes + b'\n')
        elif drift == 'manager-object':
            f.service.service_manager = services.UserServiceManager()
        else:
            proc_process(c.proc, 61236, os.getpid(), 120)
            (foreign / 'cgroup').write_text('0::' + c.group + '\n')
            (foreign / 'fd/8').symlink_to('socket:[9877]')
            (foreign / 'fd/9').symlink_to('socket:[9878]')
            c.members.write_bytes(member_bytes + b'61236\n')
        release.set()
        async with asyncio.timeout(20):
            observed = await pending
        assert_no_owned_link(observed)
        assert not entry_ready(observed, True, 'application:workspace')
        assert not c.browser.transport.calls and not c.application_checks
    finally:
        release.set()
        await asyncio.gather(pending, return_exceptions=True)
        c.manager_hook, c.producer_active = None, False
        f.service.service_manager = c.manager
        update_receipt(f.directory, service_instance_id=c.background_instance)
        unit.write_bytes(unit_bytes)
        source.write_bytes(source_bytes)
        c.members.write_bytes(member_bytes)
        if foreign.exists():
            shutil.rmtree(foreign)
    assert_background_original(c)


async def test_original_background_manager_rejects_foreign_fragment_on_specific_producer_query(background_owner):
    c = background_owner
    reads = []
    async def wrong_fragment(index, output):
        if index == 1:
            reads.append(index)
            return output.replace('FragmentPath=' + c.selected_service['unit_path'],
                'FragmentPath=/private/inert-foreign-definition.service')
        return output
    c.manager_hook = wrong_fragment
    try:
        observed = await c.remote.status(c.gid, force=True)
        assert reads == [1] and native_ready(observed, True)
        assert_no_owned_link(observed)
        assert not entry_ready(observed, True, 'application:workspace')
        with pytest.raises(DraftConflict):
            await open_entry(c)
        assert not c.browser.transport.calls and not c.application_checks
    finally:
        c.manager_hook, c.producer_active = None, False
    assert_background_original(c)


async def test_custom_http_rechecks_original_background_instance_and_closes_only_owned_forward(background_owner):
    c = background_owner
    entered, release = asyncio.Event(), asyncio.Event()
    async def http(port, path):
        assert (port, path) == (FIELD_PORT, '/workspace')
        c.application_checks.append((port, path))
        entered.set()
        await release.wait()
        return 200
    c.browser.application_check = http
    pending = asyncio.create_task(open_entry(c))
    try:
        async with asyncio.timeout(20):
            await entered.wait()
        assert len(c.browser.transport.calls) == 1
        handle = c.browser.transport.handles[0]
        update_receipt(c.f.directory, service_instance_id=str(uuid4()))
        release.set()
        async with asyncio.timeout(20):
            result = await pending
        assert result['status'] == 'unavailable' and result['local_url'] is None
        assert not result['transport_reachable'] and result['cleanup_confirmed']
        assert handle.closes == 1 and not handle.live
        assert c.browser.records[(c.gid, c.did, 'application:workspace')]['handle'] is None
        assert c.application_checks == [(FIELD_PORT, '/workspace')]
    finally:
        release.set()
        await asyncio.gather(pending, return_exceptions=True)
        update_receipt(c.f.directory, service_instance_id=c.background_instance)
    assert_background_original(c)
