"""Bounded archive inventory with all byte and confinement checks retained."""
from dataclasses import replace
import hashlib
import tracemalloc

import pytest

from launchkit import runtime_candidates as candidates
from test_meta_toolkit_runtime_selection import prefix_bytes


def archive(tmp_path, nodes):
    content = prefix_bytes(nodes)
    path = tmp_path / "product.tar.gz"
    path.write_bytes(content)
    return candidates.ArtifactFile(path, len(content), hashlib.sha256(content).hexdigest()), content


def test_streamed_inventory_matches_bytes_without_archive_sized_allocation(tmp_path):
    # Incompressible data makes retaining the compressed input observable.
    import random
    payload = random.Random(710).randbytes(12 * 1024 * 1024)
    nodes = {"product": ("directory", 0o755, None),
        "product/data": ("file", 0o644, payload),
        "product/current": ("link", 0o777, "data")}
    artifact, content = archive(tmp_path, nodes)
    expected = candidates.product_prefix_entries(content, "product")
    inputs = candidates._Inputs(tmp_path)
    tracemalloc.start()
    try:
        actual = inputs.product_entries(artifact, "product")
        _, peak = tracemalloc.get_traced_memory()
    finally:
        tracemalloc.stop()
    assert actual == expected
    assert peak < 6 * 1024 * 1024
    inputs.finish()
    assert inputs.files[artifact.path][0] == artifact


@pytest.mark.parametrize("change", ["hash", "size", "truncated", "escaping"])
def test_streamed_inventory_preserves_refusals(tmp_path, change):
    name = "product/../escape" if change == "escaping" else "product/data"
    artifact, content = archive(tmp_path, {"product": ("directory", 0o755, None),
        name: ("file", 0o644, b"actual bytes")})
    if change == "hash":
        artifact = replace(artifact, sha256="0" * 64)
    elif change == "size":
        artifact = replace(artifact, size=artifact.size + 1)
    elif change == "truncated":
        content = content[:-8]
        artifact.path.write_bytes(content)
        artifact = replace(artifact, size=len(content), sha256=hashlib.sha256(content).hexdigest())
    with pytest.raises((candidates.CandidateArtifactError, EOFError)):
        candidates._Inputs(tmp_path).product_entries(artifact, "product")


def test_streamed_inventory_keeps_changed_file_fence(tmp_path):
    artifact, _ = archive(tmp_path, {"product": ("directory", 0o755, None),
        "product/data": ("file", 0o644, b"actual bytes")})
    inputs = candidates._Inputs(tmp_path)
    inputs.product_entries(artifact, "product")
    artifact.path.write_bytes(b"changed")
    with pytest.raises(candidates.CandidateArtifactError, match="changed"):
        inputs.finish()
