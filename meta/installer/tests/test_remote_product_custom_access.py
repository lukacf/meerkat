"""Original product actor/custom-access joins with explicitly inert boundaries.

Real run_initial_install, factory/package/source reads, Prepare/Finish canonical
UUIDs, original worker Task/admission, retained private IPC, origin adopt and
protected browser readers are exercised after integration. The native child
handle, proc files, SDK/bootstrap/health specimen, tunnel and HTTP are inert.
These controls do not establish login, useful work or native/cloud operation.
"""
import asyncio
from copy import deepcopy
import os
from types import SimpleNamespace
from uuid import uuid4

import pytest

from launchkit import deployment, remote_destination as controller
from launchkit.application_access import selected_application_entry
from launchkit.projects import DraftConflict
from launchkit.remote_browser_access import entry_ready, native_ready
from launchkit.remote_deployment import read_receipt, update_receipt
import test_remote_product_access as protected_fixture
from test_remote_product_access import protected_package as base_protected_package, protected_owner
from test_remote_product_controls import (original_control_package, original_control_destination,
    original_controls, action)
from test_remote_product_control import control_destination
from test_destination_application_inputs import inputs_destination, finalize_original_destination
from test_destination_application_factory import destination
from test_product_destination_install import installation, destination_source, artifact_custody, product
from test_product_destination_owner import no_destination_execution
from test_meta_toolkit_runtime_selection import no_product_execution


def proc_process(proc, pid, parent, ticks):
    directory = proc / str(pid)
    (directory / 'ns').mkdir(parents=True)
    (directory / 'fd').mkdir()
    (directory / 'ns/net').symlink_to('net:[123]')
    fields = ['S', str(parent), str(pid), str(pid)] + ['0'] * 15 + [str(ticks)] + ['0'] * 3
    (directory / 'stat').write_text(f'{pid} (inert-original-child) ' + ' '.join(fields))
    (directory / 'cgroup').write_text('0::/same-user-shared-group\n')


@pytest.fixture
def protected_package(base_protected_package, original_controls, monkeypatch, tmp_path):
    """Add only the explicit spawned-child/kernel boundary to the existing body.

    The source package was staged by base_protected_package before the actual
    original owner entered. No capability literal or saved listener token is
    injected, and the real original _launch and its Task/admission are retained.
    """
    c, f = original_controls, original_controls.f
    proc = tmp_path / 'custom-inert-proc'
    for relative in ('self/net', 'self/ns', 'sys/kernel/random'):
        (proc / relative).mkdir(parents=True, exist_ok=True)
    (proc / 'sys/kernel/random/boot_id').write_text('11111111-1111-4111-8111-111111111111\n')
    (proc / 'self/ns/net').symlink_to('net:[123]')
    proc_process(proc, 61233, os.getpid(), 90)
    proc_process(proc, 61234, 61233, 100)
    # The holder creates another process group/session. Kernel lineage, not a
    # shared group or user's shared cgroup, must retain actual application scope.
    proc_process(proc, 61235, 61234, 110)
    socket_fd = proc / '61235/fd/7'
    socket_fd.symlink_to('socket:[9876]')
    (proc / 'self/net/tcp6').write_text('header\n')
    monkeypatch.setattr(controller, 'PROC_ROOT', proc)
    monkeypatch.setattr(controller, 'CGROUP_ROOT', tmp_path / 'custom-inert-cgroups')
    original_inert = protected_fixture.inert_worker
    def inert_child(original, patch):
        assert original is f
        launches = original_inert(original, patch)
        unspawned_body = original.service._run
        async def child_body(directory, mode, **kwargs):
            assert directory == f.directory and mode == 'start'
            task = asyncio.current_task()
            admission = f.service.worker_admissions[c.did]
            assert f.service.tasks[c.did] is task and admission['mode'] == mode
            assert admission['lifecycle_id'] == read_receipt(directory)['interactive_lifecycle_id']
            process = SimpleNamespace(pid=61233, returncode=None, stdin=None)
            c.inert_custom_process = process
            c.inert_custom_admission = admission
            c.inert_custom_task = task
            c.proc, c.socket_fd = proc, socket_fd
            admission['spawn_started'] = True
            f.service.processes[c.did] = process
            canonical = read_receipt(directory)
            port = int(canonical['console_url'].split(':')[2].split('/')[0])
            (proc / 'self/net/tcp').write_text('header\n' +
                f'0: 0100007F:{port:04X} 00000000:0000 0A 00000000:00000000 '
                f'00:00000000 00000000 {os.getuid()} 0 9876 1\n')
            try:
                await unspawned_body(directory, mode, **kwargs)
            finally:
                # Only this explicit inert edge closes. The original Task and
                # actual owner shutdown still own cancellation and its join.
                process.returncode = 0
                if f.service.processes.get(c.did) is process:
                    f.service.processes.pop(c.did)
                admission['spawn_started'] = False
        patch.setattr(original.service, '_run', child_body)
        return launches
    monkeypatch.setattr(protected_fixture, 'inert_worker', inert_child)


@pytest.fixture
async def custom_owner(protected_owner):
    c = protected_owner
    c.application_checks = []
    async def application_http(port, path):
        assert (port, path) == (c.port, '/workspace')
        c.application_checks.append((port, path))
        return 200
    c.browser.application_check = application_http
    assert 'APPLICATION_EXPERIENCES_SOURCE' not in vars(controller)
    assert c.f.service.processes[c.did] is c.inert_custom_process
    assert c.f.service.worker_admissions[c.did] is c.inert_custom_admission
    assert c.f.service.tasks[c.did] is c.inert_custom_task is c.launches[0][1]
    yield c


