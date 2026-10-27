"""Stage keeps the event loop free and reads each selected product once.

Inert product, package and owner fixtures only. No application, SDK, provider
or native program runs. A slowed selected product read stands in for a large
native product on a small host.
"""
import asyncio
from collections import Counter
import json
import time

import pytest

from launchkit import deployment, deployment_updates as updates, runtime_candidates
from launchkit.local_application_registry import LocalApplicationRegistry
from launchkit.projects import DraftConflict
from test_candidate_deployment_updates import installation, packed
from test_deployment_updates import prepared
from test_local_application_registry import (registry, new_guide, build, accept_source_fixture,
    install_fixture, no_processes)
from test_meta_toolkit_initial_package import inputs
from test_meta_toolkit_preserved_refresh import crossing, original_receipt, products
from test_meta_toolkit_runtime_selection import product, no_product_execution
from test_update_after_setup_registry import set_up, continuation, stage as stage_after_setup

# Each slowed read lasts SLOWED_READ_SECONDS, so a read on the event loop
# leaves a gap at least that long. The bound leaves room for the short
# synchronous publication, which writes and syncs the update's own files on
# the loop after the final fences (about 0.3-0.6 s on a busy host).
SLOWED_READ_SECONDS = 2.0
LOOP_GAP_BOUND = 1.0


def counted_product_reads(monkeypatch, *, delay=0.0):
    """Count full selected product reads by target; optionally slow each one."""
    reads = Counter()
    original = runtime_candidates._read_meta_toolkit_artifacts
    def counted(target, *args, **kwargs):
        reads[target] += 1
        if delay:
            time.sleep(delay)  # A large native product on a small host.
        return original(target, *args, **kwargs)
    monkeypatch.setattr(runtime_candidates, "_read_meta_toolkit_artifacts", counted)
    return reads


async def longest_loop_gap(operation):
    """Run operation while another request waits on the loop; return its result and the longest gap."""
    gaps, done = [], asyncio.Event()
    async def other_request():
        while not done.is_set():
            started = time.monotonic()
            await asyncio.sleep(0.01)
            gaps.append(time.monotonic() - started)
    probe = asyncio.create_task(other_request())
    await asyncio.sleep(0)  # The other request is waiting on the loop.
    try:
        result = await operation
    finally:
        done.set()
        await probe
    return result, max(gaps)


def staged_product_refresh(tmp_path, crossing):
    _, target, previous, candidate, _, new_report = crossing
    directory, destination = installation(tmp_path, crossing)
    receipt = updates.read_receipt(directory)
    receipt.update(original_receipt(previous))
    updates.write_private(directory / "receipt.json", receipt)
    return directory, target, packed(candidate), new_report


def test_product_runtime_refresh_stage_reads_each_selected_product_once(crossing, tmp_path, monkeypatch):
    old_product, target, _, _, _, _ = crossing
    directory, target, payload, report = staged_product_refresh(tmp_path, crossing)
    reads = counted_product_reads(monkeypatch)
    staged = updates.stage(directory, payload, report, runtime_target=target)["update"]
    assert staged["status"] == "staged" and staged["blockers"] == []
    assert staged["runtime_dependency_update"] is not None
    # Exactly two full reads, not 12: the new product and the installed prior
    # product are different files, and each is read in full once. Every later
    # use only rechecks their fingerprints.
    assert reads == {target: 1, old_product.target: 1}
    assert sum(reads.values()) == 2


def test_prepared_stage_writes_nothing_until_publication(crossing, tmp_path):
    directory, target, payload, report = staged_product_refresh(tmp_path, crossing)
    before = {path: path.read_bytes() for path in directory.rglob("*") if path.is_file()}
    ready = updates.prepare_stage(directory, payload, report, runtime_target=target)
    assert {path: path.read_bytes() for path in directory.rglob("*") if path.is_file()} == before
    staged = updates.publish_stage(directory, ready)["update"]
    assert staged == ready.transaction and staged["status"] == "staged"
    root = updates.update_directory(directory, staged["update_id"])
    assert (root / "candidate.zip").read_bytes() == payload
    assert json.loads((root / "update.json").read_text()) == staged


