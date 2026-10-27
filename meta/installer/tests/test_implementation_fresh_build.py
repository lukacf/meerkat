"""Fresh build admission uses real owners and inert native/build boundaries.

These are source admission regressions, not installed or provider execution.
"""
import asyncio
import json
import threading
from copy import deepcopy
from types import SimpleNamespace

import pytest

from launchkit.connect_installer_caller import CustodyError
from launchkit.implementation import update_receipt
from launchkit.implementation_workspace import ImplementationWorkspace, WorkspaceError
from launchkit.projects import DraftConflict, fingerprint
from test_local_application_registry import (
    registry, product, no_product_execution, no_processes, new_guide, build,
    workspace, host_receipt,
)
from test_retained_ordinary_application import newer_target, upgraded_registry
from test_implementation_stop_route import stop_api


async def incomplete(f):
    saved = new_guide(f)
    first = await build(f, saved, 'Old repair request must stay with the old build.')
    old = workspace(f.implementation, first)
    old.write_file('application/unfinished.txt', 'Retain this only in the previous build.\n')
    check = host_receipt(old)
    task = f.implementation.tasks.pop(first['build_id'])
    task.cancel()
    await asyncio.gather(task, return_exceptions=True)
    directory = f.implementation._directory(saved['id'])
    update_receipt(directory, status='interrupted', receipt=check, checks=check['checks'],
        ready_to_package=False, cleanup_pending=False, cleanup_in_progress=False,
        worker_stop_confirmed=True)
    return saved, first, old, directory


def successor(f, target, monkeypatch):
    implementation, deployment, owner = upgraded_registry(f, target, ())
    monkeypatch.setattr(implementation, '_run', f.implementation._run)
    return implementation, owner


async def revised_guide(f, saved, change):
    if change == 'unchanged':
        return saved
    if change == 'organization':
        f.guide.organization = {'schema_version': 1, 'id': 'workplace', 'version': '2.0',
            'label': 'Workplace', 'description': '', 'starting_examples': [],
            'guide_context': '  Reach workplace tools through Connect.\nKeep this original text.\n'}
        return await f.guide.respond(saved['id'], SimpleNamespace(version=saved['version'],
            action='organization', organization_fingerprint=fingerprint(f.guide.organization)))
    updated = deepcopy(saved)
    if change == 'proposal':
        updated['proposal']['project']['name'] = 'Revised application'
    else:
        updated['brief']['requirements'][0]['outcome'] = 'Review the revised local job.'
    return f.store.save(updated, saved['version'])


@pytest.mark.parametrize('change', ['unchanged', 'organization', 'proposal', 'brief'])
@pytest.mark.parametrize('status', ['failed', 'interrupted'])
async def test_fresh_build_selects_new_runtime_without_copying_source_proof_or_notes(
        registry, newer_target, monkeypatch, change, status):
    f = registry
    saved, first, old, directory = await incomplete(f)
    update_receipt(directory, status=status)
    original = {name: (directory / name).read_bytes() for name in ['input.json', 'receipt.json']}
    digest = old.digest()
    saved = await revised_guide(f, saved, change)
    implementation, owner = successor(f, newer_target, monkeypatch)
    options = owner.initial_build_options(saved['id'], saved['version'])
    try:
        predecessor = await implementation.status(saved['id'])
        assert predecessor['current'] is (change == 'unchanged')
        assert predecessor['source_verified'] and predecessor['source_digest'] == digest
        with pytest.raises(DraftConflict, match='Preserve the selected product runtime'):
            await implementation.submit(saved['id'], saved['version'],
                expected_selected_build_id=first['build_id'], **options)
        result = await implementation.submit(saved['id'], saved['version'], start_fresh=True,
            expected_fresh_source_digest=digest, expected_selected_build_id=first['build_id'], **options)
        new = workspace(implementation, result)
        frozen = json.loads((implementation._directory(saved['id']) / 'input.json').read_text())
        assert frozen['fresh_build_predecessor'] == {'build_id': first['build_id'], 'source_digest': digest}
        assert frozen['runtime_candidate']['assembly']['sha256'] == newer_target.assembly.sha256
        assert frozen['runtime_candidate']['assembly']['sha256'] != f.target.assembly.sha256
        assert frozen['framework']['reason'] == 'explicit-fresh-build'
        assert frozen['previous_implementation'] is None and frozen['implementation_requests'] == []
        assert frozen['request'] == '' and frozen['blueprint'] == saved['proposal']
        assert frozen['brief'] == saved['brief'] and frozen['messages'] == saved['messages']
        assert frozen.get('organization') == saved.get('organization')
        if change == 'organization':
            assert frozen['organization']['guide_context'] == f.guide.organization['guide_context']
        assert not (new.project / 'application/unfinished.txt').exists()
        assert not result['ready_to_package'] and not result['acceptance_trusted']
        assert new.verification() is None
        assert old.digest() == digest
        for name, content in original.items():
            assert (directory / name).read_bytes() == content
        with pytest.raises(DraftConflict):
            await implementation.submit(saved['id'], saved['version'], start_fresh=True,
                expected_fresh_source_digest=digest, expected_selected_build_id=first['build_id'], **options)
        assert len(implementation.tasks) == 1
    finally:
        await implementation.shutdown()


