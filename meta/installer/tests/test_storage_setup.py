"""Storage initialization keeps native schema authority and existing-state boundaries."""
import importlib.util
import json
import os
from pathlib import Path
import subprocess
from types import SimpleNamespace

import pytest


@pytest.fixture
def storage():
    path = Path(__file__).parents[1] / 'launchkit/templates/storage_setup.py'
    spec = importlib.util.spec_from_file_location('generated_storage_setup', path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


@pytest.mark.asyncio
async def test_fresh_initialization_stops_runtime_before_native_migration(tmp_path, storage, monkeypatch):
    (tmp_path / 'deployment.json').write_text(json.dumps({'slug': 'test-team', 'mobkitVersion': '0.8.42'}))
    events = []
    class Runtime:
        async def identity_bootstrap_status(self):
            return SimpleNamespace(complete=True, ready=True, counts=SimpleNamespace(broken=0))
        async def shutdown(self):
            events.append('stopped')
    async def build():
        events.append('started')
        (tmp_path / '.mobkit/state/continuity.sqlite3').touch()
        return Runtime()
    def migrate(root, *, apply):
        if apply:
            assert events == ['started', 'stopped']
            events.append('native-migrate')
        else:
            events.append('native-inspect')
        return {'ledger': [{'domain': 'mobkit-continuity', 'after': 2}]}
    monkeypatch.setattr(storage, 'migration', migrate)
    await storage.prepare_storage(tmp_path, build)
    assert events == ['started', 'stopped', 'native-migrate']
    await storage.prepare_storage(tmp_path, build)
    assert events[-1] == 'native-inspect', 'Ready projects only inspect at ordinary startup'
    marker = tmp_path / '.mobkit/launch-storage.json'
    assert json.loads(marker.read_text())['phase'] == 'ready'
    assert marker.stat().st_mode & 0o777 == 0o600


@pytest.mark.asyncio
async def test_imported_legacy_state_is_never_automatically_migrated(tmp_path, storage, monkeypatch):
    (tmp_path / 'deployment.json').write_text(json.dumps({'slug': 'test-team', 'mobkitVersion': '0.8.42'}))
    state = tmp_path / '.mobkit/state'
    state.mkdir(parents=True)
    (state / 'continuity.sqlite3').write_text('existing-owner-data')
    calls = []
    def migrate(root, *, apply):
        calls.append(apply)
        return {'ledger': [{'domain': 'mobkit-continuity', 'before': 1}]}
    monkeypatch.setattr(storage, 'migration', migrate)
    with pytest.raises(RuntimeError, match='back up'):
        await storage.prepare_storage(tmp_path, None)
    assert calls == [False]
    assert (state / 'continuity.sqlite3').read_text() == 'existing-owner-data'
    assert not (tmp_path / '.mobkit/launch-storage.json').exists()


@pytest.mark.asyncio
async def test_interrupted_new_project_resumes_native_initialization(tmp_path, storage, monkeypatch):
    (tmp_path / 'deployment.json').write_text(json.dumps({'slug': 'test-team', 'mobkitVersion': '0.8.42'}))
    state = tmp_path / '.mobkit/state'
    state.mkdir(parents=True)
    (state / 'continuity.sqlite3').write_text('pending')
    storage.write_private(tmp_path / '.mobkit/launch-storage.json', {'format': 1, 'phase': 'preparing', 'project': 'test-team', 'version': '0.8.42'})
    def migrate(root, *, apply):
        assert apply
        return {'ledger': [{'domain': 'mobkit-continuity', 'after': 2}]}
    monkeypatch.setattr(storage, 'migration', migrate)
    await storage.prepare_storage(tmp_path, None)
    assert json.loads((tmp_path / '.mobkit/launch-storage.json').read_text())['phase'] == 'ready'


def test_second_process_cannot_prepare_or_serve_same_project(tmp_path, storage):
    with storage.project_lock(tmp_path):
        with pytest.raises(RuntimeError, match='already running'):
            with storage.project_lock(tmp_path):
                pytest.fail('Concurrent project owner was admitted')
    with storage.project_lock(tmp_path):
        pass


@pytest.mark.asyncio
async def test_failed_boot_stops_and_does_not_mark_ready(tmp_path, storage):
    (tmp_path / 'deployment.json').write_text(json.dumps({'slug': 'test-team', 'mobkitVersion': '0.8.42'}))
    stopped=[]
    class Runtime:
        async def identity_bootstrap_status(self):
            return SimpleNamespace(complete=True, ready=False, counts=SimpleNamespace(broken=1))
        async def shutdown(self):
            stopped.append(True)
    async def build():
        return Runtime()
    with pytest.raises(RuntimeError, match='Initial agent setup'):
        await storage.prepare_storage(tmp_path, build)
    assert stopped == [True]
    assert json.loads((tmp_path / '.mobkit/launch-storage.json').read_text())['phase'] == 'preparing'


@pytest.mark.asyncio
@pytest.mark.parametrize('value', [None, False, 0, [], {}, {'format': 99, 'phase': 'ready'}])
async def test_malformed_marker_is_refused(tmp_path, storage, value):
    (tmp_path / 'deployment.json').write_text(json.dumps({'slug': 'test-team', 'mobkitVersion': '0.8.42'}))
    directory = tmp_path / '.mobkit'
    directory.mkdir()
    (directory / 'launch-storage.json').write_text(json.dumps(value))
    with pytest.raises(RuntimeError, match='Unrecognized'):
        await storage.prepare_storage(tmp_path, None)


@pytest.mark.asyncio
async def test_ready_marker_with_restored_legacy_state_only_inspects(tmp_path, storage, monkeypatch):
    (tmp_path / 'deployment.json').write_text(json.dumps({'slug': 'test-team', 'mobkitVersion': '0.8.42'}))
    state = tmp_path / '.mobkit/state'
    state.mkdir(parents=True)
    (state / 'continuity.sqlite3').write_text('restored-state')
    storage.write_private(tmp_path / '.mobkit/launch-storage.json', {'format': 1, 'phase': 'ready', 'project': 'test-team', 'version': '0.8.42'})
    calls=[]
    def migrate(root, *, apply):
        calls.append(apply)
        return {'ledger': [{'domain': 'mobkit-continuity', 'before': 1}]}
    monkeypatch.setattr(storage, 'migration', migrate)
    with pytest.raises(RuntimeError, match='explicit MobKit migration'):
        await storage.prepare_storage(tmp_path, None)
    assert calls == [False]


@pytest.mark.asyncio
async def test_ready_marker_without_state_does_not_skip_initialization(tmp_path, storage):
    (tmp_path / 'deployment.json').write_text(json.dumps({'slug': 'test-team', 'mobkitVersion': '0.8.42'}))
    (tmp_path / '.mobkit').mkdir()
    storage.write_private(tmp_path / '.mobkit/launch-storage.json', {'format': 1, 'phase': 'ready', 'project': 'test-team', 'version': '0.8.42'})
    async def build():
        raise RuntimeError('initializer was reached')
    with pytest.raises(RuntimeError, match='initializer was reached'):
        await storage.prepare_storage(tmp_path, build)
    assert json.loads((tmp_path / '.mobkit/launch-storage.json').read_text())['phase'] == 'preparing'


@pytest.fixture
def upgrade_project(tmp_path, storage):
    (tmp_path / 'deployment.json').write_text(json.dumps({'slug': 'test-team', 'mobkitVersion': '0.8.46'}))
    (tmp_path / '.mobkit').mkdir(mode=0o700)
    state = tmp_path / '.mobkit/state'
    state.mkdir(parents=True)
    (state / 'continuity.sqlite3').write_bytes(b'original private admission and session history')
    marker = tmp_path / '.mobkit/launch-storage.json'
    storage.write_private(marker, {'format': 1, 'phase': 'ready', 'project': 'test-team', 'version': '0.8.42'})
    (tmp_path / '.bin').mkdir()
    (tmp_path / '.bin/mobkit_gateway').write_bytes(b'mocked native executable; never run')
    return tmp_path, marker, marker.read_bytes()


def native_upgrade_report(root, *, apply=False, version=2):
    """Use the shipped native report vocabulary, including already-current blobs."""
    entry = {'database': str(root / '.mobkit/state/continuity.sqlite3'),
             'domain': 'mobkit-continuity', 'before': version,
             'action': 'already-current' if apply else 'recorded'}
    if apply:
        entry['after'] = version
    return {'mode': 'apply' if apply else 'dry_run', 'state_dir': str(root / '.mobkit/state'),
            'ledger': [entry], 'errors': [], 'renames': [], 'twins': [],
            'head_canonical_backfill': {'examined': 7, 'converted': [], 'reconverted': [],
                'failures': [], 'vanished': [], 'skipped_unparseable': [],
                'applied': apply, 'ledger_stamped': apply}}


def mock_native(monkeypatch, storage, reports, *, effect=None, returncodes=None):
    calls = []
    def run(command, **kwargs):
        calls.append(command)
        if effect:
            effect(command, len(calls))
        report = reports[len(calls) - 1]
        return SimpleNamespace(stdout=json.dumps(report), returncode=returncodes[len(calls) - 1] if returncodes else 0)
    monkeypatch.setattr(storage.subprocess, 'run', run)
    return calls


def test_explicit_upgrade_with_current_native_ledger_only_reads_and_retains_report(upgrade_project, storage, monkeypatch):
    root, marker, original = upgrade_project
    report = native_upgrade_report(root)
    calls = mock_native(monkeypatch, storage, [report])
    with storage.project_lock(root):
        result = storage.upgrade_storage(root, '0.8.42', '0.8.46')
    assert calls == [[str(root / '.bin/mobkit_gateway'), 'storage-migrate', '--state-dir',
                      str(root / '.mobkit/state'), '--json']]
    assert result['applied'] is False and result['apply_report'] is None
    assert json.loads(Path(result['read_report']).read_text()) == report
    assert Path(result['read_report']).stat().st_mode & 0o777 == 0o600
    assert json.loads(marker.read_text()) == result['marker']
    assert json.loads(original)['version'] == '0.8.42'
    assert marker.stat().st_mode & 0o777 == 0o600
    assert (root / '.mobkit/state/continuity.sqlite3').read_bytes() == b'original private admission and session history'


def test_explicit_upgrade_applies_native_required_crossing_before_marker_rebind(upgrade_project, storage, monkeypatch):
    root, marker, original = upgrade_project
    read = native_upgrade_report(root, version=1)
    applied = native_upgrade_report(root, apply=True)
    applied['ledger'][0].update(before=1, action='stamped')
    applied['head_canonical_backfill'].update(converted=['new-head'], reconverted=['corrected-head'])
    def effect(command, number):
        assert marker.read_bytes() == original
        if number == 2:
            assert command[-1] == '--apply'
            with (root / '.mobkit/state/continuity.sqlite3').open('ab') as stream:
                stream.write(b'; native-owned migration')
    calls = mock_native(monkeypatch, storage, [read, applied], effect=effect)
    with storage.project_lock(root):
        result = storage.upgrade_storage(root, '0.8.42', '0.8.46')
    assert len(calls) == 2 and result['applied'] is True
    assert json.loads(Path(result['read_report']).read_text()) == read
    assert json.loads(Path(result['apply_report']).read_text()) == applied
    assert json.loads(marker.read_text())['version'] == '0.8.46'
    assert (root / '.mobkit/state/continuity.sqlite3').read_bytes().startswith(b'original private admission')


@pytest.mark.parametrize('from_version,to_version', [('0.8.42', '0.8.49'), ('0.8.46', '0.8.42'),
                                                   ('0.8.42', '0.8.42'), (None, '0.8.46'),
                                                   ('0.8.48', '0.8.47'), ('0.8.47', '0.8.47'),
                                                   ('0.8.48', '0.8.48')])
def test_unsupported_upgrade_tuple_never_calls_native(upgrade_project, storage, monkeypatch, from_version, to_version):
    root, marker, original = upgrade_project
    calls = mock_native(monkeypatch, storage, [])
    with pytest.raises(RuntimeError, match='not supported'):
        storage.upgrade_storage(root, from_version, to_version)
    assert not calls and marker.read_bytes() == original


@pytest.mark.parametrize('replacement', [None, [], {}, {'format': True, 'phase': 'ready', 'project': 'test-team', 'version': '0.8.42'},
    {'format': 1, 'phase': 'preparing', 'project': 'test-team', 'version': '0.8.42'},
    {'format': 1, 'phase': 'unknown', 'project': 'test-team', 'version': '0.8.42'},
    {'format': 1, 'phase': 'ready', 'project': 'another-team', 'version': '0.8.42'},
    {'format': 1, 'phase': 'ready', 'project': 'test-team', 'version': '0.8.46'},
    {'format': 1, 'phase': 'ready', 'project': 'test-team', 'version': '0.8.42', 'foreign': True}, 'malformed', 'duplicate'])
def test_upgrade_requires_original_ready_project_marker_before_native(upgrade_project, storage, monkeypatch, replacement):
    root, marker, _ = upgrade_project
    if replacement == 'duplicate':
        marker.write_text('{"format":1,"phase":"ready","project":"test-team","version":"0.8.47","version":"0.8.42"}')
    else:
        marker.write_text('{bad json' if replacement == 'malformed' else json.dumps(replacement))
    original = marker.read_bytes()
    calls = mock_native(monkeypatch, storage, [])
    with pytest.raises(RuntimeError, match='Unrecognized'):
        storage.upgrade_storage(root, '0.8.42', '0.8.46')
    assert not calls and marker.read_bytes() == original


@pytest.mark.parametrize('definition', [{'slug': 'test-team', 'mobkitVersion': '0.8.42'},
                                       {'slug': '../another-team', 'mobkitVersion': '0.8.46'},
                                       {'slug': 'another-team', 'mobkitVersion': '0.8.46'}, []])
def test_upgrade_requires_same_valid_desired_project_and_new_version(upgrade_project, storage, monkeypatch, definition):
    root, marker, original = upgrade_project
    (root / 'deployment.json').write_text(json.dumps(definition))
    calls = mock_native(monkeypatch, storage, [])
    with pytest.raises(RuntimeError, match='Unrecognized'):
        storage.upgrade_storage(root, '0.8.42', '0.8.46')
    assert not calls and marker.read_bytes() == original


@pytest.mark.parametrize('kind', ['absent-marker', 'empty-state', 'marker-link', 'marker-hardlink'])
def test_upgrade_refuses_missing_or_unowned_marker_and_absent_existing_state(upgrade_project, storage, monkeypatch, kind):
    root, marker, original = upgrade_project
    if kind == 'absent-marker':
        marker.unlink()
    elif kind == 'empty-state':
        (root / '.mobkit/state/continuity.sqlite3').unlink()
    elif kind == 'marker-link':
        saved = root / 'original-marker'
        marker.rename(saved)
        marker.symlink_to(saved)
    else:
        os.link(marker, root / 'original-marker')
    calls = mock_native(monkeypatch, storage, [])
    with pytest.raises(RuntimeError, match='Unrecognized'):
        storage.upgrade_storage(root, '0.8.42', '0.8.46')
    assert not calls
    if marker.exists():
        assert marker.read_bytes() == original


@pytest.mark.parametrize('case', ['missing-ledger', 'empty-ledger', 'missing-errors', 'native-error', 'missing-backfill',
    'partial-backfill', 'skipped-backfill', 'vanished-backfill', 'false-version', 'future-version', 'duplicate-row',
    'foreign-state', 'foreign-database', 'unknown-action', 'wrong-mode'])
def test_incomplete_or_foreign_native_read_never_rebinds_marker(upgrade_project, storage, monkeypatch, case):
    root, marker, original = upgrade_project
    report = native_upgrade_report(root)
    if case == 'missing-ledger': report.pop('ledger')
    elif case == 'empty-ledger': report['ledger'] = []
    elif case == 'missing-errors': report.pop('errors')
    elif case == 'native-error': report['errors'] = ['native fence refused']
    elif case == 'missing-backfill': report.pop('head_canonical_backfill')
    elif case == 'partial-backfill': report['head_canonical_backfill']['failures'] = [['session', 'failed']]
    elif case == 'skipped-backfill': report['head_canonical_backfill']['skipped_unparseable'] = ['session']
    elif case == 'vanished-backfill': report['head_canonical_backfill']['vanished'] = ['session']
    elif case == 'false-version': report['ledger'][0]['before'] = True
    elif case == 'future-version': report['ledger'][0]['before'] = 99
    elif case == 'duplicate-row': report['ledger'].append(dict(report['ledger'][0]))
    elif case == 'foreign-state': report['state_dir'] = str(root / 'other-state')
    elif case == 'foreign-database': report['ledger'][0]['database'] = str(root / '.mobkit/state/../foreign.db')
    elif case == 'unknown-action': report['ledger'][0]['action'] = 'unknown'
    elif case == 'wrong-mode': report['mode'] = 'apply'
    calls = mock_native(monkeypatch, storage, [report])
    with pytest.raises(RuntimeError):
        with storage.project_lock(root):
            storage.upgrade_storage(root, '0.8.42', '0.8.46')
    assert len(calls) == 1 and marker.read_bytes() == original
    paths = list((root / '.mobkit/storage-upgrades').glob('*/read.json'))
    assert len(paths) == 1 and json.loads(paths[0].read_text()) == report


@pytest.mark.parametrize('case', ['missing-ledger', 'legacy-row', 'second-legacy-row', 'unstamped', 'not-applied',
                                'partial-backfill', 'native-error', 'wrong-mode'])
def test_failed_native_apply_keeps_old_marker_and_all_native_reports(upgrade_project, storage, monkeypatch, case):
    root, marker, original = upgrade_project
    read, applied = native_upgrade_report(root, version=1), native_upgrade_report(root, apply=True)
    if case == 'missing-ledger': applied.pop('ledger')
    elif case == 'legacy-row': applied['ledger'][0]['after'] = 1
    elif case == 'second-legacy-row':
        applied['ledger'].append({**applied['ledger'][0], 'database': str(root / '.mobkit/state/legacy.db'), 'after': 1})
    elif case == 'unstamped': applied['head_canonical_backfill']['ledger_stamped'] = False
    elif case == 'not-applied': applied['head_canonical_backfill']['applied'] = False
    elif case == 'partial-backfill': applied['head_canonical_backfill']['failures'] = [['session', 'failed']]
    elif case == 'native-error': applied['errors'] = ['native migration failed']
    elif case == 'wrong-mode': applied['mode'] = 'dry_run'
    calls = mock_native(monkeypatch, storage, [read, applied])
    with pytest.raises(RuntimeError):
        with storage.project_lock(root):
            storage.upgrade_storage(root, '0.8.42', '0.8.46')
    assert len(calls) == 2 and calls[1][-1] == '--apply'
    assert marker.read_bytes() == original
    paths = list((root / '.mobkit/storage-upgrades').glob('*/apply.json'))
    assert len(paths) == 1 and json.loads(paths[0].read_text()) == applied


@pytest.mark.parametrize('failed_apply', [False, True])
def test_native_nonzero_exit_cannot_authorize_marker_even_with_success_shaped_json(upgrade_project, storage, monkeypatch, failed_apply):
    root, marker, original = upgrade_project
    reports = [native_upgrade_report(root, version=1), native_upgrade_report(root, apply=True)] if failed_apply else [native_upgrade_report(root)]
    calls = mock_native(monkeypatch, storage, reports, returncodes=[0, 2] if failed_apply else [2])
    with pytest.raises(RuntimeError, match='needs attention'):
        with storage.project_lock(root):
            storage.upgrade_storage(root, '0.8.42', '0.8.46')
    assert len(calls) == len(reports) and marker.read_bytes() == original
    phase = 'apply' if failed_apply else 'read'
    retained = list((root / '.mobkit/storage-upgrades').glob(f'*/{phase}.json'))
    assert len(retained) == 1 and json.loads(retained[0].read_text()) == reports[-1]


def test_one_current_continuity_row_does_not_hide_another_pending_native_row(upgrade_project, storage, monkeypatch):
    root, marker, _ = upgrade_project
    read, applied = native_upgrade_report(root), native_upgrade_report(root, apply=True)
    legacy = {**read['ledger'][0], 'database': str(root / '.mobkit/state/legacy.sqlite3'), 'before': 1}
    read['ledger'].append(legacy)
    applied['ledger'].append({**legacy, 'after': 2, 'action': 'stamped'})
    calls = mock_native(monkeypatch, storage, [read, applied])
    with storage.project_lock(root):
        result = storage.upgrade_storage(root, '0.8.42', '0.8.46')
    assert len(calls) == 2 and result['applied'] is True
    assert json.loads(marker.read_text())['version'] == '0.8.46'


def test_native_would_stamp_another_domain_requires_apply_even_with_current_continuity(upgrade_project, storage, monkeypatch):
    root, _, _ = upgrade_project
    read, applied = native_upgrade_report(root), native_upgrade_report(root, apply=True)
    pending = {'database': str(root / '.mobkit/state/metadata.sqlite3'), 'domain': 'mobkit-metadata', 'action': 'would-stamp'}
    read['ledger'].append(pending)
    applied['ledger'].append({**pending, 'after': 1, 'action': 'stamped'})
    calls = mock_native(monkeypatch, storage, [read, applied])
    with storage.project_lock(root):
        result = storage.upgrade_storage(root, '0.8.42', '0.8.46')
    assert len(calls) == 2 and result['applied'] is True


@pytest.mark.parametrize('domain,before,ceiling', [('mobkit-console', 1, 3),
                                                 ('mobkit-console', 2, 3),
                                                 ('mobkit-memory', 2, 3), ('jobs', 3, 4)])
def test_recorded_pending_native_domain_requires_apply_with_current_continuity(upgrade_project, storage, monkeypatch, domain, before, ceiling):
    root, marker, original = upgrade_project
    read, applied = native_upgrade_report(root), native_upgrade_report(root, apply=True)
    pending = {'database': str(root / '.mobkit/state/domain.sqlite3'), 'domain': domain,
               'before': before, 'action': 'recorded'}
    read['ledger'].append(pending)
    applied['ledger'].append({**pending, 'after': ceiling, 'action': 'stamped'})
    calls = mock_native(monkeypatch, storage, [read, applied],
                        effect=lambda command, number: marker.read_bytes() == original or pytest.fail('marker rebound before native migration'))
    with storage.project_lock(root):
        result = storage.upgrade_storage(root, '0.8.42', '0.8.46')
    assert len(calls) == 2 and calls[1][-1] == '--apply' and result['applied'] is True
    assert json.loads(marker.read_text())['version'] == '0.8.46'


@pytest.mark.parametrize('domain,ceiling', [('mobkit-continuity', 2), ('mobkit-metadata', 1),
                                          ('mobkit-console', 3), ('mobkit-memory', 3), ('jobs', 4)])
@pytest.mark.parametrize('phase', ['future-read', 'incomplete-apply'])
def test_every_stampable_native_domain_requires_exact_shipped_current_ceiling(upgrade_project, storage, monkeypatch, domain, ceiling, phase):
    root, marker, original = upgrade_project
    read, applied = native_upgrade_report(root), native_upgrade_report(root, apply=True)
    row = {'database': str(root / '.mobkit/state/domain.sqlite3'), 'domain': domain,
           'before': ceiling + 1 if phase == 'future-read' else None,
           'action': 'recorded' if phase == 'future-read' else 'would-stamp'}
    read['ledger'].append(row)
    applied['ledger'].append({**row, 'after': ceiling - 1, 'action': 'stamped'})
    calls = mock_native(monkeypatch, storage, [read, applied])
    with pytest.raises(RuntimeError, match='complete upgrade storage'):
        with storage.project_lock(root):
            storage.upgrade_storage(root, '0.8.42', '0.8.46')
    assert len(calls) == (1 if phase == 'future-read' else 2)
    assert marker.read_bytes() == original


def test_core_report_only_versions_remain_native_authority(upgrade_project, storage, monkeypatch):
    root, _, _ = upgrade_project
    report = native_upgrade_report(root)
    report['ledger'].append({'database': str(root / '.mobkit/state/core.sqlite3'),
                            'domain': 'core-owned-domain', 'before': 9, 'action': 'report-only'})
    calls = mock_native(monkeypatch, storage, [report])
    with storage.project_lock(root):
        result = storage.upgrade_storage(root, '0.8.42', '0.8.46')
    assert len(calls) == 1 and result['applied'] is False


@pytest.mark.parametrize('failure', ['timeout', 'interruption'])
def test_interrupted_native_apply_retains_original_admission_and_old_marker(upgrade_project, storage, monkeypatch, failure):
    root, marker, original = upgrade_project
    def effect(command, number):
        if number == 2:
            with (root / '.mobkit/state/continuity.sqlite3').open('ab') as stream:
                stream.write(b'; native partially advanced')
            if failure == 'timeout':
                raise subprocess.TimeoutExpired(command, 120)
            raise KeyboardInterrupt
    calls = mock_native(monkeypatch, storage, [native_upgrade_report(root, version=1)], effect=effect)
    with pytest.raises(RuntimeError if failure == 'timeout' else KeyboardInterrupt):
        with storage.project_lock(root):
            storage.upgrade_storage(root, '0.8.42', '0.8.46')
    assert len(calls) == 2 and marker.read_bytes() == original
    assert (root / '.mobkit/state/continuity.sqlite3').read_bytes() == b'original private admission and session history; native partially advanced'
    assert len(list((root / '.mobkit/storage-upgrades').glob('*/read.json'))) == 1


def test_marker_write_interruption_recovers_from_new_native_read_without_reapplying(upgrade_project, storage, monkeypatch):
    root, marker, original = upgrade_project
    replace = storage.os.replace
    def interrupted(source, destination):
        if Path(destination) == marker:
            raise OSError('interrupted marker write')
        replace(source, destination)
    monkeypatch.setattr(storage.os, 'replace', interrupted)
    first = mock_native(monkeypatch, storage, [native_upgrade_report(root, version=1), native_upgrade_report(root, apply=True)])
    with pytest.raises(OSError, match='interrupted'):
        with storage.project_lock(root):
            storage.upgrade_storage(root, '0.8.42', '0.8.46')
    assert len(first) == 2 and marker.read_bytes() == original
    assert not list(marker.parent.glob('.launch-storage-upgrade-*'))
    monkeypatch.setattr(storage.os, 'replace', replace)
    second = mock_native(monkeypatch, storage, [native_upgrade_report(root)])
    with storage.project_lock(root):
        result = storage.upgrade_storage(root, '0.8.42', '0.8.46')
    assert len(second) == 1 and result['applied'] is False
    assert len(list((root / '.mobkit/storage-upgrades').glob('*/read.json'))) == 2
    assert len(list((root / '.mobkit/storage-upgrades').glob('*/apply.json'))) == 1


def test_marker_changed_during_native_read_is_not_overwritten(upgrade_project, storage, monkeypatch):
    root, marker, _ = upgrade_project
    replacement = b'{"foreign":"new owner marker"}'
    calls = mock_native(monkeypatch, storage, [native_upgrade_report(root)],
                        effect=lambda command, number: marker.write_bytes(replacement))
    with pytest.raises(RuntimeError, match='marker changed'):
        with storage.project_lock(root):
            storage.upgrade_storage(root, '0.8.42', '0.8.46')
    assert len(calls) == 1 and marker.read_bytes() == replacement


@pytest.mark.asyncio
async def test_ordinary_prepare_still_refuses_version_change_without_explicit_upgrade(upgrade_project, storage, monkeypatch):
    root, marker, original = upgrade_project
    calls = mock_native(monkeypatch, storage, [])
    with pytest.raises(RuntimeError, match='Unrecognized'):
        await storage.prepare_storage(root, None)
    assert not calls and marker.read_bytes() == original


@pytest.mark.parametrize("from_version", ["0.8.42", "0.8.46"])
@pytest.mark.parametrize("requires_apply", [False, True])
def test_qualified_047_upgrade_retains_private_history_and_uses_only_native_migration(
        upgrade_project, storage, monkeypatch, from_version, requires_apply):
    root, marker, _ = upgrade_project
    (root / "deployment.json").write_text(json.dumps({"slug": "test-team", "mobkitVersion": "0.8.47"}))
    storage.write_private(marker, {"format": 1, "phase": "ready", "project": "test-team", "version": from_version})
    original_marker = marker.read_bytes()
    state = root / ".mobkit/state/continuity.sqlite3"
    original, inode = state.read_bytes(), state.stat().st_ino
    read = native_upgrade_report(root, version=1 if requires_apply else 2)
    reports = [read, native_upgrade_report(root, apply=True)] if requires_apply else [read]
    calls = mock_native(monkeypatch, storage, reports, effect=lambda command, number: (
        None if marker.read_bytes() == original_marker else pytest.fail("Marker changed before native report")))
    with storage.project_lock(root):
        result = storage.upgrade_storage(root, from_version, "0.8.47")
    assert len(calls) == (2 if requires_apply else 1)
    assert result["applied"] is requires_apply
    assert result["from_version"] == from_version and result["to_version"] == "0.8.47"
    assert json.loads(marker.read_bytes())["version"] == "0.8.47"
    assert state.read_bytes() == original and state.stat().st_ino == inode
    assert all(command[:4] == [str(root / ".bin/mobkit_gateway"), "storage-migrate", "--state-dir", str(root / ".mobkit/state")] for command in calls)


@pytest.mark.parametrize("from_version", ["0.8.42", "0.8.46", "0.8.47"])
@pytest.mark.parametrize("requires_apply", [False, True])
def test_explicit_048_forward_upgrade_keeps_original_history_until_complete_native_report(
        upgrade_project, storage, monkeypatch, from_version, requires_apply):
    root, marker, _ = upgrade_project
    (root / "deployment.json").write_text(json.dumps({"slug": "test-team", "mobkitVersion": "0.8.48"}))
    storage.write_private(marker, {"format": 1, "phase": "ready", "project": "test-team", "version": from_version})
    original_marker = marker.read_bytes()
    state = root / ".mobkit/state/continuity.sqlite3"
    original, inode = state.read_bytes(), state.stat().st_ino
    read = native_upgrade_report(root, version=1 if requires_apply else 2)
    reports = [read, native_upgrade_report(root, apply=True)] if requires_apply else [read]
    calls = mock_native(monkeypatch, storage, reports, effect=lambda command, number: (
        None if marker.read_bytes() == original_marker else pytest.fail("Marker changed before native report")))
    with storage.project_lock(root):
        result = storage.upgrade_storage(root, from_version, "0.8.48")
    assert len(calls) == (2 if requires_apply else 1)
    assert result["applied"] is requires_apply
    assert result["from_version"] == from_version and result["to_version"] == "0.8.48"
    assert json.loads(marker.read_bytes())["version"] == "0.8.48"
    assert marker.stat().st_mode & 0o777 == 0o600
    assert state.read_bytes() == original and state.stat().st_ino == inode
    assert json.loads(Path(result["read_report"]).read_bytes()) == read
    assert all(command[:4] == [str(root / ".bin/mobkit_gateway"), "storage-migrate", "--state-dir", str(root / ".mobkit/state")] for command in calls)


def test_047_to_048_failed_native_apply_keeps_predecessor_marker_and_all_reports(
        upgrade_project, storage, monkeypatch):
    root, marker, _ = upgrade_project
    (root / "deployment.json").write_text(json.dumps({"slug": "test-team", "mobkitVersion": "0.8.48"}))
    storage.write_private(marker, {"format": 1, "phase": "ready", "project": "test-team", "version": "0.8.47"})
    original = marker.read_bytes()
    read, applied = native_upgrade_report(root, version=1), native_upgrade_report(root, apply=True)
    applied["errors"] = ["Native apply did not complete."]
    calls = mock_native(monkeypatch, storage, [read, applied])
    with storage.project_lock(root), pytest.raises(RuntimeError, match="needs attention"):
        storage.upgrade_storage(root, "0.8.47", "0.8.48")
    assert len(calls) == 2 and marker.read_bytes() == original
    retained = list((root / ".mobkit/storage-upgrades").iterdir())
    assert len(retained) == 1
    assert json.loads((retained[0] / "read.json").read_bytes()) == read
    assert json.loads((retained[0] / "apply.json").read_bytes()) == applied
