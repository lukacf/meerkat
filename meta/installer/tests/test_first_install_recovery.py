"""New first-install counterexamples with synthetic Python and no worker execution."""
from copy import deepcopy
import json
import os
from pathlib import Path
import shlex
import stat
import subprocess
import sys

import pytest

from launchkit import deployment
from launchkit.blueprint import starter_blueprint
from launchkit.compiler import generate_files
from launchkit.deployment_worker import read_settings
from test_deployment import Guide, Implementation, Service, submit


def fake_python(path, kind):
    path.write_text('#!/bin/sh\n'
        'printf "%s\\n" "$*" >> "$INSTALL_TEST_LOG"\n'
        'case "$1:${2:-}" in\n'
        '  -c:*)\n'
        '    case "$2" in\n'
        '      *require_project_directory*) exec ' + shlex.quote(sys.executable) + ' -B "$@" ;;\n'
        '      *) exit 0 ;;\n'
        '    esac ;;\n'
        '  -m:venv)\n'
        '    /bin/mkdir -p "$3/bin"\n'
        '    printf "%s\\n" "Retain this partial environment" > "$3/partial-marker"\n'
        + ('    echo "Synthetic ensurepip failure" >&2; exit 42 ;;\n' if kind == 'partial' else
           '    /bin/cp "$0" "$3/bin/python"; exit 0 ;;\n')
        + '  -m:pip)\n'
        + ('    echo "Synthetic missing pip" >&2; exit 43 ;;\n' if kind == 'pip' else '    exit 0 ;;\n')
        + '  install_gateway.py:*|app.py:*|setup.py:*) exit 0 ;;\n'
        'esac\n'
        'echo "Unexpected synthetic Python invocation" >&2; exit 44\n')
    path.chmod(0o755)


@pytest.mark.parametrize('failure,mode', [('partial', ''), ('pip', '--non-interactive')])
def test_generated_installer_preserves_partial_environment_and_prints_working_manual_retry(tmp_path, failure, mode):
    root = tmp_path / 'exported application'
    root.mkdir()
    root.chmod(0o700)
    files = generate_files(starter_blueprint())
    (root / 'install.sh').write_text(files['install.sh'])
    (root / 'storage_setup.py').write_text(files['storage_setup.py'])
    (root / '.env').write_text('PRIVATE_TEST_VALUE="synthetic-retained-setting"\n')
    (root / '.mobkit').mkdir()
    (root / '.mobkit/keep').write_text('Retained application history')
    retained = {path: path.read_bytes() for path in (root / '.env', root / '.mobkit/keep')}
    broken, complete = tmp_path / 'incomplete-python', tmp_path / 'complete-python'
    fake_python(broken, failure)
    fake_python(complete, 'ready')
    log = tmp_path / 'invocations.log'
    environment = {'PATH': '/usr/bin:/bin', 'PYTHON_BIN': str(broken), 'INSTALL_TEST_LOG': str(log)}
    argv = ['/bin/bash', str(root / 'install.sh'), *([mode] if mode else [])]
    first = subprocess.run(argv, cwd=tmp_path, env=environment, capture_output=True, text=True, timeout=10)
    assert first.returncode != 0
    assert 'complete Python 3.11+' in first.stderr and 'ensurepip' in first.stderr
    assert 'stopped before installing dependencies' in first.stderr
    commands = log.read_text().splitlines()
    assert not any('pip install' in command or 'install_gateway.py' in command or 'app.py' in command for command in commands)
    partial = {path.relative_to(root / '.venv'): path.read_bytes() for path in (root / '.venv').rglob('*') if path.is_file()}
    assert partial and all(path.read_bytes() == value for path, value in retained.items())
    assert not (root / '.venv.previous').exists()

    # A complete selected Python must not silently replace a retained venv.
    environment['PYTHON_BIN'] = str(complete)
    second = subprocess.run(argv, cwd=tmp_path, env=environment, capture_output=True, text=True, timeout=10)
    assert second.returncode != 0
    assert {path.relative_to(root / '.venv'): path.read_bytes() for path in (root / '.venv').rglob('*') if path.is_file()} == partial
    assert log.read_text().splitlines().count('-m venv .venv') == 1
    backup = next(line.strip() for line in second.stderr.splitlines() if line.startswith('  test ! -e .venv.previous'))
    retry = next(line.strip() for line in second.stderr.splitlines() if line.startswith('  PYTHON_BIN='))
    assert retry == 'PYTHON_BIN="/absolute/path/to/python3.11" bash install.sh' + (' --non-interactive' if mode else '')
    # The printed guard also protects an occupied backup name represented by a dangling link.
    (root / '.venv.previous').symlink_to(root / 'absent-backup-target')
    refused = subprocess.run(['/bin/bash', '-c', backup], cwd=root, env=environment, capture_output=True, timeout=10)
    assert refused.returncode != 0 and (root / '.venv/partial-marker').read_text().strip() == 'Retain this partial environment'
    (root / '.venv.previous').unlink()
    subprocess.run(['/bin/bash', '-c', backup], cwd=root, env=environment, check=True, timeout=10)
    actual_retry = retry.replace('/absolute/path/to/python3.11', str(complete))
    recovered = subprocess.run(['/bin/bash', '-c', actual_retry], cwd=root, env=environment, capture_output=True, text=True, timeout=10)
    assert recovered.returncode == 0, recovered.stderr
    assert {path.relative_to(root / '.venv.previous'): path.read_bytes() for path in (root / '.venv.previous').rglob('*') if path.is_file()} == partial
    assert all(path.read_bytes() == value for path, value in retained.items())
    final = log.read_text().splitlines()
    assert final.count('-m venv .venv') == 2
    assert final.count('setup.py') == int(not mode)
    assert final.count('install_gateway.py') == 1


