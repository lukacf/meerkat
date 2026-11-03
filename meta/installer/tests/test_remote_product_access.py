"""Actual retained product source/factory/IPC with inert runtime and browser edges.

Original byte Install, Prepare/Finish/Restore readers, canonical UUIDs, factory
status, actor Task/IPC, source/currentness and human-report writers remain real.
The existing unspawned runtime body emits explicit synthetic SDK/native health
specimens. No native process, authentication, useful task, SSH or cloud is proven.
"""
import asyncio
from copy import deepcopy
from datetime import datetime, timedelta, timezone
import json
from types import SimpleNamespace
from unittest.mock import AsyncMock
from uuid import uuid4

import pytest

from launchkit import deployment, deployment_worker, remote_product_control as control
from launchkit.application_access import selected_application_entry
from launchkit.application_experiences import declared_application_experiences
from launchkit.artifact_binding import package_matches
from launchkit.projects import DraftConflict
from launchkit.remote_browser_access import access_binding, entry_ready, native_ready, retained_product_status
from launchkit.remote_deployment import read_receipt, update_receipt
from launchkit.remote_installed_feedback import _busy
from test_remote_browser_access import Forward
from test_remote_product_controls import (original_control_package, original_control_destination,
    original_controls, action)
from test_remote_product_owner import inert_worker
from test_remote_product_control import control_destination
from test_destination_application_inputs import inputs_destination, finalize_original_destination
from test_destination_application_factory import destination
from test_product_destination_install import installation, destination_source, artifact_custody, product
from test_product_destination_owner import no_destination_execution
from test_meta_toolkit_runtime_selection import no_product_execution
import test_product_destination_install as install_fixture


@pytest.fixture
def protected_package(original_control_package, monkeypatch):
    # The original fixture's bounded package additions plus interface metadata
    # enter before real source stage, never into an accepted installed tree.
    from launchkit.projects import fingerprint
    package = install_fixture.package_bytes
    def source_package(files):
        selected = dict(files)
        report = json.loads(selected['implementation-report.json'])
        report['brief_hash'] = fingerprint({})
        selected['implementation-report.json'] = control._json(report)
        for name in ('setup.py', 'application_configuration.py', 'mcp_connections.py'):
            selected.setdefault(name, b"raise RuntimeError('inert source must not execute')\n")
        selected['application/experience.json'] = control._json({'entries': [
            {'id': 'workspace', 'label': 'Application workspace', 'path': '/workspace'}]})
        return package(selected)
    monkeypatch.setattr(install_fixture, 'package_bytes', source_package)