@pytest.mark.parametrize('change', ['source', 'running', 'completed', 'cleanup', 'unconfirmed'])
async def test_fresh_build_refuses_stale_or_unsettled_predecessor(registry, newer_target, monkeypatch, change):
    f = registry
    saved, first, old, directory = await incomplete(f)
    digest = old.digest()
    saved = await revised_guide(f, saved, 'organization')
    if change == 'source':
        old.write_file('application/later.txt', 'Newer unsent work.\n')
    elif change in {'running', 'completed'}:
        update_receipt(directory, status=change)
    elif change == 'cleanup':
        update_receipt(directory, cleanup_pending=True)
    else:
        update_receipt(directory, worker_stop_confirmed=False)
    implementation, owner = successor(f, newer_target, monkeypatch)
    try:
        with pytest.raises(DraftConflict):
            await implementation.submit(saved['id'], saved['version'], start_fresh=True,
                expected_fresh_source_digest=digest, expected_selected_build_id=first['build_id'],
                **owner.initial_build_options(saved['id'], saved['version']))
        assert not implementation.tasks
        assert len(list(directory.parent.glob('*/receipt.json'))) == 1
    finally:
        await implementation.shutdown()


@pytest.mark.parametrize('change', ['source', 'guide', 'cleanup', 'unconfirmed', 'framework-removed', 'owned-task'])
async def test_fresh_build_fences_changes_during_preparation(registry, newer_target, monkeypatch, change):
    f = registry
    saved, first, old, directory = await incomplete(f)
    digest = old.digest()
    saved = await revised_guide(f, saved, 'organization')
    implementation, owner = successor(f, newer_target, monkeypatch)
    entered, release = threading.Event(), threading.Event()
    original = ImplementationWorkspace.seed

    def held(new, *args, **kwargs):
        if kwargs.get('fresh_reason') == 'explicit-fresh-build':
            entered.set()
            assert release.wait(10)
        return original(new, *args, **kwargs)

    monkeypatch.setattr(ImplementationWorkspace, 'seed', held)
    task = asyncio.create_task(implementation.submit(saved['id'], saved['version'], start_fresh=True,
        expected_fresh_source_digest=digest, expected_selected_build_id=first['build_id'],
        **owner.initial_build_options(saved['id'], saved['version'])))
    try:
        assert await asyncio.to_thread(entered.wait, 10)
        if change == 'source':
            old.write_file('application/later.txt', 'Changed during preparation.\n')
        elif change == 'guide':
            await revised_guide(f, saved, 'brief')
        elif change == 'cleanup':
            update_receipt(directory, cleanup_pending=True)
        elif change == 'unconfirmed':
            update_receipt(directory, worker_stop_confirmed=False)
        elif change == 'owned-task':
            implementation.tasks[first['build_id']] = asyncio.create_task(asyncio.sleep(3600))
        else:
            with old._locked():
                stored = old._state()
                stored.pop('framework')
                old._save_state(stored)
            assert old.digest() == digest
        release.set()
        if change == 'guide':
            # The original source owner refuses the changed exact guide
            # before any new worker or receipt exists, ahead of the later
            # fence.
            with pytest.raises(CustodyError) as refused:
                await task
            assert str(refused.value) == "Initial Build requires this guide's exact current completed version."
        else:
            with pytest.raises(DraftConflict, match='changed while preparing'):
                await task
        if change == 'owned-task':
            retained = implementation.tasks.pop(first['build_id'])
            retained.cancel()
            await asyncio.gather(retained, return_exceptions=True)
        assert not implementation.tasks
        assert len(list(directory.parent.glob('*/receipt.json'))) == 1
    finally:
        release.set()
        await asyncio.gather(task, return_exceptions=True)
        await implementation.shutdown()


