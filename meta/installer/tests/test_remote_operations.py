"""Offline package/service authority tests; these do not attest a live Linux unit."""
import asyncio
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import shutil
import shlex
import signal
import socket
import subprocess
import sys
import tarfile
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from types import SimpleNamespace

import pytest

from launchkit.blueprint import starter_blueprint
from launchkit.compiler import generate_files
from launchkit import deployment_deadlines
from test_gateway_shutdown_horizon import (DRAIN_WINDOWS_MS, EARLIER_DROP_IN_BUDGETS, FOREIGN_DROP_INS,
                                           TRANSPORT_AND_ADAPTER_MARGIN, drained_horizon_ms, gateway_horizon_ms)


def load_helper(path=None):
    content = path.read_text() if path is not None else generate_files(starter_blueprint())['remote_operations.py']
    path = path or Path(__file__).parents[1] / 'launchkit/templates_v2/remote_operations.py'
    spec = importlib.util.spec_from_file_location('generated_remote_operations', path)
    module = importlib.util.module_from_spec(spec)
    # Exercise the actual standalone export, not the unrendered host template.
    exec(compile(content, str(path), 'exec'), module.__dict__)
    return module


def manifest(root):
    private = {'.env', '.mobkit', '.venv'}
    files = [item for item in root.rglob('*') if item.is_file() and item.name != 'SHA256SUMS'
             and not private.intersection(item.relative_to(root).parts)
             and item.relative_to(root).parts[0] not in {'data', 'workspace'}]
    (root / 'SHA256SUMS').write_text(''.join(
        hashlib.sha256(item.read_bytes()).hexdigest() + '  ' + item.relative_to(root).as_posix() + '\n'
        for item in sorted(files)))


@pytest.fixture
def package(tmp_path):
    root = tmp_path / 'package with spaces'
    root.mkdir(mode=0o700)
    for name, content in generate_files(starter_blueprint()).items():
        path = root / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(content)
    (root / '.env').write_text('MOBKIT_PORT="4231"\nOPENAI_API_KEY="private-canary"\n')
    (root / '.env').chmod(0o600)
    return root


def test_archive_keeps_flexible_accepted_ui_and_assets_and_excludes_private_state(package, tmp_path):
    module = load_helper()
    for name in ['assets/star-map.svg', 'assets/data/example.json', 'assets/source/icon.svg',
                 'ui/workspace/help.html', 'ui/index.html', 'docs/operations.txt', 'application/custom.py']:
        path = package / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text('ordinary accepted ' + name)
    manifest(package)
    for name in ['.mobkit/state/identity.db', 'data/results.json', 'workspace/my-private-note.txt', 'ui/.env']:
        path = package / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text('must-not-upload')
    output = tmp_path / 'upload.tar.gz'
    module.archive_package(output, package)
    with tarfile.open(output) as archive:
        names = archive.getnames()
        assert {'assets/star-map.svg', 'ui/index.html', 'docs/operations.txt', 'application/custom.py',
                'remote_operations.py', 'SHA256SUMS', 'assets/data/example.json', 'assets/source/icon.svg',
                'ui/workspace/help.html'} <= set(names)
        assert not any(module.excluded_source(name) for name in names)
        assert all(b'must-not-upload' not in archive.extractfile(name).read() for name in names)
        manifest_paths = {row.split('  ', 1)[1] for row in archive.extractfile('SHA256SUMS').read().decode().splitlines()}
        assert manifest_paths == set(names) - {'SHA256SUMS'}


def test_install_command_runs_locally_before_linux_guard(package, monkeypatch, capsys):
    module = load_helper(package / 'remote_operations.py')
    monkeypatch.setattr(sys, 'platform', 'darwin')
    monkeypatch.setattr(sys, 'argv', ['remote_operations.py', 'install-command', 'my-team', 'a' * 32, 'b' * 64])
    module.main()
    command = shlex.split(capsys.readouterr().out)
    assert command[:3] == ['python3', '-I', '-c']
    assert command[4:] == ['my-team', 'a' * 32, 'b' * 64,
                          hashlib.sha256((package / 'remote_operations.py').read_bytes()).hexdigest()]
    blueprint = starter_blueprint()
    blueprint = blueprint.model_copy(update={'target': 'gcp',
        'gcp': blueprint.gcp.model_copy(update={'project': 'forge-test-project'})})
    script = generate_files(blueprint)['deploy/deploy-gcp.sh']
    assignment = next(line for line in script.splitlines() if line.startswith('REMOTE_INSTALL='))
    assert 'remote_operations.py install-command' in assignment and '<<' not in assignment


def test_generated_install_command_verifies_helper_and_receives_exact_archive(package, tmp_path):
    module = load_helper(package / 'remote_operations.py')
    remote = tmp_path / 'remote home'
    remote.mkdir()
    archive = remote / 'mobkit-package.tar.gz'
    module.archive_package(archive, package)
    digest = module.source_digest(package)
    # Quoting must survive the shell's command string without altering arguments.
    instance = "team with 'quote'"
    args = shlex.split(module.remote_install_command(instance, 'a' * 32, digest, package))
    assert args[4:7] == [instance, 'a' * 32, digest]
    env = {**os.environ, 'HOME': str(remote)}
    refused = subprocess.run([sys.executable, *args[1:-1], '0' * 64], env=env,
                             capture_output=True, text=True, timeout=10)
    assert refused.returncode != 0 and 'differs from accepted source' in refused.stderr
    assert not (remote / 'src').exists()
    accepted = subprocess.run([sys.executable, *args[1:]], env=env,
                              capture_output=True, text=True, timeout=10)
    assert accepted.returncode == 0, accepted.stderr
    assert module.source_digest(remote / 'src' / instance) == digest
    assert not (remote / 'src' / instance / '.env').exists()


def test_changed_top_level_asset_invalidates_resume_binding(package):
    module = load_helper()
    (package / 'assets').mkdir()
    (package / 'assets/map.svg').write_text('accepted')
    manifest(package)
    before = module.source_digest(package)
    (package / 'assets/map.svg').write_text('changed')
    with pytest.raises(ValueError, match='changed'):
        module.source_digest(package)
    manifest(package)
    assert module.source_digest(package) != before


@pytest.mark.parametrize('name', ['../escape.txt', '/absolute.txt', 'assets/../escape.txt', 'assets\\escape.txt'])
def test_archive_rejects_unsafe_manifest_paths(package, name):
    module = load_helper()
    (package / 'SHA256SUMS').write_text('0' * 64 + '  ' + name + '\n')
    with pytest.raises(ValueError, match='unsafe'):
        module.source_files(package)


def test_manifest_source_links_and_duplicate_paths_are_refused(package):
    module = load_helper()
    original = (package / 'SHA256SUMS').read_text()
    (package / 'SHA256SUMS').write_text(original + original.splitlines()[0] + '\n')
    with pytest.raises(ValueError, match='duplicate'):
        module.source_files(package)
    (package / 'SHA256SUMS').write_text(original)
    (package / 'app.py').rename(package / 'elsewhere.py')
    (package / 'app.py').symlink_to(package / 'elsewhere.py')
    with pytest.raises(ValueError, match='symbolic'):
        module.source_files(package)


def fake_operations(module, package, home):
    class Fake(module.Operations):
        def __init__(self):
            super().__init__(package, home)
            self.calls, self.timeouts, self.current, self.check_failed = [], [], 'inactive', False
            self.load_state, self.control_pid, self.control_group = 'loaded', '0', ''

        def checks(self):
            self.calls.append(['checks'])
            if self.check_failed:
                raise ValueError('offline check failed')

        def manager(self, arguments, **kwargs):
            self.calls.append(arguments)
            self.timeouts.append((arguments[0], kwargs.get('timeout')))
            if arguments[0] == 'show':
                receipt = json.loads(self.path.read_text())
                return 0, ('ActiveState=' + self.current + '\nSubState=running\nMainPID=' +
                    ('317' if self.current == 'active' else '0') + '\nExecMainStatus=0\nFragmentPath=' + receipt['unit_path']
                    + '\nLoadState=' + self.load_state + '\nControlPID=' + self.control_pid
                    + '\nControlGroup=' + self.control_group)
            if arguments[0] in {'start', 'restart'}:
                self.current = 'active'
                receipt = self.load()
                module.write_private(self.health_path, {
                    'supervisor_pid': 317, 'runtime_pid': 318, 'source_digest': receipt['source_digest'],
                    'ready': True, 'observed_at': time.time(), 'console_url': 'http://127.0.0.1:4231/console',
                    'bootstrap': {'complete': True, 'ready': True, 'counts': {'broken': 0},
                                  'identities': {item['identity']: {} for item in self.spec['agents']}}})
            if arguments[0] == 'stop':
                self.current = 'inactive'
            return 0, ''
    return Fake()


def test_service_is_repeatable_exact_owned_and_preserves_private_state(package, tmp_path, monkeypatch):
    module = load_helper()
    monkeypatch.setattr(module, 'console_healthy', lambda *args: True)
    operations = fake_operations(module, package, tmp_path / 'home')
    unrelated = operations.home / '.config/systemd/user/unrelated.service'
    unrelated.parent.mkdir(parents=True)
    unrelated.write_text('other owner')
    data = package / 'data/history.json'
    data.parent.mkdir()
    data.write_text('history must survive')
    settings = (package / '.env').read_bytes()
    first = operations.start()
    receipt = operations.path.read_bytes()
    assert first['ready'] and first['scope'] == 'native-bootstrap-and-console'
    assert first['custom_interfaces_verified'] is False
    assert operations.start()['ready']
    assert operations.path.read_bytes() == receipt
    assert operations.stop()['ready'] is False
    assert operations.start(restart=True)['ready']
    assert operations.stop(remove=True)['status'] == 'service-removed'
    assert not operations.path.exists()
    assert data.read_text() == 'history must survive'
    assert (package / '.env').read_bytes() == settings
    assert unrelated.read_text() == 'other owner'