@pytest.fixture
async def protected_owner(protected_package, original_control_destination, original_controls, request, monkeypatch):
    # Prepare the original package before its explicit async destination chain;
    # the owner enters only after Pytest resolves those real dependencies.
    c = original_controls
    f = c.f
    assert f is original_control_destination
    completed = getattr(request, 'param', False)
    c.native_commands = []
    if completed:
        manager = f.service.service_manager
        original_capability = manager.capability
        c.manager_capability_commands = []
        async def capability_command(arguments, *, allow_failure=False, capture_output=True, timeout=None):
            assert arguments == ['/usr/bin/systemctl', '--user', 'show', '--property=Version']
            assert allow_failure is True and capture_output is False and timeout is None
            assert asyncio.current_task() is c.owner and f.service.lock.locked()
            assert f.service.service_manager is manager and manager.capability == original_capability
            source = f.factory._source
            assert source is not None and source is f.factory.host._product_source_custody
            source.require_owner_current()
            assert source._lease.root == f.root
            source._lease.require_current()
            c.manager_capability_commands.append((tuple(arguments), manager, asyncio.current_task(), source))
            source.require_owner_current()
            # Only this read-only external query is inert and unavailable.
            return 1, ''
        monkeypatch.setattr(manager, 'command', capability_command)
        # The native completion edge uses the existing real finalizer/Restore
        # specimen, on the actor's original Task, with inert prepare/validate.
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
    assert prepared['can_finish_setup'] and len(f.inventory_calls) == 1
    if completed:
        options = await c.remote.settings_options(c.gid, c.did, product_owner=c.selection)
        assert options['source_verified'] and options['supported'] and options['requires_stopped']
        assert all(options[key] == f.factory.context[key] for key in
            ('guide_version', 'deployment_id', 'build_id', 'source_digest', 'package_sha256'))
        assert f.factory._view()['kind'] == 'restore'
        setup = await c.remote.application_setup(c.gid, c.did, product_owner=c.selection)
        assert setup['available'] and setup['kind'] == 'restore'
        assert c.manager_capability_commands
        assert f.service.service_manager is manager and manager.capability == original_capability
    assert f.factory._view()['kind'] == ('restore' if completed else 'fresh')
    c.launches = inert_worker(f, monkeypatch)
    original_body = f.service._run
    definition = json.loads(f.selected.files['deployment.json'])
    identities = [row['identity'] for row in definition['agents']]
    port = deployment.installed_console_port(f.root, definition['port'])
    c.ready = asyncio.Event()
    c.emissions = []
    publications = asyncio.Queue()
    async def native_body(directory, mode, **kwargs):
        assert directory == f.directory and mode == 'start'
        # Keep native health inert until the real queued action reply settles.
        # Each retained launch Task owns a distinct publication handoff.
        publication = asyncio.Event()
        publications.put_nowait((asyncio.current_task(), publication))
        await publication.wait()
        original = read_receipt(directory)
        lifecycle = original['interactive_lifecycle_id']
        attempt = str(uuid4())
        bootstrap = {'complete': True, 'ready': True, 'counts': {'broken': 0, 'ready': len(identities)},
            'identities': {identity: {'state': 'ready'} for identity in identities}}
        url = f'http://127.0.0.1:{port}/console'
        # Consume the raw specimen through the original readiness parser first.
        deployment_worker.readiness({'pid': 61234, 'bootstrap': bootstrap, 'console_url': url},
            definition, 61234, port)
        update_receipt(directory, status='running', phase='run', runtime_pid=61234, process_live=True,
            startup_attempt_id=attempt, startup_console_health={'attempt_id': attempt, 'status': 'pass', 'attempts': 1},
            bootstrap=bootstrap, console_url=url, health='ready', health_checked_at=deployment.now())
        c.emissions.append((attempt, lifecycle, asyncio.current_task()))
        c.ready.set()
        try:
            await original_body(directory, mode, **kwargs)
        finally:
            # No subprocess/native boundary actually entered. Clear only the
            # inert producer fields before the real original unspawned cleanup.
            update_receipt(directory, status='queued', phase='start', runtime_pid=None, process_live=False,
                startup_attempt_id=None, startup_console_health=None, bootstrap=None,
                console_url=None, health='unknown', health_checked_at=None,
                interactive_lifecycle_id=lifecycle)
    monkeypatch.setattr(f.service, '_run', native_body)
    original_action = c.remote.action
    async def admitted_action(*args, **kwargs):
        result = await original_action(*args, **kwargs)
        name = args[2] if len(args) > 2 else kwargs['action']
        if name in {'finish-setup', 'restart'}:
            request = result['product_control']['action_request']
            assert request['action'] == name and request['action_request_id'] == kwargs['action_request_id']
            assert request['phase'] == 'settled' and request['outcome'] == 'returned'
            async with asyncio.timeout(20):
                task, publication = await publications.get()
            assert task is f.service.tasks[c.did] is c.launches[-1][1] and not task.done()
            publication.set()
        return result
    monkeypatch.setattr(c.remote, 'action', admitted_action)
    c.finish_id = str(uuid4())
    await action(c, 'finish-setup', c.finish_id)
    async with asyncio.timeout(20):
        await c.ready.wait()
    observed = await c.remote.status(c.gid, force=True)
    actual = observed['product_control']['factory_status']
    assert observed['ready'] and observed['worker_live'] is True and observed['cleanup_confirmed'] is False
    assert observed['service'] == {} and not observed['service_owned'] and not observed['service_observed']
    assert actual['source_verified'] and actual['target'] == 'local' and actual['status'] == 'running'
    assert observed['product_control']['native_evidence']['native_runtime_available'] is True
    assert native_ready(observed, True) and not c.owner.done() and f.parent is c.owner
    assert f.service.tasks[c.did] is c.launches[0][1] is c.emissions[0][2]
    if hasattr(c, 'inert_custom_process'):
        # Only the custom actor specimen admits its explicit inert child edge.
        assert f.service.processes[c.did] is c.inert_custom_process
    else:
        assert not f.service.processes
    assert f.factory._source is None
    browser = c.remote.browser_access
    browser.transport, browser.monitor_interval = Forward(), 999
    c.http_checks = []
    async def http(port_value, authentication):
        assert port_value == port and authentication == 'local-password'
        c.http_checks.append((port_value, authentication))
        return True
    browser.browser_check = http
    c.browser, c.port, c.observed = browser, port, observed
    # Existing origin fixture acceptance is explicit and inert. Keep the exact
    # package/current source join real before presenting it as a checked build.
    original_status = c.remote.implementation.status
    async def checked(*args, **kwargs):
        value = await original_status(*args, **kwargs)
        state = kwargs.get('state') or await c.remote.guide.get(c.gid)
        files, _, _ = c.remote.frozen(c.directory, read_receipt(c.directory))
        report = json.loads(files['implementation-report.json'][0])
        assert report['verification']['digest'] == value['source_digest']
        value['source_verified'] = value['current'] and package_matches(report, state)
        return value
    monkeypatch.setattr(c.remote.implementation, 'status', checked)
    c.remote.implementation.submit = AsyncMock(return_value={'build_id': 'inert-next-build', 'status': 'queued'})
    try:
        yield c
    finally:
        await browser.shutdown()
        # Original_controls finally cancels/joins the same actor on its original
        # loop; its normal shutdown owns the inert runtime Task cleanup.


