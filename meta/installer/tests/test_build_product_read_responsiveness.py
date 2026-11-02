"""Full product reads stay off-loop; final admission retains currentness fences."""
import asyncio
import os

import pytest

from launchkit import runtime_candidates
from test_meta_toolkit_initial_package import (
    service, product, inputs, prepare, setup_spec, catalog_target, no_product_execution,
)


async def test_build_never_reads_product_archive_on_event_loop(service, product, inputs, monkeypatch):
    service.guide.state["proposal"] = setup_spec()
    target = catalog_target(product)
    prepare(service, inputs, monkeypatch)
    original = runtime_candidates._read_meta_toolkit_artifacts
    reads = []

    def checked(*args, **kwargs):
        with pytest.raises(RuntimeError, match="no running event loop"):
            asyncio.get_running_loop()
        reads.append(args[0])
        return original(*args, **kwargs)

    monkeypatch.setattr(runtime_candidates, "_read_meta_toolkit_artifacts", checked)
    try:
        result = await service.submit(service.guide.identity, 3, runtime_target=target)
        assert result["build_id"]
        assert reads
    finally:
        await service.shutdown()


async def test_final_build_admission_rejects_changed_verified_product(service, product, inputs, monkeypatch):
    service.guide.state["proposal"] = setup_spec()
    target = catalog_target(product)
    prepared = []
    prepare(service, inputs, monkeypatch, after=lambda _: prepared.append(True))
    original = service._guide

    async def changed(identity):
        state = await original(identity)
        if prepared:
            path = target.assembly.path
            info = path.stat()
            os.utime(path, ns=(info.st_atime_ns, info.st_mtime_ns + 1_000_000_000))
        return state

    monkeypatch.setattr(service, "_guide", changed)
    with pytest.raises(runtime_candidates.CandidateArtifactError, match="changed"):
        await service.submit(service.guide.identity, 3, runtime_target=target)
    assert not service.tasks
    assert not (service.root / service.guide.identity / "latest.json").exists()
