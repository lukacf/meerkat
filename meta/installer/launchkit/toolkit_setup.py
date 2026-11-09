"""Explicit host opt-in to Toolkit's existing software setup MCP controller."""
from __future__ import annotations

import asyncio
import copy
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import re
import stat
import uuid

from jsonschema import Draft202012Validator
from referencing import Registry

from .templates_v2.mcp_connections import MCPConnection
from .toolkit_catalog import ToolkitCatalog

CONFIG_ENV = 'MOBKIT_TOOLKIT_SETUP_CONFIG'
SCOPE = 'standing_local_software_installation_only'
RECOVERY_SCOPE = 'retained_software_setup_readback_only'
RECOVERY_TOOLS = frozenset({'setup_state', 'setup_jobs', 'setup_job', 'setup_reconcile'})
TOOLS = frozenset({'setup_catalog', 'setup_catalog_entry', 'setup_catalog_route',
    'setup_state', 'setup_jobs', 'setup_job', 'setup_prepare', 'setup_retry_prepare',
    'setup_help', 'setup_check', 'setup_install', 'setup_reconcile', 'setup_stop',
    'setup_successor', 'setup_recovery_manifest'})
REQUIRED_TOOLS = frozenset({'setup_state', 'setup_job', 'setup_prepare', 'setup_help',
    'setup_check', 'setup_install', 'setup_catalog', 'setup_catalog_entry', 'setup_jobs', 'setup_stop'})
UNAVAILABLE = ('Toolkit software setup is unavailable. No action was retried. '
    'Read the same setup session or its exact job before requesting another installation.')

# Known local prerelease bytes, not a public release or a future-version contract.
_PREPARED_SETUP_FILES = {
    'acquired_inspection.py': '2fa1b271c2f0627a56f582658ceb42ff7fccf8922a3f9dd50221d7ada4f0a67c',
    'backend.py': '28946df6e25aa611322ea07f40b70dede193f22012ab646888843310001b8d65',
    'bootstrap_host.py': 'f8bab8230f91b742c3f9c2db881261310e1ae9d25a0724a67a94ef5392b3cf7e',
    'mcp_adapter.py': '7c76ebd0af1debdb0dfc1da0f352d8d36de6cb8d6d2b11cd77d0df529a934c8e',
    'node_acquisition.py': '7b9527a99da4c2433f88f82dcb51abdab7a2c94a9a657307b4c5794a0b5af924',
    'prepare_host.py': '3ea3461356bc64af830d8c27af9534119aaec7c1770548ee9c3769b792cf9313',
    'python_acquisition.py': '7721011d0e67e9e66214913259eead0f4bd66e7c917dc26e452ffc944152503b'}
_RECOVERY_SETUP_FILES = {
    'acquired_inspection.py': '2fa1b271c2f0627a56f582658ceb42ff7fccf8922a3f9dd50221d7ada4f0a67c',
    'backend.py': '8eba6272841cfe7c57565e9162a5b99769853b626582111dca867bd1109e9069',
    'bootstrap_host.py': 'bf25f4506f193fdeb5891ab36a0c883b3943bbdbccbc9178e3519fe97a862bf5',
    'compose_acquisition.py': '9441fdc96dbfd8eb480d6d3c5277fdaf66ad359dd2b711d3cd3b819917c2349e',
    'compose_bridge.py': 'f6cc17465f9cc1aea5dae78e8fe3e3c8ffe9a306969d60b3d09a963e07545f05',
    'compose_runtime.py': 'cb40cf0fdfe6e24c4a356a60f1cfcc105056a10630dea49b56c2320dd206a4a7',
    'mcp_adapter.py': 'f99de44753a9985922b26cb06375071e292e4ae3af8d41df5c1becc6944755d6',
    'mcpb_acquisition.py': '5b3d074c0c8e84977602caf231bec07af7aa8bae8bcb9fe031d872f7739e8387',
    'node_acquisition.py': '7b9527a99da4c2433f88f82dcb51abdab7a2c94a9a657307b4c5794a0b5af924',
    'prepare_host.py': '05458fe9d92e1dfd5f9df17d6a6e78619853534cedb145f899bfcf8d84b0caa6',
    'python_acquisition.py': '7721011d0e67e9e66214913259eead0f4bd66e7c917dc26e452ffc944152503b'}
_SERVICES_0121_SETUP_FILES = {
    **_RECOVERY_SETUP_FILES,
    'backend.py': 'f75a9025dfa734b1185935f6d194ada78f3762dc918938f270458a2b70c4217b',
    'mcp_adapter.py': 'a788dbdd7ad1dfeff3c9d38629cf6b46ffd50fe482621a8eeedcba90f6127eac'}
_SAVED_SETUP_FILES = {
    **_SERVICES_0121_SETUP_FILES,
    'bootstrap_host.py': '40c8332239cb92984eba0ab8ee8398f1ef583055fc127cbf84063cb7f0c44671',
    'prepare_host.py': 'e56ab28edec1d18d498065a97062983b4449a1991f7ba3db162cde3eefbb27f2'}
