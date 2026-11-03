"""Read-only retained archive research using the pinned installer's parsers.

This private worker accepts only controller-authored stdin. It never imports
archive members, extracts files, resolves dependencies or chooses entrypoints.
"""
from __future__ import annotations

import hashlib
import json
import os
from pathlib import Path
import stat
import sys
import unicodedata


PAGE = 64
CHUNK = 32 * 1024
TEXT_LIMIT = 1024 * 1024
OUTPUT_LIMIT = 256 * 1024
SCOPE = "acquired_primary_archive_only"
DEPENDENCY_SCOPE = "acquired_dependency_archive_only"


def sha(raw):
    return hashlib.sha256(raw).hexdigest()


def read_tool(path, expected):
    """Bootstrap the already pinned owner without following path aliases."""
    path = Path(path)
    if not path.is_absolute() or ".." in path.parts:
        raise ValueError()
    descriptor = os.open(path.anchor, os.O_RDONLY | os.O_DIRECTORY)
    try:
        for part in path.parts[1:-1]:
            child = os.open(part, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW, dir_fd=descriptor)
            os.close(descriptor)
            descriptor = child
        fd = os.open(path.name, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK, dir_fd=descriptor)
        with os.fdopen(fd, "rb") as stream:
            info = os.fstat(stream.fileno())
            if not stat.S_ISREG(info.st_mode) or info.st_size > 64 * 1024 * 1024:
                raise ValueError()
            raw = stream.read(64 * 1024 * 1024 + 1)
        if len(raw) > 64 * 1024 * 1024 or sha(raw) != expected:
            raise ValueError()
        return raw
    finally:
        os.close(descriptor)


def inspect(request):
    role = request.get("archive_role", "primary")
    if role not in ("primary", "dependency"):
        raise ValueError()
    owner_raw = read_tool(request["installer"], request["installer_sha256"])
    owner = {"__name__": "toolkit_pinned_archive_inspector"}
    exec(compile(owner_raw, request["installer"], "exec"), owner)
    path = Path(request["artifact"])
    raw = owner["read"](path.parent, path.name)
    if sha(raw) != request["archive_sha256"]:
        raise ValueError()
    profile = request["profile"]
    if profile == "python":
        parsed = owner["selected_wheel"](request["filename"], raw, set(sys.stdlib_module_names))
    elif profile == "node":
        parsed = owner["npm_archive"](raw)
    else:
        raise ValueError()
    files = parsed["files"]
    result = {"schema_version": 1, "scope": SCOPE if role == "primary" else DEPENDENCY_SCOPE, "profile": profile,
              "filename": request["filename"], "archive_sha256": sha(raw)}
    selection = request["selection"]
    if set(selection) == {"after"}:
        after = selection["after"]
        names = sorted(files)
        if after is not None and after not in files:
            raise ValueError()
        start = 0 if after is None else names.index(after) + 1
        page = names[start:start + PAGE]
        result.update(members=[{"path": name, "bytes": len(files[name]), "sha256": sha(files[name])} for name in page],
                      next_after=page[-1] if start + len(page) < len(names) else None)
    elif set(selection) == {"member", "offset"}:
        member, offset = selection["member"], selection["offset"]
        if member not in files or type(offset) is not int or not 0 <= offset <= len(files[member]):
            raise ValueError()
        data = files[member]
        if len(data) > TEXT_LIMIT:
            return {"error": "acquired_member_unreadable"}
        try:
            text = data.decode("utf-8")
            if any(unicodedata.category(c) == "Cc" and c not in "\t\r\n" for c in text):
                raise UnicodeError()
            data[:offset].decode("utf-8")
        except UnicodeError:
            return {"error": "acquired_member_unreadable"}
        end = min(len(data), offset + CHUNK)
        # A UTF-8 character can cross the chunk edge by at most three bytes.
        while True:
            try:
                text = data[offset:end].decode("utf-8")
                break
            except UnicodeDecodeError:
                end -= 1
        result.update(member=member, member_bytes=len(data), member_sha256=sha(data),
                      offset=offset, text=text, next_offset=end if end < len(data) else None)
    else:
        raise ValueError()
    if owner["read"](path.parent, path.name) != raw or read_tool(request["installer"], request["installer_sha256"]) != owner_raw:
        raise ValueError()
    return result


def main():
    try:
        raw = sys.stdin.buffer.read(65537)
        if len(raw) > 65536:
            raise ValueError()
        result = inspect(json.loads(raw))
        encoded = json.dumps(result, ensure_ascii=True, separators=(",", ":")).encode() + b"\n"
        if len(encoded) > OUTPUT_LIMIT:
            raise ValueError()
        sys.stdout.buffer.write(encoded)
        return 2 if "error" in result else 0
    except Exception:
        sys.stdout.buffer.write(b'{"error":"acquired_inspection_unavailable"}\n')
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
