"""Offline controls; no SSH, services, providers or pip installs.

Linux child cases exercise pip's filename/path projection with its installer
entry point replaced by a metadata-only observer.
"""
import base64
import errno
import fcntl
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import uuid

import pytest

from launchkit import remote_destination as controller
from launchkit.remote_deployment import source_archive
from test_application_wheel_assets import ASSET, wheel_bytes
from test_remote_destination import destination


def descriptor_bytes(descriptor):
    return os.pread(descriptor, controller.MAX_INPUT + 1, 0)


def invoke(monkeypatch, root, files, observe, name='requirements.application.txt'):
    (root / '.mobkit/launch-remote').mkdir(parents=True, exist_ok=True)
    commands = []
    def command(arguments, *args, **kwargs):
        commands.append((arguments, kwargs['pass_fds']))
        observe(arguments, kwargs['pass_fds'])
    monkeypatch.setattr(controller, '_command', command)
    controller._dependencies(root, {}, str(uuid.uuid4()), files, name, 'fixture')
    return commands


def test_registry_only_dependency_command_and_exact_descriptor_remain_unchanged(tmp_path, monkeypatch):
    content = b'# Preserve exact accepted registry text.\nordinary-package==1.2.3\n'
    def observe(arguments, descriptors):
        assert arguments[2:6] == ['-m', 'pip', 'install', '--disable-pip-version-check']
        assert descriptor_bytes(descriptors[0]) == content
        assert arguments[-1] == '/proc/self/fd/' + str(descriptors[0])
    invoke(monkeypatch, tmp_path, {'requirements.txt': content}, observe, name='requirements.txt')


def test_accepted_wheel_uses_exact_frozen_bytes_and_releases_all_descriptors(tmp_path, monkeypatch):
    content = wheel_bytes()
    public = tmp_path / ASSET
    public.parent.mkdir(parents=True)
    public.write_bytes(content)
    requirements = ('./' + ASSET + '\npython-telegram-bot==22.7\n').encode()
    captured = []
    def observe(arguments, descriptors):
        assert arguments[2:4] == ['-c', controller.ACCEPTED_DEPENDENCIES]
        assert content[:8].hex() not in repr(arguments)
        capsule = json.loads(descriptor_bytes(int(arguments[-1])))
        rows = descriptor_bytes(capsule['requirements_descriptor']).decode().splitlines()
        assert rows[1] == 'python-telegram-bot==22.7'
        assert rows[0].endswith('#sha256=' + hashlib.sha256(content).hexdigest())
        url, location = next(iter(capsule['wheel_paths'].items()))
        assert url.endswith('/' + Path(ASSET).name)
        wheel_fd = int(location.rsplit('/', 1)[1])
        public.write_bytes(b'changed public source must not reach pip')
        assert descriptor_bytes(wheel_fd) == content
        if sys.platform == 'linux':
            seals = fcntl.fcntl(wheel_fd, controller._memfd_seal_constant('F_GET_SEALS'))
            assert seals & controller._memfd_seal_constant('F_SEAL_WRITE')
            with pytest.raises(OSError) as failure:
                os.pwrite(wheel_fd, b'changed', 0)
            assert failure.value.errno == errno.EPERM
        captured.extend(descriptors)
    commands = invoke(monkeypatch, tmp_path, {'requirements.application.txt': requirements, ASSET: content}, observe)
    assert len(commands) == 1
    for descriptor in captured:
        with pytest.raises(OSError):
            os.fstat(descriptor)


@pytest.mark.parametrize('row', [
    '../foreign.whl', '/application/vendor/foreign.whl',
    './application/vendor/../foreign.whl', './application/vendor/nested/foreign.whl',
    './application/vendor/foreign.whl --no-deps', './application/vendor/foreign.whl; os_name=="posix"',
    './application/vendor/foreign.WHL', './application/vendor/foreign.whl[extra]',
    './application/vendor/not present.whl', './assets/foreign.whl',
    '-r requirements-other.txt', '-e ./application/vendor/foreign.whl',
    '--index-url https://example.invalid', 'foreign @ file:///application/vendor/foreign.whl',
])
def test_arbitrary_paths_flags_markers_editables_and_urls_never_start_command(tmp_path, monkeypatch, row):
    def unexpected(*args):
        pytest.fail('Unsupported dependency text must not start any command.')
    with pytest.raises(controller.ControllerError) as failure:
        invoke(monkeypatch, tmp_path, {'requirements.application.txt': (row + '\n').encode()}, unexpected)
    assert failure.value.code == 'invalid-package'