def test_fresh_build_route_forwards_explicit_selection_without_repair_note(stop_api):
    client, implementation = stop_api
    implementation.submit.return_value = {'build_id': 'new', 'status': 'queued'}
    result = client.post('/api/guide/guide/implementation', json={'version': 3,
        'start_fresh': True, 'previous_build_id': 'old', 'previous_source_digest': 'a' * 64})
    assert result.status_code == 202
    implementation.submit.assert_awaited_once_with('guide', 3, '', expected_selected_build_id='old',
        refresh_framework=False, start_fresh=True, expected_fresh_source_digest='a' * 64)


@pytest.mark.parametrize('change', [
    {'start_fresh': 'true'}, {'previous_source_digest': None}, {'previous_build_id': None},
    {'request': 'A new repair note'}, {'refresh_framework': True}, {'preserve_design': True},
    {'start_fresh': False},
])
def test_fresh_build_route_refuses_ambiguous_or_mixed_requests(stop_api, change):
    client, implementation = stop_api
    body = {'version': 3, 'start_fresh': True, 'previous_build_id': 'old',
        'previous_source_digest': 'a' * 64, **change}
    result = client.post('/api/guide/guide/implementation', json=body)
    assert result.status_code == 422
    implementation.submit.assert_not_awaited()


async def incomplete_publication(f, check_status='fail', tests=8, status='failed'):
    saved, first, old, directory = await incomplete(f)
    # The real workspace owner signs the inert checker result. A failure or
    # zero executed tests cannot make this source publishable.
    check = host_receipt(old, status=check_status, tests=tests)
    update_receipt(directory, status=status, receipt=check, checks=check['checks'])
    return saved, first, old, directory


@pytest.mark.parametrize('check_status,tests,status', [
    ('fail', 8, 'failed'), ('fail', 0, 'interrupted'), ('pass', 0, 'failed'),
])
async def test_fresh_build_retains_authenticated_source_that_failed_publication(
        registry, newer_target, monkeypatch, check_status, tests, status):
    f = registry
    saved, first, old, directory = await incomplete_publication(f, check_status, tests, status)
    digest = old.digest()
    original = {path: path.read_bytes() for path in
        [directory / 'input.json', directory / 'receipt.json', old._state_path]}
    implementation, owner = successor(f, newer_target, monkeypatch)
    try:
        before = await implementation.status(saved['id'])
        assert before['source_snapshot_verified'] is True
        assert before['source_verified'] is False and before['ready_to_package'] is False
        assert before['source_digest'] == digest
        result = await implementation.submit(saved['id'], saved['version'], start_fresh=True,
            expected_fresh_source_digest=digest, expected_selected_build_id=first['build_id'],
            **owner.initial_build_options(saved['id'], saved['version']))
        new = workspace(implementation, result)
        frozen = json.loads((implementation._directory(saved['id']) / 'input.json').read_text())
        assert frozen['fresh_build_predecessor'] == {'build_id': first['build_id'], 'source_digest': digest}
        assert frozen['blueprint'] == saved['proposal'] and frozen['brief'] == saved['brief']
        assert frozen['messages'] == saved['messages'] and frozen['request'] == ''
        assert frozen['previous_implementation'] is None and frozen['implementation_requests'] == []
        assert not (new.project / 'application/unfinished.txt').exists()
        assert new.verification() is None
        assert result['source_snapshot_verified'] is False and result['source_verified'] is False
        assert not result['ready_to_package'] and not result['acceptance_trusted']
        assert old.digest() == digest
        for path, content in original.items():
            assert path.read_bytes() == content
        assert len(implementation.tasks) == 1
    finally:
        await implementation.shutdown()


