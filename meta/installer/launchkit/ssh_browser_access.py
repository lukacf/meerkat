"""Pinned, owner-lifetime SSH forwarding for an existing native Console.

The handle owns a local guardian, not a persisted PID or an adopted listener.
Its sole forwarding maps the requested loopback port to the same remote port.
The caller must verify HTTP readiness before presenting a browser URL.
"""
from __future__ import annotations

import asyncio
import base64
import json
import math
import os
import subprocess
import sys
import time
from collections.abc import Sequence

from .ssh_transport import (SshTransport, SshTransportError, _environment,
                            _fingerprint_digest, _verified_fingerprint, validate_endpoint,
                            _operation_deadline, _remaining)


MAX_PIN = 64 * 1024
MAX_FRAME = 128 * 1024
CLEANUP_TIMEOUT = 12
_MESSAGES = {
    'configuration-unavailable': 'Configured SSH profiles are unavailable or unsafe. Check the private META_FORGE_SSH_PROFILES files and restart Forge after correcting them. Remote work may still be running; no replacement connection was admitted.',
    'invalid-endpoint': 'The browser access destination is invalid.',
    'invalid-request': 'The browser access request is invalid.',
    'host-key-unavailable': 'The previously verified SSH server key could not be pinned.',
    'port-unavailable': 'The requested local browser port is already in use. No existing listener was adopted.',
    'connection-unavailable': 'The owned SSH browser connection is unavailable.',
    'timeout': 'The SSH browser connection did not become available within its startup limit.',
    'output-limit': 'The SSH browser connection exceeded its bounded startup diagnostics.',
    'cleanup-unavailable': 'The owned SSH browser connection has not confirmed cleanup.',
}


class BrowserAccessError(RuntimeError):
    """Fixed public diagnostics, never SSH output, credentials or endpoint text."""
    def __init__(self, code, *, handle=None):
        if code not in _MESSAGES:
            raise ValueError('Unknown browser access error code.')
        self.code = code
        if code == 'cleanup-unavailable':
            # Private process custody for the caller, never public diagnostics.
            self.handle = handle
        super().__init__(_MESSAGES[code])


