"""Retained native prefix inventory: a product Start without the archive pass.

Inert source-only fixtures only (test_meta_toolkit_runtime_selection). These
are the reader and receipt rows of the 0.9.5 design (R1-R10, R16, R17,
R21-R24). The install and Start lanes keep their rows with their fixtures.
"""
from copy import deepcopy
import hashlib
import io
import json
import os
from pathlib import Path
import stat
from types import SimpleNamespace

import pytest

from launchkit import deployment
from launchkit import native_prefix_receipt as kept
from launchkit import runtime_candidates as candidates
from launchkit.templates import install_candidate as adapter
from test_meta_toolkit_runtime_selection import product, no_product_execution, prefix_bytes, json_bytes
from test_native_prefix_streaming import select_archive, ordered_bytes


SELECTION = {"format": "inert-product-selection", "assembly": "selected"}


def write_receipt(directory, receipt):
    path = directory / "receipt.json"
    path.write_text(json.dumps(receipt))
    path.chmod(0o600)


@pytest.fixture
def installed(product, tmp_path):
    artifacts = candidates.read_meta_toolkit_artifacts(product.target)
    destination = tmp_path / "destination"
    destination.mkdir(mode=0o700)
    adapter.install_meta_toolkit_prefix(artifacts, deployment_root=destination, require_current=lambda: None)
    directory = tmp_path / "deployment"
    directory.mkdir(mode=0o700)
    proof = {"source": "meta-toolkit-product", "assembly_sha256": product.target.assembly.sha256,
        "archive_sha256": artifacts.archive.sha256,
        "prefix_inventory_sha256": candidates.prefix_inventory_sha256(artifacts.entries)}
    write_receipt(directory, {"status": "stopped", "detail": "The application is stopped.",
        "progress": [{"at": "2026-10-22T00:00:00+00:00", "message": "The application is stopped."}],
        "runtime_candidate": SELECTION, "installed_candidate_runtime_proof": proof})
    return SimpleNamespace(product=product, artifacts=artifacts, destination=destination,
        directory=directory, proof=proof, prefix=destination / ".bin" / artifacts.prefix)


def keep(f):
    named = kept.mint(f.directory, f.artifacts, proof=f.proof, runtime_candidate=SELECTION)
    assert named is not None
    assert kept.record(f.directory, named, inventory_sha256=f.proof["prefix_inventory_sha256"],
        runtime_candidate=SELECTION)
    return named


def kept_path(f, named):
    return f.directory / f"native-prefix-receipt-{named}.json"


def receipt(f):
    return deployment.read_receipt(f.directory)


def kept_files(f):
    return sorted(path.name for path in f.directory.iterdir() if "native-prefix-receipt" in path.name)


def no_archive_pass(monkeypatch, product):
    def inflate(*args, **kwargs):
        pytest.fail("A retained Start inflated the product archive.")
    monkeypatch.setattr(candidates, "product_prefix_entries", inflate)
    original = candidates._Inputs.read
    def read(self, path, expected, limit, **kwargs):
        assert Path(path) != product.archive.path, "A retained Start hashed the product archive."
        return original(self, path, expected, limit, **kwargs)
    monkeypatch.setattr(candidates._Inputs, "read", read)


def archive_passes(monkeypatch):
    calls = []
    original = candidates.product_prefix_entries
    def counted(*args, **kwargs):
        calls.append(args[1])
        return original(*args, **kwargs)
    monkeypatch.setattr(candidates, "product_prefix_entries", counted)
    return calls


def rewrite(path, content):
    if path.exists():
        path.chmod(0o600)
    path.write_bytes(content)
    path.chmod(0o600)


def document_bytes(document):
    return (json.dumps(document, sort_keys=True, separators=(",", ":")) + "\n").encode()


def tamper_readme(f, content=None):
    readme = f.prefix / "README.md"
    original = readme.read_bytes()
    changed = content if content is not None else bytes([original[0] ^ 1]) + original[1:]
    readme.chmod(0o644)
    readme.write_bytes(changed)
    readme.chmod(0o444)
    return changed