_PREPARED_RELEASES = {
    '8ee627a9cc489c5375fff31de9c01a188e31b2f4cc2071b20c9556652e8b0db1': {
        'target': 'macos-arm64', 'root': 'toolkit-library-20261004-host-bootstrap-r2-macos-arm64',
        'contents_sha256': 'e19c48feb300e188991e5608cf8d1a087cda9768e541eb8bcea5ff1f70e96625',
        'selection_sha256': '72b53ff07418d7016221c1b08c60585e6383a210a582109b0fb9304c9594fe13',
        'setup_installer_sha256': 'ed81e546d0180986d6f80f4cdb1c105fd124d4e3fdba0e21d8eb3d66c757de16'},
    '0d7fef536979dd036f341125cba2e127135a01598b9ec28fd01307f3a572d7e5': {
        'target': 'linux-x86_64', 'root': 'toolkit-library-20261004-host-bootstrap-r2-linux-x86_64',
        'contents_sha256': 'd8d11eee8e4bb965f344cc0536640a321dbd21102aba5d5a788c345ba0d5d326',
        'selection_sha256': '2550ff35f3d30ae482d97117ade9ff4eb86b342b0d1c5d801a2975a687111f1f',
        'setup_installer_sha256': '0432bda2033ab6e3cccee3ffa0cb03d569fe4d77754f2d67da49001a730d7a68'},
    'ac3050bf8b9507dd2aba45f3d063e6948c089519bbcdfd065bc57046407621c2': {
        'target': 'macos-arm64', 'root': 'toolkit-library-20261004-host-recovery-r2-macos-arm64',
        'contents_sha256': 'e19c48feb300e188991e5608cf8d1a087cda9768e541eb8bcea5ff1f70e96625',
        'selection_sha256': '85f885ff3d42794b292268fe0b049041cf50b494e9d227c8426b0c2b08db08ba',
        'setup_installer_sha256': 'ed81e546d0180986d6f80f4cdb1c105fd124d4e3fdba0e21d8eb3d66c757de16',
        'setup_files': _RECOVERY_SETUP_FILES, 'recovery': True},
    '3bb80189b5f23df31d7e1fcac85d5204e41196cd5cd33613de9cb3029b9d4214': {
        'target': 'linux-x86_64', 'root': 'toolkit-library-20261004-host-recovery-r2-linux-x86_64',
        'contents_sha256': 'd8d11eee8e4bb965f344cc0536640a321dbd21102aba5d5a788c345ba0d5d326',
        'selection_sha256': '2550ff35f3d30ae482d97117ade9ff4eb86b342b0d1c5d801a2975a687111f1f',
        'setup_installer_sha256': '0432bda2033ab6e3cccee3ffa0cb03d569fe4d77754f2d67da49001a730d7a68',
        'setup_files': _RECOVERY_SETUP_FILES, 'recovery': True},
    '876a62c79de0de7b254aaef374e4afb0f24645c5c1bd936ad3f272c38a2ff2ab': {
        'target': 'macos-arm64', 'root': 'toolkit-library-20261004-gmail-structured-draft-macos-arm64',
        'contents_sha256': '8653556a62f8007b3a749359468b0e8a88b95e9d5ba6f6eb19d2a4b18a23d5c3',
        'catalog_sha256': 'cb775748d9a2db839a31e83571d7c0ba1ffed25a08f30e7d9f70c34752eccec9',
        'verifier_sha256': '106ba8c23a411ed16f119b3345dec6574bfab27bc72b794ec63f3859c8230c1c',
        'selection_sha256': '85f885ff3d42794b292268fe0b049041cf50b494e9d227c8426b0c2b08db08ba',
        'setup_installer_sha256': 'ed81e546d0180986d6f80f4cdb1c105fd124d4e3fdba0e21d8eb3d66c757de16',
        'setup_files': _SERVICES_0121_SETUP_FILES, 'recovery': True},
    'a6bf5d313dad70e63413259169f24f45083f40664ea37a1730b20834a6279ba5': {
        'target': 'linux-x86_64', 'root': 'toolkit-library-20261004-gmail-structured-draft-linux-x86_64',
        'contents_sha256': '6dd825d0e362bb8219c75babb6a698407a16787df64ed12c11b13c7117e621e7',
        'catalog_sha256': 'cb775748d9a2db839a31e83571d7c0ba1ffed25a08f30e7d9f70c34752eccec9',
        'verifier_sha256': '106ba8c23a411ed16f119b3345dec6574bfab27bc72b794ec63f3859c8230c1c',
        'selection_sha256': '2550ff35f3d30ae482d97117ade9ff4eb86b342b0d1c5d801a2975a687111f1f',
        'setup_installer_sha256': '0432bda2033ab6e3cccee3ffa0cb03d569fe4d77754f2d67da49001a730d7a68',
        'setup_files': _SERVICES_0121_SETUP_FILES, 'recovery': True},
    '70e5a85a1de295384311962008dd2983b244264c4e822eb46e804ebd8d98294c': {
        'target': 'macos-arm64', 'root': 'toolkit-library-20261005-saved-setup-macos-arm64',
        'contents_sha256': '8653556a62f8007b3a749359468b0e8a88b95e9d5ba6f6eb19d2a4b18a23d5c3',
        'catalog_sha256': 'cb775748d9a2db839a31e83571d7c0ba1ffed25a08f30e7d9f70c34752eccec9',
        'verifier_sha256': '106ba8c23a411ed16f119b3345dec6574bfab27bc72b794ec63f3859c8230c1c',
        'selection_sha256': '85f885ff3d42794b292268fe0b049041cf50b494e9d227c8426b0c2b08db08ba',
        'setup_installer_sha256': 'ed81e546d0180986d6f80f4cdb1c105fd124d4e3fdba0e21d8eb3d66c757de16',
        'setup_files': _SAVED_SETUP_FILES, 'recovery': True, 'saved_setup': True},
    '638f681ef82114b97f0cd1dd17835260bb8a46bfa025eb0b0708f36870cc31d3': {
        'target': 'linux-x86_64', 'root': 'toolkit-library-20261005-saved-setup-linux-x86_64',
        'contents_sha256': '6dd825d0e362bb8219c75babb6a698407a16787df64ed12c11b13c7117e621e7',
        'catalog_sha256': 'cb775748d9a2db839a31e83571d7c0ba1ffed25a08f30e7d9f70c34752eccec9',
        'verifier_sha256': '106ba8c23a411ed16f119b3345dec6574bfab27bc72b794ec63f3859c8230c1c',
        'selection_sha256': '2550ff35f3d30ae482d97117ade9ff4eb86b342b0d1c5d801a2975a687111f1f',
        'setup_installer_sha256': '0432bda2033ab6e3cccee3ffa0cb03d569fe4d77754f2d67da49001a730d7a68',
        'setup_files': _SAVED_SETUP_FILES, 'recovery': True, 'saved_setup': True}}
