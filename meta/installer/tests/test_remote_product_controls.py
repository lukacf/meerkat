"""Original remote RPC, canonical product actions and actual guest writer/owner.

Only SSH delivery, provider/SDK/native effects and Linux peer/birth facts are
inert. Source/package readers, origin selection/custody, actual IPC/controller,
retained same Task/factory, Configure writer and action journal remain real.
These tests establish no cloud VM, login, useful application work or readiness.
"""
import asyncio
from copy import deepcopy
import hashlib
import json
from types import SimpleNamespace
from uuid import uuid4

import pytest

from launchkit import deployment, remote_destination as controller, remote_product_control as control
from launchkit.artifact_binding import package_matches
from launchkit.projects import DraftConflict, fingerprint
from launchkit.remote_deployment import (RemoteDeploymentService, RemoteProductActionNotSent, binding, controller_program,
    read_receipt, source_archive, update_receipt, write_bytes, write_private)
from launchkit.ssh_transport import SshTransportError
from test_remote_product_owner import start_owner, finish_owner, inert_worker
from test_remote_product_control import control_destination
from test_destination_application_inputs import inputs_destination
from test_destination_application_factory import destination
from test_product_destination_install import installation, destination_source, artifact_custody, product
from test_product_destination_owner import no_destination_execution
from test_meta_toolkit_runtime_selection import no_product_execution
import test_product_destination_install as install_fixture


@pytest.fixture
def original_control_package(monkeypatch):
    # Declare a complete original source package before its actual source stage.
    # No accepted installed bytes or currentness reader are replaced afterward.
    package = install_fixture.package_bytes
    def original_package(files):
        selected = dict(files)
        report = json.loads(selected['implementation-report.json'])
        report['brief_hash'] = fingerprint({})
        selected['implementation-report.json'] = control._json(report)
        for name in ('setup.py', 'application_configuration.py', 'mcp_connections.py'):
            selected.setdefault(name, b"raise RuntimeError('inert source must not execute')\n")
        return package(selected)
    monkeypatch.setattr(install_fixture, 'package_bytes', original_package)


@pytest.fixture
async def original_control_destination(original_control_package, control_destination):
    # Pytest resolves async dependencies before entering the owner coroutine.
    return control_destination


