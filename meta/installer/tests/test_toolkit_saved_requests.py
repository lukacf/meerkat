"""Saved-setting selection and approval, without providers or native sessions."""
import asyncio
import copy
import hashlib
import json
from pathlib import Path
from types import SimpleNamespace
from uuid import uuid4

import pytest

from launchkit import toolkit_saved_requests as module


def encoded(value):
    return json.dumps(value, sort_keys=True).encode()


def digest(raw):
    return hashlib.sha256(raw).hexdigest()


class Owner:
    """The fixture owner admits exact known anchors, not a Launch-owned format."""
    MAX_BODY = 65536
    digest = staticmethod(digest)

    def __init__(self, root, requests, entries):
        self.root, self.requests, self.entries = root, requests, entries
        self.manifest = encoded({'opaque_owner_manifest': 'selected anchors'})
        self.reads = []

    def read_recovery_selection(self, raw):
        self.reads.append(('read_recovery_selection', raw))
        if raw != self.manifest:
            raise ValueError('Owner refuses manifest')
        for row in self.entries:
            if digest(self.read_file(self.root, 'requests/' + row['id'] + '.json', self.MAX_BODY * 2)) != row['sha256']:
                raise ValueError('Owner refuses changed anchor')
        return self.root, copy.deepcopy(self.entries)

    def read_file(self, root, name, limit):
        raw = (root / name).read_bytes()
        if len(raw) > limit:
            raise ValueError('Owner limit')
        return raw

    def prior_request(self, raw):
        self.reads.append(('prior_request', raw))
        if raw not in self.requests.values():
            raise ValueError('Owner refuses request')
        return json.loads(raw)


class Helper:
    def __init__(self, owner):
        self.owner = owner

    def read_prior_manifest(self, owner, selection):
        path, expected, request_id = selection
        raw = path.read_bytes()
        assert digest(raw) == expected
        root, rows = owner.read_recovery_selection(raw)
        row = next(row for row in rows if row['id'] == request_id)
        if row['kind'] != module.CONFIGURED_REQUEST:
            raise ValueError('Owner refuses non-configured request')
        request = owner.read_file(root, 'requests/' + request_id + '.json', owner.MAX_BODY * 2)
        assert digest(request) == row['sha256']
        owner.prior_request(request)
        return {'request': request, 'sha256': row['sha256']}


class Setup:
    def __init__(self, host, config_path):
        self.host, self.config_path = host, config_path
        self.calls, self.checked_selections, self.closed = [], {}, 0
        self.job_id = '7' * 32

    async def start(self):
        self.host.started.append(self)

    def status(self):
        return {'available': True}

    async def call(self, name, arguments):
        self.calls.append((name, copy.deepcopy(arguments)))
        if name == 'setup_successor':
            selection = json.loads((self.config_path.parent / 'prior-request.json').read_bytes())['selection']
            value = {'scope': 'successor_configuration_candidate',
                'prior_request_sha256': arguments['prior_request_sha256'],
                'family': selection['family'], 'profile': selection['profile']}
            if self.host.prepare_needed and not any(name == 'setup_job' for name, args in self.calls):
                return {**value, 'status': 'preparation_required'}
            return {**value, 'status': self.host.native_status, 'values': selection['values'],
                'prior_operations': selection['operations'], 'help': {'operations': [{'id': 'new_operation'}]},
                'unapplied': []}
        if name == 'setup_prepare':
            if self.host.lost_reply:
                raise RuntimeError('Lost admission reply')
            return {'job': {'id': self.job_id, 'kind': 'prepare', 'family': arguments['family'], 'status': 'running'}}
        if name == 'setup_job':
            return {'job': {'id': self.job_id, 'kind': 'prepare', 'family': 'services',
                'status': self.host.job_status}}
        raise AssertionError('Unexpected action: ' + name)

    async def shutdown(self):
        self.host.settlement_started.set()
        await self.host.settlement_allowed.wait()
        if self.host.close_failure:
            raise RuntimeError('Unknown worker settlement')
        self.closed += 1


