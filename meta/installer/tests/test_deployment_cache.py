import hashlib
import json
import os

import pytest

from launchkit import bootstrap
from launchkit.deployment_cache import seed_release_cache


def release_cache(tmp_path):
    directory = tmp_path / "official-cache"
    directory.mkdir(mode=0o700)
    for name, prefix in (("rpc_gateway", "mobkit-rpc-gateway"), ("mobkit_gateway", "mobkit-gateway")):
        binary = directory / name
        binary.write_bytes((name + " official fixture").encode())
        binary.chmod(0o700)
        metadata = {"version": bootstrap.VERSION, "core_version": bootstrap.CORE_VERSION, "release_url": bootstrap.RELEASE_URL,
            "archive": f"{prefix}-{bootstrap.VERSION}-{bootstrap.platform_target()}.tar.gz",
            "archive_sha256": "a" * 64, "binary_sha256": hashlib.sha256(binary.read_bytes()).hexdigest()}
        receipt = directory / (name + ".receipt.json")
        receipt.write_text(json.dumps(metadata))
        receipt.chmod(0o600)
    return directory


def test_official_cache_copies_exact_checked_bytes_and_private_receipts(tmp_path):
    source = release_cache(tmp_path)
    destination = tmp_path / "deployment"
    destination.mkdir()
    result = seed_release_cache(destination, {"mobkitVersion": bootstrap.VERSION}, source_directory=source)
    assert result == ["rpc_gateway", "mobkit_gateway"]
    for name in result:
        installed = destination / ".bin" / name
        receipt = installed.with_name(name + ".receipt.json")
        assert installed.read_bytes() == (source / name).read_bytes()
        assert receipt.read_bytes() == (source / receipt.name).read_bytes()
        assert installed.stat().st_mode & 0o777 == 0o700
        assert receipt.stat().st_mode & 0o777 == 0o600
    installed.write_bytes(b"owner change")
    assert seed_release_cache(destination, {"mobkitVersion": bootstrap.VERSION}, source_directory=source) == []
    assert installed.read_bytes() == b"owner change"


@pytest.mark.parametrize("invalid", ["digest", "version", "core_version", "platform", "public", "public_directory", "link", "hardlink", "override", "missing"])
def test_ineligible_cache_falls_back_without_writing_any_binary(tmp_path, invalid):
    source = release_cache(tmp_path)
    binary = source / "mobkit_gateway"
    receipt = source / "mobkit_gateway.receipt.json"
    if invalid == "digest":
        binary.write_bytes(b"modified binary")
    elif invalid in {"version", "core_version", "platform"}:
        value = json.loads(receipt.read_text())
        value["archive" if invalid == "platform" else invalid] = "not-this-pinned-release"
        receipt.write_text(json.dumps(value))
    elif invalid == "public":
        receipt.chmod(0o644)
    elif invalid == "public_directory":
        source.chmod(0o755)
    elif invalid == "hardlink":
        os.link(binary, source / "another-binary-link")
    elif invalid == "override":
        value = json.loads(receipt.read_text())
        value["source"] = "explicit_override"
        receipt.write_text(json.dumps(value))
    elif invalid == "link":
        renamed = source / "linked-source"
        binary.rename(renamed)
        binary.symlink_to(renamed)
    else:
        binary.unlink()
    destination = tmp_path / "deployment"
    destination.mkdir()
    assert seed_release_cache(destination, {"mobkitVersion": bootstrap.VERSION}, source_directory=source) == []
    assert not (destination / ".bin").exists()


def test_cache_change_during_copy_never_installs_partial_executables(tmp_path, monkeypatch):
    source = release_cache(tmp_path)
    real = bootstrap._verified_cache
    def changed(binary, receipt, archive):
        accepted = real(binary, receipt, archive)
        if binary.name == "mobkit_gateway":
            (source / "rpc_gateway").write_bytes(b"changed after verification")
        return accepted
    monkeypatch.setattr(bootstrap, "_verified_cache", changed)
    destination = tmp_path / "deployment"
    destination.mkdir()
    with pytest.raises(ValueError, match="changed while being copied"):
        seed_release_cache(destination, {"mobkitVersion": bootstrap.VERSION}, source_directory=source)
    assert not list((destination / ".bin").iterdir())


def test_destination_link_is_rejected_without_touching_another_folder(tmp_path):
    source = release_cache(tmp_path)
    destination = tmp_path / "deployment"
    destination.mkdir()
    other = tmp_path / "other"
    other.mkdir()
    (destination / ".bin").symlink_to(other, target_is_directory=True)
    with pytest.raises(ValueError, match="owned ordinary folder"):
        seed_release_cache(destination, {"mobkitVersion": bootstrap.VERSION}, source_directory=source)
    assert list(other.iterdir()) == []


def test_other_pinned_version_never_uses_this_hosts_cache(tmp_path):
    destination = tmp_path / "deployment"
    destination.mkdir()
    assert seed_release_cache(destination, {"mobkitVersion": "different"}, source_directory=release_cache(tmp_path)) == []
    assert not (destination / ".bin").exists()
