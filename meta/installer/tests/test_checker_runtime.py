import hashlib
import io
from pathlib import Path
import tarfile

import pytest

from launchkit import checker_runtime as checker


def archive_bytes(name=None, *, kind=tarfile.REGTYPE, extra=None, target="arm64"):
    stream = io.BytesIO()
    with tarfile.open(fileobj=stream, mode="w:gz") as archive:
        root = checker._archive_name(target).removesuffix(".tar.gz")
        for path, entry_kind in [(name or root + "/bin/node", kind), *(([extra] if extra else []))]:
            entry = tarfile.TarInfo(path)
            entry.type, entry.mode = entry_kind, 0o755
            entry.size = 12 if entry_kind == tarfile.REGTYPE else 0
            entry.linkname = "../../outside"
            archive.addfile(entry, io.BytesIO(b"checked node") if entry.size else None)
    return stream.getvalue()


def release(monkeypatch, payload=None, *, target="arm64"):
    payload = archive_bytes(target=target) if payload is None else payload
    monkeypatch.setattr(checker, "platform_target", lambda: target)
    monkeypatch.setitem(checker.ARCHIVES, target, hashlib.sha256(payload).hexdigest())
    calls = []
    def download(url):
        calls.append(url)
        assert url == checker.RELEASE_URL + "/" + checker._archive_name(target)
        return payload
    monkeypatch.setattr(checker, "_download", download)
    return calls


def test_explicit_preparation_authenticates_only_node_and_reuses_without_network(tmp_path, monkeypatch):
    payload = archive_bytes(extra=("node-v25.9.0-darwin-arm64/lib/unselected.js", tarfile.REGTYPE))
    calls = release(monkeypatch, payload)
    binary = Path(checker.prepare(tmp_path))
    assert binary.read_bytes() == b"checked node"
    assert binary.stat().st_mode & 0o777 == 0o700
    folder = binary.parent
    assert folder.stat().st_mode & 0o777 == 0o700
    assert (folder / "receipt.json").stat().st_mode & 0o777 == 0o600
    assert {path.name for path in folder.iterdir()} == {"node", "receipt.json"}
    assert checker.prepare(tmp_path) == str(binary)
    assert len(calls) == 1
    assert checker.resolve_node("/configured/node", root=tmp_path) == str(binary)


def test_absent_private_checker_preserves_configured_runtime_without_download(tmp_path, monkeypatch):
    monkeypatch.setattr(checker, "_download", lambda *_: pytest.fail("Checks must not download"))
    assert checker.resolve_node("/configured/node", root=tmp_path) == "/configured/node"
    assert checker.resolve_node(None, root=tmp_path) is None


@pytest.mark.parametrize("change", ["binary", "receipt", "mode", "link", "extra", "directory_mode", "hardlink"])
def test_tampered_private_cache_refuses_without_fallback_or_overwrite(tmp_path, monkeypatch, change):
    calls = release(monkeypatch)
    binary = Path(checker.prepare(tmp_path))
    receipt = binary.parent / "receipt.json"
    if change == "binary":
        binary.write_bytes(b"changed")
    elif change == "receipt":
        receipt.write_text("{}")
    elif change == "mode":
        binary.chmod(0o755)
    elif change == "link":
        binary.unlink()
        binary.symlink_to(tmp_path / "foreign")
    elif change == "extra":
        (binary.parent / "unexpected").write_text("foreign")
    elif change == "directory_mode":
        binary.parent.chmod(0o755)
    elif change == "hardlink":
        import os
        os.link(binary, tmp_path / "other-link")
    with pytest.raises(checker.CheckerRuntimeError):
        checker.resolve_node("/configured/node", root=tmp_path)
    with pytest.raises(checker.CheckerRuntimeError):
        checker.prepare(tmp_path)
    assert len(calls) == 1


def test_checksum_failure_does_not_create_cache(tmp_path, monkeypatch):
    release(monkeypatch)
    monkeypatch.setitem(checker.ARCHIVES, "arm64", "0" * 64)
    with pytest.raises(checker.CheckerRuntimeError, match="checksum"):
        checker.prepare(tmp_path)
    assert not (tmp_path / ".tools").exists()


@pytest.mark.parametrize("payload", [
    archive_bytes(name="../node"),
    archive_bytes(name="/node-v25.9.0-darwin-arm64/bin/node"),
    archive_bytes(kind=tarfile.SYMTYPE),
    archive_bytes(kind=tarfile.LNKTYPE),
    archive_bytes(name="node-v25.9.0-darwin-arm64/bin/missing"),
    archive_bytes(extra=("node-v25.9.0-darwin-arm64/bin/node", tarfile.REGTYPE)),
    archive_bytes(extra=("node-v25.9.0-darwin-arm64/../../outside", tarfile.REGTYPE)),
    b"not an archive",
])
def test_invalid_archive_never_writes_paths(tmp_path, monkeypatch, payload):
    release(monkeypatch, payload)
    with pytest.raises(checker.CheckerRuntimeError):
        checker.prepare(tmp_path)
    assert not (tmp_path / ".tools").exists()


