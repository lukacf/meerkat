"""Trusted, standalone Forge controller for one owned Linux destination.

The SSH host supplies this module's exact bytes as CONTROLLER_SOURCE. Accepted
package helpers are executed only after the entire transferred manifest matches
the host's binding. This module has no imports from the mutable installation.
Installation consumes frozen helper bytes and guards each process boundary.
The systemd application runs ordinary installed source, with source checks
before/after control and during readiness. This is not lifetime protection
against a user editing that source under the same operating system identity.
"""
from __future__ import annotations

import base64
import builtins
import contextlib
import fcntl
import gzip
import hashlib
import io
import json
import os
from pathlib import Path
import platform
import pwd
import re
import shutil
import signal
import stat
import struct
import subprocess
import sys
import tarfile
import tempfile
import time
import types
import uuid


FRAME_PREFIX = 'MOBKIT_REMOTE_V1 '
MAX_SOURCE = 64 * 1024 * 1024
MAX_ARCHIVE_BYTES = MAX_SOURCE + 1024 * 1024
# Bound TAR headers, path metadata and padding before tarfile decodes them.
MAX_TAR_BYTES = MAX_SOURCE + 16 * 1024 * 1024
MAX_FILE = 16 * 1024 * 1024
MAX_INPUT = 96 * 1024 * 1024
MAX_RESULT = 64 * 1024
MAX_PRIVATE = 256 * 1024
BINDING_KEYS = {'guide_id', 'guide_version', 'build_id', 'accepted_source_digest',
                'package_sha256', 'transferred_source_digest', 'controller_sha256'}
DIGEST_KEYS = {'accepted_source_digest', 'package_sha256',
               'transferred_source_digest', 'controller_sha256'}
PRIVATE_COMPONENTS = {'.env', '.venv', '.bin', '.mobkit', '.git', '__pycache__', '.source'}
PRIVATE_ROOTS = {'data', 'workspace', 'source'}
ACTIVE = {'queued', 'installing', 'stopping', 'restarting'}
PHASES = {'admitted', 'configuration-pending', 'python-environment', 'dependencies',
          'application-dependencies', 'native-tools', 'application-source', 'knowledge-setup',
          'knowledge-start', 'service-install', 'service-retry-install', 'service-stop',
          'service-restart', 'service-check', 'complete', 'attention-required', 'unknown'}
PROC_ROOT = Path('/proc')
CGROUP_ROOT = Path('/sys/fs/cgroup')
INTERFACE_SECONDS = 4
MAX_LISTENER_PIDS = 256
MAX_LISTENER_FDS = 32768
INTERFACE_UNAVAILABLE = 'The declared interface listener could not be joined to this exact running application service.'
PRODUCT_RUNTIME_UNSUPPORTED = 'Selected Meta/Toolkit product packages require their local native setup owner. Remote installation is not supported.'
MESSAGES = {
    'invalid-request': 'The remote request is invalid or exceeds its supported bounds.',
    'host-changed': 'The destination user, home or installation path changed. Check the host again.',
    'unsafe-path': 'The destination must be an ordinary path owned by the remote user.',
    'not-owned': 'This folder does not belong to this exact accepted Forge installation.',
    'snapshot-changed': 'The private controller snapshot changed. No package or service was controlled.',
    'source-changed': 'The accepted source changed. Preserve this folder and use the exact accepted package.',
    'invalid-package': 'The transferred package does not match the accepted source binding.',
    'runtime-unsupported': PRODUCT_RUNTIME_UNSUPPORTED,
    'configuration-invalid': 'The private settings do not match the accepted package declarations.',
    'configuration-changed': 'The private configuration changed during setup. Its existing contents were preserved.',
    'settings-unavailable': 'Verify this exact installed source and fully stop its owned service before changing private settings.',
    'knowledge-not-owned': 'An existing knowledge container or volume belongs to another installation. No knowledge service was changed.',
    'preflight-failed': 'The remote host has unmet installation prerequisites. Check the host again.',
    'busy': 'An owned destination operation is still active. Reconnect to observe its progress.',
    'recovery-needed': 'The worker cannot be proven live or cleaned up. Preserve its custody and inspect this installation.',
    'operation-conflict': 'This operation identity belongs to a different destination action.',
    'retry-unavailable': 'This installation is not eligible for an explicit retry. Reconnect to inspect its owned state.',
    'operation-failed': 'The destination operation failed. Private settings and native state were preserved.',
    'observation-unavailable': 'The owned service could not be observed. Its custody was preserved.',
    'internal-error': 'The destination could not complete this request. Reconnect to inspect its owned state.',
}


class ControllerError(ValueError):
    def __init__(self, code, fields=None):
        self.code = code
        self.fields = fields
        super().__init__(MESSAGES[code])


def _fail(code):
    raise ControllerError(code)


def _digest(content):
    return hashlib.sha256(content).hexdigest()


def _uuid(value):
    if not isinstance(value, str):
        _fail('invalid-request')
    try:
        if str(uuid.UUID(value)) != value:
            _fail('invalid-request')
    except ValueError:
        _fail('invalid-request')
    return value


def _binding(value):
    if not isinstance(value, dict) or set(value) != BINDING_KEYS:
        _fail('invalid-request')
    if type(value['guide_version']) is not int or not 0 <= value['guide_version'] <= 10**12:
        _fail('invalid-request')
    for key in ('guide_id', 'build_id'):
        _uuid(value[key])
    for key in DIGEST_KEYS:
        if not isinstance(value[key], str) or not re.fullmatch(r'[a-f0-9]{64}', value[key]):
            _fail('invalid-request')
    return dict(value)


def _no_links(path):
    if any(item.is_symlink() for item in (path, *path.parents)):
        _fail('unsafe-path')


def _ordinary(path, *, maximum=MAX_FILE, private=False):
    _no_links(path)
    descriptor = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    with os.fdopen(descriptor, 'rb') as channel:
        info = os.fstat(channel.fileno())
        if (not stat.S_ISREG(info.st_mode) or info.st_uid != os.getuid()
                or info.st_nlink != 1 or info.st_size > maximum
                or private and info.st_mode & 0o077):
            _fail('unsafe-path')
        content = channel.read(maximum + 1)
    if len(content) > maximum:
        _fail('unsafe-path')
    return content


def _private_directory(path):
    _no_links(path)
    path.mkdir(mode=0o700, parents=True, exist_ok=True)
    info = path.stat()
    if not stat.S_ISDIR(info.st_mode) or info.st_uid != os.getuid() or info.st_mode & 0o077:
        _fail('unsafe-path')


def _write(path, payload, *, exclusive=False):
    _private_directory(path.parent)
    if path.exists() or path.is_symlink():
        _ordinary(path, private=True, maximum=max(MAX_PRIVATE, len(payload)))
    descriptor, name = tempfile.mkstemp(prefix='.' + path.name + '.', dir=path.parent)
    pending = Path(name)
    try:
        os.fchmod(descriptor, 0o600)
        with os.fdopen(descriptor, 'wb') as channel:
            channel.write(payload)
            channel.flush()
            os.fsync(channel.fileno())
        if exclusive:
            os.link(pending, path, follow_symlinks=False)
        else:
            os.replace(pending, path)
    finally:
        pending.unlink(missing_ok=True)


def _json_write(path, value, *, exclusive=False):
    _write(path, json.dumps(value, sort_keys=True, separators=(',', ':')).encode() + b'\n', exclusive=exclusive)


def _json_read(path):
    return json.loads(_ordinary(path, private=True, maximum=MAX_PRIVATE))


def _home():
    return Path(pwd.getpwuid(os.getuid()).pw_dir)


def _destination(value, home):
    if (not isinstance(value, str) or not 1 <= len(value) <= 4096
            or value != value.strip() or value.endswith('\\')
            or any(ord(char) < 32 or ord(char) == 127 for char in value)):
        _fail('unsafe-path')
    if value.startswith('~/'):
        value = str(home / value[2:])
    path = Path(value)
    if not path.is_absolute() or '..' in path.parts or path in {Path('/'), home}:
        _fail('unsafe-path')
    _no_links(path)
    nearest = path
    while not nearest.exists():
        nearest = nearest.parent
    info = nearest.stat()
    if not stat.S_ISDIR(info.st_mode) or info.st_uid != os.getuid():
        _fail('unsafe-path')
    return path


def _host(destination):
    home = _home()
    root = _destination(destination, home)
    return root, {'uid': os.getuid(), 'home': str(home), 'destination': str(root),
                  'platform': platform.system().lower(), 'python': platform.python_version()}


def _expected(request, host):
    expected = request.get('expected_host')
    if (not isinstance(expected, dict) or set(expected) != {'uid', 'home', 'destination'}
            or type(expected.get('uid')) is not int
            or expected != {key: host[key] for key in expected}):
        _fail('host-changed')


def _controller_source():
    content = globals().get('CONTROLLER_SOURCE')
    if content is None:
        content = _ordinary(Path(__file__), maximum=MAX_FILE)
    if not isinstance(content, bytes) or len(content) > MAX_FILE:
        _fail('snapshot-changed')
    return content



def _product_adapter():
    source = globals().get('PRODUCT_CONTROL_ADAPTER_SOURCE')
    manifest = globals().get('PRODUCT_CONTROL_MANIFEST')
    if not isinstance(source, bytes) or not isinstance(manifest, bytes):
        _fail('runtime-unsupported')
    namespace = {'__name__': 'forge_frozen_product_control', '__package__': 'launchkit'}
    exec(compile(source, '/__forge_frozen__/remote_product_control.py', 'exec'), namespace)
    return namespace, namespace['_decode'](manifest)

def _directory(root):
    return root / '.mobkit/launch-remote'


def _owner(root, identity, binding, host, *, required=True):
    path = _directory(root) / 'owner.json'
    if not path.exists() and not path.is_symlink():
        if required:
            _fail('not-owned')
        return None
    value = _json_read(path)
    if (not isinstance(value, dict) or value.get('protocol') != 1
            or value.get('deployment_id') != identity or value.get('binding') != binding
            or value.get('host') != {key: host[key] for key in ('uid', 'home', 'destination')}):
        _fail('not-owned')
    snapshots = value.get('snapshots')
    product = value.get('product_control') is True
    if product:
        _, manifest = _product_adapter()
        required = {'controller.py', 'control-selection.json', 'control-bundle.zip'}
        optional = set()
    else:
        required = {'controller.py', 'runtime-operations.py', 'deployment.json', 'application-configuration.py',
                    'accepted-source.tar.gz'}
        optional = {'knowledge-manage.py'}
    if (not isinstance(snapshots, dict) or not required <= set(snapshots)
            or set(snapshots) - (required | optional)
            or snapshots['controller.py'] != binding['controller_sha256']):
        _fail('snapshot-changed')
    for name, expected in snapshots.items():
        maximum = 128 * 1024 * 1024 if product and name == 'control-bundle.zip' else MAX_SOURCE + 1024 * 1024 if name == 'accepted-source.tar.gz' else MAX_FILE
        if not isinstance(expected, str) or _digest(_ordinary(_directory(root) / name, private=True, maximum=maximum)) != expected:
            _fail('snapshot-changed')
    if product and _json_read(_directory(root) / 'control-selection.json') != manifest:
        _fail('snapshot-changed')
    return value


def _transfer_owner(root, identity, binding):
    path = root / '.mobkit/gcp-package-owner.json'
    if not path.exists() and not path.is_symlink():
        return False
    value = _json_read(path)
    if value != {'id': uuid.UUID(identity).hex, 'root': str(root),
                 'source_digest': binding['transferred_source_digest']}:
        _fail('not-owned')
    return True


def _folder(root, identity, binding, host):
    owner = _owner(root, identity, binding, host, required=False)
    if owner:
        return 'owned'
    if not root.exists() or not any(root.iterdir()):
        return 'empty'
    if any(path.exists() or path.is_symlink() for path in (
            _directory(root) / 'operation.json', _directory(root) / 'admission.json',
            root / '.mobkit/remote-service.json', root / '.mobkit/remote-service-health.json')):
        _fail('recovery-needed')
    if _transfer_owner(root, identity, binding):
        return 'transfer-recoverable'
    _fail('not-owned')


def _environment(host):
    return {'HOME': host['home'], 'USER': pwd.getpwuid(host['uid']).pw_name,
            'LOGNAME': pwd.getpwuid(host['uid']).pw_name, 'LANG': 'C.UTF-8',
            'PATH': host['home'] + '/.local/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin',
            'PYTHONUNBUFFERED': '1', 'PYTHONNOUSERSITE': '1',
            'XDG_RUNTIME_DIR': '/run/user/' + str(host['uid']),
            'DBUS_SESSION_BUS_ADDRESS': 'unix:path=/run/user/' + str(host['uid']) + '/bus'}


def _probe(arguments, host):
    try:
        result = subprocess.run(arguments, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                                stderr=subprocess.DEVNULL, env=_environment(host), timeout=5, check=False)
        return result.returncode == 0
    except (OSError, subprocess.TimeoutExpired):
        return False