@pytest.mark.parametrize('mode', ['', '--non-interactive'])
def test_generated_installer_refuses_writable_owned_folder_before_effects_and_retries_after_repair(tmp_path, mode):
    root = tmp_path / 'exported application'
    root.mkdir()
    files = generate_files(starter_blueprint())
    for name in ('install.sh', 'storage_setup.py'):
        (root / name).write_text(files[name])
    retained = [root / '.env', root / '.mobkit/history', root / 'data/history', root / 'workspace/reference']
    for path in retained:
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(b'Retained synthetic private input\n')
        path.chmod(0o600)
    (root / '.mobkit').chmod(0o700)
    before = {path.relative_to(root): (path.read_bytes(), path.stat().st_ino, path.stat().st_mode)
              for path in root.rglob('*') if path.is_file()}
    root.chmod(0o775)
    python = tmp_path / 'selected-fixture-python'
    fake_python(python, 'ready')
    log = tmp_path / 'invocations.log'
    environment = {'PATH': '/usr/bin:/bin', 'PYTHON_BIN': str(python), 'INSTALL_TEST_LOG': str(log)}
    argv = ['/bin/bash', str(root / 'install.sh'), *([mode] if mode else [])]

    # Only the original generated stdlib folder check runs for real. The selected
    # fixture refuses neither permissions nor dependencies on the check's behalf.
    refused = subprocess.run(argv, cwd=tmp_path, env=environment, capture_output=True, text=True, timeout=10)
    assert refused.returncode != 0
    assert 'This owned project folder allows group or other write access.' in refused.stderr
    assert 'Run this non-recursive repair in this same project folder:\n  chmod go-w .' in refused.stderr
    assert 'Then retry the same install command from this folder.' in refused.stderr
    assert 'Installation complete' not in refused.stdout
    assert stat.S_IMODE(root.stat().st_mode) == 0o775
    assert {path.relative_to(root): (path.read_bytes(), path.stat().st_ino, path.stat().st_mode)
            for path in root.rglob('*') if path.is_file()} == before
    assert not (root / '.venv').exists() and not (root / '.bin').exists()
    assert not (root / '.mobkit/state').exists() and not (root / '.mobkit/launch.lock').exists()
    assert not (root / '__pycache__').exists()
    calls = log.read_text().splitlines()
    assert not any('venv .venv' in call or 'pip install' in call or 'install_gateway.py' in call
                   or 'app.py' in call or 'setup.py' == call for call in calls)

    # The operator repairs only the owned root and retries that same command.
    # Venv, pip, gateway and interview steps remain inert fixture operations.
    subprocess.run(['/bin/chmod', 'go-w', '.'], cwd=root, env=environment, check=True, timeout=10)
    continued = subprocess.run(argv, cwd=tmp_path, env=environment, capture_output=True, text=True, timeout=10)
    assert continued.returncode == 0, continued.stderr
    assert stat.S_IMODE(root.stat().st_mode) == 0o755
    assert 'Installation complete' in continued.stdout
    assert all((path.read_bytes(), path.stat().st_ino, path.stat().st_mode)
               == before[path.relative_to(root)] for path in retained)
    assert not (root / '__pycache__').exists() and not (root / '.mobkit/launch.lock').exists()
    calls = log.read_text().splitlines()
    assert calls.count('-m venv .venv') == 1
    assert calls.count('-m pip install --disable-pip-version-check -r requirements.txt') == 1
    assert calls.count('install_gateway.py') == 1 and calls.count('app.py --check') == 1
    assert calls.count('setup.py') == int(not mode)