async def open_console(c):
    return await c.browser.open(c.gid, c.state['version'], c.observed['build_id'], c.did)


def retained_bytes(c):
    return {str(path.relative_to(c.remote.root)): path.read_bytes()
        for path in c.remote.root.rglob('*') if path.is_file()}


async def test_idle_original_actor_opens_source_bound_console_without_legacy_service(protected_owner):
    c, f = protected_owner, protected_owner.f
    result = await open_console(c)
    assert result['status'] == 'ready' and result['native_ready']
    assert result['local_url'] == f'http://127.0.0.1:{c.port}/console'
    assert result['scope']['authentication'] == 'local-password' and result['service_name'] is None
    assert c.browser.transport.calls == [(c.observed['endpoint'], {
        'expected_fingerprint': c.observed['host_key_fingerprint'], 'port': c.port})]
    assert c.http_checks == [(c.port, 'local-password')]
    record = c.browser.records[(c.gid, c.did)]
    assert record['binding']['product_control_binding'] == c.observed['product_control']['binding']
    assert record['binding']['product_runtime_binding']['interactive_lifecycle_id'] == c.emissions[0][1]
    assert record['binding']['product_runtime_binding']['startup_attempt_id'] == c.emissions[0][0]
    assert f.parent is c.owner and not c.owner.done() and not c.launches[0][1].done()
    assert len(c.launches) == len(f.inventory_calls) == 1 and not f.service.processes
    assert all(row['action'] == 'status' for row in c.calls[-3:])


@pytest.mark.parametrize('fault', ['missing-startup', 'missing-lifecycle', 'stale-health',
    'broken-bootstrap', 'malformed-update', 'foreign-native-source'])
