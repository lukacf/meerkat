"""Status polls of a staged product runtime refresh do not read the product again.

Inert registry, product and package fixtures only. No worker, application, SDK,
provider or native program runs. A slowed product read stands in for a large
native product on a small host.
"""
import time

from launchkit import runtime_candidates
from test_local_application_registry import (registry, new_guide, build, accept_source_fixture,
    install_fixture, no_processes)
from test_meta_toolkit_preserved_refresh import products
from test_meta_toolkit_runtime_selection import product, no_product_execution
from test_runtime_update_admission_interpreter import staged_product_refresh

SLOWED_READ_SECONDS = 2.0


async def test_status_polls_of_a_staged_refresh_reuse_the_verified_product_reads(registry, products, monkeypatch):
    f = registry
    guide_id, directory, update = await staged_product_refresh(f, products, monkeypatch)
    reads = []
    original = runtime_candidates._read_meta_toolkit_artifacts
    def counted(target, *args, **kwargs):
        reads.append(target)
        time.sleep(SLOWED_READ_SECONDS)  # A large native product on a small host.
        return original(target, *args, **kwargs)
    monkeypatch.setattr(runtime_candidates, "_read_meta_toolkit_artifacts", counted)
    seconds = []
    for _ in range(3):
        started = time.monotonic()
        observed = await f.service.status(guide_id)
        seconds.append(time.monotonic() - started)
        assert observed["update"]["update_id"] == update["update_id"]
        assert observed["update"]["status"] == "staged"
    # Every poll re-verifies the staged transition, from the products' verified
    # reads kept since Stage: each input's fingerprint is rechecked, none is
    # read in full again (it was 4 full reads per poll).
    assert reads == [] and max(seconds) < SLOWED_READ_SECONDS, (len(reads), seconds)


async def test_a_staged_refresh_reads_a_changed_product_again(registry, products, monkeypatch):
    f = registry
    guide_id, directory, update = await staged_product_refresh(f, products, monkeypatch)
    reads = []
    original = runtime_candidates._read_meta_toolkit_artifacts
    def counted(target, *args, **kwargs):
        reads.append(target)
        return original(target, *args, **kwargs)
    monkeypatch.setattr(runtime_candidates, "_read_meta_toolkit_artifacts", counted)
    # Same bytes, a new file identity: the kept read no longer passes its
    # fingerprint recheck, so the next poll reads and verifies it in full.
    import os
    archive = next(products[1].target.artifact_root.rglob("*.tar.gz"))
    later = archive.stat().st_mtime_ns + 5_000_000_000
    os.utime(archive, ns=(later, later))
    observed = await f.service.status(guide_id)
    assert observed["update"]["status"] == "staged"
    assert products[1].target in reads