_PREPARED_CATALOG_SHA256 = '6794dc09196334b5113ea7db7c6a85fd1734842ef311298bc45b8fb0040d0ce2'
_PREPARED_VERIFIER_SHA256 = '106ba8c23a411ed16f119b3345dec6574bfab27bc72b794ec63f3859c8230c1c'


class ToolkitSetupUnavailable(RuntimeError):
    pass


def _file(path, maximum):
    path = Path(path)
    info = path.lstat()
    if (not stat.S_ISREG(info.st_mode) or info.st_uid not in {0, os.getuid()}
            or info.st_mode & 0o022 or not 0 < info.st_size <= maximum):
        raise ValueError('Invalid trusted setup input.')
    return path.read_bytes()


def _path(value):
    if not isinstance(value, str) or not value or '\0' in value or '${' in value:
        raise ValueError('Invalid host path.')
    path = Path(value)
    if not path.is_absolute():
        raise ValueError('Host paths must be absolute.')
    return path


def _directory(value):
    path = _path(value)
    info = path.lstat()
    if (not stat.S_ISDIR(info.st_mode) or info.st_uid != os.getuid()
            or info.st_mode & 0o022 or path.resolve(strict=True) != path):
        raise ValueError('Invalid owned setup directory.')
    return path


def _pinned_file(path, expected, maximum=4_000_000):
    if (not isinstance(expected, str) or re.fullmatch('[a-f0-9]{64}', expected) is None
            or hashlib.sha256(_file(path, maximum)).hexdigest() != expected):
        raise ValueError('The prepared setup input changed.')


def _runtime(pin):
    if (not isinstance(pin, dict) or set(pin) != {'path', 'resolved_path', 'sha256', 'executable'}
            or pin['executable'] is not True):
        raise ValueError('Invalid prepared runtime pin.')
    path, resolved = _path(pin['path']), _path(pin['resolved_path'])
    if path.resolve(strict=True) != resolved or not os.access(path, os.X_OK):
        raise ValueError('The prepared runtime changed.')
    _pinned_file(resolved, pin['sha256'], 128_000_000)
    return path