async def test_incomplete_native_observation_refuses_before_owned_forward(protected_owner, monkeypatch, fault):
    c, f = protected_owner, protected_owner.f
    assert native_ready(c.observed, True)
    if fault == 'missing-startup':
        update_receipt(f.directory, startup_attempt_id=None)
    elif fault == 'missing-lifecycle':
        update_receipt(f.directory, interactive_lifecycle_id=None)
    elif fault == 'stale-health':
        update_receipt(f.directory, health_checked_at=(datetime.now(timezone.utc) - timedelta(seconds=60)).isoformat())
    elif fault == 'broken-bootstrap':
        original = deepcopy(read_receipt(f.directory)['bootstrap'])
        original['counts']['broken'] = 1
        update_receipt(f.directory, bootstrap=original)
    else:
        original = c.remote.transport.request
        async def corrupted(*args, **kwargs):
            answer = await original(*args, **kwargs)
            if fault == 'malformed-update':
                answer['product_control']['factory_status']['update'] = {'status': []}
            else:
                digest = answer['product_control']['factory_status']['source_digest']
                answer['product_control']['native_evidence']['source_digest'] = ('b' if digest == 'a' * 64 else 'a') * 64
                assert answer['product_control']['native_evidence']['source_digest'] != digest
            return answer
        monkeypatch.setattr(c.remote.transport, 'request', corrupted)
    with pytest.raises(DraftConflict):
        await open_console(c)
    assert not c.browser.transport.calls and not c.http_checks
    assert not native_ready(read_receipt(c.directory), True)
    assert not c.owner.done() and f.service.tasks[c.did] is c.launches[0][1]
    assert len(c.launches) == len(f.inventory_calls) == 1


@pytest.mark.parametrize('phase', ['update', 'recovery'])
async def test_running_canonical_mutation_window_never_opens_native_console(protected_owner, phase):
    c, f = protected_owner, protected_owner.f
    update_receipt(f.directory, phase=phase)
    observed = await c.remote.status(c.gid, force=True)
    assert observed['status'] == 'running' and observed['ready']
    assert observed['product_control']['native_evidence']['native_runtime_available'] is True
    assert retained_product_status(observed) is not None
    assert not native_ready(observed, True) and _busy(c.remote, observed)
    with pytest.raises(DraftConflict):
        await open_console(c)
    assert not c.browser.transport.calls and not c.http_checks and not c.owner.done()


@pytest.mark.parametrize('drift', ['runtime', 'actor', 'source', 'mutation'])
async def test_drift_during_console_http_rejoins_original_owner_and_closes_forward(protected_owner, drift):
    c, f = protected_owner, protected_owner.f
    first = await open_console(c)
    handle = c.browser.transport.handles[0]
    entered, release = asyncio.Event(), asyncio.Event()
    async def held_http(port, authentication):
        assert (port, authentication) == (c.port, 'local-password')
        entered.set()
        await release.wait()
        return True
    c.browser.browser_check = held_http
    pending = asyncio.create_task(c.browser.status(c.gid, c.did))
    try:
        async with asyncio.timeout(20):
            await entered.wait()
        if drift == 'runtime':
            update_receipt(f.directory, startup_attempt_id=str(uuid4()), interactive_lifecycle_id=str(uuid4()))
        elif drift == 'actor':
            f.live = False
        elif drift == 'source':
            pin = c.directory / 'controller.py'
            pin.write_bytes(pin.read_bytes() + b'\n# changed owned origin source\n')
        else:
            update_receipt(f.directory, phase='update')
        release.set()
        async with asyncio.timeout(20):
            result = await pending
        assert result['status'] == 'unavailable' and result['local_url'] is None and not result['native_ready']
        assert handle.closes == 1 and not handle.live and result['cleanup_confirmed']
        assert c.browser.records[(c.gid, c.did)]['handle'] is None
        assert len(c.browser.transport.calls) == 1 and first['access_id'] == result['access_id']
        assert not c.owner.done() and len(c.launches) == 1
    finally:
        release.set()
        if not pending.done():
            pending.cancel()
        await asyncio.gather(pending, return_exceptions=True)
        f.live = True


