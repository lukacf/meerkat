"""Bounded stop of Meta Forge's native work owners when the server stops.

The installer host gives the server a fixed grace after SIGTERM. Each owner
stops in order within its own deadline, so a stop that hangs neither holds the
owners after it nor keeps the server alive.

An owner's row may name process groups that only this owner leads. Forge
signals them once that owner's stop is past its deadline: SIGTERM, a grace,
then SIGKILL for the groups still present. Forge's own group is never
signalled.

A row may also name the owner's live workers that stop themselves on owner
loss (stdin EOF), within the bound their live owner waits. stop_owners never
signals them. Once their owner's stop has ended, in time or not, it closes
the stdin of each one still running and releases its handle: asyncio kills a
still-running child when its subprocess transport is closed or collected,
which a normal interpreter exit does, and that kill would cut the worker's own
stop short.
"""
from __future__ import annotations

import asyncio
from collections.abc import Callable, Iterable
from dataclasses import dataclass
import logging
import os
import signal
import sys
import time

# Seconds each owner's stop may take before it counts as hung.
OWNER_STOP_DEADLINE = 10
# Seconds a signalled group has between SIGTERM and SIGKILL.
GROUP_STOP_GRACE = 3
# CPython versions whose BaseSubprocessTransport.close() returns at once when
# _closed is set and whose __del__ calls close() only when it is not. Checked
# against asyncio/base_subprocess.py of each; any other version keeps
# asyncio's kill and logs that it does.
RELEASE_VERSIONS = ((3, 11), (3, 12), (3, 13), (3, 14), (3, 15))

try:
    from asyncio.base_subprocess import BaseSubprocessTransport
except ImportError:  # Another layout keeps asyncio's kill; release_worker reports it.
    BaseSubprocessTransport = None

# uvicorn configures this logger at INFO, so the owners' stop lines show
# under every uvicorn launch.
LOG = logging.getLogger("uvicorn.error")


@dataclass(frozen=True)
class Owner:
    name: str
    service: object
    # Exact process groups only this owner leads, signalled past its deadline.
    groups: Callable[[], Iterable[int]] | None = None
    # Live worker handles that stop themselves on owner loss; stop_owners
    # never signals them.
    workers: Callable[[], Iterable[object]] | None = None


async def _stop(service):
    # The outcome, captured as the exact exception an owner raised (a
    # cancellation an owner raises itself included), or None.
    try:
        await service.shutdown()
    except BaseException as error:
        return error
    return None


def _group_present(group):
    try:
        os.killpg(group, 0)
    except ProcessLookupError:
        return False
    return True  # Permission errors are not proof of absence.


async def signal_groups(groups, grace=GROUP_STOP_GRACE):
    """SIGTERM each exact group, then SIGKILL those still present after `grace`.

    Forge's own group, and ids that address no single group, are skipped. A
    cancellation ends the grace early; it never skips the SIGKILL. Returns
    the groups that received SIGTERM.
    """
    own = os.getpgrp()
    signalled = []
    for group in dict.fromkeys(groups):
        if type(group) is not int or group <= 1 or group == own:
            continue
        try:
            os.killpg(group, signal.SIGTERM)
        except (ProcessLookupError, PermissionError):
            continue
        signalled.append(group)
    remaining = list(signalled)
    deadline = time.monotonic() + grace
    try:
        while remaining and time.monotonic() < deadline:
            await asyncio.sleep(.05)
            remaining = [group for group in remaining if _group_present(group)]
    except asyncio.CancelledError:
        pass
    for group in remaining:
        try:
            os.killpg(group, signal.SIGKILL)
        except (ProcessLookupError, PermissionError):
            pass
    return signalled


def release_worker(process):
    """Leave a live worker to stop itself, without a signal from stop_owners.

    Closes its stdin, the owner loss it stops itself on, and marks its asyncio
    subprocess transport closed, so that neither an explicit close nor its
    collection at interpreter exit kills the worker. Returns False when this
    Python's transport cannot be released; asyncio then keeps its kill.
    """
    stdin = getattr(process, "stdin", None)
    if stdin is not None:
        try:
            stdin.close()
        except (OSError, RuntimeError):
            pass  # A pipe already closed, or a loop already closed, is owner loss too.
    if process.returncode is not None:
        return True
    transport = getattr(process, "_transport", None)
    if (sys.implementation.name == "cpython" and sys.version_info[:2] in RELEASE_VERSIONS
            and BaseSubprocessTransport is not None and isinstance(transport, BaseSubprocessTransport)
            and type(vars(transport).get("_closed")) is bool):
        transport._closed = True
        return True
    return False


def _release_workers(owner):
    try:
        live = [process for process in owner.workers() if process.returncode is None]
    except Exception as error:
        LOG.warning("Meta Forge: the workers of %s could not be listed (%s)", owner.name, type(error).__name__)
        return
    released = [process for process in live if release_worker(process)]
    if released:
        LOG.info("Meta Forge: %d %s worker(s) left to stop themselves", len(released), owner.name)
    if len(released) != len(live):
        LOG.warning("Meta Forge: %d %s worker(s) could not be released on this Python; Forge's exit may end them",
                    len(live) - len(released), owner.name)


async def stop_owners(owners, *, deadline=OWNER_STOP_DEADLINE, grace=GROUP_STOP_GRACE):
    """Stop each owner in order, each within `deadline` seconds.

    Returns the (name, error) failures. A stop past its deadline is that
    owner's TimeoutError: its task is cancelled and not awaited again, since
    owners keep waiting through a cancellation for work they must not
    abandon. A cancellation of this stop counts against the owner it
    interrupts, which is left the same way, and the remaining owners still
    stop.
    """
    failures = []
    for owner in owners:
        LOG.info("Meta Forge: stopping %s", owner.name)
        started = time.monotonic()
        task = asyncio.ensure_future(_stop(owner.service))
        interrupted = None
        try:
            await asyncio.wait({task}, timeout=deadline)
        except asyncio.CancelledError as error:
            interrupted = error
        if task.done():
            error = asyncio.CancelledError() if task.cancelled() else task.result()
            if error is None:
                LOG.info("Meta Forge: stopped %s in %.1f s", owner.name, time.monotonic() - started)
            else:
                LOG.warning("Meta Forge: stopping %s failed (%s)", owner.name, type(error).__name__)
                failures.append((owner.name, error))
        else:
            task.cancel()
            if interrupted is None:
                LOG.warning("Meta Forge: %s did not stop within %s s", owner.name, deadline)
                failures.append((owner.name, TimeoutError(f"{owner.name} did not stop within {deadline} s")))
            else:
                LOG.warning("Meta Forge: stopping %s failed (%s)", owner.name, type(interrupted).__name__)
                failures.append((owner.name, interrupted))
            if owner.groups is not None:
                try:
                    groups = list(owner.groups())
                except Exception as error:
                    LOG.warning("Meta Forge: the process groups of %s could not be listed (%s)",
                                owner.name, type(error).__name__)
                else:
                    signalled = await signal_groups(groups, grace)
                    if signalled:
                        LOG.warning("Meta Forge: signalled %d process group(s) of %s", len(signalled), owner.name)
        if owner.workers is not None:
            _release_workers(owner)
    return failures