def _prepared_binding(path, value):
    parent = _directory(str(path.parent))
    receipt = json.loads(_file(parent / 'prepared.json', 32768))
    recovering = isinstance(receipt, dict) and 'recovery' in receipt
    has_prior = isinstance(receipt, dict) and 'prior_selection' in receipt
    receipt_keys = {'schema_version', 'status', 'scope', 'release', 'target', 'workspace',
        'runtimes', 'public_pypi_enabled', 'public_npm_enabled', 'files'}
    if recovering:
        receipt_keys.add('recovery')
    if has_prior:
        receipt_keys.add('prior_selection')
    selection_file = 'recover-manifest.json' if recovering else 'selected-install.json'
    file_names = {'mcp.json', selection_file} | ({'prior-request.json'} if has_prior else set())
    if (not isinstance(receipt, dict) or set(receipt) != receipt_keys
            or type(receipt['schema_version']) is not int or receipt['schema_version'] != 1
            or receipt['status'] != 'prepared' or receipt['scope'] != 'host_setup_configuration_only'
            or receipt['public_pypi_enabled'] is not False or receipt['public_npm_enabled'] is not False
            or not isinstance(receipt['files'], dict)
            or set(receipt['files']) != file_names or path.name != 'mcp.json' or recovering and has_prior):
        raise ValueError('Invalid prepared software setup scope.')
    for name, digest in receipt['files'].items():
        _pinned_file(parent / name, digest)
    release = receipt['release']
    if (not isinstance(release, dict) or set(release) != {'archive_sha256', 'archive_root', 'directory'}
            or not isinstance(release['archive_sha256'], str)):
        raise ValueError('Invalid prepared release.')
    known = _PREPARED_RELEASES.get(release['archive_sha256'])
    if known is None or release['archive_root'] != known['root'] or receipt['target'] != known['target']:
        raise ValueError('The prepared release is not supported.')
    if recovering and known.get('recovery') is not True:
        raise ValueError('The prepared release does not support recovery.')
    if has_prior:
        prior = receipt['prior_selection']
        if (known.get('saved_setup') is not True or not isinstance(prior, dict)
                or set(prior) != {'source', 'manifest_sha256', 'request_id', 'request_sha256'}
                or prior['source'] != 'recovery_manifest'
                or not isinstance(prior['manifest_sha256'], str)
                or re.fullmatch('[a-f0-9]{64}', prior['manifest_sha256']) is None
                or not isinstance(prior['request_id'], str)
                or re.fullmatch('[a-f0-9]{32}', prior['request_id']) is None
                or prior['request_sha256'] != receipt['files']['prior-request.json']):
            raise ValueError('Invalid prepared saved request selection.')
    root = _directory(release['directory'])
    if root.name != known['root']:
        raise ValueError('The prepared release directory changed.')
    setup_files = {**known.get('setup_files', _PREPARED_SETUP_FILES),
        'install-library-consumer.py': known['setup_installer_sha256']}
    setup = _directory(str(root / 'setup'))
    if {source.name for source in setup.glob('*.py')} != setup_files.keys():
        raise ValueError('The setup source inventory changed.')
    for name, digest in setup_files.items():
        _pinned_file(setup / name, digest, 2_000_000)
    bundle = _directory(str(root / 'consumers'))
    catalog_sha256 = known.get('catalog_sha256', _PREPARED_CATALOG_SHA256)
    verifier_sha256 = known.get('verifier_sha256', _PREPARED_VERIFIER_SHA256)
    _pinned_file(bundle / 'CONTENTS.json', known['contents_sha256'])
    _pinned_file(bundle / 'catalog.json', catalog_sha256)
    _pinned_file(bundle / 'install-library-consumer.py', verifier_sha256)
    catalog = json.loads(_file(bundle / 'catalog.json', 4_000_000))
    if not isinstance(catalog, list) or not 1 <= len(catalog) <= 1000:
        raise ValueError('Invalid prepared catalog.')
    selection = root / 'selected/provenance/selection.json'
    _pinned_file(selection, known['selection_sha256'])
    selected = json.loads(_file(selection, 4_000_000))
    if recovering:
        recovery = receipt['recovery']
        manifest = json.loads(_file(parent / selection_file, 131072))
        if (not isinstance(recovery, dict)
                or set(recovery) != {'mode', 'manifest_sha256', 'workspace', 'requests'}
                or recovery['mode'] != 'readback_only'
                or recovery['manifest_sha256'] != receipt['files'][selection_file]
                or not isinstance(manifest, dict) or set(manifest) != {'schema_version', 'workspace', 'requests'}
                or type(manifest['schema_version']) is not int or manifest['schema_version'] != 1
                or manifest['workspace'] != recovery['workspace']
                or not isinstance(manifest['requests'], list) or not 1 <= len(manifest['requests']) <= 256
                or type(recovery['requests']) is not int or recovery['requests'] != len(manifest['requests'])):
            raise ValueError('Invalid prepared recovery selection.')
        identifiers = set()
        for entry in manifest['requests']:
            if (not isinstance(entry, dict) or set(entry) != {'id', 'sha256'}
                    or not isinstance(entry['id'], str) or re.fullmatch('[a-f0-9]{32}', entry['id']) is None
                    or entry['id'] in identifiers or not isinstance(entry['sha256'], str)
                    or re.fullmatch('[a-f0-9]{64}', entry['sha256']) is None):
                raise ValueError('Invalid prepared recovery anchor.')
            identifiers.add(entry['id'])
        original = _directory(recovery['workspace'])
    else:
        profile = json.loads(_file(parent / selection_file, 32768))
        profile_keys = {'schema_version', 'scope', 'staging_root', 'installer', 'validators'}
        if known['target'] == 'linux-x86_64':
            profile_keys.add('python_target')
        if (not isinstance(profile, dict) or set(profile) != profile_keys
                or type(profile['schema_version']) is not int or profile['schema_version'] != 1
                or profile['scope'] != SCOPE or not isinstance(profile['validators'], dict)
                or set(profile['validators']) != {'python'} or not isinstance(selected, dict)
                or selected.get('platform') != known['target'] or not isinstance(selected.get('files'), dict)):
            raise ValueError('Invalid prepared installation profile.')
        if known['target'] == 'linux-x86_64' and profile['python_target'] != known['target']:
            raise ValueError('The prepared Python target changed.')
        _directory(profile['staging_root'])
        for pin, name in ((profile['installer'], 'bin/install-library-consumer.py'),
                (profile['validators']['python'], 'bin/toolkit-python-plan')):
            source = root / 'selected' / name
            if (not isinstance(pin, dict) or set(pin) != {'path', 'sha256'}
                    or pin['path'] != str(source) or pin['sha256'] != selected['files'].get(name)):
                raise ValueError('The prepared installation tools changed.')
            _pinned_file(source, pin['sha256'], 64_000_000)
    runtimes = receipt['runtimes']
    if not isinstance(runtimes, dict) or set(runtimes) != {'python', 'mcp_python'}:
        raise ValueError('Invalid prepared runtimes.')
    python, mcp_python = _runtime(runtimes['python']), _runtime(runtimes['mcp_python'])
    workspace = _path(receipt['workspace'])
    _directory(str(workspace.parent))
    if workspace.exists() or workspace.is_symlink():
        raise ValueError('The prepared workspace already belongs to a setup session.')
    if recovering and any(original.is_relative_to(destination) or destination.is_relative_to(original)
            for destination in (parent, workspace)):
        raise ValueError('Recovery must preserve the original workspace separately.')
    servers = value.get('mcpServers')
    if not isinstance(servers, dict) or set(servers) != {'toolkit-setup'}:
        raise ValueError('Invalid prepared MCP selection.')
    launch = servers['toolkit-setup']
    if (not isinstance(launch, dict) or set(launch) != {'command', 'args'}
            or launch['command'] != '/usr/bin/env' or not isinstance(launch['args'], list)
            or len(launch['args']) != (29 if has_prior else 25)
            or any(not isinstance(arg, str) for arg in launch['args'])):
        raise ValueError('Invalid prepared MCP command.')
    args = launch['args']
    families = args[14].split(',')
    if (not families or len(set(families)) != len(families)
            or any(family not in {'agno', 'services'} for family in families)):
        raise ValueError('The selected setup families require different host runtimes.')
    expected = ['-i', 'PATH=/usr/bin:/bin', 'PYTHONDONTWRITEBYTECODE=1', str(mcp_python), '-I', '-B',
        str(setup / 'mcp_adapter.py'), '--bundle', str(bundle), '--contents-sha256', known['contents_sha256'],
        '--workspace', str(workspace), '--families', args[14], '--verifier', str(bundle / 'install-library-consumer.py'),
        '--verifier-sha256', verifier_sha256, '--python', str(python),
        '--recover-manifest' if recovering else '--selected-install-profile', str(parent / selection_file),
        '--recover-manifest-sha256' if recovering else '--selected-install-profile-sha256',
        receipt['files'][selection_file]]
    if has_prior:
        expected += ['--prior-request', str(parent / 'prior-request.json'),
            '--prior-request-sha256', prior['request_sha256']]
    if args != expected:
        raise ValueError('The prepared MCP command changed.')
    return {'scope': RECOVERY_SCOPE if recovering else SCOPE, 'mode': 'recovery' if recovering else 'setup',
        'command': launch['command'], 'args': copy.deepcopy(args),
        'workspace_parent': str(parent), 'workspace': str(workspace), 'families': families,
        'contents_sha256': known['contents_sha256'], 'catalog_sha256': catalog_sha256,
        'catalog_count': len(catalog), **({'recovery_count': recovery['requests']} if recovering else {}),
        **({'prior_selection': copy.deepcopy(prior)} if has_prior else {})}


