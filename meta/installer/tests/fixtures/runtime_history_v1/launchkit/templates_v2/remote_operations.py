#!/usr/bin/env python3
"""Destination-owned Linux service control and verified package transfer.

Private values are entered in the destination's terminal. Service readiness
means native bootstrap and Console access, not successful external workflows.
"""
from __future__ import annotations

import argparse
import asyncio
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import re
import selectors
import signal
import stat
import subprocess
import sys
import tarfile
import time
from urllib.request import ProxyHandler, Request, build_opener

ROOT = Path(__file__).resolve().parent
START_TIMEOUT = 240
# The compiler fills these from the host's pinned gateway shutdown budget.
# They are bounded grace/reap allowances, never native cleanup acknowledgments.
SHUTDOWN_BUDGET = '__LAUNCH_SHUTDOWN_BUDGET__'
STOP_TIMEOUT = SHUTDOWN_BUDGET['application']
FORCE_REAP_TIMEOUT = SHUTDOWN_BUDGET['force_reap']
WORKER_STOP_TIMEOUT = SHUTDOWN_BUDGET['worker']
SERVICE_STOP_TIMEOUT = SHUTDOWN_BUDGET['service']
SERVICE_STOP_COMMAND_TIMEOUT = SHUTDOWN_BUDGET['command']
LEGACY_SERVICE_STOP_TIMEOUT = SHUTDOWN_BUDGET['legacy_service']
SERVICE_RESTART_COMMAND_TIMEOUT = SERVICE_STOP_COMMAND_TIMEOUT + START_TIMEOUT + 10
EXIT_OBSERVATION_TIMEOUT = 1
MAX_FILE = 16 * 1024 * 1024
MAX_SOURCE = 64 * 1024 * 1024
MAX_READY = 2 * 1024 * 1024
PRIVATE_COMPONENTS = {'.env', '.venv', '.bin', '.mobkit', '.git', '__pycache__', '.source'}
PRIVATE_ROOTS = {'data', 'workspace', 'source'}


class DestinationError(ValueError):
    """A host-authored safe message, never an external error response."""


def excluded_source(name):
    parts = name.split('/')
    return bool(PRIVATE_COMPONENTS.intersection(parts) or parts[0] in PRIVATE_ROOTS
                or name.endswith('.receipt.json'))


def no_links(path):
    if any(item.is_symlink() for item in (path, *path.parents)):
        raise DestinationError('The installation path cannot contain symbolic links.')


def ordinary(path, *, private=False, maximum=MAX_FILE):
    no_links(path)
    descriptor = os.open(path, os.O_RDONLY | os.O_NOFOLLOW)
    with os.fdopen(descriptor, 'rb') as channel:
        info = os.fstat(channel.fileno())
        if (not stat.S_ISREG(info.st_mode) or info.st_uid != os.getuid()
                or info.st_nlink != 1 or info.st_size > maximum
                or (private and info.st_mode & 0o077)):
            raise DestinationError('An installation file has unsafe ownership, permissions or size.')
        return channel.read(maximum + 1)


