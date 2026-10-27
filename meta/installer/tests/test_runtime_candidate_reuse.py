"""Explicit verified-result reuse with real inert owned-file substitutions.

All payloads are synthetic. No SDK, native program, service, provider or
network operation may execute, and no verified result is stored globally.
"""
from contextlib import contextmanager
from dataclasses import FrozenInstanceError, replace
import os
import socket
import subprocess

import pytest

from launchkit import runtime_candidates as candidates
from test_runtime_candidates import Bundle


@pytest.fixture
def bundle(tmp_path, monkeypatch):
    def forbidden(*args, **kwargs):
        pytest.fail("Candidate reuse must not execute payloads or contact a provider.")
    for owner, name in ((subprocess, "run"), (subprocess, "Popen"),
            (os, "system"), (socket, "create_connection")):
        monkeypatch.setattr(owner, name, forbidden)
    return Bundle(tmp_path / "external-artifacts")


def recheck(bundle, verified, **changes):
    return candidates.recheck_candidate_artifacts(verified,
        tuple_path=changes.get("tuple_path", bundle.tuple_path),
        accepted=changes.get("accepted", bundle.binding),
        artifact_root=changes.get("artifact_root", bundle.root))


def test_explicit_reuse_expands_once_and_independent_reads_are_fresh(bundle, monkeypatch):
    calls = []
    native = candidates._native

    def observed(*args, **kwargs):
        calls.append(args[2]["binary"])
        return native(*args, **kwargs)

    monkeypatch.setattr(candidates, "_native", observed)
    verified = bundle.read()
    for _ in range(6):
        assert recheck(bundle, verified) is verified
    assert sorted(calls) == sorted(candidates.BINARIES)
    independent = bundle.read()
    assert independent is not verified and independent == verified
    assert len(calls) == 2 * len(candidates.BINARIES)
    with pytest.raises(FrozenInstanceError):
        verified._verified_inputs = ()


@pytest.mark.parametrize("kind", ["constructed", "replaced", "json-flag"])
def test_constructed_or_replaced_results_have_no_reader_snapshot(bundle, kind):
    verified = bundle.read()
    if kind == "constructed":
        value = candidates.CandidateArtifacts(verified.identity, verified.tuple_file,
            verified.native, verified.sdk, verified.evidence)
    elif kind == "replaced":
        value = replace(verified)
    else:
        value = {"verified": True, "candidate": verified.identity}
    with pytest.raises(candidates.CandidateArtifactError, match="full reader"):
        recheck(bundle, value)


@pytest.mark.parametrize("change", ["binding", "boolean-binding", "locator", "root"])
def test_verified_result_is_bound_to_exact_selection_and_locator(bundle, tmp_path, change):
    verified = bundle.read()
    if change == "binding":
        values = {"accepted": replace(bundle.binding, native_run_id=24)}
    elif change == "boolean-binding":
        values = {"accepted": replace(bundle.binding, native_run_attempt=True)}
    elif change == "locator":
        alias = bundle.root / "tuple-copy.json"
        alias.write_bytes(bundle.tuple_path.read_bytes())
        values = {"tuple_path": alias}
    else:
        other = tmp_path / "other-owned-root"
        other.mkdir()
        values = {"artifact_root": other}
    with pytest.raises(candidates.CandidateArtifactError):
        recheck(bundle, verified, **values)


@pytest.mark.parametrize("member", ["tuple", "native", "outer", "wheel", "manifest", "sdk-provenance", "sdk-review"])
def test_changed_bytes_in_each_artifact_class_refuse_without_another_expansion(bundle, monkeypatch, member):
    verified = bundle.read()
    paths = {"tuple": bundle.tuple_path, "native": verified.native[0].archive.path,
        "outer": bundle.root / "target.zip", "wheel": bundle.wheel_path,
        "manifest": bundle.root / "manifest.json", "sdk-provenance": bundle.sdk_dir / "provenance.json",
        "sdk-review": bundle.sdk_dir / "review.json"}
    path = paths[member]
    info, before = path.stat(), path.read_bytes()
    changed = bytes([before[0] ^ 1]) + before[1:]
    path.write_bytes(changed)
    os.utime(path, ns=(info.st_atime_ns, info.st_mtime_ns))
    monkeypatch.setattr(candidates, "_native", lambda *args, **kwargs: pytest.fail("Reuse must refuse changed inputs before expansion."))
    with pytest.raises(candidates.CandidateArtifactError):
        recheck(bundle, verified)
    assert path.read_bytes() == changed


@pytest.mark.parametrize("change", ["same-byte-inode", "same-byte-rewrite", "mode", "hardlink", "symlink", "removed"])
def test_real_owned_file_substitutions_refuse_even_when_payload_is_unchanged(bundle, change):
    verified = bundle.read()
    path = bundle.wheel_path
    info, content = path.stat(), path.read_bytes()
    if change == "same-byte-inode":
        replacement = path.with_name("replacement-wheel")
        replacement.write_bytes(content)
        replacement.chmod(info.st_mode & 0o777)
        os.utime(replacement, ns=(info.st_atime_ns, info.st_mtime_ns))
        replacement.replace(path)
    elif change == "same-byte-rewrite":
        path.write_bytes(content)
        os.utime(path, ns=(info.st_atime_ns, info.st_mtime_ns))
    elif change == "mode":
        path.chmod((info.st_mode & 0o777) ^ 0o010)
    elif change == "hardlink":
        os.link(path, path.with_name("linked-wheel"))
    elif change == "symlink":
        actual = path.with_name("original-wheel")
        path.rename(actual)
        path.symlink_to(actual)
    else:
        path.unlink()
    with pytest.raises(candidates.CandidateArtifactError):
        recheck(bundle, verified)
    if change not in {"removed", "symlink"}:
        assert path.read_bytes() == content


def test_finish_check_catches_earlier_input_changed_during_later_recheck(bundle, monkeypatch):
    verified = bundle.read()
    original = candidates._Inputs.open
    first = verified._verified_inputs[0][0].path
    calls = []

    @contextmanager
    def raced(inputs, path, limit):
        with original(inputs, path, limit) as opened:
            calls.append(path)
            if len(calls) == 2:
                info = first.stat()
                first.write_bytes(first.read_bytes())
                os.utime(first, ns=(info.st_atime_ns, info.st_mtime_ns))
            yield opened

    monkeypatch.setattr(candidates._Inputs, "open", raced)
    with pytest.raises(candidates.CandidateArtifactError, match="changed during verification"):
        recheck(bundle, verified)
    assert len(calls) == len(verified._verified_inputs)
