"""One original pipe pair with an external trusted process owner.

This adapter transfers ordinary I/O only. It never authenticates the launcher,
spawns a child, signals a PID, or proves native/descendant/durable settlement.
"""
from __future__ import annotations

import inspect
import io
import math
import os
from pathlib import Path
import stat
import subprocess
import threading
import time
from types import MappingProxyType
from typing import Callable, Mapping

from .service_setup import ServiceSetupError, _MAX_WAIT


class ServiceCollectionTransport:
    """Single-use transfer of the original root-launched child's ordinary pipes.

    input is the original unbuffered binary writer to child stdin; output is
    the original unbuffered binary reader from child stdout. After adoption,
    only the SDK may touch or close them. No duplicate or replacement endpoints
    are permitted. Failed pre-adoption validation leaves them with the host.
    The host must prove the correct child pipe pairing: kernel identities can
    describe separate endpoints rather than the pair on some platforms.

    request_retirement must synchronously queue retirement of the original root
    reservation without waiting on I/O or joining any SDK worker. It is called
    at most once, on output EOF or transport failure. It must not mint a new
    reservation. wait_process(timeout) observes only that original child's exit
    and returns its integer exit code, or raises TimeoutError while still owned.
    It must honor the timeout (including zero), never relaunch, and never wait
    for SDK workers, native parent settlement, or a root operation that awaits
    those workers. The root owner separately joins descendants and settlement.

    Callbacks run on SDK threads or the caller of close. Bridge to a root event
    loop safely; do not call blocking client methods on that loop. The host
    retains its original process owner throughout failures and observer timeouts.
    Callback construction and the credential scope are host intent, not proof.
    """

    def __init__(self, *, input: io.FileIO, output: io.FileIO,
                 request_retirement: Callable[[], None], wait_process: Callable[[float], int],
                 credential_scope: Mapping[str, str]):
        try:
            scope = dict(credential_scope)
            if scope != {"kind": "native_default"} and (
                    set(scope) != {"kind", "root"} or scope["kind"] != "file"
                    or not isinstance(scope["root"], str) or "\0" in scope["root"]
                    or not Path(scope["root"]).is_absolute()):
                raise ValueError()
            if (not callable(request_retirement) or not callable(wait_process)
                    or inspect.iscoroutinefunction(request_retirement)
                    or inspect.iscoroutinefunction(wait_process)):
                raise ValueError()
            self._check_pipes(input, output)
        except Exception:
            raise ValueError("original private pipes and root process observers required") from None
        self._scope = MappingProxyType(scope)
        self._owner = _OriginalProcess(input, output, request_retirement, wait_process)
        self._claim_lock = threading.Lock()
        self._claim_token: object | None = None

    @staticmethod
    def _check_pipes(input: io.FileIO, output: io.FileIO) -> None:
        import fcntl
        if (type(input) is not io.FileIO or type(output) is not io.FileIO
                or input.closed or output.closed or input is output
                or not input.closefd or not output.closefd
                or input.fileno() == output.fileno()
                or not input.writable() or input.readable()
                or not output.readable() or output.writable()):
            raise ValueError()
        identities = []
        for stream, access in ((input, os.O_WRONLY), (output, os.O_RDONLY)):
            info = os.fstat(stream.fileno())
            if (not stat.S_ISFIFO(info.st_mode) or not os.get_blocking(stream.fileno())
                    or fcntl.fcntl(stream.fileno(), fcntl.F_GETFL) & os.O_ACCMODE != access):
                raise ValueError()
            identities.append((info.st_dev, info.st_ino))
        if identities[0] == identities[1]:
            raise ValueError()

    @property
    def credential_scope(self) -> Mapping[str, str]:
        return self._scope

    def _claim(self, token: object) -> _OriginalProcess:
        with self._claim_lock:
            if self._claim_token is not None:
                raise ValueError("original collection transport already adopted")
            # Recheck the still-host-owned descriptors at the actual transfer.
            self._check_pipes(self._owner.stdin, self._owner.stdout)
            for stream in (self._owner.stdin, self._owner.stdout):
                os.set_inheritable(stream.fileno(), False)
                if os.get_inheritable(stream.fileno()):
                    raise ValueError("original pipe remains inheritable")
            self._claim_token = token
            return self._owner

    def _claimed_owner(self, token: object) -> _OriginalProcess | None:
        # Recover only this adoption's owner if interruption followed transfer.
        with self._claim_lock:
            return self._owner if self._claim_token is token else None


class _OriginalProcess:
    """Private pipe custody plus serialized observations of one external owner."""

    def __init__(self, input: io.FileIO, output: io.FileIO,
                 retire: Callable[[], None], wait: Callable[[float], int]):
        self.stdin, self.stdout = input, output
        self.returncode: int | None = None
        self.healthy = True
        self._retire, self._wait = retire, wait
        self._retirement_lock, self._wait_lock = threading.Lock(), threading.Lock()
        self._retirement_started = False

    def request_retirement(self) -> None:
        with self._retirement_lock:
            if self._retirement_started:
                return
            self._retirement_started = True
        try:
            if self._retire() is not None:
                raise ValueError()
        except BaseException:
            self.healthy = False
            raise ServiceSetupError("original root retirement is unconfirmed") from None

    def wait(self, timeout: float) -> int:
        if (type(timeout) not in (int, float) or not math.isfinite(timeout)
                or not 0 <= timeout <= _MAX_WAIT):
            raise ValueError("invalid process observation timeout")
        deadline = time.monotonic() + timeout
        if not self._wait_lock.acquire(timeout=timeout):
            raise subprocess.TimeoutExpired("original native process", timeout)
        try:
            if self.returncode is not None:
                return self.returncode
            try:
                result = self._wait(max(0, deadline - time.monotonic()))
                if type(result) is not int or not -255 <= result <= 255:
                    raise ValueError()
            except (TimeoutError, subprocess.TimeoutExpired):
                raise subprocess.TimeoutExpired("original native process", timeout) from None
            except BaseException:
                self.healthy = False
                raise ServiceSetupError("original process exit is unconfirmed") from None
            self.returncode = result
            return result
        finally:
            self._wait_lock.release()
