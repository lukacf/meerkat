#!/usr/bin/env python3
"""Install the pinned runtime and native maintenance tools from verified releases."""
import gzip
import hashlib
import io
import json
import os
from pathlib import Path
import platform
import re
import stat
import tarfile
import tempfile
import time
from urllib.parse import urlsplit
import urllib.request

ROOT = Path(__file__).resolve().parent
VERSION = json.loads((ROOT / "deployment.json").read_text())["mobkitVersion"]
RELEASE_URL = f"https://github.com/lukacf/meerkat-mobkit/releases/download/v{VERSION}"
ARCHITECTURES = {"arm64": "aarch64", "aarch64": "aarch64", "x86_64": "x86_64", "AMD64": "x86_64"}
MAX_MANIFEST_BYTES = 1024 * 1024
MAX_ARCHIVE_BYTES = 256 * 1024 * 1024
MAX_BINARY_BYTES = 512 * 1024 * 1024
MAX_TAR_OVERHEAD = 1024 * 1024
PROGRESS_SECONDS = 5
PROGRESS_BYTES = 8 * 1024 * 1024
HEX_DIGEST = re.compile(r"^[0-9a-f]{64}$")
BINARIES = {"rpc_gateway": "mobkit-rpc-gateway", "mobkit_gateway": "mobkit-gateway"}


def platform_target():
    architecture = ARCHITECTURES.get(platform.machine())
    system = platform.system()
    suffix = {"Darwin": "apple-darwin", "Linux": "unknown-linux-gnu"}.get(system)
    if not architecture or not suffix:
        raise SystemExit("The runtime and maintenance tools support macOS and glibc Linux on Intel or ARM64.")
    if system == "Linux":
        libc, version = platform.libc_ver()
        try:
            compatible = libc == "glibc" and tuple(int(part) for part in version.split(".")[:2]) >= (2, 31)
        except ValueError:
            compatible = False
        if not compatible:
            raise SystemExit("The Linux tools require glibc 2.31 or newer. Alpine/musl is not supported.")
    return f"{architecture}-{suffix}"


def download(url, limit=MAX_ARCHIVE_BYTES):
    request = urllib.request.Request(url, headers={"User-Agent": "MobKit-Launch/1", "Accept": "application/octet-stream"})
    # Display only the requested asset name, never URLs, redirects or auth data.
    asset = re.sub(r"[^A-Za-z0-9_.-]", "_", urlsplit(url).path.rsplit("/", 1)[-1])[:200] or "release asset"
    print(f"Requesting official release asset: {asset}", flush=True)
    try:
        with urllib.request.urlopen(request, timeout=30) as response:
            if not response.geturl().startswith("https://"):
                raise SystemExit("The release download redirected to an insecure URL.")
            try:
                total = int(getattr(response, "headers", {}).get("Content-Length", ""))
            except (TypeError, ValueError):
                total = None
            if total is not None and total <= 0:
                total = None
            chunks = []
            size = 0
            reported_size, reported_at = 0, time.monotonic()
            expected = f"Content-Length: {total:,} bytes" if total is not None else "Content-Length not provided"
            print(f"Receiving {asset} ({expected}).", flush=True)
            while chunk := response.read(min(64 * 1024, limit + 1 - size)):
                size += len(chunk)
                if size > limit:
                    raise SystemExit("The release download exceeded its size limit.")
                chunks.append(chunk)
                timestamp = time.monotonic()
                if timestamp - reported_at >= PROGRESS_SECONDS or size - reported_size >= PROGRESS_BYTES:
                    declared = f" / {total:,} declared bytes" if total is not None else " bytes"
                    print(f"{asset}: {size:,}{declared} received.", flush=True)
                    reported_size, reported_at = size, timestamp
            print(f"Received {asset}: {size:,} bytes.", flush=True)
            if total is not None and size != total:
                raise SystemExit("The release download was incomplete. Retry installation; no binary was installed.")
            return b"".join(chunks)
    except OSError as error:
        raise SystemExit("Could not download the official release. Check network access and try again.") from error