@pytest.mark.parametrize('case', ['missing', 'duplicate', 'empty', 'framework-file'])
def test_local_wheel_needs_one_existing_accepted_application_payload(tmp_path, monkeypatch, case):
    files = {'requirements.application.txt': ('./' + ASSET + '\n').encode(), ASSET: wheel_bytes()}
    name = 'requirements.application.txt'
    if case == 'missing':
        files.pop(ASSET)
    elif case == 'duplicate':
        files[name] *= 2
    elif case == 'empty':
        files[ASSET] = b''
    else:
        name = 'requirements.txt'
        files[name] = files.pop('requirements.application.txt')
    with pytest.raises(controller.ControllerError) as failure:
        invoke(monkeypatch, tmp_path, files, lambda *args: pytest.fail('Refused payload must not reach pip.'), name=name)
    assert failure.value.code == 'invalid-package'


def test_existing_transfer_archive_binding_includes_binary_wheel_bytes(destination):
    _, request, files, _ = destination
    content = wheel_bytes()
    files = {**files, ASSET: content, 'requirements.application.txt': ('./' + ASSET + '\n').encode()}
    archive, transferred = source_archive({name: (value, 0o644) for name, value in files.items()})
    binding = {**request['binding'], 'transferred_source_digest': transferred}
    _, accepted, _ = controller._archive(base64.b64encode(archive).decode(), binding)
    assert accepted[ASSET] == content
    assert hashlib.sha256(accepted[ASSET]).hexdigest() == hashlib.sha256(content).hexdigest()
    changed = {**files, ASSET: content + b'changed'}
    changed_archive, _ = source_archive({name: (value, 0o644) for name, value in changed.items()})
    with pytest.raises(controller.ControllerError) as failure:
        controller._archive(base64.b64encode(changed_archive).decode(), binding)
    assert failure.value.code == 'invalid-package'


@pytest.mark.skipif(sys.platform != 'linux', reason='Actual sealed memfd/proc descriptor custody requires Linux')
def test_actual_pip_link_reads_frozen_descriptor_with_original_wheel_name_without_install(tmp_path, monkeypatch):
    content = wheel_bytes()
    shim = r'''import hashlib,json,pathlib
import pip._internal.cli.main as cli
from pip._internal.models.link import Link
def observe(arguments):
    rows=pathlib.Path(arguments[-1]).read_text().splitlines()
    link=Link(rows[0])
    assert link.is_wheel and link.filename == "example_dependency-1.0-py3-none-any.whl"
    assert link.file_path.startswith('/proc/self/fd/')
    print(json.dumps({'filename':link.filename,'sha256':hashlib.sha256(pathlib.Path(link.file_path).read_bytes()).hexdigest()}))
    return 0
cli.main=observe
'''
    def observe(arguments, descriptors):
        result = subprocess.run([sys.executable, '-I', '-c', shim + controller.ACCEPTED_DEPENDENCIES, arguments[-1]],
            pass_fds=descriptors, capture_output=True, timeout=10)
        assert result.returncode == 0, result.stderr.decode()
        assert json.loads(result.stdout) == {'filename': Path(ASSET).name, 'sha256': hashlib.sha256(content).hexdigest()}
    invoke(monkeypatch, tmp_path, {'requirements.application.txt': ('./' + ASSET + '\n').encode(), ASSET: content}, observe)


@pytest.mark.skipif(sys.platform != 'linux', reason='Actual descriptor bootstrap requires Linux')
def test_unsupported_pip_path_interface_refuses_before_install_and_releases_descriptors(tmp_path, monkeypatch):
    held = []
    shim = 'from pip._internal.models.link import Link\nLink.file_path=None\n'
    def observe(arguments, descriptors):
        held.extend(descriptors)
        result = subprocess.run([sys.executable, '-I', '-c', shim + controller.ACCEPTED_DEPENDENCIES, arguments[-1]],
            pass_fds=descriptors, capture_output=True, timeout=10)
        assert result.returncode != 0
        assert b'does not support accepted wheel descriptors' in result.stderr
        raise controller.ControllerError('operation-failed')
    with pytest.raises(controller.ControllerError) as failure:
        invoke(monkeypatch, tmp_path, {'requirements.application.txt': ('./' + ASSET + '\n').encode(), ASSET: wheel_bytes()}, observe)
    assert failure.value.code == 'operation-failed'
    for descriptor in held:
        with pytest.raises(OSError):
            os.fstat(descriptor)