def test_r1_mint_keeps_a_private_content_addressed_preimage_of_the_installed_proof(installed):
    f = installed
    proof = deepcopy(f.proof)
    named = keep(f)
    path = kept_path(f, named)
    info = path.lstat()
    assert stat.S_ISREG(info.st_mode) and stat.S_IMODE(info.st_mode) == 0o600
    assert info.st_uid == os.getuid() and info.st_nlink == 1
    assert hashlib.sha256(path.read_bytes()).hexdigest() == named
    document = json.loads(path.read_bytes())
    entries = tuple(candidates.ProductPrefixEntry(**item) for item in document["entries"])
    assert entries == f.artifacts.entries
    assert candidates.prefix_inventory_sha256(entries) == document["inventory_sha256"] == proof["prefix_inventory_sha256"]
    assert document["prefix"] == f.artifacts.prefix
    assert document["assembly_sha256"] == f.product.target.assembly.sha256
    assert (document["archive_sha256"], document["archive_bytes"]) == (f.artifacts.archive.sha256, f.artifacts.archive.size)
    assert document["runtime_candidate_sha256"] == kept.selection_sha256(SELECTION)
    assert f.proof == proof and receipt(f)["installed_candidate_runtime_proof"] == proof
    assert receipt(f)[kept.RECEIPT_FIELD] == named and kept_files(f) == [path.name]


def test_r2_start_with_the_named_inventory_reads_no_archive_and_checks_the_prefix(installed, monkeypatch):
    f = installed
    keep(f)
    retained = kept.load(f.directory, receipt(f))
    assert retained is not None and retained.entries == f.artifacts.entries
    no_archive_pass(monkeypatch, f.product)
    artifacts = candidates.read_meta_toolkit_artifacts(f.product.target, retained=retained)
    assert artifacts._inventory_source == "retained" and artifacts.entries == f.artifacts.entries
    assert adapter.verify_meta_toolkit_prefix(artifacts, deployment_root=f.destination) == f.prefix / "bin/toolkit_gateway"
    # The unread archive keeps its fingerprint pin for every later recheck.
    assert candidates.recheck_meta_toolkit_artifacts(artifacts, f.product.target) is artifacts
    f.product.archive.path.chmod(0o600)
    os.utime(f.product.archive.path, ns=(1, 1))
    with pytest.raises(candidates.CandidateArtifactError, match="artifact changed"):
        candidates.recheck_meta_toolkit_artifacts(artifacts, f.product.target)


@pytest.mark.parametrize("change, message", [
    ("modified", "An installed native prefix file changed."),
    ("added", "unexpected or unowned path"),
    ("removed", "complete installed native prefix inventory changed"),
    ("chmod", "An installed native prefix file or mode changed."),
    ("symlink", "An installed native prefix file or mode changed."),
])
def test_r3_r4_r5_r16_a_retained_start_still_refuses_every_prefix_change(installed, monkeypatch, change, message):
    f = installed
    keep(f)
    readme = f.prefix / "README.md"
    if change == "modified":
        tamper_readme(f)
    elif change == "added":
        (f.prefix / "added.txt").write_bytes(b"added\n")
    elif change == "removed":
        readme.unlink()
    elif change == "chmod":
        readme.chmod(0o644)
    else:
        readme.unlink()
        readme.symlink_to("LICENSE-MIT")
    retained = kept.load(f.directory, receipt(f))
    no_archive_pass(monkeypatch, f.product)
    artifacts = candidates.read_meta_toolkit_artifacts(f.product.target, retained=retained)
    assert artifacts._inventory_source == "retained"
    with pytest.raises(candidates.CandidateArtifactError, match=message):
        adapter.verify_meta_toolkit_prefix(artifacts, deployment_root=f.destination)


def test_r16_an_extra_hardlink_refuses(installed):
    f = installed
    keep(f)
    os.link(f.prefix / "README.md", f.destination / "outside-link")
    artifacts = candidates.read_meta_toolkit_artifacts(f.product.target, retained=kept.load(f.directory, receipt(f)))
    with pytest.raises(candidates.CandidateArtifactError, match="file or mode changed"):
        adapter.verify_meta_toolkit_prefix(artifacts, deployment_root=f.destination)