def _distribution(path=Path('/etc/os-release')):
    """Read only bounded, nonsecret distribution identifiers for setup advice."""
    try:
        descriptor = os.open(path, os.O_RDONLY | os.O_NONBLOCK)
        with os.fdopen(descriptor, 'rb') as stream:
            if not stat.S_ISREG(os.fstat(stream.fileno()).st_mode):
                return {}
            content = stream.read(16 * 1024 + 1)
        if len(content) > 16 * 1024:
            return {}
        values = {}
        for line in content.decode('utf-8').splitlines():
            key, separator, value = line.partition('=')
            if separator and key in {'ID', 'ID_LIKE'}:
                value = value.strip().strip('"').strip("'")
                if re.fullmatch(r'[a-z0-9_ -]{1,100}', value):
                    values[key] = value
        return values
    except (OSError, UnicodeError):
        return {}


def _python_environment_guidance(distribution):
    family = {distribution.get('ID'), *distribution.get('ID_LIKE', '').split()}
    if family.intersection({'ubuntu', 'debian'}):
        return ('Python needs venv and ensurepip support. On this Ubuntu or Debian computer, '
                'ask its administrator to install python3-venv for the Python interpreter in use, '
                'then recheck. Forge did not install system packages.')
    return ('Python needs venv and ensurepip support. Ask this Linux computer\'s administrator '
            'to install the matching Python virtual-environment package, then recheck. '
            'Forge did not install system packages.')


def _lingering(host):
    """Observe this exact user's logout policy without changing logind settings."""
    try:
        result = subprocess.run(['/usr/bin/loginctl', 'show-user', str(host['uid']),
                                 '--property=Linger', '--value', '--no-pager'],
            stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
            env=_environment(host), timeout=5, check=False)
        if result.returncode == 0 and len(result.stdout) <= 32:
            value = result.stdout.strip()
            if value in {b'yes', b'no'}:
                return value == b'yes'
    except (OSError, subprocess.TimeoutExpired):
        pass
    return None


def connection_prerequisites(definition):
    """Project granted stdio commands from checked source without resolving input."""
    try:
        used = {grant['connection_id'] for item in definition['classes']
                for grant in item['tools']['mcp']}
        connections = definition.get('mcpConnections', [])
        if (not isinstance(connections, list) or len(connections) > 33
                or any(not isinstance(item, dict) for item in connections)
                or any(not isinstance(key, str) or not re.fullmatch(r'[a-z][a-z0-9_-]{0,39}', key)
                       for key in used)):
            _fail('invalid-package')
        selected = []
        for key in sorted(used):
            matches = [item for item in connections if item.get('id') == key]
            if len(matches) != 1 or matches[0].get('transport') not in {'stdio', 'http'}:
                _fail('invalid-package')
            if matches[0]['transport'] == 'stdio':
                selected.append(matches[0])
        if len(selected) > 32:
            _fail('invalid-package')
        commands, deferred = set(), []
        for item in selected:
            command = item.get('command')
            if not isinstance(command, str) or not command or len(command) > 500 or '\x00' in command:
                _fail('invalid-package')
            if re.fullmatch(r'[A-Za-z0-9][A-Za-z0-9._+-]{0,79}', command):
                commands.add(command)
            else:
                # A package path or private reference may become available only
                # after materialization and setup. Never expand or run it here.
                deferred.append(item['id'])
        return {'mcp_prerequisites': sorted(commands), 'mcp_deferred_prerequisites': deferred}
    except (KeyError, TypeError, AttributeError):
        _fail('invalid-package')


def _preflight(request, root, identity, binding, host):
    folder = _folder(root, identity, binding, host)
    checks = []
    def check(name, passed, success, failure):
        checks.append({'name': name, 'status': 'passed' if passed else 'failed',
                       'detail': success if passed else failure})
    check('linux', host['platform'] == 'linux', 'Linux destination verified.', 'A Linux destination is required.')
    check('python', sys.version_info >= (3, 11), 'Python 3.11 or newer is available.', 'Python 3.11 or newer is required.')
    if host['platform'] == 'linux':
        # Match bootstrap.platform_target and the generated install_gateway.py
        # before package materialization, private writes or dependency work.
        architecture = {'arm64': 'aarch64', 'aarch64': 'aarch64',
                        'x86_64': 'x86_64', 'AMD64': 'x86_64'}.get(platform.machine())
        check('gateway-architecture', architecture is not None,
              'The runtime and maintenance tools support this Intel or ARM64 CPU.',
              'The runtime and maintenance tools require an Intel 64-bit (x86_64) or ARM64 Linux computer. Choose a supported computer, then recheck.')
        libc, version = platform.libc_ver()
        try:
            compatible_libc = libc == 'glibc' and tuple(int(part) for part in version.split('.')[:2]) >= (2, 31)
        except ValueError:
            compatible_libc = False
        check('gateway-libc', compatible_libc,
              'glibc 2.31 or newer is available for the runtime and maintenance tools.',
              'The runtime and maintenance tools require glibc 2.31 or newer. Alpine/musl and unknown libc are unsupported. Choose a compatible Linux computer or ask its administrator to upgrade it, then recheck.')
        distribution = _distribution()
        python_environment = _probe([sys.executable, '-I', '-c',
                                    'import venv, ensurepip; ensurepip.version()'], host)
        check('python-environment', python_environment,
              'Python venv and ensurepip modules are available. Environment creation and dependency downloads are checked during installation.',
              _python_environment_guidance(distribution))
    check('destination', True, 'The destination is empty or belongs to this exact installation.', '')
    manager = host['platform'] == 'linux' and _probe(['/usr/bin/systemctl', '--user', 'show-environment'], host)
    check('systemd-user', manager, 'The current user service manager is reachable.',
          'The current user service manager is unavailable; no system settings were changed.')
    lingering = _lingering(host) if host['platform'] == 'linux' else None
    continuity = {'user_lingering': 'enabled' if lingering is True else 'disabled' if lingering is False else 'unknown',
                  'reboot_verified': False}
    checks.append({'name': 'user-service-continuity', 'required': False,
                  'status': 'passed' if lingering is True else 'warning',
                  'detail': 'User lingering is enabled. This allows user services after logout; application restart after reboot is still unverified.'
                            if lingering is True else
                            'User lingering is disabled. This service may stop after SSH disconnects or logout. For unattended operation, ask the administrator to enable lingering for this SSH user, then recheck. Forge changed no system settings; reboot restart is unverified.'
                            if lingering is False else
                            'User lingering could not be verified. Do not rely on this service staying up after logout. Ask the administrator to check this SSH user\'s lingering policy, then recheck. Forge changed no system settings; reboot restart is unverified.'})
    mode = request.get('knowledge_mode', 'none')
    if mode not in {'none', 'remote', 'local'}:
        _fail('invalid-request')
    if mode == 'local':
        docker = shutil.which('docker', path=_environment(host)['PATH'])
        check('docker', bool(docker) and _probe([docker, 'info'], host),
              'Docker is reachable for local knowledge.', 'Docker is required for local knowledge.')
        check('docker-compose', bool(docker) and _probe([docker, 'compose', 'version'], host),
              'Docker Compose is available.', 'Docker Compose is required for local knowledge.')
        check('git', bool(shutil.which('git', path=_environment(host)['PATH'])),
              'Git is available for local knowledge.', 'Git is required for local knowledge.')
    prerequisites = request.get('mcp_prerequisites', [])
    if not isinstance(prerequisites, list) or len(prerequisites) > 32:
        _fail('invalid-request')
    for executable in prerequisites:
        if not isinstance(executable, str) or not re.fullmatch(r'[A-Za-z0-9][A-Za-z0-9._+-]{0,79}', executable):
            _fail('invalid-request')
        check('mcp-executable:' + executable, bool(shutil.which(executable, path=_environment(host)['PATH'])),
              'The selected connectors\' executable ' + executable + ' is available on the service PATH.',
              'The selected connectors require ' + executable + ' on the service PATH. Install it for this Linux user or ask its administrator, then recheck. No connector was started.')
    deferred = request.get('mcp_deferred_prerequisites', [])
    if (not isinstance(deferred, list) or len(deferred) > 32
            or any(not isinstance(key, str) or not re.fullmatch(r'[a-z][a-z0-9_-]{0,39}', key)
                   for key in deferred)):
        _fail('invalid-request')
    for key in deferred:
        checks.append({'name': 'mcp-command-deferred:' + key, 'required': False, 'status': 'warning',
                      'detail': 'The command for connector ' + key + ' depends on a path or private setup. Its availability will be checked after package dependencies and setup. Destination checks have not started this connector or verified its sign-in or tools.'})
    if prerequisites:
        checks.append({'name': 'mcp-connection-readiness', 'required': False, 'status': 'warning',
                      'detail': 'Executable availability does not verify connector startup, sign-in or selected tools. Installation checks those real connections after private setup.'})
    return {'status': 'preflight',
            'ready': all(item['status'] == 'passed' for item in checks if item.get('required', True)),
            'checks': checks, 'destination_state': folder, 'continuity': continuity}


def _product_interpreter_check(manifest):
    """The failed preflight check for a guest the product's control tools cannot use."""
    needed = manifest.get('interpreter') if isinstance(manifest, dict) else None
    needed = needed if isinstance(needed, dict) else {}
    tag = needed.get('python_tag')
    match = re.fullmatch(r'cp(3)(\d{1,2})', tag) if isinstance(tag, str) else None
    python = 'CPython ' + match.group(1) + '.' + match.group(2) if match else 'its selected CPython'
    machine = needed.get('machine')
    machine = machine if isinstance(machine, str) and re.fullmatch(r'[A-Za-z0-9_]{1,16}', machine) else 'its selected CPU'
    floor = needed.get('glibc_minimum')
    floor = ('glibc %d.%d or newer' % tuple(floor) if isinstance(floor, list) and len(floor) == 2
             and all(type(part) is int and 0 <= part < 1000 for part in floor) else 'its selected glibc')
    libc, version = platform.libc_ver()
    guest = 'CPython %d.%d on %s with %s' % (sys.version_info.major, sys.version_info.minor,
        platform.machine()[:32] or 'an unknown CPU', (libc + ' ' + version)[:32] if libc else 'an unknown libc')
    return {'name': 'product-interpreter', 'status': 'failed',
            'detail': 'The selected product\'s control tools need ' + python + ' on Linux ' + machine + ' with '
                      + floor + '; this destination runs ' + guest + '. Make that CPython the destination\'s '
                      'python3, or choose a destination that has it, then recheck.'}


def _excluded(name):
    parts = name.split('/')
    return bool(PRIVATE_COMPONENTS.intersection(parts) or parts[0] in PRIVATE_ROOTS or name.endswith('.receipt.json'))


def _archive(payload, binding):
    if not isinstance(payload, str) or len(payload) > MAX_ARCHIVE_BYTES * 4 // 3 + 8:
        _fail('invalid-package')
    try:
        data = base64.b64decode(payload, validate=True)
        if len(data) > MAX_ARCHIVE_BYTES:
            _fail('invalid-package')
        with gzip.GzipFile(fileobj=io.BytesIO(data)) as compressed:
            tar_bytes = compressed.read(MAX_TAR_BYTES + 1)
        if len(tar_bytes) > MAX_TAR_BYTES:
            _fail('invalid-package')
        files, total = {}, 0
        with tarfile.open(fileobj=io.BytesIO(tar_bytes), mode='r:') as archive:
            for entry in archive:
                name = entry.name
                parts = name.split('/')
                if (not entry.isfile() or name in files or name.startswith('/') or '\\' in name
                        or any(part in {'', '.', '..'} for part in parts)
                        or any(ord(char) < 32 or ord(char) == 127 for char in name)
                        or _excluded(name) or entry.size > MAX_FILE or len(files) >= 2049):
                    _fail('invalid-package')
                total += entry.size
                if total > MAX_SOURCE + 512 * 1024:
                    _fail('invalid-package')
                content = archive.extractfile(entry).read(MAX_FILE + 1)
                if len(content) != entry.size:
                    _fail('invalid-package')
                files[name] = content
        manifest = files.pop('SHA256SUMS')
        if len(manifest) > 512 * 1024:
            _fail('invalid-package')
        declared = {}
        for row in manifest.decode().splitlines():
            match = re.fullmatch(r'([a-f0-9]{64})  (.+)', row)
            if not match or match[2] in declared:
                _fail('invalid-package')
            declared[match[2]] = match[1]
        if set(declared) != set(files) or not files or len(files) > 2048:
            _fail('invalid-package')
        digest = hashlib.sha256()
        for name, content in sorted(files.items()):
            actual = _digest(content)
            if actual != declared[name]:
                _fail('invalid-package')
            digest.update(name.encode() + b'\0' + bytes.fromhex(actual))
        if digest.hexdigest() != binding['transferred_source_digest']:
            _fail('invalid-package')
        if not {'deployment.json', 'app.py', 'install.sh', 'install_gateway.py', 'requirements.txt', 'remote_operations.py',
                'application_configuration.py', 'implementation-report.json'} <= set(files):
            _fail('invalid-package')
        definition = json.loads(files['deployment.json'])
        report = json.loads(files['implementation-report.json'])
        if (definition.get('schemaVersion') != 2 or definition.get('target') not in {'linux', 'gcp'}
                or not re.fullmatch(r'[a-z][a-z0-9-]{1,39}', definition.get('slug', ''))
                or type(report.get('guide_version')) is not int
                or report['guide_version'] != binding['guide_version']
                or report.get('build_id') != binding['build_id']
                or report.get('verification', {}).get('digest') != binding['accepted_source_digest']):
            _fail('invalid-package')
        runtime = definition.get('runtimeCandidate')
        if isinstance(runtime, dict) and runtime.get('source') == 'meta-toolkit-product':
            _fail('runtime-unsupported')
        return data, files, definition
    except ControllerError:
        raise
    except Exception:
        _fail('invalid-package')


