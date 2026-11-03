"""Read-only Linux prerequisites and explicitly limited continuity evidence."""
import os
from pathlib import Path
import subprocess
import sys
from types import SimpleNamespace
import uuid

import pytest

from launchkit import remote_destination as controller


@pytest.fixture
def checked_host(tmp_path, monkeypatch):
    home = tmp_path / 'owned-home'
    home.mkdir(mode=0o700)
    root = home / 'src/application'
    host = {'uid': os.getuid(), 'home': str(home), 'destination': str(root),
            'platform': 'linux', 'python': '3.12.3'}
    monkeypatch.setattr(controller.platform, 'machine', lambda: 'x86_64')
    monkeypatch.setattr(controller.platform, 'libc_ver', lambda: ('glibc', '2.31'))
    monkeypatch.setattr(controller, '_distribution', lambda: {'ID': 'ubuntu'})
    monkeypatch.setattr(controller, '_probe', lambda arguments, selected: True)
    return root, host


def preflight(checked_host):
    root, host = checked_host
    return controller._preflight({}, root, str(uuid.uuid4()), {}, host)


def check(value, name):
    return next(item for item in value['checks'] if item['name'] == name)


def test_missing_python_environment_support_blocks_before_private_installation(checked_host, monkeypatch):
    root, host = checked_host
    calls = []
    def probe(arguments, selected):
        calls.append(arguments)
        return arguments[0] != sys.executable
    monkeypatch.setattr(controller, '_probe', probe)
    monkeypatch.setattr(controller, '_lingering', lambda selected: True)
    value = preflight(checked_host)
    assert value['ready'] is False
    environment = check(value, 'python-environment')
    assert environment['status'] == 'failed'
    assert 'python3-venv' in environment['detail'] and 'administrator' in environment['detail']
    assert [sys.executable, '-I', '-c', 'import venv, ensurepip; ensurepip.version()'] in calls
    assert not root.exists() and list(Path(host['home']).iterdir()) == []
    assert all('sudo' not in arguments and 'install' not in arguments for arguments in calls)


@pytest.mark.parametrize('lingering,expected,status', [
    (True, 'enabled', 'passed'), (False, 'disabled', 'warning'), (None, 'unknown', 'warning'),
])
def test_lingering_observation_never_claims_application_reboot_proof(checked_host, monkeypatch, lingering, expected, status):
    root, host = checked_host
    monkeypatch.setattr(controller, '_lingering', lambda selected: lingering)
    value = preflight(checked_host)
    assert value['ready'] is True
    assert value['continuity'] == {'user_lingering': expected, 'reboot_verified': False}
    continuity = check(value, 'user-service-continuity')
    assert continuity['status'] == status and continuity['required'] is False
    assert 'unverified' in continuity['detail']
    if lingering is not True:
        assert 'logout' in continuity['detail'] and 'administrator' in continuity['detail']
    assert not root.exists() and list(Path(host['home']).iterdir()) == []


def test_lingering_reads_only_exact_user_property_and_discards_private_environment(checked_host, monkeypatch):
    _, host = checked_host
    monkeypatch.setenv('OPENAI_API_KEY', 'private-provider-canary')
    calls = []
    def run(arguments, **kwargs):
        calls.append((arguments, kwargs))
        return SimpleNamespace(returncode=0, stdout=b'no\n')
    monkeypatch.setattr(controller.subprocess, 'run', run)
    assert controller._lingering(host) is False
    arguments, options = calls[0]
    assert arguments == ['/usr/bin/loginctl', 'show-user', str(host['uid']),
                         '--property=Linger', '--value', '--no-pager']
    assert options['timeout'] == 5 and options['stdin'] is subprocess.DEVNULL
    assert options['stderr'] is subprocess.DEVNULL
    assert 'OPENAI_API_KEY' not in options['env']
    assert 'private-provider-canary' not in repr(calls)


@pytest.mark.parametrize('output,code', [(b'', 0), (b'yes\nprivate-canary', 0),
                                      (b'y' * 33, 0), (b'yes\n', 1)])
def test_unrecognized_lingering_output_is_unknown_and_not_public(checked_host, monkeypatch, output, code):
    _, host = checked_host
    monkeypatch.setattr(controller.subprocess, 'run', lambda *args, **kwargs:
                        SimpleNamespace(returncode=code, stdout=output))
    assert controller._lingering(host) is None


@pytest.mark.parametrize('error', [OSError('private-canary'), subprocess.TimeoutExpired(['loginctl'], 5)])
def test_unavailable_lingering_observation_is_unknown(checked_host, monkeypatch, error):
    _, host = checked_host
    def unavailable(*args, **kwargs):
        raise error
    monkeypatch.setattr(controller.subprocess, 'run', unavailable)
    assert controller._lingering(host) is None


def test_distribution_advice_uses_only_bounded_identifiers(tmp_path):
    path = tmp_path / 'os-release'
    path.write_text('ID="ubuntu"\nID_LIKE="debian"\nPRETTY_NAME="private-canary"\n')
    assert controller._distribution(path) == {'ID': 'ubuntu', 'ID_LIKE': 'debian'}
    path.write_bytes(b'ID=ubuntu\n' + b'x' * (16 * 1024))
    assert controller._distribution(path) == {}
    path.write_bytes(b'ID=ubuntu\n\xff')
    assert controller._distribution(path) == {}


def test_distribution_nonregular_reader_does_not_wait(tmp_path):
    fifo = tmp_path / 'os-release'
    os.mkfifo(fifo)
    assert controller._distribution(fifo) == {}


def test_unknown_distribution_gets_generic_interpreter_guidance():
    detail = controller._python_environment_guidance({'ID': 'fedora'})
    assert 'matching Python' in detail and 'administrator' in detail
    assert 'python3-venv' not in detail and 'Ubuntu' not in detail