def test_exported_helper_uses_shared_budgets_without_launch_installed(package, tmp_path):
    helper = package / 'remote_operations.py'
    module = load_helper(helper)
    assert module.STOP_TIMEOUT == deployment_deadlines.APPLICATION_STOP_TIMEOUT
    assert module.FORCE_REAP_TIMEOUT == deployment_deadlines.PROCESS_FORCE_REAP_TIMEOUT
    assert module.WORKER_STOP_TIMEOUT == deployment_deadlines.WORKER_STOP_TIMEOUT
    assert module.SERVICE_STOP_TIMEOUT == deployment_deadlines.USER_SERVICE_STOP_TIMEOUT
    assert module.SERVICE_STOP_COMMAND_TIMEOUT == deployment_deadlines.SERVICE_STOP_COMMAND_TIMEOUT
    assert module.STOP_TIMEOUT + module.FORCE_REAP_TIMEOUT < module.WORKER_STOP_TIMEOUT < module.SERVICE_STOP_TIMEOUT
    assert module.SERVICE_RESTART_COMMAND_TIMEOUT > module.SERVICE_STOP_COMMAND_TIMEOUT + module.START_TIMEOUT
    # Isolated Python has neither Launch nor the test's sys.path. It still loads
    # the literal standalone helper and renders the exact new unit definition.
    script = ('import importlib.util,json,sys\n'
        'assert "launchkit" not in sys.modules\n'
        's=importlib.util.spec_from_file_location("owned_export",sys.argv[1])\n'
        'm=importlib.util.module_from_spec(s);s.loader.exec_module(m)\n'
        'print(m.unit_content({"root":sys.argv[2],"stop_timeout_seconds":m.SERVICE_STOP_TIMEOUT}).decode())\n')
    result = subprocess.run([sys.executable, '-I', '-B', '-c', script, str(helper), str(package)],
                            cwd=tmp_path, capture_output=True, text=True, timeout=10)
    assert result.returncode == 0, result.stderr
    assert f'TimeoutStopSec={deployment_deadlines.USER_SERVICE_STOP_TIMEOUT}\n' in result.stdout
    operations = fake_operations(module, package, tmp_path / 'home')
    receipt = operations.prepare()
    assert receipt['stop_timeout_seconds'] == deployment_deadlines.USER_SERVICE_STOP_TIMEOUT
    assert '__LAUNCH_SHUTDOWN_BUDGET__' not in helper.read_text()


@pytest.mark.parametrize('legacy_directory', [False, True])
def test_recorded_legacy_budget_and_unit_are_preserved_for_control(package, tmp_path, monkeypatch, legacy_directory):
    module = load_helper(package / 'remote_operations.py')
    operations = fake_operations(module, package, tmp_path / 'home')
    receipt = operations.prepare()
    receipt.pop('stop_timeout_seconds')
    unit = Path(receipt['unit_path'])
    content = module.unit_content(receipt, legacy_directory=legacy_directory)
    assert b'TimeoutStopSec=45\n' in content
    receipt['unit_sha256'] = hashlib.sha256(content).hexdigest()
    unit.write_bytes(content)
    module.write_private(operations.path, receipt)
    before = (unit.read_bytes(), operations.path.read_bytes(), (package / '.env').read_bytes())
    assert operations.status()['definition_verified'] is True
    assert operations.stop()['ready'] is False
    if not legacy_directory:
        monkeypatch.setattr(module, 'console_healthy', lambda *args: True)
        assert operations.start(restart=True)['ready'] is True
    else:
        with pytest.raises(ValueError, match='definition changed'):
            operations.start(restart=True)
    assert (unit.read_bytes(), operations.path.read_bytes(), (package / '.env').read_bytes()) == before


@pytest.mark.parametrize('budget', [None, False, True, '397', 30, 46, 398, 10**9])
def test_forged_receipt_shutdown_budget_cannot_control_even_rehashed_unit(package, tmp_path, budget):
    module = load_helper()
    operations = fake_operations(module, package, tmp_path / 'home')
    receipt = operations.prepare()
    unit = Path(receipt['unit_path'])
    current = f'TimeoutStopSec={module.SERVICE_STOP_TIMEOUT}\n'.encode()
    assert current in unit.read_bytes()
    content = unit.read_bytes().replace(current, f'TimeoutStopSec={budget}\n'.encode())
    unit.write_bytes(content)
    receipt.update(stop_timeout_seconds=budget, unit_sha256=hashlib.sha256(content).hexdigest())
    module.write_private(operations.path, receipt)
    before = (unit.read_bytes(), operations.path.read_bytes())
    for action in (operations.status, operations.stop, lambda: operations.start(restart=True)):
        with pytest.raises(ValueError, match='unsupported shutdown budget'):
            action()
    assert operations.calls == []
    assert (unit.read_bytes(), operations.path.read_bytes()) == before


@pytest.mark.parametrize('budget', ['missing', 45])
def test_budget_removal_or_legacy_claim_cannot_relabel_current_definition(package, tmp_path, budget):
    module = load_helper()
    operations = fake_operations(module, package, tmp_path / 'home')
    receipt = operations.prepare()
    if budget == 'missing':
        receipt.pop('stop_timeout_seconds')
    else:
        receipt['stop_timeout_seconds'] = budget
    module.write_private(operations.path, receipt)
    with pytest.raises(ValueError, match='definition changed'):
        operations.stop()
    assert operations.calls == []


def test_stop_and_restart_wait_for_slow_control_beyond_short_command_budget(package, tmp_path, monkeypatch):
    module = load_helper(package / 'remote_operations.py')
    operations = fake_operations(module, package, tmp_path / 'home')
    receipt = operations.prepare()
    original_manager, commands = operations.manager, []
    monkeypatch.setattr(module, 'SERVICE_STOP_COMMAND_TIMEOUT', 0.7)
    monkeypatch.setattr(module, 'SERVICE_RESTART_COMMAND_TIMEOUT', 0.9)
    monkeypatch.setattr(module, 'console_healthy', lambda *args: True)
    def delayed_manager(arguments, **kwargs):
        if arguments[0] in {'stop', 'restart'}:
            commands.append((arguments[0], kwargs.get('timeout')))
            # This real owned command exceeds the modeled short-command bound.
            module.command([sys.executable, '-I', '-c', 'import time;time.sleep(0.15)'],
                           root=package, timeout=kwargs.get('timeout', 0.04))
        return original_manager(arguments, **kwargs)
    operations.manager = delayed_manager
    assert operations.stop()['ready'] is False
    assert operations.start(restart=True)['ready'] is True
    assert commands == [('stop', 0.7), ('restart', 0.9)]
    assert operations.load() == receipt


def test_modified_unit_or_receipt_never_controls_or_replaces_unit(package, tmp_path):
    module = load_helper()
    operations = fake_operations(module, package, tmp_path / 'home')
    receipt = operations.prepare()
    unit = Path(receipt['unit_path'])
    unit.write_text('unrelated replacement')
    before = list(operations.calls)
    for method in [operations.start, operations.stop, operations.status]:
        with pytest.raises(ValueError, match='definition changed'):
            method()
    assert unit.read_text() == 'unrelated replacement'
    assert operations.calls == before


def test_stale_source_cannot_start_but_exact_owned_unit_can_be_stopped(package, tmp_path):
    module = load_helper()
    operations = fake_operations(module, package, tmp_path / 'home')
    operations.prepare()
    (package / 'app.py').write_text('changed source')
    with pytest.raises(ValueError, match='source changed'):
        operations.start()
    assert ['checks'] not in operations.calls
    assert operations.status()['source_current'] is False
    assert operations.stop()['ready'] is False


def test_failed_check_then_private_configuration_repair_can_retry_without_duplicate_unit(package, tmp_path, monkeypatch):
    module = load_helper()
    monkeypatch.setattr(module, 'console_healthy', lambda *args: True)
    operations = fake_operations(module, package, tmp_path / 'home')
    operations.prepare()
    original = operations.path.read_bytes()
    operations.check_failed = True
    with pytest.raises(ValueError, match='check failed'):
        operations.start()
    assert not any(call[0] in {'start', 'restart'} for call in operations.calls if call)
    (package / '.env').write_text('MOBKIT_PORT="4231"\nOPENAI_API_KEY="repaired-private-value"\n')
    operations.check_failed = False
    assert operations.start()['ready']
    assert operations.path.read_bytes() == original


def test_stale_health_pid_timestamp_and_native_failure_never_show_ready(package, tmp_path, monkeypatch):
    module = load_helper()
    operations = fake_operations(module, package, tmp_path / 'home')
    receipt = operations.prepare()
    operations.current = 'active'
    healthy = {'supervisor_pid': 317, 'source_digest': receipt['source_digest'], 'ready': True,
               'observed_at': time.time(), 'console_url': 'http://127.0.0.1:4231/console', 'bootstrap': {}}
    monkeypatch.setattr(module, 'console_healthy', lambda *args: True)
    for changes in [{'supervisor_pid': 316}, {'observed_at': time.time() - 16},
                    {'source_digest': 'stale'}, {'ready': False}]:
        module.write_private(operations.health_path, {**healthy, **changes})
        assert operations.status()['ready'] is False
    module.write_private(operations.health_path, healthy)
    monkeypatch.setattr(module, 'console_healthy', lambda *args: False)
    assert operations.status()['ready'] is False


def test_no_credentials_in_unit_and_execstart_arguments_stay_quoted(package, tmp_path):
    module = load_helper()
    receipt = module.Operations(package, tmp_path / 'home').prepare()
    text = Path(receipt['unit_path']).read_text()
    assert 'private-canary' not in text and 'OPENAI_API_KEY' not in text
    assert '"' + str(package / '.venv/bin/python') + '" "-I"' in text
    assert 'KillMode=mixed' in text and 'Restart=on-failure' in text
    tricky = module.unit_content({**receipt, 'root': '/owned/a "$x%q\\b'}).decode()
    assert '$$x%%q' in tricky and '\\"' in tricky and '\\\\b' in tricky


@pytest.mark.parametrize('root', ['/owned/new\nExecStart=/bad', '/owned/tab\tdir', '/owned/control\x00dir',
                                  '/owned/terminal ', '/owned/terminal\\', 'relative/path', '/owned/../escaped'])
def test_unit_rejects_unsafe_path_directives_before_creating_any_unit(root):
    module = load_helper()
    with pytest.raises(ValueError, match='absolute path'):
        module.unit_content({'root': root})


@pytest.mark.skipif(sys.platform != 'linux' or not shutil.which('systemd-analyze'),
                    reason='Actual systemd parser is available only on Linux; destination evidence is separate.')
def test_actual_systemd_parser_accepts_directory_with_spaces_and_rejects_legacy_quotes(tmp_path):
    module = load_helper()
    root = tmp_path / 'source with spaces % and $value'
    root.mkdir()
    (root / '.venv/bin').mkdir(parents=True)
    (root / '.venv/bin/python').symlink_to(sys.executable)
    receipt = {'root': str(root)}
    path = tmp_path / 'syntax-proof.service'
    path.write_bytes(module.unit_content(receipt, legacy_directory=True))
    old = subprocess.run(['systemd-analyze', '--user', '--man=no', 'verify', str(path)],
                         capture_output=True, text=True, timeout=15)
    assert old.returncode != 0 and 'path is not absolute' in old.stderr
    path.write_bytes(module.unit_content(receipt))
    new = subprocess.run(['systemd-analyze', '--user', '--man=no', 'verify', str(path)],
                         capture_output=True, text=True, timeout=15)
    assert new.returncode == 0, new.stderr


