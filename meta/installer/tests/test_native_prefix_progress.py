"""Progress from existing inert copy/readback owners, not runtime readiness."""
import asyncio
import re
import threading
from types import SimpleNamespace

import pytest

from launchkit import deployment_worker as worker, runtime_candidates as candidates
from launchkit import runtime_dependencies as runtime
from launchkit.templates import install_candidate as adapter
from test_meta_toolkit_runtime_selection import product, no_product_execution
from test_product_relocated_runtime import installed_source, destination_source, artifact_custody, no_destination_execution


@pytest.fixture
def prefix(product, tmp_path):
    artifacts = candidates.read_meta_toolkit_artifacts(product.target)
    destination = tmp_path / "private-path-must-not-appear-in-progress"
    destination.mkdir(mode=0o700)
    return SimpleNamespace(artifacts=artifacts, destination=destination,
        files=[entry for entry in artifacts.entries if entry.kind == "file"])


def install(fixture, messages, **kwargs):
    return adapter.install_meta_toolkit_prefix(fixture.artifacts,
        deployment_root=fixture.destination, require_current=lambda: None,
        report_progress=messages.append, **kwargs)


def final_count(fixture, action):
    return f"{action}: {len(fixture.files)} files, {sum(entry.size for entry in fixture.files)} bytes completed."


def test_real_copy_and_readback_report_completed_bytes_without_paths_or_readiness(prefix, monkeypatch):
    messages = []
    monkeypatch.setattr(adapter, "time", SimpleNamespace(monotonic=lambda: 0))
    gateway = install(prefix, messages)
    assert adapter.digest(gateway.read_bytes()) == next(
        entry.sha256 for entry in prefix.files if entry.name.endswith("toolkit_gateway"))
    assert messages == [
        "Checking the selected native runtime archive.",
        "Copying native runtime files: 0 files, 0 bytes completed.",
        final_count(prefix, "Copying native runtime files"),
        "Checking native runtime files: 0 files, 0 bytes completed.",
        final_count(prefix, "Checking native runtime files"),
    ]
    assert all("private-path" not in value and "ready" not in value and "%" not in value for value in messages)
    adapter.verify_meta_toolkit_prefix(prefix.artifacts, deployment_root=prefix.destination)


def test_progress_is_throttled_between_completed_operations_with_final_remainder(prefix, monkeypatch):
    clock, observations = [0.0], []
    monkeypatch.setattr(adapter, "time", SimpleNamespace(monotonic=lambda: clock[0]))
    original_write = adapter.atomic_write
    def copy(path, data, mode):
        original_write(path, data, mode)
        clock[0] += 0.75
    monkeypatch.setattr(adapter, "atomic_write", copy)
    adapter.install_meta_toolkit_prefix(prefix.artifacts, deployment_root=prefix.destination,
        require_current=lambda: None, report_progress=lambda message: observations.append((clock[0], message)))
    copies = [(at, message) for at, message in observations if message.startswith("Copying native runtime files:")]
    assert len(copies) < len(prefix.files)
    assert [message.split(": ", 1)[1].split(" files", 1)[0] for _, message in copies] == ["0", "3", "6", "7"]
    assert all(later[0] - earlier[0] >= 2 for earlier, later in zip(copies[:-2], copies[1:-1]))
    assert copies[-1][1] == final_count(prefix, "Copying native runtime files")


def test_existing_prefix_reports_verification_without_claiming_another_copy(prefix, monkeypatch):
    install(prefix, [])
    messages = []
    monkeypatch.setattr(adapter, "time", SimpleNamespace(monotonic=lambda: 0))
    install(prefix, messages)
    assert not any(value.startswith("Copying native runtime files") for value in messages)
    assert messages[-1] == final_count(prefix, "Checking native runtime files")


def test_failed_write_is_not_counted_and_does_not_reach_verification(prefix, monkeypatch):
    messages = []
    def refuse(*args):
        raise OSError("inert write failure")
    monkeypatch.setattr(adapter, "atomic_write", refuse)
    with pytest.raises(OSError, match="inert write failure"):
        install(prefix, messages)
    assert messages == ["Checking the selected native runtime archive.",
        "Copying native runtime files: 0 files, 0 bytes completed."]