def test_r6_edited_inventory_bytes_refuse_even_when_the_prefix_matches_them(installed):
    f = installed
    named = keep(f)
    changed = tamper_readme(f)
    path = kept_path(f, named)
    document = json.loads(path.read_bytes())
    for item in document["entries"]:
        if item["name"] == f.artifacts.prefix + "/README.md":
            item["sha256"] = hashlib.sha256(changed).hexdigest()
    document["inventory_sha256"] = candidates.prefix_inventory_sha256(
        tuple(candidates.ProductPrefixEntry(**item) for item in document["entries"]))
    rewrite(path, document_bytes(document))
    with pytest.raises(kept.RetainedInventoryError, match="differs from the copy receipt.json names") as raised:
        kept.load(f.directory, receipt(f))
    assert str(path) in str(raised.value) and "Remove that file" in str(raised.value)


def test_r7_a_renamed_forgery_whose_entries_miss_their_digest_refuses(installed):
    f = installed
    named = keep(f)
    document = json.loads(kept_path(f, named).read_bytes())
    document["entries"] = [item for item in document["entries"] if not item["name"].endswith("/README.md")]
    content = document_bytes(document)
    forged = hashlib.sha256(content).hexdigest()
    rewrite(kept_path(f, forged), content)
    write_receipt(f.directory, {**receipt(f), kept.RECEIPT_FIELD: forged})
    with pytest.raises(kept.RetainedInventoryError, match="differ from its own inventory digest"):
        kept.load(f.directory, receipt(f))


def test_r7_a_self_consistent_forgery_is_stale_and_the_full_check_refuses(installed, monkeypatch):
    f = installed
    named = keep(f)
    (f.prefix / "README.md").unlink()
    document = json.loads(kept_path(f, named).read_bytes())
    document["entries"] = [item for item in document["entries"] if not item["name"].endswith("/README.md")]
    document["inventory_sha256"] = candidates.prefix_inventory_sha256(
        tuple(candidates.ProductPrefixEntry(**item) for item in document["entries"]))
    content = document_bytes(document)
    forged = hashlib.sha256(content).hexdigest()
    rewrite(kept_path(f, forged), content)
    write_receipt(f.directory, {**receipt(f), kept.RECEIPT_FIELD: forged})
    assert kept.load(f.directory, receipt(f)) is None
    passes = archive_passes(monkeypatch)
    artifacts = candidates.read_meta_toolkit_artifacts(f.product.target)
    assert passes == [f.artifacts.prefix] and artifacts._inventory_source == "archive"
    with pytest.raises(candidates.CandidateArtifactError, match="inventory changed"):
        adapter.verify_meta_toolkit_prefix(artifacts, deployment_root=f.destination)


def test_r8_a_legacy_receipt_takes_the_full_path_once_and_then_keeps_the_inventory(installed, monkeypatch):
    f = installed
    assert kept.RECEIPT_FIELD not in receipt(f) and kept.load(f.directory, receipt(f)) is None
    passes = archive_passes(monkeypatch)
    artifacts = candidates.read_meta_toolkit_artifacts(f.product.target, retained=None)
    adapter.verify_meta_toolkit_prefix(artifacts, deployment_root=f.destination)
    assert passes == [f.artifacts.prefix] and artifacts._inventory_source == "archive"
    named = kept.mint(f.directory, artifacts, proof=f.proof, runtime_candidate=SELECTION)
    assert kept.record(f.directory, named, inventory_sha256=f.proof["prefix_inventory_sha256"],
        runtime_candidate=SELECTION)
    retained = kept.load(f.directory, receipt(f))
    again = candidates.read_meta_toolkit_artifacts(f.product.target, retained=retained)
    assert passes == [f.artifacts.prefix] and again._inventory_source == "retained"
    # An inventory read from a kept copy is never minted again.
    assert kept.mint(f.directory, again, proof=f.proof, runtime_candidate=SELECTION) is None


