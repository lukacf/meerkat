"""Source-bound executable checks without connector execution or private setup."""
from contextlib import nullcontext
import json
import os
from pathlib import Path
import uuid

import pytest

from launchkit import remote_destination as controller


def definition(commands, unused=()):
    return {'classes': [{'tools': {'mcp': [{'connection_id': key} for key in commands]}}],
            'knowledge': {'mode': 'none'},
            'mcpConnections': [{'id': key, 'transport': 'stdio', 'command': command}
                               for key, command in {**commands, **dict(unused)}.items()]}


def test_used_commands_are_deduplicated_and_private_references_are_not_expanded(monkeypatch):
    monkeypatch.setenv('PRIVATE_MCP_COMMAND', 'private-command-canary')
    source = definition({'first': 'npx', 'second': 'npx', 'python-package': '.venv/bin/python',
                         'absolute': '/opt/custom/mcp', 'configured': '${PRIVATE_MCP_COMMAND}'},
                        [('unused', 'unavailable-unused')])
    source['mcpConnections'].append({'id': 'remote', 'transport': 'http', 'url': 'https://example.test/mcp'})
    source['classes'][0]['tools']['mcp'].append({'connection_id': 'remote'})
    result = controller.connection_prerequisites(source)
    assert result == {'mcp_prerequisites': ['npx'],
                      'mcp_deferred_prerequisites': ['absolute', 'configured', 'python-package']}
    assert 'private-command-canary' not in json.dumps(result)


def test_all_32_external_connectors_plus_reserved_http_knowledge_are_supported():
    source = definition({f'connector-{index}': f'program-{index}' for index in range(32)})
    source['mcpConnections'].append({'id': 'elephant', 'transport': 'http'})
    result = controller.connection_prerequisites(source)
    assert len(result['mcp_prerequisites']) == 32 and result['mcp_deferred_prerequisites'] == []


@pytest.mark.parametrize('change', ['missing', 'duplicate', 'invalid-grant', 'invalid-command', 'overflow'])
def test_bad_used_source_cannot_silently_drop_a_requirement(change):
    source = definition({'connector': 'npx'})
    if change == 'missing':
        source.pop('mcpConnections')
    elif change == 'duplicate':
        source['mcpConnections'] *= 2
    elif change == 'invalid-grant':
        source['classes'][0]['tools']['mcp'][0]['connection_id'] = []
    elif change == 'invalid-command':
        source['mcpConnections'][0]['command'] = ''
    else:
        source = definition({f'connector-{index}': f'program-{index}' for index in range(33)})
    with pytest.raises(controller.ControllerError) as failure:
        controller.connection_prerequisites(source)
    assert failure.value.code == 'invalid-package'


def test_legacy_package_without_mcp_declarations_and_without_grants_remains_supported():
    assert controller.connection_prerequisites({'classes': [{'tools': {'mcp': []}}]}) == {
        'mcp_prerequisites': [], 'mcp_deferred_prerequisites': []}


@pytest.fixture
def host(tmp_path, monkeypatch):
    home = tmp_path / 'home'
    home.mkdir(mode=0o700)
    value = {'uid': os.getuid(), 'home': str(home), 'destination': str(home / 'src/team'),
             'platform': 'linux', 'python': '3.12.3'}
    monkeypatch.setattr(controller, '_probe', lambda arguments, selected: True)
    monkeypatch.setattr(controller, '_lingering', lambda selected: True)
    monkeypatch.setattr(controller.platform, 'machine', lambda: 'aarch64')
    monkeypatch.setattr(controller.platform, 'libc_ver', lambda: ('glibc', '2.35'))
    monkeypatch.setattr(controller, '_distribution', lambda: {'ID': 'ubuntu'})
    return value


