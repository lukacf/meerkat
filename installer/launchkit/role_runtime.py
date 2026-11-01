"""Trusted admission of separate Forge runtimes on the original root channel.

The launcher installs this hook before HTTP startup. Admission, failed startup
and retirement retain their original tasks; cancellation ends an observation,
not the I/O thread or the native owner's custody. State is selected explicitly
before the SDK builder receives it. No path is a native generation receipt.
"""
from __future__ import annotations

import asyncio
import os
import weakref

from .guide_transport import SelectedGuideTransport
from .protected_startup import SelectedProtectedRuntime


_preparations = None
_sealed = False
_claimed = weakref.WeakSet()


def select_runtime_preparer(preparer):
    """Install the trusted synchronous runtime(role=..., timeout=60) callback."""
    global _preparations
    if _preparations is not None and _preparations.preparer is preparer:
        return _preparations
    if _sealed or _preparations is not None or not callable(preparer):
        raise RuntimeError("Select one original runtime preparer before HTTP startup.")
    _preparations = RuntimePreparations(preparer)
    return _preparations


def seal_runtime_preparer():
    global _sealed
    _sealed = True
    return _preparations


def _retain(coroutine):
    task = asyncio.create_task(coroutine)
    task.add_done_callback(lambda done: None if done.cancelled() else done.exception())
    return task


class RuntimePreparations:
    def __init__(self, preparer):
        self.preparer = preparer
        self.pid = os.getpid()
        self.loop = None
        self.owners = []
        self.stop_task = None

    def check(self):
        if os.getpid() != self.pid:
            raise RuntimeError("Runtime admission belongs to its original process.")
        loop = asyncio.get_running_loop()
        if self.loop is None:
            self.loop = loop
        elif loop is not self.loop:
            raise RuntimeError("Runtime admission belongs to its original event loop.")

    def begin_shutdown(self):
        self.check()
        if self.stop_task is None:
            self.stop_task = _retain(self._shutdown())
        return self.stop_task

    async def shutdown(self):
        await asyncio.shield(self.begin_shutdown())

    async def join(self):
        """The HTTP loop cannot exit while retained application I/O needs it."""
        task = self.begin_shutdown()
        interrupted = []
        while not task.done():
            try:
                await asyncio.shield(task)
            except asyncio.CancelledError as error:
                if task.cancelled():
                    break
                interrupted.append(error)
            except BaseException:
                break
        try:
            task.result()
        except BaseException as error:
            interrupted.append(error)
        if interrupted:
            raise BaseExceptionGroup("Runtime retirement joined with failures.", interrupted)

    async def _shutdown(self):
        results = await asyncio.gather(*(owner.shutdown() for owner in tuple(self.owners)),
                                       return_exceptions=True)
        failures = [result for result in results if isinstance(result, BaseException)]
        if failures:
            raise BaseExceptionGroup("Forge runtime admission cleanup failed.", failures)