@pytest.mark.parametrize("change", ["proof", "selection", "newer-schema", "older-schema", "missing"])
def test_r9_r10_r17_r22_stale_or_missing_inventories_fall_back_to_the_full_path(installed, change):
    f = installed
    named = keep(f)
    current = receipt(f)
    if change == "proof":
        current["installed_candidate_runtime_proof"] = {**f.proof, "prefix_inventory_sha256": "9" * 64}
    elif change == "selection":
        current["runtime_candidate"] = {**SELECTION, "assembly": "another selected product"}
    elif change.endswith("schema"):
        document = json.loads(kept_path(f, named).read_bytes())
        if change == "older-schema":
            document = {key: value for key, value in document.items() if key != "fingerprints"}
        content = document_bytes({**document, "schema": kept.SCHEMA + (1 if change == "newer-schema" else -1)})
        newer = hashlib.sha256(content).hexdigest()
        rewrite(kept_path(f, newer), content)
        current[kept.RECEIPT_FIELD] = newer
    else:
        kept_path(f, named).unlink()
    write_receipt(f.directory, current)
    assert kept.load(f.directory, receipt(f)) is None


@pytest.mark.parametrize("change, problem", [
    ("symlink", "is not a private ordinary file"),
    ("mode", "is not a private ordinary file"),
    ("hardlink", "is not a private ordinary file"),
    ("truncated", "differs from the copy receipt.json names"),
])
def test_r10_a_named_inventory_that_is_not_private_or_intact_refuses(installed, tmp_path, change, problem):
    f = installed
    named = keep(f)
    path = kept_path(f, named)
    if change == "symlink":
        copy = tmp_path / "elsewhere.json"
        copy.write_bytes(path.read_bytes())
        path.unlink()
        path.symlink_to(copy)
    elif change == "mode":
        path.chmod(0o640)
    elif change == "hardlink":
        os.link(path, tmp_path / "second-name.json")
    else:
        rewrite(path, path.read_bytes()[:-2])
    with pytest.raises(kept.RetainedInventoryError, match=problem):
        kept.load(f.directory, receipt(f))


def test_r17_a_new_archive_pin_rereads_the_archive_and_refuses_the_old_prefix(installed, monkeypatch):
    f = installed
    keep(f)
    retained = kept.load(f.directory, receipt(f))
    nodes = {**f.product.nodes, f.artifacts.prefix + "/README.md": ("file", 0o444, b"another selected product\n")}
    passes = archive_passes(monkeypatch)
    artifacts = select_archive(f.product, prefix_bytes(nodes))
    target = artifacts.selection
    selected = candidates.read_meta_toolkit_artifacts(target, retained=retained)
    assert selected._inventory_source == "archive" and len(passes) == 2
    with pytest.raises(candidates.CandidateArtifactError, match="file or mode changed|file changed"):
        adapter.verify_meta_toolkit_prefix(selected, deployment_root=f.destination)


def test_r17_the_same_inventory_in_a_reordered_archive_is_reread_not_trusted(installed, monkeypatch):
    f = installed
    keep(f)
    retained = kept.load(f.directory, receipt(f))
    passes = archive_passes(monkeypatch)
    reordered = select_archive(f.product, ordered_bytes(f.product.nodes, sorted(f.product.nodes, reverse=True)))
    assert reordered.entries == f.artifacts.entries and reordered.archive.sha256 != f.artifacts.archive.sha256
    selected = candidates.read_meta_toolkit_artifacts(reordered.selection, retained=retained)
    assert selected._inventory_source == "archive" and len(passes) == 2


def test_r21_another_prefix_or_assembly_is_stale(installed, monkeypatch):
    f = installed
    keep(f)
    retained = kept.load(f.directory, receipt(f))
    passes = archive_passes(monkeypatch)
    for changed in ({"prefix": "meerkat-connect-relocated"}, {"assembly_sha256": "8" * 64}):
        other = candidates.RetainedPrefixInventory(**{**retained.__dict__, **changed})
        artifacts = candidates.read_meta_toolkit_artifacts(f.product.target, retained=other)
        assert artifacts._inventory_source == "archive"
    assert len(passes) == 2
    with pytest.raises(candidates.CandidateArtifactError, match="bound owner"):
        candidates.read_meta_toolkit_artifacts(f.product.target, retained=retained.__dict__)