async def open_custom(c):
    return await c.browser.open(c.gid, c.state['version'], c.observed['build_id'], c.did,
        'application:workspace')


def assert_original(c):
    assert not c.owner.done() and c.f.parent is c.owner
    assert c.f.service.tasks[c.did] is c.inert_custom_task and not c.inert_custom_task.done()
    assert c.f.service.processes[c.did] is c.inert_custom_process
    assert c.f.service.worker_admissions[c.did] is c.inert_custom_admission
    assert len(c.launches) == len(c.f.inventory_calls) == 1
    assert c.f.factory._source is None and c.f.factory.host._product_source_custody._closed


def assert_no_owned_link(observed):
    assert not any(row.get('listener_owned') for row in
        (observed.get('application_experiences') or {}).get('entries', []))


async def test_original_actor_projects_custom_listener_and_origin_shared_console_port(custom_owner):
    c = custom_owner
    entry = selected_application_entry(c.observed, 'application:workspace')
    assert entry is not None and entry['listener_owned'] and entry['destination_port'] == c.port
    assert len(entry['listener_token']) == 64 and entry['verified'] is False
    assert c.observed['service'] == {} and c.observed['product_runtime'] is True
    assert native_ready(c.observed, True) and entry_ready(c.observed, True, 'application:workspace')
    assert c.observed['application_experiences']['source_digest'] == c.observed['accepted_source_digest']
    result = await open_custom(c)
    assert result['status'] == 'ready' and result['transport_reachable'] and result['http_status'] == 200
    assert result['local_url'] == f'http://127.0.0.1:{c.port}/workspace'
    assert result['application_verified'] is result['authentication_verified'] is result['workflow_verified'] is False
    assert c.application_checks == [(c.port, '/workspace')]
    assert len(c.browser.transport.calls) == 1 and not c.http_checks
    record = c.browser.records[(c.gid, c.did, 'application:workspace')]
    assert record['binding']['listener_token'] == entry['listener_token']
    assert_original(c)


async def test_declared_custom_metadata_without_actual_listener_holder_stays_unavailable(custom_owner):
    c = custom_owner
    c.socket_fd.unlink()
    observed = await c.remote.status(c.gid, force=True)
    assert native_ready(observed, True) and observed['application_experiences']['supported']
    assert_no_owned_link(observed)
    with pytest.raises(DraftConflict):
        await open_custom(c)
    assert not c.browser.transport.calls and not c.application_checks
    assert_original(c)


@pytest.mark.parametrize('drift', ['factory', 'installed-source', 'process-handle'])
async def test_actual_producer_rejoins_original_facts_after_package_reader_await(custom_owner, monkeypatch, drift):
    c, f = custom_owner, custom_owner.f
    entered, release = asyncio.Event(), asyncio.Event()
    original_validation = deployment.await_validation
    reads = []
    async def validation(function, *args, **kwargs):
        value = await original_validation(function, *args, **kwargs)
        if function is deployment._observe_status_package and args[0].__name__ == 'source_observation':
            reads.append(value)
            if len(reads) == 1:
                entered.set()
                await release.wait()
        return value
    monkeypatch.setattr(deployment, 'await_validation', validation)
    source = f.root / 'application/experience.json'
    source_bytes = source.read_bytes()
    factory_context = deepcopy(f.factory.context)
    pending = asyncio.create_task(c.remote.status(c.gid, force=True))
    try:
        async with asyncio.timeout(20):
            await entered.wait()
        assert f.service.lock.locked() and reads and not c.owner.done()
        if drift == 'factory':
            f.factory.context['deployment_id'] = str(uuid4())
        elif drift == 'installed-source':
            source.write_bytes(source_bytes + b'\n')
        else:
            # An equal saved PID and live-looking handle must not replace the
            # actual original reference captured before source observation.
            f.service.processes[c.did] = SimpleNamespace(pid=61233, returncode=None, stdin=None)
        release.set()
        async with asyncio.timeout(20):
            observed = await pending
        assert_no_owned_link(observed)
        assert not entry_ready(observed, True, 'application:workspace')
        assert not c.browser.transport.calls and not c.application_checks
    finally:
        release.set()
        await asyncio.gather(pending, return_exceptions=True)
        f.factory.context.clear()
        f.factory.context.update(factory_context)
        source.write_bytes(source_bytes)
        f.service.processes[c.did] = c.inert_custom_process
    assert_original(c)


@pytest.mark.parametrize('drift', ['listener-holder', 'original-process-exit'])
async def test_custom_http_await_rechecks_actual_original_listener_and_handle(custom_owner, drift):
    c = custom_owner
    entered, release = asyncio.Event(), asyncio.Event()
    async def held_http(port, path):
        assert (port, path) == (c.port, '/workspace')
        c.application_checks.append((port, path))
        entered.set()
        await release.wait()
        return 200
    c.browser.application_check = held_http
    pending = asyncio.create_task(open_custom(c))
    try:
        async with asyncio.timeout(20):
            await entered.wait()
        assert len(c.browser.transport.calls) == 1
        handle = c.browser.transport.handles[0]
        if drift == 'listener-holder':
            c.socket_fd.unlink()
        else:
            c.inert_custom_process.returncode = 0
        release.set()
        async with asyncio.timeout(20):
            result = await pending
        assert result['status'] == 'unavailable' and result['local_url'] is None
        assert not result['transport_reachable'] and result['cleanup_confirmed']
        assert handle.closes == 1 and not handle.live
        assert c.browser.records[(c.gid, c.did, 'application:workspace')]['handle'] is None
        assert c.application_checks == [(c.port, '/workspace')]
    finally:
        release.set()
        await asyncio.gather(pending, return_exceptions=True)
        c.inert_custom_process.returncode = None
    assert_original(c)
