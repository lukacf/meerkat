from __future__ import annotations

import hashlib
import io
import json
from pathlib import Path
import tarfile

import pytest

from launchkit import bootstrap


def archive_bytes(name="rpc_gateway", kind=tarfile.REGTYPE, content=b"verified executable"):
    stream = io.BytesIO()
    with tarfile.open(fileobj=stream, mode="w:gz") as archive:
        entry = tarfile.TarInfo(name)
        entry.type = kind
        entry.size = len(content) if kind == tarfile.REGTYPE else 0
        if kind == tarfile.SYMTYPE:
            entry.linkname = "/outside"
        archive.addfile(entry, io.BytesIO(content) if entry.size else None)
    return stream.getvalue()


def release(monkeypatch, content=None, checksum=None):
    payload = content if content is not None else archive_bytes()
    name = f"mobkit-rpc-gateway-{bootstrap.VERSION}-aarch64-apple-darwin.tar.gz"
    digest = checksum or hashlib.sha256(payload).hexdigest()
    calls = []
    monkeypatch.setattr(bootstrap, "platform_target", lambda: "aarch64-apple-darwin")
    def download(url, limit):
        calls.append(url)
        assert url.startswith(bootstrap.RELEASE_URL + "/")
        return (digest + "  " + name + "\n").encode() if url.endswith("checksums.sha256") else payload
    monkeypatch.setattr(bootstrap, "_download", download)
    return calls


def test_install_and_reuse_only_verified_cached_binary(tmp_path, monkeypatch):
    calls = release(monkeypatch)
    destination = tmp_path / ".tools" / "rpc_gateway"
    assert bootstrap.download_gateway(destination) == destination
    assert destination.read_bytes() == b"verified executable"
    assert destination.stat().st_mode & 0o777 == 0o700
    receipt = destination.with_name("rpc_gateway.receipt.json")
    assert receipt.stat().st_mode & 0o777 == 0o600
    assert json.loads(receipt.read_text())["binary_sha256"] == hashlib.sha256(destination.read_bytes()).hexdigest()
    assert len(calls) == 2
    bootstrap.download_gateway(destination)
    assert len(calls) == 2
    destination.write_bytes(b"tampered")
    bootstrap.download_gateway(destination)
    assert len(calls) == 4
    assert destination.read_bytes() == b"verified executable"


def test_unreceipted_binary_is_not_trusted(tmp_path, monkeypatch):
    calls = release(monkeypatch)
    destination = tmp_path / "rpc_gateway"
    destination.write_bytes(b"unverified executable")
    bootstrap.download_gateway(destination)
    assert len(calls) == 2
    assert destination.read_bytes() == b"verified executable"


def test_checksum_failure_preserves_previous_binary(tmp_path, monkeypatch):
    release(monkeypatch, checksum="0" * 64)
    destination = tmp_path / "rpc_gateway"
    destination.write_bytes(b"previous gateway")
    with pytest.raises(bootstrap.GatewayInstallError, match="checksum mismatch"):
        bootstrap.download_gateway(destination)
    assert destination.read_bytes() == b"previous gateway"
    assert not destination.with_name("rpc_gateway.receipt.json").exists()


@pytest.mark.parametrize("name,kind", [("../rpc_gateway", tarfile.REGTYPE), ("rpc_gateway", tarfile.SYMTYPE), ("other", tarfile.REGTYPE)])
def test_archive_paths_and_links_cannot_escape(tmp_path, monkeypatch, name, kind):
    release(monkeypatch, content=archive_bytes(name, kind))
    with pytest.raises(bootstrap.GatewayInstallError, match="unexpected layout"):
        bootstrap.download_gateway(tmp_path / "tools" / "rpc_gateway")
    assert not (tmp_path / "tools").exists()