def test_r23_a_lazy_mint_changes_only_the_field_and_updated_at(installed):
    f = installed
    before = receipt(f)
    named = kept.mint(f.directory, f.artifacts, proof=f.proof, runtime_candidate=SELECTION)
    assert kept.record(f.directory, named, inventory_sha256=f.proof["prefix_inventory_sha256"],
        runtime_candidate=SELECTION)
    after = receipt(f)
    assert {key: value for key, value in after.items() if key not in {kept.RECEIPT_FIELD, "updated_at"}} == before
    assert after[kept.RECEIPT_FIELD] == named


def test_r23_a_mint_for_a_changed_receipt_is_not_named_and_old_copies_are_pruned(installed):
    f = installed
    first = keep(f)
    current = receipt(f)
    write_receipt(f.directory, {**current, "runtime_candidate": {**SELECTION, "assembly": "replaced"}})
    named = kept.mint(f.directory, f.artifacts, proof=f.proof, runtime_candidate=SELECTION)
    assert not kept.record(f.directory, named, inventory_sha256=f.proof["prefix_inventory_sha256"],
        runtime_candidate=SELECTION)
    assert receipt(f)[kept.RECEIPT_FIELD] == first
    kept.prune(f.directory)
    assert kept_files(f) == [kept_path(f, first).name]
    write_receipt(f.directory, {**current, "runtime_candidate": SELECTION})
    other = dict(f.proof)
    assert kept.mint(f.directory, f.artifacts, proof={**other, "prefix_inventory_sha256": "7" * 64},
        runtime_candidate=SELECTION) is None


def test_r24_a_same_size_archive_change_keeps_the_start_but_refuses_the_next_full_read(installed, monkeypatch):
    f = installed
    keep(f)
    raw = f.product.archive.path.read_bytes()
    position = len(raw) // 2
    rewrite(f.product.archive.path, raw[:position] + bytes([raw[position] ^ 1]) + raw[position + 1:])
    retained = kept.load(f.directory, receipt(f))
    artifacts = candidates.read_meta_toolkit_artifacts(f.product.target, retained=retained)
    assert artifacts._inventory_source == "retained"
    adapter.verify_meta_toolkit_prefix(artifacts, deployment_root=f.destination)
    with pytest.raises(candidates.CandidateArtifactError):
        candidates.read_meta_toolkit_artifacts(f.product.target)


# F2: fingerprints from a verified, hashed walk skip re-reading unchanged files.

def keep_with_fingerprints(f, monkeypatch, racy_ns=0):
    """Mint after a recording walk, as an install or a legacy Start does."""
    monkeypatch.setattr(kept, "RACY_NS", racy_ns)
    record = {}
    adapter.verify_meta_toolkit_prefix(f.artifacts, deployment_root=f.destination, record=record)
    named = kept.mint(f.directory, f.artifacts, proof=f.proof, runtime_candidate=SELECTION, fingerprints=record)
    assert kept.record(f.directory, named, inventory_sha256=f.proof["prefix_inventory_sha256"],
        runtime_candidate=SELECTION)
    return named, record


def counted_reads(monkeypatch, f):
    reads = []
    original = adapter.read_owned
    def read_owned(path, *args, **kwargs):
        if f.prefix in Path(path).parents:
            reads.append(Path(path).relative_to(f.prefix.parent).as_posix())
        return original(path, *args, **kwargs)
    monkeypatch.setattr(adapter, "read_owned", read_owned)
    return reads


def retained_artifacts(f):
    artifacts = candidates.read_meta_toolkit_artifacts(f.product.target, retained=kept.load(f.directory, receipt(f)))
    assert artifacts._inventory_source == "retained"
    return artifacts


def file_names(f):
    return sorted(entry.name for entry in f.artifacts.entries if entry.kind == "file")


def verify_known(f, artifacts, known=None):
    adapter.verify_meta_toolkit_prefix(artifacts, deployment_root=f.destination,
        known=dict(artifacts._known_fingerprints) if known is None else known)