@pytest.fixture
async def original_controls(original_control_destination, monkeypatch, tmp_path):
    f = original_control_destination
    files = deployment.package_files(f.selected.package)
    _, transferred = source_archive(files)
    f.remote_owner['binding']['transferred_source_digest'] = transferred
    controller._json_write(f.remote_folder / 'owner.json', f.remote_owner)
    owner = await start_owner(f, monkeypatch)
    gid, did = f.remote_owner['binding']['guide_id'], f.remote_owner['deployment_id']
    state = {'id': gid, 'version': f.selected.context['guide_version'], 'proposal': deepcopy(f.spec)}
    report = json.loads(files['implementation-report.json'][0])
    assert package_matches(report, state)
    selection = f.control_bundle.selection

    class Guide:
        async def get(self, identity):
            assert identity == gid
            return deepcopy(state)
    guide = Guide()
    class Implementation:
        async def status(self, identity, *, state=None):
            now = state if state is not None else await guide.get(identity)
            current = package_matches(report, now)
            return {'guide_id': gid, 'guide_version': report['guide_version'], 'build_id': report['build_id'],
                'current': current, 'current_guide_version': now['version'] if current else None,
                'status': 'completed', 'ready_to_package': True, 'source_digest': report['verification']['digest'],
                'acceptance_current': True, 'acceptance_trusted': True, 'acceptance': {'status': 'pass'}}
        async def package(self, identity, build_id, *, state=None):
            assert identity == gid and build_id == report['build_id']
            return f.selected.package, f.spec['project']['slug']
        def selected_runtime_target(self, identity, *, build_id, source_digest):
            assert identity == gid and build_id == report['build_id'] and source_digest == report['verification']['digest']
            return f.selected.custody.plan.origin
    implementation = Implementation()
    endpoint, key = {'host': 'inert-original.example', 'user': 'fixture', 'ssh_port': 22}, 'SHA256:' + 'A' * 43
    original_adapter = controller._product_adapter
    def adapter():
        namespace, manifest = original_adapter()
        namespace['_control_peer'] = control._control_peer
        return namespace, manifest
    monkeypatch.setattr(controller, '_product_adapter', adapter)
    calls, dropped = [], set()
    class Transport:
        async def request(self, observed_endpoint, program, payload, timeout=30, expected_fingerprint=None):
            assert observed_endpoint == endpoint and expected_fingerprint == key
            assert program == controller_program(f.remote_source)
            calls.append(deepcopy(payload))
            if 'before:' + payload['action'] in dropped:
                dropped.remove('before:' + payload['action'])
                raise SshTransportError('timeout')
            answer = await asyncio.to_thread(controller.handle, payload)
            if payload['action'] in dropped:
                dropped.remove(payload['action'])
                raise SshTransportError('timeout')
            return {**answer, '_transport': {'host_key_fingerprint': key}}
    local = deployment.DeploymentService(tmp_path / 'original-origin', guide, implementation)
    remote = RemoteDeploymentService(tmp_path / 'original-origin', guide, implementation, local,
        transport=Transport(), product_control_selection=selection)
    directory = remote.root / gid / did
    directory.mkdir(mode=0o700, parents=True)
    for name, data in [('package.zip', f.selected.package), ('controller.py', f.remote_source),
        ('product-control.json', control._json(f.control_bundle.manifest)), ('control-bundle.zip', f.control_bundle.content)]:
        write_bytes(directory / name, data)
    receipt = {**f.remote_owner['binding'], 'protocol': 1, 'deployment_id': did, 'target': 'linux',
        'build_guide_version': report['guide_version'], 'product_runtime': True,
        'endpoint': endpoint, 'host_key_fingerprint': key, 'host': f.remote_owner['host'],
        'destination': str(f.root), 'destination_input': str(f.root), 'status': 'installing', 'last_known_status': 'installing',
        'fields': [], 'console_scope': {'authentication': 'local-password'}, 'checks': [], 'progress': [],
        'remote_admission_requested': True, 'install_operation_id': f.remote_operation,
        'pending_operation': {'id': f.remote_operation, 'action': 'install'}, 'preflight': {'ready': True},
        'created_at': deployment.now(), 'updated_at': deployment.now(), 'observation': {'available': False},
        'product_control_manifest_sha256': hashlib.sha256(control._json(f.control_bundle.manifest)).hexdigest(),
        'product_control_bundle_sha256': f.control_bundle.manifest['bundle']['sha256'],
        'product_control_bundle_size': f.control_bundle.manifest['bundle']['size']}
    write_private(directory / 'receipt.json', receipt)
    write_private(directory.parent / 'latest.json', {'deployment_id': did})
    context = SimpleNamespace(f=f, owner=owner, remote=remote, directory=directory, gid=gid, did=did,
        state=state, selection=selection, calls=calls, dropped=dropped)
    try:
        initial = await remote.status(gid, force=True)
        assert initial['status'] == 'stopped' and not initial['pending_operation']
        assert initial['product_scope'] == 'retained-original-product-owner' and initial['worker_live'] is True
        yield context
    finally:
        await finish_owner(owner)


async def configure(c, identity, password):
    receipt = read_receipt(c.directory)
    return await c.remote.configure(c.gid, c.state['version'], c.did, receipt['build_id'],
        receipt['accepted_source_digest'], receipt['package_sha256'],
        settings={'CONSOLE_LOGIN_PASSWORD': password}, remove_fields=[],
        action_request_id=identity, product_owner=c.selection)


async def action(c, name, identity=None):
    return await c.remote.action(c.gid, c.did, name,
        action_request_id=identity or str(uuid4()), product_owner=c.selection)