@pytest.fixture
def host(tmp_path, monkeypatch):
    original = tmp_path / 'original'
    requests_dir = original / 'requests'
    requests_dir.mkdir(parents=True)
    selected_id, other_id = '1' * 32, '2' * 32
    request = {'selection': {'family': 'services', 'profile': 'gmail',
        'operations': ['historical_operation'], 'values': {'token_env': 'OLD_TOKEN_REFERENCE',
            'limit': 7, 'enabled': True, 'labels': ['old', 'draft']}},
        'reported_runtime': '/old/runtime/never-execute', 'reported_outcome': 'unconfirmed'}
    selected_raw = encoded(request)
    other_raw = encoded({'opaque_acquisition': 'never a configured selection'})
    requests = {selected_id: selected_raw, other_id: other_raw}
    entries = [{'id': key, 'sha256': digest(raw),
        'kind': module.CONFIGURED_REQUEST if key == selected_id else 'toolkit.other_request'}
        for key, raw in requests.items()]
    for key, raw in requests.items():
        (requests_dir / (key + '.json')).write_bytes(raw)
    owner = Owner(original, requests, entries)
    value = SimpleNamespace(owner=owner, selected_id=selected_id, other_id=other_id,
        original=original, request=request, raw=selected_raw, request_sha256=digest(selected_raw),
        started=[], prepared=[], prepare_needed=True, native_status='review_required',
        job_status='succeeded', lost_reply=False, close_failure=False,
        settlement_started=asyncio.Event(), settlement_allowed=asyncio.Event())
    value.settlement_allowed.set()
    value.binding = {'helper': Helper(owner), 'backend': owner,
        'host': {'target': 'macos-arm64', 'families': ['services'], 'scope': 'current_settings_review_only'},
        'fingerprint': 'a' * 64}
    monkeypatch.setattr(module, '_source_host', lambda *args: value.binding)
    monkeypatch.setattr(module.toolkit_setup, 'ToolkitSetup',
        lambda config_path: Setup(value, config_path))

    async def prepare(service, guide_id, current, state):
        value.prepared.append(copy.deepcopy(state))
        output = service._directory(guide_id) / state['attempt'] / 'configuration'
        output.mkdir(parents=True)
        manifest = service._directory(guide_id) / 'recovery-manifest.json'
        selected = current['helper'].read_prior_manifest(current['backend'],
            (manifest, state['manifest_sha256'], state['selection']['request_id']))
        (output / 'prior-request.json').write_bytes(selected['request'])
        (output / 'prepared.json').write_bytes(encoded({'prior_selection': {
            'source': 'recovery_manifest', 'manifest_sha256': state['manifest_sha256'],
            'request_id': state['selection']['request_id'],
            'request_sha256': state['selection']['request_sha256']}}))
        (output / 'mcp.json').write_bytes(b'{}')
        return output / 'mcp.json'

    monkeypatch.setattr(module.ToolkitSavedRequests, '_prepare', prepare)
    value.service = module.ToolkitSavedRequests(tmp_path / 'private',
        config_path='/host/prepared.json', release_archive='/host/explicit-archive.tar.gz')
    value.guide_id = str(uuid4())
    return value


async def select(host):
    service = host.service
    state = await service.inspect(host.guide_id, host.owner.manifest)
    return await service.preview(host.guide_id, state['manifest_sha256'], host.selected_id)


async def approve(host, preview, **changes):
    arguments = {'guide_id': host.guide_id, 'version': 3,
        'manifest_sha256': preview['manifest_sha256'], 'request_id': preview['selection']['request_id'],
        'request_sha256': preview['selection']['request_sha256'],
        'host_fingerprint': preview['host_fingerprint'], 'audience_approved': True}
    arguments.update(changes)
    return await host.service.review(**arguments)


async def settle(host):
    task = host.service.tasks.get(host.guide_id)
    if task:
        await task
    return host.service.status(host.guide_id)


@pytest.mark.asyncio
async def test_complete_review_reuses_owner_readers_retains_types_and_never_current_operations(host):
    preview = await select(host)
    assert preview['requests'] == [{'request_id': host.selected_id, 'request_sha256': host.request_sha256,
        'family': 'services', 'profile': 'gmail'}]
    assert preview['request_text'].encode() == host.raw and preview['request'] == host.request
    assert preview['audience_approval_required'] is True and not host.prepared and not host.started
    before = {path.name: path.read_bytes() for path in (host.original / 'requests').iterdir()}
    preparing = await approve(host, preview)
    assert preparing['status'] == 'preparing' and preparing['audience_approved'] is True
    state = await settle(host)
    assert state['status'] == 'review_required' and state['current_operations'] == []
    assert state['review']['values'] == host.request['selection']['values']
    assert state['review']['prior_operations'] == ['historical_operation']
    assert 'operations' not in state['review'] and state['selection']['version'] == 3
    setup = host.started[0]
    assert [name for name, args in setup.calls] == ['setup_successor', 'setup_prepare', 'setup_job', 'setup_successor']
    assert setup.calls[1] == ('setup_prepare', {'family': 'services'}) and setup.closed == 1
    assert setup.checked_selections == {} and len(host.prepared) == 1
    assert before == {path.name: path.read_bytes() for path in (host.original / 'requests').iterdir()}
    repeated = await approve(host, preview)
    assert repeated['review'] == state['review'] and len(host.prepared) == 1
    restored = module.ToolkitSavedRequests(host.service.root,
        config_path='/host/prepared.json', release_archive='/host/explicit-archive.tar.gz')
    assert restored.status(host.guide_id)['review'] == state['review']
    for path in (host.service.root / host.guide_id).glob('*.json'):
        assert path.stat().st_mode & 0o077 == 0


