"""The final guide fence also protects continuation of saved application work."""
import asyncio

import pytest

from launchkit.implementation import update_receipt
from launchkit.implementation_workspace import ImplementationWorkspace
from launchkit.projects import DraftConflict
from test_implementation import service, workspace


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
