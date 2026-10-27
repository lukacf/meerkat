"""Checker admission regressions using the existing inert builder fixture."""
import pytest

from launchkit.architect import ArchitectUnavailable
from test_implementation import complete, service


class Checker:
    def __init__(self, status):
        self.current = status
        self.reads = 0

    async def status(self):
        self.reads += 1
        return {"status": self.current}


def admit_with_fake_sandbox(monkeypatch):
    # Admission only checks presence; the Service fixture never starts a worker.
    monkeypatch.setattr("launchkit.implementation_workspace._sandbox_executable",
                        lambda: "/fictional/host/sandbox")


async def test_missing_checker_refuses_before_workspace_or_worker(service, monkeypatch):
    admit_with_fake_sandbox(monkeypatch)
    checker = service.checker_setup = Checker("needs_preparation")
    with pytest.raises(ArchitectUnavailable, match="No builder was started"):
        await service.submit(service.guide.identity, 3, "Build the proposed application")
    assert checker.reads == 1
    assert not service.root.exists()
    assert not service.tasks and not service.processes


async def test_ready_checker_admits_one_inert_owned_build(service, monkeypatch):
    admit_with_fake_sandbox(monkeypatch)
    checker = service.checker_setup = Checker("ready")
    try:
        first = await service.submit(service.guide.identity, 3, "Build the proposed application")
        directory = service._directory(service.guide.identity)
        assert directory.name == first["build_id"]
        assert (directory / "receipt.json").is_file()
        assert (directory / "input.json").is_file()
        assert checker.reads == 1
        assert set(service.tasks) == {first["build_id"]}
        assert not service.processes
    finally:
        await service.shutdown()


async def test_active_duplicate_reconnects_when_checker_later_unavailable(service, monkeypatch):
    admit_with_fake_sandbox(monkeypatch)
    checker = service.checker_setup = Checker("ready")
    try:
        first = await service.submit(service.guide.identity, 3, "Build the proposed application")
        task = service.tasks[first["build_id"]]
        directory = service._directory(service.guide.identity)
        before = (directory / "receipt.json").read_bytes()
        checker.current = "unavailable"
        recovered = await service.submit(service.guide.identity, 3, "Build the proposed application",
                                         expected_selected_build_id=first["build_id"])
        assert recovered["build_id"] == first["build_id"]
        assert checker.reads == 1
        assert service.tasks == {first["build_id"]: task}
        assert not service.processes
        assert (directory / "receipt.json").read_bytes() == before
        assert [path.name for path in directory.parent.iterdir() if path.is_dir()] == [first["build_id"]]
    finally:
        await service.shutdown()


async def test_completed_duplicate_preserves_checked_build_without_checker_read(service):
    from launchkit.implementation_workspace import _sandbox_executable
    if not _sandbox_executable():
        pytest.skip("Existing protected completion fixture requires the host sandbox.")
    checker = service.checker_setup = Checker("ready")
    try:
        first = await service.submit(service.guide.identity, 3)
        await complete(service, first)
        accepted = await service.status(service.guide.identity)
        assert accepted["ready_to_package"]
        directory = service._directory(service.guide.identity)
        before = (directory / "receipt.json").read_bytes()
        checker.current = "unavailable"
        recovered = await service.submit(service.guide.identity, 3,
                                         expected_selected_build_id=first["build_id"])
        assert recovered["build_id"] == first["build_id"]
        assert recovered["ready_to_package"]
        assert recovered["source_digest"] == accepted["source_digest"]
        assert checker.reads == 1
        assert not service.tasks and not service.processes
        assert (directory / "receipt.json").read_bytes() == before
        assert [path.name for path in directory.parent.iterdir() if path.is_dir()] == [first["build_id"]]
    finally:
        await service.shutdown()