@pytest.mark.asyncio
@pytest.mark.parametrize('change', ['approval', 'manifest', 'request-id', 'request-digest', 'host', 'version', 'preview'])
async def test_approval_must_bind_explicit_exact_preview_before_preparation(host, change):
    preview = await select(host)
    changes = {}
    if change == 'approval': changes['audience_approved'] = False
    elif change == 'manifest': changes['manifest_sha256'] = '0' * 64
    elif change == 'request-id': changes['request_id'] = host.other_id
    elif change == 'request-digest': changes['request_sha256'] = '0' * 64
    elif change == 'host': changes['host_fingerprint'] = '0' * 64
    elif change == 'version': changes['version'] = True
    else:
        await host.service.inspect(host.guide_id, host.owner.manifest)
    with pytest.raises(module.ToolkitSavedRequestsError):
        await approve(host, preview, **changes)
    assert not host.prepared and not host.started


@pytest.mark.asyncio
@pytest.mark.parametrize('selected', [True, False])
async def test_any_changed_manifest_anchor_refuses_before_disclosing_or_preparing(host, selected):
    preview = await select(host)
    changed = host.selected_id if selected else host.other_id
    (host.original / 'requests' / (changed + '.json')).write_bytes(b'changed historical bytes')
    with pytest.raises(module.ToolkitSavedRequestsError, match='saved_settings_manifest_refused'):
        await approve(host, preview)
    assert not host.prepared and not host.started


@pytest.mark.asyncio
async def test_changed_host_after_preview_refuses_and_cannot_inherit_approval(host):
    preview = await select(host)
    host.binding['fingerprint'] = 'b' * 64
    host.binding['host']['families'] = ['agno']
    with pytest.raises(module.ToolkitSavedRequestsError, match='saved_settings_host_changed'):
        await approve(host, preview)
    assert not host.prepared and not host.started


@pytest.mark.asyncio
async def test_nonconfigured_selection_and_invalid_upload_leave_previous_selection_intact(host):
    preview = await select(host)
    before = host.service.status(host.guide_id)
    with pytest.raises(module.ToolkitSavedRequestsError, match='saved_settings_request_not_selected'):
        await host.service.preview(host.guide_id, preview['manifest_sha256'], host.other_id)
    with pytest.raises(module.ToolkitSavedRequestsError, match='saved_settings_manifest_refused'):
        await host.service.inspect(host.guide_id, b'unknown owner manifest')
    assert host.service.status(host.guide_id) == before
    assert (host.service.root / host.guide_id / 'recovery-manifest.json').read_bytes() == host.owner.manifest


@pytest.mark.asyncio
async def test_completed_candidate_is_not_published_before_worker_settlement(host):
    host.settlement_allowed.clear()
    preview = await select(host)
    await approve(host, preview)
    await host.settlement_started.wait()
    state = host.service.status(host.guide_id)
    assert state['status'] == 'preparing' and 'review' not in state
    host.settlement_allowed.set()
    assert (await settle(host))['status'] == 'review_required'


@pytest.mark.asyncio
@pytest.mark.parametrize('failure', ['lost-reply', 'unconfirmed-job', 'failed-settlement'])
async def test_uncertain_preparation_retains_attempt_and_never_repeats_effects(host, failure):
    if failure == 'lost-reply': host.lost_reply = True
    elif failure == 'unconfirmed-job': host.job_status = 'unconfirmed'
    else: host.close_failure = True
    preview = await select(host)
    await approve(host, preview)
    state = await settle(host)
    assert state['status'] == 'unconfirmed' and 'review' not in state and state['config_path']
    setup = host.started[0]
    assert sum(name == 'setup_prepare' for name, args in setup.calls) == 1
    await approve(host, preview)
    assert len(host.prepared) == 1 and len(host.started) == 1
    if failure == 'failed-settlement':
        with pytest.raises(module.ToolkitSavedRequestsError, match='review_in_progress'):
            await host.service.inspect(host.guide_id, host.owner.manifest)