# This program holds the unique parent pipe and its own exact SSH child. The
# parent never signals a remembered PID. EOF survives host exit or SIGKILL.
GUARDIAN = r'''import base64,json,math,os,select,selectors,signal,stat,subprocess,sys,tempfile,time
MAX=65536
child=None
temporary=None
watch=None
stopping=False
observed_exit=False
def emit(value):
    print(json.dumps(value,separators=(',',':')),flush=True)
def stop(*_):
    global stopping
    stopping=True
for name in (signal.SIGTERM,signal.SIGHUP,signal.SIGINT): signal.signal(name,stop)
def exited():
    global observed_exit
    if observed_exit: return True
    if hasattr(os,'waitid') and hasattr(os,'WNOWAIT'):
        observed_exit=os.waitid(os.P_PID,child.pid,os.WEXITED|os.WNOHANG|os.WNOWAIT) is not None
    else:
        observed_exit=bool(watch.control([],1,0))
    return observed_exit
def cleanup():
    if child is None: return True
    if child.returncode is None:
        try: os.killpg(child.pid,signal.SIGKILL)
        except ProcessLookupError: pass
        # macOS can refuse a group signal for an unreaped exited leader. The
        # kernel exit observation still permits reaping that exact child.
        except PermissionError:
            if not exited(): return False
        except OSError: return False
        try: child.wait(timeout=5)
        except (OSError,subprocess.TimeoutExpired): return False
    # After reaping, never signal the numerical group again. Its disappearance
    # can still confirm eventual cleanup; an unfamiliar group remains refused.
    deadline=time.monotonic()+5
    while time.monotonic()<deadline:
        try: os.killpg(child.pid,0)
        except ProcessLookupError: return True
        except OSError: return False
        time.sleep(.02)
    return False
def finish(event='closed',code=None):
    global temporary
    cleaned=cleanup()
    value={'event':event,'cleaned':cleaned}
    if code: value['code']=code
    try: emit(value)
    except (OSError,BrokenPipeError): pass
    if cleaned:
        if temporary: temporary.cleanup(); temporary=None
        return True
    return False
try:
    # Do not read ahead into later owner commands before os.read takes over.
    raw=sys.stdin.buffer.raw.readline(131073)
    if not raw.endswith(b'\n') or len(raw)>131072: raise ValueError()
    request=json.loads(raw)
    deadline=request['deadline']; timeout=request['timeout']
    if any(isinstance(value,bool) or not isinstance(value,(int,float)) or not math.isfinite(value)
            for value in (deadline,timeout)) or not 0<timeout<=60: raise ValueError()
    # Both processes use the local monotonic clock. IPC and private pin setup
    # consume the caller's budget; receiving this request never renews it.
    deadline=min(deadline,time.monotonic()+timeout)
    if time.monotonic()>=deadline: raise TimeoutError()
    pin=base64.b64decode(request['pin'],validate=True)
    if not pin or len(pin)>MAX: raise ValueError()
    temporary=tempfile.TemporaryDirectory(prefix='mobkit-browser-pin-')
    path=os.path.join(temporary.name,'known_hosts')
    fd=os.open(path,os.O_WRONLY|os.O_CREAT|os.O_EXCL|os.O_NOFOLLOW,0o600)
    with os.fdopen(fd,'wb') as channel: channel.write(pin)
    argv=request['argv']
    if argv.count('UserKnownHostsFile=@PIN@')!=1: raise ValueError()
    argv[argv.index('UserKnownHostsFile=@PIN@')]='UserKnownHostsFile='+path
    env={key:os.environ[key] for key in ('HOME','USER','PATH','SSH_AUTH_SOCK') if key in os.environ}
    env['LANG']='C'
    if time.monotonic()>=deadline: raise TimeoutError()
    child=subprocess.Popen(argv,stdin=subprocess.DEVNULL,stdout=subprocess.DEVNULL,
        stderr=subprocess.PIPE,env=env,start_new_session=True,close_fds=True,bufsize=0)
    if not hasattr(os,'waitid') or not hasattr(os,'WNOWAIT'):
        watch=select.kqueue()
        watch.control([select.kevent(child.pid,filter=select.KQ_FILTER_PROC,
            flags=select.KQ_EV_ADD|select.KQ_EV_ENABLE,fflags=select.KQ_NOTE_EXIT)],0,0)
    os.set_blocking(0,False)
    os.set_blocking(child.stderr.fileno(),False)
    selection=selectors.DefaultSelector()
    selection.register(0,selectors.EVENT_READ,'owner')
    selection.register(child.stderr,selectors.EVENT_READ,'diagnostics')
    kept=bytearray(); commands=bytearray(); ready=False; ended=False
    listening=('debug1: Local forwarding listening on 127.0.0.1 port '+str(request['port'])+'.').encode()
    conflict=('bind [127.0.0.1]:'+str(request['port'])+': Address already in use').encode()
    while True:
        if stopping:
            if finish(): break
            time.sleep(.1); continue
        if exited():
            if not ready:
                while len(kept)<=MAX:
                    try: data=os.read(child.stderr.fileno(),4096)
                    except BlockingIOError: break
                    if not data: break
                    kept.extend(data)
            code='port-unavailable' if conflict in kept.splitlines() else 'connection-unavailable'
            if len(kept)>MAX: code='output-limit'
            if finish('error' if not ready else 'closed',code if not ready else None): break
            stopping=True; continue
        if not ready and time.monotonic()>=deadline:
            if finish('error','timeout'): break
            stopping=True; continue
        for key,_ in selection.select(.05):
            if key.data=='owner':
                data=os.read(0,4096)
                if not data: stopping=True; break
                commands.extend(data)
                if len(commands)>4096: stopping=True; break
                while b'\n' in commands:
                    line,_,rest=commands.partition(b'\n'); commands=bytearray(rest)
                    if line==b'close': stopping=True; break
                    if line!=b'observe': stopping=True; break
                    emit({'event':'observed','live':not exited()})
            else:
                data=os.read(child.stderr.fileno(),4096)
                if not data:
                    selection.unregister(child.stderr)
                    ended=True
                elif not ready:
                    kept.extend(data)
                    if len(kept)>MAX:
                        if finish('error','output-limit'): sys.exit(0)
                        stopping=True; break
                if not ready and listening in kept.splitlines() and b'debug1: Entering interactive session.' in kept.splitlines():
                    if not exited():
                        if time.monotonic()>=deadline: raise TimeoutError()
                        emit({'event':'ready','diagnostics':base64.b64encode(kept).decode()})
                        ready=True
        if ended and not ready:
            if finish('error','port-unavailable' if conflict in kept.splitlines() else 'connection-unavailable'): break
            stopping=True
except BaseException as error:
    if not finish('error','timeout' if isinstance(error,TimeoutError) else 'connection-unavailable'):
        while not cleanup(): time.sleep(.1)
        if temporary: temporary.cleanup(); temporary=None
finally:
    if watch: watch.close()
    if child and child.stderr: child.stderr.close()
'''


