"""Async consumer of one root-selected held launcher's restricted worker channel.

Only the trusted launcher supplies the actual client for the selected job.
This adapter cannot allocate a worker, choose its state or recover an owner
from a PID, descriptor number or saved receipt. The held launcher retains those originals.
"""
from __future__ import annotations

import asyncio
from dataclasses import dataclass, field
import os
import weakref

from .deployment_deadlines import WORKER_STOP_TIMEOUT


_claimed_clients = weakref.WeakSet()


@dataclass(frozen=True, eq=False)
class RemoteProtectedWorker:
    client: object = field(repr=False)
    _pid: int = field(default_factory=os.getpid, init=False, repr=False)
    _loop: object = field(default_factory=asyncio.get_running_loop, init=False, repr=False)
    _lock: object = field(default_factory=asyncio.Lock, init=False, repr=False)
    _io_task: object = field(default=None, init=False, repr=False)
    _stop_task: object = field(default=None, init=False, repr=False)
    _start_attempted: bool = field(default=False, init=False, repr=False)
    _observation: object = field(default=None, init=False, repr=False)

    def __post_init__(self):
        try:
            from workspace_worker_client import RestrictedWorkerClient
        except ModuleNotFoundError as error:
            if error.name != "workspace_worker_client":
                raise
            from .bootstrap import GatewayInstallError
            raise GatewayInstallError("The restricted worker needs its host-supplied client module.") from error

        if type(self.client) is not RestrictedWorkerClient:
            raise TypeError("The trusted launcher must supply its original restricted worker client.")
        if self.client in _claimed_clients:
            raise RuntimeError("The original worker client already belongs to one consumer.")
        _claimed_clients.add(self.client)

    def _check(self):
        # Refuse a fork before touching any inherited event-loop lock.
        if os.getpid() != self._pid or asyncio.get_running_loop() is not self._loop:
            raise RuntimeError("The restricted worker belongs to its original process and loop.")

    async def _exchange(self, action, payload, timeout):
        try:
            result = await asyncio.to_thread(self.client.operation, action, payload, timeout=timeout)
            object.__setattr__(self, "_observation", result)
            return result
        finally:
            self._lock.release()

    async def _operation(self, action, payload=None, *, timeout=15, stopping=False):
        self._check()
        # Cancellation before admission must remove the waiter, not leave a
        # shielded queued command that sends later without its caller.
        await self._lock.acquire()
        try:
            self._check()
            if self._stop_task is not None and not stopping:
                raise RuntimeError("The original worker is stopping.")
            task = self._loop.create_task(self._exchange(action, payload, timeout))
        except BaseException:
            self._lock.release()
            raise
        object.__setattr__(self, "_io_task", task)
        # An abandoned observer must not orphan the I/O thread or its exception.
        # The task owns the lock until the thread returns, and stop joins it.
        task.add_done_callback(lambda completed: None if completed.cancelled() else completed.exception())
        return await asyncio.shield(task)

    async def start(self):
        self._check()
        if self._start_attempted or self._stop_task is not None:
            raise RuntimeError("An original worker start cannot be replayed.")
        object.__setattr__(self, "_start_attempted", True)
        result = await self._operation("start")
        if not result["started"]:
            raise RuntimeError("The original worker start is unconfirmed.")

    @property
    def returncode(self):
        self._check()
        # The last exact root observation is a cache, never a cleanup claim.
        return None if self._observation is None else self._observation["exit_code"]

    async def wait(self):
        self._check()
        if self._observation is None or not self._observation["started"]:
            raise RuntimeError("The original worker has not completed process admission.")
        while self.returncode is None:
            result = await self._operation("status")
            if result["exit_code"] is not None:
                break
            if result["failure"] is not None or result["retirement"] in {"joined", "failed"}:
                raise RuntimeError("The original worker exit remains unconfirmed.")
            await asyncio.sleep(.05)
        return self.returncode

    async def send(self, command):
        from workspace_gateway_wire import json_bytes, json_value

        self._check()
        if (self._observation is None or not self._observation["started"]
                or self.returncode is not None or self._stop_task is not None):
            raise RuntimeError("The original worker control is unavailable.")
        # Freeze the entire bounded command before waiting for any other I/O.
        value = json_value(json_bytes({"version": 1, "action": "send", "payload": command}, 65536))
        result = await self._operation("send", value["payload"])
        return result["pipe_bytes"]  # Delivery only; Forge retains command semantics.

    async def _stop(self):
        failures = []
        try:
            previous = self._io_task
            if previous is not None:
                # Even a timed-out/cancelled send observer retains this exact
                # original operation. Do not close its socket underneath it.
                await asyncio.gather(asyncio.shield(previous), return_exceptions=True)
            result = await self._operation("stop", timeout=WORKER_STOP_TIMEOUT, stopping=True)
            if result["retirement"] != "joined":
                raise RuntimeError("The original worker cleanup remains unconfirmed.")
        except BaseException as error:
            failures.append(error)
        try:
            # Local close only releases this channel. It cannot turn a failed
            # or timed-out remote join into successful worker termination.
            await asyncio.to_thread(self.client.close)
        except BaseException as error:
            failures.append(error)
        if failures:
            raise BaseExceptionGroup("Protected remote worker stop failed.", failures)

    async def stop(self):
        self._check()
        if self._stop_task is None:
            object.__setattr__(self, "_stop_task", self._loop.create_task(self._stop()))
        await asyncio.shield(self._stop_task)

    def __reduce_ex__(self, protocol):
        raise TypeError("An original restricted worker cannot be copied or serialized.")
