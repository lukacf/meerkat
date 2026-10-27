#!/usr/bin/env python3
"""Export the selected setup starter into a fresh, self-contained directory."""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import stat


ROOT = Path(__file__).resolve().parents[1]
SOURCE_FILES = ("start.py", "bootstrap_host.py", "prepare_host.py")
DIR_FLAGS = os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW
INPUT_LIMIT = 2 * 1024 * 1024
README = """# Toolkit setup starter

Obtain this complete starter from a trusted maintainer, together with the current
Mac arm64 or Linux x86_64 library archive named in release-manifest.json. Put that
archive in a local downloads directory. This export does not publish or download
the release, and its copied manifest is a projection of the maintainer's current
distribution selection.

From this directory, use a trusted native CPython 3.11 outside a virtualenv:

```sh
python3.11 start.py --archive-dir /ABS/downloads --destination /ABS/fresh-toolkit
```

The destination must be fresh and its parent must exist. The invoking Python is
the selected runtime; compatible platform runtimes are still required. start.py
verifies the selected archive's size and hash before using its contents. See
`python3.11 start.py --help` for explicit optional runtimes and acquisition.

This prepares the local setup client and configuration. Register the reported
mcp.json through your existing MCP host, then ask its agent for the original
task. Preparation does not start an agent or authenticate an account. If it
fails, preserve the partial destination and use a fresh path for another attempt.
"""


class ExportError(ValueError):
    pass


def read_regular(path):
    """Read a bounded file without following source file or directory symlinks."""
    directory = os.open(path.anchor, DIR_FLAGS)
    try:
        for part in path.parts[1:-1]:
            child = os.open(part, DIR_FLAGS, dir_fd=directory)
            os.close(directory)
            directory = child
        flags = os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK
        with os.fdopen(os.open(path.name, flags, dir_fd=directory), "rb") as stream:
            if not stat.S_ISREG(os.fstat(stream.fileno()).st_mode):
                raise ExportError("source input must be a regular file")
            raw = stream.read(INPUT_LIMIT + 1)
            if len(raw) > INPUT_LIMIT:
                raise ExportError("source input exceeds the starter limit")
            return raw
    finally:
        os.close(directory)


def write_new(directory, name, raw):
    flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW
    with os.fdopen(os.open(name, flags, 0o600, dir_fd=directory), "wb") as stream:
        stream.write(raw)
        stream.flush()
        os.fsync(stream.fileno())


def export(destination, *, manifest=None, manifest_sha256=None):
    destination = Path(destination)
    if not destination.is_absolute():
        raise ExportError("destination must be an absolute fresh directory")
    source = ROOT / "sdk/consumer-setup"
    if manifest is None and manifest_sha256 is None:
        pointer = json.loads(read_regular(source / "current-distribution.json"))
        if (not isinstance(pointer, dict)
                or set(pointer) != {"schema_version", "manifest", "manifest_sha256"}
                or type(pointer["schema_version"]) is not int or pointer["schema_version"] != 1
                or not isinstance(pointer["manifest"], str) or not pointer["manifest"]
                or Path(pointer["manifest"]).is_absolute()
                or not isinstance(pointer["manifest_sha256"], str)
                or re.fullmatch(r"[0-9a-f]{64}", pointer["manifest_sha256"]) is None):
            raise ExportError("invalid current distribution selector")
        manifest_path = source / pointer["manifest"]
    else:
        if manifest is None or manifest_sha256 is None:
            raise ExportError("explicit manifest and digest are required together")
        manifest_path = Path(manifest)
        if (not manifest_path.is_absolute() or not isinstance(manifest_sha256, str)
                or re.fullmatch(r"[0-9a-f]{64}", manifest_sha256) is None):
            raise ExportError("invalid explicit manifest selection")
        # A private composition selects an existing-shape manifest without
        # retargeting the ordinary distribution or inspecting archive directories.
        pointer = {"schema_version": 1, "manifest": "release-manifest.json",
                   "manifest_sha256": manifest_sha256}
    manifest = read_regular(manifest_path)
    if hashlib.sha256(manifest).hexdigest() != pointer["manifest_sha256"]:
        raise ExportError("selected release manifest hash mismatch")
    payloads = {name: read_regular(source / name) for name in SOURCE_FILES}
    payloads["release-manifest.json"] = manifest
    exported = dict(pointer, manifest="release-manifest.json")
    payloads["current-distribution.json"] = (json.dumps(exported, indent=2) + "\n").encode()
    payloads["README.md"] = README.encode()
    # Validate every input before creating output. Never remove partial exports.
    destination.mkdir(mode=0o700)
    directory = os.open(destination, DIR_FLAGS)
    try:
        for name, raw in payloads.items():
            write_new(directory, name, raw)
    finally:
        os.close(directory)
    return {"status": "exported", "directory": str(destination),
            "manifest_sha256": pointer["manifest_sha256"], "files": sorted(payloads)}


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__, allow_abbrev=False)
    parser.add_argument("--destination", required=True, help="fresh absolute directory; parent must exist")
    parser.add_argument("--manifest", help="explicit absolute release manifest; requires --manifest-sha256")
    parser.add_argument("--manifest-sha256", help="exact digest of the explicitly selected manifest")
    args = parser.parse_args(argv)
    try:
        result = export(args.destination, manifest=args.manifest, manifest_sha256=args.manifest_sha256)
    except FileExistsError:
        parser.exit(1, "Export refused: destination is occupied. Preserve it and use a fresh path.\n")
    except ExportError as error:
        parser.exit(1, "Export refused: " + str(error) + ". Preserve any partial output.\n")
    except (OSError, ValueError):
        parser.exit(1, "Export refused: input or output unavailable. Preserve any partial output.\n")
    print(json.dumps(result, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
