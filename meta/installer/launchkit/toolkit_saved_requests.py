"""Host-approved saved Toolkit settings, using the selected release's owners."""
from __future__ import annotations

import asyncio
import copy
import hashlib
import json
import os
from pathlib import Path
import re
import signal
import stat
from types import ModuleType
from uuid import uuid4

from .guide_store import GuideStore
from . import toolkit_setup

CONFIG_ENV = 'MOBKIT_TOOLKIT_SAVED_REQUEST_CONFIG'
ARCHIVE_ENV = 'MOBKIT_TOOLKIT_SAVED_REQUEST_ARCHIVE'
MAX_MANIFEST_BYTES = 65536  # The archived reader owns the actual format and limit.
MAX_RECORD_BYTES = 2_500_000
REVIEW_TIMEOUT = 180
PREPARE_TIMEOUT = 90
UNAVAILABLE = 'Saved Toolkit settings review is unavailable on this host.'
CONFIGURED_REQUEST = 'toolkit.consumer_setup.configured_install_request'


class ToolkitSavedRequestsError(ValueError):
    """A fixed host refusal, without historical contents or runtime diagnostics."""


def _digest(raw):
    return hashlib.sha256(raw).hexdigest()


def _encoded(value):
    return (json.dumps(value, ensure_ascii=True, sort_keys=True,
        separators=(',', ':'), allow_nan=False) + '\n').encode()