async def test_actual_live_product_owner_exposes_bound_setup_options_and_stopped_configure(original_controls):
    c, f = original_controls, original_controls.f
    before = (f.remote_folder / 'product-install-completed.json').read_bytes()
    first = await c.remote.status(c.gid, force=True)
    assert first['can_configure'] and first['can_prepare_setup'] and not first['can_restart'] and not first['ready']
    setup = await c.remote.application_setup(c.gid, c.did, product_owner=c.selection)
    assert setup['action'] == 'prepare-setup' and setup['remote_control_binding'] == first['product_control']['binding']
    options = await c.remote.settings_options(c.gid, c.did, product_owner=c.selection)
    assert options['scope'] == 'verified-remote-installed-private-configuration'
    assert options['remote_control_binding'] == first['product_control']['binding']
    assert options['source_digest'] == setup['source_digest'] and options['can_use_host_provider_credentials'] is False
    password = 'source-control-new-private-password'
    identity = str(uuid4())
    result = await configure(c, identity, password)
    assert result['configuration_saved'] and result['can_configure'] and result['status'] == 'stopped'
    assert result['worker_live'] is True and result['cleanup_confirmed'] is False
    assert not result['ready'] and not result['service_owned'] and not result['service_observed']
    assert deployment.private_configuration_snapshot(f.root)[0]['CONSOLE_LOGIN_PASSWORD'] == password
    assert before == (f.remote_folder / 'product-install-completed.json').read_bytes()
    assert c.calls[-1]['operation_id'] == identity and c.calls[-1]['action'] == 'configure'
    assert c.calls[-1]['expected_host'] == f.remote_owner['host'] and c.calls[-1]['binding'] == binding(result)
    assert password not in json.dumps(result) + json.dumps(options)
    assert all(password.encode() not in p.read_bytes() for p in c.remote.root.rglob('*') if p.is_file())
    assert not f.inventory_calls and not f.service.tasks and not c.owner.done()


async def test_lost_configure_reply_reads_same_canonical_uuid_without_secret_repost(original_controls):
    c = original_controls
    identity = str(uuid4())
    c.dropped.add('configure')
    result = await configure(c, identity, 'saved-once-private-password')
    assert result['status'] == 'observation-unavailable' and result.get('configuration_saved') is not True
    assert read_receipt(c.directory)['pending_operation'] == {'id': identity, 'action': 'configure'}
    before = (c.f.root / '.env').read_bytes()
    next = await configure(c, identity, 'must-never-be-reposted-password')
    assert next['configuration_saved'] and not next['pending_operation']
    assert next['product_control']['action_request']['action_request_id'] == identity
    assert before == (c.f.root / '.env').read_bytes()
    assert sum(row['action'] == 'configure' for row in c.calls) == 1
    assert c.calls[-1]['action'] == 'status' and c.calls[-1]['operation_id'] == identity
    assert not any(row.get('settings', {}).get('CONSOLE_LOGIN_PASSWORD') == 'must-never-be-reposted-password' for row in c.calls)


async def test_delivery_lost_before_journal_never_reposts_same_or_replacement_uuid(original_controls):
    c = original_controls
    identity = str(uuid4())
    before = (c.f.root / '.env').read_bytes()
    c.dropped.add('before:configure')
    await configure(c, identity, 'must-remain-unsent-password')
    await configure(c, identity, 'must-not-be-redelivered-password')
    assert sum(row['action'] == 'configure' for row in c.calls) == 1
    assert read_receipt(c.directory)['pending_operation'] == {'id': identity, 'action': 'configure'}
    with pytest.raises(DraftConflict, match='pending product action'):
        await configure(c, str(uuid4()), 'must-not-replace-uncertain-password')
    assert before == (c.f.root / '.env').read_bytes()
    assert not list((c.f.service.root / 'action-requests').glob('*.json'))


