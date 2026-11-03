"""Byte-observation controls with inert files; no gateway or model is run."""
import hashlib
import os

import pytest

from launchkit import guide_sdk
from launchkit.bootstrap import SelectedGuideRuntime
from launchkit.connect_installer_caller import CustodyError, ExactFile
from launchkit.guide_runtime import configure_builder


@pytest.fixture
def selected(tmp_path, monkeypatch):
    path = (tmp_path / 'inert-gateway').resolve()
    path.write_bytes(b'An inert byte-verification fixture, never a native gateway.\n')
    path.chmod(0o700)
    reads = []
    original = ExactFile.verify

    def verify(pin, **options):
        reads.append(pin.path)
        return original(pin, **options)

    monkeypatch.setattr(ExactFile, 'verify', verify)
    pin = ExactFile(path, hashlib.sha256(path.read_bytes()).hexdigest())
    selected = guide_sdk.SELECTED_VERSION
    runtime = SelectedGuideRuntime(pin, selected, selected, 'a' * 40, 'b' * 40,
        _model_route={'provider': 'openai', 'model': 'gpt-6.1-sol'})
    return runtime, path, reads


def test_repeated_metadata_reads_reuse_observed_bytes_but_startup_rechecks(selected):
    runtime, path, reads = selected
    for _ in range(10):
        runtime.recheck()
        assert runtime.model_configuration == {'provider': 'openai', 'model': 'gpt-6.1-sol'}
    assert reads == [path]

    class Builder:
        def gateway(self, value):
            self.path = value

    builder = Builder()
    assert configure_builder(builder, runtime) is builder
    assert builder.path == str(path)
    assert reads == [path, path]


@pytest.mark.parametrize('change', ['replace', 'timestamp'])
def test_changed_metadata_rechecks_identical_bytes(selected, change):
    runtime, path, reads = selected
    if change == 'replace':
        replacement = path.with_name('replacement')
        replacement.write_bytes(path.read_bytes())
        replacement.chmod(0o700)
        replacement.replace(path)
    else:
        info = path.stat()
        os.utime(path, ns=(info.st_atime_ns, info.st_mtime_ns + 1_000_000))
    runtime.recheck()
    assert reads == [path, path]
    runtime.recheck()
    assert reads == [path, path]


def test_in_place_byte_change_is_detected_even_when_mtime_is_restored(selected):
    runtime, path, reads = selected
    info = path.stat()
    path.write_bytes(b'X' + path.read_bytes()[1:])
    os.utime(path, ns=(info.st_atime_ns, info.st_mtime_ns))
    with pytest.raises(CustodyError, match='selected bytes'):
        runtime.recheck()
    assert reads == [path, path]


@pytest.mark.parametrize('change', ['permissions', 'symlink', 'fifo', 'hardlink'])
def test_cached_observation_does_not_admit_unsafe_file_changes(selected, change):
    runtime, path, reads = selected
    if change == 'permissions':
        path.chmod(0o755)
    elif change == 'hardlink':
        os.link(path, path.with_name('second-link'))
    else:
        replacement = path.with_name('original')
        path.rename(replacement)
        if change == 'symlink':
            path.symlink_to(replacement)
        else:
            os.mkfifo(path, 0o600)
    with pytest.raises(CustodyError):
        runtime.recheck()
    assert reads == [path]


def test_original_digest_cannot_be_replaced_to_admit_changed_bytes(selected):
    runtime, path, reads = selected
    path.write_bytes(b'changed fixture')
    replacement = ExactFile(path, hashlib.sha256(path.read_bytes()).hexdigest())
    object.__setattr__(runtime, 'gateway', replacement)
    with pytest.raises(CustodyError, match='original byte pin'):
        runtime.recheck()
    assert reads == [path]


def test_startup_checksum_refuses_changed_bytes_even_if_metadata_is_unchanged(selected, monkeypatch):
    from pathlib import Path
    runtime, path, reads = selected
    unchanged = path.stat()
    path.write_bytes(b'X' + path.read_bytes()[1:])
    original_lstat = Path.lstat
    monkeypatch.setattr(os, 'fstat', lambda descriptor: unchanged)
    monkeypatch.setattr(Path, 'lstat', lambda value: unchanged if value == path else original_lstat(value))
    with pytest.raises(CustodyError, match='selected bytes'):
        configure_builder(object(), runtime)
    assert reads == [path, path]


def test_change_during_cached_metadata_observation_is_refused(selected, monkeypatch):
    runtime, path, reads = selected
    original = os.fstat
    observations = 0

    def fstat(descriptor):
        nonlocal observations
        observations += 1
        if observations == 2:
            info = path.stat()
            os.utime(path, ns=(info.st_atime_ns, info.st_mtime_ns + 1_000_000))
        return original(descriptor)

    monkeypatch.setattr(os, 'fstat', fstat)
    with pytest.raises(CustodyError, match='changed during observation'):
        runtime.recheck()
    assert reads == [path]
