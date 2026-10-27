"""Explicit preparation of the private, pinned JavaScript checker.

Checks only resolve an existing runtime. They never download or install one.
This executable does not replace Node used by the frontend or MCP connectors.
"""
from __future__ import annotations

import argparse
import hashlib
import io
import json
import os
from pathlib import Path, PurePosixPath
import platform
import stat
import tarfile
import urllib.request

VERSION = "25.9.0"
RELEASE_URL = f"https://nodejs.org/dist/v{VERSION}"
# Official SHASUMS256.txt, checked independently before introducing these pins.
ARCHIVES = {
    "arm64": "e479f3c469d3d9303a44f00a8ea37a3788395d171bb8059c48a4bbbd2e371b59",
    "x64": "7d737b53ce191142bfa1c17cfa5b070d96e84eebf76b8dd06d84981cbdc3f7e3",
    "linux-arm64": "8fb4283301b8c720fc9f18bffff0f659e72cc14d0cf207a3bb411808aaa73a57",
    "linux-x64": "134e55b2408448a219760fe04dc44d6851f9de8a79549021ffd870e9082d9e7b",
}
MAX_ARCHIVE_BYTES = 128 * 1024 * 1024
MAX_BINARY_BYTES = 256 * 1024 * 1024
MAX_EXPANDED_BYTES = 512 * 1024 * 1024


class CheckerRuntimeError(ValueError):
    pass


def platform_target():
    architecture = {"arm64": "arm64", "aarch64": "arm64", "x86_64": "x64", "AMD64": "x64"}.get(platform.machine())
    system = platform.system()
    if system == "Darwin" and architecture is not None:
        # Keep existing Darwin target keys and receipt bytes unchanged.
        return architecture
    if system == "Linux" and architecture is not None:
        libc, version = platform.libc_ver()
        try:
            supported_libc = libc == "glibc" and tuple(int(part) for part in version.split(".")) >= (2, 28)
            kernel = tuple(int(part) for part in platform.release().split("-", 1)[0].split(".")[:2])
        except ValueError:
            supported_libc, kernel = False, ()
        if supported_libc and kernel >= (4, 18):
            return "linux-" + architecture
    raise CheckerRuntimeError("Private checker preparation supports macOS Intel/ARM64 and Linux Intel/ARM64 with glibc 2.28+ and kernel 4.18+. Executable checks also require a supported OS sandbox.")


def _archive_name(target):
    if target not in ARCHIVES:
        raise CheckerRuntimeError("The checker target has no reviewed official archive.")
    suffix = target if target.startswith("linux-") else "darwin-" + target
    return f"node-v{VERSION}-{suffix}.tar.gz"


def _paths(root=None):
    root = Path(root) if root is not None else Path(__file__).resolve().parents[1]
    folder = root.absolute() / ".tools" / f"checker-node-{VERSION}"
    return folder, folder / "node", folder / "receipt.json"


def _no_links(path):
    if any(parent.is_symlink() for parent in (path, *path.parents)):
        raise CheckerRuntimeError("The private checker path cannot contain links.")


def _owned_file(path, mode, limit):
    _no_links(path)
    descriptor = os.open(path, os.O_RDONLY | os.O_NOFOLLOW)
    try:
        before = os.fstat(descriptor)
        if (not stat.S_ISREG(before.st_mode) or before.st_uid != os.getuid() or before.st_nlink != 1
                or before.st_mode & 0o777 != mode or not 0 < before.st_size <= limit):
            raise CheckerRuntimeError("The private checker must use bounded, ordinary, owned files with its original modes.")
        with os.fdopen(os.dup(descriptor), "rb") as handle:
            content = handle.read(limit + 1)
        after = os.fstat(descriptor)
        if len(content) != before.st_size or (before.st_size, before.st_mtime_ns) != (after.st_size, after.st_mtime_ns):
            raise CheckerRuntimeError("The private checker changed while being checked.")
        return content
    finally:
        os.close(descriptor)


def prepared_node(root=None):
    """Return a checked private binary, None if absent; refuse a damaged cache."""
    folder, binary, receipt = _paths(root)
    _no_links(folder)
    if not folder.exists():
        return None
    try:
        info = folder.stat()
        if not stat.S_ISDIR(info.st_mode) or info.st_uid != os.getuid() or info.st_mode & 0o777 != 0o700:
            raise CheckerRuntimeError("The private checker directory must remain owned and private.")
        architecture = platform_target()
        data = json.loads(_owned_file(receipt, 0o600, 4096))
        content = _owned_file(binary, 0o700, MAX_BINARY_BYTES)
        expected = {"version": VERSION, "archive": _archive_name(architecture),
                    "archive_sha256": ARCHIVES[architecture], "release_url": RELEASE_URL,
                    "binary_sha256": hashlib.sha256(content).hexdigest(), "binary_bytes": len(content)}
        if data != expected or {path.name for path in folder.iterdir()} != {"node", "receipt.json"}:
            raise CheckerRuntimeError("The private checker no longer matches its prepared receipt.")
    except (OSError, ValueError, TypeError) as error:
        if isinstance(error, CheckerRuntimeError):
            raise
        raise CheckerRuntimeError("The private checker cache is incomplete or changed; prepare it in a fresh installer copy.") from None
    return str(binary)


