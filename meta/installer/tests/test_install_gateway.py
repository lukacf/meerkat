"""Generated installer recovery preserves pinned release verification."""
import hashlib
import importlib.util
import io
import json
from pathlib import Path
import tarfile

import pytest


def installer(tmp_path, monkeypatch):
    source = Path(__file__).resolve().parents[1] / "launchkit/templates/install_gateway.py"
    (tmp_path / "install_gateway.py").write_text(source.read_text())
    (tmp_path / "deployment.json").write_text(json.dumps({"mobkitVersion": "0.8.42"}))
    spec = importlib.util.spec_from_file_location("isolated_gateway_installer", tmp_path / "install_gateway.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    monkeypatch.delenv("MOBKIT_RPC_GATEWAY_BIN", raising=False)
    monkeypatch.setattr(module, "platform_target", lambda: "aarch64-apple-darwin")
    return module


def official_assets(module, monkeypatch):
    assets = {}
    for name, prefix in module.BINARIES.items():
        stream = io.BytesIO()
        with tarfile.open(fileobj=stream, mode="w:gz") as archive:
            binary = b"verified official fixture " + name.encode()
            member = tarfile.TarInfo(name)
            member.size = len(binary)
            archive.addfile(member, io.BytesIO(binary))
        assets[f"{prefix}-{module.VERSION}-aarch64-apple-darwin.tar.gz"] = stream.getvalue()
    assets["checksums.sha256"] = "".join(
        hashlib.sha256(payload).hexdigest() + "  " + name + "\n"
        for name, payload in assets.items()).encode()
    calls = []
    def download(url, limit):
        name = url.rsplit("/", 1)[-1]
        calls.append(name)
        assert url.startswith(module.RELEASE_URL + "/")
        assert len(assets[name]) <= limit
        return assets[name]
    monkeypatch.setattr(module, "download", download)
    return assets, calls


def test_reinstallation_rehashes_owned_official_cache_without_network_or_rewrites(tmp_path, monkeypatch):
    module = installer(tmp_path, monkeypatch)
    _, calls = official_assets(module, monkeypatch)
    module.main()
    assert len(calls) == 3
    paths = list((tmp_path / ".bin").iterdir())
    saved = {path.name: (path.read_bytes(), path.stat().st_mtime_ns) for path in paths}
    monkeypatch.setattr(module, "download", lambda *args: pytest.fail("A complete verified cache must not download again"))
    module.main()
    assert saved == {path.name: (path.read_bytes(), path.stat().st_mtime_ns) for path in paths}


@pytest.mark.parametrize("invalid", [
    "modified-binary", "modified-receipt", "missing-receipt", "bad-json", "wrong-version",
    "wrong-target", "wrong-release", "invalid-archive-hash", "explicit-override",
    "public-binary", "public-receipt", "not-executable", "binary-link", "receipt-link",
    "binary-hardlink", "receipt-hardlink", "oversized-binary", "oversized-receipt",
])
def test_invalid_cache_redownloads_only_the_affected_official_binary(tmp_path, monkeypatch, invalid):
    module = installer(tmp_path, monkeypatch)
    assets, calls = official_assets(module, monkeypatch)
    module.main()
    calls.clear()
    binary, receipt = tmp_path / ".bin/rpc_gateway", tmp_path / ".bin/rpc_gateway.receipt.json"
    unchanged = tmp_path / ".bin/mobkit_gateway"
    previous = (unchanged.read_bytes(), unchanged.stat().st_mtime_ns)
    if invalid == "modified-binary":
        binary.write_bytes(b"not the verified bytes")
    elif invalid == "missing-receipt":
        receipt.unlink()
    elif invalid == "bad-json":
        receipt.write_text("[]")
    elif invalid in {"public-binary", "public-receipt", "not-executable"}:
        (receipt if invalid == "public-receipt" else binary).chmod(0o644 if invalid != "not-executable" else 0o600)
    elif invalid.endswith("-link") or invalid.endswith("-hardlink"):
        target = receipt if invalid.startswith("receipt") else binary
        outside = tmp_path / ("outside-" + target.name)
        target.replace(outside)
        if invalid.endswith("-hardlink"):
            target.hardlink_to(outside)
        else:
            target.symlink_to(outside)
    elif invalid == "oversized-binary":
        monkeypatch.setattr(module, "MAX_BINARY_BYTES", len(binary.read_bytes()) - 1)
    elif invalid == "oversized-receipt":
        # The manifest itself remains under this limit, but the padded cached
        # receipt cannot be loaded as an unbounded trusted cache record.
        receipt.write_text(receipt.read_text() + " " * 2000)
        monkeypatch.setattr(module, "MAX_MANIFEST_BYTES", 1000)
    else:
        data = json.loads(receipt.read_text())
        field, value = {
            "modified-receipt": ("binary_sha256", "0" * 64),
            "wrong-version": ("version", "0.0.0"),
            "wrong-target": ("archive", "mobkit-rpc-gateway-0.8.42-x86_64-apple-darwin.tar.gz"),
            "wrong-release": ("release_url", "https://untrusted.example/release"),
            "invalid-archive-hash": ("archive_sha256", "z" * 64),
            "explicit-override": ("source", "explicit_override"),
        }[invalid]
        data[field] = value
        receipt.write_text(json.dumps(data))
    if invalid == "oversized-binary":
        # Restore the ordinary bound only after proving this cache is rejected;
        # the actual newly downloaded fixture must still pass bounded extraction.
        assert not module.verified_cache(tmp_path / ".bin", "rpc_gateway", module.platform_target())
        monkeypatch.setattr(module, "MAX_BINARY_BYTES", 512 * 1024 * 1024)
        binary.write_bytes(b"changed")
    module.main()
    assert calls == ["checksums.sha256", "mobkit-rpc-gateway-0.8.42-aarch64-apple-darwin.tar.gz"]
    assert (unchanged.read_bytes(), unchanged.stat().st_mtime_ns) == previous
    assert module.verified_cache(tmp_path / ".bin", "rpc_gateway", module.platform_target())


def test_failed_replacement_preserves_cached_peer_and_previous_invalid_binary(tmp_path, monkeypatch):
    module = installer(tmp_path, monkeypatch)
    assets, calls = official_assets(module, monkeypatch)
    module.main()
    binary = tmp_path / ".bin/mobkit_gateway"
    binary.write_bytes(b"previous bytes needing repair")
    saved = {path.name: path.read_bytes() for path in (tmp_path / ".bin").iterdir()}
    name = "mobkit-gateway-0.8.42-aarch64-apple-darwin.tar.gz"
    assets[name] = b"corrupted download"
    with pytest.raises(SystemExit, match="checksum mismatch"):
        module.main()
    assert saved == {path.name: path.read_bytes() for path in (tmp_path / ".bin").iterdir()}


def test_cache_does_not_follow_a_symbolic_binary_directory(tmp_path, monkeypatch):
    module = installer(tmp_path, monkeypatch)
    outside = tmp_path / "outside"
    outside.mkdir()
    (tmp_path / ".bin").symlink_to(outside, target_is_directory=True)
    monkeypatch.setattr(module, "download", lambda *args: pytest.fail("Reject the unsafe destination before any download"))
    with pytest.raises(SystemExit, match="ordinary directory"):
        module.main()
    assert not list(outside.iterdir())


def test_content_length_truncation_never_returns_incomplete_archive_bytes(tmp_path, monkeypatch):
    module = installer(tmp_path, monkeypatch)
    class Response(io.BytesIO):
        headers = {"Content-Length": "10"}
        def geturl(self):
            return "https://release.example/archive"
    monkeypatch.setattr(module.urllib.request, "urlopen", lambda *args, **kwargs: Response(b"truncated"))
    with pytest.raises(SystemExit, match="incomplete"):
        module.download("https://release.example/archive", 20)


def test_explicit_override_never_becomes_an_official_cached_gateway(tmp_path, monkeypatch):
    module = installer(tmp_path, monkeypatch)
    _, calls = official_assets(module, monkeypatch)
    module.main()
    selected = tmp_path / "explicit-selected-rpc"
    selected.write_bytes(b"explicitly selected executable")
    selected.chmod(0o700)
    monkeypatch.setenv("MOBKIT_RPC_GATEWAY_BIN", str(selected))
    calls.clear()
    module.main()
    assert not calls
    assert not module.verified_cache(tmp_path / ".bin", "rpc_gateway", module.platform_target())
    assert (tmp_path / ".bin/rpc_gateway").read_bytes() == selected.read_bytes()