def test_r11_an_unchanged_prefix_is_checked_reading_only_the_gateway(installed, monkeypatch):
    f = installed
    named, record = keep_with_fingerprints(f, monkeypatch)
    assert sorted(record) == file_names(f)
    assert sorted(json.loads(kept_path(f, named).read_bytes())["fingerprints"]) == file_names(f)
    artifacts = retained_artifacts(f)
    reads = counted_reads(monkeypatch, f)
    verify_known(f, artifacts)
    # The executed gateway is always hashed; no other unchanged file is read.
    assert reads == [f.artifacts.prefix + "/bin/toolkit_gateway"]


def test_r12_a_content_change_with_its_mtime_restored_moves_ctime_and_refuses(installed, monkeypatch):
    f = installed
    keep_with_fingerprints(f, monkeypatch)
    readme = f.prefix / "README.md"
    before = readme.stat()
    tamper_readme(f)
    os.utime(readme, ns=(before.st_atime_ns, before.st_mtime_ns))
    after = readme.stat()
    assert (after.st_size, after.st_mtime_ns) == (before.st_size, before.st_mtime_ns)
    assert after.st_ctime_ns != before.st_ctime_ns
    artifacts = retained_artifacts(f)
    reads = counted_reads(monkeypatch, f)
    with pytest.raises(candidates.CandidateArtifactError, match="An installed native prefix file changed."):
        verify_known(f, artifacts)
    assert reads == [f.artifacts.prefix + "/README.md"]


def test_r13_a_fingerprint_edited_to_match_a_changed_file_refuses(installed, monkeypatch):
    f = installed
    named, _ = keep_with_fingerprints(f, monkeypatch)
    tamper_readme(f)
    path = kept_path(f, named)
    document = json.loads(path.read_bytes())
    document["fingerprints"][f.artifacts.prefix + "/README.md"] = list(
        adapter.fingerprint((f.prefix / "README.md").lstat()))
    rewrite(path, document_bytes(document))
    with pytest.raises(kept.RetainedInventoryError, match="differs from the copy receipt.json names"):
        kept.load(f.directory, receipt(f))


def test_r14_racy_fingerprints_are_never_kept(installed, monkeypatch):
    f = installed
    assert kept.RACY_NS == 2_000_000_000
    named, record = keep_with_fingerprints(f, monkeypatch, racy_ns=10 ** 18)
    assert json.loads(kept_path(f, named).read_bytes())["fingerprints"] == {}
    artifacts = retained_artifacts(f)
    reads = counted_reads(monkeypatch, f)
    verify_known(f, artifacts)
    assert sorted(reads) == file_names(f)
    name = file_names(f)[0]
    fingerprint = record[name][0]
    monkeypatch.setattr(kept, "RACY_NS", 2_000_000_000)
    for seen, kept_it in ((fingerprint[7] + 1_999_999_999, False), (fingerprint[7] + 2_000_000_001, True)):
        minted = kept.mint(f.directory, f.artifacts, proof=f.proof, runtime_candidate=SELECTION,
            fingerprints={name: (fingerprint, seen)})
        assert (name in json.loads(kept_path(f, minted).read_bytes())["fingerprints"]) is kept_it


def test_r15_the_same_bytes_in_a_new_inode_are_hashed_again_and_pass(installed, monkeypatch):
    f = installed
    named, _ = keep_with_fingerprints(f, monkeypatch)
    readme = f.prefix / "README.md"
    copy = f.destination / "README.copy"
    copy.write_bytes(readme.read_bytes())
    copy.chmod(0o444)
    os.replace(copy, readme)
    artifacts = retained_artifacts(f)
    reads = counted_reads(monkeypatch, f)
    verify_known(f, artifacts)
    assert sorted(reads) == sorted([f.artifacts.prefix + "/README.md", f.artifacts.prefix + "/bin/toolkit_gateway"])
    assert receipt(f)[kept.RECEIPT_FIELD] == named and kept_files(f) == [kept_path(f, named).name]