def resolve_node(configured=None, *, root=None):
    """Prefer explicitly prepared bytes; preserve the existing PATH fallback."""
    return prepared_node(root) or configured


def _download(url):
    request = urllib.request.Request(url, headers={"User-Agent": "Meta-Forge-Checker/1"})
    try:
        with urllib.request.urlopen(request, timeout=30) as response:
            if response.geturl() != url:
                raise CheckerRuntimeError("The checker download redirected away from its pinned official URL.")
            content = response.read(MAX_ARCHIVE_BYTES + 1)
            if not 0 < len(content) <= MAX_ARCHIVE_BYTES:
                raise CheckerRuntimeError("The checker archive exceeds its download bound.")
            return content
    except OSError:
        raise CheckerRuntimeError("The official checker download failed; no checker was installed.") from None


def _binary(content, architecture):
    if hashlib.sha256(content).hexdigest() != ARCHIVES[architecture]:
        raise CheckerRuntimeError("The checker archive checksum does not match its official pin.")
    root = _archive_name(architecture).removesuffix(".tar.gz")
    selected, seen, total = None, set(), 0
    try:
        with tarfile.open(fileobj=io.BytesIO(content), mode="r:gz") as archive:
            for count, member in enumerate(archive, 1):
                name = member.name.rstrip("/")
                path = PurePosixPath(name)
                if (count > 10000 or path.is_absolute() or path.as_posix() != name
                        or any(part in ("", ".", "..") for part in path.parts)
                        or not path.parts or path.parts[0] != root or name in seen
                        or any(ord(char) < 32 for char in name)):
                    raise CheckerRuntimeError("The checker archive has an unexpected layout.")
                seen.add(name)
                total += member.size
                if member.size < 0 or total > MAX_EXPANDED_BYTES:
                    raise CheckerRuntimeError("The checker archive exceeds its expanded bound.")
                # Only this regular member is read. No archive paths or links
                # are extracted, and no npm, addon or arbitrary script runs.
                if name == root + "/bin/node":
                    if not member.isfile() or not 0 < member.size <= MAX_BINARY_BYTES or not member.mode & 0o111:
                        raise CheckerRuntimeError("The checker archive does not contain a regular executable.")
                    handle = archive.extractfile(member)
                    selected = handle.read(MAX_BINARY_BYTES + 1) if handle is not None else None
                    if selected is None or len(selected) != member.size:
                        raise CheckerRuntimeError("The checker executable is incomplete.")
    except (tarfile.TarError, OSError, EOFError):
        raise CheckerRuntimeError("The checker archive could not be read.") from None
    if selected is None:
        raise CheckerRuntimeError("The checker archive is missing bin/node.")
    return selected


def prepare(root=None):
    architecture = platform_target()
    existing = prepared_node(root)
    if existing is not None:
        return existing
    name = _archive_name(architecture)
    content = _binary(_download(RELEASE_URL + "/" + name), architecture)
    folder, binary, receipt = _paths(root)
    _no_links(folder)
    folder.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
    info = folder.parent.stat()
    if info.st_uid != os.getuid() or not stat.S_ISDIR(info.st_mode) or info.st_mode & 0o022:
        raise CheckerRuntimeError("The checker cache parent must be owned and not writable by other users.")
    # An exclusive directory protects a concurrent or foreign preparation.
    # A partial failed install remains visible and cannot be reused silently.
    folder.mkdir(mode=0o700)
    for path, payload, mode in (
        (binary, content, 0o700),
        (receipt, (json.dumps({"version": VERSION, "archive": name, "archive_sha256": ARCHIVES[architecture],
            "release_url": RELEASE_URL, "binary_sha256": hashlib.sha256(content).hexdigest(),
            "binary_bytes": len(content)}, sort_keys=True) + "\n").encode(), 0o600),
    ):
        descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, mode)
        with os.fdopen(descriptor, "wb") as handle:
            os.fchmod(handle.fileno(), mode)
            handle.write(payload)
            handle.flush()
            os.fsync(handle.fileno())
    return prepared_node(root)


def main():
    parser = argparse.ArgumentParser(description="Explicitly prepare the private Node.js 25.9.0 checker")
    parser.parse_args()
    try:
        path = prepare()
    except (CheckerRuntimeError, OSError) as error:
        parser.exit(1, f"Checker preparation needs attention: {error}\n")
    print(f"Verified private Node.js {VERSION} checker: {path}")


if __name__ == "__main__":
    main()
