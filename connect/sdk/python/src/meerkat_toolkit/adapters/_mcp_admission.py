"""Finite admission to an existing asyncio MCP execution lock.

This helper owns no worker, retry, task, credential or shutdown lifecycle.
The caller retains its lock until provider execution and cleanup have joined.
"""

from __future__ import annotations

import asyncio
from collections.abc import AsyncIterator, Callable
from contextlib import asynccontextmanager
import math
from typing import Literal


class AdmissionRefused(Exception):
    """A bounded refusal before the operation has been entered."""

    def __init__(self, kind: Literal["server_busy", "queue_timeout", "closed"]) -> None:
        self.kind = kind
        super().__init__(kind)


class CallAdmission:
    """Admit at most four calls, with one executing and a finite queue wait.

    Instances belong to the same asyncio owner as their supplied lock. Limits
    cover this MCP boundary, not arbitrary direct calls on the borrowed client.
    """

    def __init__(
        self, lock: asyncio.Lock, *, maximum_admitted: int = 4,
        queue_timeout_seconds: float = 30.0,
    ) -> None:
        if (type(maximum_admitted) is not int or maximum_admitted < 1
                or type(queue_timeout_seconds) not in (int, float)
                or not math.isfinite(queue_timeout_seconds) or queue_timeout_seconds <= 0):
            raise ValueError("Invalid MCP admission limits")
        self._lock = lock
        self._maximum_admitted = maximum_admitted
        self._queue_timeout_seconds = queue_timeout_seconds
        self._admitted = 0

    @asynccontextmanager
    async def enter(self, *, is_closed: Callable[[], bool] = lambda: False) -> AsyncIterator[None]:
        # No await may separate the capacity check and reservation. Excess
        # requests never become semaphore/lock waiters or copy their arguments.
        if is_closed():
            raise AdmissionRefused("closed")
        if self._admitted >= self._maximum_admitted:
            raise AdmissionRefused("server_busy")
        self._admitted += 1
        acquired = False
        loop = asyncio.get_running_loop()
        deadline = loop.time() + self._queue_timeout_seconds
        try:
            try:
                async with asyncio.timeout_at(deadline):
                    acquired = await self._lock.acquire()
            except TimeoutError:
                raise AdmissionRefused("closed" if is_closed() else "queue_timeout") from None
            # A release can wake a waiter after its deadline before the timer
            # callback runs. Check again before releasing anything to the caller.
            if is_closed():
                raise AdmissionRefused("closed")
            if loop.time() >= deadline:
                raise AdmissionRefused("queue_timeout")
            # The queue timer is gone before execution. Cancellation cleanup in
            # the caller must finish before this context releases the lock.
            yield
        finally:
            if acquired:
                self._lock.release()
            self._admitted -= 1