def _binding(path):
    path = _path(str(path))
    value = json.loads(_file(path, 32768))
    if isinstance(value, dict) and set(value) == {'mcpServers'}:
        return _prepared_binding(path, value)
    expected = {'scope', 'command', 'args', 'workspace_parent', 'contents_sha256',
        'catalog_sha256', 'catalog_count', 'setup_files'}
    if not isinstance(value, dict) or set(value) != expected or value['scope'] != SCOPE:
        raise ValueError('Invalid software setup scope.')
    for key in ('contents_sha256', 'catalog_sha256'):
        if not isinstance(value[key], str) or re.fullmatch('[a-f0-9]{64}', value[key]) is None:
            raise ValueError('Invalid setup pin.')
    if type(value['catalog_count']) is not int or not 1 <= value['catalog_count'] <= 1000:
        raise ValueError('Invalid catalog count.')
    command = _path(value['command'])
    if not command.is_file() or not os.access(command, os.X_OK):
        raise ValueError('Invalid host interpreter.')
    args = value['args']
    if (not isinstance(args, list) or len(args) != 11
            or any(not isinstance(item, str) for item in args)):
        raise ValueError('Invalid fixed setup arguments.')
    adapter, bundle = _path(args[2]), _path(args[8])
    families = args[4].split(',')
    if (not families or len(set(families)) != len(families)
            or any(item not in {'agno', 'services'} for item in families)):
        raise ValueError('The selected setup families require different host runtimes.')
    if args != ['-I', '-B', str(adapter), '--families', args[4], '--python', str(command),
            '--bundle', str(bundle), '--contents-sha256', value['contents_sha256']]:
        raise ValueError('Invalid fixed setup command.')
    if adapter.name != 'mcp_adapter.py':
        raise ValueError('Invalid setup adapter.')
    files = value['setup_files']
    if (not isinstance(files, dict) or not {'mcp_adapter.py', 'backend.py'} <= files.keys()
            or len(files) > 32 or any(re.fullmatch('[a-zA-Z0-9_-]+\\.py', name) is None
                or not isinstance(digest, str) or re.fullmatch('[a-f0-9]{64}', digest) is None
                for name, digest in files.items())):
        raise ValueError('Invalid frozen setup source.')
    actual = {source.name for source in adapter.parent.glob('*.py')}
    if actual != files.keys():
        raise ValueError('The setup source inventory changed.')
    for name, digest in files.items():
        if hashlib.sha256(_file(adapter.parent / name, 2_000_000)).hexdigest() != digest:
            raise ValueError('The setup source changed.')
    for name, key in (('CONTENTS.json', 'contents_sha256'), ('catalog.json', 'catalog_sha256')):
        if hashlib.sha256(_file(bundle / name, 4_000_000)).hexdigest() != value[key]:
            raise ValueError('The setup bundle selection changed.')
    parent = _path(value['workspace_parent'])
    info = parent.lstat()
    if not stat.S_ISDIR(info.st_mode) or info.st_uid != os.getuid() or info.st_mode & 0o022:
        raise ValueError('Invalid owned setup workspace parent.')
    return {**value, 'families': families}