def _module(content, filename):
    namespace = {'__name__': 'launch_frozen_helper', '__file__': str(filename)}
    # No helper main(), package import, or mutable sys.path is invoked.
    exec(compile(content, str(filename), 'exec'), namespace)
    return namespace


def _runtime(root, owner):
    directory = _directory(root)
    content = _ordinary(directory / 'runtime-operations.py', private=True)
    metadata = _ordinary(directory / 'deployment.json', private=True)
    if (_digest(content) != owner['snapshots']['runtime-operations.py']
            or _digest(metadata) != owner['snapshots']['deployment.json']):
        _fail('snapshot-changed')
    module = _module(content, root / 'remote_operations.py')
    definition = json.loads(metadata)
    module['ROOT'] = root
    module['definition'] = lambda selected=root: definition
    operations = module['Operations'](root=root, home=Path(owner['host']['home']))
    return module, operations


def _console_configuration_helper():
    source = globals().get('CONSOLE_CONFIGURATION_SOURCE')
    if isinstance(source, bytes):
        return _module(source, Path('/launch/console_configuration.py'))
    # The repository test/import surface uses the same canonical pure module.
    # A standalone controller needs its frozen literal, never mutable imports.
    if globals().get('__package__'):
        from .templates_v2 import console_configuration
        return vars(console_configuration)
    _fail('configuration-invalid')


def _fields(files, definition, *, with_validator=False):
    rows = definition.get('environment', [])
    if not isinstance(rows, list) or len(rows) > 200:
        _fail('configuration-invalid')
    result, seen = [], set()
    reserved = {'PATH', 'HOME', 'USER', 'LOGNAME', 'SHELL', 'TMPDIR', 'TEMP', 'TMP',
                'NODE_OPTIONS', 'BASH_ENV', 'ENV', 'SSL_CERT_FILE', 'SSL_CERT_DIR'}
    knowledge = definition.get('knowledge') or {}
    automatic = {knowledge.get('token_env'), knowledge.get('space_env')} if knowledge.get('mode') == 'local' else set()
    for row in rows:
        if not isinstance(row, dict):
            _fail('configuration-invalid')
        key = row.get('key')
        if (not isinstance(key, str) or not re.fullmatch(r'[A-Z][A-Z0-9_]{0,79}', key)
                or key in seen or key in reserved or key.startswith(('PYTHON', 'DYLD_', 'LD_'))
                or type(row.get('secret')) is not bool or type(row.get('required')) is not bool):
            _fail('configuration-invalid')
        seen.add(key)
        copied = dict(row)
        if key in automatic:
            copied['required'] = False
            copied['automatic'] = True
        default = copied.get('default')
        if default is not None and (copied['secret'] or not isinstance(default, str)
                                    or len(default) > 32768 or '\0' in default):
            _fail('configuration-invalid')
        result.append(copied)
    if knowledge.get('mode') == 'local' and 'OPENAI_API_KEY' not in seen:
        result.append({'key': 'OPENAI_API_KEY', 'label': 'OpenAI API key for knowledge embeddings',
                       'secret': True, 'required': True})
        seen.add('OPENAI_API_KEY')
    helper = _module(files['application_configuration.py'], Path('/accepted/application_configuration.py'))
    value = json.loads(files.get('application/configuration.json', b'{"fields":[]}'))
    try:
        result.extend(helper['validate_application_fields'](value, seen))
    except ValueError:
        _fail('configuration-invalid')
    validator = helper.get('configuration_issues')
    if not callable(validator):
        validator = None
        if any(set(field) & {'min_length', 'max_length', 'nonblank', 'type', 'minimum', 'maximum'}
               for field in result):
            _fail('configuration-invalid')
    metadata = files.get('application/console.json')
    if metadata is not None:
        if len(metadata) > 4096:
            _fail('configuration-invalid')
        console = _console_configuration_helper()
        try:
            console['console_authentication'](json.loads(metadata), result)
        except (ValueError, UnicodeError):
            _fail('configuration-invalid')
        scalar_validator = validator
        def validator(fields, values):
            issues = scalar_validator(fields, values) if scalar_validator else [
                {'key': field['key'], 'label': field['label'],
                 'message': 'Enter a required value that is not only whitespace.'}
                for field in fields if field['required'] and not values.get(field['key'], '').strip()]
            invalid_keys = {issue['key'] for issue in issues}
            return issues + [issue for issue in console['console_credentials_issues'](fields, values)
                             if issue['key'] not in invalid_keys]
    return (result, validator) if with_validator else result


def private_settings_input(fields, settings, remove_fields=()):
    """Validate a bounded edit, without private I/O or invented field rules."""
    allowed = {field['key']: field for field in fields}
    if (not isinstance(settings, dict) or len(settings) > 100
            or set(settings) - (set(allowed) | {'MOBKIT_PORT'})
            or any(not isinstance(value, str) or len(value) > 32768 or '\0' in value
                   for value in settings.values())
            or any(field.get('automatic') and field['key'] in settings for field in fields)):
        _fail('configuration-invalid')
    if (not isinstance(remove_fields, (list, tuple)) or len(remove_fields) > 100
            or any(not isinstance(key, str) for key in remove_fields)
            or len(set(remove_fields)) != len(remove_fields)):
        _fail('configuration-invalid')
    if any(key not in allowed or allowed[key].get('required') or allowed[key].get('automatic')
           or settings.get(key) for key in remove_fields):
        _fail('configuration-invalid')


def _configuration_values(root):
    path = root / '.env'
    before, identity, values = None, None, {}
    if path.exists() or path.is_symlink():
        before = _ordinary(path, private=True, maximum=MAX_PRIVATE)
        info = path.stat()
        identity = (info.st_dev, info.st_ino, info.st_mtime_ns)
        for row in before.decode().splitlines():
            if not row or row.startswith('#'):
                continue
            key, separator, raw = row.partition('=')
            if not separator or not re.fullmatch(r'[A-Z][A-Z0-9_]*', key):
                _fail('configuration-invalid')
            raw = raw.strip()
            value = json.loads(raw) if raw.startswith('"') else raw[1:-1] if raw.startswith("'") and raw.endswith("'") else raw
            if not isinstance(value, str) or len(value) > 32768 or '\0' in value:
                _fail('configuration-invalid')
            values[key] = value
    return path, before, identity, values


def _configuration(root, settings, fields, validator=None, *, installed=False, remove_fields=()):
    if not isinstance(settings, dict) or len(settings) > 301:
        _fail('configuration-invalid')
    keys = {row['key'] for row in fields}
    if set(settings) - (keys | {'MOBKIT_PORT'}):
        _fail('configuration-invalid')
    if installed:
        private_settings_input(fields, settings, remove_fields)
    path, before, identity, values = _configuration_values(root)
    for key, value in settings.items():
        if not isinstance(key, str) or not isinstance(value, str) or len(value) > 32768 or '\0' in value:
            _fail('configuration-invalid')
        if value:
            values[key] = value
    for field in fields:
        key = field['key']
        if (key not in values and not field['secret'] and 'default' in field
                and (not installed or before is None) and key not in remove_fields):
            values[key] = field['default']
        if field.get('automatic') and key in settings and settings[key]:
            _fail('configuration-invalid')
    for key in remove_fields:
        values.pop(key, None)
    if validator is None:
        # Older frozen packages declare presence only. Never invent constraints.
        if any(set(field) & {'min_length', 'max_length', 'nonblank', 'type', 'minimum', 'maximum'}
               for field in fields):
            _fail('configuration-invalid')
        issues = [{'key': field['key'], 'label': field['label'],
                   'message': 'Enter a required value that is not only whitespace.'}
                  for field in fields if field['required'] and not values.get(field['key'], '').strip()]
    else:
        issues = validator(fields, values)
    if issues:
        raise ControllerError('configuration-invalid', fields=issues)
    if values.get('MOBKIT_PORT'):
        if not re.fullmatch(r'[0-9]{4,5}', values['MOBKIT_PORT']) or not 1024 <= int(values['MOBKIT_PORT']) <= 65535:
            _fail('configuration-invalid')
    payload = b'# Private Forge destination settings.\n' + ''.join(
        key + '=' + json.dumps(value, ensure_ascii=True) + '\n' for key, value in sorted(values.items())).encode()
    if len(payload) > MAX_PRIVATE:
        _fail('configuration-invalid')
    return path, before, identity, payload


def _save_configuration(prepared):
    path, before, identity, payload = prepared
    def unchanged():
        if before is None:
            return not path.exists() and not path.is_symlink()
        current = _ordinary(path, private=True, maximum=MAX_PRIVATE)
        info = path.stat()
        return current == before and (info.st_dev, info.st_ino, info.st_mtime_ns) == identity
    if not unchanged():
        _fail('configuration-changed')
    # The temporary file is private; the final check precedes replacement.
    descriptor, name = tempfile.mkstemp(prefix='.env.', dir=path.parent)
    pending = Path(name)
    try:
        with os.fdopen(descriptor, 'wb') as channel:
            os.fchmod(channel.fileno(), 0o600)
            channel.write(payload)
            channel.flush()
            os.fsync(channel.fileno())
        if not unchanged():
            _fail('configuration-changed')
        os.replace(pending, path)
    finally:
        pending.unlink(missing_ok=True)


@contextlib.contextmanager
def _lock(root, host):
    directory = Path(host['home']) / '.local/state/mobkit-launch/destination-locks'
    _private_directory(directory)
    path = directory / (_digest(str(root).encode()) + '.lock')
    _no_links(path)
    descriptor = os.open(path, os.O_RDWR | os.O_CREAT | os.O_NOFOLLOW, 0o600)
    try:
        info = os.fstat(descriptor)
        if (not stat.S_ISREG(info.st_mode) or info.st_uid != os.getuid()
                or info.st_nlink != 1 or info.st_mode & 0o077):
            _fail('unsafe-path')
        fcntl.flock(descriptor, fcntl.LOCK_EX)
        yield
    finally:
        os.close(descriptor)


def _job(root):
    path = _directory(root) / 'operation.json'
    return _json_read(path) if path.exists() or path.is_symlink() else None


def _process(pid):
    """Linux birth evidence; a reused numeric PID is never worker authority."""
    try:
        if type(pid) is not int or pid <= 0:
            return None
        path = Path('/proc') / str(pid)
        text = (path / 'stat').read_text()
        fields = text[text.rfind(')') + 2:].split()
        return {'pid': pid, 'start_ticks': fields[19], 'state': fields[0], 'group': int(fields[2]),
                'uid': path.stat().st_uid,
                'boot_id': Path('/proc/sys/kernel/random/boot_id').read_text().strip()}
    except (OSError, ValueError, IndexError):
        return None


def _live(evidence):
    if not isinstance(evidence, dict):
        return False
    current = _process(evidence.get('pid'))
    return bool(current and current['state'] not in {'Z', 'X'}
                and all(current.get(key) == evidence.get(key) for key in ('pid', 'start_ticks', 'uid', 'boot_id')))


def _worker_dead(evidence):
    """Absence requires recorded birth identity and a readable Linux process view."""
    if (not isinstance(evidence, dict) or type(evidence.get('pid')) is not int or evidence['pid'] <= 0
            or evidence.get('uid') != os.getuid() or type(evidence.get('uid')) is not int
            or not isinstance(evidence.get('start_ticks'), str) or not evidence['start_ticks'].isdigit()
            or not isinstance(evidence.get('boot_id'), str)):
        return False
    try:
        if str(uuid.UUID(evidence['boot_id'])) != evidence['boot_id']:
            return False
        processes = Path('/proc')
        boot = (processes / 'sys/kernel/random/boot_id').read_text().strip()
        if str(uuid.UUID(boot)) != boot:
            return False
        (processes / 'self/stat').read_text()
        if boot != evidence['boot_id']:
            return True
        current = _process(evidence['pid'])
        if current is not None:
            return (any(current.get(key) != evidence[key] for key in ('pid', 'start_ticks', 'uid', 'boot_id'))
                    or current.get('state') in {'Z', 'X'})
        try:
            os.stat(processes / str(evidence['pid']), follow_symlinks=False)
        except FileNotFoundError:
            return True
    except (OSError, ValueError):
        pass
    return False


def _retry_cleanup(job):
    return bool(job.get('cleanup_confirmed') is True
                and ('command' not in job or job.get('command_cleaned') is True))


def _retry_admission(root, owner):
    try:
        admission = _json_read(_directory(root) / 'admission.json')
        return (isinstance(admission, dict) and set(admission) == {'deployment_id', 'first_operation_id'}
                and admission['deployment_id'] == owner['deployment_id']
                and _uuid(admission['first_operation_id']) == admission['first_operation_id'])
    except (OSError, ValueError, TypeError):
        return False