@pytest.mark.parametrize('change', ['guide', 'frozen-source'])
async def test_final_local_validation_rejection_publishes_no_unsent_uuid_and_allows_manual_retry(original_controls, monkeypatch, change):
    c, f = original_controls, original_controls.f
    identity = str(uuid4())
    before = (f.root / '.env').read_bytes()
    install_witness = (f.remote_folder / 'product-install-completed.json').read_bytes()
    controller_path = c.directory / 'controller.py'
    original_source = controller_path.read_bytes()
    original_rpc, original_get = c.remote.rpc, c.remote.guide.get
    final = {'active': False, 'changed': False, 'calls': None}

    async def guide_read(gid):
        snapshot = await original_get(gid)
        if final['active'] and change == 'guide' and not final['changed']:
            # This is the real rpc's final awaited Guide read, after the earlier
            # Configure admission checks and real status/IPC readbacks returned.
            assert snapshot['version'] == c.state['version']
            assert read_receipt(c.directory).get('pending_operation') is None
            final['calls'] = deepcopy(c.calls)
            await asyncio.sleep(0)
            c.state['version'] += 1
            final['changed'] = True
            return await original_get(gid)
        return snapshot

    async def final_rpc(directory, receipt, name, **kwargs):
        if name != 'configure':
            return await original_rpc(directory, receipt, name, **kwargs)
        final['active'] = True
        if change == 'frozen-source' and not final['changed']:
            assert read_receipt(c.directory).get('pending_operation') is None
            final['calls'] = deepcopy(c.calls)
            controller_path.write_bytes(original_source + b'\n# changed after earlier source admission\n')
            final['changed'] = True
        try:
            return await original_rpc(directory, receipt, name, **kwargs)
        finally:
            final['active'] = False
            if change == 'frozen-source':
                controller_path.write_bytes(original_source)

    monkeypatch.setattr(c.remote.guide, 'get', guide_read)
    monkeypatch.setattr(c.remote, 'rpc', final_rpc)
    with pytest.raises(RemoteProductActionNotSent) as rejected:
        await configure(c, identity, 'proven-unsent-private-password')
    assert rejected.value.code == 'remote_product_action_not_sent'
    assert rejected.value.action == 'configure' and rejected.value.action_request_id == identity
    assert final['changed'] and final['calls'] == c.calls
    assert not any(row['action'] == 'configure' for row in c.calls)
    assert read_receipt(c.directory).get('pending_operation') is None
    assert before == (f.root / '.env').read_bytes()
    assert install_witness == (f.remote_folder / 'product-install-completed.json').read_bytes()
    assert original_source == controller_path.read_bytes()
    assert not list((f.service.root / 'action-requests').glob('*.json'))
    assert not f.inventory_calls and not f.service.tasks and not f.service.processes and not c.owner.done()
    assert all(b'proven-unsent-private-password' not in p.read_bytes()
        for p in c.remote.root.rglob('*') if p.is_file())

    # A successful checked metadata reload precedes an explicit new user action.
    # A transcript-only Guide revision keeps the original accepted source bound.
    reloaded = await c.remote.settings_options(c.gid, c.did, product_owner=c.selection)
    assert reloaded['version'] == c.state['version'] and reloaded['source_verified']
    replacement = str(uuid4())
    result = await configure(c, replacement, 'manually-retried-private-password')
    assert result['configuration_saved'] and not result['pending_operation']
    assert sum(row['action'] == 'configure' for row in c.calls) == 1
    assert c.calls[-1]['operation_id'] == replacement
    assert deployment.private_configuration_snapshot(f.root)[0]['CONSOLE_LOGIN_PASSWORD'] == 'manually-retried-private-password'


async def test_final_cloud_removal_rejection_keeps_original_action_unsent(original_controls):
    c, f = original_controls, original_controls.f
    identity = str(uuid4())
    before = (f.root / '.env').read_bytes(), deepcopy(c.calls)
    # Exercise the original product-send cloud guard with the same retained
    # actor/source. This logical origin label is not a GCP deployment proof.
    update_receipt(c.directory, target='gcp')
    checks = []
    async def removal(endpoint):
        checks.append(deepcopy(endpoint))
        await asyncio.sleep(0)
        return 'removing' if len(checks) == 2 else None
    c.remote.cloud_removal_status = removal
    with pytest.raises(RemoteProductActionNotSent) as rejected:
        await c.remote._product_send(c.directory, read_receipt(c.directory), 'configure', identity,
            product_owner=c.selection, version=c.state['version'],
            settings={'CONSOLE_LOGIN_PASSWORD': 'cloud-unsent-private-password'}, remove_fields=[])
    assert len(checks) == 2 and rejected.value.action_request_id == identity
    assert rejected.value.action == 'configure' and rejected.value.code == 'remote_product_action_not_sent'
    assert read_receipt(c.directory).get('pending_operation') is None
    assert before == ((f.root / '.env').read_bytes(), c.calls)
    assert not list((f.service.root / 'action-requests').glob('*.json'))
    assert not f.inventory_calls and not f.service.tasks and not f.service.processes and not c.owner.done()