def write_private(path, value, *, exclusive=False):
    no_links(path)
    path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
    if path.parent.stat().st_uid != os.getuid() or path.parent.stat().st_mode & 0o077:
        raise DestinationError('Service records need a private directory owned by this user.')
    payload = json.dumps(value, sort_keys=True, indent=2).encode() + b'\n'
    import uuid
    temporary = path.with_name(path.name + '.' + uuid.uuid4().hex + '.pending')
    descriptor = os.open(temporary, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
    try:
        with os.fdopen(descriptor, 'wb') as channel:
            channel.write(payload)
            channel.flush()
            os.fsync(channel.fileno())
        if exclusive:
            os.link(temporary, path, follow_symlinks=False)
            temporary.unlink()
        else:
            if path.exists():
                ordinary(path, private=True)
            temporary.replace(path)
    finally:
        temporary.unlink(missing_ok=True)


def source_files(root=ROOT):
    """Read the accepted source manifest; never sweep runtime or private files."""
    rows = ordinary(root / 'SHA256SUMS', maximum=512 * 1024).decode('utf-8').splitlines()
    files, seen, total = {}, set(), 0
    if not rows or len(rows) > 2048:
        raise DestinationError('The package source manifest is missing or too large.')
    for row in rows:
        match = re.fullmatch(r'([a-f0-9]{64})  (.+)', row)
        if not match:
            raise DestinationError('The package source manifest is invalid.')
        expected, name = match.groups()
        parts = name.split('/')
        if (PurePosixPath(name).is_absolute() or '\\' in name
                or any(part in {'', '.', '..'} for part in parts)
                or any(ord(char) < 32 for char in name) or name in seen):
            raise DestinationError('The source manifest contains an unsafe or duplicate path.')
        seen.add(name)
        if excluded_source(name):
            continue
        content = ordinary(root / name)
        if hashlib.sha256(content).hexdigest() != expected:
            raise DestinationError('Package source changed. Use the exact accepted package before installing or resuming.')
        total += len(content)
        if total > MAX_SOURCE:
            raise DestinationError('The package exceeded its source size bound.')
        files[name] = content
    if not {'deployment.json', 'app.py', 'remote_operations.py'}.issubset(files):
        raise DestinationError('Rebuild this package with the current Launch installer.')
    return files


def source_digest(root=ROOT):
    digest = hashlib.sha256()
    for name, content in sorted(source_files(root).items()):
        digest.update(name.encode() + b'\0' + hashlib.sha256(content).digest())
    return digest.hexdigest()


def archive_package(destination, root=ROOT):
    files = source_files(root)
    # No source links, undeclared assets or private files can enter the upload.
    with tarfile.open(destination, 'w:gz') as archive:
        import io
        manifest = ''.join(hashlib.sha256(content).hexdigest() + '  ' + name + '\n'
                           for name, content in sorted(files.items())).encode()
        for name, content in sorted({**files, 'SHA256SUMS': manifest}.items()):
            info = tarfile.TarInfo(name)
            info.size, info.mode, info.mtime = len(content), stat.S_IMODE((root / name).stat().st_mode), 0
            archive.addfile(info, io.BytesIO(content))


def receive_archive(archive_path, destination, identity, expected_digest):
    """Resume only an exact owned folder, preserving every existing source byte."""
    root = Path(destination).absolute()
    no_links(root)
    if not re.fullmatch(r'[a-f0-9]{32}', identity) or not re.fullmatch(r'[a-f0-9]{64}', expected_digest):
        raise DestinationError('Invalid cloud installation ownership binding.')
    ordinary(Path(archive_path), maximum=MAX_SOURCE + 1024 * 1024)
    files, total = {}, 0
    with tarfile.open(archive_path) as archive:
        for member in archive:
            name = member.name
            parts = name.split('/')
            if (len(files) >= 2049 or not member.isfile() or member.size > MAX_FILE
                    or PurePosixPath(name).is_absolute() or '\\' in name
                    or any(part in {'', '.', '..'} for part in parts)
                    or any(ord(char) < 32 for char in name) or name in files or excluded_source(name)):
                raise DestinationError('The uploaded package contains an unsafe source path or file.')
            content = archive.extractfile(member).read(MAX_FILE + 1)
            total += len(content)
            if total > MAX_SOURCE:
                raise DestinationError('The uploaded package exceeded its source size bound.')
            files[name] = (content, 0o755 if member.mode & 0o111 else 0o644)
    manifest = files.get('SHA256SUMS', (b'',))[0].decode('utf-8')
    names, digest = set(), hashlib.sha256()
    for row in sorted(manifest.splitlines(), key=lambda row: row.split('  ', 1)[-1]):
        match = re.fullmatch(r'([a-f0-9]{64})  (.+)', row)
        if not match:
            raise DestinationError('The uploaded source manifest is invalid.')
        expected, name = match.groups()
        if name in names or name == 'SHA256SUMS' or name not in files or hashlib.sha256(files[name][0]).hexdigest() != expected:
            raise DestinationError('The uploaded source does not match its accepted manifest.')
        names.add(name)
        digest.update(name.encode() + b'\0' + bytes.fromhex(expected))
    if names != set(files) - {'SHA256SUMS'} or digest.hexdigest() != expected_digest:
        raise DestinationError('The uploaded source differs from this cloud installation.')
    owner_path = root / '.mobkit/gcp-package-owner.json'
    binding = {'id': identity, 'root': str(root), 'source_digest': expected_digest}
    if owner_path.exists() or owner_path.is_symlink():
        if json.loads(ordinary(owner_path, private=True, maximum=16 * 1024)) != binding:
            raise DestinationError('This destination folder belongs to another installation. Nothing was replaced.')
    else:
        if root.exists() and any(root.iterdir()):
            raise DestinationError('The destination folder is not empty or owned by this installation. Nothing was replaced.')
        root.mkdir(parents=True, exist_ok=True, mode=0o700)
    if root.stat().st_uid != os.getuid():
        raise DestinationError('The destination folder is not owned by this user.')
    # Validate every existing file before making any source changes. A partial
    # prior transfer can have missing files; changed existing files are refused.
    for name, (content, _) in files.items():
        path = root / name
        no_links(path)
        if path.exists() and ordinary(path) != content:
            raise DestinationError('Installed source changed. Resume preserves it; use the original owned source or a new installation.')
    if not owner_path.exists():
        write_private(owner_path, binding, exclusive=True)
    for name, (content, mode) in files.items():
        path = root / name
        if path.exists():
            continue
        no_links(path)
        path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
        import uuid
        temporary = owner_path.parent / (uuid.uuid4().hex + '.transfer-pending')
        descriptor = os.open(temporary, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, mode)
        try:
            with os.fdopen(descriptor, 'wb') as channel:
                channel.write(content)
                channel.flush()
                os.fsync(channel.fileno())
            os.link(temporary, path, follow_symlinks=False)
        finally:
            temporary.unlink(missing_ok=True)
    if source_digest(root) != expected_digest:
        raise DestinationError('Transferred source could not be verified. Preserve this folder and retry the exact package.')
    return binding


def environment():
    allowed = {'HOME', 'USER', 'LOGNAME', 'LANG', 'LC_ALL', 'TMPDIR', 'SHELL',
               'SSL_CERT_FILE', 'SSL_CERT_DIR'}
    values = {key: value for key, value in os.environ.items() if key in allowed}
    values.update(PATH=str(Path.home() / '.local/bin') + ':/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin',
                  PYTHONUNBUFFERED='1', PYTHONNOUSERSITE='1',
                  XDG_RUNTIME_DIR='/run/user/' + str(os.getuid()),
                  DBUS_SESSION_BUS_ADDRESS='unix:path=/run/user/' + str(os.getuid()) + '/bus')
    return values


def command(arguments, *, timeout=60, allow_failure=False, terminal=False, root=ROOT):
    """Bounded process ownership; timeout kills and reaps the exact process group."""
    process = subprocess.Popen(arguments, cwd=root, env=environment(), start_new_session=True,
        stdin=None if terminal else subprocess.DEVNULL,
        stdout=None if terminal else subprocess.PIPE, stderr=None if terminal else subprocess.STDOUT)
    output = bytearray()
    try:
        if terminal:
            process.wait(timeout=timeout)
        else:
            deadline = time.monotonic() + timeout
            with selectors.DefaultSelector() as selector:
                selector.register(process.stdout, selectors.EVENT_READ)
                while selector.get_map():
                    remaining = deadline - time.monotonic()
                    if remaining <= 0:
                        raise TimeoutError('The destination command reached its time limit.')
                    for key, _ in selector.select(min(remaining, 1)):
                        chunk = os.read(key.fileobj.fileno(), 8192)
                        if not chunk:
                            selector.unregister(key.fileobj)
                        else:
                            output.extend(chunk)
                            if len(output) > 128 * 1024:
                                raise DestinationError('A destination operation exceeded its output bound.')
                process.wait(timeout=max(0.01, deadline - time.monotonic()))
    except BaseException:
        # The leader can exit while its descendants still hold captured output.
        # Signal before polling/reaping, but an interrupted wait can already
        # have reaped the leader. Its cached returncode then forbids signaling.
        if process.returncode is None:
            signal_group(process, signal.SIGKILL)
        process.wait(timeout=FORCE_REAP_TIMEOUT)
        raise
    finally:
        if process.stdout:
            process.stdout.close()
    if process.returncode and not allow_failure:
        # Application check output can contain credentials; do not echo it.
        raise DestinationError('The destination operation failed. Check prerequisites and private configuration, then retry.')
    return process.returncode, output.decode(errors='replace')


def signal_group(process, selected):
    try:
        os.killpg(process.pid, selected)
    except ProcessLookupError:
        # An exit between observation and signaling is still reaped by wait().
        pass


async def signal_supervised_group(process, selected):
    try:
        signal_group(process, selected)
    except PermissionError:
        # Pipe EOF can precede the asyncio child watcher. Some kernels report
        # EPERM for that already-exited group until its child is reaped. Only
        # ignore this race after observing our child's exit and an absent group.
        deadline = asyncio.get_running_loop().time() + EXIT_OBSERVATION_TIMEOUT
        while process.returncode is None and asyncio.get_running_loop().time() < deadline:
            await asyncio.sleep(0.01)
        if process.returncode is None:
            raise
        try:
            os.killpg(process.pid, 0)
        except ProcessLookupError:
            return
        # An existing or inaccessible group is a real cleanup failure.
        raise


def supervised_group_exists(process):
    try:
        os.killpg(process.pid, 0)
    except ProcessLookupError:
        return False
    # Permission failure cannot establish that our owned group is absent.
    return True


async def supervised_parent_exit(process):
    # Inherited output can keep Process.wait() pending after the application
    # exits. Observe its owned child watcher before cleaning up descendants.
    while process.returncode is None:
        await asyncio.sleep(0.01)


async def stop_supervised_application(process):
    if process.returncode is None:
        try:
            # The SDK's gateway inherits this group. Let the application keep
            # its callback pipes open until its native cleanup acknowledges.
            process.terminate()
        except ProcessLookupError:
            pass
        except PermissionError:
            # Some kernels report a just-exited process before the asyncio
            # watcher publishes its result. A live inaccessible owner fails.
            try:
                await asyncio.wait_for(supervised_parent_exit(process), EXIT_OBSERVATION_TIMEOUT)
            except TimeoutError:
                pass
            if process.returncode is None:
                raise
        try:
            await asyncio.wait_for(supervised_parent_exit(process), STOP_TIMEOUT)
        except TimeoutError:
            pass
    # Parent exit is separate from owned group cleanup. Escalate only after
    # its graceful window, including descendants that still hold stdout.
    if supervised_group_exists(process):
        await signal_supervised_group(process, signal.SIGTERM)
        await asyncio.sleep(0.2)
        if supervised_group_exists(process):
            await signal_supervised_group(process, signal.SIGKILL)

    async def confirm_cleanup():
        await process.wait()
        while supervised_group_exists(process):
            await asyncio.sleep(0.01)
    await asyncio.wait_for(confirm_cleanup(), FORCE_REAP_TIMEOUT)


def definition(root=ROOT):
    value = json.loads(ordinary(root / 'deployment.json'))
    if value.get('schemaVersion') != 2:
        raise DestinationError('Verified background services require a current schemaVersion 2 package with native startup reporting. Rebuild in Launch.')
    if not re.fullmatch(r'[a-z][a-z0-9-]{1,39}', value.get('slug', '')):
        raise DestinationError('The package has an invalid project identity.')
    return value


def private_settings(root=ROOT):
    values = {}
    for row in ordinary(root / '.env', private=True, maximum=256 * 1024).decode().splitlines():
        if not row or row.startswith('#'):
            continue
        key, separator, raw = row.partition('=')
        if not separator or not re.fullmatch(r'[A-Z][A-Z0-9_]*', key):
            raise DestinationError('The private configuration is invalid. Run destination setup again.')
        raw = raw.strip()
        value = json.loads(raw) if raw.startswith('"') else raw[1:-1] if raw.startswith("'") and raw.endswith("'") else raw
        if not isinstance(value, str) or len(value) > 32768 or '\0' in value:
            raise DestinationError('The private configuration contains an invalid value.')
        values[key] = value
    return values


def service_stop_timeout(receipt):
    value = receipt.get('stop_timeout_seconds', LEGACY_SERVICE_STOP_TIMEOUT)
    if type(value) is not int or value not in {LEGACY_SERVICE_STOP_TIMEOUT, SERVICE_STOP_TIMEOUT}:
        raise DestinationError('The service receipt has an unsupported shutdown budget. No service was controlled.')
    return value


def unit_content(receipt, *, legacy_directory=False):
    stop_timeout = service_stop_timeout(receipt)
    def quote(value, *, expand_dollars=True):
        value = str(value).replace('\\', '\\\\').replace('"', '\\"').replace('%', '%%')
        return '"' + (value.replace('$', '$$') if expand_dollars else value) + '"'
    root = Path(receipt['root'])
    path = str(root)
    if (not root.is_absolute() or '..' in root.parts or path != path.strip() or path.endswith('\\')
            or any(ord(char) < 32 or ord(char) == 127 for char in path)):
        raise DestinationError('The service directory must be an absolute path without control characters or trailing whitespace/backslash.')
    # WorkingDirectory is a single path directive, not an ExecStart word list.
    # systemd preserves quotation marks here; only percent specifiers expand.
    directory = quote(root) if legacy_directory else path.replace('%', '%%')
    args = [root / '.venv/bin/python', '-I', root / 'remote_operations.py', 'supervise']
    # The executable token has no environment expansion. Other command words
    # retain dollar escaping so application paths cannot become variables.
    executable = ' '.join(quote(value, expand_dollars=(index != 0 or legacy_directory))
                          for index, value in enumerate(args))
    return ('[Unit]\nDescription=MobKit destination-owned agent system\nAfter=network-online.target\nWants=network-online.target\n\n'
        '[Service]\nType=simple\nWorkingDirectory=' + directory + '\nExecStart=' + executable + '\n'
        'Restart=on-failure\nRestartSec=30\nTimeoutStopSec=' + str(stop_timeout) + '\nKillMode=mixed\nUMask=0077\nNoNewPrivileges=true\n\n'
        '[Install]\nWantedBy=default.target\n').encode()


class Operations:
    def __init__(self, root=ROOT, home=None):
        self.root = Path(root).absolute()
        self.home = Path(home) if home is not None else Path.home()
        no_links(self.root)
        self.spec = definition(self.root)
        self.path = self.root / '.mobkit/remote-service.json'
        self.health_path = self.root / '.mobkit/remote-service-health.json'

    def manager(self, arguments, **kwargs):
        return command(['/usr/bin/systemctl', '--user', *arguments], root=self.root, **kwargs)

    def load(self, *, require_source=True, allow_legacy_directory=False):
        receipt = json.loads(ordinary(self.path, private=True, maximum=16 * 1024))
        import uuid
        if str(uuid.UUID(receipt.get('id', ''))) != receipt['id']:
            raise DestinationError('The service ownership receipt has an invalid identity.')
        unit = self.home / '.config/systemd/user' / ('mobkit-launch.' + receipt['id'] + '.service')
        if (receipt.get('root') != str(self.root) or receipt.get('slug') != self.spec['slug']
                or receipt.get('unit_path') != str(unit)
                or (require_source and receipt.get('source_digest') != source_digest(self.root))):
            raise DestinationError('The service receipt does not match this package and destination. No service was controlled.')
        content = ordinary(unit, private=True, maximum=16 * 1024)
        known_content = content == unit_content(receipt)
        if allow_legacy_directory and not known_content:
            known_content = content == unit_content(receipt, legacy_directory=True)
        if not known_content or hashlib.sha256(content).hexdigest() != receipt.get('unit_sha256'):
            raise DestinationError('The owned service definition changed. No existing unit was replaced or controlled.')
        return receipt

    def prepare(self):
        if self.path.exists() or self.path.is_symlink():
            return self.load()
        import uuid
        identity = str(uuid.uuid4())
        unit = self.home / '.config/systemd/user' / ('mobkit-launch.' + identity + '.service')
        no_links(unit)
        unit.parent.mkdir(parents=True, mode=0o700, exist_ok=True)
        receipt = {'format': 1, 'id': identity, 'root': str(self.root), 'slug': self.spec['slug'],
                   'unit_path': str(unit), 'source_digest': source_digest(self.root),
                   'stop_timeout_seconds': SERVICE_STOP_TIMEOUT}
        content = unit_content(receipt)
        receipt['unit_sha256'] = hashlib.sha256(content).hexdigest()
        descriptor = os.open(unit, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
        with os.fdopen(descriptor, 'wb') as channel:
            channel.write(content)
        try:
            write_private(self.path, receipt, exclusive=True)
        except BaseException:
            # Only the exact new ordinary file is ours to remove.
            if ordinary(unit, private=True) == content:
                unit.unlink()
            raise
        return self.load()

    def inspect(self, receipt):
        _, output = self.manager(['show', Path(receipt['unit_path']).name, '--no-pager',
            '--property=LoadState,ActiveState,SubState,MainPID,ControlPID,ControlGroup,ExecMainStatus,FragmentPath'])
        values = dict(row.split('=', 1) for row in output.splitlines() if '=' in row)
        if values.get('FragmentPath') != receipt['unit_path']:
            raise DestinationError('The operating-system manager did not confirm the owned unit definition.')
        return values

    def status(self):
        if not self.path.exists():
            return {'status': 'not-installed', 'ready': False, 'scope': 'native-bootstrap-and-console'}
        receipt = self.load(require_source=False, allow_legacy_directory=True)
        observed = self.inspect(receipt)
        result = {'status': observed.get('ActiveState', 'unknown'), 'substate': observed.get('SubState'),
            'ready': False, 'definition_verified': True, 'load_state': observed.get('LoadState'),
            'service': Path(receipt['unit_path']).name,
            'scope': 'native-bootstrap-and-console', 'source_digest': receipt['source_digest'],
            'custom_interfaces_verified': False, 'external_workflows_verified': False}
        try:
            result['source_current'] = source_digest(self.root) == receipt['source_digest']
        except (OSError, ValueError):
            result['source_current'] = False
        if not result['source_current']:
            return result
        try:
            health = json.loads(ordinary(self.health_path, private=True, maximum=MAX_READY))
            pid = int(observed.get('MainPID', '0'))
            if (observed.get('ActiveState') != 'active' or pid <= 0
                    or health.get('supervisor_pid') != pid or health.get('source_digest') != receipt['source_digest']
                    or health.get('ready') is not True or not 0 <= time.time() - health.get('observed_at', 0) <= 15):
                return result
            if console_healthy(self.root, self.spec, health['console_url']):
                result.update(status='running', ready=True, console_url=health['console_url'], bootstrap=health['bootstrap'])
        except (OSError, ValueError, KeyError, TypeError, RuntimeError):
            pass
        return result

    def checks(self):
        command([str(self.root / '.venv/bin/python'), 'app.py', '--doctor'], root=self.root, timeout=180)
        command([str(self.root / '.venv/bin/python'), 'app.py', '--application-check'], root=self.root, timeout=180)
        knowledge = self.spec.get('knowledge') or {}
        if knowledge.get('mode') != 'none' and (self.root / 'knowledge/manage.py').is_file():
            command([str(self.root / '.venv/bin/python'), 'knowledge/manage.py', 'verify'], root=self.root, timeout=180)
        if any(item['tools'].get('mcp') or item['tools'].get('knowledge') for item in self.spec['classes']):
            command([str(self.root / '.venv/bin/python'), 'app.py', '--connections-check'], root=self.root, timeout=180)

    def start(self, *, restart=False):
        # Verify an existing receipt before executing checks or touching a unit.
        if self.path.exists() or self.path.is_symlink():
            self.load()
        self.checks()
        receipt = self.prepare()
        self.manager(['daemon-reload'])
        self.manager(['enable', Path(receipt['unit_path']).name])
        self.manager(['restart' if restart else 'start', Path(receipt['unit_path']).name],
                     **({'timeout': SERVICE_RESTART_COMMAND_TIMEOUT} if restart else {}))
        deadline = time.monotonic() + START_TIMEOUT + 10
        while time.monotonic() < deadline:
            result = self.status()
            if result['ready']:
                return result
            if result['status'] == 'failed':
                raise DestinationError('The owned service failed. Private settings and native state are preserved; use status and restart after fixing prerequisites.')
            time.sleep(1)
        raise DestinationError('The service has not reported verified native readiness. Its owned unit remains available for status or stop.')

    def stop(self, *, remove=False):
        receipt = self.load(require_source=False, allow_legacy_directory=True)
        before = self.inspect(receipt)
        stopped, _ = self.manager(['stop', Path(receipt['unit_path']).name], allow_failure=True,
                                  timeout=SERVICE_STOP_COMMAND_TIMEOUT)
        observed = self.inspect(receipt)
        def invalid_and_inactive(value):
            return (value.get('LoadState') == 'bad-setting' and value.get('ActiveState') == 'inactive'
                    and value.get('MainPID') == '0' and value.get('ControlPID') == '0'
                    and value.get('ControlGroup') == '')
        # systemd cannot stop a syntactically invalid unit which never started.
        # A stop error is recoverable only with exact ownership and two explicit
        # manager observations proving this narrow inactive state, not a live
        # or inaccessible service. All other stop failures remain failures.
        if stopped and not (invalid_and_inactive(before) and invalid_and_inactive(observed)):
            raise DestinationError('The owned service stop failed. Its definition and private state were preserved.')
        if (observed.get('ActiveState') not in {'inactive', 'failed'}
                or int(observed.get('MainPID', '0')) or int(observed.get('ControlPID', '0'))):
            raise DestinationError('The service manager has not confirmed the owned process stopped.')
        if remove:
            self.manager(['disable', Path(receipt['unit_path']).name])
            if self.load(require_source=False, allow_legacy_directory=True) != receipt:
                raise DestinationError('The owned service receipt changed. Its definition was preserved.')
            latest = self.inspect(receipt)
            if ((stopped and not invalid_and_inactive(latest))
                    or latest.get('ActiveState') not in {'inactive', 'failed'}
                    or int(latest.get('MainPID', '0')) or int(latest.get('ControlPID', '0'))):
                raise DestinationError('The owned service is no longer stopped. Its definition was preserved.')
            # Recheck authority and manager state immediately before deletion.
            if self.load(require_source=False, allow_legacy_directory=True) != receipt:
                raise DestinationError('The owned service receipt changed. Its definition was preserved.')
            Path(receipt['unit_path']).unlink()
            self.path.unlink()
            self.manager(['daemon-reload'])
            return {'status': 'service-removed', 'ready': False, 'files_and_private_state_preserved': True}
        return self.status()


def console_healthy(root, spec, url):
    settings = private_settings(root)
    port = int(settings.get('MOBKIT_PORT') or spec['port'])
    if not 1024 <= port <= 65535 or url != f'http://127.0.0.1:{port}/console':
        raise DestinationError('The service Console URL does not match private destination configuration.')
    headers = {}
    if (root / 'application/console.json').exists():
        # Imported host-owned helper only; no custom application module is loaded.
        sys.path.insert(0, str(root))
        try:
            from console_auth import health_headers
            headers = health_headers(root)
        finally:
            sys.path.pop(0)
    opener = build_opener(ProxyHandler({}))
    with opener.open(Request(url.removesuffix('/console') + '/console/identities', headers=headers), timeout=3) as response:
        payload = response.read(MAX_READY + 1)
    if len(payload) > MAX_READY:
        raise DestinationError('The native Console health response is too large.')
    actual = {item['identity'] for item in json.loads(payload)['identities']}
    return {item['identity'] for item in spec['agents']}.issubset(actual)


def startup(payload, spec, pid, port):
    bootstrap = payload.get('bootstrap') or {}
    if (payload.get('pid') != pid or bootstrap.get('complete') is not True or bootstrap.get('ready') is not True
            or bootstrap.get('counts', {}).get('broken') != 0
            or not {item['identity'] for item in spec['agents']}.issubset(bootstrap.get('identities', {}))
            or payload.get('console_url') != f'http://127.0.0.1:{port}/console'):
        raise DestinationError('The owned application did not report exact native bootstrap and loopback Console readiness.')
    return bootstrap


async def supervise(operations):
    receipt = operations.load()
    settings = private_settings(operations.root)
    port = int(settings.get('MOBKIT_PORT') or operations.spec['port'])
    stop = asyncio.Event()
    loop = asyncio.get_running_loop()
    for selected in (signal.SIGINT, signal.SIGTERM):
        loop.add_signal_handler(selected, stop.set)
    # Never inherit provider credentials; the application reads its private .env.
    read_fd, write_fd = os.pipe()
    reader = asyncio.StreamReader(limit=MAX_READY)
    pipe = os.fdopen(read_fd, 'rb', buffering=0)
    transport, _ = await loop.connect_read_pipe(lambda: asyncio.StreamReaderProtocol(reader), pipe)
    process = None
    cleanup_confirmed = False
    tasks = []
    health = {'supervisor_pid': os.getpid(), 'source_digest': receipt['source_digest'], 'ready': False}
    def report(**values):
        health.update(values, observed_at=time.time())
        write_private(operations.health_path, health)
    try:
        report()
        process = await asyncio.create_subprocess_exec(str(operations.root / '.venv/bin/python'), 'app.py',
            cwd=operations.root, env={**environment(), 'MOBKIT_LAUNCH_READY_FD': str(write_fd)},
            pass_fds=(write_fd,), stdout=asyncio.subprocess.PIPE, stderr=asyncio.subprocess.STDOUT,
            start_new_session=True)
        os.close(write_fd)
        write_fd = None
        async def output():
            pending = b''
            while chunk := await process.stdout.read(4096):
                pending += chunk
                while b'\n' in pending or len(pending) >= 8192:
                    if b'\n' in pending:
                        row, pending = pending.split(b'\n', 1)
                    else:
                        row, pending = pending[:8192], pending[8192:]
                    text = row.decode(errors='replace')
                    for value in sorted({value for value in settings.values() if value}, key=len, reverse=True):
                        text = text.replace(value, '[private]')
                    text = re.sub(r'(?i)(bearer\s+)[^\s"\']+', r'\1[private]', text)
                    print(text, flush=True)
        output_task = asyncio.create_task(output())
        exited, stopped, ready = asyncio.create_task(process.wait()), asyncio.create_task(stop.wait()), asyncio.create_task(reader.readline())
        tasks = [output_task, exited, stopped, ready]
        done, _ = await asyncio.wait({exited, stopped, ready}, timeout=START_TIMEOUT, return_when=asyncio.FIRST_COMPLETED)
        if stopped in done:
            return
        if ready not in done or exited in done or not ready.result():
            raise DestinationError('Native startup did not report readiness before exit or the time limit.')
        payload = json.loads(ready.result())
        bootstrap = startup(payload, operations.spec, process.pid, port)
        while not stop.is_set() and process.returncode is None:
            try:
                healthy = await asyncio.to_thread(console_healthy, operations.root, operations.spec, payload['console_url'])
            except Exception:
                healthy = False
            report(ready=healthy, bootstrap=bootstrap, console_url=payload['console_url'], runtime_pid=process.pid)
            done, _ = await asyncio.wait({exited, stopped}, timeout=3, return_when=asyncio.FIRST_COMPLETED)
            if done:
                break
        if not stop.is_set():
            raise DestinationError('The application exited. The service manager owns recovery; private state is preserved.')
    finally:
        try:
            if write_fd is not None:
                os.close(write_fd)
            transport.close()
            if process:
                await stop_supervised_application(process)
                cleanup_confirmed = True
        finally:
            for task in tasks:
                task.cancel()
            try:
                await asyncio.gather(*tasks, return_exceptions=True)
            finally:
                try:
                    # A failed termination still owns a possibly live child.
                    # Do not erase its PID or imply it was stopped/reaped.
                    report(ready=False, runtime_pid=process.pid if process and process.returncode is None else None,
                        cleanup_confirmed=not process or cleanup_confirmed,
                        owned_process_group=process.pid if process and not cleanup_confirmed else None)
                finally:
                    for selected in (signal.SIGINT, signal.SIGTERM):
                        loop.remove_signal_handler(selected)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('action', choices=['archive', 'digest', 'setup', 'start', 'status', 'stop', 'restart', 'remove-service', 'supervise'])
    parser.add_argument('destination', nargs='?')
    args = parser.parse_args()
    if args.action == 'archive':
        if not args.destination:
            parser.error('archive requires its output file path')
        archive_package(args.destination)
        return
    if args.action == 'digest':
        print(source_digest())
        return
    if sys.platform != 'linux':
        raise DestinationError('Destination service operations require Linux and the current user systemd manager.')
    operations = Operations()
    if args.action == 'supervise':
        asyncio.run(supervise(operations))
        return
    if args.action == 'setup':
        if not sys.stdin.isatty():
            raise DestinationError('Open a destination terminal for private setup; no credentials or source files were changed.')
        if operations.path.exists() and operations.status()['status'] in {'active', 'running', 'activating'}:
            raise DestinationError('Stop the owned service before changing its private setup.')
        source_digest()
        command([str(ROOT / '.venv/bin/python'), 'setup.py'], terminal=True, timeout=1800)
        if operations.spec.get('knowledge', {}).get('mode') == 'local':
            command([str(ROOT / '.venv/bin/python'), 'knowledge/manage.py', 'up'], terminal=True, timeout=1200)
        print('Background services normally stop when you log out. Enabling lingering may ask for sudo permission.')
        if input('Keep this user\'s services running after disconnecting? [y/N]: ').strip().lower() == 'y':
            import pwd
            user = pwd.getpwuid(os.getuid()).pw_name
            command(['sudo', 'loginctl', 'enable-linger', user], terminal=True)
        result = operations.start()
    elif args.action in {'start', 'restart'}:
        result = operations.start(restart=args.action == 'restart')
    elif args.action in {'stop', 'remove-service'}:
        result = operations.stop(remove=args.action == 'remove-service')
    else:
        result = operations.status()
    print(json.dumps(result, indent=2))
    print('Checks cover native bootstrap and the Console. Custom UI, external workflows and scheduled actions need their own live evidence.')


if __name__ == '__main__':
    try:
        main()
    except DestinationError as error:
        print(str(error), file=sys.stderr)
        sys.exit(1)
    except (Exception, KeyboardInterrupt):
        # Exceptions can include credential-bearing third-party response bodies.
        print('Destination operation failed. Private configuration and runtime data were preserved. Check the owned service with remote_operations.py status, then retry.', file=sys.stderr)
        sys.exit(1)
