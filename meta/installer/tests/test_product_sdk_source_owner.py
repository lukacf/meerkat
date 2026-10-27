"""Selected SDK parent-task ownership with existing inert installer fixtures.

The production pair checker, byte installer, installed comparator, Supervisor
command and cleanup paths remain in use. The existing supervised fixture uses
only disposable stdlib children in place of pip and its poisoned interpreter;
no selected SDK, native gateway, provider or application source executes.
"""
import asyncio
from pathlib import Path
import sys
import threading

import pytest

from launchkit.templates import install_candidate as installed
from test_selected_sdk_supervisor import (
    assert_reaped, pair_files, sequence_files, supervised_files, wait_until,
)

from sdk_source_import_guard import no_selected_sdk_import


def _install_owned(files, require_owner_current):
    return files.owner.install_selected_sdk_pair(files.meta_path, files.toolkit_path,
        meta=files.meta[1], toolkit=files.toolkit[1], site_packages=files.site,
        sdk_python=files.python, require_current=files.require_current,
        dependency_requirements=files.dependency_requirements,
        dependency_wheels=files.dependency_wheels, timeout=5,
        require_owner_current=require_owner_current)


def _leaves(error):
    if isinstance(error, BaseExceptionGroup):
        for item in error.exceptions:
            yield from _leaves(item)
    else:
        yield error


async def test_selected_sdk_owner_callback_stays_on_original_task_through_real_readback(supervised_files):
    files = supervised_files
    files.cache = True
    loop = asyncio.get_running_loop()
    parent = asyncio.current_task()
    owner_thread = threading.get_ident()
    callbacks = []

    def original_owner():
        assert asyncio.get_running_loop() is loop
        assert asyncio.current_task() is parent
        assert threading.get_ident() == owner_thread
        callbacks.append(parent)

    result = await _install_owned(files, original_owner)
    assert result is files.verified
    assert len(callbacks) >= 3 and all(task is parent for task in callbacks)
    assert len(files.commands) == len(files.children) == 4
    assert len(files.probes) == 2
    assert any(thread != owner_thread for thread in files.fence_threads)
    assert files.owner.child is None and not files.owner.stop.is_set()
    assert files.owner._selected_sdk_install_task is None
    assert not list((files.root / ".venv").glob(".forge-selected-*"))
    assert (files.root / "retained.txt").read_bytes() == b"application state stays owned\n"
    assert not (files.root / ".venv/forge-sdk.receipt.json").exists()
    assert "fixture_sequence" not in sys.modules
    for child in files.children:
        assert_reaped(child)


@pytest.mark.parametrize("interruption", ["owner", "cancel"])
async def test_sdk_parent_owner_interruption_stops_and_joins_same_child_worker(supervised_files, interruption):
    files = supervised_files
    files.mode = "blocked"
    files.cache = True
    loop = asyncio.get_running_loop()
    owner_thread = threading.get_ident()
    original_task = None
    owner_current = True
    refusal = installed.CandidateArtifactError("Original selected SDK source owner changed.")
    callbacks = []
    cleanup_entered, cleanup_release = asyncio.Event(), asyncio.Event()
    cleanup_calls = []
    original_terminate = files.owner.terminate
    original_cleanup = files.owner.cleanup_child

    def original_owner():
        assert asyncio.get_running_loop() is loop
        assert asyncio.current_task() is original_task
        assert threading.get_ident() == owner_thread
        callbacks.append(original_task)
        if not owner_current:
            raise refusal

    async def retained_cleanup(process, tasks=(), **options):
        assert process is files.children[0]
        cleanup_calls.append((process, tuple(tasks)))
        return await original_cleanup(process, tasks, **options)

    async def held_terminate(timeout=None):
        assert files.owner.child is files.children[0]
        assert files.owner.stop.is_set(), "Stop must precede the original child drain."
        cleanup_entered.set()
        await cleanup_release.wait()
        return await original_terminate(timeout=timeout)

    files.owner.cleanup_child = retained_cleanup
    files.owner.terminate = held_terminate

    async def install():
        nonlocal original_task
        original_task = asyncio.current_task()
        return await _install_owned(files, original_owner)

    task = asyncio.create_task(install())
    error = None
    staged = None
    entered_worker = None
    try:
        await wait_until(lambda: bool(files.children))
        child = files.children[0]
        staged = Path(files.commands[0][0][-1])
        entered_worker = files.owner._selected_sdk_install_task
        assert staged.is_file() and entered_worker is not None and not entered_worker.done()
        assert child.returncode is None and not task.done()
        if interruption == "owner":
            owner_current = False
        else:
            task.cancel()
        await asyncio.wait_for(cleanup_entered.wait(), 30)
        assert files.owner.stop.is_set()
        assert not task.done() and not entered_worker.done()
        assert files.owner._selected_sdk_install_task is entered_worker
        assert files.owner.child is child and child.returncode is None
        assert staged.is_file() and len(files.commands) == len(files.children) == 1
        assert len(cleanup_calls) == 1 and cleanup_calls[0][0] is child
        drains = cleanup_calls[0][1]
        assert len(drains) == 4 and all(isinstance(drain, asyncio.Task) for drain in drains)
        assert any(not drain.done() for drain in drains)
        with pytest.raises(RuntimeError, match="previous owned child"):
            await _install_owned(files, original_owner)
        assert files.probes == []
        if interruption == "cancel":
            task.cancel()
            await asyncio.sleep(0.01)
            assert not task.done() and not entered_worker.done()
            assert files.owner.child is child and staged.is_file()
            assert cleanup_calls == [(child, drains)]
        cleanup_release.set()
        try:
            await asyncio.wait_for(asyncio.shield(task), 4)
        except BaseException as failure:
            error = failure
        else:
            pytest.fail("An interrupted original SDK owner cannot return success.")
    finally:
        cleanup_release.set()
        if not task.done():
            files.owner.stop.set()
            task.cancel()
            await asyncio.wait_for(asyncio.gather(task, return_exceptions=True), 4)

    assert error is not None
    if interruption == "owner":
        assert any(leaf is refusal for leaf in _leaves(error)), (
            "Worker cancellation must not discard the original source-owner refusal."
        )
    else:
        assert any(isinstance(leaf, asyncio.CancelledError) for leaf in _leaves(error))
    assert callbacks and all(callback is original_task for callback in callbacks)
    assert any(thread != owner_thread for thread in files.fence_threads)
    assert entered_worker.done() and files.owner._selected_sdk_install_task is None
    assert files.owner.child is None and files.owner.stop.is_set()
    assert len(files.commands) == len(files.children) == 1 and files.probes == []
    assert all(drain.done() for drain in cleanup_calls[0][1])
    assert staged is not None and not staged.exists()
    assert not list((files.root / ".venv").glob(".forge-selected-*"))
    assert (files.root / "retained.txt").read_bytes() == b"application state stays owned\n"
    assert not (files.root / ".venv/forge-sdk.receipt.json").exists()
    assert "fixture_sequence" not in sys.modules
    assert_reaped(files.children[0])
