"""The final guide fence also protects continuation of saved application work."""
import asyncio
import sqlite3
from types import SimpleNamespace

import pytest

from launchkit.implementation import update_receipt
from launchkit.implementation_workspace import ImplementationWorkspace
from launchkit.projects import DraftConflict
from test_implementation import decision_brief, service, workspace


@pytest.mark.parametrize("journal_mode", ["delete", "wal"])
@pytest.mark.parametrize("resume", [False, True], ids=["build", "resume"])
async def test_publication_excludes_saved_guide_writers(service, monkeypatch, journal_mode, resume):
    import launchkit.implementation as implementation_module
    from launchkit.guide import GuideService
    from launchkit.guide_store import GuideStore

    store = GuideStore(service.root.parent / "guide")
    created = store.create({**service.guide.state, "brief": decision_brief()})
    database = store.root / "guided-setups.sqlite3"
    with sqlite3.connect(database) as db:
        assert db.execute(f"PRAGMA journal_mode={journal_mode}").fetchone()[0] == journal_mode
    service.guide = GuideService(store, SimpleNamespace())
    selected = None
    try:
        if resume:
            first = await service.submit(created["id"], 1, expected_selected_build_id=None)
            selected = first["build_id"]
            task = service.tasks.pop(selected)
            task.cancel()
            await asyncio.gather(task, return_exceptions=True)
            update_receipt(service._directory(created["id"]), status="failed", ready_to_package=False)

        excluded = []

        def require_writer_excluded(publication):
            writer = sqlite3.connect(database, timeout=0)
            try:
                with pytest.raises(sqlite3.OperationalError, match="locked"):
                    writer.execute("UPDATE guided_setups SET body=body WHERE id=?", (created["id"],))
                excluded.append(publication)
            finally:
                writer.close()

        original_write = implementation_module.write_private

        def publish(target, value):
            if target.name in {"receipt.json", "latest.json"}:
                require_writer_excluded(target.name)
            return original_write(target, value)

        class PublishedTasks(dict):
            def __setitem__(self, identity, task):
                # Retain custody even if the assertion fails so shutdown can
                # cancel the inert worker in the mutation control.
                super().__setitem__(identity, task)
                require_writer_excluded("task")

        service.tasks = PublishedTasks(service.tasks)
        with monkeypatch.context() as publication_patch:
            publication_patch.setattr(implementation_module, "write_private", publish)
            admitted = await service.submit(created["id"], 1, expected_selected_build_id=selected)
        assert excluded == ["receipt.json", "latest.json", "task"]
        assert admitted["build_id"] in service.tasks
        assert store.get(created["id"]) == created
        # A writer can commit after admission; the reservation did not leak.
        with sqlite3.connect(database, timeout=0) as writer:
            assert writer.execute("UPDATE guided_setups SET body=body WHERE id=?", (created["id"],)).rowcount == 1
    finally:
        await service.shutdown()


async def test_resumed_build_rejects_late_guide_change_without_replacing_saved_source(service, monkeypatch):
    first = await service.submit(service.guide.identity, 3, expected_selected_build_id=None)
    task = service.tasks.pop(first["build_id"])
    task.cancel()
    await asyncio.gather(task, return_exceptions=True)
    directory = service._directory(service.guide.identity)
    update_receipt(directory, status="failed", ready_to_package=False)
    project = workspace(service, first)
    source = "def build(context):\n    return 'preserve this unfinished work'\n"
    project.write_file("application/__init__.py", source)
    saved_input = (directory / "input.json").read_bytes()
    selected = (directory.parent / "latest.json").read_bytes()
    original = ImplementationWorkspace.require_preparation_current
    reached_final_fence = False

    def preparation_current(prepared):
        nonlocal reached_final_fence
        original(prepared)
        reached_final_fence = True
        # Both async guide reads have returned and the copied-source lock is
        # held. The publication fence must still read the actual guide owner.
        service.guide.state["version"] += 1

    monkeypatch.setattr(ImplementationWorkspace, "require_preparation_current", preparation_current)
    try:
        with pytest.raises(DraftConflict, match="setup changed"):
            await service.submit(service.guide.identity, 3, "Repair the saved source",
                                 expected_selected_build_id=first["build_id"])
        assert reached_final_fence
        assert not service.tasks and not service.processes
        assert (directory.parent / "latest.json").read_bytes() == selected
        assert (directory / "input.json").read_bytes() == saved_input
        assert project.read_file("application/__init__.py") == source
        assert len(list(service.root.glob("*/*/receipt.json"))) == 1
    finally:
        await service.shutdown()
