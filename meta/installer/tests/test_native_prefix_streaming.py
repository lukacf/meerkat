"""Deterministic archive traversal controls with inert producer-shaped bytes."""
from contextlib import contextmanager
from copy import copy
from dataclasses import replace
import gzip
import io
import os
import random
import stat
import tarfile
import zlib

import pytest

from launchkit import runtime_candidates as candidates
from launchkit.templates import install_candidate as adapter
from test_meta_toolkit_runtime_selection import (
    product, no_product_execution, prefix_bytes, put, json_bytes,
)


def select_archive(product, raw):
    archive = put(product.archive.path, raw)
    product.assembly.update(archive_sha256=archive.sha256, archive_bytes=archive.size)
    target = replace(product.target, assembly=put(product.target.assembly.path, json_bytes(product.assembly)))
    return candidates.read_meta_toolkit_artifacts(target)


def ordered_bytes(nodes, names):
    output = io.BytesIO()
    with gzip.GzipFile(filename="", fileobj=output, mode="wb", mtime=0) as zipped:
        with tarfile.open(fileobj=zipped, mode="w", format=tarfile.PAX_FORMAT) as archive:
            for name in names:
                kind, mode, value = nodes[name]
                member = tarfile.TarInfo(name)
                member.mode = mode
                if kind == "directory":
                    member.type = tarfile.DIRTYPE
                elif kind == "link":
                    member.type, member.linkname = tarfile.SYMTYPE, value
                else:
                    member.size = len(value)
                archive.addfile(member, io.BytesIO(value) if kind == "file" else None)
    return output.getvalue()


def assert_prefix(artifacts, destination):
    for entry in artifacts.entries:
        path = destination / ".bin" / entry.name
        assert stat.S_IMODE(path.lstat().st_mode) == entry.mode
        if entry.kind == "file":
            assert path.stat().st_size == entry.size
            assert adapter.digest(path.read_bytes()) == entry.sha256
        elif entry.kind == "link":
            assert os.readlink(path) == entry.link
    adapter.verify_meta_toolkit_prefix(artifacts, deployment_root=destination)


def trace_install(monkeypatch, artifacts, destination):
    """Count actual decompressor output and compressed backwards seeks per pass."""
    original_open, original_bytes = tarfile.open, io.BytesIO
    original_decompress = zlib.decompressobj
    original_gzip_decompress = getattr(zlib, "_ZlibDecompressor", None)
    state = {"phase": "validation", "validation": {"expanded": 0, "decompressors": 0, "backwards": 0},
        "extraction": {"expanded": 0, "decompressors": 0, "backwards": 0}, "modes": []}
    class CountedBytes(original_bytes):
        def seek(self, offset, whence=io.SEEK_SET):
            before = self.tell()
            result = super().seek(offset, whence)
            if result < before:
                state[state["phase"]]["backwards"] += 1
            return result
    class CountedDecompressor:
        def __init__(self, inner):
            self.inner = inner
            self.counts = state[state["phase"]]
            self.counts["decompressors"] += 1
        def __getattr__(self, name):
            return getattr(self.inner, name)
        def decompress(self, *args, **kwargs):
            data = self.inner.decompress(*args, **kwargs)
            self.counts["expanded"] += len(data)
            return data
    def traced_open(*args, **kwargs):
        mode = kwargs.get("mode", "r")
        state["modes"].append(mode)
        if mode in ("r:gz", "r|gz"):
            state["phase"] = "extraction"
        return original_open(*args, **kwargs)
    with monkeypatch.context() as held:
        held.setattr(io, "BytesIO", CountedBytes)
        held.setattr(zlib, "decompressobj",
            lambda *args, **kwargs: CountedDecompressor(original_decompress(*args, **kwargs)))
        if original_gzip_decompress is not None:
            held.setattr(zlib, "_ZlibDecompressor",
                lambda *args, **kwargs: CountedDecompressor(original_gzip_decompress(*args, **kwargs)))
        held.setattr(tarfile, "open", traced_open)
        adapter.install_meta_toolkit_prefix(artifacts, deployment_root=destination, require_current=lambda: None)
    return state