def test_destination_checks_real_executable_presence_without_running_it(host):
    home, root = Path(host['home']), Path(host['destination'])
    binary = home / '.local/bin/launchkit-preflight-fixture'
    binary.parent.mkdir(parents=True)
    sentinel = home / 'connector-ran'
    binary.write_text('#!/bin/sh\ntouch "' + str(sentinel) + '"\n')
    binary.chmod(0o700)
    selected = controller.connection_prerequisites(definition({'test': binary.name,
                                                               'configured': '${MCP_COMMAND}'}))
    result = controller._preflight(selected, root, str(uuid.uuid4()), {}, host)
    observed = next(item for item in result['checks'] if item['name'] == 'mcp-executable:' + binary.name)
    assert observed['status'] == 'passed' and result['ready'] is True
    deferred = next(item for item in result['checks'] if item['name'] == 'mcp-command-deferred:configured')
    assert deferred['status'] == 'warning' and deferred['required'] is False
    assert 'sign-in' in deferred['detail']
    assert not root.exists() and not sentinel.exists()


def test_missing_selected_executable_blocks_before_private_install_with_specific_guidance(host):
    root = Path(host['destination'])
    request = controller.connection_prerequisites(definition({'test': 'launchkit-preflight-unavailable'}))
    result = controller._preflight(request, root, str(uuid.uuid4()), {}, host)
    observed = next(item for item in result['checks'] if item['name'].startswith('mcp-executable:'))
    assert result['ready'] is False and observed['status'] == 'failed'
    assert 'launchkit-preflight-unavailable' in observed['detail'] and 'recheck' in observed['detail']
    assert not root.exists() and not (Path(host['home']) / '.env').exists()


def test_destination_accepts_32_checks_without_silently_truncating(host, monkeypatch):
    commands = [f'program-{index}' for index in range(32)]
    seen = []
    monkeypatch.setattr(controller.shutil, 'which', lambda command, path: seen.append(command) or '/bin/fixture')
    result = controller._preflight({'mcp_prerequisites': commands}, Path(host['destination']),
                                  str(uuid.uuid4()), {}, host)
    assert result['ready'] is True and seen == commands


@pytest.mark.parametrize('action', ['install', 'retry-install'])
def test_destination_mutation_rederives_checks_from_verified_source_instead_of_caller_claims(host, monkeypatch, action):
    root, identity = Path(host['destination']), str(uuid.uuid4())
    source = definition({'test': 'launchkit-preflight-unavailable'})
    previous, operation = str(uuid.uuid4()), str(uuid.uuid4())
    request = {'operation_id': operation, 'previous_operation_id': previous, 'settings': {},
               'mcp_prerequisites': [], 'mcp_deferred_prerequisites': ['caller-fiction']}
    monkeypatch.setattr(controller, '_lock', lambda *args: nullcontext())
    monkeypatch.setattr(controller, '_folder', lambda *args: 'empty')
    monkeypatch.setattr(controller, '_fields', lambda *args, **kwargs: ([], None))
    monkeypatch.setattr(controller, '_configuration', lambda *args: {})
    if action == 'install':
        monkeypatch.setattr(controller, '_archive', lambda *args: (b'', {}, source))
        monkeypatch.setattr(controller, '_owner', lambda *args, **kwargs: None)
        invoke = lambda: controller._install(request, root, identity, {}, host, b'')
    else:
        monkeypatch.setattr(controller, '_owner', lambda *args: {'binding': {}})
        monkeypatch.setattr(controller, '_job', lambda *args: {'operation_id': previous})
        monkeypatch.setattr(controller, '_observe', lambda *args: {'worker_live': False, 'status': 'failed',
                          'source_current': True, 'retryable': True})
        monkeypatch.setattr(controller, '_accepted_files', lambda *args: {'deployment.json': json.dumps(source)})
        invoke = lambda: controller._retry_install(request, root, identity, {}, host)
    with pytest.raises(controller.ControllerError) as failure:
        invoke()
    assert failure.value.code == 'preflight-failed'
    assert not root.exists()