async def _io_ready(descriptor, *, writing=False):
    loop = asyncio.get_running_loop()
    future = loop.create_future()
    def ready():
        if not future.done():
            future.set_result(None)
    method = loop.add_writer if writing else loop.add_reader
    remove = loop.remove_writer if writing else loop.remove_reader
    method(descriptor, ready)
    try:
        await future
    finally:
        remove(descriptor)


class _Handle:
    def __init__(self, process):
        self._process = process
        self._buffer = bytearray()
        self._lock = asyncio.Lock()
        self._closed = False
        self._cleaned = False
        self._cleanup_confirmed = False
        for stream in (process.stdin, process.stdout):
            os.set_blocking(stream.fileno(), False)

    async def _send(self, content):
        view = memoryview(content)
        while view:
            try:
                count = os.write(self._process.stdin.fileno(), view)
                if count <= 0:
                    raise BrokenPipeError()
                view = view[count:]
            except BlockingIOError:
                await _io_ready(self._process.stdin.fileno(), writing=True)

    async def _frame(self):
        while b'\n' not in self._buffer:
            try:
                data = os.read(self._process.stdout.fileno(), 4096)
            except BlockingIOError:
                await _io_ready(self._process.stdout.fileno())
                continue
            if not data:
                raise BrowserAccessError('connection-unavailable')
            self._buffer.extend(data)
            if len(self._buffer) > MAX_FRAME:
                raise BrowserAccessError('output-limit')
        raw, _, rest = self._buffer.partition(b'\n')
        self._buffer = bytearray(rest)
        try:
            value = json.loads(raw)
        except (ValueError, UnicodeError):
            raise BrowserAccessError('connection-unavailable') from None
        if not isinstance(value, dict):
            raise BrowserAccessError('connection-unavailable')
        if value.get('event') in {'closed', 'error'} and value.get('cleaned') is True:
            self._cleanup_confirmed = True
        return value

    async def observe(self):
        async with self._lock:
            if self._closed:
                return False
            try:
                await asyncio.wait_for(self._send(b'observe\n'), 2)
                value = await asyncio.wait_for(self._frame(), 2)
                return value.get('event') == 'observed' and value.get('live') is True
            except (BrowserAccessError, OSError, ValueError, asyncio.TimeoutError):
                return False

    async def _close(self):
        async with self._lock:
            if self._closed:
                return self._cleaned
            confirmed = self._cleanup_confirmed
            try:
                try:
                    await asyncio.wait_for(self._send(b'close\n'), 1)
                except (OSError, ValueError, asyncio.TimeoutError):
                    pass
                finally:
                    self._process.stdin.close()
                async def confirmation():
                    while True:
                        value = await self._frame()
                        if value.get('event') in {'closed', 'error'}:
                            return value.get('cleaned') is True
                if not confirmed:
                    confirmed = await asyncio.wait_for(confirmation(), CLEANUP_TIMEOUT)
            except (BrowserAccessError, OSError, ValueError, asyncio.TimeoutError):
                pass
            if confirmed:
                try:
                    await asyncio.to_thread(self._process.wait, timeout=1)
                    self._closed = self._cleaned = True
                    self._process.stdout.close()
                except (OSError, subprocess.TimeoutExpired):
                    confirmed = False
            return confirmed

    async def close(self):
        result, cancelled = await _settle_cleanup(self)
        if cancelled:
            raise asyncio.CancelledError()
        return result


async def _settle_cleanup(handle):
    task = asyncio.create_task(handle._close())
    cancelled = False
    while True:
        try:
            return await asyncio.shield(task), cancelled
        except asyncio.CancelledError:
            cancelled = True
            if task.done():
                return False, cancelled