async def test_rejected_reply_after_actual_delivery_never_claims_request_was_not_sent(original_controls, monkeypatch):
    c, f = original_controls, original_controls.f
    identity = str(uuid4())
    original = c.remote.transport.request
    async def foreign_reply(*args, **kwargs):
        answer = await original(*args, **kwargs)
        if args[2]['action'] == 'configure':
            before = answer['binding']['accepted_source_digest']
            answer['binding']['accepted_source_digest'] = ('c' if before != 'c' * 64 else 'd') * 64
            assert answer['binding']['accepted_source_digest'] != before
        return answer
    monkeypatch.setattr(c.remote.transport, 'request', foreign_reply)
    with pytest.raises(DraftConflict) as rejected:
        await configure(c, identity, 'delivered-before-invalid-reply-password')
    assert not isinstance(rejected.value, RemoteProductActionNotSent)
    assert read_receipt(c.directory)['pending_operation'] == {'id': identity, 'action': 'configure'}
    assert deployment.private_configuration_snapshot(f.root)[0]['CONSOLE_LOGIN_PASSWORD'] == 'delivered-before-invalid-reply-password'
    assert sum(row['action'] == 'configure' for row in c.calls) == 1
    monkeypatch.setattr(c.remote.transport, 'request', original)
    recovered = await configure(c, identity, 'must-not-replace-delivered-password')
    assert recovered['configuration_saved'] and not recovered['pending_operation']
    assert sum(row['action'] == 'configure' for row in c.calls) == 1
    assert deployment.private_configuration_snapshot(f.root)[0]['CONSOLE_LOGIN_PASSWORD'] == 'delivered-before-invalid-reply-password'


@pytest.mark.parametrize('change', ['settings-selection', 'request-validation', 'frozen-source'])
async def test_proven_new_configure_rejection_before_send_allows_checked_manual_retry(original_controls, monkeypatch, change):
    c, f = original_controls, original_controls.f
    identity = str(uuid4())
    before = (f.root / '.env').read_bytes()
    install_witness = (f.remote_folder / 'product-install-completed.json').read_bytes()
    controller_path = c.directory / 'controller.py'
    original_source = controller_path.read_bytes()
    original_readback, original_get = c.remote._product_request_readback, c.remote.guide.get
    preparation = {'new_request': False, 'changed': False, 'guide_reads': 0, 'calls': None}

    async def readback(*args, **kwargs):
        result = await original_readback(*args, **kwargs)
        if result[1]:
            assert result[0].get('pending_operation') is None
            preparation['new_request'] = True
            if change == 'frozen-source' and not preparation['changed']:
                preparation['calls'] = deepcopy(c.calls)
                controller_path.write_bytes(original_source + b'\n# changed after canonical new-request readback\n')
                preparation['changed'] = True
        return result

    async def guide_read(gid):
        snapshot = await original_get(gid)
        if preparation['new_request'] and not preparation['changed']:
            preparation['guide_reads'] += 1
            # The first read is _settings_selection's admitted snapshot. The
            # third is Configure's explicit validation after that selection and
            # its own final currentness check have both returned successfully.
            selected_read = 1 if change == 'settings-selection' else 3
            if change != 'frozen-source' and preparation['guide_reads'] == selected_read:
                assert read_receipt(c.directory).get('pending_operation') is None
                preparation['calls'] = deepcopy(c.calls)
                await asyncio.sleep(0)
                c.state['version'] += 1
                preparation['changed'] = True
                return await original_get(gid)
        return snapshot

    monkeypatch.setattr(c.remote, '_product_request_readback', readback)
    monkeypatch.setattr(c.remote.guide, 'get', guide_read)
    try:
        with pytest.raises(RemoteProductActionNotSent) as rejected:
            await configure(c, identity, 'earlier-proven-unsent-private-password')
    finally:
        if change == 'frozen-source':
            controller_path.write_bytes(original_source)
    assert rejected.value.code == 'remote_product_action_not_sent'
    assert rejected.value.action == 'configure' and rejected.value.action_request_id == identity
    assert preparation['new_request'] and preparation['changed']
    assert preparation['calls'] == c.calls
    assert not any(row['action'] == 'configure' for row in c.calls)
    assert read_receipt(c.directory).get('pending_operation') is None
    assert before == (f.root / '.env').read_bytes()
    assert install_witness == (f.remote_folder / 'product-install-completed.json').read_bytes()
    assert original_source == controller_path.read_bytes()
    assert not list((f.service.root / 'action-requests').glob('*.json'))
    assert not f.inventory_calls and not f.service.tasks and not f.service.processes and not c.owner.done()
    assert all(b'earlier-proven-unsent-private-password' not in p.read_bytes()
        for p in c.remote.root.rglob('*') if p.is_file())

    reloaded = await c.remote.settings_options(c.gid, c.did, product_owner=c.selection)
    assert reloaded['version'] == c.state['version'] and reloaded['source_verified']
    replacement = str(uuid4())
    saved = await configure(c, replacement, 'checked-explicit-retry-private-password')
    assert saved['configuration_saved'] and not saved['pending_operation']
    assert sum(row['action'] == 'configure' for row in c.calls) == 1
    assert c.calls[-1]['operation_id'] == replacement
    assert deployment.private_configuration_snapshot(f.root)[0]['CONSOLE_LOGIN_PASSWORD'] == 'checked-explicit-retry-private-password'