def _service(operations):
    if not operations.path.exists() and not operations.path.is_symlink():
        return {'status': 'not-installed', 'ready': False}, False, True
    operations.load(require_source=False, allow_legacy_directory=True)
    # status() itself requires an exact live manager FragmentPath observation.
    try:
        value = operations.status()
    except Exception:
        return {'status': 'unknown', 'ready': False}, True, False
    result = {'ready': value.get('ready') is True}
    enums = {'status': {'running', 'active', 'inactive', 'failed', 'activating', 'deactivating', 'reloading', 'unknown'},
             'substate': {'running', 'dead', 'failed', 'start', 'start-pre', 'start-post', 'stop', 'stop-sigterm',
                          'stop-sigkill', 'stop-post', 'auto-restart', 'exited'},
             'load_state': {'loaded', 'not-found', 'bad-setting', 'error', 'masked', 'stub', 'merged'}}
    for key, choices in enums.items():
        if value.get(key) in choices:
            result[key] = value[key]
    result.setdefault('status', 'unknown')
    result['quiescent'] = False
    if result['status'] in {'inactive', 'failed'}:
        try:
            observed = operations.inspect(operations.load(require_source=False, allow_legacy_directory=True))
            result['quiescent'] = (observed.get('ActiveState') in {'inactive', 'failed'}
                                    and observed.get('MainPID') == '0' and observed.get('ControlPID') == '0'
                                    and observed.get('ControlGroup') == '')
        except Exception:
            return {'status': 'unknown', 'ready': False, 'quiescent': False}, True, False
    for key in ('source_current', 'definition_verified'):
        if type(value.get(key)) is bool:
            result[key] = value[key]
    if isinstance(value.get('source_digest'), str) and re.fullmatch(r'[a-f0-9]{64}', value['source_digest']):
        result['source_digest'] = value['source_digest']
    if isinstance(value.get('service'), str) and re.fullmatch(r'mobkit-launch\.[a-f0-9-]{36}\.service', value['service']):
        result['service'] = value['service']
    if isinstance(value.get('console_url'), str) and re.fullmatch(r'http://127\.0\.0\.1:[0-9]{4,5}/console', value['console_url']):
        result['console_url'] = value['console_url']
    result.update(scope='native-bootstrap-and-console', custom_interfaces_verified=False,
                  external_workflows_verified=False)
    return result, True, True


def _observe(root, owner):
    if owner.get('product_control') is True:
        adapter, manifest = _product_adapter()
        return adapter['observe'](globals(), root, owner, manifest)
    module, operations = _runtime(root, owner)
    try:
        current = module['source_digest'](root) == owner['binding']['transferred_source_digest']
    except Exception:
        current = False
    job = _job(root)
    live = bool(job and _live(job.get('worker')))
    state = job.get('status', 'not-installed') if job else 'not-installed'
    if job and state in ACTIVE and not live:
        state = 'recovery-needed'
    try:
        service, owned, observed = _service(operations)
    except Exception:
        service, owned, observed = {'status': 'unknown', 'ready': False}, False, False
    custody = bool(job or (_directory(root) / 'admission.json').exists()
                   or operations.path.exists() or operations.path.is_symlink())
    if not job and custody:
        state = 'recovery-needed'
    if job and state == 'failed' and (live or not _retry_cleanup(job) or not _worker_dead(job.get('worker'))):
        state = 'recovery-needed'
    if job and state == 'failed' and (not observed or service.get('status') == 'unknown'
                                    or not _retry_admission(root, owner)):
        state = 'recovery-needed'
    if job and state not in ACTIVE | {'recovery-needed', 'failed'}:
        state = ('running' if service.get('status') in {'running', 'active', 'activating', 'reloading'}
                 else 'stopped' if observed and service.get('status') in {'inactive', 'failed', 'not-installed'}
                 else 'recovery-needed')
    result = {'status': state, 'ready': bool(service.get('ready') and current and not live
              and state == 'running'), 'worker_pid': job.get('worker', {}).get('pid') if job else None,
              'worker_live': live, 'source_current': current, 'service': service,
                'service_owned': owned, 'service_observed': observed, 'admitted': custody,
                'cleanup_confirmed': bool(job and job.get('cleanup_confirmed') is True and not live),
                'retryable': False}
    if job:
        result.update(operation_id=job['operation_id'], phase=job.get('phase') if job.get('phase') in PHASES else 'unknown',
                      operation_status=job['status'])
        if 'failure_phase' in job:
            result['failure_phase'] = job['failure_phase'] if job['failure_phase'] in PHASES else 'unknown'
        if job.get('error_code'):
            result['error'] = {'code': job['error_code'], 'message': MESSAGES[job['error_code']]}
        if job.get('action') == 'retry-install':
            result['previous_operation_id'] = _uuid(job.get('previous_operation_id'))
        before_service = job.get('failure_phase') in {'admitted', 'python-environment', 'dependencies',
                          'application-dependencies', 'native-tools', 'application-source',
                          'knowledge-setup', 'knowledge-start'}
        result['retryable'] = bool(state == 'failed' and job.get('action') in {'install', 'retry-install'}
                                   and current and observed and _retry_admission(root, owner)
                                   and ((service.get('status') == 'not-installed' and not owned and before_service
                                         and not (root / '.mobkit/remote-service-health.json').exists()
                                         and not (root / '.mobkit/remote-service-health.json').is_symlink())
                                        or owned and service.get('status') in {'inactive', 'failed'}
                                           and service.get('quiescent') is True))
        if result['retryable']:
            result['retry_of_operation_id'] = _uuid(job['operation_id'])
    result['private_settings_configurable'] = _settings_stopped(root, owner, result, job)
    return result


def _settings_stopped(root, owner, observation, job):
    """Use original service, worker and admission custody for private edits."""
    return bool(observation.get('status') == 'stopped' and observation.get('source_current') is True
                and observation.get('service_owned') is True and observation.get('service_observed') is True
                and (observation.get('service') or {}).get('quiescent') is True
                and observation.get('worker_live') is False and job and job.get('status') == 'completed'
                and job.get('action') in {'install', 'retry-install', 'stop', 'restart'} and _retry_cleanup(job)
                and _worker_dead(job.get('worker')) and _retry_admission(root, owner))


def _installed_settings(request, root, identity, binding, host):
    """Observe or replace private values without starting work or altering history."""
    with _lock(root, host):
        owner = _owner(root, identity, binding, host)
        module, _ = _runtime(root, owner)
        _assert_current(root, owner, module)
        observation = _observe(root, owner)
        if not observation['private_settings_configurable']:
            _fail('settings-unavailable')
        files = _accepted_files(root, owner)
        definition = json.loads(files['deployment.json'])
        fields, validator = _fields(files, definition, with_validator=True)
        if request['action'] == 'configure':
            prepared = _configuration(root, request.get('settings', {}), fields, validator,
                installed=True, remove_fields=request.get('remove_fields', ()))
            _assert_current(root, owner, module)
            if not _observe(root, owner)['private_settings_configurable']:
                _fail('settings-unavailable')
            if prepared[1] != prepared[3]:
                _save_configuration(prepared)
            elif _configuration_values(root)[:3] != prepared[:3]:
                _fail('configuration-changed')
            return {**_observe(root, owner), 'configuration_saved': True}
        _, _, _, values = _configuration_values(root)
        port = values.get('MOBKIT_PORT', str(definition['port']))
        if not re.fullmatch(r'[0-9]{4,5}', port) or not 1024 <= int(port) <= 65535:
            port = str(definition['port'])
        _assert_current(root, owner, module)
        current = _observe(root, owner)
        if not current['private_settings_configurable']:
            _fail('settings-unavailable')
        return {**current, 'configured_keys': [field['key'] for field in fields if values.get(field['key'])],
                'console_port': int(port)}


def _experience_parser(root, owner):
    """Load the bundled canonical parser with one verified in-memory dependency."""
    content = globals().get('APPLICATION_EXPERIENCES_SOURCE')
    if not isinstance(content, bytes) or len(content) > 256 * 1024:
        _fail('snapshot-changed')
    configuration = _ordinary(_directory(root) / 'application-configuration.py', private=True)
    if _digest(configuration) != owner['snapshots']['application-configuration.py']:
        _fail('snapshot-changed')
    dependency = types.ModuleType('launch_interfaces.templates_v2.application_configuration')
    dependency.__file__ = str(root / 'application_configuration.py')
    exec(compile(configuration, dependency.__file__, 'exec'), dependency.__dict__)
    package = types.ModuleType('launch_interfaces')
    package.__path__ = ()
    parser = types.ModuleType('launch_interfaces.application_experiences')
    parser.__package__ = package.__name__
    parser.__file__ = '<frozen-application-experiences>'
    def imported(name, global_values=None, local_values=None, fromlist=(), level=0):
        if level:
            if level == 1 and name == 'templates_v2.application_configuration':
                return dependency
            _fail('snapshot-changed')
        return builtins.__import__(name, global_values, local_values, fromlist, level)
    parser.__dict__['__builtins__'] = {**vars(builtins), '__import__': imported}
    exec(compile(content, parser.__file__, 'exec'), parser.__dict__)
    return parser


def _interface_settings(root, keys):
    """Only source-declared nonsecret port values leave the private file reader."""
    if not keys:
        return {}
    content = _ordinary(root / '.env', private=True, maximum=MAX_PRIVATE)
    values, seen = {}, set()
    for line in content.decode().splitlines():
        key, separator, raw = line.strip().partition('=')
        if not separator or key not in keys:
            continue
        if key in seen:
            _fail('configuration-invalid')
        seen.add(key)
        raw = raw.strip()
        value = json.loads(raw) if raw.startswith('"') else raw[1:-1] if raw.startswith("'") and raw.endswith("'") else raw
        if not isinstance(value, str) or len(value) > 32768:
            _fail('configuration-invalid')
        values[key] = value
    return values


def _listener_budget(deadline):
    if time.monotonic() >= deadline:
        _fail('observation-unavailable')


def _proc_bytes(path, maximum, deadline, *, directory=None):
    _listener_budget(deadline)
    fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK, dir_fd=directory)
    with os.fdopen(fd, 'rb') as source:
        if not stat.S_ISREG(os.fstat(source.fileno()).st_mode):
            _fail('observation-unavailable')
        result = source.read(maximum + 1)
    _listener_budget(deadline)
    if len(result) > maximum:
        _fail('observation-unavailable')
    return result


def _listener_members(group, deadline):
    path = CGROUP_ROOT / group.lstrip('/') / 'cgroup.procs'
    _no_links(path)
    data = _proc_bytes(path, 8192, deadline).decode('ascii').splitlines()
    if (not data or len(data) > MAX_LISTENER_PIDS
            or any(not re.fullmatch(r'[1-9][0-9]{0,9}', item) for item in data)):
        _fail('observation-unavailable')
    values = [int(item) for item in data]
    if len(set(values)) != len(values):
        _fail('observation-unavailable')
    return set(values)


def _listener_socket(port, deadline):
    found = []
    for family, filename in ((4, 'tcp'), (6, 'tcp6')):
        rows = _proc_bytes(PROC_ROOT / 'self/net' / filename, 2 * 1024 * 1024, deadline).decode('ascii').splitlines()
        if not rows or len(rows) > 16384:
            _fail('observation-unavailable')
        for row in rows[1:]:
            _listener_budget(deadline)
            fields = row.split()
            if len(row) > 1024 or len(fields) < 4:
                _fail('observation-unavailable')
            if fields[3] != '0A':
                continue
            if len(fields) < 10:
                _fail('observation-unavailable')
            address, separator, raw_port = fields[1].partition(':')
            if (not separator or not re.fullmatch('[0-9A-Fa-f]{' + str(8 if family == 4 else 32) + '}', address)
                    or not re.fullmatch(r'[0-9A-Fa-f]{4}', raw_port)):
                _fail('observation-unavailable')
            if int(raw_port, 16) != port:
                continue
            if not re.fullmatch(r'[0-9]{1,10}', fields[7]) or not re.fullmatch(r'[1-9][0-9]{0,19}', fields[9]):
                _fail('observation-unavailable')
            found.append({'family': family, 'address': address.upper(), 'uid': int(fields[7]), 'inode': int(fields[9])})
    loopback = format(struct.unpack('=I', b'\x7f\x00\x00\x01')[0], '08X')
    # /proc renders native-endian IPv4 words; wildcard is also reachable through 127.0.0.1.
    if (len(found) != 1 or found[0]['family'] != 4 or found[0]['address'] not in {loopback, '00000000'}
            or found[0]['uid'] != os.getuid()):
        _fail('observation-unavailable')
    return found[0]


def _listener_birth(directory, pid, boot, deadline):
    info = os.fstat(directory)
    current = os.stat(PROC_ROOT / str(pid), follow_symlinks=False)
    text = _proc_bytes('stat', 8192, deadline, directory=directory).decode()
    fields = text[text.rfind(')') + 2:].split()
    if ((current.st_dev, current.st_ino) != (info.st_dev, info.st_ino)
            or info.st_uid != os.getuid() or not text.startswith(str(pid) + ' (') or len(fields) < 20
            or fields[0] in {'Z', 'X'} or not re.fullmatch(r'[0-9]{1,20}', fields[19])):
        _fail('observation-unavailable')
    return {'pid': pid, 'start_ticks': fields[19], 'uid': info.st_uid, 'boot_id': boot,
            'proc_device': info.st_dev, 'proc_inode': info.st_ino}


def _listener_group(directory, deadline):
    rows = _proc_bytes('cgroup', 8192, deadline, directory=directory).decode().splitlines()
    if len(rows) != 1 or not rows[0].startswith('0::'):
        _fail('observation-unavailable')
    return rows[0][3:]