def test_archive_expansion_bound_precedes_install(tmp_path, monkeypatch):
    release(monkeypatch)
    monkeypatch.setattr(checker, "MAX_EXPANDED_BYTES", 11)
    with pytest.raises(checker.CheckerRuntimeError, match="expanded"):
        checker.prepare(tmp_path)
    assert not (tmp_path / ".tools").exists()


def test_cache_parent_link_cannot_redirect_install(tmp_path, monkeypatch):
    release(monkeypatch)
    foreign = tmp_path / "foreign"
    foreign.mkdir()
    (tmp_path / ".tools").symlink_to(foreign, target_is_directory=True)
    with pytest.raises(checker.CheckerRuntimeError, match="links"):
        checker.prepare(tmp_path)
    assert not list(foreign.iterdir())


def test_concurrent_directory_creation_is_not_overwritten(tmp_path, monkeypatch):
    payload = archive_bytes()
    release(monkeypatch, payload)
    folder = checker._paths(tmp_path)[0]
    def concurrent(_):
        folder.mkdir(parents=True)
        (folder / "foreign").write_text("retained")
        return payload
    monkeypatch.setattr(checker, "_download", concurrent)
    with pytest.raises(FileExistsError):
        checker.prepare(tmp_path)
    assert (folder / "foreign").read_text() == "retained"
    assert not (folder / "node").exists()


@pytest.mark.parametrize("system,machine,libc,kernel,expected", [
    ("Darwin", "arm64", ("", ""), "25.0.0", "arm64"),
    ("Darwin", "x86_64", ("", ""), "25.0.0", "x64"),
    ("Linux", "aarch64", ("glibc", "2.36"), "6.12.0-host", "linux-arm64"),
    ("Linux", "x86_64", ("glibc", "2.28"), "4.18.0", "linux-x64"),
    ("Linux", "x86_64", ("glibc", "2.27"), "6.12.0", None),
    ("Linux", "aarch64", ("musl", "1.2.5"), "6.12.0", None),
    ("Linux", "x86_64", ("glibc", "2.36"), "4.17.0", None),
    ("Linux", "x86_64", ("glibc", "unknown"), "6.12.0", None),
    ("Darwin", "unknown", ("", ""), "25.0.0", None),
    ("Windows", "AMD64", ("", ""), "10", None),
])
def test_only_supported_official_binary_hosts_can_prepare(monkeypatch, system, machine, libc, kernel, expected):
    monkeypatch.setattr(checker.platform, "system", lambda: system)
    monkeypatch.setattr(checker.platform, "machine", lambda: machine)
    monkeypatch.setattr(checker.platform, "libc_ver", lambda: libc)
    monkeypatch.setattr(checker.platform, "release", lambda: kernel)
    if expected is None:
        monkeypatch.setattr(checker, "_download", lambda *_: pytest.fail("Unsupported hosts must not download"))
        with pytest.raises(checker.CheckerRuntimeError, match="macOS"):
            checker.prepare()
    else:
        assert checker.platform_target() == expected


@pytest.mark.parametrize("target", ["linux-arm64", "linux-x64"])
def test_linux_preparation_uses_exact_target_archive_and_reuses_checked_cache(tmp_path, monkeypatch, target):
    import json
    calls = release(monkeypatch, target=target)
    binary = Path(checker.prepare(tmp_path))
    receipt = json.loads((binary.parent / "receipt.json").read_text())
    assert receipt["archive"] == f"node-v25.9.0-{target}.tar.gz"
    assert receipt["archive_sha256"] == checker.ARCHIVES[target]
    assert binary.read_bytes() == b"checked node"
    assert checker.prepare(tmp_path) == str(binary)
    assert len(calls) == 1
    monkeypatch.setattr(checker, "platform_target", lambda: "arm64")
    with pytest.raises(checker.CheckerRuntimeError, match="receipt"):
        checker.resolve_node("/another/node", root=tmp_path)
    assert binary.read_bytes() == b"checked node" and len(calls) == 1


def test_linux_archive_cannot_select_a_darwin_executable(tmp_path, monkeypatch):
    release(monkeypatch, archive_bytes(), target="linux-arm64")
    with pytest.raises(checker.CheckerRuntimeError, match="layout"):
        checker.prepare(tmp_path)
    assert not (tmp_path / ".tools").exists()


@pytest.mark.parametrize("redirect,oversized", [(True, False), (False, True)])
def test_download_rejects_redirect_and_oversize_response(monkeypatch, redirect, oversized):
    class Response:
        def __enter__(self): return self
        def __exit__(self, *_): pass
        def geturl(self): return "https://foreign.invalid/node" if redirect else "https://nodejs.org/exact"
        def read(self, limit):
            assert limit == checker.MAX_ARCHIVE_BYTES + 1
            return b"x" * limit if oversized else b"content"
    monkeypatch.setattr(checker, "MAX_ARCHIVE_BYTES", 10)
    monkeypatch.setattr(checker.urllib.request, "urlopen", lambda *args, **kwargs: Response())
    with pytest.raises(checker.CheckerRuntimeError):
        checker._download("https://nodejs.org/exact")