@pytest.mark.parametrize('protected_owner', [True], indirect=True, ids=['original-completed-restore'])
async def test_same_port_canonical_restart_revokes_prior_forward_without_fresh_replay(protected_owner):
    c, f = protected_owner, protected_owner.f
    assert f.factory._view()['kind'] == 'restore' and len(c.native_commands) == 2
    original_pin = f.input_owner.preparation_owner._restore_pin.read()
    first = await open_console(c)
    handle = c.browser.transport.handles[0]
    old = deepcopy(c.browser.records[(c.gid, c.did)]['binding'])
    stopped = await action(c, 'stop')
    assert stopped['status'] == 'stopped' and c.launches[0][1].cancelled()
    c.ready.clear()
    identity = str(uuid4())
    restarted = await action(c, 'restart', identity)
    async with asyncio.timeout(20):
        await c.ready.wait()
    current = await c.remote.status(c.gid, force=True)
    request = (await f.service.action_request(c.gid, c.did, identity))['action_request']
    assert request == restarted['product_control']['action_request']
    assert request['action_request_id'] == identity and request['phase'] == 'settled'
    assert request['outcome'] == 'returned'
    assert current['product_control']['factory_status']['console_url'] == first['local_url']
    assert handle.closes == 1 and not handle.live
    assert access_binding(current) != old and c.browser.public((c.gid, c.did))['local_url'] is None
    assert f.input_owner.preparation_owner._restore_pin.read() == original_pin
    second = await open_console(c)
    assert second['status'] == 'ready' and second['access_id'] != first['access_id']
    assert len(c.browser.transport.calls) == len(c.launches) == 2
    assert c.emissions[0][1] != c.emissions[1][1] and c.emissions[0][0] != c.emissions[1][0]
    assert len(f.inventory_calls) == 1 and len(c.native_commands) == 2 and not c.owner.done()


async def test_idle_actor_human_report_and_correction_do_not_dispatch_destination_tasks(protected_owner):
    c = protected_owner
    before, calls = retained_bytes(c), deepcopy(c.calls)
    use = await c.remote.use(c.gid, c.did)
    assert use['native_runtime_available'] and use['installed_source_verified']
    assert use['success_reporting_available'] and use['continuation']['available']
    assert use['application_outcomes_verified'] is False
    assert retained_bytes(c) == before and c.calls == calls
    report = await c.remote.record_success(c.gid, c.state['version'], c.did, use['build_id'],
        use['source_digest'], use['package_sha256'], request_id=str(uuid4()), task='Inert reported task only.')
    assert report['outcome'] == 'worked' and report['application_outcomes_verified'] is False
    after = retained_bytes(c)
    result = await c.remote.feedback(c.gid, c.state['version'], use['remote_binding'],
        use['continuation']['build_id'], use['continuation']['source_digest'],
        request='Make the reported first job clearer.', outcome='partly', task='Inert reported task only.')
    assert result == {'build_id': 'inert-next-build', 'status': 'queued'}
    c.remote.implementation.submit.assert_awaited_once()
    assert c.remote.implementation.submit.await_args.kwargs == {
        'expected_previous_build_id': use['continuation']['build_id'],
        'expected_source_digest': use['continuation']['source_digest']}
    assert retained_bytes(c) == after and c.calls == calls and len(c.launches) == 1 and not c.owner.done()


