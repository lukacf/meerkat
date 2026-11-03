"""Install the pinned local studio gateway from verified release archives."""

from __future__ import annotations

import argparse
import hashlib
import io
import json
import os
from pathlib import Path
import platform
import re
import tarfile
import tempfile
import urllib.request

VERSION = "0.8.46"
CORE_VERSION = "0.8.51"
RELEASE_URL = f"https://github.com/lukacf/meerkat-mobkit/releases/download/v{VERSION}"
MAX_ARCHIVE_BYTES = 256 * 1024 * 1024
MAX_BINARY_BYTES = 512 * 1024 * 1024
HEX_DIGEST = re.compile(r"^[0-9a-f]{64}$")


class GatewayInstallError(RuntimeError):
    """A gateway could not be verified or installed."""


def platform_target() -> str:
    architecture = {"arm64": "aarch64", "aarch64": "aarch64", "x86_64": "x86_64", "AMD64": "x86_64"}.get(platform.machine())
    system = platform.system()
    suffix = {"Darwin": "apple-darwin", "Linux": "unknown-linux-gnu"}.get(system)
    if not architecture or not suffix:
        raise GatewayInstallError("The studio gateway supports macOS and glibc Linux on Intel or ARM64.")
    if system == "Linux":
        libc, version = platform.libc_ver()
        try:
            compatible = libc == "glibc" and tuple(int(p) for p in version.split(".")[:2]) >= (2, 31)
        except ValueError:
            compatible = False
        if not compatible:
            raise GatewayInstallError("The Linux gateway requires glibc 2.31 or newer. Alpine/musl is not supported.")
    return f"{architecture}-{suffix}"


def _download(url: str, limit: int) -> bytes:
    request = urllib.request.Request(url, headers={"User-Agent": "MobKit-Launch/1", "Accept": "application/octet-stream"})
    try:
        with urllib.request.urlopen(request, timeout=30) as response:
            if not response.geturl().startswith("https://"):
                raise GatewayInstallError("The gateway download redirected to an insecure URL.")
            chunks = []
            size = 0
            while chunk := response.read(min(1024 * 1024, limit + 1 - size)):
                size += len(chunk)
                if size > limit:
                    raise GatewayInstallError("The gateway download exceeded its size limit.")
                chunks.append(chunk)
            return b"".join(chunks)
    except OSError as error:
        raise GatewayInstallError("Could not download the official gateway release. Check network access and try again.") from error


def _file_digest(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        while chunk := handle.read(1024 * 1024):
            digest.update(chunk)
    return digest.hexdigest()


def _verified_cache(destination: Path, receipt: Path, archive_name: str) -> bool:
    if not destination.is_file() or destination.is_symlink() or not receipt.is_file() or receipt.is_symlink():
        return False
    try:
        if receipt.stat().st_mode & 0o077:
            return False
        data = json.loads(receipt.read_text())
        if (not isinstance(data, dict) or data.get("version") != VERSION or data.get("core_version") != CORE_VERSION
                or data.get("archive") != archive_name
                or data.get("release_url") != RELEASE_URL
                or not HEX_DIGEST.fullmatch(str(data.get("archive_sha256", "")))
                or not HEX_DIGEST.fullmatch(str(data.get("binary_sha256", "")))):
            return False
        return os.access(destination, os.X_OK) and _file_digest(destination) == data["binary_sha256"]
    except (OSError, ValueError, TypeError):
        return False


def _atomic_write(path: Path, content: bytes, mode: int) -> None:
    temporary = None
    try:
        with tempfile.NamedTemporaryFile(prefix=f".{path.name}-", dir=path.parent, delete=False) as handle:
            temporary = Path(handle.name)
            os.fchmod(handle.fileno(), mode)
            handle.write(content)
            handle.flush()
            os.fsync(handle.fileno())
        os.replace(temporary, path)
    finally:
        if temporary is not None:
            temporary.unlink(missing_ok=True)


def download_gateway(destination: Path, *, binary_name: str = "rpc_gateway") -> Path:
    """Install the pinned gateway locally, reusing only verified cached bytes."""
    destination = Path(destination).expanduser().absolute()
    target = platform_target()
    archive_prefix = {"rpc_gateway": "mobkit-rpc-gateway", "mobkit_gateway": "mobkit-gateway"}.get(binary_name)
    if archive_prefix is None:
        raise GatewayInstallError("Choose an official MobKit runtime or maintenance binary.")
    archive_name = f"{archive_prefix}-{VERSION}-{target}.tar.gz"
    receipt = destination.with_name(destination.name + ".receipt.json")
    if _verified_cache(destination, receipt, archive_name):
        return destination

    checksum_bytes = _download(f"{RELEASE_URL}/checksums.sha256", 1024 * 1024)
    try:
        entries = [line.split() for line in checksum_bytes.decode("ascii").splitlines()]
    except UnicodeDecodeError as error:
        raise GatewayInstallError("The release checksum manifest is invalid.") from error
    matches = [row[0] for row in entries if len(row) == 2 and row[1].lstrip("*") == archive_name]
    if len(matches) != 1 or not HEX_DIGEST.fullmatch(matches[0]):
        raise GatewayInstallError("The release checksum manifest must name this archive exactly once.")
    archive_bytes = _download(f"{RELEASE_URL}/{archive_name}", MAX_ARCHIVE_BYTES)
    archive_digest = hashlib.sha256(archive_bytes).hexdigest()
    if archive_digest != matches[0]:
        raise GatewayInstallError("Gateway download checksum mismatch. The existing gateway was not changed.")
    try:
        with tarfile.open(fileobj=io.BytesIO(archive_bytes), mode="r:gz") as archive:
            members = archive.getmembers()
            if (len(members) != 1 or members[0].name != binary_name or not members[0].isfile()
                    or not 0 < members[0].size <= MAX_BINARY_BYTES):
                raise GatewayInstallError("The gateway archive has an unexpected layout. Nothing was installed.")
            handle = archive.extractfile(members[0])
            if handle is None:
                raise GatewayInstallError("The gateway archive is missing its executable.")
            binary = handle.read(MAX_BINARY_BYTES + 1)
            if len(binary) != members[0].size:
                raise GatewayInstallError("The gateway archive is incomplete.")
    except (tarfile.TarError, EOFError, OSError) as error:
        raise GatewayInstallError("The gateway archive could not be read. Nothing was installed.") from error

    destination.parent.mkdir(parents=True, mode=0o700, exist_ok=True)
    _atomic_write(destination, binary, 0o700)
    receipt_data = {"version": VERSION, "core_version": CORE_VERSION, "archive": archive_name, "release_url": RELEASE_URL,
                    "archive_sha256": archive_digest, "binary_sha256": hashlib.sha256(binary).hexdigest()}
    _atomic_write(receipt, (json.dumps(receipt_data, indent=2) + "\n").encode(), 0o600)
    return destination


def main() -> None:
    parser = argparse.ArgumentParser(description="Install the verified MobKit gateway for the local studio")
    parser.add_argument("--destination", type=Path, default=Path(__file__).resolve().parents[1] / ".tools/rpc_gateway")
    parser.add_argument("--maintenance", action="store_true", help="Also install the native storage maintenance tool")
    args = parser.parse_args()
    try:
        path = download_gateway(args.destination)
        if args.maintenance:
            download_gateway(args.destination.with_name("mobkit_gateway"), binary_name="mobkit_gateway")
    except (GatewayInstallError, OSError) as error:
        parser.exit(1, f"Gateway setup needs attention: {error}\n")
    print(f"Verified MobKit {VERSION} gateway: {path}")


if __name__ == "__main__":
    main()