@pytest.mark.parametrize('change', ['missing-proof', 'bad-signature', 'edited-after-check'])
async def test_fresh_build_keeps_unchecked_snapshot_without_promoting_or_copying_it(
        registry, newer_target, monkeypatch, change):
    f = registry
    saved, first, old, directory = await incomplete_publication(f)
    if change == 'edited-after-check':
        old.write_file('application/later.txt', 'Last worker edits before timeout.\n')
    else:
        with old._locked():
            stored = old._state()
            if change == 'missing-proof':
                stored['last_check'] = None
            else:
                stored['last_check']['signature'] = '0' * 64
            old._save_state(stored)
    digest = old.digest()
    original = {path: path.read_bytes() for path in
        [directory / 'input.json', directory / 'receipt.json', old._state_path]}
    original_source = {item.path: item.as_bytes() for item in old.export_files(require_verified=False)}
    implementation, owner = successor(f, newer_target, monkeypatch)
    try:
        before = await implementation.status(saved['id'])
        assert before['source_snapshot_verified'] is False and before['source_verified'] is False
        assert before['source_digest'] == digest
        assert before['ready_to_package'] is False and before['acceptance_trusted'] is False
        with pytest.raises(WorkspaceError, match='passing executable checks'):
            old.export_files()
        result = await implementation.submit(saved['id'], saved['version'], start_fresh=True,
            expected_fresh_source_digest=digest, expected_selected_build_id=first['build_id'],
            **owner.initial_build_options(saved['id'], saved['version']))
        new = workspace(implementation, result)
        frozen = json.loads((implementation._directory(saved['id']) / 'input.json').read_text())
        assert frozen['fresh_build_predecessor'] == {'build_id': first['build_id'], 'source_digest': digest}
        assert frozen['previous_implementation'] is None and frozen['implementation_requests'] == []
        assert frozen['request'] == '' and frozen['messages'] == saved['messages']
        assert not (new.project / 'application/unfinished.txt').exists()
        assert not (new.project / 'application/later.txt').exists()
        assert new.verification() is None and old.verification() is None
        assert not result['source_verified'] and not result['ready_to_package']
        assert not result['acceptance_trusted']
        assert old.digest() == digest
        assert {item.path: item.as_bytes() for item in old.export_files(require_verified=False)} == original_source
        for path, content in original.items():
            assert path.read_bytes() == content
        with pytest.raises(WorkspaceError, match='passing executable checks'):
            old.export_files()
        assert len(implementation.tasks) == 1
    finally:
        await implementation.shutdown()


@pytest.mark.parametrize('change', ['framework-signature', 'protected-file', 'symlink'])
async def test_fresh_build_refuses_invalid_workspace_custody_without_a_check(
        registry, newer_target, monkeypatch, change):
    f = registry
    saved, first, old, directory = await incomplete_publication(f)
    digest = old.digest()
    with old._locked():
        stored = old._state()
        stored['last_check'] = None
        if change == 'framework-signature':
            stored['framework']['signature'] = '0' * 64
        old._save_state(stored)
    if change == 'protected-file':
        (old.project / 'requirements.txt').write_text('Unowned framework replacement.\n')
    elif change == 'symlink':
        (old.project / 'application/link.txt').symlink_to(old.project / 'application/unfinished.txt')
    implementation, owner = successor(f, newer_target, monkeypatch)
    try:
        with pytest.raises(WorkspaceError):
            await implementation.submit(saved['id'], saved['version'], start_fresh=True,
                expected_fresh_source_digest=digest, expected_selected_build_id=first['build_id'],
                **owner.initial_build_options(saved['id'], saved['version']))
        assert not implementation.tasks
        assert len(list(directory.parent.glob('*/receipt.json'))) == 1
    finally:
        await implementation.shutdown()


@pytest.mark.parametrize('change', ['missing-check', 'framework-signature'])
async def test_fresh_build_rechecks_custody_without_requiring_check_after_preparation(
        registry, newer_target, monkeypatch, change):
    f = registry
    saved, first, old, directory = await incomplete_publication(f)
    digest = old.digest()
    implementation, owner = successor(f, newer_target, monkeypatch)
    entered, release = threading.Event(), threading.Event()
    original = ImplementationWorkspace.seed

    def held(new, *args, **kwargs):
        if kwargs.get('fresh_reason') == 'explicit-fresh-build':
            entered.set()
            assert release.wait(10)
        return original(new, *args, **kwargs)

    monkeypatch.setattr(ImplementationWorkspace, 'seed', held)
    task = asyncio.create_task(implementation.submit(saved['id'], saved['version'], start_fresh=True,
        expected_fresh_source_digest=digest, expected_selected_build_id=first['build_id'],
        **owner.initial_build_options(saved['id'], saved['version'])))
    try:
        assert await asyncio.to_thread(entered.wait, 10)
        with old._locked():
            stored = old._state()
            if change == 'missing-check':
                stored['last_check'] = None
            else:
                stored['framework']['signature'] = '0' * 64
            old._save_state(stored)
        retained_state = old._state_path.read_bytes()
        release.set()
        if change == 'missing-check':
            result = await task
            assert result['build_id'] != first['build_id']
            assert not result['ready_to_package'] and not result['source_verified']
            assert old.digest() == digest and old.verification() is None
            assert old._state_path.read_bytes() == retained_state
            assert len(implementation.tasks) == 1
        else:
            with pytest.raises(WorkspaceError, match='framework provenance'):
                await task
            assert not implementation.tasks
            assert len(list(directory.parent.glob('*/receipt.json'))) == 1
    finally:
        release.set()
        await asyncio.gather(task, return_exceptions=True)
        await implementation.shutdown()