@pytest.mark.asyncio
async def test_unavailable_host_executes_no_reader_and_uploaded_limit_is_owner_bound(tmp_path, monkeypatch):
    monkeypatch.delenv(module.CONFIG_ENV, raising=False)
    monkeypatch.delenv(module.ARCHIVE_ENV, raising=False)
    monkeypatch.setattr(module, '_source_host', lambda *args: pytest.fail('No host helper should execute'))
    service = module.ToolkitSavedRequests(tmp_path / 'private')
    guide_id = str(uuid4())
    assert service.status(guide_id)['available'] is False
    with pytest.raises(module.ToolkitSavedRequestsError, match='host_unavailable'):
        await service.inspect(guide_id, b'{}')
    with pytest.raises(module.ToolkitSavedRequestsError, match='manifest_too_large'):
        await service.inspect(guide_id, b'x' * 65537)


@pytest.mark.asyncio
async def test_reloading_interrupted_preparation_reports_unconfirmed_without_restart(host):
    preview = await select(host)
    state = host.service._read(host.guide_id)
    state.update(status='preparing', audience_approved=True,
        selection={**preview['selection'], 'version': 3}, attempt='retained-attempt')
    host.service._save(host.guide_id, state)
    restored = module.ToolkitSavedRequests(host.service.root,
        config_path='/host/prepared.json', release_archive='/host/explicit-archive.tar.gz')
    observed = restored.status(host.guide_id)
    assert observed['status'] == 'unconfirmed' and observed['error'] == 'saved_settings_review_interrupted'
    assert observed['attempt'] == 'retained-attempt' and not host.prepared and not host.started


@pytest.mark.asyncio
async def test_profile_unavailable_stays_owner_refusal_without_fallback_or_install(host):
    host.native_status = 'profile_unavailable'
    preview = await select(host)
    await approve(host, preview)
    state = await settle(host)
    assert state['status'] == 'profile_unavailable' and state['review']['status'] == 'profile_unavailable'
    assert state['current_operations'] == [] and len(host.started) == 1


@pytest.mark.asyncio
async def test_status_during_preparing_publication_has_registered_owner(host, monkeypatch):
    preview = await select(host)
    original = host.service._save
    seen = []

    def save(guide_id, state):
        original(guide_id, state)
        if state['status'] == 'preparing':
            seen.append(host.service.status(guide_id)['status'])

    monkeypatch.setattr(host.service, '_save', save)
    await approve(host, preview)
    await settle(host)
    assert seen and set(seen) == {'preparing'}


@pytest.mark.asyncio
async def test_shutdown_preserves_approved_attempt_and_reports_interruption(host, monkeypatch):
    preview = await select(host)
    entered = asyncio.Event()

    async def preparing(*args):
        entered.set()
        await asyncio.Event().wait()

    monkeypatch.setattr(host.service, '_prepare', preparing)
    await approve(host, preview)
    await entered.wait()
    await host.service.shutdown()
    state = host.service._read(host.guide_id)
    assert state['status'] == 'unconfirmed' and state['error'] == 'saved_settings_review_interrupted'
    assert state['selection'] == {'request_id': host.selected_id, 'request_sha256': host.request_sha256, 'version': 3}
    assert not host.started and 'review' not in state