def legacy_invalid_service(module, package, home):
    operations = fake_operations(module, package, home)
    receipt = operations.prepare()
    receipt.pop('stop_timeout_seconds')
    unit = Path(receipt['unit_path'])
    content = module.unit_content(receipt, legacy_directory=True)
    unit.write_bytes(content)
    receipt['unit_sha256'] = hashlib.sha256(content).hexdigest()
    module.write_private(operations.path, receipt)
    operations.load_state = 'bad-setting'
    manager = operations.manager
    def invalid_manager(arguments, **kwargs):
        if arguments[0] == 'stop':
            operations.calls.append(arguments)
            assert kwargs.get('allow_failure') is True
            return 5, 'Unit has a bad unit file setting.'
        return manager(arguments, **kwargs)
    operations.manager = invalid_manager
    return operations, receipt


def test_exact_legacy_invalid_unit_can_be_removed_without_native_or_private_state_loss(package, tmp_path):
    module = load_helper()
    operations, receipt = legacy_invalid_service(module, package, tmp_path / 'home')
    unit = Path(receipt['unit_path'])
    native = package / '.mobkit/native-occurrences.json'
    native.write_text('actual retained native occurrence')
    private = (package / '.env').read_bytes()
    with pytest.raises(ValueError, match='definition changed'):
        operations.start()
    assert operations.status()['ready'] is False
    assert operations.status()['load_state'] == 'bad-setting'
    assert operations.stop(remove=True)['status'] == 'service-removed'
    assert not unit.exists() and not operations.path.exists()
    assert native.read_text() == 'actual retained native occurrence'
    assert (package / '.env').read_bytes() == private
    assert ['checks'] not in operations.calls
    assert not any(call[0] in {'start', 'restart'} for call in operations.calls)


@pytest.mark.parametrize('change', [{'load_state': 'loaded'}, {'current': 'active'},
                                  {'control_pid': '318'}, {'control_group': '/live-child'}])
def test_failed_stop_never_removes_live_or_unproven_invalid_unit(package, tmp_path, change):
    module = load_helper()
    operations, receipt = legacy_invalid_service(module, package, tmp_path / 'home')
    for key, value in change.items():
        setattr(operations, key, value)
    unit = Path(receipt['unit_path'])
    before = (unit.read_bytes(), operations.path.read_bytes())
    with pytest.raises(ValueError, match='stop failed'):
        operations.stop(remove=True)
    assert (unit.read_bytes(), operations.path.read_bytes()) == before
    assert not any(call[0] == 'disable' for call in operations.calls)


def test_invalid_unit_cleanup_rechecks_ownership_after_disable(package, tmp_path):
    module = load_helper()
    operations, receipt = legacy_invalid_service(module, package, tmp_path / 'home')
    unit = Path(receipt['unit_path'])
    manager = operations.manager
    def replace_after_disable(arguments, **kwargs):
        value = manager(arguments, **kwargs)
        if arguments[0] == 'disable':
            unit.write_text('a different service definition')
        return value
    operations.manager = replace_after_disable
    with pytest.raises(ValueError, match='definition changed'):
        operations.stop(remove=True)
    assert unit.read_text() == 'a different service definition' and operations.path.exists()


def test_invalid_unit_cleanup_preserves_new_control_process(package, tmp_path):
    module = load_helper()
    operations, receipt = legacy_invalid_service(module, package, tmp_path / 'home')
    unit = Path(receipt['unit_path'])
    manager = operations.manager
    def change_after_disable(arguments, **kwargs):
        value = manager(arguments, **kwargs)
        if arguments[0] == 'disable':
            operations.control_pid = '319'
        return value
    operations.manager = change_after_disable
    with pytest.raises(ValueError, match='no longer stopped'):
        operations.stop(remove=True)
    assert unit.exists() and operations.path.exists()


def test_invalid_unit_cleanup_rechecks_definition_after_final_manager_observation(package, tmp_path):
    module = load_helper()
    operations, receipt = legacy_invalid_service(module, package, tmp_path / 'home')
    unit = Path(receipt['unit_path'])
    manager, observations = operations.manager, 0
    def replace_during_final_observation(arguments, **kwargs):
        nonlocal observations
        value = manager(arguments, **kwargs)
        if arguments[0] == 'show':
            observations += 1
            if observations == 3:
                unit.write_text('a replacement after the final manager observation')
        return value
    operations.manager = replace_during_final_observation
    with pytest.raises(ValueError, match='definition changed'):
        operations.stop(remove=True)
    assert unit.read_text() == 'a replacement after the final manager observation'
    assert operations.path.exists()


def test_invalid_unit_cleanup_does_not_swallow_disable_failure(package, tmp_path):
    module = load_helper()
    operations, receipt = legacy_invalid_service(module, package, tmp_path / 'home')
    unit = Path(receipt['unit_path'])
    manager = operations.manager
    def failed_disable(arguments, **kwargs):
        if arguments[0] == 'disable':
            raise module.DestinationError('The service manager denied disabling this unit.')
        return manager(arguments, **kwargs)
    operations.manager = failed_disable
    with pytest.raises(ValueError, match='denied disabling'):
        operations.stop(remove=True)
    assert unit.exists() and operations.path.exists()


def test_private_receipt_link_or_public_permissions_are_refused(package, tmp_path):
    module = load_helper()
    operations = module.Operations(package, tmp_path / 'home')
    operations.prepare()
    operations.path.chmod(0o644)
    with pytest.raises(ValueError, match='permissions'):
        operations.load()
    operations.path.chmod(0o600)
    operations.path.rename(operations.path.with_suffix('.old'))
    operations.path.symlink_to(operations.path.with_suffix('.old'))
    with pytest.raises(ValueError, match='symbolic'):
        operations.load()


def test_legacy_service_start_explicitly_requires_current_readiness_package(package, tmp_path):
    module = load_helper()
    value = json.loads((package / 'deployment.json').read_text())
    value['schemaVersion'] = 1
    (package / 'deployment.json').write_text(json.dumps(value))
    with pytest.raises(ValueError, match='schemaVersion 2.*native startup'):
        module.Operations(package, tmp_path / 'home')


@pytest.mark.parametrize('change', [{'pid': 1}, {'console_url': 'http://127.0.0.1:4999/console'},
                                  {'bootstrap': {'complete': True, 'ready': True, 'counts': {'broken': 1}}}])
def test_owned_ready_pipe_rejects_wrong_pid_port_and_broken_native_bootstrap(package, change):
    module = load_helper()
    spec = module.definition(package)
    payload = {'pid': 317, 'console_url': 'http://127.0.0.1:4231/console', 'bootstrap': {
        'complete': True, 'ready': True, 'counts': {'broken': 0},
        'identities': {item['identity']: {} for item in spec['agents']}}}
    assert module.startup(payload, spec, 317, 4231)['ready']
    with pytest.raises(ValueError, match='bootstrap'):
        module.startup({**payload, **change}, spec, 317, 4231)


def test_console_health_uses_private_auth_without_proxy_or_token_url(package, monkeypatch):
    module = load_helper()
    observed = []
    spec = module.definition(package)
    class Handler(BaseHTTPRequestHandler):
        def do_GET(self):
            observed.append((self.path, self.headers.get('Authorization')))
            if self.headers.get('Authorization') != 'Bearer private-health-token':
                self.send_error(401)
                return
            self.send_response(200)
            self.end_headers()
            self.wfile.write(json.dumps({'identities': [{'identity': item['identity']} for item in spec['agents']]}).encode())
        def log_message(self, *args):
            pass
    server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    port = server.server_port
    (package / '.env').write_text('MOBKIT_PORT=' + json.dumps(str(port)) + '\n')
    (package / 'application').mkdir()
    (package / 'application/console.json').write_text('{"authentication":"local-password"}')
    called = []
    monkeypatch.setitem(sys.modules, 'console_auth', SimpleNamespace(
        health_headers=lambda root: called.append(root) or {'Authorization': 'Bearer private-health-token'}))
    monkeypatch.setenv('HTTP_PROXY', 'http://unreachable.invalid')
    try:
        assert module.console_healthy(package, spec, f'http://127.0.0.1:{port}/console')
        assert observed == [('/console/identities', 'Bearer private-health-token')]
        assert called == [package]
        with pytest.raises(ValueError, match='URL'):
            module.console_healthy(package, spec, f'http://127.0.0.1:{port}/console?token=secret')
    finally:
        server.shutdown()
        server.server_close()
        thread.join(timeout=2)


def test_clean_environment_never_passes_ambient_provider_or_python_override(monkeypatch):
    module = load_helper()
    for key in ['OPENAI_API_KEY', 'ANTHROPIC_API_KEY', 'PYTHONPATH', 'HTTP_PROXY', 'MOBKIT_RPC_GATEWAY_BIN']:
        monkeypatch.setenv(key, 'ambient-private-value')
    env = module.environment()
    assert 'ambient-private-value' not in env.values()
    assert env['XDG_RUNTIME_DIR'] == '/run/user/' + str(os.getuid())
    assert 'PYTHONPATH' not in env


def test_receive_is_exact_owned_repeatable_and_preserves_all_private_state(package, tmp_path):
    module = load_helper()
    archive = tmp_path / 'source.tar.gz'
    module.archive_package(archive, package)
    destination = tmp_path / 'remote home/src/my-team'
    digest = module.source_digest(package)
    binding = module.receive_archive(archive, destination, 'a' * 32, digest)
    assert module.source_digest(destination) == digest
    assert binding['root'] == str(destination)
    (destination / '.env').write_text('private credentials survive')
    (destination / 'data').mkdir()
    (destination / 'data/history').write_text('native data survives')
    before = (destination / 'app.py').stat().st_mtime_ns
    # Simulate a partial previous transfer by dropping just one owned source.
    (destination / 'start.sh').unlink()
    assert module.receive_archive(archive, destination, 'a' * 32, digest) == binding
    assert (destination / 'app.py').stat().st_mtime_ns == before
    assert (destination / 'start.sh').is_file()
    assert (destination / '.env').read_text() == 'private credentials survive'
    assert (destination / 'data/history').read_text() == 'native data survives'


def test_receive_refuses_unowned_folder_changed_source_and_links_before_writing(package, tmp_path):
    module = load_helper()
    archive = tmp_path / 'source.tar.gz'
    module.archive_package(archive, package)
    digest = module.source_digest(package)
    unowned = tmp_path / 'unowned'
    unowned.mkdir()
    (unowned / 'app.py').write_text('belongs to another project')
    with pytest.raises(ValueError, match='not empty'):
        module.receive_archive(archive, unowned, 'a' * 32, digest)
    assert list(unowned.iterdir()) == [unowned / 'app.py']
    owned = tmp_path / 'owned'
    module.receive_archive(archive, owned, 'a' * 32, digest)
    with pytest.raises(ValueError, match='another installation'):
        module.receive_archive(archive, owned, 'b' * 32, digest)
    (owned / 'app.py').write_text('changed locally')
    with pytest.raises(ValueError, match='Installed source changed'):
        module.receive_archive(archive, owned, 'a' * 32, digest)
    assert (owned / 'app.py').read_text() == 'changed locally'
    outside = tmp_path / 'outside'
    outside.mkdir()
    linked = tmp_path / 'linked'
    linked.symlink_to(outside, target_is_directory=True)
    with pytest.raises(ValueError, match='symbolic'):
        module.receive_archive(archive, linked, 'a' * 32, digest)
    assert not list(outside.iterdir())