class PreparedRuntime:
    def __init__(self, manager, role, original, legacy_path):
        self.manager, self.role, self.original = manager, role, original
        self.legacy_path = legacy_path
        self.admitted = self.selected = self.runtime = None
        self.persistent_state = None
        self.prepare_task = self.build_task = self.stop_task = None
        self.startup_cleanup = []

    def _open(self):
        self.manager.check()
        if self.stop_task is not None or self.manager.stop_task is not None:
            raise RuntimeError("The original runtime is retiring.")
        self.original.recheck()

    async def prepare(self):
        self._open()
        if self.prepare_task is not None:
            raise RuntimeError("Runtime admission cannot be replayed.")
        self.prepare_task = _retain(self._prepare())
        await asyncio.shield(self.prepare_task)
        return self

    async def _prepare(self):
        try:
            from workspace_runtime_admission_client import AdmittedGatewayRuntime
            from workspace_gateway_transport import RestrictedGatewayTransportFactory
        except ModuleNotFoundError as error:
            if error.name not in {"workspace_runtime_admission_client", "workspace_gateway_transport"}:
                raise
            from .bootstrap import GatewayInstallError
            raise GatewayInstallError("The restricted runtime needs its host-supplied admission modules.") from error

        value = await asyncio.to_thread(self.manager.preparer, role=self.role, timeout=60)
        if type(value) is not AdmittedGatewayRuntime:
            raise TypeError("The launcher must return its actual admitted runtime.")
        if value in _claimed:
            raise RuntimeError("This admitted runtime already belongs to one consumer.")
        _claimed.add(value)
        # Retain before validation so an invalid association still closes through
        # its actual original owner, including a factory not consumed by an SDK.
        self.admitted = value
        value.check()
        if value.role != self.role or type(value.factory) is not RestrictedGatewayTransportFactory:
            raise RuntimeError("The original runtime role or factory changed.")
        self._open()
        original = self.original
        self.selected = SelectedGuideTransport(value.factory, original.version,
            original.core_version, original.meta_source, original.core_source,
            original.model_configuration, persistent_state=value.persistent_state)
        self.persistent_state = (self.selected.worker_state_path(self.legacy_path)
            if self.legacy_path is not None else self.selected.persistent_state)

    async def build(self, builder):
        self._open()
        if self.selected is None or self.build_task is not None:
            raise RuntimeError("One prepared runtime binds exactly one builder.")
        self.admitted.check()
        # The caller already passed persistent_state to the SDK. No init or
        # configuration rewrite is allowed here.
        self.selected.configure_builder(builder)
        self.build_task = _retain(self._build(builder))
        return await asyncio.shield(self.build_task)

    async def _build(self, builder):
        from .guide_runtime import build_with_cleanup
        self.runtime = await build_with_cleanup(builder, self.startup_cleanup)
        return self.runtime

    async def shutdown(self):
        self.manager.check()
        if self.stop_task is None:
            self.stop_task = _retain(self._shutdown())
        await asyncio.shield(self.stop_task)

    async def _shutdown(self):
        # Preparation/build failures belong to their observers. Join those exact
        # tasks before cleanup, without re-running either operation.
        for task in (self.prepare_task, self.build_task):
            if task is not None:
                await asyncio.gather(asyncio.shield(task), return_exceptions=True)
        failures = []
        for owner in ([self.runtime] if self.runtime is not None else []) + self.startup_cleanup:
            try:
                await owner.shutdown()
            except BaseException as error:
                failures.append(error)
        if self.admitted is not None:
            try:
                await asyncio.to_thread(self.admitted.close)
            except BaseException as error:
                failures.append(error)
        if failures:
            # In particular, close before facade start reports unobserved native
            # stop even after its local I/O has joined. Preserve that failure.
            raise BaseExceptionGroup("Prepared Forge runtime cleanup failed.", failures)
        self.manager.owners.remove(self)

    def __reduce_ex__(self, protocol):
        raise TypeError("An original runtime admission cannot be copied or serialized.")


async def prepare_runtime(role, original, legacy_path, retained_cleanup):
    """Return a fresh protected owner, or None for the ordinary local path."""
    if role not in ("architect", "planner") or (legacy_path is None and role != "planner"):
        raise ValueError("This role needs its actual native generation contract.")
    original.recheck()
    if type(original) not in (SelectedGuideTransport, SelectedProtectedRuntime):
        if _preparations is not None:
            raise RuntimeError("Protected admission needs its original restricted startup selection.")
        return None
    if _preparations is None:
        raise RuntimeError("The restricted runtime needs its trusted per-role preparer.")
    manager = _preparations
    manager.check()
    if manager.stop_task is not None:
        raise RuntimeError("Runtime admission is closed.")
    owner = PreparedRuntime(manager, role, original, legacy_path)
    manager.owners.append(owner)
    retained_cleanup.append(owner)
    return await owner.prepare()


async def shutdown_runtime(runtime, retained_cleanup):
    for owner in retained_cleanup:
        if type(owner) is PreparedRuntime and owner.runtime is runtime:
            await owner.shutdown()
            return
    await runtime.shutdown()