def _listener_manager(operations, receipt, deadline):
    _listener_budget(deadline)
    manager = getattr(operations, 'manager', None)
    if manager is not None:
        def bounded(arguments, **kwargs):
            _listener_budget(deadline)
            kwargs['timeout'] = min(kwargs.get('timeout', 2), 2, max(0.01, deadline - time.monotonic()))
            return manager(arguments, **kwargs)
        operations.manager = bounded
    try:
        result = operations.inspect(receipt)
    finally:
        if manager is not None:
            operations.manager = manager
    _listener_budget(deadline)
    group = result.get('ControlGroup')
    if (result.get('LoadState') != 'loaded' or result.get('ActiveState') != 'active'
            or not re.fullmatch(r'[1-9][0-9]{0,9}', result.get('MainPID', ''))
            or result.get('ControlPID') != '0' or result.get('FragmentPath') != receipt['unit_path']
            or not isinstance(group, str) or len(group) > 1024 or not group.startswith('/') or group.startswith('//')
            or any(part in {'', '.', '..'} for part in group[1:].split('/'))
            or any(ord(character) < 33 or character in '\\' for character in group)
            or Path(group).name != Path(receipt['unit_path']).name):
        _fail('observation-unavailable')
    return {key: result[key] for key in ('LoadState', 'ActiveState', 'MainPID', 'ControlPID', 'ControlGroup', 'FragmentPath')}