def _private_write(path, raw):
    if path.exists() or path.is_symlink():
        info = path.lstat()
        if not stat.S_ISREG(info.st_mode) or info.st_uid != os.getuid() or info.st_nlink != 1:
            raise ToolkitSavedRequestsError('saved_settings_storage_unavailable')
    temporary = path.parent / ('.write-' + uuid4().hex)
    descriptor = os.open(temporary, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
    try:
        with os.fdopen(descriptor, 'wb') as stream:
            stream.write(raw)
            stream.flush()
            os.fsync(stream.fileno())
        os.replace(temporary, path)
        directory = os.open(path.parent, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
        try:
            os.fsync(directory)
        finally:
            os.close(directory)
    finally:
        temporary.unlink(missing_ok=True)


def _source_host(config_path, archive_path):
    """Verify a host's existing preparation, without starting its old workspace."""
    path = toolkit_setup._path(str(config_path))
    if path.name != 'prepared.json':
        raise ValueError('Invalid saved setup host receipt.')
    toolkit_setup._directory(str(path.parent))
    raw = toolkit_setup._file(path, 32768)
    receipt = json.loads(raw)
    required = {'schema_version', 'status', 'scope', 'release', 'target', 'workspace',
        'runtimes', 'public_pypi_enabled', 'public_npm_enabled', 'files'}
    has_prior = isinstance(receipt, dict) and 'prior_selection' in receipt
    if (not isinstance(receipt, dict) or set(receipt) != required | ({'prior_selection'} if has_prior else set())
            or type(receipt['schema_version']) is not int or receipt['schema_version'] != 1
            or receipt['status'] != 'prepared' or receipt['scope'] != 'host_setup_configuration_only'
            or receipt['public_pypi_enabled'] is not False or receipt['public_npm_enabled'] is not False):
        raise ValueError('Invalid saved setup host scope.')
    release = receipt['release']
    if not isinstance(release, dict) or set(release) != {'archive_sha256', 'archive_root', 'directory'}:
        raise ValueError('Invalid saved setup release.')
    known = toolkit_setup._PREPARED_RELEASES.get(release['archive_sha256'])
    if (known is None or known.get('saved_setup') is not True
            or release['archive_root'] != known['root'] or receipt['target'] != known['target']):
        raise ValueError('Unsupported saved setup release.')
    root = toolkit_setup._directory(release['directory'])
    if root.name != known['root']:
        raise ValueError('Changed saved setup release.')
    expected_files = {'mcp.json', 'selected-install.json'} | ({'prior-request.json'} if has_prior else set())
    if not isinstance(receipt['files'], dict) or set(receipt['files']) != expected_files:
        raise ValueError('Invalid saved setup source files.')
    for name, digest in receipt['files'].items():
        toolkit_setup._pinned_file(path.parent / name, digest)
    profile = json.loads(toolkit_setup._file(path.parent / 'selected-install.json', 32768))
    if not isinstance(profile, dict) or profile.get('scope') != toolkit_setup.SCOPE:
        raise ValueError('Invalid saved setup installation scope.')
    staging = toolkit_setup._directory(profile['staging_root'])
    runtimes = receipt['runtimes']
    if not isinstance(runtimes, dict) or set(runtimes) != {'python', 'mcp_python'}:
        raise ValueError('Invalid saved setup host runtimes.')
    python = toolkit_setup._runtime(runtimes['python'])
    mcp_python = toolkit_setup._runtime(runtimes['mcp_python'])
    mcp = json.loads(toolkit_setup._file(path.parent / 'mcp.json', 32768))
    if not isinstance(mcp, dict) or set(mcp) != {'mcpServers'} or set(mcp['mcpServers']) != {'toolkit-setup'}:
        raise ValueError('Invalid saved setup host command.')
    launch = mcp['mcpServers']['toolkit-setup']
    if not isinstance(launch, dict) or set(launch) != {'command', 'args'} or not isinstance(launch['args'], list):
        raise ValueError('Invalid saved setup host command.')
    args = launch['args']
    if len(args) != (29 if has_prior else 25) or any(not isinstance(arg, str) for arg in args):
        raise ValueError('Invalid saved setup host arguments.')
    families = args[14].split(',')
    if not families or len(set(families)) != len(families) or not set(families) <= {'agno', 'services'}:
        raise ValueError('Unsupported saved setup host families.')
    setup, bundle = root / 'setup', root / 'consumers'
    expected = ['-i', 'PATH=/usr/bin:/bin', 'PYTHONDONTWRITEBYTECODE=1', str(mcp_python), '-I', '-B',
        str(setup / 'mcp_adapter.py'), '--bundle', str(bundle), '--contents-sha256', known['contents_sha256'],
        '--workspace', receipt['workspace'], '--families', args[14], '--verifier', str(bundle / 'install-library-consumer.py'),
        '--verifier-sha256', known['verifier_sha256'], '--python', str(python),
        '--selected-install-profile', str(path.parent / 'selected-install.json'),
        '--selected-install-profile-sha256', receipt['files']['selected-install.json']]
    if has_prior:
        prior = receipt['prior_selection']
        if (not isinstance(prior, dict) or set(prior) != {'source', 'manifest_sha256', 'request_id', 'request_sha256'}
                or prior['source'] != 'recovery_manifest'
                or re.fullmatch('[a-f0-9]{64}', prior['manifest_sha256']) is None
                or re.fullmatch('[a-f0-9]{32}', prior['request_id']) is None
                or prior['request_sha256'] != receipt['files']['prior-request.json']):
            raise ValueError('Invalid saved setup host selection.')
        expected += ['--prior-request', str(path.parent / 'prior-request.json'),
            '--prior-request-sha256', prior['request_sha256']]
    if launch['command'] != '/usr/bin/env' or args != expected:
        raise ValueError('Changed saved setup host command.')
    # Load only the helper bytes pinned by the already-supported release. The
    # helper verifies the entire archive/extraction and supplies its own parser.
    helper_path = setup / 'prepare_host.py'
    helper_raw = toolkit_setup._file(helper_path, 2_000_000)
    if _digest(helper_raw) != known['setup_files']['prepare_host.py']:
        raise ValueError('Changed saved setup helper.')
    helper = ModuleType('launch_saved_setup_host')
    helper.__file__ = str(helper_path)
    exec(compile(helper_raw, str(helper_path), 'exec'), helper.__dict__)
    archive = toolkit_setup._path(str(archive_path))
    snapshot, retained = helper.release_snapshot(str(archive), release['archive_sha256'], root, receipt['target'])
    if snapshot['root'] != known['root']:
        raise ValueError('Changed saved setup archive.')
    backend_raw = retained['setup/backend.py']
    if _digest(backend_raw) != known['setup_files']['backend.py']:
        raise ValueError('Changed saved setup reader.')
    backend = helper.owner_module('launch_saved_setup_backend', setup / 'backend.py', backend_raw)
    if toolkit_setup._file(path, 32768) != raw:
        raise ValueError('Changed saved setup host receipt.')
    public = {'target': receipt['target'], 'families': families,
        'scope': 'current_settings_review_only', 'release': known['root']}
    return {'receipt': receipt, 'receipt_sha256': _digest(raw), 'archive': archive,
        'root': root, 'staging': staging, 'python': python, 'mcp_python': mcp_python,
        'helper_path': helper_path, 'helper': helper, 'backend': backend, 'host': public,
        'fingerprint': _digest(_encoded({'receipt_sha256': _digest(raw), 'archive': str(archive)}))}


class ToolkitSavedRequests:
    def __init__(self, storage_dir, *, config_path=None, release_archive=None):
        self.root = Path(storage_dir).absolute()
        self.config_path = config_path or os.environ.get(CONFIG_ENV)
        self.release_archive = release_archive or os.environ.get(ARCHIVE_ENV)
        self.tasks = {}
        self.setups = {}
        self.locks = {}
        self.closed = False

    def _directory(self, guide_id):
        GuideStore.identity(guide_id)
        self.root.mkdir(mode=0o700, parents=True, exist_ok=True)
        toolkit_setup._directory(str(self.root))
        os.chmod(self.root, 0o700)
        path = self.root / guide_id
        path.mkdir(mode=0o700, exist_ok=True)
        toolkit_setup._directory(str(path))
        os.chmod(path, 0o700)
        return path

    def _read(self, guide_id):
        path = self._directory(guide_id) / 'state.json'
        if not path.exists():
            return {'guide_id': guide_id, 'status': 'idle', 'requests': [], 'current_operations': []}
        state = json.loads(toolkit_setup._file(path, MAX_RECORD_BYTES))
        if not isinstance(state, dict) or state.get('guide_id') != guide_id:
            raise ToolkitSavedRequestsError('saved_settings_storage_unavailable')
        if state.get('status') == 'preparing' and guide_id not in self.tasks:
            state.update(status='unconfirmed', error='saved_settings_review_interrupted')
            self._save(guide_id, state)
        return state

    def _save(self, guide_id, state):
        raw = _encoded(state)
        if len(raw) > MAX_RECORD_BYTES:
            raise ToolkitSavedRequestsError('saved_settings_review_too_large')
        _private_write(self._directory(guide_id) / 'state.json', raw)

    def _lock(self, guide_id):
        GuideStore.identity(guide_id)
        return self.locks.setdefault(guide_id, asyncio.Lock())

    def status(self, guide_id):
        state = self._read(guide_id)
        configured = bool(self.config_path and self.release_archive) and not self.closed
        return {'available': configured and state.get('error') != 'saved_settings_host_unavailable',
            'host': state.get('host'), **copy.deepcopy(state), 'current_operations': [],
            **({'status': 'unavailable', 'detail': UNAVAILABLE} if not configured else {})}

    async def _host(self):
        if self.closed or not self.config_path or not self.release_archive:
            raise ToolkitSavedRequestsError('saved_settings_host_unavailable')
        try:
            async with asyncio.timeout(30):
                host = await asyncio.to_thread(_source_host, self.config_path, self.release_archive)
            if self.closed:
                raise ToolkitSavedRequestsError('saved_settings_host_unavailable')
            return host
        except Exception:
            raise ToolkitSavedRequestsError('saved_settings_host_unavailable') from None

    def _requests(self, raw, host):
        try:
            root, entries = host['backend'].read_recovery_selection(raw)
            requests = []
            for entry in entries:
                if entry['kind'] == CONFIGURED_REQUEST:
                    request = host['backend'].read_file(root, 'requests/' + entry['id'] + '.json',
                        host['backend'].MAX_BODY * 2)
                    if host['backend'].digest(request) != entry['sha256']:
                        raise ValueError('Changed historical request.')
                    record = host['backend'].prior_request(request)
                    requests.append({'request_id': entry['id'], 'request_sha256': entry['sha256'],
                        'family': record['selection']['family'], 'profile': record['selection']['profile']})
            return requests
        except Exception:
            raise ToolkitSavedRequestsError('saved_settings_manifest_refused') from None

    def _manifest(self, guide_id, state, host):
        raw = toolkit_setup._file(self._directory(guide_id) / 'recovery-manifest.json', MAX_MANIFEST_BYTES)
        if _digest(raw) != state.get('manifest_sha256'):
            raise ToolkitSavedRequestsError('saved_settings_manifest_changed')
        return self._requests(raw, host)

    async def inspect(self, guide_id, raw_manifest_bytes):
        if not isinstance(raw_manifest_bytes, bytes) or not 0 < len(raw_manifest_bytes) <= MAX_MANIFEST_BYTES:
            raise ToolkitSavedRequestsError('saved_settings_manifest_too_large')
        async with self._lock(guide_id):
            if guide_id in self.tasks or guide_id in self.setups:
                raise ToolkitSavedRequestsError('saved_settings_review_in_progress')
            host = await self._host()
            # Validate before replacing a prior selection or completed review.
            requests = self._requests(raw_manifest_bytes, host)
            state = {'guide_id': guide_id, 'status': 'inspected', 'host': host['host'],
                'host_fingerprint': host['fingerprint'], 'manifest_sha256': _digest(raw_manifest_bytes),
                'requests': requests, 'current_operations': []}
            _private_write(self._directory(guide_id) / 'recovery-manifest.json', raw_manifest_bytes)
            self._save(guide_id, state)
            return self.status(guide_id)

    def _selected(self, guide_id, state, host, manifest_sha256, request_id):
        if manifest_sha256 != state.get('manifest_sha256'):
            raise ToolkitSavedRequestsError('saved_settings_manifest_changed')
        requests = self._manifest(guide_id, state, host)
        row = next((row for row in requests if row['request_id'] == request_id), None)
        if row is None:
            raise ToolkitSavedRequestsError('saved_settings_request_not_selected')
        try:
            selected = host['helper'].read_prior_manifest(host['backend'],
                (self._directory(guide_id) / 'recovery-manifest.json', manifest_sha256, request_id))
            request = host['backend'].prior_request(selected['request'])
        except Exception:
            raise ToolkitSavedRequestsError('saved_settings_request_changed') from None
        return row, selected['request'], request

    async def preview(self, guide_id, manifest_sha256, request_id):
        async with self._lock(guide_id):
            if guide_id in self.tasks or guide_id in self.setups:
                raise ToolkitSavedRequestsError('saved_settings_review_in_progress')
            host, state = await self._host(), self._read(guide_id)
            row, raw, request = self._selected(guide_id, state, host, manifest_sha256, request_id)
            state.update(status='preview', host=host['host'], host_fingerprint=host['fingerprint'],
                selection={**row}, current_operations=[])
            for key in ('review', 'error', 'config_path', 'job', 'attempt', 'audience_approved'):
                state.pop(key, None)
            self._save(guide_id, state)
            return {**self.status(guide_id), 'request_text': raw.decode('utf-8'), 'request': request,
                'audience_approval_required': True}

    async def review(self, guide_id, version, manifest_sha256, request_id, request_sha256,
            *, host_fingerprint, audience_approved=False):
        if audience_approved is not True:
            raise ToolkitSavedRequestsError('saved_settings_audience_approval_required')
        if type(version) is not int or version < 1:
            raise ToolkitSavedRequestsError('saved_settings_guide_version_required')
        async with self._lock(guide_id):
            state = self._read(guide_id)
            selection = state.get('selection') or {}
            desired = {'request_id': request_id, 'request_sha256': request_sha256, 'version': version}
            if (state.get('manifest_sha256') != manifest_sha256
                    or selection.get('request_id') != request_id or selection.get('request_sha256') != request_sha256
                    or state.get('host_fingerprint') != host_fingerprint):
                raise ToolkitSavedRequestsError('saved_settings_approval_changed')
            if state.get('status') in {'preparing', 'review_required', 'profile_unavailable', 'unconfirmed'}:
                if selection != desired:
                    raise ToolkitSavedRequestsError('saved_settings_approval_changed')
                return self.status(guide_id)
            if state.get('status') != 'preview':
                raise ToolkitSavedRequestsError('saved_settings_preview_required')
            host = await self._host()
            if host['fingerprint'] != host_fingerprint:
                raise ToolkitSavedRequestsError('saved_settings_host_changed')
            row, raw, request = self._selected(guide_id, state, host, manifest_sha256, request_id)
            if row['request_sha256'] != request_sha256:
                raise ToolkitSavedRequestsError('saved_settings_request_changed')
            if row['family'] not in host['host']['families']:
                raise ToolkitSavedRequestsError('saved_settings_family_unavailable')
            attempt = 'review-' + uuid4().hex
            if guide_id in self.setups:
                raise ToolkitSavedRequestsError('saved_settings_worker_settlement_unconfirmed')
            state.update(status='preparing', selection=desired, attempt=attempt, audience_approved=True)
            # Register ownership before publication: status is also read from
            # server threads and must not misclassify admission as orphaned.
            task = asyncio.create_task(self._review(guide_id, host, state, raw),
                name='toolkit-saved-review-' + guide_id)
            self.tasks[guide_id] = task
            try:
                self._save(guide_id, state)
            except BaseException:
                task.cancel()
                self.tasks.pop(guide_id, None)
                raise
            return self.status(guide_id)

    async def _prepare(self, guide_id, host, state):
        attempt = self._directory(guide_id) / state['attempt']
        attempt.mkdir(mode=0o700)
        output, workspace = attempt / 'configuration', attempt / 'workspace'
        receipt = host['receipt']
        for pin in receipt['runtimes'].values():
            toolkit_setup._runtime(pin)
        toolkit_setup._pinned_file(host['helper_path'],
            toolkit_setup._PREPARED_RELEASES[receipt['release']['archive_sha256']]['setup_files']['prepare_host.py'], 2_000_000)
        args = [str(host['python']), '-I', '-B', str(host['helper_path']),
            '--release-archive', str(host['archive']), '--release-sha256', receipt['release']['archive_sha256'],
            '--release-dir', str(host['root']), '--target', receipt['target'], '--output', str(output),
            '--workspace', str(workspace), '--staging-root', str(host['staging']),
            '--python', str(host['python']), '--python-sha256', receipt['runtimes']['python']['sha256'],
            '--mcp-python', str(host['mcp_python']), '--mcp-python-sha256', receipt['runtimes']['mcp_python']['sha256'],
            '--families', ','.join(host['host']['families']),
            '--prior-manifest', str(self._directory(guide_id) / 'recovery-manifest.json'),
            '--prior-manifest-sha256', state['manifest_sha256'], '--prior-request-id', state['selection']['request_id']]
        process = await asyncio.create_subprocess_exec(*args, stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.DEVNULL, start_new_session=True,
            env={'PATH': '/usr/bin:/bin', 'PYTHONDONTWRITEBYTECODE': '1', 'LANG': 'C.UTF-8'})
        try:
            async with asyncio.timeout(PREPARE_TIMEOUT):
                total = 0
                while chunk := await process.stdout.read(8192):
                    total += len(chunk)
                    if total > 65536:
                        raise ToolkitSavedRequestsError('saved_settings_preparation_refused')
                if await process.wait() != 0:
                    raise ToolkitSavedRequestsError('saved_settings_preparation_refused')
        finally:
            if process.returncode is None:
                os.killpg(process.pid, signal.SIGTERM)
                try:
                    await asyncio.wait_for(process.wait(), 3)
                except asyncio.TimeoutError:
                    os.killpg(process.pid, signal.SIGKILL)
                    await process.wait()
        return output / 'mcp.json'

    async def _review(self, guide_id, host, state, approved_raw):
        setup = None
        completed = None
        try:
            async with asyncio.timeout(REVIEW_TIMEOUT):
                config = await self._prepare(guide_id, host, state)
                prepared = json.loads(toolkit_setup._file(config.parent / 'prepared.json', 32768))
                expected = {'source': 'recovery_manifest', 'manifest_sha256': state['manifest_sha256'],
                    'request_id': state['selection']['request_id'],
                    'request_sha256': state['selection']['request_sha256']}
                if (prepared.get('prior_selection') != expected
                        or toolkit_setup._file(config.parent / 'prior-request.json', 131072) != approved_raw):
                    raise ToolkitSavedRequestsError('saved_settings_approved_request_changed')
                state['config_path'] = str(config)
                self._save(guide_id, state)
                setup = toolkit_setup.ToolkitSetup(config_path=config)
                self.setups[guide_id] = setup
                await setup.start()
                if not setup.status()['available'] or setup.checked_selections:
                    raise ToolkitSavedRequestsError('saved_settings_current_owner_unavailable')
                arguments = {'prior_request_sha256': state['selection']['request_sha256']}
                result = await setup.call('setup_successor', arguments)
                if result.get('status') == 'preparation_required':
                    family = result.get('family')
                    if family != next(row['family'] for row in state['requests']
                            if row['request_id'] == state['selection']['request_id']):
                        raise ToolkitSavedRequestsError('saved_settings_current_family_changed')
                    admitted = await setup.call('setup_prepare', {'family': family})
                    job = admitted.get('job')
                    if (not isinstance(job, dict) or not isinstance(job.get('id'), str)
                            or re.fullmatch('[a-f0-9]{32}', job['id']) is None
                            or job.get('family') != family or job.get('kind') != 'prepare'):
                        raise ToolkitSavedRequestsError('saved_settings_preparation_unconfirmed')
                    state['job'] = {'id': job['id'], 'family': family, 'kind': 'prepare'}
                    self._save(guide_id, state)
                    while True:
                        observed = await setup.call('setup_job', {'job_id': job['id']})
                        current = observed.get('job')
                        if (not isinstance(current, dict) or any(current.get(key) != value
                                for key, value in state['job'].items())
                                or current.get('status') not in {'running', 'succeeded', 'unconfirmed'}):
                            raise ToolkitSavedRequestsError('saved_settings_preparation_unconfirmed')
                        if current['status'] == 'succeeded':
                            break
                        if current['status'] == 'unconfirmed':
                            raise ToolkitSavedRequestsError('saved_settings_preparation_unconfirmed')
                        await asyncio.sleep(.2)
                    result = await setup.call('setup_successor', arguments)
                row = next(row for row in state['requests']
                    if row['request_id'] == state['selection']['request_id'])
                if (result.get('status') not in {'review_required', 'profile_unavailable'}
                        or result.get('scope') != 'successor_configuration_candidate'
                        or result.get('prior_request_sha256') != state['selection']['request_sha256']
                        or result.get('family') != row['family'] or result.get('profile') != row['profile']
                        or 'operations' in result or setup.checked_selections):
                    raise ToolkitSavedRequestsError('saved_settings_current_review_unconfirmed')
                completed = result
        except BaseException as error:
            state.pop('review', None)
            state.update(status='unconfirmed', error=(str(error) if isinstance(error, ToolkitSavedRequestsError)
                else 'saved_settings_review_interrupted' if isinstance(error, asyncio.CancelledError)
                else 'saved_settings_current_review_unconfirmed'))
            self._save(guide_id, state)
        finally:
            try:
                if setup is not None:
                    try:
                        await setup.shutdown()
                        self.setups.pop(guide_id, None)
                    except Exception:
                        state.pop('review', None)
                        state.update(status='unconfirmed', error='saved_settings_worker_settlement_unconfirmed')
                        self._save(guide_id, state)
                        completed = None
                if completed is not None:
                    # Publication is after worker settlement. A polling client or
                    # guide tool cannot observe a candidate whose close is pending.
                    state.update(status=completed['status'], review=completed, current_operations=[])
                    self._save(guide_id, state)
            finally:
                # A failed final save cannot retain a finished task. Unsettled
                # native workers stay in setups and continue to block new work.
                self.tasks.pop(guide_id, None)

    async def shutdown(self):
        self.closed = True
        tasks = list(self.tasks.values())
        for task in tasks:
            task.cancel()
        if tasks:
            await asyncio.gather(*tasks, return_exceptions=True)
        for guide_id, setup in list(self.setups.items()):
            try:
                await setup.shutdown()
                self.setups.pop(guide_id, None)
            except Exception:
                raise ToolkitSavedRequestsError('saved_settings_worker_settlement_unconfirmed') from None