def verified_binary(name, target, manifest):
    archive_name = f"{BINARIES[name]}-{VERSION}-{target}.tar.gz"
    matching = [row[0] for row in manifest if len(row) == 2 and row[1].lstrip("*") == archive_name]
    if len(matching) != 1 or not HEX_DIGEST.fullmatch(matching[0]):
        raise SystemExit(f"The release checksum manifest must name {archive_name} exactly once.")
    archive_bytes = download(f"{RELEASE_URL}/{archive_name}", MAX_ARCHIVE_BYTES)
    if len(archive_bytes) > MAX_ARCHIVE_BYTES:
        raise SystemExit("The release archive exceeded its size limit. No binary was installed.")
    print(f"Verifying SHA-256 for {archive_name}...", flush=True)
    digest = hashlib.sha256(archive_bytes).hexdigest()
    if digest != matching[0]:
        raise SystemExit(f"{name} download checksum mismatch. No binary was installed.")
    print(f"Verified SHA-256 for {archive_name}.", flush=True)
    try:
        # Bound decompression as well as the HTTP response, including tar padding.
        with gzip.GzipFile(fileobj=io.BytesIO(archive_bytes)) as compressed:
            tar_bytes = compressed.read(MAX_BINARY_BYTES + MAX_TAR_OVERHEAD + 1)
        if len(tar_bytes) > MAX_BINARY_BYTES + MAX_TAR_OVERHEAD:
            raise SystemExit("The release archive exceeded its decompressed size limit. No binary was installed.")
        with tarfile.open(fileobj=io.BytesIO(tar_bytes), mode="r:") as archive:
            member = archive.next()
            if (member is None or member.name != name or not member.isfile()
                    or member.sparse is not None or not 0 < member.size <= MAX_BINARY_BYTES):
                raise SystemExit("Unexpected gateway archive structure or binary size. No binary was installed.")
            source = archive.extractfile(member)
            if source is None:
                raise SystemExit("Gateway binary is missing from archive. No binary was installed.")
            binary = source.read(MAX_BINARY_BYTES + 1)
            if len(binary) != member.size:
                raise SystemExit("The release archive is incomplete. No binary was installed.")
            if archive.next() is not None:
                raise SystemExit("Unexpected gateway archive structure. No binary was installed.")
    except (tarfile.TarError, EOFError, OSError) as error:
        raise SystemExit("The release archive could not be read. No binary was installed.") from error
    receipt = {
        "version": VERSION, "archive": archive_name, "release_url": RELEASE_URL,
        "archive_sha256": digest, "binary_sha256": hashlib.sha256(binary).hexdigest(),
    }
    return binary, receipt


def atomic_write(destination, payload, mode):
    temporary = None
    try:
        with tempfile.NamedTemporaryFile(prefix=f".{destination.name}-", dir=destination.parent, delete=False) as handle:
            temporary = Path(handle.name)
            os.fchmod(handle.fileno(), mode)
            handle.write(payload)
            handle.flush()
            os.fsync(handle.fileno())
        temporary.replace(destination)
    finally:
        if temporary is not None:
            temporary.unlink(missing_ok=True)