@pytest.mark.parametrize('has_previous', [False, True])
async def test_first_private_settings_size_refusal_precedes_ownership_and_keeps_retry_usable(tmp_path, monkeypatch, has_previous):
    guide = Guide()
    implementation = Implementation(guide)
    implementation.digest = 'a' * 64
    service = Service(tmp_path / 'private-server', guide, implementation)
    implementation.application_configuration = {'fields': [
        {'key': name, 'label': name, 'secret': False, 'required': False} for name in ('SOURCE_CONTEXT', 'TEAM_CONTEXT')
    ]}
    retained = {}
    if has_previous:
        receipt = await submit(service, tmp_path / 'retained')
        await service.shutdown()
        directory = service._directory(guide.identity)
        deployment.update_receipt(directory, status='stopped', interactive_lifecycle_id=None, interactive_cleanup=None)
        retained = {path: path.read_bytes() for path in (
            directory.parent / 'latest.json', directory / 'receipt.json', tmp_path / 'retained/.env',
            tmp_path / 'retained/.mobkit/launch-deployment-owner.json')}
    destination = tmp_path / 'new destination'
    settings = {'OPENAI_API_KEY': 'synthetic-key', 'SOURCE_CONTEXT': '\u00e5' * 22000, 'TEAM_CONTEXT': '\u00e5' * 22000}
    assert len(json.dumps(settings, ensure_ascii=False).encode()) < 100000
    port_checks = []
    monkeypatch.setattr(deployment, 'require_available_console_port', lambda port: port_checks.append(port))
    try:
        with pytest.raises(ValueError, match='private configuration exceeded its safe size') as failure:
            await submit(service, destination, settings=settings)
        assert all(value not in str(failure.value) for value in settings.values())
        assert not destination.exists() and not service.tasks and not port_checks
        assert all(path.read_bytes() == value for path, value in retained.items())
        assert len(list(service.root.rglob('package.zip'))) == int(has_previous)
        if has_previous:
            assert (await service.status(guide.identity))['deployment_id'] == receipt['deployment_id']
        else:
            assert not service.root.exists()
        # A smaller retry follows the ordinary admission owner, preserving exact Unicode values.
        smaller = {**settings, 'SOURCE_CONTEXT': '\u00e5' * 20000, 'TEAM_CONTEXT': '\u00e5' * 20000}
        accepted = await submit(service, destination, settings=smaller)
        assert accepted['status'] == 'queued' and port_checks == [implementation.definition['port']]
        assert read_settings(destination) == {**smaller, 'MOBKIT_PORT': str(implementation.definition['port'])}
        assert (destination / '.env').stat().st_size <= 256 * 1024
        assert all(path.read_bytes() == value for path, value in retained.items() if path.name != 'latest.json')
    finally:
        await service.shutdown()


def test_private_settings_serialization_exact_limit_and_defensive_writer_preserve_original(tmp_path):
    fields = [{'key': f'CONTEXT_{index}', 'label': f'Context {index}', 'secret': False, 'required': False} for index in range(9)]
    values = {field['key']: 'x' * 29000 for field in fields}
    header = '# Private deployment configuration. Never commit this file.\n'
    values['MOBKIT_PORT'] = '18088'
    content = lambda: header + ''.join(f'{key}={json.dumps(value, ensure_ascii=True)}\n' for key, value in sorted(values.items()))
    extra = 256 * 1024 - len(content().encode())
    assert 0 < extra < 32768 - len(values['CONTEXT_0'])
    values['CONTEXT_0'] += 'x' * extra
    prepared = deployment.prepare_private_settings(tmp_path, fields, values, False, port=18088)
    deployment.write_private_settings(tmp_path, prepared)
    path = tmp_path / '.env'
    original = path.read_bytes()
    assert len(original) == 256 * 1024 and read_settings(tmp_path) == values
    too_large = {**values, 'CONTEXT_0': values['CONTEXT_0'] + 'x'}
    with pytest.raises(ValueError, match='private configuration exceeded its safe size'):
        deployment.write_private_settings(tmp_path, too_large)
    assert path.read_bytes() == original and not (tmp_path / '.env.launch-pending').exists()
    # Both ordinary writer paths keep the existing exact-byte ceiling.
    _, snapshot = deployment.private_configuration_snapshot(tmp_path)
    with pytest.raises(ValueError, match='private configuration exceeded its safe size'):
        deployment.replace_private_configuration(tmp_path, too_large, snapshot)
    assert path.read_bytes() == original