@pytest.mark.parametrize("change", ["symlink", "hardlink", "chmod", "added", "removed"])
def test_r16_structural_checks_refuse_before_any_fingerprint_skip(installed, monkeypatch, change):
    f = installed
    keep_with_fingerprints(f, monkeypatch)
    readme = f.prefix / "README.md"
    if change == "symlink":
        readme.unlink()
        readme.symlink_to("LICENSE-MIT")
    elif change == "hardlink":
        os.link(readme, f.destination / "outside-link")
    elif change == "chmod":
        readme.chmod(0o644)
    elif change == "added":
        (f.prefix / "added.txt").write_bytes(b"added\n")
    else:
        readme.unlink()
    artifacts = retained_artifacts(f)
    with pytest.raises(candidates.CandidateArtifactError,
            match="file or mode changed|unexpected or unowned path|inventory changed"):
        verify_known(f, artifacts)


def test_r18_the_recording_walk_syncs_each_file_before_reading_it(installed, monkeypatch):
    f = installed
    events = []
    original_fsync, original_read = os.fsync, adapter.read_owned
    def fsync(descriptor):
        events.append(("fsync", os.readlink(f"/proc/self/fd/{descriptor}")))
        return original_fsync(descriptor)
    def read_owned(path, *args, **kwargs):
        events.append(("read", str(path)))
        return original_read(path, *args, **kwargs)
    monkeypatch.setattr(os, "fsync", fsync)
    monkeypatch.setattr(adapter, "read_owned", read_owned)
    adapter.verify_meta_toolkit_prefix(f.artifacts, deployment_root=f.destination)
    assert not any(kind == "fsync" for kind, _ in events)
    events.clear()
    record = {}
    adapter.verify_meta_toolkit_prefix(f.artifacts, deployment_root=f.destination, record=record)
    for name in file_names(f):
        path = str(f.destination / ".bin" / name)
        assert events.index(("fsync", path)) < events.index(("read", path))
    assert sorted(record) == file_names(f)


@pytest.mark.parametrize("fstype, allowed", [("ext4", True), ("xfs", True), ("btrfs", True),
    ("tmpfs", False), ("overlay", False), ("fuse.sshfs", False), ("nfs4", False)])
def test_r19_only_allow_listed_local_disks_keep_or_use_fingerprints(tmp_path, monkeypatch, fstype, allowed):
    device = os.stat(tmp_path).st_dev
    mounts = ("22 1 0:5 / /proc rw,nosuid - proc proc rw\n"
        f"36 25 {os.major(device)}:{os.minor(device)} / / rw,relatime shared:1 - {fstype} /dev/inert rw\n")
    real_open = open
    def fake_open(path, *args, **kwargs):
        if path == "/proc/self/mountinfo":
            return io.StringIO(mounts)
        return real_open(path, *args, **kwargs)
    monkeypatch.setattr(kept, "open", fake_open, raising=False)
    assert kept.local_disk_filesystem(tmp_path) is allowed


def test_r19_an_unknown_device_or_no_proc_disables_fingerprints(tmp_path, monkeypatch):
    monkeypatch.setattr(kept, "open", lambda path, *args, **kwargs: io.StringIO(
        "22 1 0:5 / /proc rw - proc proc rw\n"), raising=False)
    assert kept.local_disk_filesystem(tmp_path) is False
    def missing(path, *args, **kwargs):
        raise FileNotFoundError(path)
    monkeypatch.setattr(kept, "open", missing, raising=False)
    assert kept.local_disk_filesystem(tmp_path) is False


def test_r20_a_fingerprint_on_another_device_is_hashed_again(installed, monkeypatch):
    f = installed
    keep_with_fingerprints(f, monkeypatch)
    artifacts = retained_artifacts(f)
    known = {name: (fingerprint[0] + 1, *fingerprint[1:]) for name, fingerprint in artifacts._known_fingerprints}
    reads = counted_reads(monkeypatch, f)
    verify_known(f, artifacts, known)
    assert sorted(reads) == file_names(f)


