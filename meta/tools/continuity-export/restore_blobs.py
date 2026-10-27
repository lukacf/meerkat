#!/usr/bin/env python3
"""Write the images a continuity mirror copied into a state directory's blob
store, so that the sessions a restore imports find their images.

A Meta 0.9.4 session holds each image as a blob reference; the image bytes
live in the state directory's blob store (``<state_dir>/blobs``). A continuity
mirror copies those bytes beside its records and sessions, one JSON Lines row
per blob in ``blobs.jsonl``::

    {"blob_id": "sha256:<64 hex>", "media_type": "image/png", "data": "<base64>"}

Restore runs this script after ``meta_gateway storage-migrate-native --apply``
has imported the sessions into the new state directory, and before the gateway
starts there:

    python3 restore_blobs.py --blobs MIRROR_DIR/blobs.jsonl --state-dir NEW_STATE_DIR

It writes each blob in the layout the gateway's blob store reads:
``blobs/objects/<hex>.bin`` holds the decoded bytes and ``blobs/meta/<hex>.json``
the media type and size. Every write goes through a temporary file, a flush to
disk and a rename, and the metadata file is written last, so a blob is visible
only once it is whole.

A row is written only after its id is verified against its own bytes, under
either address a Meta store issues: Meerkat's image address,
``sha256(media_type || 0x00 || base64 text)``, or Meta's raw address,
``sha256(media_type || 0x00 || bytes)``. A line that is not such a row, an id
that matches neither address, invalid base64, or a blob already in the store
with different bytes stops the restore with exit status 2, naming the file and
line; blobs written before it stay (they are content addressed). Running it
again writes nothing it already wrote.

The mirror appends one row at a time, so a copy taken while it was writing can
end in a row it never finished: a last line with no newline that is not JSON.
That line is skipped and reported (``torn_tail``); the mirror writes a blob's
row before any session that references it, so no copied session needs it.

The script needs Python 3.10 or later and the standard library only. It
prints a JSON summary: rows read, blobs written, blobs already present, and
whether it skipped a torn last line.
"""

from __future__ import annotations

import argparse
import base64
import binascii
import hashlib
import json
import os
import sys
import time
from pathlib import Path

_HEX = frozenset("0123456789abcdef")


class RestoreError(Exception):
    """A row this script refuses to write."""


def _blob_key(blob_id: str) -> str:
    if not isinstance(blob_id, str) or not blob_id.startswith("sha256:"):
        raise RestoreError(f"blob_id {blob_id!r} is not sha256:<64 hex>")
    key = blob_id[len("sha256:") :]
    if len(key) != 64 or not set(key) <= _HEX:
        raise RestoreError(f"blob_id {blob_id!r} is not sha256:<64 hex>")
    return key


def _address(media_type: str, payload: bytes) -> str:
    digest = hashlib.sha256()
    digest.update(media_type.encode("utf-8"))
    digest.update(b"\x00")
    digest.update(payload)
    return "sha256:" + digest.hexdigest()


def _verified_row(row: object) -> tuple[str, str, bytes]:
    """The row's key, media type and decoded bytes, once its id verifies."""
    if not isinstance(row, dict) or set(row) != {"blob_id", "media_type", "data"}:
        raise RestoreError("a row must hold exactly blob_id, media_type and data")
    blob_id, media_type, data = row["blob_id"], row["media_type"], row["data"]
    key = _blob_key(blob_id)
    if not isinstance(media_type, str) or not media_type:
        raise RestoreError("media_type must be a non-empty string")
    if not isinstance(data, str):
        raise RestoreError("data must be a base64 string")
    try:
        payload = base64.b64decode(data.encode("ascii"), validate=True)
    except (binascii.Error, UnicodeEncodeError) as error:
        raise RestoreError(f"data is not standard base64: {error}") from None
    if blob_id not in (
        _address(media_type, data.encode("ascii")),
        _address(media_type, payload),
    ):
        raise RestoreError(f"blob_id {blob_id} does not match its media type and bytes")
    return key, media_type, payload


def _write_atomically(path: Path, content: bytes) -> None:
    partial = path.with_name(f".{path.name}.partial")
    with open(partial, "wb") as handle:
        handle.write(content)
        handle.flush()
        os.fsync(handle.fileno())
    os.replace(partial, path)
    directory = os.open(path.parent, os.O_RDONLY)
    try:
        os.fsync(directory)
    finally:
        os.close(directory)


def _write_blob(blobs: Path, key: str, media_type: str, payload: bytes) -> bool:
    """Write one verified blob; False when the store already holds it."""
    objects = blobs / "objects"
    metas = blobs / "meta"
    objects.mkdir(parents=True, exist_ok=True)
    metas.mkdir(parents=True, exist_ok=True)
    object_path = objects / f"{key}.bin"
    meta_path = metas / f"{key}.json"
    if object_path.exists() and meta_path.exists():
        if object_path.read_bytes() != payload:
            raise RestoreError(f"the store already holds blob {key} with different bytes")
        return False
    _write_atomically(object_path, payload)
    meta = {
        "media_type": media_type,
        "size": len(payload),
        "created_at_ms": int(time.time() * 1000),
        "source": "meta",
    }
    _write_atomically(meta_path, json.dumps(meta, separators=(",", ":")).encode("utf-8"))
    return True


def restore_blobs(blobs_file: Path, state_dir: Path) -> dict[str, int | bool]:
    """Write every row of ``blobs_file`` into ``state_dir``'s blob store."""
    blobs = state_dir / "blobs"
    rows = written = present = 0
    torn_tail = False
    seen: dict[str, bytes] = {}
    with open(blobs_file, "r", encoding="utf-8", newline="") as handle:
        for number, line in enumerate(handle, start=1):
            text = line.rstrip("\r\n")
            if not text.strip():
                continue
            try:
                try:
                    row = json.loads(text)
                except json.JSONDecodeError:
                    # Only the last line can lack its newline. A row is a JSON
                    # object, so no part of one cut short is JSON.
                    if not line.endswith("\n"):
                        torn_tail = True
                        break
                    raise
                key, media_type, payload = _verified_row(row)
                if key in seen:
                    if seen[key] != payload:
                        raise RestoreError(f"blob {key} appears twice with different bytes")
                    continue
                seen[key] = payload
                if _write_blob(blobs, key, media_type, payload):
                    written += 1
                else:
                    present += 1
            except (RestoreError, json.JSONDecodeError) as error:
                raise RestoreError(f"{blobs_file}:{number}: {error}") from None
            rows += 1
    return {"rows": rows, "written": written, "present": present, "torn_tail": torn_tail}


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--blobs", required=True, type=Path, help="the mirror's blobs.jsonl")
    parser.add_argument(
        "--state-dir", required=True, type=Path, help="the restored Meta state directory"
    )
    args = parser.parse_args(argv)
    if not args.state_dir.is_dir():
        print(f"state directory {args.state_dir} does not exist", file=sys.stderr)
        return 2
    try:
        summary = restore_blobs(args.blobs, args.state_dir)
    except RestoreError as error:
        print(f"restore refused: {error}", file=sys.stderr)
        return 2
    except OSError as error:
        print(f"restore failed: {error}", file=sys.stderr)
        return 1
    print(json.dumps(summary, sort_keys=True))
    return 0


if __name__ == "__main__":
    sys.exit(main())
