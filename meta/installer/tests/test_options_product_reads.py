"""Options and install checks read a selected product once while it is unchanged.

Inert product, build and package fixtures only. No worker, application, SDK,
provider or native program runs. A slowed product read stands in for a large
native product on a small host.
"""
import asyncio
import os
import time

from launchkit import runtime_candidates
from test_candidate_implementation import service as implementation_service
from test_meta_toolkit_initial_install import initial, no_processes
from test_meta_toolkit_runtime_selection import product, no_product_execution


def counted_product_reads(monkeypatch, *, delay=0.0):
    reads = []
    original = runtime_candidates._read_meta_toolkit_artifacts
    def counted(target, *args, **kwargs):
        reads.append(target)
        if delay:
            time.sleep(delay)  # A large native product on a small host.
        return original(target, *args, **kwargs)
    monkeypatch.setattr(runtime_candidates, "_read_meta_toolkit_artifacts", counted)
    return reads


async def test_repeated_options_read_the_selected_product_once(initial, monkeypatch):
    f = initial
    reads = counted_product_reads(monkeypatch)
    first = await f.service.options(f.guide_id, f.build_id)
    second = await f.service.options(f.guide_id, f.build_id)
    assert first["fresh_install"] == second["fresh_install"] == {
        "available": True, "reason": None, "next_step": "configure"}
    # The second options call rechecks the verified inputs' fingerprints only.
    assert reads == [f.target]


async def test_concurrent_options_share_one_product_read(initial, monkeypatch):
    f = initial
    reads = counted_product_reads(monkeypatch, delay=0.5)
    first, second = await asyncio.gather(f.service.options(f.guide_id, f.build_id),
        f.service.options(f.guide_id, f.build_id))
    assert first["fresh_install"]["available"] and second["fresh_install"]["available"]
    assert reads == [f.target]


async def test_a_changed_product_input_is_read_again_in_full(initial, monkeypatch):
    f = initial
    reads = counted_product_reads(monkeypatch)
    await f.service.options(f.guide_id, f.build_id)
    # Same bytes, a new file identity: the fingerprint recheck refuses the kept
    # read, and the product is read and verified again in full.
    archive = next(f.target.artifact_root.rglob("*.tar.gz"))
    later = archive.stat().st_mtime_ns + 5_000_000_000
    os.utime(archive, ns=(later, later))
    options = await f.service.options(f.guide_id, f.build_id)
    assert options["fresh_install"]["available"] is True
    assert reads == [f.target, f.target]