def test_producer_order_has_one_extraction_pass_no_backwards_seek_and_exact_payload(product, tmp_path, monkeypatch):
    prefix = next(name for name in product.nodes if "/" not in name)
    product.nodes.update({
        prefix + "/A-first.bin": ("file", 0o444, random.Random(14).randbytes(256 * 1024)),
        prefix + "/a-nested": ("directory", 0o555, None),
        prefix + "/a-nested/inside.bin": ("file", 0o444, b"inert deep first\n"),
        prefix + "/b-nested": ("directory", 0o555, None),
        prefix + "/b-nested/deeper": ("directory", 0o555, None),
        prefix + "/b-nested/deeper/inside.bin": ("file", 0o444, b"inert deeper\n"),
        prefix + "/z-last.bin": ("file", 0o444, b"inert shallow last\n"),
    })
    raw = prefix_bytes(product.nodes)
    expanded_size = len(gzip.decompress(raw))
    artifacts = select_archive(product, raw)
    destination = tmp_path / "streamed destination"
    destination.mkdir(mode=0o700)
    trace = trace_install(monkeypatch, artifacts, destination)
    assert trace["validation"]["expanded"] == expanded_size
    assert trace["extraction"]["backwards"] == 0, trace
    assert trace["extraction"]["decompressors"] == 1, trace
    assert 0 < trace["extraction"]["expanded"] <= expanded_size, trace
    assert trace["modes"] == ["r|", "r|gz"]
    assert_prefix(artifacts, destination)


def test_streaming_copy_never_requests_a_random_access_member_index(product, tmp_path, monkeypatch):
    artifacts = candidates.read_meta_toolkit_artifacts(product.target)
    destination = tmp_path / "no member index"
    destination.mkdir(mode=0o700)
    def forbidden(*args, **kwargs):
        pytest.fail("Native copying must not index or randomly look up compressed members.")
    monkeypatch.setattr(tarfile.TarFile, "getmember", forbidden)
    monkeypatch.setattr(tarfile.TarFile, "getmembers", forbidden)
    adapter.install_meta_toolkit_prefix(artifacts, deployment_root=destination, require_current=lambda: None)
    assert_prefix(artifacts, destination)


def test_valid_archive_with_files_before_directories_preserves_exact_graph(product, tmp_path):
    raw = ordered_bytes(product.nodes, sorted(product.nodes, reverse=True))
    artifacts = select_archive(product, raw)
    destination = tmp_path / "late archive directories"
    destination.mkdir(mode=0o700)
    adapter.install_meta_toolkit_prefix(artifacts, deployment_root=destination, require_current=lambda: None)
    assert_prefix(artifacts, destination)


def test_changed_selected_archive_refuses_before_destination_effects(product, tmp_path):
    artifacts = candidates.read_meta_toolkit_artifacts(product.target)
    product.archive.path.write_bytes(product.archive.path.read_bytes() + b"changed")
    destination = tmp_path / "unchanged destination"
    destination.mkdir(mode=0o700)
    with pytest.raises(candidates.CandidateArtifactError):
        adapter.install_meta_toolkit_prefix(artifacts, deployment_root=destination, require_current=lambda: None)
    assert list(destination.iterdir()) == []


@pytest.mark.parametrize("change", ["mode", "size", "kind", "name", "duplicate", "omit"])
def test_second_pass_must_match_complete_validated_entries(product, tmp_path, monkeypatch, change):
    artifacts = candidates.read_meta_toolkit_artifacts(product.target)
    destination = tmp_path / "inert second pass fault"
    destination.mkdir(mode=0o700)
    original_open = tarfile.open
    class AlteredArchive:
        def __init__(self, source):
            self.source = source
        def __iter__(self):
            changed = False
            for original in self.source:
                member = copy(original)
                if member.isfile() and not changed:
                    changed = True
                    if change == "mode": member.mode ^= 0o100
                    elif change == "size": member.size += 1
                    elif change == "kind": member.type = tarfile.SYMTYPE
                    elif change == "name": member.name = "../foreign"
                    elif change == "omit": continue
                    elif change == "duplicate":
                        yield member
                yield member
        def extractfile(self, member):
            return self.source.extractfile(member)
    @contextmanager
    def opened(*args, **kwargs):
        with original_open(*args, **kwargs) as archive:
            yield AlteredArchive(archive) if kwargs.get("mode") == "r|gz" else archive
    monkeypatch.setattr(tarfile, "open", opened)
    with pytest.raises(candidates.CandidateArtifactError):
        adapter.install_meta_toolkit_prefix(artifacts, deployment_root=destination, require_current=lambda: None)