@pytest.mark.parametrize("machine,system,libc,expected", [
    ("arm64", "Darwin", ("", ""), "aarch64-apple-darwin"),
    ("x86_64", "Darwin", ("", ""), "x86_64-apple-darwin"),
    ("aarch64", "Linux", ("glibc", "2.31"), "aarch64-unknown-linux-gnu"),
    ("x86_64", "Linux", ("glibc", "2.36"), "x86_64-unknown-linux-gnu"),
    ("x86_64", "Linux", ("glibc", "2.30"), None),
    ("x86_64", "Linux", ("musl", "1.2"), None),
    ("x86_64", "Windows", ("", ""), None),
])
def test_platform_support(monkeypatch, machine, system, libc, expected):
    monkeypatch.setattr(bootstrap.platform, "machine", lambda: machine)
    monkeypatch.setattr(bootstrap.platform, "system", lambda: system)
    monkeypatch.setattr(bootstrap.platform, "libc_ver", lambda: libc)
    if expected:
        assert bootstrap.platform_target() == expected
    else:
        with pytest.raises(bootstrap.GatewayInstallError):
            bootstrap.platform_target()


def test_world_readable_or_wrong_release_receipt_is_not_reused(tmp_path, monkeypatch):
    calls = release(monkeypatch)
    destination = tmp_path / "rpc_gateway"
    bootstrap.download_gateway(destination)
    receipt = destination.with_name("rpc_gateway.receipt.json")
    receipt.chmod(0o644)
    bootstrap.download_gateway(destination)
    assert len(calls) == 4
    data = json.loads(receipt.read_text())
    data["version"] = "0.0.0"
    receipt.write_text(json.dumps(data))
    bootstrap.download_gateway(destination)
    assert len(calls) == 6


def test_previous_version_cache_and_wrong_core_binding_are_not_reused(tmp_path, monkeypatch):
    calls = release(monkeypatch)
    destination = tmp_path / "rpc_gateway"
    bootstrap.download_gateway(destination)
    receipt = destination.with_name("rpc_gateway.receipt.json")
    data = json.loads(receipt.read_text())
    assert data["version"] == "0.8.47" and data["core_version"] == "0.8.51"
    data["version"] = "0.8.42"
    receipt.write_text(json.dumps(data))
    bootstrap.download_gateway(destination)
    assert len(calls) == 4
    data = json.loads(receipt.read_text())
    data["core_version"] = "0.8.50"
    receipt.write_text(json.dumps(data))
    bootstrap.download_gateway(destination)
    assert len(calls) == 6


@pytest.mark.parametrize("receipt_content", ["[]", "null", "broken JSON"])
def test_malformed_cache_receipt_triggers_verified_download(tmp_path, monkeypatch, receipt_content):
    calls = release(monkeypatch)
    destination = tmp_path / "rpc_gateway"
    bootstrap.download_gateway(destination)
    destination.with_name("rpc_gateway.receipt.json").write_text(receipt_content)
    bootstrap.download_gateway(destination)
    assert len(calls) == 4


def test_native_maintenance_binary_uses_its_own_release_artifact(tmp_path, monkeypatch):
    payload=archive_bytes(name='mobkit_gateway')
    digest=hashlib.sha256(payload).hexdigest()
    filename=f'mobkit-gateway-{bootstrap.VERSION}-aarch64-apple-darwin.tar.gz'
    monkeypatch.setattr(bootstrap,'platform_target',lambda:'aarch64-apple-darwin')
    monkeypatch.setattr(bootstrap,'_download',lambda url,limit:(digest+'  '+filename+'\n').encode() if url.endswith('checksums.sha256') else payload)
    result=bootstrap.download_gateway(tmp_path/'mobkit_gateway',binary_name='mobkit_gateway')
    assert result.read_bytes()==b'verified executable'
    assert json.loads(result.with_name('mobkit_gateway.receipt.json').read_text())['archive']==filename
    with pytest.raises(bootstrap.GatewayInstallError):
        bootstrap.download_gateway(tmp_path/'invented',binary_name='invented')


def test_package_tool_path_rejects_non_official_binary_name(tmp_path):
    with pytest.raises(bootstrap.GatewayInstallError):
        bootstrap.package_tool_path(tmp_path, "../../old-tools/mobkit_gateway")
