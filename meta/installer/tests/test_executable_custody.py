"""Executable byte custody and streaming bounds, without native execution."""
import hashlib
import os

import pytest

from launchkit import connect_installer_caller as module


def selected(tmp_path, content=b"selected gateway bytes\n"):
    tmp_path.chmod(0o700)
    path = tmp_path / "gateway"
    path.write_bytes(content)
    path.chmod(0o700)
    return module.ExactFile(path, hashlib.sha256(content).hexdigest())


def test_streaming_gateway_accepts_above_materialized_read_limit(tmp_path, monkeypatch):
    pin = selected(tmp_path, b"finite gateway payload" * 13)
    monkeypatch.setattr(module, "MAX_BINARY_BYTES", 8)
    monkeypatch.setattr(module, "MAX_EXECUTABLE_BYTES", 1024)
    monkeypatch.setattr(module, "VERIFY_CHUNK_BYTES", 7)
    with pytest.raises(module.CustodyError, match="bounded"):
        pin.read(executable=True)
    assert pin.verify(executable=True) is None
    assert module.MAX_EXECUTABLE_BYTES > module.MAX_BINARY_BYTES


@pytest.mark.parametrize("change", ["bytes", "access", "executable", "empty", "link", "symlink", "digest", "bound"])
def test_streaming_custody_refuses_changed_or_unowned_inputs(tmp_path, monkeypatch, change):
    pin = selected(tmp_path)
    if change == "bytes":
        pin.path.write_bytes(b"changed bytes\n")
    elif change == "access":
        pin.path.chmod(0o755)
    elif change == "executable":
        pin.path.chmod(0o600)
    elif change == "empty":
        pin.path.write_bytes(b"")
    elif change == "link":
        os.link(pin.path, tmp_path / "alias")
    elif change == "symlink":
        original = tmp_path / "original"
        pin.path.rename(original)
        pin.path.symlink_to(original)
    elif change == "digest":
        pin = module.ExactFile(pin.path, "not-a-digest")
    else:
        monkeypatch.setattr(module, "MAX_EXECUTABLE_BYTES", 8)
    with pytest.raises(module.CustodyError):
        pin.verify(executable=True)


@pytest.mark.parametrize("change", ["replace", "mode", "append", "hardlink"])
def test_streaming_detects_changes_between_hash_and_final_observation(tmp_path, monkeypatch, change):
    pin = selected(tmp_path)
    digest = hashlib.sha256
    changed = False

    class ChangingDigest:
        def __init__(self):
            self.digest = digest()

        def update(self, chunk):
            nonlocal changed
            self.digest.update(chunk)
            if changed:
                return
            changed = True
            if change == "replace":
                replacement = tmp_path / "replacement"
                replacement.write_bytes(pin.path.read_bytes())
                replacement.chmod(0o700)
                replacement.replace(pin.path)
            elif change == "mode":
                pin.path.chmod(0o755)
            elif change == "append":
                with pin.path.open("ab") as stream:
                    stream.write(b"extra")
            else:
                os.link(pin.path, tmp_path / "alias")

        def hexdigest(self):
            return self.digest.hexdigest()

    monkeypatch.setattr(module.hashlib, "sha256", ChangingDigest)
    with pytest.raises(module.CustodyError, match="changed"):
        pin.verify(executable=True)


def test_streaming_nonexecutables_keep_existing_artifact_limit(tmp_path, monkeypatch):
    pin = selected(tmp_path, b"input exceeds its limit")
    pin.path.chmod(0o600)
    monkeypatch.setattr(module, "MAX_INPUT_BYTES", 8)
    monkeypatch.setattr(module, "MAX_EXECUTABLE_BYTES", 1024)
    with pytest.raises(module.CustodyError, match="bounded"):
        pin.verify()


def test_streaming_has_no_materialized_read_dependency(tmp_path, monkeypatch):
    pin = selected(tmp_path)
    monkeypatch.setattr(module, "_read", lambda *args, **kwargs: pytest.fail("materialized read"))
    assert pin.verify(executable=True) is None
