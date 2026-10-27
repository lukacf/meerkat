"""Reuse Forge's verified official release bytes in an owned deployment."""
from __future__ import annotations

import hashlib
import json
import os
from pathlib import Path
import tempfile

from . import bootstrap


def seed_release_cache(destination, definition, *, source_directory=None):
    """Copy both official cache entries, preserving their verification receipts.

    This is a host optimization, not an agent-controlled executable override.
    Missing or ineligible cached bytes leave the normal release installer in charge.
    """
    if definition.get("mobkitVersion") != bootstrap.VERSION:
        return []
    source = Path(source_directory) if source_directory is not None else Path(__file__).resolve().parents[1] / ".tools"
    if (not source.is_dir() or source.is_symlink() or source.stat().st_uid != os.getuid()
            or source.stat().st_mode & 0o077):
        return []
    target = bootstrap.platform_target()
    entries = []
    for name, prefix in (("rpc_gateway", "mobkit-rpc-gateway"), ("mobkit_gateway", "mobkit-gateway")):
        binary = source / name
        receipt = source / (name + ".receipt.json")
        archive = f"{prefix}-{bootstrap.VERSION}-{target}.tar.gz"
        if (not binary.is_file() or binary.is_symlink() or not receipt.is_file() or receipt.is_symlink()
                or binary.stat().st_uid != os.getuid() or receipt.stat().st_uid != os.getuid()
                or binary.stat().st_nlink != 1 or receipt.stat().st_nlink != 1
                or binary.stat().st_mode & 0o077 or receipt.stat().st_mode & 0o077
                or not 0 < binary.stat().st_size <= bootstrap.MAX_BINARY_BYTES
                or receipt.stat().st_size > 16_384):
            return []
        metadata = receipt.read_bytes()
        if not bootstrap._verified_cache(binary, receipt, archive) or receipt.read_bytes() != metadata:
            return []
        if "source" in json.loads(metadata):
            return []
        entries.append((name, binary, metadata, json.loads(metadata)["binary_sha256"]))
    directory = Path(destination) / ".bin"
    if directory.is_symlink() or directory.exists() and (not directory.is_dir() or directory.stat().st_uid != os.getuid()):
        raise ValueError("The deployment runtime cache must be an owned ordinary folder.")
    if directory.exists() and any(directory.iterdir()):
        return []
    directory.mkdir(mode=0o700, exist_ok=True)
    temporary = []
    pending = []
    try:
        for name, binary, metadata, expected in entries:
            with tempfile.NamedTemporaryFile(prefix=".launch-release-", dir=directory, delete=False) as handle:
                cached = Path(handle.name)
                temporary.append(cached)
                os.fchmod(handle.fileno(), 0o700)
                digest, size = hashlib.sha256(), 0
                with binary.open("rb") as stream:
                    while chunk := stream.read(1024 * 1024):
                        size += len(chunk)
                        if size > bootstrap.MAX_BINARY_BYTES:
                            raise ValueError("The verified host runtime changed while being copied.")
                        digest.update(chunk)
                        handle.write(chunk)
                handle.flush()
                os.fsync(handle.fileno())
            if not size or digest.hexdigest() != expected:
                raise ValueError("The verified host runtime changed while being copied.")
            pending.append((cached, directory / name))
            with tempfile.NamedTemporaryFile(prefix=".launch-receipt-", dir=directory, delete=False) as handle:
                cached_receipt = Path(handle.name)
                temporary.append(cached_receipt)
                os.fchmod(handle.fileno(), 0o600)
                handle.write(metadata)
                handle.flush()
                os.fsync(handle.fileno())
            pending.append((cached_receipt, directory / (name + ".receipt.json")))
        # All copied bytes passed verification before any executable is installed.
        # Hard-link creation refuses a concurrently created destination file.
        for cached, installed in pending:
            os.link(cached, installed)
        return [entry[0] for entry in entries]
    finally:
        for path in temporary:
            path.unlink(missing_ok=True)