async def test_uncertain_configure_readback_conflict_remains_generic_and_preserves_original_intent(original_controls, monkeypatch):
    c, f = original_controls, original_controls.f
    identity = str(uuid4())
    c.dropped.add('configure')
    delivered = await configure(c, identity, 'uncertain-prefix-saved-private-password')
    assert delivered['status'] == 'observation-unavailable'
    before = (f.root / '.env').read_bytes()
    original = c.remote.transport.request
    async def foreign_status(*args, **kwargs):
        answer = await original(*args, **kwargs)
        if args[2]['action'] == 'status':
            original_digest = answer['binding']['accepted_source_digest']
            answer['binding']['accepted_source_digest'] = ('c' if original_digest != 'c' * 64 else 'd') * 64
            assert answer['binding']['accepted_source_digest'] != original_digest
        return answer
    monkeypatch.setattr(c.remote.transport, 'request', foreign_status)
    with pytest.raises(DraftConflict) as rejected:
        await configure(c, identity, 'uncertain-prefix-must-not-be-posted')
    assert not isinstance(rejected.value, RemoteProductActionNotSent)
    assert read_receipt(c.directory)['pending_operation'] == {'id': identity, 'action': 'configure'}
    assert before == (f.root / '.env').read_bytes()
    assert sum(row['action'] == 'configure' for row in c.calls) == 1
    monkeypatch.setattr(c.remote.transport, 'request', original)
    observed = await configure(c, identity, 'uncertain-prefix-must-not-be-retried')
    assert observed['configuration_saved'] and not observed['pending_operation']
    assert before == (f.root / '.env').read_bytes()
    assert sum(row['action'] == 'configure' for row in c.calls) == 1


async def test_prepare_finish_lost_reply_and_owned_stop_use_actual_original_task(original_controls, monkeypatch):
    c, f = original_controls, original_controls.f
    inventory = f.input_owner.host._inventory
    f.actual = {'schema': 1, 'mob_id': inventory['mob_id'], 'helper_identity': inventory['helper_identity'],
        'realm_id': 'inert-destination-realm', 'members': [{'identity': row['identity'], 'member': {
            'mob_id': inventory['mob_id'], 'role': row['profile'], 'member': 'inert-member-' + str(i)}}
            for i, row in enumerate(inventory['roster'])]}
    monkeypatch.setattr(f.held.supervisor, 'command', f.inventory_command)
    prepared = await action(c, 'prepare-setup')
    assert prepared['can_finish_setup'] and not prepared['can_prepare_setup'] and len(f.inventory_calls) == 1
    launches = inert_worker(f, monkeypatch)
    identity = str(uuid4())
    c.dropped.add('finish-setup')
    lost = await action(c, 'finish-setup', identity)
    assert lost['status'] == 'observation-unavailable'
    assert len(launches) == 1 and not launches[0][1].done() and not c.owner.done()
    recovered = await action(c, 'finish-setup', identity)
    assert recovered['product_control']['action_request']['outcome'] == 'returned'
    assert recovered['can_stop'] and not recovered['can_configure'] and len(launches) == 1
    stopped = await action(c, 'stop')
    assert stopped['status'] == 'stopped' and stopped['worker_live'] is True and not stopped['cleanup_confirmed']
    assert launches[0][1].cancelled() and not f.service.tasks and not f.service.processes
    assert not c.owner.done() and len(f.inventory_calls) == 1
    assert all(row.get('operation_id') for row in c.calls if row['action'] in {'prepare-setup', 'finish-setup', 'stop'})