def test_receive_rejects_wrong_full_source_digest_without_creating_destination(package, tmp_path):
    module = load_helper()
    archive = tmp_path / 'source.tar.gz'
    module.archive_package(archive, package)
    destination = tmp_path / 'not-created'
    with pytest.raises(ValueError, match='differs'):
        module.receive_archive(archive, destination, 'a' * 32, 'b' * 64)
    assert not destination.exists()


@pytest.mark.parametrize('overflow', [False, True])
def test_bounded_command_stops_and_reaps_real_owned_process(package, tmp_path, overflow):
    module = load_helper()
    pidfile = tmp_path / 'owned.pid'
    code = ('import os,time,pathlib\npathlib.Path(' + repr(str(pidfile)) + ').write_text(str(os.getpid()))\n'
            + ('while True: os.write(1,b"x"*8192)\n' if overflow else 'time.sleep(30)\n'))
    before = time.monotonic()
    expected = module.DestinationError if overflow else TimeoutError
    with pytest.raises(expected):
        module.command([sys.executable, '-I', '-c', code], root=package, timeout=0.5)
    assert time.monotonic() - before < 5
    pid = int(pidfile.read_text())
    with pytest.raises(ProcessLookupError):
        os.kill(pid, 0)


def test_command_timeout_kills_owned_descendant_after_leader_exit_without_touching_other_group(package, tmp_path, monkeypatch):
    module = load_helper(package / 'remote_operations.py')
    pidfile = tmp_path / 'owned-descendant.json'
    leader = ('import json,os,pathlib,subprocess,sys\n'
        'child=subprocess.Popen([sys.executable,"-I","-c","import time;time.sleep(30)"])\n'
        'pathlib.Path(' + repr(str(pidfile)) + ').write_text(json.dumps({"leader":os.getpid(),"child":child.pid,"group":os.getpgrp()}))\n'
        'os._exit(0)\n')
    unrelated = subprocess.Popen([sys.executable, '-I', '-c', 'import time;time.sleep(30)'],
                                 start_new_session=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    original_signal, signals = module.signal_group, []
    def recorded_signal(process, selected):
        # A None returncode at signaling proves command did not poll/reap first.
        signals.append((process.pid, selected, process.returncode))
        original_signal(process, selected)
    monkeypatch.setattr(module, 'signal_group', recorded_signal)
    owned = None
    try:
        with pytest.raises(TimeoutError, match='time limit'):
            module.command([sys.executable, '-I', '-c', leader], root=package, timeout=0.7)
        owned = json.loads(pidfile.read_text())
        assert owned['group'] == owned['leader'] and owned['group'] != os.getpgid(unrelated.pid)
        assert signals == [(owned['group'], signal.SIGKILL, None)]
        assert unrelated.poll() is None
        # The leader is reaped by command; the orphan descendant is reaped by
        # the OS. Observe both exact PIDs, not killpg(0), which can report EPERM
        # for an already-empty group on Mac.
        deadline = time.monotonic() + 3
        while time.monotonic() < deadline:
            present = []
            for pid in (owned['leader'], owned['child']):
                try:
                    os.kill(pid, 0)
                except ProcessLookupError:
                    pass
                else:
                    present.append(pid)
            if not present:
                break
            time.sleep(0.02)
        with pytest.raises(ProcessLookupError):
            os.kill(owned['leader'], 0)
        with pytest.raises(ProcessLookupError):
            os.kill(owned['child'], 0)
        assert unrelated.poll() is None
    finally:
        if owned is None and pidfile.exists():
            owned = json.loads(pidfile.read_text())
        try:
            if owned is not None:
                for pid in (owned['leader'], owned['child']):
                    try:
                        group = os.getpgid(pid)
                    except ProcessLookupError:
                        continue
                    if group == owned['group']:
                        try:
                            os.killpg(group, signal.SIGKILL)
                        except ProcessLookupError:
                            pass
        finally:
            unrelated.terminate()
            try:
                unrelated.wait(timeout=2)
            except subprocess.TimeoutExpired:
                unrelated.kill()
                unrelated.wait(timeout=2)


FIXTURE_APP = '''\
import json, os, pathlib, signal, sys, threading
from http.server import BaseHTTPRequestHandler, HTTPServer
root = pathlib.Path.cwd()
spec = json.loads((root / 'deployment.json').read_text())
env = dict(row.split('=', 1) for row in (root / '.env').read_text().splitlines() if '=' in row)
port = int(json.loads(env['MOBKIT_PORT']))
class Handler(BaseHTTPRequestHandler):
    def do_GET(self):
        self.send_response(200); self.end_headers()
        self.wfile.write(json.dumps({'identities': [{'identity': item['identity']} for item in spec['agents']]}).encode())
    def log_message(self, *args): pass
server = HTTPServer(('127.0.0.1', port), Handler)
signal.signal(signal.SIGTERM, lambda *args: sys.exit(0))
payload = {'pid': os.getpid(), 'console_url': 'http://127.0.0.1:' + str(port) + '/console',
           'bootstrap': {'complete': True, 'ready': True, 'counts': {'broken': 0},
                         'identities': {item['identity']: {} for item in spec['agents']}}}
print('redact private-canary', flush=True)
with os.fdopen(int(os.environ['MOBKIT_LAUNCH_READY_FD']), 'w') as stream:
    stream.write(json.dumps(payload) + '\\n')
    stream.flush()
server.serve_forever()
'''


def fixture_supervisor(package, tmp_path, module, app):
    (package / 'app.py').write_text(app)
    (package / '.venv/bin').mkdir(parents=True)
    (package / '.venv/bin/python').symlink_to(sys.executable)
    with socket.socket() as selected:
        selected.bind(('127.0.0.1', 0))
        port = selected.getsockname()[1]
    (package / '.env').write_text('MOBKIT_PORT=' + json.dumps(str(port)) + '\nOPENAI_API_KEY="private-canary"\n')
    manifest(package)
    operations = module.Operations(package, tmp_path / 'home')
    operations.prepare()
    return operations


@pytest.mark.asyncio
async def test_actual_isolated_supervisor_child_pipe_health_redaction_and_stop(package, tmp_path, capsys):
    module = load_helper()
    operations = fixture_supervisor(package, tmp_path, module, FIXTURE_APP)
    task = asyncio.create_task(module.supervise(operations))
    pid = None
    try:
        deadline = time.monotonic() + 8
        while time.monotonic() < deadline:
            await asyncio.sleep(0.1)
            if operations.health_path.exists():
                health = json.loads(operations.health_path.read_text())
                if health['ready']:
                    pid = health['runtime_pid']
                    assert health['bootstrap']['counts']['broken'] == 0
                    assert health['source_digest'] == operations.load()['source_digest']
                    break
            if task.done():
                await task
        assert pid is not None, 'Fixture child must send a real inherited startup pipe and serve actual loopback health'
        os.kill(pid, 0)
    finally:
        task.cancel()
        await asyncio.gather(task, return_exceptions=True)
    assert json.loads(operations.health_path.read_text())['ready'] is False
    with pytest.raises(ProcessLookupError):
        os.kill(pid, 0)
    logs = capsys.readouterr().out
    assert 'private-canary' not in logs and '[private]' in logs


@pytest.mark.asyncio
@pytest.mark.parametrize('ignore_term', [False, True])
async def test_exported_supervisor_allows_slow_grace_then_bounded_owned_force_reap(package, tmp_path, monkeypatch, ignore_term):
    module = load_helper(package / 'remote_operations.py')
    marker = package / 'data/graceful-stop.txt'
    marker.parent.mkdir()
    if ignore_term:
        handler = 'signal.signal(signal.SIGTERM, lambda *args: None)'
    else:
        handler = ('def cleanup(*args):\n'
                   '    import time\n'
                   '    time.sleep(0.15)\n'
                   '    (root / "data/graceful-stop.txt").write_text("adapter cleanup finished")\n'
                   '    sys.exit(0)\n'
                   'signal.signal(signal.SIGTERM, cleanup)')
    app = FIXTURE_APP.replace('signal.signal(signal.SIGTERM, lambda *args: sys.exit(0))', handler)
    operations = fixture_supervisor(package, tmp_path, module, app)
    monkeypatch.setattr(module, 'STOP_TIMEOUT', 0.5 if not ignore_term else 0.15)
    monkeypatch.setattr(module, 'FORCE_REAP_TIMEOUT', 0.5)
    original_signal, signals = module.signal_group, []
    def recorded_signal(process, selected):
        signals.append((process.pid, selected))
        original_signal(process, selected)
    monkeypatch.setattr(module, 'signal_group', recorded_signal)
    task = asyncio.create_task(module.supervise(operations))
    pid = None
    try:
        deadline = time.monotonic() + 8
        while time.monotonic() < deadline:
            await asyncio.sleep(0.05)
            if operations.health_path.exists():
                health = json.loads(operations.health_path.read_text())
                if health['ready']:
                    pid = health['runtime_pid']
                    break
            if task.done():
                await task
        assert pid is not None
    finally:
        before = time.monotonic()
        task.cancel()
        await asyncio.gather(task, return_exceptions=True)
    assert 0.1 < time.monotonic() - before < 3
    assert signals == ([(pid, signal.SIGTERM), (pid, signal.SIGKILL)] if ignore_term else [])
    assert marker.exists() is not ignore_term
    health = json.loads(operations.health_path.read_text())
    assert health['runtime_pid'] is None and health['cleanup_confirmed'] is True
    assert health['owned_process_group'] is None
    with pytest.raises(ProcessLookupError):
        os.kill(pid, 0)


@pytest.mark.asyncio
async def test_exported_supervisor_failed_force_reap_is_bounded_and_preserves_owned_pid(package, tmp_path, monkeypatch):
    module = load_helper(package / 'remote_operations.py')
    operations = module.Operations(package, tmp_path / 'home')
    operations.prepare()
    output = asyncio.StreamReader()
    output.feed_eof()
    child = SimpleNamespace(pid=7777777, returncode=None, stdout=output)
    async def never_reaped():
        await asyncio.Event().wait()
    child.wait = never_reaped
    parent_signals = []
    child.terminate = lambda: parent_signals.append(child.pid)
    async def admitted_fixture_child(*args, **kwargs):
        fd = kwargs['pass_fds'][0]
        payload = {'pid': child.pid, 'console_url': 'http://127.0.0.1:4231/console',
                   'bootstrap': {'complete': True, 'ready': True, 'counts': {'broken': 0},
                       'identities': {item['identity']: {} for item in operations.spec['agents']}}}
        os.write(fd, json.dumps(payload).encode() + b'\n')
        return child
    monkeypatch.setattr(module.asyncio, 'create_subprocess_exec', admitted_fixture_child)
    monkeypatch.setattr(module, 'console_healthy', lambda *args: True)
    monkeypatch.setattr(module, 'STOP_TIMEOUT', 0.02)
    monkeypatch.setattr(module, 'FORCE_REAP_TIMEOUT', 0.03)
    signals = []
    monkeypatch.setattr(module, 'signal_group', lambda process, selected: signals.append((process.pid, selected)))
    monkeypatch.setattr(module.os, 'killpg', lambda pid, selected: None)
    task = asyncio.create_task(module.supervise(operations))
    deadline = time.monotonic() + 30
    while time.monotonic() < deadline:
        await asyncio.sleep(0.01)
        if operations.health_path.exists() and json.loads(operations.health_path.read_text())['ready']:
            break
    assert json.loads(operations.health_path.read_text())['ready'] is True
    before = time.monotonic()
    task.cancel()
    with pytest.raises(BaseExceptionGroup) as failure:
        await task
    assert [type(error) for error in failure.value.exceptions] == [asyncio.CancelledError, TimeoutError]
    assert time.monotonic() - before < 0.5
    assert parent_signals == [child.pid]
    assert signals == [(child.pid, signal.SIGTERM), (child.pid, signal.SIGKILL)]
    health = json.loads(operations.health_path.read_text())
    assert health['ready'] is False and health['runtime_pid'] == child.pid
    assert health['cleanup_confirmed'] is False and health['owned_process_group'] == child.pid


def test_command_failed_force_reap_has_its_own_bounded_wait(package, monkeypatch):
    module = load_helper()
    waits, signals = [], []
    process = SimpleNamespace(pid=7777777, returncode=None, stdout=None, poll=lambda: None)
    def unproven_wait(timeout):
        waits.append(timeout)
        raise subprocess.TimeoutExpired('owned fixture', timeout)
    process.wait = unproven_wait
    monkeypatch.setattr(module.subprocess, 'Popen', lambda *args, **kwargs: process)
    monkeypatch.setattr(module, 'signal_group', lambda child, selected: signals.append((child.pid, selected)))
    with pytest.raises(subprocess.TimeoutExpired):
        module.command(['owned-fixture'], root=package, timeout=0.01, terminal=True)
    assert waits == [0.01, deployment_deadlines.PROCESS_FORCE_REAP_TIMEOUT]
    assert signals == [(process.pid, signal.SIGKILL)]


def test_interrupted_wait_that_already_reaped_leader_never_signals_reusable_group(package, monkeypatch):
    module = load_helper(package / 'remote_operations.py')
    waits, signals = [], []
    process = SimpleNamespace(pid=7777777, returncode=None, stdout=None)
    interruption = KeyboardInterrupt('owned fixture wait interrupted after reap')
    def interrupted_wait(timeout):
        waits.append(timeout)
        if len(waits) == 1:
            # Model Popen.wait's KeyboardInterrupt path: its internal _wait
            # can reap the leader and set returncode before rethrowing.
            process.returncode = 0
            raise interruption
        return process.returncode
    def forbidden_poll():
        raise AssertionError('Cleanup must never poll before deciding group ownership')
    process.wait, process.poll = interrupted_wait, forbidden_poll
    monkeypatch.setattr(module.subprocess, 'Popen', lambda *args, **kwargs: process)
    monkeypatch.setattr(module, 'signal_group', lambda child, selected: signals.append((child.pid, selected)))
    with pytest.raises(KeyboardInterrupt) as caught:
        module.command(['owned-fixture'], root=package, timeout=0.25, terminal=True)
    assert caught.value is interruption
    assert process.returncode == 0
    assert waits == [0.25, deployment_deadlines.PROCESS_FORCE_REAP_TIMEOUT]
    assert signals == []


@pytest.mark.asyncio
async def test_actual_child_exit_zero_without_startup_evidence_is_failed_and_reaped(package, tmp_path, monkeypatch):
    module = load_helper()
    operations = fixture_supervisor(package, tmp_path, module, 'raise SystemExit(0)\n')
    children = []
    create = asyncio.create_subprocess_exec
    async def tracked_child(*args, **kwargs):
        child = await create(*args, **kwargs)
        children.append(child)
        return child
    monkeypatch.setattr(module.asyncio, 'create_subprocess_exec', tracked_child)
    with pytest.raises(ValueError, match='startup did not report'):
        await module.supervise(operations)
    result = json.loads(operations.health_path.read_text())
    assert result['ready'] is False and result['runtime_pid'] is None
    assert len(children) == 1 and children[0].returncode == 0
    with pytest.raises(ProcessLookupError):
        os.kill(children[0].pid, 0)


@pytest.mark.asyncio
async def test_pipe_eof_before_child_watcher_permission_race_preserves_startup_failure(package, tmp_path, monkeypatch):
    module = load_helper()
    operations = fixture_supervisor(package, tmp_path, module, 'raise SystemExit(0)\n')
    watcher = asyncio.Event()
    output = asyncio.StreamReader()
    output.feed_eof()
    child = SimpleNamespace(pid=7777777, returncode=None, stdout=output)
    waits = []
    async def wait():
        await watcher.wait()
        child.returncode = 0
        waits.append(0)
        return 0
    child.wait = wait
    parent_signals = []
    def terminate():
        parent_signals.append(child.pid)
        watcher.set()
        raise PermissionError('The already-exited child watcher has not run yet.')
    child.terminate = terminate
    async def create(*args, **kwargs):
        assert kwargs['start_new_session'] is True
        return child
    signals = []
    def killpg(pid, selected):
        assert pid == child.pid
        signals.append(selected)
        assert selected == 0
        assert child.returncode == 0
        raise ProcessLookupError
    monkeypatch.setattr(module.asyncio, 'create_subprocess_exec', create)
    monkeypatch.setattr(module.os, 'killpg', killpg)
    with pytest.raises(module.DestinationError, match='startup did not report'):
        await module.supervise(operations)
    assert parent_signals == [child.pid]
    assert signals == [0, 0] and len(waits) == 2
    result = json.loads(operations.health_path.read_text())
    assert result['ready'] is False and result['runtime_pid'] is None
    assert result['cleanup_confirmed'] is True and result['owned_process_group'] is None


@pytest.mark.asyncio
async def test_live_permission_failure_still_cleans_supervisor_tasks_health_and_handlers(package, tmp_path, monkeypatch):
    module = load_helper()
    operations = fixture_supervisor(package, tmp_path, module, 'raise SystemExit(0)\n')
    output = asyncio.StreamReader()
    output.feed_eof()
    child = SimpleNamespace(pid=7777777, returncode=None, stdout=output)
    wait_cancelled = asyncio.Event()
    async def wait():
        try:
            await asyncio.Event().wait()
        finally:
            wait_cancelled.set()
    child.wait = wait
    parent_signals = []
    def terminate():
        parent_signals.append(child.pid)
        raise PermissionError('Owned live application permission denied.')
    child.terminate = terminate
    async def create(*args, **kwargs):
        return child
    signals = []
    def killpg(pid, selected):
        signals.append((pid, selected))
        raise PermissionError('Owned live group permission denied.')
    loop = asyncio.get_running_loop()
    added, removed = [], []
    monkeypatch.setattr(loop, 'add_signal_handler', lambda selected, *args: added.append(selected))
    monkeypatch.setattr(loop, 'remove_signal_handler', lambda selected: removed.append(selected))
    monkeypatch.setattr(module.asyncio, 'create_subprocess_exec', create)
    monkeypatch.setattr(module.os, 'killpg', killpg)
    monkeypatch.setattr(module, 'EXIT_OBSERVATION_TIMEOUT', 0.03)
    with pytest.raises(PermissionError, match='live application') as failure:
        await module.supervise(operations)
    assert isinstance(failure.value.__context__, module.DestinationError)
    assert parent_signals == [child.pid] and signals == []
    assert wait_cancelled.is_set() and removed == added == [module.signal.SIGINT, module.signal.SIGTERM]
    result = json.loads(operations.health_path.read_text())
    assert result['ready'] is False and result['runtime_pid'] == child.pid
    assert result['cleanup_confirmed'] is False and result['owned_process_group'] == child.pid


@pytest.mark.asyncio
@pytest.mark.parametrize('inaccessible', [False, True])
async def test_reaped_child_does_not_hide_existing_or_inaccessible_group(monkeypatch, inaccessible):
    module = load_helper()
    child = SimpleNamespace(pid=7777777, returncode=0)
    signals = []
    def killpg(pid, selected):
        assert pid == child.pid
        signals.append(selected)
        if selected or inaccessible:
            raise PermissionError('The group must not be treated as gone.')
    monkeypatch.setattr(module.os, 'killpg', killpg)
    with pytest.raises(PermissionError, match='must not'):
        await module.signal_supervised_group(child, module.signal.SIGTERM)
    assert signals == [module.signal.SIGTERM, 0]


@pytest.mark.asyncio
async def test_supervisor_preserves_child_cleanup_protocol_during_graceful_parent_stop(package, tmp_path, monkeypatch):
    module = load_helper(package / 'remote_operations.py')
    (package / 'data').mkdir()
    child_code = '''\
import pathlib, signal, sys, time
root = pathlib.Path.cwd() / 'data'
def interrupted(*args):
    (root / 'child-signal.txt').write_text('group interrupted callback cleanup')
signal.signal(signal.SIGTERM, interrupted)
print('ready', flush=True)
assert sys.stdin.readline().strip() == 'shutdown'
time.sleep(0.15)
print('cleanup-ack', flush=True)
'''
    handler = ('import subprocess\n'
        f'child = subprocess.Popen([sys.executable, "-I", "-c", {child_code!r}], '
        'stdin=subprocess.PIPE, stdout=subprocess.PIPE)\n'
        'assert child.stdout.readline().strip() == b"ready"\n'
        'def cleanup(*args):\n'
        '    child.stdin.write(b"shutdown\\n"); child.stdin.flush()\n'
        '    assert child.stdout.readline().strip() == b"cleanup-ack"\n'
        '    assert child.wait(timeout=2) == 0\n'
        '    (root / "data/cleanup-ack.txt").write_text("child cleanup acknowledged")\n'
        '    sys.exit(0)\n'
        'signal.signal(signal.SIGTERM, cleanup)')
    app = FIXTURE_APP.replace('signal.signal(signal.SIGTERM, lambda *args: sys.exit(0))', handler)
    operations = fixture_supervisor(package, tmp_path, module, app)
    monkeypatch.setattr(module, 'STOP_TIMEOUT', 1)
    monkeypatch.setattr(module, 'FORCE_REAP_TIMEOUT', 0.5)
    signals, original = [], module.signal_group
    def group_signal(child, selected):
        signals.append(selected)
        original(child, selected)
    monkeypatch.setattr(module, 'signal_group', group_signal)
    task = asyncio.create_task(module.supervise(operations))
    pid = None
    try:
        deadline = time.monotonic() + 8
        while time.monotonic() < deadline:
            await asyncio.sleep(0.05)
            if operations.health_path.exists():
                health = json.loads(operations.health_path.read_text())
                if health['ready']:
                    pid = health['runtime_pid']
                    break
            if task.done():
                await task
        assert pid is not None
    finally:
        task.cancel()
        results = await asyncio.gather(task, return_exceptions=True)
    assert isinstance(results[0], asyncio.CancelledError)
    assert (package / 'data/cleanup-ack.txt').read_text() == 'child cleanup acknowledged'
    assert not (package / 'data/child-signal.txt').exists()
    assert signals == [], 'Graceful child cleanup must finish before any group signal.'
    health = json.loads(operations.health_path.read_text())
    assert health['cleanup_confirmed'] is True and health['owned_process_group'] is None
    with pytest.raises(ProcessLookupError):
        os.killpg(pid, 0)


@pytest.mark.asyncio
async def test_reaped_application_still_requires_owned_group_cleanup(monkeypatch):
    module = load_helper()
    child = SimpleNamespace(pid=7777777, returncode=0)
    async def wait():
        return child.returncode
    child.wait = wait
    child.terminate = lambda: pytest.fail('The application is already reaped.')
    present, signals = True, []
    def killpg(pid, selected):
        nonlocal present
        assert pid == child.pid
        if not present:
            raise ProcessLookupError
        if selected:
            signals.append(selected)
        if selected == signal.SIGKILL:
            present = False
    monkeypatch.setattr(module.os, 'killpg', killpg)
    monkeypatch.setattr(module, 'FORCE_REAP_TIMEOUT', 0.1)
    await module.stop_supervised_application(child)
    assert signals == [signal.SIGTERM, signal.SIGKILL] and present is False


@pytest.mark.asyncio
@pytest.mark.parametrize('inaccessible', [False, True])
async def test_supervisor_retains_unconfirmed_group_after_application_reaped(package, tmp_path, monkeypatch, inaccessible):
    module = load_helper()
    operations = module.Operations(package, tmp_path / 'home')
    operations.prepare()
    output = asyncio.StreamReader()
    output.feed_eof()
    child = SimpleNamespace(pid=7777777, returncode=0, stdout=output)
    async def wait():
        return child.returncode
    child.wait = wait
    async def create(*args, **kwargs):
        return child
    signals = []
    def killpg(pid, selected):
        assert pid == child.pid
        if inaccessible:
            raise PermissionError('Owned residual group cannot be inspected.')
        if selected:
            signals.append(selected)
    monkeypatch.setattr(module.asyncio, 'create_subprocess_exec', create)
    monkeypatch.setattr(module.os, 'killpg', killpg)
    monkeypatch.setattr(module, 'FORCE_REAP_TIMEOUT', 0.03)
    expected = PermissionError if inaccessible else TimeoutError
    with pytest.raises(expected):
        await module.supervise(operations)
    assert signals == ([] if inaccessible else [signal.SIGTERM, signal.SIGKILL])
    health = json.loads(operations.health_path.read_text())
    assert health['ready'] is False and health['runtime_pid'] is None
    assert health['cleanup_confirmed'] is False and health['owned_process_group'] == child.pid


@pytest.mark.asyncio
@pytest.mark.parametrize('cleanup_fails', [False, True])
async def test_cancelled_supervisor_admission_keeps_exact_child_and_cleanup_joined(
        package, tmp_path, monkeypatch, cleanup_fails):
    module = load_helper(package / 'remote_operations.py')
    operations = module.Operations(package, tmp_path / 'home')
    operations.prepare()
    admitting, admit, cleaning, clean = (asyncio.Event() for _ in range(4))
    child = SimpleNamespace(pid=7777777, returncode=None)
    admissions, closures, admission_cancelled, cleanup_cancelled = [], [], [], []
    failure = PermissionError('The owned fake child could not be cleaned up.')
    loop = asyncio.get_running_loop()
    added, removed = [], []
    monkeypatch.setattr(loop, 'add_signal_handler', lambda selected, *args: added.append(selected))
    monkeypatch.setattr(loop, 'remove_signal_handler', lambda selected: removed.append(selected))
    monkeypatch.setattr(module.os, 'killpg', lambda *args: pytest.fail('No real group may be signaled.'))

    async def create(*args, **kwargs):
        assert kwargs['start_new_session'] is True
        admissions.append(child)
        admitting.set()
        try:
            await admit.wait()
        except asyncio.CancelledError:
            admission_cancelled.append(child)
            raise
        return child

    async def close(selected, **options):
        assert selected is child
        closures.append(selected)
        cleaning.set()
        try:
            await clean.wait()
        except asyncio.CancelledError:
            cleanup_cancelled.append(selected)
            raise
        if cleanup_fails:
            raise failure
        child.returncode = 0

    monkeypatch.setattr(module.asyncio, 'create_subprocess_exec', create)
    monkeypatch.setattr(module, 'stop_supervised_application', close)
    task = asyncio.create_task(module.supervise(operations))
    try:
        await asyncio.wait_for(admitting.wait(), 30)
        task.cancel('original admission cancellation')
        await asyncio.sleep(0)
        task.cancel('repeated admission cancellation')
        await asyncio.sleep(0)
        assert not task.done(), 'Cancellation discarded the admitted child before handle custody.'
        assert admissions == [child] and not admission_cancelled and not closures
        admit.set()
        await asyncio.wait_for(cleaning.wait(), 30)
        task.cancel('first cleanup cancellation')
        await asyncio.sleep(0)
        task.cancel('repeated cleanup cancellation')
        await asyncio.sleep(0)
        assert not task.done(), 'The exact child cleanup must remain joined.'
        assert closures == [child] and not cleanup_cancelled
        clean.set()
        if cleanup_fails:
            with pytest.raises(BaseExceptionGroup) as caught:
                await task
            assert caught.value.exceptions[1] is failure
            assert isinstance(caught.value.exceptions[0], asyncio.CancelledError)
            assert caught.value.exceptions[0].args == ('original admission cancellation',)
        else:
            with pytest.raises(asyncio.CancelledError) as caught:
                await task
            assert caught.value.args == ('original admission cancellation',)
        assert not admission_cancelled and not cleanup_cancelled
        health = json.loads(operations.health_path.read_text())
        assert health['ready'] is False
        assert health['cleanup_confirmed'] is not cleanup_fails
        assert health['runtime_pid'] == (child.pid if cleanup_fails else None)
        assert health['owned_process_group'] == (child.pid if cleanup_fails else None)
        assert removed == added == [signal.SIGINT, signal.SIGTERM]
    finally:
        admit.set()
        clean.set()
        if not task.done():
            task.cancel()
        try:
            await task
        except BaseException:
            pass


@pytest.mark.asyncio
async def test_cancelled_supervisor_admission_failure_preserves_error_without_cleanup_claim(
        package, tmp_path, monkeypatch):
    module = load_helper(package / 'remote_operations.py')
    operations = module.Operations(package, tmp_path / 'home')
    operations.prepare()
    entered, release = asyncio.Event(), asyncio.Event()
    failure = OSError('The admitted fake spawn did not yield a process handle.')
    calls = []
    loop = asyncio.get_running_loop()
    monkeypatch.setattr(loop, 'add_signal_handler', lambda *args: None)
    monkeypatch.setattr(loop, 'remove_signal_handler', lambda *args: None)
    monkeypatch.setattr(module.os, 'killpg', lambda *args: pytest.fail('No real group may be signaled.'))

    async def create(*args, **kwargs):
        calls.append(args)
        entered.set()
        await release.wait()
        raise failure

    monkeypatch.setattr(module.asyncio, 'create_subprocess_exec', create)
    monkeypatch.setattr(module, 'stop_supervised_application',
        lambda *args, **options: pytest.fail('No process handle was supplied.'))
    task = asyncio.create_task(module.supervise(operations))
    try:
        await asyncio.wait_for(entered.wait(), 30)
        task.cancel('original admission cancellation')
        await asyncio.sleep(0)
        task.cancel('repeated admission cancellation')
        await asyncio.sleep(0)
        release.set()
        with pytest.raises(BaseExceptionGroup) as caught:
            await task
        assert isinstance(caught.value.exceptions[0], asyncio.CancelledError)
        assert caught.value.exceptions[0].args == ('original admission cancellation',)
        assert caught.value.exceptions[1] is failure and len(calls) == 1
        health = json.loads(operations.health_path.read_text())
        assert health['ready'] is False and health['runtime_pid'] is None
        assert health['cleanup_confirmed'] is False and health['owned_process_group'] is None
    finally:
        release.set()
        if not task.done():
            task.cancel()
        try:
            await task
        except BaseException:
            pass


@pytest.mark.asyncio
async def test_supervisor_cleanup_preserves_startup_failure_through_repeated_cancellation(
        package, tmp_path, monkeypatch):
    module = load_helper(package / 'remote_operations.py')
    operations = module.Operations(package, tmp_path / 'home')
    operations.prepare()
    output = asyncio.StreamReader()
    output.feed_eof()
    child = SimpleNamespace(pid=7777777, returncode=0, stdout=output)
    entered, release = asyncio.Event(), asyncio.Event()
    cleaning_cancelled, calls = [], []
    loop = asyncio.get_running_loop()
    monkeypatch.setattr(loop, 'add_signal_handler', lambda *args: None)
    monkeypatch.setattr(loop, 'remove_signal_handler', lambda *args: None)
    monkeypatch.setattr(module.os, 'killpg', lambda *args: pytest.fail('No real group may be signaled.'))

    async def create(*args, **kwargs):
        return child

    async def wait():
        return child.returncode

    async def close(selected, **options):
        assert selected is child
        calls.append(selected)
        entered.set()
        try:
            await release.wait()
        except asyncio.CancelledError:
            cleaning_cancelled.append(selected)
            raise

    child.wait = wait
    monkeypatch.setattr(module.asyncio, 'create_subprocess_exec', create)
    monkeypatch.setattr(module, 'stop_supervised_application', close)
    task = asyncio.create_task(module.supervise(operations))
    try:
        await asyncio.wait_for(entered.wait(), 30)
        task.cancel('first cleanup cancellation')
        await asyncio.sleep(0)
        task.cancel('repeated cleanup cancellation')
        await asyncio.sleep(0)
        assert not task.done(), 'The entered cleanup must settle before its original startup failure.'
        assert calls == [child] and not cleaning_cancelled
        release.set()
        with pytest.raises(module.DestinationError, match='startup did not report'):
            await task
        health = json.loads(operations.health_path.read_text())
        assert health['ready'] is False and health['runtime_pid'] is None
        assert health['cleanup_confirmed'] is True and health['owned_process_group'] is None
    finally:
        release.set()
        if not task.done():
            task.cancel()
        try:
            await task
        except BaseException:
            pass



# The destination supervisor mirrors the local worker: the application reports
# the horizon its gateway advertised and every enclosing remote budget follows.
def remote_budget(name, horizon_ms):
    application = horizon_ms // 1000 + TRANSPORT_AND_ADAPTER_MARGIN
    service = application + 10 + 5 + 15
    return {'application': application, 'service': service, 'command': service + 15}[name]


@pytest.mark.parametrize('reported', [None, 392_000, 432_000], ids=['unreported', 'meta-0.9.4', 'meta-0.9.5'])
async def test_supervised_stop_outlasts_the_gateway_horizon_the_application_reported(package, monkeypatch, reported):
    module = load_helper(package / 'remote_operations.py')
    process = SimpleNamespace(pid=7782, returncode=None, terminate=lambda: None)
    async def wait():
        return process.returncode
    process.wait = wait
    windows = []
    actual_wait = asyncio.wait_for
    async def model_wait(awaitable, timeout):
        if getattr(getattr(awaitable, 'cr_code', None), 'co_name', None) == 'supervised_parent_exit':
            windows.append(timeout)
            process.returncode = 0
        return await actual_wait(awaitable, timeout)
    monkeypatch.setattr(module.asyncio, 'wait_for', model_wait)
    monkeypatch.setattr(module, 'supervised_group_exists', lambda process: False)
    stop = module.stop_supervised_application
    await (stop(process) if reported is None else stop(process, horizon_ms=reported))
    assert windows == [remote_budget('application', gateway_horizon_ms() if reported is None else reported)]


@pytest.mark.parametrize('value, expected', [
    (None, None), (392_000, 392_000), (432_000, 432_000),
    ('392000', ValueError), (True, ValueError), (0, ValueError), (-1, ValueError), (1.5, ValueError),
    (2_147_483_648, ValueError)], ids=str)
def test_remote_readiness_horizon_is_validated_like_the_local_worker(value, expected):
    module = load_helper()
    parse = getattr(module, 'reported_gateway_horizon', None)
    assert parse is not None, 'the destination supervisor cannot read a reported horizon'
    payload = {} if value is None else {'gateway_shutdown_horizon_ms': value}
    if expected is ValueError:
        with pytest.raises(ValueError, match='gateway shutdown horizon'):
            parse(payload)
    else:
        assert parse(payload) == expected


def test_remote_unit_budgets_are_exactly_the_recognised_ones():
    module = load_helper()
    # The legacy 45-second unit, the unit the 347-second pin wrote, and the
    # unreported horizon's unit. A reported horizon widens a drop-in instead.
    for budget in (45, 397, remote_budget('service', gateway_horizon_ms())):
        assert module.service_stop_timeout({'stop_timeout_seconds': budget}) == budget
    for budget in (396, 398, remote_budget('service', gateway_horizon_ms() + 40_000), 3600):
        with pytest.raises(ValueError, match='unsupported shutdown budget'):
            module.service_stop_timeout({'stop_timeout_seconds': budget})


def remote_earlier_unit(module, operations):
    """Record the unit an earlier Forge wrote from the 347-second pin."""
    receipt = operations.prepare()
    earlier = {**receipt, 'stop_timeout_seconds': 397}
    content = module.unit_content(earlier)
    Path(receipt['unit_path']).write_bytes(content)
    earlier['unit_sha256'] = hashlib.sha256(content).hexdigest()
    module.write_private(operations.path, earlier)
    return earlier, content


def remote_drop_in(receipt):
    return Path(receipt['unit_path'] + '.d/forge-stop-budget.conf')


def test_remote_earlier_unit_gets_a_stop_budget_drop_in_and_reload(package, tmp_path):
    module = load_helper(package / 'remote_operations.py')
    operations = fake_operations(module, package, tmp_path / 'home')
    earlier, content = remote_earlier_unit(module, operations)
    widen = getattr(operations, 'widen_unit', None)
    assert widen is not None, 'the destination supervisor cannot widen its unit'
    for reported in (392_000, 432_000):
        operations.calls.clear()
        assert widen(reported) is True
        receipt = operations.load()
        # The owned unit and its receipt identity never change.
        assert Path(earlier['unit_path']).read_bytes() == content
        assert {key: value for key, value in receipt.items() if key != 'service_stop_budget'} == earlier
        assert receipt['service_stop_budget'] == {'gateway_shutdown_horizon_ms': reported,
                                                  'stop_timeout_seconds': remote_budget('service', reported)}
        assert remote_drop_in(earlier).read_bytes() == (
            f"[Service]\nTimeoutStopSec={remote_budget('service', reported)}\n".encode())
        assert operations.calls == [['daemon-reload']]
    # The applied budget leaves the unit bytes and unit_sha256 exactly as recorded.
    assert operations.load()['unit_sha256'] == earlier['unit_sha256'] == hashlib.sha256(
        Path(earlier['unit_path']).read_bytes()).hexdigest()
    # A shorter or equal report never narrows or rewrites it. Each readiness
    # reloads the manager again, completing a reload an interrupted run missed.
    operations.calls.clear()
    before = (remote_drop_in(earlier).read_bytes(), operations.path.read_bytes())
    assert widen(392_000) is True and widen(432_000) is True
    assert operations.calls == [['daemon-reload'], ['daemon-reload']]
    assert (remote_drop_in(earlier).read_bytes(), operations.path.read_bytes()) == before


def test_remote_stop_budget_drop_in_is_verified_byte_exact(package, tmp_path):
    module = load_helper(package / 'remote_operations.py')
    operations = fake_operations(module, package, tmp_path / 'home')
    earlier, _ = remote_earlier_unit(module, operations)
    operations.widen_unit(432_000)
    drop_in = remote_drop_in(earlier)
    written, recorded = drop_in.read_bytes(), json.loads(operations.path.read_text())
    drop_in.write_bytes(b'[Service]\nTimeoutStopSec=3600\n')
    with pytest.raises(ValueError, match='definition changed'):
        operations.load()
    drop_in.write_bytes(written)
    module.write_private(operations.path, earlier)
    with pytest.raises(ValueError, match='definition changed'):
        operations.load()
    # A record whose drop-in write was interrupted stays controllable, and the
    # next readiness completes it.
    module.write_private(operations.path, recorded)
    drop_in.unlink()
    assert operations.load() == recorded
    assert operations.widen_unit(432_000) is True and drop_in.read_bytes() == written
    for forged in ({'gateway_shutdown_horizon_ms': 432_000, 'stop_timeout_seconds': 3600},
                   {'gateway_shutdown_horizon_ms': '432000', 'stop_timeout_seconds': 482}):
        module.write_private(operations.path, {**recorded, 'service_stop_budget': forged})
        with pytest.raises(ValueError, match='unsupported shutdown budget'):
            operations.load()


@pytest.mark.parametrize('horizon_ms', [None, 432_000])
def test_remote_application_stop_fits_the_budget_its_manager_enforces(package, horizon_ms):
    module = load_helper(package / 'remote_operations.py')
    derived = remote_budget('application', horizon_ms or gateway_horizon_ms())
    assert module.application_stop_window(horizon_ms) == derived
    # An earlier unit enforces 397 seconds until a widening and its reload
    # succeed; a legacy one 45.
    for enforced in (397, 45):
        assert module.application_stop_window(horizon_ms, stop_budget=enforced) == enforced - 10 - 5 - 15
    assert module.application_stop_window(horizon_ms, stop_budget=remote_budget('service', 432_000)) == derived


def test_remote_unit_at_the_unreported_budget_gets_no_drop_in(package, tmp_path):
    module = load_helper(package / 'remote_operations.py')
    operations = fake_operations(module, package, tmp_path / 'home')
    receipt = operations.prepare()
    assert operations.widen_unit(gateway_horizon_ms()) is False and operations.calls == []
    assert not remote_drop_in(receipt).parent.exists()
    assert 'service_stop_budget' not in json.loads(operations.path.read_text())


def test_remote_earlier_owned_drop_in_below_its_record_stays_controllable(package, tmp_path):
    module = load_helper(package / 'remote_operations.py')
    operations = fake_operations(module, package, tmp_path / 'home')
    earlier, _ = remote_earlier_unit(module, operations)
    operations.widen_unit(432_000)
    drop_in = remote_drop_in(earlier)
    recorded = json.loads(operations.path.read_text())
    for budget in EARLIER_DROP_IN_BUDGETS:
        drop_in.write_bytes(f'[Service]\nTimeoutStopSec={budget}\n'.encode())
        assert operations.load() == recorded
    for content in FOREIGN_DROP_INS:
        drop_in.write_bytes(content)
        with pytest.raises(ValueError, match='definition changed'):
            operations.load()


def test_remote_stop_and_restart_wait_for_the_widened_budget(package, tmp_path, monkeypatch):
    module = load_helper(package / 'remote_operations.py')
    monkeypatch.setattr(module, 'console_healthy', lambda *args: True)
    operations = fake_operations(module, package, tmp_path / 'home')
    remote_earlier_unit(module, operations)
    operations.widen_unit(432_000)
    operations.current = 'active'
    operations.stop()
    operations.start(restart=True)
    assert [item for item in operations.timeouts if item[0] in {'stop', 'restart'}] == [
        ('stop', remote_budget('command', 432_000)),
        ('restart', remote_budget('command', 432_000) + module.START_TIMEOUT + 10)]


def test_remote_service_removal_also_removes_its_stop_budget_drop_in(package, tmp_path):
    module = load_helper(package / 'remote_operations.py')
    operations = fake_operations(module, package, tmp_path / 'home')
    earlier, _ = remote_earlier_unit(module, operations)
    operations.widen_unit(432_000)
    assert operations.stop(remove=True)['status'] == 'service-removed'
    assert not Path(earlier['unit_path']).exists() and not remote_drop_in(earlier).parent.exists()


def failing_once(operations, verb):
    """Kill the next manager call with this verb, as a crash at that point would."""
    original = operations.manager
    state = {'armed': True}
    def manager(arguments, **kwargs):
        if state['armed'] and arguments[0] == verb:
            state['armed'] = False
            raise RuntimeError('killed')
        return original(arguments, **kwargs)
    operations.manager = manager
    return original


@pytest.mark.parametrize('killed', ['after-record', 'before-reload', 're-widening-after-record'])
def test_remote_interrupted_widening_stays_controllable_and_completes(package, tmp_path, monkeypatch, killed):
    module = load_helper(package / 'remote_operations.py')
    monkeypatch.setattr(module, 'console_healthy', lambda *args: True)
    operations = fake_operations(module, package, tmp_path / 'home')
    earlier, _ = remote_earlier_unit(module, operations)
    drop_in = remote_drop_in(earlier)
    if killed == 'after-record':
        with monkeypatch.context() as patch:
            def killed_writer(stop_timeout):
                raise RuntimeError('killed')
            patch.setattr(module, 'drop_in_bytes', killed_writer)
            with pytest.raises(RuntimeError, match='killed'):
                operations.widen_unit(432_000)
        assert not drop_in.exists()
    elif killed == 'before-reload':
        original = failing_once(operations, 'daemon-reload')
        with pytest.raises(RuntimeError, match='killed'):
            operations.widen_unit(432_000)
        operations.manager = original
        assert drop_in.exists()
    else:
        # An earlier readiness widened the unit for a 392-second horizon. A
        # longer report records its budget, then the supervisor dies before
        # its drop-in replaces the earlier one.
        assert operations.widen_unit(392_000) is True
        replace = Path.replace
        def killed_replace(path, destination):
            if Path(destination).name == drop_in.name:
                raise RuntimeError('killed')
            return replace(path, destination)
        with monkeypatch.context() as patch:
            patch.setattr(Path, 'replace', killed_replace)
            with pytest.raises(RuntimeError, match='killed'):
                operations.widen_unit(432_000)
        assert drop_in.read_bytes() == f"[Service]\nTimeoutStopSec={remote_budget('service', 392_000)}\n".encode()
    assert operations.load()['service_stop_budget']['stop_timeout_seconds'] == remote_budget('service', 432_000)
    # Status, stop and restart keep working on the interrupted state.
    assert operations.status()['definition_verified'] is True
    operations.current = 'active'
    assert operations.stop()['ready'] is False
    assert operations.start(restart=True)['ready'] is True
    # The next readiness completes the step: the drop-in and a manager reload.
    operations.calls.clear()
    assert operations.widen_unit(432_000) is True
    assert drop_in.read_bytes() == f"[Service]\nTimeoutStopSec={remote_budget('service', 432_000)}\n".encode()
    assert operations.calls == [['daemon-reload']]


@pytest.mark.parametrize('killed', ['before-reload', 'before-record-drop'])
def test_remote_interrupted_removal_can_be_run_again(package, tmp_path, monkeypatch, killed):
    module = load_helper(package / 'remote_operations.py')
    operations = fake_operations(module, package, tmp_path / 'home')
    earlier, _ = remote_earlier_unit(module, operations)
    operations.widen_unit(432_000)
    if killed == 'before-reload':
        # Killed after the drop-in is deleted, before the manager reloads.
        original = failing_once(operations, 'daemon-reload')
        with pytest.raises(RuntimeError, match='killed'):
            operations.stop(remove=True)
        operations.manager = original
    else:
        # Killed after the reload, before the record is dropped.
        with monkeypatch.context() as patch:
            def killed_record(path, value, **kwargs):
                raise RuntimeError('killed')
            patch.setattr(module, 'write_private', killed_record)
            with pytest.raises(RuntimeError, match='killed'):
                operations.stop(remove=True)
    assert not remote_drop_in(earlier).exists() and Path(earlier['unit_path']).exists()
    # The uninstall is run again and completes.
    operations.calls.clear()
    assert operations.stop(remove=True)['status'] == 'service-removed'
    assert not Path(earlier['unit_path']).exists() and not operations.path.exists()
    assert not remote_drop_in(earlier).parent.exists()
    assert operations.calls[-1] == ['daemon-reload']


def test_remote_removal_deletes_the_drop_in_reloads_then_drops_the_record(package, tmp_path, monkeypatch):
    module = load_helper(package / 'remote_operations.py')
    operations = fake_operations(module, package, tmp_path / 'home')
    earlier, _ = remote_earlier_unit(module, operations)
    operations.widen_unit(432_000)
    order = []
    original_manager, original_write = operations.manager, module.write_private
    def manager(arguments, **kwargs):
        if arguments[0] == 'daemon-reload':
            recorded = operations.path.exists() and 'service_stop_budget' in json.loads(operations.path.read_text())
            order.append(('reload', remote_drop_in(earlier).exists(), recorded))
        return original_manager(arguments, **kwargs)
    def write_private(path, value, **kwargs):
        order.append(('record', 'service_stop_budget' in value))
        return original_write(path, value, **kwargs)
    operations.manager = manager
    monkeypatch.setattr(module, 'write_private', write_private)
    operations.stop(remove=True)
    # The first reload follows the drop-in's deletion while the record still
    # stands; the record is dropped after it.
    assert order[:2] == [('reload', False, True), ('record', False)]


FIXTURE_APP_REPORTING = FIXTURE_APP.replace("payload = {'pid'", "payload = {'gateway_shutdown_horizon_ms': HORIZON, 'pid'", 1)


@pytest.mark.asyncio
@pytest.mark.parametrize('reported, widening', [(432_000, 'widened'), (432_000, 'refused'), (432_000, 'timed-out'),
                                                (None, 'widened'), ('432000', 'widened')],
                         ids=['reported', 'widening-refused', 'widening-timed-out', 'unreported', 'malformed'])
async def test_actual_supervisor_widens_at_readiness_and_stops_within_the_reported_horizon(
        package, tmp_path, monkeypatch, capsys, reported, widening):
    assert FIXTURE_APP_REPORTING != FIXTURE_APP
    module = load_helper()
    operations = fixture_supervisor(package, tmp_path, module,
        FIXTURE_APP_REPORTING.replace('HORIZON', repr(reported)))
    if reported != '432000':
        # An earlier 397-second unit: a widening that succeeds raises the stop
        # budget, and one that fails keeps the unit's own.
        remote_earlier_unit(module, operations)
    widened, stops = [], []
    # No operating-system manager is reached: the unit widening is recorded only.
    def widen(horizon_ms):
        widened.append(horizon_ms)
        if widening == 'refused':
            raise module.DestinationError('The owned service definition changed. No existing unit was replaced or controlled.')
        if widening == 'timed-out':
            # A manager reload that outlives its bound.
            raise subprocess.TimeoutExpired(['/usr/bin/systemctl', '--user', 'daemon-reload'], 60)
        return True
    monkeypatch.setattr(operations, 'widen_unit', widen, raising=False)
    owned_stop = module.stop_supervised_application
    budgets = []
    async def recorded_stop(process, **options):
        stops.append(options.get('horizon_ms'))
        budgets.append(options.get('stop_budget'))
        await owned_stop(process)
    monkeypatch.setattr(module, 'stop_supervised_application', recorded_stop)
    task = asyncio.create_task(module.supervise(operations))
    try:
        deadline = time.monotonic() + 8
        while time.monotonic() < deadline and not task.done():
            await asyncio.sleep(0.1)
            if operations.health_path.exists() and json.loads(operations.health_path.read_text())['ready']:
                break
    finally:
        task.cancel()
        outcome = (await asyncio.gather(task, return_exceptions=True))[0]
    if reported == '432000':
        assert isinstance(outcome, ValueError) and 'gateway shutdown horizon' in str(outcome)
        assert (widened, stops) == ([], [None])
    else:
        # A refused or failed widening is reported and never blocks readiness.
        # An application that reports nothing is widened for the unreported
        # horizon. The stop fits the unit's own budget until a widening succeeds.
        assert isinstance(outcome, asyncio.CancelledError)
        assert (widened, stops) == ([reported or gateway_horizon_ms()], [reported])
        unit = module.service_stop_timeout(json.loads(operations.path.read_text()))
        assert budgets == [max(unit, remote_budget('service', widened[0])) if widening == 'widened' else unit]
        assert ('stop budget' in capsys.readouterr().out) is (widening != 'widened')


def test_remote_budgets_without_a_drain_window_are_the_plain_horizons(package, tmp_path):
    # The destination's mirror of the shutdown budget gives the plain
    # horizon's budgets, exactly, and its fresh unit needs no drop-in.
    module = load_helper(package / 'remote_operations.py')
    horizon = drained_horizon_ms(0)
    assert module.shutdown_budget(horizon) == deployment_deadlines.shutdown_budget(horizon) == {
        'application': 452, 'force_reap': 10, 'worker': 467, 'service': 482, 'command': 497,
        'legacy_service': 45}
    assert module.application_stop_window(horizon) == 452
    operations = fake_operations(module, package, tmp_path / 'home')
    assert operations.prepare()['stop_timeout_seconds'] == 482
    assert operations.widen_unit(horizon) is False and operations.calls == []


@pytest.mark.parametrize('window_ms', DRAIN_WINDOWS_MS, ids=['window-60s', 'window-300s'])
def test_remote_budgets_follow_a_drain_windows_report(package, tmp_path, window_ms):
    # The mirror computes the host's budgets for the drained horizon. The
    # report widens the destination's fresh unit through its owned drop-in,
    # and a reload of the receipt keeps the widened record.
    module = load_helper(package / 'remote_operations.py')
    horizon = drained_horizon_ms(window_ms)
    assert module.shutdown_budget(horizon) == deployment_deadlines.shutdown_budget(horizon)
    assert module.application_stop_window(horizon) == remote_budget('application', horizon)
    operations = fake_operations(module, package, tmp_path / 'home')
    receipt = operations.prepare()
    assert operations.widen_unit(horizon) is True and operations.calls == [['daemon-reload']]
    record = {'gateway_shutdown_horizon_ms': horizon, 'stop_timeout_seconds': remote_budget('service', horizon)}
    assert operations.load()['service_stop_budget'] == record
    assert remote_drop_in(receipt).read_bytes() == (
        f"[Service]\nTimeoutStopSec={remote_budget('service', horizon)}\n".encode())
    # Inside the widened unit budget the application keeps its whole window.
    assert module.application_stop_window(horizon, stop_budget=remote_budget('service', horizon)) == (
        remote_budget('application', horizon))


def test_remote_grown_drain_window_widens_again_and_a_smaller_report_never_narrows(package, tmp_path):
    module = load_helper(package / 'remote_operations.py')
    operations = fake_operations(module, package, tmp_path / 'home')
    receipt = operations.prepare()
    first, grown = (drained_horizon_ms(window) for window in DRAIN_WINDOWS_MS)
    operations.widen_unit(first)
    assert operations.widen_unit(grown) is True
    record = {'gateway_shutdown_horizon_ms': grown, 'stop_timeout_seconds': remote_budget('service', grown)}
    assert operations.load()['service_stop_budget'] == record
    for later in (first, gateway_horizon_ms()):
        operations.widen_unit(later)
        assert operations.load()['service_stop_budget'] == record
        assert remote_drop_in(receipt).read_bytes() == (
            f"[Service]\nTimeoutStopSec={remote_budget('service', grown)}\n".encode())