@pytest.mark.parametrize("change", ["status", "package", "staged-update"])
def test_prepared_stage_refuses_an_installation_change_before_publication(crossing, tmp_path, change):
    directory, target, payload, report = staged_product_refresh(tmp_path, crossing)
    ready = updates.prepare_stage(directory, payload, report, runtime_target=target)
    if change == "status":
        updates.update_receipt(directory, status="running", runtime_pid=4242)
    elif change == "package":
        updates.update_receipt(directory, package_sha256="0" * 64)
    else:
        updates.publish_stage(directory, updates.prepare_stage(directory, payload, report, runtime_target=target))
    before = updates.read_receipt(directory)
    roots = set((directory / "updates").glob("*")) if (directory / "updates").exists() else set()
    with pytest.raises(DraftConflict, match="changed while its update was being prepared"):
        updates.publish_stage(directory, ready)
    assert updates.read_receipt(directory) == before
    assert (set((directory / "updates").glob("*")) if (directory / "updates").exists() else set()) == roots


async def test_source_update_stage_leaves_the_event_loop_free(tmp_path, monkeypatch):
    service, directory, _, previous, _ = await prepared(tmp_path)
    for module in (deployment, updates):
        original = module.package_files
        def slowed(payload, original=original):
            time.sleep(SLOWED_READ_SECONDS)  # A large package on a small host.
            return original(payload)
        monkeypatch.setattr(module, "package_files", slowed)
    receipt, gap = await longest_loop_gap(service.stage_update(service.guide.identity,
        previous["deployment_id"], service.guide.state["version"], service.implementation.identity))
    assert receipt["update"]["status"] == "staged"
    assert gap < LOOP_GAP_BOUND, gap
    await service.shutdown()


async def test_set_up_product_source_update_stage_leaves_the_event_loop_free(set_up, monkeypatch):
    f = set_up
    second = await continuation(f)
    reads = counted_product_reads(monkeypatch, delay=SLOWED_READ_SECONDS)
    update, gap = await longest_loop_gap(stage_after_setup(f, second["build_id"]))
    assert update["status"] == "staged" and update["blockers"] == []
    assert update["setup_restore_sha256"] == f.restore.sha256
    assert set(reads) == {f.target} and gap < LOOP_GAP_BOUND, (reads, gap)


async def test_product_runtime_refresh_stage_leaves_the_event_loop_free(registry, products, monkeypatch):
    f = registry
    saved = new_guide(f)
    first = await build(f, saved)
    await accept_source_fixture(f, first)
    directory, _ = await install_fixture(f, saved, first, monkeypatch)
    new_target = products[1].target
    # The restarted launcher selects the newer product for a runtime refresh.
    current_host = LocalApplicationRegistry(guide_store=f.store, implementation=f.implementation,
        deployment=f.service, runtime_target=new_target)
    f.implementation.prepare_workspace = current_host.prepare_workspace
    saved = f.store.get(saved["id"])
    refreshed = await f.implementation.submit(saved["id"], saved["version"], refresh_framework=True,
        preserve_design=True, expected_selected_build_id=first["build_id"], runtime_target=new_target)
    await accept_source_fixture(f, refreshed)
    reads = counted_product_reads(monkeypatch, delay=SLOWED_READ_SECONDS)
    receipt, gap = await longest_loop_gap(f.service.stage_update(saved["id"], directory.name,
        f.store.get(saved["id"])["version"], refreshed["build_id"]))
    update = receipt["update"]
    assert update["runtime_dependency_update"] is not None, update
    # The new product is read in full once. The installed product's read may
    # be kept from its Install checks, then only its fingerprints are rechecked.
    assert reads[new_target] == 1 and reads[f.target] <= 1, dict(reads)
    assert gap < LOOP_GAP_BOUND, (gap, dict(reads))