async def test_changed_guide_cannot_start_but_original_active_product_still_stops(original_controls, monkeypatch):
    c, f = original_controls, original_controls.f
    inventory = f.input_owner.host._inventory
    f.actual = {'schema': 1, 'mob_id': inventory['mob_id'], 'helper_identity': inventory['helper_identity'],
        'realm_id': 'inert-destination-realm', 'members': [{'identity': row['identity'], 'member': {
            'mob_id': inventory['mob_id'], 'role': row['profile'], 'member': 'inert-member-' + str(i)}}
            for i, row in enumerate(inventory['roster'])]}
    monkeypatch.setattr(f.held.supervisor, 'command', f.inventory_command)
    await action(c, 'prepare-setup')
    launches = inert_worker(f, monkeypatch)
    await action(c, 'finish-setup')
    c.state['proposal']['name'] = 'A different application need'
    c.state['version'] += 1
    observed = await c.remote.status(c.gid, force=True)
    assert not observed['source_binding_current'] and observed['can_stop']
    assert not observed['can_restart'] and not observed['can_configure'] and not observed['can_finish_setup']
    before = len(c.calls)
    with pytest.raises(DraftConflict, match='current accepted installed source'):
        await action(c, 'restart')
    assert not any(row['action'] == 'restart' for row in c.calls[before:])
    stopped = await action(c, 'stop')
    assert stopped['status'] == 'stopped' and launches[0][1].cancelled() and not f.service.tasks
    assert not c.owner.done()


async def test_public_or_foreign_owner_cannot_admit_product_settings_or_actions(original_controls):
    c = original_controls
    before = len(c.calls), (c.f.root / '.env').read_bytes()
    with pytest.raises(DraftConflict, match='Remote installation is not supported'):
        await c.remote.settings_options(c.gid, c.did)
    with pytest.raises(DraftConflict, match='Remote installation is not supported'):
        await c.remote.action(c.gid, c.did, 'prepare-setup', action_request_id=str(uuid4()))
    with pytest.raises(ValueError, match='canonical request UUID'):
        await c.remote.action(c.gid, c.did, 'prepare-setup', product_owner=c.selection)
    assert before == (len(c.calls), (c.f.root / '.env').read_bytes())


async def test_foreign_actor_binding_and_cloud_removal_withhold_product_controls(original_controls, monkeypatch):
    c = original_controls
    original = c.remote.transport.request
    async def foreign(*args, **kwargs):
        answer = await original(*args, **kwargs)
        answer['product_control']['binding']['command']['pid'] += 1
        return answer
    monkeypatch.setattr(c.remote.transport, 'request', foreign)
    with pytest.raises(DraftConflict):
        await c.remote.settings_options(c.gid, c.did, product_owner=c.selection)
    monkeypatch.setattr(c.remote.transport, 'request', original)
    update_receipt(c.directory, target='gcp')
    async def removed(endpoint):
        return 'removing'
    c.remote.cloud_removal_status = removed
    before = len(c.calls)
    observed = await c.remote.status(c.gid, force=True)
    assert not observed['can_configure'] and not observed['can_prepare_setup'] and not observed['can_finish_setup']
    with pytest.raises(DraftConflict, match='Cloud resource removal'):
        await action(c, 'prepare-setup')
    assert len(c.calls) == before


async def test_ordinary_live_worker_still_cannot_configure_through_product_guards(original_controls):
    c = original_controls
    receipt = read_receipt(c.directory)
    value = c.remote.public({**receipt, 'product_runtime': False}, current=True)
    assert not value['can_configure'] and not value['can_stop'] and not value['can_restart']
    assert value['worker_live'] is True and value['cleanup_confirmed'] is False