def verified_cache(directory, name, target):
    """Reuse only owned private bytes with an exact pinned official receipt."""
    destination, receipt = directory / name, directory / (name + ".receipt.json")
    try:
        directory_info = directory.stat()
        if (directory.is_symlink() or directory_info.st_uid != os.getuid()
                or directory_info.st_mode & 0o077):
            return False
        for path, limit in ((destination, MAX_BINARY_BYTES), (receipt, MAX_MANIFEST_BYTES)):
            info = path.lstat()
            if (not stat.S_ISREG(info.st_mode) or info.st_nlink != 1
                    or info.st_uid != os.getuid() or info.st_mode & 0o077
                    or not 0 < info.st_size <= limit):
                return False
        if not os.access(destination, os.X_OK):
            return False
        with receipt.open("rb") as handle:
            payload = handle.read(MAX_MANIFEST_BYTES + 1)
        if len(payload) > MAX_MANIFEST_BYTES:
            return False
        data = json.loads(payload)
        archive = f"{BINARIES[name]}-{VERSION}-{target}.tar.gz"
        if (not isinstance(data, dict) or data.get("source") is not None
                or data.get("version") != VERSION or data.get("archive") != archive
                or data.get("release_url") != RELEASE_URL
                or not HEX_DIGEST.fullmatch(str(data.get("archive_sha256", "")))
                or not HEX_DIGEST.fullmatch(str(data.get("binary_sha256", "")))):
            return False
        digest, size = hashlib.sha256(), 0
        with destination.open("rb") as handle:
            while chunk := handle.read(min(1024 * 1024, MAX_BINARY_BYTES + 1 - size)):
                size += len(chunk)
                if size > MAX_BINARY_BYTES:
                    return False
                digest.update(chunk)
        return size > 0 and digest.hexdigest() == data["binary_sha256"]
    except (OSError, ValueError, TypeError):
        return False


def main():
    target = platform_target()
    directory = ROOT / ".bin"
    if directory.is_symlink() or directory.exists() and not directory.is_dir():
        raise SystemExit("The local .bin directory must be an ordinary directory, not a symbolic link.")
    override = os.environ.get("MOBKIT_RPC_GATEWAY_BIN")
    pending, reused = {}, set()
    if override:
        source = Path(override).expanduser().resolve(strict=True)
        if not source.is_file() or not os.access(source, os.X_OK):
            raise SystemExit("The explicitly selected gateway must be an executable file.")
        with source.open("rb") as handle:
            binary = handle.read(MAX_BINARY_BYTES + 1)
        if not 0 < len(binary) <= MAX_BINARY_BYTES:
            raise SystemExit("The explicitly selected gateway has an invalid size.")
        pending["rpc_gateway"] = (binary, {
            "version": VERSION, "source": "explicit_override",
            "binary_sha256": hashlib.sha256(binary).hexdigest(),
        })
    for name in BINARIES:
        if name not in pending and verified_cache(directory, name, target):
            reused.add(name)
            print(f"Reusing verified official MobKit {VERSION} .bin/{name}; binary SHA-256 checked.", flush=True)
    required = [name for name in BINARIES if name not in pending and name not in reused]
    if required:
        print(f"Downloading MobKit {VERSION} runtime and maintenance tools for {target}...", flush=True)
        checksum_bytes = download(f"{RELEASE_URL}/checksums.sha256", MAX_MANIFEST_BYTES)
        if len(checksum_bytes) > MAX_MANIFEST_BYTES:
            raise SystemExit("The release checksum manifest exceeded its size limit.")
        try:
            manifest = [line.split() for line in checksum_bytes.decode("ascii").splitlines()]
        except UnicodeDecodeError as error:
            raise SystemExit("The release checksum manifest is invalid.") from error
    # Verify every required binary before replacing any installed executable.
    for name in required:
        pending[name] = verified_binary(name, target, manifest)
    print("All required native binaries passed verification. Preparing local writes.", flush=True)
    directory.mkdir(mode=0o700, exist_ok=True)
    os.chmod(directory, 0o700)
    for name, (binary, receipt) in pending.items():
        print(f"Writing native executable .bin/{name} ({len(binary):,} bytes)...", flush=True)
        atomic_write(directory / name, binary, 0o700)
        atomic_write(directory / (name + ".receipt.json"), (json.dumps(receipt, indent=2) + "\n").encode(), 0o600)
        print(f"Wrote .bin/{name} and its verification receipt.", flush=True)
        if receipt.get("source") == "explicit_override":
            print("Using explicitly selected local rpc_gateway. Verify its version matches requirements.txt.", flush=True)
        else:
            print(f"{name} installed. Archive SHA-256: {receipt['archive_sha256']}", flush=True)


if __name__ == "__main__":
    main()