class ToolkitSetup:
    def __init__(self, config_path=None):
        self.config_path = config_path
        self.connection = None
        self.binding = None
        self.workspace = None
        self.state = 'disabled'
        self.lock = asyncio.Lock()
        self.schemas = {}
        self.checked_selections = {}
        self.catalog = ToolkitCatalog()

    def status(self):
        recovering = self.binding is not None and self.binding.get('mode') == 'recovery'
        ready_detail = ('Toolkit historical setup readback is enabled. Installation, retry, authentication '
            'and agent grants are unavailable.' if recovering else 'Toolkit software setup is enabled. '
            'Authentication, MCP execution and agent grants remain separate.')
        result = {'available': self.state == 'ready', 'status': self.state,
            'mode': 'recovery' if recovering else 'setup', 'scope': RECOVERY_SCOPE if recovering else SCOPE,
            'detail': (ready_detail if self.state == 'ready'
                else 'Optional Toolkit software setup is not enabled or could not start.')}
        if self.state == 'ready' and self.binding.get('prior_selection'):
            result['prior_selection'] = copy.deepcopy(self.binding['prior_selection'])
        return result

    async def start(self):
        async with self.lock:
            if self.connection is not None:
                return
            selected = self.config_path or os.environ.get(CONFIG_ENV)
            if not selected:
                self.state = 'disabled'
                return
            self.state = 'unavailable'
            try:
                self.binding = _binding(selected)
                self.catalog.binding = self.binding
                session = Path(self.binding['workspace_parent']) / ('setup-' + uuid.uuid4().hex)
                session.mkdir(mode=0o700)
                for name in ('home', 'tmp'):
                    (session / name).mkdir(mode=0o700)
                self.workspace = Path(self.binding['workspace']) if 'workspace' in self.binding else session / 'workspace'
                args = self.binding['args'] if 'workspace' in self.binding else [*self.binding['args'], '--workspace', str(self.workspace)]
                self.connection = MCPConnection({'name': 'Toolkit software setup', 'transport': 'stdio',
                    'preserve_tool_errors': True,
                    'command': self.binding['command'],
                    'args': args,
                    'env': {'PATH': '/usr/bin:/bin', 'HOME': str(session / 'home'),
                        'TMPDIR': str(session / 'tmp'), 'TEMP': str(session / 'tmp'),
                        'LANG': 'C.UTF-8', 'PYTHONDONTWRITEBYTECODE': '1'}})
                await self.connection.start()
                recovering = self.binding.get('mode') == 'recovery'
                required = RECOVERY_TOOLS if recovering else REQUIRED_TOOLS
                enabled = RECOVERY_TOOLS if recovering else TOOLS
                prior = self.binding.get('prior_selection')
                if prior:
                    required = required | {'setup_successor'}
                else:
                    enabled = enabled - {'setup_successor'}
                if not required <= self.connection.tools.keys():
                    raise ValueError('Required setup tools are missing.')
                for name in enabled & self.connection.tools.keys():
                    schema = self.connection.tools[name].inputSchema
                    Draft202012Validator.check_schema(schema)
                    self.schemas[name] = copy.deepcopy(schema)
                if not recovering:
                    catalog = self._result('setup_catalog', {}, await self.connection.call('setup_catalog', {}))
                    if 'error' in catalog:
                        raise ValueError('The selected catalog is unavailable.')
                state = self._result('setup_state', {}, await self.connection.call('setup_state', {}))
                families = state.get('families')
                if (state.get('mode') != ('recovery' if recovering else 'setup') or not isinstance(families, list)
                        or len(families) != len(self.binding['families'])
                        or any(not isinstance(row, dict) for row in families)
                        or {row.get('id') for row in families} != set(self.binding['families'])):
                    raise ValueError('The setup session differs from the host selection.')
                if prior:
                    selection = state.get('prior_selection')
                    if (not isinstance(selection, dict) or set(selection) != {'family', 'profile', 'request_sha256'}
                            or selection['request_sha256'] != prior['request_sha256']
                            or selection['family'] not in self.binding['families']
                            or not isinstance(selection['profile'], str) or not 1 <= len(selection['profile']) <= 128):
                        raise ValueError('The saved request differs from the host selection.')
                if recovering and (not isinstance(state.get('job_history'), dict)
                        or state['job_history'].get('recovery_selection') is not True
                        or type(state['job_history'].get('count')) is not int
                        or state['job_history'].get('count') != self.binding['recovery_count']
                        or any(not isinstance(state.get(name), dict) or state[name].get('available') is not False
                            for name in ('catalog', 'stop', 'preparation_retry', 'selected_retry', 'export_retry'))):
                    raise ValueError('The recovery session differs from the host selection.')
                self.state = 'ready'
            except BaseException as error:
                await self._close()
                if not isinstance(error, Exception):
                    raise

    def _result(self, name, arguments, response):
        value = response.get('structuredContent') if isinstance(response, dict) else None
        if not isinstance(value, dict) or len(json.dumps(value).encode()) > 2_000_000:
            raise ValueError('Invalid setup response.')
        if response.get('isError'):
            # The selected Toolkit owner emits fixed error codes and repair
            # guidance. Preserve a known refusal without inventing lost admission.
            if (set(value) - {'error', 'diagnostic'} or not isinstance(value.get('error'), str)
                    or re.fullmatch('[a-z][a-z0-9_]{0,99}', value['error']) is None):
                raise ValueError('Invalid setup error.')
            return copy.deepcopy(value)
        if name in {'setup_catalog', 'setup_catalog_entry'}:
            return self.catalog._checked(name, arguments, response)
        return copy.deepcopy(value)

    async def call(self, name, arguments):
        if name not in TOOLS or name not in self.schemas:
            raise ValueError('Only enabled Toolkit software setup tools are permitted.')
        if not isinstance(arguments, dict) or len(json.dumps(arguments).encode()) > 65536:
            raise ValueError('Invalid setup arguments.')
        try:
            Draft202012Validator(self.schemas[name], registry=Registry()).validate(arguments)
        except Exception:
            raise ValueError('Use the advertised Toolkit setup arguments.') from None
        arguments = copy.deepcopy(arguments)
        async with self.lock:
            if self.state != 'ready' or self.connection is None:
                raise ToolkitSetupUnavailable(UNAVAILABLE)
            if name == 'setup_successor' and arguments.get('prior_request_sha256') != self.binding['prior_selection']['request_sha256']:
                raise ValueError('Review the exact host-selected saved request.')
            try:
                result = self._result(name, arguments, await self.connection.call(name, arguments))
                if (name == 'setup_check' and result.get('scope') == 'configuration_only'
                        and result.get('family') == arguments.get('family')
                        and result.get('profile') == arguments.get('profile')
                        and result.get('contents_sha256') == self.binding['contents_sha256']
                        and isinstance(result.get('configuration_sha256'), str)
                        and re.fullmatch('[a-f0-9]{64}', result['configuration_sha256'])
                        and isinstance(arguments.get('operations'), list)
                        and arguments['operations']
                        and all(isinstance(value, str) for value in arguments['operations'])):
                    key = (result['family'], result['profile'], result['configuration_sha256'])
                    self.checked_selections[key] = copy.deepcopy(arguments['operations'])
                return result
            except Exception:
                # Keep the same session for exact-job readback after a lost reply.
                # An observation failure never repeats an installation.
                raise ToolkitSetupUnavailable(UNAVAILABLE) from None

    def register_tools(self, draft, *, handoff=None):
        if draft.profile_name != 'connectors' or self.state != 'ready':
            return
        recovering = self.binding.get('mode') == 'recovery'
        scope = (' Host scope: retained setup readback only. Historical outcomes stay unconfirmed; '
            'reconciliation is an independent observation. No installation, retry, stop, authentication, '
            'native attachment or agent grants.' if recovering else ' Host scope: local software setup only. '
            'No credentials in arguments, account authorization, native attachment or agent grants. '
            'Observe the exact admitted job after a lost reply; do not repeat effects.')
        for name, schema in self.schemas.items():
            tool = self.connection.tools[name]
            async def dispatch(arguments, selected=name):
                return await self.call(selected, arguments)
            prior_scope = (f" Host-selected saved request: {self.binding['prior_selection']['request_sha256']}. "
                'Review current settings with setup_successor; select current operations explicitly before setup_check/install. '
                'Prior settings and operation names do not transfer account access, jobs, grants or agent sessions.'
                if name == 'setup_successor' else '')
            draft.register_tool(name, dispatch, input_schema=copy.deepcopy(schema),
                description=(tool.description or name) + scope + prior_scope)
        if recovering:
            return
        draft.register_tool('inspect_toolkit_installation', handoff or self.handoff,
            input_schema={'type': 'object', 'properties': {
                'job_id': {'type': 'string', 'pattern': '^[a-f0-9]{32}$'},
                'connection_id': {'type': 'string', 'pattern': '^[a-z][a-z0-9_-]{0,39}$'},
                'name': {'type': 'string', 'minLength': 1, 'maxLength': 100}},
                'required': ['job_id', 'connection_id', 'name'], 'additionalProperties': False},
            description='Read a succeeded installation from this same Toolkit owner and return its exact ungranted MCP connection proposal with private credential-reference slots. Software installation only; no account authentication, MCP launch or agent grants. Absolute paths are current-host-only. The architect must separately publish a compatible proposal and explicit tools. Never rewrite or infer the installed command.')

    async def handoff(self, arguments):
        from .toolkit_handoff import normalize_toolkit_handoff
        if self.binding is not None and self.binding.get('mode') == 'recovery':
            raise ValueError('Historical setup readback cannot authorize an installation handoff.')
        if (not isinstance(arguments, dict) or set(arguments) != {'job_id', 'connection_id', 'name'}
                or not isinstance(arguments['job_id'], str)
                or re.fullmatch('[a-f0-9]{32}', arguments['job_id']) is None):
            raise ValueError('Choose an exact Toolkit job and connection identity.')
        observed = await self.call('setup_job', {'job_id': arguments['job_id']})
        job = observed.get('job')
        if not isinstance(job, dict) or job.get('id') != arguments['job_id']:
            raise ToolkitSetupUnavailable('The selected Toolkit job could not be confirmed.')
        result = normalize_toolkit_handoff(job, connection_id=arguments['connection_id'], name=arguments['name'])
        installed = job['result']
        key = (installed.get('family'), installed.get('profile'), installed.get('configuration_sha256'))
        selected = self.checked_selections.get(key)
        # Agno's maintained operation IDs are its published MCP tool names.
        # Services uses the publisher's literal mapping. Older help without
        # the selected mappings still yields only an ungranted connection.
        if (key[0] in {'agno', 'services'} and selected
                and installed.get('contents_sha256') == self.binding['contents_sha256']):
            help_value = await self.call('setup_help', {'family': key[0], 'profile': key[1]})
            rows = help_value.get('operations')
            if (help_value.get('family') != key[0] or help_value.get('profile') != key[1]
                    or not isinstance(rows, list) or not 1 <= len(rows) <= 128
                    or any(not isinstance(row, dict) or not isinstance(row.get('id'), str)
                        or re.fullmatch(r'[A-Za-z0-9_.-]{1,128}', row['id']) is None for row in rows)
                    or len({row['id'] for row in rows}) != len(rows)
                    or not set(selected) <= {row['id'] for row in rows}):
                raise ToolkitSetupUnavailable('The installed operation names could not be confirmed.')
            tool_names = copy.deepcopy(selected)
            if key[0] == 'services':
                named = [row for row in rows if 'tool' in row]
                if (any(not isinstance(row['tool'], str)
                        or re.fullmatch(r'[A-Za-z0-9_.-]{1,128}', row['tool']) is None for row in named)
                        or len({row['tool'] for row in named}) != len(named)):
                    raise ToolkitSetupUnavailable('The installed operation names could not be confirmed.')
                mapping = {row['id']: row['tool'] for row in named}
                if not set(selected) <= mapping.keys():
                    return result
                tool_names = [mapping[operation] for operation in selected]
            document = json.dumps(help_value, sort_keys=True, separators=(',', ':'), ensure_ascii=True).encode()
            result['connection']['tool_evidence'] = {
                'source_url': f"toolkit-setup:{key[0]}/{key[1]}?contents_sha256={installed['contents_sha256']}",
                'sha256': hashlib.sha256(document).hexdigest(),
                'retrieved_at': datetime.now(timezone.utc).isoformat(),
                'tool_names': tool_names}
            result['provenance']['tool_evidence_scope'] = 'selected_maintained_operation_metadata'
        return result

    async def _close(self):
        self.state = 'unavailable'
        if self.connection is not None:
            await self.connection.close()
            self.connection = None
        self.schemas.clear()
        self.checked_selections.clear()

    async def shutdown(self):
        async with self.lock:
            if self.connection is not None and self.schemas:
                self.state = 'unavailable'
                try:
                    # The MCP transport may terminate its child shortly after
                    # close. Settle the owner's separately spawned workers first.
                    async with asyncio.timeout(30):
                        before, seen, running = None, set(), []
                        while True:
                            response = self._result('setup_jobs', {},
                                await self.connection.call('setup_jobs', {'before': before}))
                            rows = response.get('jobs')
                            if ('next_before' not in response or not isinstance(rows, list)
                                    or len(seen) + len(rows) > 1000):
                                raise ValueError('Invalid owned job inventory.')
                            for row in rows:
                                identifier = row.get('id') if isinstance(row, dict) else None
                                if (not isinstance(identifier, str)
                                        or re.fullmatch('[a-f0-9]{32}', identifier) is None
                                        or identifier in seen
                                        or row.get('status') not in {'running', 'succeeded', 'unconfirmed'}):
                                    raise ValueError('Invalid owned job observation.')
                                seen.add(identifier)
                                if row['status'] == 'running':
                                    if self.binding.get('mode') == 'recovery':
                                        raise ValueError('Historical setup readback cannot stop workers.')
                                    running.append(identifier)
                            before = response.get('next_before')
                            if before is None:
                                break
                            if not rows or before != rows[-1]['id']:
                                raise ValueError('Invalid owned job cursor.')
                        for identifier in running:
                            stopped = self._result('setup_stop', {},
                                await self.connection.call('setup_stop', {'job_id': identifier}))
                            if 'error' in stopped:
                                raise ValueError('The setup owner could not request stopping.')
                            while True:
                                observed = self._result('setup_job', {},
                                    await self.connection.call('setup_job', {'job_id': identifier}))
                                job = observed.get('job')
                                if (not isinstance(job, dict) or job.get('id') != identifier
                                        or job.get('status') not in {'running', 'succeeded', 'unconfirmed'}):
                                    raise ValueError('The owned worker could not be observed.')
                                if job['status'] != 'running':
                                    break
                                await asyncio.sleep(.2)
                except Exception:
                    raise ToolkitSetupUnavailable('Toolkit worker settlement is unconfirmed. Its existing owner is retained for cleanup; no replacement was started.') from None
            await self._close()