def test_remote_api_forwards_selected_product_owner_uuid_and_keeps_private_rejections_redacted(monkeypatch):
    from unittest.mock import AsyncMock
    from fastapi.testclient import TestClient
    from launchkit import server
    gid, did, identity = (str(uuid4()) for _ in range(3))
    selected = object()
    monkeypatch.setattr(server, '_selected_remote_product_owner', lambda: selected)
    result = {'deployment_id': did, 'status': 'stopped'}
    action_mock, setup_mock, configure_mock = (AsyncMock(return_value=result) for _ in range(3))
    monkeypatch.setattr(server.remote_deployment, 'action', action_mock)
    monkeypatch.setattr(server.remote_deployment, 'application_setup', setup_mock)
    monkeypatch.setattr(server.remote_deployment, 'configure', configure_mock)
    client = TestClient(server.app)
    base = f'/api/guide/{gid}/remote-deployment'
    response = client.post(base + '/action', json={'deployment_id': did, 'action': 'prepare-setup', 'action_request_id': identity})
    assert response.status_code == 202
    action_mock.assert_awaited_once_with(gid, did, 'prepare-setup', product_owner=selected, action_request_id=identity)
    response = client.get(base + '/application-setup', params={'deployment_id': did})
    assert response.status_code == 200
    setup_mock.assert_awaited_once_with(gid, did, product_owner=selected)
    body = {'version': 3, 'deployment_id': did, 'build_id': 'original-build', 'source_digest': 'a' * 64,
        'package_sha256': 'b' * 64, 'settings': {'CONSOLE_LOGIN_PASSWORD': 'api-private-canary-password'},
        'remove_fields': [], 'action_request_id': identity}
    response = client.post(base + '/settings', json=body)
    assert response.status_code == 200
    configure_mock.assert_awaited_once_with(gid, 3, did, 'original-build', 'a' * 64, 'b' * 64,
        settings=body['settings'], remove_fields=[], product_owner=selected, action_request_id=identity)
    for changes in ({'action_request_id': 'private-invalid-uuid'}, {'product_owner': 'private-untrusted-owner'}):
        response = client.post(base + '/settings', json={**body, **changes})
        assert response.status_code == 422
        assert 'api-private-canary-password' not in response.text and 'private-invalid' not in response.text
    assert configure_mock.await_count == 1



def test_remote_api_not_sent_code_binds_exact_action_uuid_without_labeling_generic_conflict(monkeypatch):
    from unittest.mock import AsyncMock
    from fastapi.testclient import TestClient
    from launchkit import server
    gid, did, identity = (str(uuid4()) for _ in range(3))
    selected = object()
    monkeypatch.setattr(server, '_selected_remote_product_owner', lambda: selected)
    rejected = AsyncMock(side_effect=RemoteProductActionNotSent('configure', identity))
    monkeypatch.setattr(server.remote_deployment, 'configure', rejected)
    body = {'version': 3, 'deployment_id': did, 'build_id': 'original-build', 'source_digest': 'a' * 64,
        'package_sha256': 'b' * 64, 'settings': {'CONSOLE_LOGIN_PASSWORD': 'not-sent-api-private-canary'},
        'remove_fields': [], 'action_request_id': identity}
    client = TestClient(server.app)
    url = f'/api/guide/{gid}/remote-deployment/settings'
    response = client.post(url, json=body)
    assert response.status_code == 409
    assert response.json() == {'detail': str(rejected.side_effect), 'code': 'remote_product_action_not_sent',
        'action': 'configure', 'action_request_id': identity}
    assert 'not-sent-api-private-canary' not in response.text
    rejected.assert_awaited_once_with(gid, 3, did, 'original-build', 'a' * 64, 'b' * 64,
        settings=body['settings'], remove_fields=[], product_owner=selected, action_request_id=identity)
    generic = AsyncMock(side_effect=DraftConflict('The remote response could not be joined.'))
    monkeypatch.setattr(server.remote_deployment, 'configure', generic)
    response = client.post(url, json=body)
    assert response.status_code == 409 and response.json() == {'detail': str(generic.side_effect)}
    assert 'not-sent-api-private-canary' not in response.text