@pytest.mark.asyncio
@pytest.mark.parametrize('failure', ['final-publication', 'unconfirmed-settlement'])
async def test_terminal_save_failure_releases_task_but_keeps_unsettled_native_owner(host, monkeypatch, failure):
    preview = await select(host)
    host.close_failure = failure == 'unconfirmed-settlement'
    original_save = host.service._save
    observed = []

    def save(guide_id, state):
        original_save(guide_id, state)
        if state['status'] in {'review_required', 'unconfirmed'}:
            observed.append(state['status'])
            raise OSError('Synthetic final directory persistence failure')

    monkeypatch.setattr(host.service, '_save', save)
    await approve(host, preview)
    task = host.service.tasks[host.guide_id]
    with pytest.raises(OSError, match='Synthetic final directory persistence failure'):
        await task
    assert task.done() and observed
    assert host.service.tasks == {}, 'A finished coroutine must not keep setup commands blocked.'
    setup = host.started[0]
    assert sum(name == 'setup_prepare' for name, args in setup.calls) == 1
    assert (host.original / 'requests' / (host.selected_id + '.json')).read_bytes() == host.raw
    if host.close_failure:
        assert setup.closed == 0 and host.service.setups == {host.guide_id: setup}
        with pytest.raises(module.ToolkitSavedRequestsError, match='review_in_progress'):
            await host.service.inspect(host.guide_id, host.owner.manifest)
    else:
        assert setup.closed == 1 and not host.service.setups
        monkeypatch.setattr(host.service, '_save', original_save)
        assert (await host.service.inspect(host.guide_id, host.owner.manifest))['status'] == 'inspected'


@pytest.fixture
def source_binding(tmp_path, monkeypatch):
    """Small pinned release fixture, never a runtime or connector installation."""
    root = tmp_path / 'toolkit-library-test-macos-arm64'
    setup = root / 'setup'
    setup.mkdir(parents=True)
    (root / 'consumers').mkdir()
    archive = tmp_path / 'release.tar.gz'
    archive.write_bytes(b'exact host-selected archive')
    backend = b'MAX_BODY = 65536\n'
    (setup / 'backend.py').write_bytes(backend)
    helper = b'''import hashlib
from pathlib import Path
from types import ModuleType
def release_snapshot(path, expected, root, target):
    if hashlib.sha256(Path(path).read_bytes()).hexdigest() != expected:
        raise ValueError('changed archive')
    return {'root': root.name}, {'setup/backend.py': (root / 'setup/backend.py').read_bytes()}
def owner_module(name, path, raw):
    module = ModuleType(name)
    exec(compile(raw, str(path), 'exec'), module.__dict__)
    return module
'''
    (setup / 'prepare_host.py').write_bytes(helper)
    python = tmp_path / 'trusted-interpreter'
    python.write_bytes(b'pinned executable bytes')
    python.chmod(0o755)
    output, staging, consumed = tmp_path / 'prepared', tmp_path / 'staging', tmp_path / 'already-consumed'
    for path in (output, staging, consumed):
        path.mkdir(mode=0o700)
    known = {'saved_setup': True, 'target': 'macos-arm64', 'root': root.name,
        'contents_sha256': 'b' * 64, 'verifier_sha256': 'c' * 64,
        'setup_files': {'prepare_host.py': digest(helper), 'backend.py': digest(backend)}}
    monkeypatch.setattr(module.toolkit_setup, '_PREPARED_RELEASES', {digest(archive.read_bytes()): known})
    profile = {'scope': module.toolkit_setup.SCOPE, 'staging_root': str(staging)}
    (output / 'selected-install.json').write_bytes(encoded(profile))
    pin = {'path': str(python), 'resolved_path': str(python), 'sha256': digest(python.read_bytes()), 'executable': True}
    args = ['-i', 'PATH=/usr/bin:/bin', 'PYTHONDONTWRITEBYTECODE=1', str(python), '-I', '-B',
        str(setup / 'mcp_adapter.py'), '--bundle', str(root / 'consumers'), '--contents-sha256', 'b' * 64,
        '--workspace', str(consumed), '--families', 'services', '--verifier', str(root / 'consumers/install-library-consumer.py'),
        '--verifier-sha256', 'c' * 64, '--python', str(python), '--selected-install-profile', str(output / 'selected-install.json'),
        '--selected-install-profile-sha256', digest((output / 'selected-install.json').read_bytes())]
    mcp = {'mcpServers': {'toolkit-setup': {'command': '/usr/bin/env', 'args': args}}}
    (output / 'mcp.json').write_bytes(encoded(mcp))
    receipt = {'schema_version': 1, 'status': 'prepared', 'scope': 'host_setup_configuration_only',
        'release': {'archive_sha256': digest(archive.read_bytes()), 'archive_root': root.name, 'directory': str(root)},
        'target': 'macos-arm64', 'workspace': str(consumed), 'runtimes': {'python': pin, 'mcp_python': pin},
        'public_pypi_enabled': False, 'public_npm_enabled': False,
        'files': {name: digest((output / name).read_bytes()) for name in ('mcp.json', 'selected-install.json')}}
    config = output / 'prepared.json'
    config.write_bytes(encoded(receipt))
    return SimpleNamespace(root=root, archive=archive, config=config, receipt=receipt, known=known,
        mcp=mcp, consumed=consumed, python=python, output=output, staging=staging)