def _listener_proof(root, owner, operations, port, deadline):
    """Observe current listener association; this is not lifetime or HTTP proof."""
    receipt = operations.load(require_source=True, allow_legacy_directory=True)
    before = _listener_manager(operations, receipt, deadline)
    group = before['ControlGroup']
    members = _listener_members(group, deadline)
    if int(before['MainPID']) not in members:
        _fail('observation-unavailable')
    socket = _listener_socket(port, deadline)
    boot = _proc_bytes(PROC_ROOT / 'sys/kernel/random/boot_id', 128, deadline).decode().strip()
    if not re.fullmatch(r'[a-f0-9-]{36}', boot):
        _fail('observation-unavailable')
    namespace = os.readlink(PROC_ROOT / 'self/ns/net')
    if not re.fullmatch(r'net:\[[0-9]{1,20}\]', namespace):
        _fail('observation-unavailable')
    holders, retained, count = [], [], 0
    with contextlib.ExitStack() as custody:
        for pid in sorted(members):
            _listener_budget(deadline)
            process = os.open(PROC_ROOT / str(pid), os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
            custody.callback(os.close, process)
            birth = _listener_birth(process, pid, boot, deadline)
            if _listener_group(process, deadline) != group or os.readlink('ns/net', dir_fd=process) != namespace:
                _fail('observation-unavailable')
            descriptors = os.open('fd', os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW, dir_fd=process)
            custody.callback(os.close, descriptors)
            matches, local_count = [], 0
            with os.scandir(descriptors) as rows:
                for item in rows:
                    _listener_budget(deadline)
                    count += 1
                    local_count += 1
                    if count > MAX_LISTENER_FDS or local_count > 4096 or not item.name.isdecimal():
                        _fail('observation-unavailable')
                    if os.readlink(item.name, dir_fd=descriptors) == 'socket:[' + str(socket['inode']) + ']':
                        matches.append(item.name)
            if (_listener_birth(process, pid, boot, deadline) != birth or _listener_group(process, deadline) != group
                    or os.readlink('ns/net', dir_fd=process) != namespace):
                _fail('observation-unavailable')
            if matches:
                holders.append({**birth, 'fds': sorted(matches)})
            retained.append((process, descriptors, birth, matches))
        if (not holders or _listener_members(group, deadline) != members or _listener_socket(port, deadline) != socket
                or operations.load(require_source=True, allow_legacy_directory=True) != receipt
                or _listener_manager(operations, receipt, deadline) != before
                or _proc_bytes(PROC_ROOT / 'sys/kernel/random/boot_id', 128, deadline).decode().strip() != boot
                or os.readlink(PROC_ROOT / 'self/ns/net') != namespace):
            _fail('observation-unavailable')
        for process, descriptors, birth, matches in retained:
            if (_listener_birth(process, birth['pid'], boot, deadline) != birth
                    or _listener_group(process, deadline) != group or os.readlink('ns/net', dir_fd=process) != namespace
                    or any(os.readlink(name, dir_fd=descriptors) != 'socket:[' + str(socket['inode']) + ']' for name in matches)):
                _fail('observation-unavailable')
        proof = {'deployment_id': owner['deployment_id'], 'binding': owner['binding'], 'service': receipt,
                 'group': group, 'port': port, 'socket': socket, 'namespace': namespace,
                 'main': next(birth for _, _, birth, _ in retained if birth['pid'] == int(before['MainPID'])),
                 'holders': holders}
        return _digest(json.dumps(proof, sort_keys=True, separators=(',', ':')).encode())


def _product_listener_snapshot(anchor, runtime_pid, port, deadline):
    """Join a socket to actual kernel ancestry rooted at an owned live handle.

    The caller supplies its original handle or verified manager observation.
    Neither a saved application PID nor a shared user cgroup selects an owner.
    Application children may create their own sessions and process groups.
    """
    boot = _proc_bytes(PROC_ROOT / 'sys/kernel/random/boot_id', 128, deadline).decode().strip()
    namespace = os.readlink(PROC_ROOT / 'self/ns/net')
    if (not re.fullmatch(r'[a-f0-9-]{36}', boot) or not re.fullmatch(r'net:\[[0-9]{1,20}\]', namespace)
            or anchor.get('kind') not in {'interactive-handle', 'background-manager'}
            or type(anchor.get('pid')) is not int or anchor['pid'] <= 0
            or type(runtime_pid) is not int or runtime_pid <= 0 or runtime_pid == anchor['pid']):
        _fail('observation-unavailable')
    socket = _listener_socket(port, deadline)
    rows, holders, count = {}, [], 0
    with contextlib.ExitStack() as custody:
        with os.scandir(PROC_ROOT) as entries:
            pids = [item.name for item in entries if item.name.isdecimal()]
        # A global same-user scan detects a foreign socket holder even if that
        # process happens to share the user's session or cgroup with Forge.
        if len(pids) > 4096:
            _fail('observation-unavailable')
        for text_pid in sorted(pids, key=int):
            _listener_budget(deadline)
            pid = int(text_pid)
            try:
                info = os.stat(PROC_ROOT / text_pid, follow_symlinks=False)
            except FileNotFoundError:
                continue
            if info.st_uid != os.getuid():
                continue
            with contextlib.ExitStack() as process_custody:
                process = os.open(PROC_ROOT / text_pid, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
                process_custody.callback(os.close, process)
                data = _proc_bytes('stat', 8192, deadline, directory=process).decode()
                fields = data[data.rfind(')') + 2:].split()
                if len(fields) < 20 or any(not re.fullmatch(r'[0-9]{1,10}', item) for item in fields[1:4]):
                    _fail('observation-unavailable')
                if fields[0] in {'Z', 'X'}:
                    continue
                birth = _listener_birth(process, pid, boot, deadline)
                row = {**birth, 'parent': int(fields[1]), 'group': int(fields[2]), 'session': int(fields[3])}
                rows[pid] = (None, row)
                if os.readlink('ns/net', dir_fd=process) != namespace:
                    continue
                with contextlib.ExitStack() as descriptor_custody:
                    descriptors = os.open('fd', os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW, dir_fd=process)
                    descriptor_custody.callback(os.close, descriptors)
                    matches, local_count = [], 0
                    with os.scandir(descriptors) as entries:
                        for item in entries:
                            _listener_budget(deadline)
                            count += 1
                            local_count += 1
                            if count > MAX_LISTENER_FDS or local_count > 4096 or not item.name.isdecimal():
                                _fail('observation-unavailable')
                            try:
                                link = os.readlink(item.name, dir_fd=descriptors)
                            except FileNotFoundError:
                                continue
                            if link == 'socket:[' + str(socket['inode']) + ']':
                                matches.append(item.name)
                    if matches:
                        custody.enter_context(descriptor_custody.pop_all())
                        holders.append((pid, descriptors, sorted(matches)))
                if matches or pid in {anchor['pid'], runtime_pid}:
                    custody.enter_context(process_custody.pop_all())
                    rows[pid] = (process, row)
                # Unrelated proc/fd directories close immediately. Initial
                # ancestry data can only be joined to a fresh directory with
                # the exact same birth/inode before entering the final proof.
        if anchor['pid'] not in rows or runtime_pid not in rows or not holders:
            _fail('observation-unavailable')
        worker = rows[anchor['pid']][1]
        runtime = rows[runtime_pid][1]
        # The original supervisor starts each application root in a fresh
        # session. Descendants may create further groups without losing lineage.
        if (runtime['group'] != runtime_pid or runtime['session'] != runtime_pid
                or anchor['kind'] == 'interactive-handle' and (
                    worker['group'] != anchor['pid'] or worker['session'] != anchor['pid']
                    or worker['parent'] != anchor.get('parent_pid'))):
            _fail('observation-unavailable')
        selected = {}
        def lineage(pid, parent):
            seen = set()
            for _ in range(64):
                if pid in seen or pid not in rows:
                    _fail('observation-unavailable')
                seen.add(pid)
                row = rows[pid][1]
                selected[pid] = row
                if pid == parent:
                    return
                next_pid = row['parent']
                if next_pid not in rows or int(row['start_ticks']) < int(rows[next_pid][1]['start_ticks']):
                    _fail('observation-unavailable')
                pid = next_pid
            _fail('observation-unavailable')
        lineage(runtime_pid, anchor['pid'])
        for pid, _, _ in holders:
            lineage(pid, runtime_pid)
        group, members = None, None
        if anchor['kind'] == 'background-manager':
            group = anchor.get('control_group')
            if (not isinstance(group, str) or len(group) > 1024 or not group.startswith('/')
                    or group.startswith('//') or any(part in {'', '.', '..'} for part in group[1:].split('/'))
                    or any(ord(character) < 33 or character in '\\' for character in group)):
                _fail('observation-unavailable')
            members = _listener_members(group, deadline)
            if not set(selected).issubset(members):
                _fail('observation-unavailable')
        for pid, row in selected.items():
            process = rows[pid][0]
            if process is None:
                process = os.open(PROC_ROOT / str(pid), os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
                custody.callback(os.close, process)
                rows[pid] = (process, row)
            if (_listener_birth(process, pid, boot, deadline) != {key: row[key] for key in (
                    'pid', 'start_ticks', 'uid', 'boot_id', 'proc_device', 'proc_inode')}
                    or os.readlink('ns/net', dir_fd=process) != namespace
                    or group is not None and _listener_group(process, deadline) != group):
                _fail('observation-unavailable')
            data = _proc_bytes('stat', 8192, deadline, directory=process).decode()
            fields = data[data.rfind(')') + 2:].split()
            if tuple(map(int, fields[1:4])) != (row['parent'], row['group'], row['session']):
                _fail('observation-unavailable')
        if (_listener_socket(port, deadline) != socket
                or group is not None and _listener_members(group, deadline) != members
                or _proc_bytes(PROC_ROOT / 'sys/kernel/random/boot_id', 128, deadline).decode().strip() != boot
                or os.readlink(PROC_ROOT / 'self/ns/net') != namespace):
            _fail('observation-unavailable')
        for _, descriptors, matches in holders:
            if any(os.readlink(name, dir_fd=descriptors) != 'socket:[' + str(socket['inode']) + ']' for name in matches):
                _fail('observation-unavailable')
        return {'anchor': anchor, 'port': port, 'socket': socket, 'namespace': namespace,
            'runtime': runtime, 'lineage': [selected[pid] for pid in sorted(selected)],
            'holders': [{'pid': pid, 'fds': matches} for pid, _, matches in holders]}


def _product_listener_proof(owner, runtime_binding, anchor, port, deadline, require_current):
    """Observe association twice, retaining the original application owner."""
    require_current()
    before = _product_listener_snapshot(anchor, runtime_binding['runtime_pid'], port, deadline)
    require_current()
    after = _product_listener_snapshot(anchor, runtime_binding['runtime_pid'], port, deadline)
    require_current()
    if before != after:
        _fail('observation-unavailable')
    return _digest(json.dumps({'binding': owner['binding'], 'deployment_id': owner['deployment_id'],
        'runtime_binding': runtime_binding, 'listener': before}, sort_keys=True, separators=(',', ':')).encode())


def _application_experiences(root, owner, observation):
    binding = owner['binding']
    result = {'protocol': 1, 'supported': isinstance(globals().get('APPLICATION_EXPERIENCES_SOURCE'), bytes),
              'status': 'unavailable', 'source_digest': binding['accepted_source_digest'],
              'installed_source_digest': binding['transferred_source_digest'], 'metadata_sha256': None, 'entries': []}
    if not result['supported'] or not observation.get('source_current'):
        return result
    try:
        module, operations = _runtime(root, owner)
        _assert_current(root, owner, module)
        files = _accepted_files(root, owner)
        definition = json.loads(files['deployment.json'])
        parser = _experience_parser(root, owner)
        declared = parser.declared_application_experiences(files, definition)
        result['metadata_sha256'] = declared['metadata_sha256']
        if not declared['entries']:
            result['status'] = 'none'
            return result
        keys = {entry['port_field'] for entry in declared['entries'] if 'port_field' in entry}
        native = observation.get('service') or {}
        console_url = native.get('console_url') if observation.get('ready') else None
        resolved = parser.resolve_application_experiences(files, definition,
            source_digest=binding['accepted_source_digest'], console_url=console_url,
            configuration_values=_interface_settings(root, keys))
        deadline = time.monotonic() + INTERFACE_SECONDS
        eligible = (observation.get('ready') is True and observation.get('worker_live') is False
                    and observation.get('service_owned') is True and observation.get('service_observed') is True)
        tokens = {}
        for entry in resolved['entries']:
            port, token = entry['destination_port'], None
            reason = entry['reason']
            if port is not None and not reason and eligible:
                if port not in tokens:
                    try:
                        tokens[port] = _listener_proof(root, owner, operations, port, deadline)
                    except Exception:
                        tokens[port] = None
                token = tokens[port]
            if not token and not reason:
                reason = INTERFACE_UNAVAILABLE
            result['entries'].append({key: entry[key] for key in ('id', 'label', 'path')}
                | {'destination_port': port, 'listener_owned': bool(token), 'listener_token': token,
                   'reason': reason, 'verified': False})
        _assert_current(root, owner, module)
        result['status'] = 'resolved'
    except Exception:
        result.update(status='unavailable', entries=[])
    return result


# This fixed bootstrap verifies private source before executing the worker. The
# snapshot path, hash and correlation ID are public argv; private values are not.
WORKER_BOOTSTRAP = """import hashlib,os,stat,sys
p=sys.argv[1]
fd=os.open(p,os.O_RDONLY|os.O_NOFOLLOW|os.O_NONBLOCK)
with os.fdopen(fd,'rb') as f:
 s=os.fstat(f.fileno()); b=f.read(16777217)
if not stat.S_ISREG(s.st_mode) or s.st_uid!=os.getuid() or s.st_nlink!=1 or s.st_mode&63 or len(b)>16777216 or hashlib.sha256(b).hexdigest()!=sys.argv[2]:
 sys.exit(1)
n={'__name__':'launch_remote_worker','__file__':p,'CONTROLLER_SOURCE':b}
exec(compile(b,p,'exec'),n)
n['worker'](sys.argv[3],sys.argv[4],**({'private_settings_fd':int(sys.argv[5])} if len(sys.argv)==6 else {}))
"""


def _spawn(root, owner, operation_id, *, private_settings_fd=None):
    arguments = [sys.executable, '-I', '-c', WORKER_BOOTSTRAP,
                 str(_directory(root) / 'controller.py'), owner['binding']['controller_sha256'],
                 str(root), operation_id]
    if private_settings_fd is not None:
        arguments.append(str(private_settings_fd))
    process = subprocess.Popen(arguments, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                               stderr=subprocess.DEVNULL, close_fds=True, start_new_session=True,
                               pass_fds=() if private_settings_fd is None else (private_settings_fd,),
                               env=_environment(owner['host']))
    evidence = _process(process.pid)
    if evidence is None:
        evidence = {'pid': process.pid, 'start_ticks': None, 'uid': os.getuid(), 'boot_id': None}
    # No wait: SSH admission ends while this separately owned worker continues.
    return evidence


def _archive_operation(root, job):
    operation_id = _uuid(job.get('operation_id'))
    content = _ordinary(_directory(root) / 'operation.json', private=True, maximum=MAX_PRIVATE)
    if json.loads(content) != job:
        _fail('recovery-needed')
    path = _directory(root) / 'operations' / (operation_id + '.json')
    if path.exists() or path.is_symlink():
        if _ordinary(path, private=True, maximum=MAX_PRIVATE) != content:
            _fail('operation-conflict')
    else:
        _write(path, content, exclusive=True)


def _queue(root, owner, action, operation_id, *, previous=None, configuration=None, product_settings=None):
    job = {'operation_id': operation_id, 'action': action, 'status': 'queued',
           'phase': 'configuration-pending' if configuration else 'admitted',
           'worker': {}, 'admitted_at': time.time()}
    if action == 'retry-install':
        job['previous_operation_id'] = previous['operation_id']
    if previous:
        _archive_operation(root, previous)
    admission = _directory(root) / 'admission.json'
    if not admission.exists():
        _json_write(admission, {'deployment_id': owner['deployment_id'], 'first_operation_id': operation_id}, exclusive=True)
    _json_write(_directory(root) / 'operation.json', job)
    try:
        if configuration:
            _save_configuration(configuration)
        if product_settings is not None:
            if owner.get('product_control') is not True or action != 'install' or configuration is not None:
                _fail('invalid-request')
            content = json.dumps(product_settings, ensure_ascii=True, allow_nan=False).encode('ascii')
            if len(content) > MAX_PRIVATE:
                _fail('configuration-invalid')
            with _input_descriptor(content, root) as private:
                job['worker'] = _spawn(root, owner, operation_id, private_settings_fd=private)
        else:
            job['worker'] = _spawn(root, owner, operation_id)
        job['status'] = {'install': 'installing', 'retry-install': 'installing',
                         'stop': 'stopping', 'restart': 'restarting'}[action]
        job['phase'] = 'admitted'
        _json_write(_directory(root) / 'operation.json', job)
    except Exception:
        # Configuration/spawn interruption retains the new operation's custody.
        _fail('recovery-needed')
    return {**_observe(root, owner), 'admitted': True}


def _admit(root, owner, action, operation_id):
    previous = _job(root)
    if previous is None and ((_directory(root) / 'admission.json').exists()
                             or (root / '.mobkit/remote-service.json').exists()):
        _fail('recovery-needed')
    if previous:
        if previous['operation_id'] == operation_id:
            if previous['action'] != action:
                _fail('operation-conflict')
            return {**_observe(root, owner), 'admitted': True}
        observation = _observe(root, owner)
        if observation['worker_live']:
            _fail('busy')
        if observation['status'] == 'recovery-needed':
            _fail('recovery-needed')
        if action == 'install':
            # A new install identity cannot repeat dependency or data setup.
            return {**observation, 'admitted': True}
    module, operations = _runtime(root, owner)
    if action != 'stop':
        try:
            current = module['source_digest'](root) == owner['binding']['transferred_source_digest']
        except Exception:
            current = False
        if not current:
            _fail('source-changed')
    if action != 'install':
        operations.load(require_source=action == 'restart', allow_legacy_directory=action == 'stop')
    if action == 'restart':
        files = _accepted_files(root, owner)
        fields, validator = _fields(files, json.loads(files['deployment.json']), with_validator=True)
        _configuration(root, {}, fields, validator)
    return _queue(root, owner, action, operation_id, previous=previous)


def _retry_install(request, root, identity, binding, host):
    previous_id = _uuid(request.get('previous_operation_id'))
    operation_id = request['operation_id']
    if operation_id == previous_id:
        _fail('operation-conflict')
    with _lock(root, host):
        owner = _owner(root, identity, binding, host)
        previous = _job(root)
        if previous and previous.get('operation_id') == operation_id:
            if previous.get('action') != 'retry-install' or previous.get('previous_operation_id') != previous_id:
                _fail('operation-conflict')
            return {**_observe(root, owner), 'admitted': True}
        archived = _directory(root) / 'operations' / (operation_id + '.json')
        if archived.exists() or archived.is_symlink():
            _fail('operation-conflict')
        if not previous or previous.get('operation_id') != previous_id:
            _fail('operation-conflict' if previous else 'recovery-needed')
        observation = _observe(root, owner)
        if observation['worker_live']:
            _fail('busy')
        if observation['status'] == 'recovery-needed':
            _fail('recovery-needed')
        if not observation['source_current']:
            _fail('source-changed')
        if not observation['retryable']:
            _fail('retry-unavailable')
        if 'settings' not in request:
            _fail('configuration-invalid')
        files = _accepted_files(root, owner)
        definition = json.loads(files['deployment.json'])
        fields, validator = _fields(files, definition, with_validator=True)
        configuration = _configuration(root, request['settings'], fields, validator)
        checked = _preflight({**request, **connection_prerequisites(definition),
                             'knowledge_mode': (definition.get('knowledge') or {}).get('mode', 'none')},
                             root, identity, binding, host)
        if not checked['ready']:
            _fail('preflight-failed')
        module, operations = _runtime(root, owner)
        _assert_current(root, owner, module)
        if not _observe(root, owner)['retryable']:
            _fail('recovery-needed')
        return _queue(root, owner, 'retry-install', operation_id, previous=previous, configuration=configuration)


def _product_install(request, root, identity, binding, host):
    with _lock(root, host):
        owner = _owner(root, identity, binding, host)
        if owner.get('product_control') is not True:
            _fail('runtime-unsupported')
        previous = _job(root)
        if previous:
            if previous['operation_id'] == request['operation_id'] and previous['action'] != 'install':
                _fail('operation-conflict')
            # A changed UUID cannot repeat the original byte Install or Fresh input retention.
            return {**_observe(root, owner), 'admitted': True}
        if (_directory(root) / 'admission.json').exists():
            _fail('recovery-needed')
        if type(request.get('settings')) is not dict:
            _fail('configuration-invalid')
        return _queue(root, owner, 'install', request['operation_id'], product_settings=request['settings'])


def _install(request, root, identity, binding, host, source):
    data, files, definition = _archive(request.get('archive'), binding)
    with _lock(root, host):
        folder = _folder(root, identity, binding, host)
        owner = _owner(root, identity, binding, host, required=False)
        if owner and not _job(root) and ((_directory(root) / 'admission.json').exists()
                                         or (root / '.mobkit/remote-service.json').exists()):
            _fail('recovery-needed')
        if owner and _job(root):
            return _admit(root, owner, 'install', request['operation_id'])
        fields, validator = _fields(files, definition, with_validator=True)
        configuration = _configuration(root, request.get('settings', {}), fields, validator)
        checked = _preflight({**request, **connection_prerequisites(definition),
                             'knowledge_mode': (definition.get('knowledge') or {}).get('mode', 'none')},
                             root, identity, binding, host)
        if not checked['ready']:
            _fail('preflight-failed')
        helper = _module(files['remote_operations.py'], root / 'remote_operations.py')
        # receive_archive validates every file before materializing any source.
        parent = root
        while not parent.exists():
            parent = parent.parent
        descriptor, name = tempfile.mkstemp(prefix='.launch-transfer.', dir=parent)
        transfer = Path(name)
        try:
            with os.fdopen(descriptor, 'wb') as channel:
                os.fchmod(channel.fileno(), 0o600)
                channel.write(data)
                channel.flush()
                os.fsync(channel.fileno())
            helper['receive_archive'](transfer, root, uuid.UUID(identity).hex, binding['transferred_source_digest'])
        finally:
            transfer.unlink(missing_ok=True)
        directory = _directory(root)
        _private_directory(directory)
        snapshots = {'controller.py': source, 'runtime-operations.py': files['remote_operations.py'],
                     'deployment.json': files['deployment.json'],
                     'application-configuration.py': files['application_configuration.py'],
                     'accepted-source.tar.gz': data}
        if (definition.get('knowledge') or {}).get('mode') == 'local':
            snapshots['knowledge-manage.py'] = files['knowledge/manage.py']
        for name, content in snapshots.items():
            path = directory / name
            if path.exists() or path.is_symlink():
                if _ordinary(path, private=True, maximum=max(MAX_FILE, len(content))) != content:
                    _fail('snapshot-changed')
            else:
                _write(path, content, exclusive=True)
        value = {'protocol': 1, 'deployment_id': identity, 'binding': binding,
                 'host': {key: host[key] for key in ('uid', 'home', 'destination')},
                 'snapshots': {name: _digest(content) for name, content in snapshots.items()}}
        if owner is None:
            _json_write(directory / 'owner.json', value, exclusive=True)
        elif owner != value:
            _fail('not-owned')
        _save_configuration(configuration)
        return _admit(root, value, 'install', request['operation_id'])


def _group_live(group):
    result = []
    for path in Path('/proc').iterdir():
        if path.name.isdigit():
            observed = _process(int(path.name))
            if observed and observed['uid'] == os.getuid() and observed['group'] == group and observed['state'] not in {'Z', 'X'}:
                result.append(observed)
    return result


def _cleanup_child(process):
    # Popen still owns its unreaped child. Never poll before signaling, since
    # reaping would remove that protection against numeric group ID reuse.
    if process.returncode is not None:
        return False
    try:
        os.killpg(process.pid, signal.SIGKILL)
    except ProcessLookupError:
        pass
    except OSError:
        return False
    try:
        process.wait(timeout=10)
    except (OSError, subprocess.TimeoutExpired):
        return False
    deadline = time.monotonic() + 10
    while time.monotonic() < deadline:
        try:
            os.killpg(process.pid, 0)
        except ProcessLookupError:
            return True
        except OSError:
            return False
        if sys.platform == 'linux' and not _group_live(process.pid):
            return True
        time.sleep(0.05)
    return False


def _command(arguments, root, owner, operation_id, phase, timeout=1200, *, pass_fds=()):
    process = None
    cleaned = False
    try:
        process = subprocess.Popen(arguments, cwd=root, env=_environment(owner['host']),
                                   stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                                   close_fds=True, start_new_session=True, pass_fds=pass_fds, umask=0o077)
        evidence = _process(process.pid)
        if evidence is None:
            _fail('recovery-needed')
        _progress(root, owner, operation_id, phase, command=evidence, command_cleaned=False, cleanup_confirmed=False)
        deadline = None if timeout is None else time.monotonic() + timeout
        # A retained zombie preserves the owned group ID before cleanup.
        while True:
            current = _process(process.pid)
            if not current or current['state'] in {'Z', 'X'}:
                break
            if deadline is not None and time.monotonic() >= deadline:
                _fail('operation-failed')
            time.sleep(0.1)
        if not current or any(current[key] != evidence[key] for key in ('pid', 'start_ticks', 'uid', 'boot_id')):
            _fail('recovery-needed')
        if _group_live(process.pid):
            cleaned = _cleanup_child(process)
            _fail('operation-failed' if cleaned else 'recovery-needed')
        result = process.wait(timeout=10)
        cleaned = True
        _progress(root, owner, operation_id, phase, command_cleaned=True, cleanup_confirmed=True)
        if result:
            _fail('operation-failed')
    except BaseException as error:
        if process is not None and not cleaned:
            cleaned = _cleanup_child(process)
        # Publication failure never skips cleanup or promotes unknown custody
        # to a known failure. These attributes contain no external error text.
        failure = ControllerError('operation-failed' if cleaned else 'recovery-needed')
        failure.cleanup_confirmed = cleaned
        try:
            _progress(root, owner, operation_id, phase, command_cleaned=cleaned, cleanup_confirmed=cleaned)
        except BaseException:
            pass
        raise failure from None


# Linux memfd seal values (include/uapi/linux/fcntl.h: F_LINUX_SPECIFIC_BASE
# 1024 + 9 and + 10, and the seal bits), for Python builds whose fcntl module
# lacks the names although the kernel supports them, such as the standalone
# CPython builds uv installs. They apply on Linux only and only where fcntl
# has no value of its own.
_LINUX_MEMFD_SEALS = {'F_ADD_SEALS': 1033, 'F_GET_SEALS': 1034, 'F_SEAL_SEAL': 0x0001,
                      'F_SEAL_SHRINK': 0x0002, 'F_SEAL_GROW': 0x0004, 'F_SEAL_WRITE': 0x0008}


def _memfd_seal_constant(name):
    value = getattr(fcntl, name, None)
    if value is None and sys.platform.startswith('linux'):
        value = _LINUX_MEMFD_SEALS[name]
    if value is None:
        raise AttributeError(name)
    return value


def _seal_memfd(descriptor):
    """Seal a memfd against writes, size changes and further seals, and read
    the seals back: a descriptor the kernel did not seal is refused."""
    seals = (_memfd_seal_constant('F_SEAL_WRITE') | _memfd_seal_constant('F_SEAL_GROW')
             | _memfd_seal_constant('F_SEAL_SHRINK') | _memfd_seal_constant('F_SEAL_SEAL'))
    fcntl.fcntl(descriptor, _memfd_seal_constant('F_ADD_SEALS'), seals)
    if fcntl.fcntl(descriptor, _memfd_seal_constant('F_GET_SEALS')) & seals != seals:
        raise OSError('The memfd seals did not hold.')


@contextlib.contextmanager
def _input_descriptor(content, root):
    """Sealed Linux input, or an unlinked private descriptor for offline tests."""
    if hasattr(os, 'memfd_create'):
        descriptor = os.memfd_create('launch-accepted-input', os.MFD_CLOEXEC | os.MFD_ALLOW_SEALING)
        try:
            pending = memoryview(content)
            while pending:
                pending = pending[os.write(descriptor, pending):]
            os.lseek(descriptor, 0, os.SEEK_SET)
            _seal_memfd(descriptor)
            yield descriptor
        finally:
            os.close(descriptor)
    else:
        # There is no pathname for another edit to replace after this opens.
        with tempfile.TemporaryFile(dir=_directory(root)) as channel:
            os.fchmod(channel.fileno(), 0o600)
            channel.write(content)
            channel.flush()
            channel.seek(0)
            yield channel.fileno()


ACCEPTED_PYTHON = r'''import base64,hashlib,importlib.abc,importlib.util,io,json,os,pathlib,stat,subprocess,sys
fd=int(sys.argv[1])
with os.fdopen(fd,'rb') as channel:
    capsule=json.loads(channel.read(100663297))
root=pathlib.Path(capsule['root'])
files={key:base64.b64decode(value,validate=True) for key,value in capsule['files'].items()}
class AcceptedLoader(importlib.abc.Loader):
    def __init__(self,name): self.name=name
    def create_module(self,spec): return None
    def exec_module(self,module):
        module.__file__=str(root/self.name)
        exec(compile(files[self.name],module.__file__,'exec'),module.__dict__)
class AcceptedFinder(importlib.abc.MetaPathFinder):
    def find_spec(self,fullname,path=None,target=None):
        stem=fullname.replace('.','/')
        for name,package in ((stem+'/__init__.py',True),(stem+'.py',False)):
            if name in files:
                spec=importlib.util.spec_from_loader(fullname,AcceptedLoader(name),origin=str(root/name),is_package=package)
                spec.has_location=True
                if package: spec.submodule_search_locations=[str(root/stem)]
                return spec
        if (root/(stem+'.py')).exists() or (root/stem/'__init__.py').exists():
            raise ImportError('An undeclared installation module cannot be loaded.')
        return None
sys.meta_path.insert(0,AcceptedFinder())
original_spec=importlib.util.spec_from_file_location
def accepted_spec(name,location,*args,**kwargs):
    path=os.path.abspath(location)
    relative=path.removeprefix(str(root)+'/')
    if relative in files and relative.endswith('.py'):
        kwargs['loader']=AcceptedLoader(relative)
        if relative.endswith('/__init__.py'): kwargs['submodule_search_locations']=[str(pathlib.Path(path).parent)]
    elif path.startswith(str(root)+'/') and path.endswith('.py'):
        raise ImportError('An undeclared installation module cannot be loaded.')
    return original_spec(name,location,*args,**kwargs)
importlib.util.spec_from_file_location=accepted_spec
original_open=pathlib.Path.open
def current_source():
    for name,expected in files.items():
        descriptor=os.open(root/name,os.O_RDONLY|os.O_NOFOLLOW|os.O_NONBLOCK)
        with os.fdopen(descriptor,'rb') as channel:
            info=os.fstat(channel.fileno())
            if not stat.S_ISREG(info.st_mode) or info.st_uid!=os.getuid() or info.st_nlink!=1 or info.st_size>16777216:
                raise RuntimeError('Accepted installation source changed.')
            content=channel.read(16777217)
        if hashlib.sha256(content).digest()!=hashlib.sha256(expected).digest():
            raise RuntimeError('Accepted installation source changed.')
original_popen=subprocess.Popen
def accepted_popen(*args,**kwargs):
    current_source()
    return original_popen(*args,**kwargs)
subprocess.Popen=accepted_popen
def accepted_open(path,*args,**kwargs):
    mode=args[0] if args else kwargs.get('mode','r')
    try: name=os.path.abspath(path).removeprefix(str(root)+'/')
    except (ValueError,TypeError): name=None
    if name in files and 'r' in mode and not any(flag in mode for flag in '+wax'):
        raw=io.BytesIO(files[name])
        if 'b' in mode: return raw
        encoding=kwargs.get('encoding',args[2] if len(args)>2 else None)
        errors=kwargs.get('errors',args[3] if len(args)>3 else None)
        newline=kwargs.get('newline',args[4] if len(args)>4 else None)
        return io.TextIOWrapper(raw,encoding=encoding,errors=errors,newline=newline)
    return original_open(path,*args,**kwargs)
pathlib.Path.open=accepted_open
entry=capsule['entry']
sys.argv=[str(root/entry),*capsule['arguments']]
exec(compile(files[entry],sys.argv[0],'exec'),{'__name__':'__main__','__file__':sys.argv[0]})
'''


def _accepted_files(root, owner):
    name = 'accepted-source.tar.gz'
    content = _ordinary(_directory(root) / name, private=True, maximum=MAX_SOURCE + 1024 * 1024)
    if _digest(content) != owner['snapshots'].get(name):
        _fail('snapshot-changed')
    return _archive(base64.b64encode(content).decode(), owner['binding'])[1]


def _assert_current(root, owner, module):
    try:
        current = module['source_digest'](root) == owner['binding']['transferred_source_digest']
    except Exception:
        current = False
    if not current:
        _fail('source-changed')


def _accepted_python(root, owner, operation_id, files, entry, arguments, phase):
    if entry not in files:
        _fail('invalid-package')
    payload = json.dumps({'root': str(root), 'entry': entry, 'arguments': arguments,
                          'files': {name: base64.b64encode(content).decode() for name, content in files.items()}},
                         separators=(',', ':'), ensure_ascii=True).encode()
    if len(payload) > MAX_INPUT:
        _fail('invalid-package')
    with _input_descriptor(payload, root) as descriptor:
        _command([str(root / '.venv/bin/python'), '-I', '-c', ACCEPTED_PYTHON, str(descriptor)],
                 root, owner, operation_id, phase, pass_fds=(descriptor,))


ACCEPTED_DEPENDENCIES = r'''import json,os,sys
from pip._internal.cli.main import main
from pip._internal.models.link import Link
with os.fdopen(int(sys.argv[1]),'rb') as channel:
    capsule=json.loads(channel.read(100663297))
original=Link.file_path
if not isinstance(original,property) or not callable(original.fget):
    raise RuntimeError('The installed pip does not support accepted wheel descriptors.')
paths=capsule['wheel_paths']
def accepted_path(link):
    selected=paths.get(link.url_without_fragment)
    return selected if selected is not None else original.fget(link)
# Keep the accepted wheel basename in the URL for pip metadata parsing.
# Every payload read uses the sealed descriptor, never the public source path.
Link.file_path=property(accepted_path)
raise SystemExit(main(['install','--disable-pip-version-check','-r',
    '/proc/self/fd/'+str(capsule['requirements_descriptor'])]))
'''


def _dependencies(root, owner, operation_id, files, name, phase):
    # Includes, editable paths and arbitrary options can execute mutable local
    # source during pip. Only pinned packages and accepted application wheels
    # enter this host-owned phase.
    content = files[name]
    rows, wheels = [], {}
    for row in content.decode().splitlines():
        row = row.strip()
        if row and not row.startswith('#'):
            if name == 'requirements.application.txt' and re.fullmatch(
                    r'\./application/vendor/[A-Za-z0-9][A-Za-z0-9._+!-]*\.whl', row):
                path = row[2:]
                payload = files.get(path)
                if path in wheels or not isinstance(payload, bytes) or not 0 < len(payload) <= MAX_FILE:
                    _fail('invalid-package')
                wheels[path] = payload
            elif not re.fullmatch(
                    r'[A-Za-z0-9][A-Za-z0-9._-]*(?:\[[A-Za-z0-9,._-]+\])?==[A-Za-z0-9][A-Za-z0-9.!+_-]*(?:\s*;[^\r\n]*)?', row):
                _fail('invalid-package')
        rows.append(row)
    if not wheels:
        with _input_descriptor(content, root) as descriptor:
            _command([str(root / '.venv/bin/python'), '-I', '-m', 'pip', 'install',
                      '--disable-pip-version-check', '-r', '/proc/self/fd/' + str(descriptor)],
                     root, owner, operation_id, phase, pass_fds=(descriptor,))
        return
    with contextlib.ExitStack() as custody:
        paths, requirements, descriptors = {}, [], []
        for row in rows:
            path = row[2:] if row.startswith('./') else None
            if path not in wheels:
                requirements.append(row)
                continue
            descriptor = custody.enter_context(_input_descriptor(wheels[path], root))
            descriptors.append(descriptor)
            location = '/proc/self/fd/' + str(descriptor)
            url = 'file://' + location + '/' + path.rsplit('/', 1)[1]
            paths[url] = location
            requirements.append(url + '#sha256=' + _digest(wheels[path]))
        requirements_fd = custody.enter_context(_input_descriptor(('\n'.join(requirements) + '\n').encode(), root))
        capsule = json.dumps({'requirements_descriptor': requirements_fd, 'wheel_paths': paths},
                             separators=(',', ':'), ensure_ascii=True).encode()
        capsule_fd = custody.enter_context(_input_descriptor(capsule, root))
        _command([str(root / '.venv/bin/python'), '-I', '-c', ACCEPTED_DEPENDENCIES, str(capsule_fd)],
                 root, owner, operation_id, phase,
                 pass_fds=(*descriptors, requirements_fd, capsule_fd))


def _venv_ready(root, owner):
    """Repair only ordinary owned venv paths, preserving all existing data."""
    virtualenv = root / '.venv'
    version = 'python' + str(sys.version_info.major) + '.' + str(sys.version_info.minor)
    for name in ('', 'bin', 'include', 'include/' + version, 'lib', 'lib/' + version,
                 'lib/' + version + '/site-packages'):
        path = virtualenv / name
        _no_links(path)
        if path.exists():
            info = path.stat()
            if not stat.S_ISDIR(info.st_mode) or info.st_uid != os.getuid():
                _fail('unsafe-path')
    configuration = virtualenv / 'pyvenv.cfg'
    if configuration.exists() or configuration.is_symlink():
        _ordinary(configuration, maximum=32768)
    binary = virtualenv / 'bin'
    if binary.exists():
        host_python = Path(sys.executable).resolve()
        for path in binary.iterdir():
            info = path.lstat()
            if info.st_uid != os.getuid():
                _fail('unsafe-path')
            if stat.S_ISLNK(info.st_mode):
                if not re.fullmatch(r'python(?:[0-9]+(?:\.[0-9]+)?)?', path.name) or path.resolve() != host_python:
                    _fail('unsafe-path')
            elif not stat.S_ISREG(info.st_mode) or info.st_nlink != 1:
                _fail('unsafe-path')
    interpreter = binary / 'python'
    return bool(configuration.exists() and interpreter.exists()
                and _probe([str(interpreter), '-I', '-m', 'pip', '--version'], owner['host']))


def _install_dependencies(root, owner, operation_id, module):
    """Host-owned phases consume accepted code without executing mutable helpers."""
    files = _accepted_files(root, owner)
    _assert_current(root, owner, module)
    virtualenv = root / '.venv'
    if not _venv_ready(root, owner):
        _command([sys.executable, '-I', '-m', 'venv', str(virtualenv)],
                 root, owner, operation_id, 'python-environment')
    for name, phase in (('requirements.txt', 'dependencies'),
                        ('requirements.application.txt', 'application-dependencies')):
        if name in files:
            _assert_current(root, owner, module)
            _dependencies(root, owner, operation_id, files, name, phase)
            _assert_current(root, owner, module)
    _assert_current(root, owner, module)
    _accepted_python(root, owner, operation_id, files, 'install_gateway.py', [], 'native-tools')
    _assert_current(root, owner, module)
    for name in ('workspace', 'data', '.mobkit/state'):
        path = root / name
        _no_links(path)
        path.mkdir(mode=0o700, parents=True, exist_ok=True)
        if not path.is_dir() or path.stat().st_uid != os.getuid():
            _fail('unsafe-path')
    _accepted_python(root, owner, operation_id, files, 'app.py', ['--check'], 'application-source')
    _assert_current(root, owner, module)


def _verified_service_commands(root, owner, operation_id, module):
    """Guard check/manager boundaries; installed runtime stays ordinary source."""
    original = module['command']
    files = _accepted_files(root, owner)
    def verified(arguments, **kwargs):
        _assert_current(root, owner, module)
        if (len(arguments) >= 2 and arguments[0] == str(root / '.venv/bin/python')
                and arguments[1] in {'app.py', 'knowledge/manage.py'}):
            _accepted_python(root, owner, operation_id, files, arguments[1], arguments[2:], 'service-check')
            result = (0, '')
        else:
            result = original(arguments, **kwargs)
        _assert_current(root, owner, module)
        return result
    module['command'] = verified


def _knowledge_preflight(root, module, owner):
    # Inspect only exact package files and bounded Docker responses. Never let
    # a familiar Compose project name establish ownership of another database.
    files = module['source_files'](root)
    digest = hashlib.sha256()
    for path, content in sorted(files.items()):
        digest.update(path.encode() + b'\0' + hashlib.sha256(content).digest())
    if digest.hexdigest() != owner['binding']['transferred_source_digest']:
        _fail('source-changed')
    content = _ordinary(_directory(root) / 'knowledge-manage.py', private=True)
    if _digest(content) != owner['snapshots'].get('knowledge-manage.py'):
        _fail('snapshot-changed')
    helper = _module(content, root / 'knowledge/manage.py')
    compose = json.loads(files['knowledge/compose.json'])
    name = compose.get('name')
    if not isinstance(name, str) or not re.fullmatch(r'[a-z][a-z0-9-]{1,60}', name):
        _fail('knowledge-not-owned')
    probe = helper['_docker_probe']
    message = MESSAGES['knowledge-not-owned']
    identities = probe(['docker', 'ps', '-a', '--filter', 'label=com.docker.compose.project=' + name,
                        '--format', '{{.ID}}'], message).splitlines()
    if len(identities) > 100:
        _fail('knowledge-not-owned')
    mounted = set()
    for identity in identities:
        if not re.fullmatch(r'[a-f0-9]{12,64}', identity):
            _fail('knowledge-not-owned')
        container = json.loads(probe(['docker', 'inspect', '--format', '{{json .}}', identity], message))
        labels = container.get('Config', {}).get('Labels') or {}
        if (labels.get('com.docker.compose.project') != name
                or labels.get('com.docker.compose.project.working_dir') != str(root / 'knowledge')
                or labels.get('com.docker.compose.project.config_files') != str(root / 'knowledge/compose.json')):
            _fail('knowledge-not-owned')
        mounted.update(item.get('Name') for item in container.get('Mounts', []) if item.get('Type') == 'volume')
    expected = set()
    for key, options in compose.get('volumes', {}).items():
        if not re.fullmatch(r'[a-z][a-z0-9-]{0,60}', key) or options:
            _fail('knowledge-not-owned')
        expected.add(name + '_' + key)
    existing = probe(['docker', 'volume', 'ls', '--filter', 'label=com.docker.compose.project=' + name,
                      '--format', '{{.Name}}'], message).splitlines()
    if len(existing) > 100 or any(item not in expected or item not in mounted for item in existing):
        _fail('knowledge-not-owned')
    all_volumes = probe(['docker', 'volume', 'ls', '--format', '{{.Name}}'], message).splitlines()
    if any(item in expected and item not in mounted for item in all_volumes):
        _fail('knowledge-not-owned')
    for path in (root / 'knowledge/.env', root / '.env'):
        if path.exists() or path.is_symlink():
            _ordinary(path, private=True, maximum=MAX_PRIVATE)
    _no_links(root / 'knowledge/.source')


def _progress(root, owner, operation_id, phase, **updates):
    with _lock(root, owner['host']):
        job = _job(root)
        if not job or job['operation_id'] != operation_id:
            _fail('not-owned')
        job.update(phase=phase, **updates)
        _json_write(_directory(root) / 'operation.json', job)


def worker(destination, operation_id, *, private_settings_fd=None):
    """Private detached entry point; never prints package output or secrets."""
    root, host = _host(destination)
    directory = _directory(root)
    raw = _json_read(directory / 'owner.json')
    owner = _owner(root, raw['deployment_id'], _binding(raw['binding']), host)
    if _digest(_controller_source()) != owner['binding']['controller_sha256']:
        _fail('snapshot-changed')
    with _lock(root, host):
        job = _job(root)
        if not job or job['operation_id'] != operation_id or not _live(job.get('worker')) or job['worker']['pid'] != os.getpid():
            _fail('not-owned')
    action = job['action']
    service_started = False
    try:
        if owner.get('product_control') is True:
            if action != 'install':
                _fail('runtime-unsupported')
            adapter, manifest = _product_adapter()
            adapter['worker_install'](globals(), root, owner, operation_id, private_settings_fd, manifest)
            _progress(root, owner, operation_id, 'complete', status='completed', completed_at=time.time(), cleanup_confirmed=True)
            return
        module, operations = _runtime(root, owner)
        if action in {'install', 'retry-install'}:
            _install_dependencies(root, owner, operation_id, module)
            if module['source_digest'](root) != owner['binding']['transferred_source_digest']:
                _fail('source-changed')
            if operations.spec.get('knowledge', {}).get('mode') == 'local':
                _knowledge_preflight(root, module, owner)
                files = _accepted_files(root, owner)
                _assert_current(root, owner, module)
                _accepted_python(root, owner, operation_id, files, 'knowledge/manage.py', ['setup'], 'knowledge-setup')
                _assert_current(root, owner, module)
                _accepted_python(root, owner, operation_id, files, 'knowledge/manage.py', ['up'], 'knowledge-start')
                _assert_current(root, owner, module)
        _progress(root, owner, operation_id, 'service-' + action, cleanup_confirmed=False)
        service_started = True
        if action == 'stop':
            operations.stop(remove=False)
        else:
            _assert_current(root, owner, module)
            _verified_service_commands(root, owner, operation_id, module)
            operations.start(restart=action == 'restart')
        _progress(root, owner, operation_id, 'complete', status='completed', completed_at=time.time(), cleanup_confirmed=True)
    except BaseException as error:
        code = error.code if isinstance(error, ControllerError) else 'operation-failed'
        # A command without proven cleanup cannot become a terminal failure.
        current = _job(root)
        cleanup = bool(not service_started and (getattr(error, 'cleanup_confirmed', False)
                       or current and current.get('cleanup_confirmed') is True))
        status = 'failed' if cleanup else 'recovery-needed'
        failure_phase = current.get('phase') if current and current.get('phase') in PHASES else 'unknown'
        _progress(root, owner, operation_id, 'attention-required', status=status, error_code=code,
                  cleanup_confirmed=cleanup, failure_phase=failure_phase)


def handle(request, *, product_stream=None):
    """Return one bounded public result, with exact validated request authority."""
    base = {'protocol': 1, 'ready': False, 'retryable': False, 'private_settings_supported': True}
    try:
        if not isinstance(request, dict) or request.get('action') not in {
                'preflight', 'install', 'retry-install', 'status', 'stop', 'restart', 'settings-options', 'configure', 'product-control',
                'application-setup', 'prepare-setup', 'finish-setup', 'start-background'}:
            _fail('invalid-request')
        identity, binding = _uuid(request.get('deployment_id')), _binding(request.get('binding'))
        base.update(deployment_id=identity, binding=binding)
        root, host = _host(request.get('destination'))
        base['host'] = host
        source = _controller_source()
        if _digest(source) != binding['controller_sha256']:
            _fail('snapshot-changed')
        action = request['action']
        if action == 'preflight':
            result = _preflight(request, root, identity, binding, host)
            if globals().get('PRODUCT_CONTROL_MANIFEST') is not None:
                adapter, manifest = _product_adapter()
                try:
                    result['product_interpreter'] = adapter['guest_interpreter'](manifest)
                except ValueError:
                    # Name what the selected product needs as a failed check,
                    # before any transfer, rather than as an internal error.
                    result['checks'].append(_product_interpreter_check(manifest))
                    result['ready'] = False
                result['private_settings_supported'] = False
        else:
            _expected(request, host)
            if action in {'install', 'retry-install', 'stop', 'restart'}:
                _uuid(request.get('operation_id'))
            if action == 'product-control':
                adapter, manifest = _product_adapter()
                with _lock(root, host):
                    result = adapter['receive_bundle'](globals(), root, identity, binding, host, source,
                        manifest, request, product_stream)
            elif action == 'install' and globals().get('PRODUCT_CONTROL_MANIFEST') is not None:
                result = _product_install(request, root, identity, binding, host)
            elif action == 'install':
                result = _install(request, root, identity, binding, host, source)
            elif globals().get('PRODUCT_CONTROL_MANIFEST') is not None:
                adapter, manifest = _product_adapter()
                with _lock(root, host):
                    owner = _owner(root, identity, binding, host, required=True)
                    endpoint = _directory(root) / 'product-control-owner.json'
                    if action == 'status' and not endpoint.exists() and not endpoint.is_symlink():
                        result = adapter['observe'](globals(), root, owner, manifest)
                    else:
                        result = adapter['request_control'](globals(), root, owner, manifest, request)
            elif action in {'application-setup', 'prepare-setup', 'finish-setup', 'start-background'}:
                _fail('runtime-unsupported')
            elif action == 'retry-install':
                result = _retry_install(request, root, identity, binding, host)
            elif action in {'settings-options', 'configure'}:
                result = _installed_settings(request, root, identity, binding, host)
            else:
                with _lock(root, host):
                    owner = _owner(root, identity, binding, host, required=action != 'status')
                    if owner is None:
                        _folder(root, identity, binding, host)
                        result = {'status': 'not-installed', 'ready': False, 'admitted': False,
                                  'worker_pid': None, 'worker_live': False, 'source_current': False,
                                  'service': {'status': 'not-installed', 'ready': False},
                                  'service_owned': False, 'service_observed': False}
                    else:
                        result = _observe(root, owner) if action == 'status' else _admit(root, owner, action, request['operation_id'])
                        if action == 'status':
                            result['application_experiences'] = _application_experiences(root, owner, result)
        base.update(result)
    except ControllerError as error:
        base['error'] = {'code': error.code, 'message': MESSAGES[error.code]}
        if error.fields:
            base['error']['fields'] = error.fields
    except Exception:
        base['error'] = {'code': 'internal-error', 'message': MESSAGES['internal-error']}
    if len(json.dumps(base, ensure_ascii=True).encode()) > MAX_RESULT:
        return {key: value for key, value in {**base, 'ready': False,
            'error': {'code': 'internal-error', 'message': MESSAGES['internal-error']}}.items()
            if key in {'protocol', 'deployment_id', 'binding', 'host', 'ready', 'error'}}
    return base


def main():
    try:
        if globals().get('PRODUCT_CONTROL_MANIFEST') is not None:
            adapter, _ = _product_adapter()
            request, stream = adapter['read_request'](sys.stdin.buffer, MAX_INPUT)
            with open(os.devnull, 'w') as sink, contextlib.redirect_stdout(sink), contextlib.redirect_stderr(sink):
                result = handle(request, product_stream=stream)
            print(FRAME_PREFIX + json.dumps(result, separators=(',', ':'), ensure_ascii=True), flush=True)
            return
        payload = sys.stdin.buffer.read(MAX_INPUT + 1)
        if len(payload) > MAX_INPUT:
            _fail('invalid-request')
        # Duplicate JSON keys are invalid rather than last-write-wins authority.
        def unique(pairs):
            result = {}
            for key, value in pairs:
                if key in result:
                    _fail('invalid-request')
                result[key] = value
            return result
        request = json.loads(payload, object_pairs_hook=unique)
        with open(os.devnull, 'w') as sink, contextlib.redirect_stdout(sink), contextlib.redirect_stderr(sink):
            result = handle(request)
    except Exception:
        result = {'protocol': 1, 'ready': False,
                  'error': {'code': 'invalid-request', 'message': MESSAGES['invalid-request']}}
    print(FRAME_PREFIX + json.dumps(result, separators=(',', ':'), ensure_ascii=True), flush=True)


if __name__ == '__main__':
    main()