class SshBrowserAccessTransport:
    """Trusted host composition only; browser input cannot choose SSH arguments."""
    def __init__(self, *, executable='/usr/bin/ssh', argv_prefix: Sequence[str] = (), known_hosts_paths=None):
        self._ssh = SshTransport(known_hosts_paths=known_hosts_paths)
        self._executable = executable
        self._argv_prefix = tuple(argv_prefix)

    async def open(self, endpoint, *, expected_fingerprint, port, timeout=15, deadline=None):
        try:
            target = validate_endpoint(endpoint)
        except SshTransportError:
            raise BrowserAccessError('invalid-endpoint') from None
        expected = _fingerprint_digest(expected_fingerprint)
        if (expected is None or type(port) is not int or not 1024 <= port <= 65535
                or isinstance(timeout, bool) or not isinstance(timeout, (int, float))):
            raise BrowserAccessError('invalid-request')
        try:
            if not math.isfinite(timeout) or not 0 < timeout <= 60:
                raise BrowserAccessError('invalid-request')
        except OverflowError:
            raise BrowserAccessError('invalid-request') from None
        loop = asyncio.get_running_loop()
        try:
            deadline = _operation_deadline(timeout, deadline)
        except SshTransportError:
            raise BrowserAccessError('invalid-request') from None
        handle = None
        try:
            try:
                remaining = _remaining(deadline)
                pinned = await asyncio.wait_for(self._ssh._pin(target, expected, deadline), remaining)
            except SshTransportError as error:
                raise BrowserAccessError('timeout' if error.code == 'timeout' else 'host-key-unavailable') from None
            if not pinned or len(pinned) > MAX_PIN:
                raise BrowserAccessError('output-limit')
            argv = [self._executable, *self._argv_prefix, '-F', '/dev/null', '-v', '-N', '-T',
                    '-o', 'UserKnownHostsFile=@PIN@', '-o', 'GlobalKnownHostsFile=/dev/null',
                    '-o', 'StrictHostKeyChecking=yes', '-o', 'BatchMode=yes', '-o', 'UpdateHostKeys=no',
                    '-o', 'ForwardAgent=no', '-o', 'PermitLocalCommand=no', '-o', 'ControlMaster=no',
                    '-o', 'ControlPath=none', '-o', 'ClearAllForwardings=no', '-o', 'GatewayPorts=no',
                    '-o', 'ExitOnForwardFailure=yes', '-o', 'ConnectTimeout=10',
                    '-o', 'ServerAliveInterval=15', '-o', 'ServerAliveCountMax=2',
                    '-o', 'FingerprintHash=sha256', '-L', f'127.0.0.1:{port}:127.0.0.1:{port}',
                    '-p', str(target['ssh_port']), '-l', target['user'], '--', target['host']]
            # Sample the process clock first so translating an event-loop
            # deadline cannot add time between the two clock observations.
            guardian_clock = time.monotonic()
            remaining = _remaining(deadline)
            body = json.dumps({'argv': argv, 'pin': base64.b64encode(pinned).decode(),
                               'port': port, 'timeout': remaining,
                               'deadline': guardian_clock + remaining}, separators=(',', ':')).encode() + b'\n'
            if len(body) > MAX_FRAME:
                raise BrowserAccessError('invalid-request')
            if deadline - loop.time() <= 0:
                raise BrowserAccessError('timeout')
            # Synchronous Popen gives the cancellation path custody immediately.
            process = subprocess.Popen([sys.executable, '-I', '-c', GUARDIAN],
                stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
                env=_environment(), start_new_session=True, close_fds=True, bufsize=0)
            handle = _Handle(process)
            remaining = _remaining(deadline)
            await asyncio.wait_for(handle._send(body), remaining)
            remaining = _remaining(deadline)
            value = await asyncio.wait_for(handle._frame(), remaining)
            if value.get('event') != 'ready':
                code = value.get('code')
                # Preserve the guardian's exact cleanup evidence for close().
                handle._buffer[:0] = json.dumps(value, separators=(',', ':')).encode() + b'\n'
                raise BrowserAccessError(code if code in _MESSAGES else 'connection-unavailable')
            try:
                diagnostics = base64.b64decode(value['diagnostics'], validate=True)
            except (ValueError, TypeError, KeyError):
                raise BrowserAccessError('connection-unavailable') from None
            if len(diagnostics) > MAX_PIN or _fingerprint_digest(_verified_fingerprint(diagnostics)) != expected:
                raise BrowserAccessError('host-key-unavailable')
            _remaining(deadline)
            return handle
        except BaseException as error:
            cancelled = False
            if handle is not None:
                cleaned, cancelled = await _settle_cleanup(handle)
                if not cleaned:
                    raise BrowserAccessError('cleanup-unavailable', handle=handle) from None
            if cancelled and not isinstance(error, asyncio.CancelledError):
                raise asyncio.CancelledError() from None
            if isinstance(error, asyncio.CancelledError):
                raise
            if isinstance(error, BrowserAccessError):
                raise error from None
            if isinstance(error, asyncio.TimeoutError):
                raise BrowserAccessError('timeout') from None
            if isinstance(error, SshTransportError) and error.code == 'timeout':
                raise BrowserAccessError('timeout') from None
            raise BrowserAccessError('connection-unavailable') from None