@pytest.mark.parametrize("change", ["not-a-file", "short-row", "text-field"])
def test_malformed_kept_fingerprints_refuse(installed, monkeypatch, change):
    f = installed
    named, _ = keep_with_fingerprints(f, monkeypatch)
    document = json.loads(kept_path(f, named).read_bytes())
    name = file_names(f)[0]
    if change == "not-a-file":
        document["fingerprints"][f.artifacts.prefix] = document["fingerprints"][name]
    elif change == "short-row":
        document["fingerprints"][name] = document["fingerprints"][name][:7]
    else:
        document["fingerprints"][name][0] = "device"
    content = document_bytes(document)
    forged = hashlib.sha256(content).hexdigest()
    rewrite(kept_path(f, forged), content)
    write_receipt(f.directory, {**receipt(f), kept.RECEIPT_FIELD: forged})
    with pytest.raises(kept.RetainedInventoryError, match="is not a readable inventory"):
        kept.load(f.directory, receipt(f))


def test_a_kept_inventory_removed_after_its_lstat_is_stale_not_refused(installed, monkeypatch):
    f = installed
    named = keep(f)
    path = kept_path(f, named)
    original = os.open
    def vanished(name, *args, **kwargs):
        if Path(name) == path:
            raise FileNotFoundError(name)
        return original(name, *args, **kwargs)
    monkeypatch.setattr(os, "open", vanished)
    assert kept.load(f.directory, receipt(f)) is None


def test_kept_fingerprints_never_skip_the_gateway_or_another_device(installed, monkeypatch):
    f = installed
    keep_with_fingerprints(f, monkeypatch)
    artifacts = retained_artifacts(f)
    monkeypatch.setattr(kept, "local_disk_filesystem", lambda path: True)
    gateway = f.artifacts.prefix + "/bin/toolkit_gateway"
    known = kept.known_fingerprints(artifacts, f.prefix)
    assert gateway not in known and sorted(known) == [name for name in file_names(f) if name != gateway]
    device = os.lstat(f.prefix).st_dev
    elsewhere = tuple((name, (device + 1, *fingerprint[1:]) if name.endswith("README.md") else fingerprint)
        for name, fingerprint in artifacts._known_fingerprints)
    object.__setattr__(artifacts, "_known_fingerprints", elsewhere)
    assert f.artifacts.prefix + "/README.md" not in kept.known_fingerprints(artifacts, f.prefix)
    reads = counted_reads(monkeypatch, f)
    adapter.verify_meta_toolkit_prefix(artifacts, deployment_root=f.destination,
        known=kept.known_fingerprints(artifacts, f.prefix))
    assert sorted(reads) == sorted([gateway, f.artifacts.prefix + "/README.md"])
    record = {name: ((device + 1, *value[0][1:]), value[1]) if name.endswith("README.md") else value
        for name, value in {name: ((device, 1, 2, 3, 1, 4, 5, 6), 9) for name in file_names(f)}.items()}
    assert sorted(kept.same_device_fingerprints(record, f.prefix)) == [
        name for name in file_names(f) if not name.endswith("README.md")]
    monkeypatch.setattr(kept, "local_disk_filesystem", lambda path: False)
    assert kept.known_fingerprints(artifacts, f.prefix) is None


def test_a_fingerprint_equal_but_changed_gateway_is_still_caught(installed, monkeypatch):
    f = installed
    keep_with_fingerprints(f, monkeypatch)
    gateway = f.prefix / "bin/toolkit_gateway"
    content = gateway.read_bytes()
    gateway.write_bytes(bytes([content[0] ^ 1]) + content[1:])
    artifacts = retained_artifacts(f)
    # Even a known fingerprint equal to the changed gateway's never skips it.
    known = {**dict(artifacts._known_fingerprints),
        f.artifacts.prefix + "/bin/toolkit_gateway": adapter.fingerprint(gateway.lstat())}
    reads = counted_reads(monkeypatch, f)
    with pytest.raises(candidates.CandidateArtifactError, match="An installed native prefix file changed."):
        adapter.verify_meta_toolkit_prefix(artifacts, deployment_root=f.destination, known=known)
    assert reads == [f.artifacts.prefix + "/bin/toolkit_gateway"]