def test_failed_hash_is_not_counted_or_recast_as_completed_verification(prefix, monkeypatch):
    install(prefix, [])
    first = sorted(prefix.files, key=lambda entry: entry.name)[0]
    path = prefix.destination / ".bin" / first.name
    path.chmod(0o600)
    path.write_bytes(b"x" * first.size)
    path.chmod(first.mode)
    messages = []
    monkeypatch.setattr(adapter, "time", SimpleNamespace(monotonic=lambda: 0))
    with pytest.raises(candidates.CandidateArtifactError, match="file changed"):
        adapter.verify_meta_toolkit_prefix(prefix.artifacts, deployment_root=prefix.destination,
            report_progress=messages.append)
    assert messages == ["Checking native runtime files: 0 files, 0 bytes completed."]


def test_owner_refusal_precedes_progress_and_any_destination_effect(prefix):
    messages = []
    with pytest.raises(candidates.CandidateArtifactError, match="owner fence"):
        adapter.install_meta_toolkit_prefix(prefix.artifacts, deployment_root=prefix.destination,
            require_current=lambda: False, report_progress=messages.append)
    assert messages == [] and list(prefix.destination.iterdir()) == []


def test_progress_callback_failure_remains_failure_and_cannot_publish_success(prefix):
    def refused(message):
        raise RuntimeError("inert original progress owner refused")
    with pytest.raises(RuntimeError, match="original progress owner"):
        adapter.install_meta_toolkit_prefix(prefix.artifacts, deployment_root=prefix.destination,
            require_current=lambda: None, report_progress=refused)
    assert list(prefix.destination.iterdir()) == []


async def test_cancel_during_owned_copy_joins_worker_and_stops_subsequent_writes(prefix, monkeypatch):
    supervisor = SimpleNamespace(stop=asyncio.Event())
    entered, release, settled = threading.Event(), threading.Event(), threading.Event()
    writes, messages = [], []
    original_write = adapter.atomic_write
    def paused_write(path, data, mode):
        original_write(path, data, mode)
        writes.append(path)
        entered.set()
        assert release.wait(3), "The inert copy test must release its own worker."
    monkeypatch.setattr(adapter, "atomic_write", paused_write)
    def current():
        if supervisor.stop.is_set():
            raise asyncio.CancelledError
    def mutation():
        try:
            return adapter.install_meta_toolkit_prefix(prefix.artifacts, deployment_root=prefix.destination,
                require_current=current, report_progress=messages.append)
        finally:
            settled.set()
    task = asyncio.create_task(worker._joined_product_mutation(supervisor, mutation, _owner_current=current))
    try:
        assert await asyncio.to_thread(entered.wait, 30)
        task.cancel()
        await asyncio.sleep(0)
        assert supervisor.stop.is_set() and not task.done()
    finally:
        release.set()
    with pytest.raises(asyncio.CancelledError):
        await task
    assert settled.is_set() and len(writes) == 1
    assert not any(value.startswith("Checking native runtime files:") for value in messages)
    assert final_count(prefix, "Copying native runtime files") not in messages


def test_existing_runtime_proof_route_delivers_readback_progress_without_readiness(installed_source, monkeypatch):
    f, messages = installed_source, []
    monkeypatch.setattr(adapter, "time", SimpleNamespace(monotonic=lambda: 0))
    proof = runtime.verify_runtime(f.selected.destination, f.version,
        runtime_target=f.selected.custody.plan.origin, sdk_python=f.python,
        site_packages=f.site, report_progress=messages.append)
    assert messages[0] == "Checking native runtime files: 0 files, 0 bytes completed."
    assert re.fullmatch(r"Checking native runtime files: [1-9][0-9]* files, [1-9][0-9]* bytes completed\.", messages[-1])
    assert proof["source"] == "meta-toolkit-product"
    assert not any(proof.get(key) is True for key in ("ready", "installed", "qualified", "native_ready"))