@pytest.mark.parametrize('uncertainty', ['pending', 'unknown'])
async def test_original_pending_or_lost_uuid_blocks_browser_reports_and_correction(protected_owner, monkeypatch, uncertainty):
    c = protected_owner
    positive = await c.remote.use(c.gid, c.did)
    assert positive['success_reporting_available'] and positive['continuation']['available']
    identity = str(uuid4())
    if uncertainty == 'pending':
        entered, release = asyncio.Event(), asyncio.Event()
        original = c.remote.transport.request
        async def held(*args, **kwargs):
            payload = args[2]
            if payload['action'] == 'stop':
                entered.set()
                await release.wait()
            return await original(*args, **kwargs)
        monkeypatch.setattr(c.remote.transport, 'request', held)
        operation = asyncio.create_task(action(c, 'stop', identity))
        try:
            async with asyncio.timeout(20):
                await entered.wait()
            receipt = read_receipt(c.directory)
            assert receipt['status'] == 'running' and receipt['pending_operation'] == {'id': identity, 'action': 'stop'}
            assert not native_ready(receipt, True) and _busy(c.remote, receipt)
            assert not c.browser.transport.calls and not c.remote.implementation.submit.await_count
            assert not c.owner.done() and not c.launches[0][1].done()
        finally:
            release.set()
            async with asyncio.timeout(20):
                await operation
    else:
        c.dropped.add('before:stop')
        lost = await action(c, 'stop', identity)
        assert lost['status'] == 'observation-unavailable'
        assert read_receipt(c.directory)['pending_operation'] == {'id': identity, 'action': 'stop'}
        use = await c.remote.use(c.gid, c.did)
        assert not use['native_runtime_available'] and not use['success_reporting_available'] and not use['continuation']['available']
        before = deepcopy(c.calls)
        with pytest.raises(DraftConflict):
            await c.remote.record_success(c.gid, c.state['version'], c.did, positive['build_id'],
                positive['source_digest'], positive['package_sha256'], request_id=str(uuid4()), task='Must not report.')
        with pytest.raises(DraftConflict):
            await c.remote.feedback(c.gid, c.state['version'], positive['remote_binding'],
                positive['continuation']['build_id'], positive['continuation']['source_digest'],
                request='Must not submit.', outcome='blocked')
        assert c.calls == before and not c.remote.implementation.submit.await_count
        with pytest.raises(DraftConflict):
            await open_console(c)
        assert sum(row['action'] == 'stop' for row in c.calls) == 1
        assert not c.browser.transport.calls and not c.owner.done() and not c.launches[0][1].done()


async def test_source_declared_product_custom_ui_without_listener_proof_cannot_open(protected_owner):
    c = protected_owner
    files, _, _ = c.remote.frozen(c.directory, read_receipt(c.directory))
    declared = declared_application_experiences(files, json.loads(files['deployment.json'][0]))
    assert declared['entries'] == [{'id': 'workspace', 'label': 'Application workspace', 'path': '/workspace'}]
    assert declared['metadata_sha256'] and native_ready(c.observed, True)
    receipt = read_receipt(c.directory)
    observation = receipt['application_experiences']
    assert observation['protocol'] == 1 and observation['supported'] is True and observation['status'] == 'resolved'
    assert observation['source_digest'] == receipt['accepted_source_digest']
    assert observation['installed_source_digest'] == receipt['transferred_source_digest']
    assert observation['metadata_sha256'] == declared['metadata_sha256']
    entry = selected_application_entry(receipt, 'application:workspace')
    assert entry is not None and len(observation['entries']) == 1
    assert {key: entry[key] for key in ('id', 'label', 'path')} == declared['entries'][0]
    assert entry['metadata_sha256'] == declared['metadata_sha256'] and entry['destination_port'] == c.port
    assert entry['listener_owned'] is False and entry['listener_token'] is None and entry['verified'] is False
    assert not entry_ready(receipt, True, 'application:workspace')
    with pytest.raises(DraftConflict):
        await c.browser.open(c.gid, c.state['version'], c.observed['build_id'], c.did, 'application:workspace')
    assert not c.browser.transport.calls and not c.http_checks and not c.owner.done()
    assert (c.gid, c.did, 'application:workspace') not in c.browser.records
    assert c.f.parent is c.owner and c.f.service.tasks[c.did] is c.launches[0][1] is c.emissions[0][2]
    assert not c.launches[0][1].done() and not c.f.service.processes