def test_host_binding_is_pinned_but_never_starts_or_spends_seed_workspace(source_binding):
    value = source_binding
    before = {path: path.read_bytes() for path in value.output.iterdir()}
    host = module._source_host(value.config, value.archive)
    assert host['host']['families'] == ['services'] and host['host']['target'] == 'macos-arm64'
    assert value.consumed.is_dir() and not list(value.consumed.iterdir())
    assert before == {path: path.read_bytes() for path in value.output.iterdir()}


@pytest.mark.parametrize('change,refusal', [
    ('archive', 'changed archive'),
    ('helper', 'Changed saved setup helper.'),
    ('backend', 'Changed saved setup reader.'),
    ('runtime', 'The prepared setup input changed.'),
    ('public-network', 'Invalid saved setup host scope.'),
    ('family', 'Unsupported saved setup host families.'),
    ('command', 'Changed saved setup host command.'),
])
def test_changed_host_source_refuses_before_any_new_preparation(source_binding, change, refusal):
    value = source_binding
    if change == 'archive': value.archive.write_bytes(b'changed archive')
    elif change == 'helper': (value.root / 'setup/prepare_host.py').write_bytes(b'raise AssertionError("Never execute")')
    elif change == 'backend': (value.root / 'setup/backend.py').write_bytes(b'raise AssertionError("Never execute")')
    elif change == 'runtime': value.python.write_bytes(b'changed runtime')
    elif change == 'public-network':
        value.receipt['public_pypi_enabled'] = True
        value.config.write_bytes(encoded(value.receipt))
    else:
        if change == 'family': value.mcp['mcpServers']['toolkit-setup']['args'][14] = 'activepieces'
        else: value.mcp['mcpServers']['toolkit-setup']['command'] = '/unselected/executable'
        mcp_path = value.output / 'mcp.json'
        mcp_path.write_bytes(encoded(value.mcp))
        value.receipt['files']['mcp.json'] = digest(mcp_path.read_bytes())
        value.config.write_bytes(encoded(value.receipt))
    # Tampered helper/backend bytes deliberately raise AssertionError if run.
    # Accept only the intended custody refusal, never that execution failure.
    with pytest.raises(ValueError) as caught:
        module._source_host(value.config, value.archive)
    assert str(caught.value) == refusal
    assert not list(value.consumed.iterdir())


@pytest.mark.asyncio
async def test_helper_invocation_uses_fresh_paths_exact_manifest_triple_and_current_host_only(source_binding, tmp_path, monkeypatch):
    value = source_binding
    host = module._source_host(value.config, value.archive)
    service = module.ToolkitSavedRequests(tmp_path / 'service', config_path=value.config, release_archive=value.archive)
    guide_id = str(uuid4())
    state = {'attempt': 'review-' + 'f' * 32, 'manifest_sha256': 'a' * 64,
        'selection': {'request_id': 'd' * 32, 'request_sha256': 'e' * 64, 'version': 4}}
    seen = []
    stream = asyncio.StreamReader()
    stream.feed_data(b'{"status":"prepared"}')
    stream.feed_eof()

    async def spawn(*args, **options):
        seen.append((args, options))
        async def wait(): return 0
        return SimpleNamespace(stdout=stream, wait=wait, returncode=0)

    monkeypatch.setattr(module.asyncio, 'create_subprocess_exec', spawn)
    config = await service._prepare(guide_id, host, state)
    args, options = seen[0]
    assert args[:4] == (str(value.python), '-I', '-B', str(value.root / 'setup/prepare_host.py'))
    assert args[args.index('--prior-manifest') + 1] == str(service.root / guide_id / 'recovery-manifest.json')
    assert args[args.index('--prior-manifest-sha256') + 1] == state['manifest_sha256']
    assert args[args.index('--prior-request-id') + 1] == state['selection']['request_id']
    assert '--recover-manifest' not in args and '--prior-request' not in args and '--allow-public-pypi' not in args
    assert args[args.index('--workspace') + 1] != str(value.consumed)
    assert str(config) == str(service.root / guide_id / state['attempt'] / 'configuration/mcp.json')
    assert options['start_new_session'] is True and options['env']['PYTHONDONTWRITEBYTECODE'] == '1'
