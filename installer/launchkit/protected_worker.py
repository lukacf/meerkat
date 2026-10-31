"""Forge's parent handle for an original King receiver preparation.

The trusted host supplies one preparation before any child exists. Allocation,
credential drop, the actual Popen/pidfd and containment remain with that owner.
This handle is never restored from a saved receipt or a worker PID.
"""
from __future__ import annotations

import asyncio
from dataclasses import dataclass, field
import os
from pathlib import Path

from .deployment_deadlines import WORKER_STOP_TIMEOUT
from .remote_protected_worker import RemoteProtectedWorker


def selected_worker(owner, kind, directory):
    """Consume the launcher's actual per-job owner without a fallback spawn.

    The launcher associates a restricted client with the original selected job
    before inheritance. No job identity or path is reconstructed over its wire.
    Root-local composition retains the existing preparation interface.
    """
    from workspace_receiver_preparation import OriginalReceiverPreparation

    if type(owner) is OriginalReceiverPreparation:
        return ProtectedWorker(owner, kind, str(directory))
    return RemoteProtectedWorker(owner)


def is_protected_worker(worker):
    return type(worker) in (ProtectedWorker, RemoteProtectedWorker)


def select_worker_preparer(preparer, *services):
    """Trusted launcher hook, before either service admits any worker.

    The synchronous callback takes kind and directory as keyword arguments and
    returns one unused actual preparation or inherited restricted worker client.
    It must not spawn before returning.
    Native allocation and its failure cleanup remain the launcher's work.
    """
    if not callable(preparer) or not services:
        raise TypeError("The trusted host must supply its worker preparer and services.")
    if any(service._worker_preparer is not None and service._worker_preparer is not preparer
           for service in services):
        raise RuntimeError("The original worker preparer cannot be replaced.")
    if all(service._worker_preparer is preparer for service in services):
        return
    if any(service.tasks or service.processes for service in services):
        raise RuntimeError("Select the worker preparer before any worker admission.")
    for service in services:
        service._worker_preparer = preparer


@dataclass(frozen=True, eq=False)
class ProtectedWorker:
    preparation: object = field(repr=False)
    kind: str
    directory: str
    _pid: int = field(default_factory=os.getpid, init=False, repr=False)
    _loop: object = field(default_factory=asyncio.get_running_loop, init=False, repr=False)
    _request: object = field(default=None, init=False, repr=False)
    _gateway: object = field(default=None, init=False, repr=False)
    _state: object = field(default=None, init=False, repr=False)
    _spawn: object = field(default=None, init=False, repr=False)
    _child: object = field(default=None, init=False, repr=False)
    _control: object = field(default=None, init=False, repr=False)
    _start_attempted: bool = field(default=False, init=False, repr=False)
    _stop_task: object = field(default=None, init=False, repr=False)

    def __post_init__(self):
        from workspace_receiver_preparation import OriginalReceiverPreparation
        p = self.preparation
        if type(p) is not OriginalReceiverPreparation or not p.current():
            raise TypeError("The trusted host must supply its original receiver preparation.")
        gateway = p.gateway
        state = gateway.original_state_owner
        if state is None or gateway.state_owner is not state:
            raise TypeError("The worker needs its own explicitly selected gateway state owner.")
        object.__setattr__(self, "_gateway", gateway)
        object.__setattr__(self, "_state", state)
        # This synchronous selection precedes every spawn await. The upstream
        # owner refuses reuse, an already-created child or a changed request.
        request = p.select_worker(kind=self.kind, directory=self.directory)
        object.__setattr__(self, "_request", request)

    def _check(self):
        if os.getpid() != self._pid or asyncio.get_running_loop() is not self._loop:
            raise RuntimeError("The protected worker belongs to its original parent and loop.")
        p = self.preparation
        if p.request is not self._request or p.original_request is not self._request:
            raise RuntimeError("The original worker request changed.")
        gateway, state = self._gateway, self._state
        suffix = {"implementation": ".mobkit/state", "rehearsal": ".meta/state"}[self.kind]
        expected = str(Path(self.directory) / "native" / suffix)
        if (p.gateway is not gateway or gateway.original_receiver_preparation is not p
                or gateway.receiver_preparation is not p or gateway.original_state_owner is not state
                or gateway.state_owner is not state or state.gateway is not gateway
                or state.path != expected):
            raise RuntimeError("The worker's original gateway or exact persistent state changed.")
        if self._spawn is not None and (p.spawn is not self._spawn
                or p.original_spawn is not self._spawn or self._spawn.child is not self._child
                or self._spawn.process.child is not self._child
                or self._spawn.control is not self._control
                or self._spawn.original_control is not self._control):
            raise RuntimeError("The original worker process or control association changed.")

    async def start(self):
        self._check()
        if self._start_attempted or self._stop_task is not None:
            raise RuntimeError("An original worker start cannot be replayed.")
        object.__setattr__(self, "_start_attempted", True)
        spawn = await self.preparation.spawn_held()
        object.__setattr__(self, "_spawn", spawn)
        object.__setattr__(self, "_child", spawn.child)
        object.__setattr__(self, "_control", spawn.original_control)
        self._check()
        await spawn.activate()
        self._check()

    @property
    def returncode(self):
        self._check()
        return None if self._child is None else self._child.poll()

    async def wait(self):
        if self._child is None:
            raise RuntimeError("The original worker has not completed process admission.")
        while (code := self.returncode) is None:
            await asyncio.sleep(.01)
        # Process exit is only an observation. stop() still joins original
        # control, process-domain, channel and file owners before release.
        return code

    async def send(self, command):
        self._check()
        if self._control is None or self._stop_task is not None or self.returncode is not None:
            raise RuntimeError("The original worker control is unavailable.")
        return await self._control.send(command)

    async def _stop(self):
        failures = []
        try:
            self._check()
            control = self._control
            if (control is not None and control.started and not control.closing
                    and self.returncode is None):
                # EOF lets the same worker stop its SDK with the original
                # channel still available. No local signal or PID adoption.
                await control.finish()
                try:
                    await asyncio.wait_for(self.wait(), WORKER_STOP_TIMEOUT)
                except TimeoutError:
                    pass  # The original owner performs containment below.
        except BaseException as error:
            failures.append(error)
        try:
            # Also joins a spawn/activation which outlived its cancelled
            # observer, even before start() received the original child.
            await self.preparation.close()
            if not self.preparation.borrower_joined():
                raise RuntimeError("The original worker cleanup remains unconfirmed.")
        except BaseException as error:
            failures.append(error)
        try:
            # Receiver fencing can suppress the channel-loss retire callback.
            # Join this independently selected gateway explicitly, including
            # native startup and state pins, even if receiver cleanup failed.
            await self._gateway.close()
            if not self._gateway.closed or not self._state.joined():
                raise RuntimeError("The original worker gateway and state remain unjoined.")
        except BaseException as error:
            failures.append(error)
        if failures:
            raise BaseExceptionGroup("Protected worker stop failed.", failures)

    async def stop(self):
        if os.getpid() != self._pid or asyncio.get_running_loop() is not self._loop:
            raise RuntimeError("The protected worker belongs to its original parent and loop.")
        if self._stop_task is None:
            object.__setattr__(self, "_stop_task", self._loop.create_task(self._stop()))
        await asyncio.shield(self._stop_task)

    def __reduce_ex__(self, protocol):
        raise TypeError("An original protected worker cannot be copied or serialized.")
