"""Retain application intent before Connect prepares one authenticated original.

Connect owns current member/controller selection, native grants and replay.
This adapter neither submits work nor selects authority from text or topology.
"""
from __future__ import annotations

import asyncio
from copy import deepcopy
from dataclasses import asdict
import json
import math

from .connect_installer_caller import CustodyError


async def _join(task):
    """Join an entered callback or blocking SDK call despite cancellation."""
    while True:
        try:
            return await asyncio.shield(task)
        except asyncio.CancelledError:
            if task.cancelled():
                raise


class NativeOriginalOperations:
    def __init__(self, owner, application, runtime):
        self.owner, self.application, self.runtime = owner, application, runtime
        self.callbacks = owner.callbacks
        self._closed = False
        self._pending = set()

    def _current(self):
        self.owner._require_current()
        self.callbacks._current()
        context = self.application.context
        if (self._closed or self.owner.runtime is not self.runtime
                or context.runtime is not self.runtime
                or self.owner.application is not self.application
                or self.application._application_callbacks is not self.callbacks
                or self.owner._shutdown_task is not None or context.stop_event.is_set()):
            raise CustodyError("Native preparation requires its current connected application owner.")

    async def prepare(self, submission, request, *, retain_request, timeout=60.0):
        """Prepare exactly once after the app durably retains immutable JSON.

        A returned original is a preparation receipt, not admission or delivery.
        Unknown settlement preserves the same original, request and issuance IDs.
        """
        from meerkat_toolkit.host_control import (
            PrivateOriginalRef, PrivateOriginalSubmit, PrivateSourceEvidence,
        )

        self._current()
        if (not isinstance(submission, PrivateOriginalSubmit)
                or not isinstance(submission.original, PrivateOriginalRef)
                or not isinstance(submission.content, str)
                or not isinstance(submission.transport_authentication, PrivateSourceEvidence)
                or not isinstance(request, dict) or not callable(retain_request)):
            raise TypeError("Native preparation requires an authenticated submission and retention callback.")
        if timeout is not None and (isinstance(timeout, bool) or not isinstance(timeout, (int, float))
                or not math.isfinite(timeout) or timeout <= 0):
            raise ValueError("Native preparation timeout must be positive and finite.")
        # Preserve exact values without sharing mutable request aliases with
        # application callbacks. The public SDK validates the native vocabulary.
        submission = deepcopy(submission)
        serialized = json.dumps({"submission": asdict(submission), "request": request},
            ensure_ascii=True, allow_nan=False, separators=(",", ":"))
        selected = json.loads(serialized)["request"]
        client = self.runtime.host_control
        prepare = getattr(client, "prepare_original_native_operations", None)
        if not callable(prepare):
            raise CustodyError("The selected Connect SDK does not support native operation preparation.")
        cancelled = asyncio.Event()

        async def enter():
            self._current()
            if cancelled.is_set():
                raise asyncio.CancelledError
            await retain_request(serialized)
            self._current()
            if cancelled.is_set():
                raise asyncio.CancelledError
            if self.runtime.host_control is not client:
                raise CustodyError("The selected private host client changed before native preparation.")
            # The blocking SDK call is retained until it returns. Native checks
            # current member routes and completed Add state at its own boundary.
            return await asyncio.to_thread(prepare, submission, selected, timeout=timeout)

        task = asyncio.create_task(enter())
        self._pending.add(task)
        try:
            return await asyncio.shield(task)
        except asyncio.CancelledError:
            cancelled.set()
            try:
                await _join(task)
            except BaseException:
                # Cancellation is not evidence that a native write rolled back.
                pass
            raise
        finally:
            self._pending.discard(task)

    async def close(self):
        """Close entry, then join callbacks before the application store closes."""
        self._closed = True
        pending = tuple(self._pending)
        if pending:
            # Each caller owns its native result/refusal. Closing only joins
            # custody; it does not turn an unknown outcome into success.
            await _join(asyncio.gather(*pending, return_exceptions=True))


def bind_native_operations(owner, application, runtime):
    """Same private host client for fixed/dynamic members on Fresh and Restore."""
    application.context._native_original_operations = NativeOriginalOperations(owner, application, runtime)
