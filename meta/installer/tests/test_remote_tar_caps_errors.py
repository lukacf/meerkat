"""Finite compressed-cap and malformed-deflate archive controls.

Set FORGE_REMOTE_TAR_SOURCE_ROOT to the maintained installer or a retained
source tree. Reuse the original real-source loader and its finite 64/65/80 MiB
budget relationship. No shell, cloud service or application is started.
"""
import base64
import gzip
import hashlib
import zlib

import pytest

import test_remote_tar_bounds as controls


COMPRESSED_CAP = 65 * 1024 * 1024 // controls.SCALE
EXPANDED_CAP = 80 * 1024 * 1024 // controls.SCALE
CONTROLLER_REFUSAL = "The transferred package does not match the accepted source binding."
RECEIVER_COMPRESSED_REFUSAL = "An installation file has unsafe ownership, permissions or size."
RECEIVER_DEFLATE_REFUSAL = "The uploaded package could not be decompressed."
BOOTSTRAP_COMPRESSED_REFUSAL = "Uploaded installation archive exceeded its compressed size bound."
BOOTSTRAP_DEFLATE_REFUSAL = "Uploaded installation archive could not be decompressed."


def assert_safe_refusal(surface, damage, archive, digest, tmp_path, monkeypatch):
    home = tmp_path.resolve() / "inert-home"
    home.mkdir(mode=0o700)
    path = home / "mobkit-package.tar.gz"
    destination = home / "src/finite-fixture"
    path.write_bytes(archive)
    path.chmod(0o600)
    before = path.stat()

    if surface == "controller":
        module = controls.load_module("remote_destination.py")
        invoke = lambda: module._archive(base64.b64encode(archive).decode(), controls.binding(digest))
        error_type, expected = module.ControllerError, CONTROLLER_REFUSAL
    elif surface == "receiver":
        module = controls.load_module("templates_v2/remote_operations.py")
        invoke = lambda: module.receive_archive(path, destination, controls.OWNER_ID, digest)
        error_type = module.DestinationError
        expected = RECEIVER_COMPRESSED_REFUSAL if damage == "compressed" else RECEIVER_DEFLATE_REFUSAL
    else:
        expected_helper = hashlib.sha256(controls.INERT_HELPER).hexdigest()
        invoke = lambda: controls.bootstrap_run(monkeypatch, home, expected_helper, digest)
        error_type = SystemExit
        expected = BOOTSTRAP_COMPRESSED_REFUSAL if damage == "compressed" else BOOTSTRAP_DEFLATE_REFUSAL

    calls = controls.parser_spy(monkeypatch)
    with pytest.raises(error_type) as caught:
        invoke()

    # These strings are independently taken from the actual host-authored
    # refusal paths, not read from a constant in the loaded module.
    assert str(caught.value) == expected
    if surface == "controller":
        assert caught.value.code == "invalid-package"
        assert caught.value.fields is None
    elif damage == "deflate":
        assert caught.value.__cause__ is None
        assert caught.value.__suppress_context__ is True

    assert calls == [], "Rejected bytes must not enter the actual TAR parser."
    assert not destination.exists(), "Refusal must precede source publication."
    assert not (home / "qualified-helper-executed.txt").exists(), "Refusal must precede helper import."
    assert not (home / "qualified-helper-received.jsonl").exists(), "Refusal must precede receiver handoff."
    assert sorted(home.iterdir()) == [path], "The archive must be the only file left in the inert home."
    assert path.read_bytes() == archive
    after = path.stat()
    assert (after.st_ino, after.st_size, after.st_mtime_ns, after.st_mode) == (
        before.st_ino, before.st_size, before.st_mtime_ns, before.st_mode)


@pytest.mark.parametrize("surface", ["controller", "receiver", "bootstrap"])
def test_compressed_input_one_byte_over_cap_refused_before_parser_and_effects(surface, tmp_path, monkeypatch):
    archive, digest, expanded = controls.transfer(controls.public_files())
    assert len(archive) < COMPRESSED_CAP
    assert len(expanded) < EXPANDED_CAP
    # Zero padding is valid gzip padding. This is a valid small TAR with legal
    # source bytes; only its total compressed transport length exceeds the cap.
    archive += b"\0" * (COMPRESSED_CAP + 1 - len(archive))
    assert len(archive) == COMPRESSED_CAP + 1
    assert gzip.decompress(archive) == expanded
    # At precisely one byte over, controller base64 length is still within
    # its outer representation allowance. Its decoded-byte bound must act.
    assert len(base64.b64encode(archive)) <= COMPRESSED_CAP * 4 // 3 + 8
    assert_safe_refusal(surface, "compressed", archive, digest, tmp_path, monkeypatch)


@pytest.mark.parametrize("surface", ["controller", "receiver", "bootstrap"])
def test_invalid_deflate_maps_real_zlib_error_to_safe_host_refusal(surface, tmp_path, monkeypatch):
    archive, digest, _ = controls.transfer(controls.public_files())
    assert archive[:4] == b"\x1f\x8b\x08\x00"
    # Preserve a genuine gzip header and trailer, but use a reserved DEFLATE
    # block type (BFINAL=1, BTYPE=3). Real zlib must reject this payload before
    # a CRC/truncated-trailer path could account for the refusal.
    archive = archive[:10] + b"\x07" + archive[-8:]
    assert len(archive) < COMPRESSED_CAP
    with pytest.raises(zlib.error, match="invalid block type"):
        gzip.decompress(archive)
    assert_safe_refusal(surface, "deflate", archive, digest, tmp_path, monkeypatch)
