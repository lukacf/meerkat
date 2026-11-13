#!/usr/bin/env python3
"""Deep extraction for release-payload-credential-scan.py (a library; it has no command line).

expand_file(PAYLOAD, NEW_TREE) writes a "deep tree": a copy of the payload file and, for every
archive found by its magic bytes, the members in a sibling directory named
"<archive name>.extracted/", again and again up to MAX_LEVEL levels. Formats: zip (also a zip
appended to another file), tar, gzip, bzip2, xz and zstd, each as a tar or as a single stream.
zstd needs Python 3.14 or a zstd executable.

The input is untrusted data and nothing from it is executed. A member with an absolute path, a
".." component, a link or a special type is not written; it is counted per archive under a reason
("skips"). An archive that cannot be read, is read only in part, has a format this file does not
open, or sits deeper than MAX_LEVEL is listed ("not_opened", "partial"): the caller decides, and the
credential scan refuses. Only names, counts and sizes leave this file, never content.
Standard library only, no network.
"""
import bz2
import collections
import contextlib
import gzip
import hashlib
import lzma
import os
import shutil
import stat
import subprocess
import tarfile
import zipfile

MAX_LEVEL = 32
MAX_MEMBER_BYTES = 4 << 30
MAX_TOTAL_BYTES = 60 << 30
# Refuse to go on when the file system of the tree has less free space than this, or (when set)
# is fuller than this percentage. The caller may change both before a run.
MIN_FREE_BYTES = 1 << 30
DISK_STOP_PERCENT = None
CHUNK = 1 << 20
ZSTD_TOOL = shutil.which("zstd", path="/usr/bin:/bin:/usr/local/bin")

UNSUPPORTED_MAGIC = [
    (0, b"7z\xbc\xaf\x27\x1c", "7z"),
    (0, b"Rar!\x1a\x07", "rar"),
    (0, b"!<arch>\n", "ar"),
    (0, b"070701", "cpio"),
    (0, b"070702", "cpio"),
    (0, b"070707", "cpio"),
    (0, b"\x04\x22\x4d\x18", "lz4"),
    (0, b"LZIP", "lzip"),
    (0, b"MSCF", "cab"),
    (0, b"\xed\xab\xee\xdb", "rpm"),
    (0, b"hsqs", "squashfs"),
    (0, b"\x1f\x9d", "compress-Z"),
    (0, b"\x1f\xa0", "compress-lzh"),
    (0, b"xar!", "xar"),
]


class ExtractError(Exception):
    """The run must stop: a disk guard, the total size cap or an unusable output location."""


class Stats:
    def __init__(self, root):
        self.root = root
        self.opened = collections.Counter()
        self.member_skips = collections.Counter()
        self.not_opened = []
        self.partial = []
        self.archives = []
        self.total_bytes = 0


ST = None


def disk_guard():
    usage = shutil.disk_usage(ST.root)
    if usage.free < MIN_FREE_BYTES:
        raise ExtractError("less free space than the minimum")
    if DISK_STOP_PERCENT is not None and 100.0 * usage.used / usage.total >= DISK_STOP_PERCENT:
        raise ExtractError("disk use at or over the stop percentage")


def sniff(path):
    """Return (kind, label). kind in zip|gzip|bz2|xz|zstd|tar|unsupported|None."""
    try:
        with open(path, "rb") as f:
            head = f.read(600)
    except OSError:
        return (None, None)
    if head[:4] in (b"PK\x03\x04", b"PK\x05\x06", b"PK\x07\x08"):
        return ("zip", "zip")
    if head[:2] == b"\x1f\x8b":
        return ("gzip", "gzip")
    if head[:3] == b"BZh" and len(head) >= 10 and head[3:4] in b"123456789" and head[4:10] in (b"1AY&SY", b"\x17rE8P\x90"):
        return ("bz2", "bz2")
    if head[:6] == b"\xfd7zXZ\x00":
        return ("xz", "xz")
    if head[:4] == b"\x28\xb5\x2f\xfd":
        return ("zstd", "zstd")
    if head[257:262] == b"ustar":
        return ("tar", "tar")
    for off, magic, label in UNSUPPORTED_MAGIC:
        if head[off:off + len(magic)] == magic:
            return ("unsupported", label)
    try:
        if os.path.getsize(path) > 22 and zipfile.is_zipfile(path):
            return ("zip", "zip-embedded")
    except OSError:
        pass
    return (None, None)


def safe_parts(name):
    """Return (parts, None) or (None, reason)."""
    if not name:
        return (None, "empty-name")
    if "\x00" in name:
        return (None, "nul-in-name")
    if name.startswith("/") or name.startswith("\\"):
        return (None, "absolute-path")
    if len(name) >= 2 and name[1] == ":" and name[0].isalpha():
        return (None, "absolute-path")
    parts = []
    for comp in name.split("/"):
        if comp in ("", "."):
            continue
        if comp == "..":
            return (None, "dotdot")
        if ".." in comp.split("\\"):
            return (None, "dotdot")
        if len(comp.encode("utf-8", "surrogateescape")) > 255:
            return (None, "name-too-long")
        parts.append(comp)
    if not parts:
        return (None, "empty-name")
    return (parts, None)


def inside(base, target):
    base_r = os.path.realpath(base)
    targ_r = os.path.realpath(target)
    return targ_r == base_r or targ_r.startswith(base_r + os.sep)


def write_member(outdir, parts, src, fmt):
    """Write a regular member; returns bytes written or None when skipped."""
    parent = os.path.join(outdir, *parts[:-1]) if len(parts) > 1 else outdir
    try:
        os.makedirs(parent, mode=0o700, exist_ok=True)
    except (FileExistsError, NotADirectoryError):
        ST.member_skips[fmt + ":path-conflict"] += 1
        return None
    if not inside(outdir, parent):
        ST.member_skips[fmt + ":escapes-root"] += 1
        return None
    dest = os.path.join(parent, parts[-1])
    n = 0
    while os.path.lexists(dest):
        n += 1
        dest = os.path.join(parent, parts[-1] + ".dup%d" % n)
    if n:
        ST.member_skips[fmt + ":duplicate-name-renamed"] += 1
    written = 0
    fd = os.open(dest, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
    with os.fdopen(fd, "wb") as out:
        while True:
            buf = src.read(CHUNK)
            if not buf:
                break
            written += len(buf)
            ST.total_bytes += len(buf)
            if written > MAX_MEMBER_BYTES:
                raise RuntimeError("member exceeds size cap")
            if ST.total_bytes > MAX_TOTAL_BYTES:
                raise ExtractError("total extracted bytes exceed the cap")
            out.write(buf)
    return written


def make_dir_member(outdir, parts, fmt):
    target = os.path.join(outdir, *parts)
    try:
        os.makedirs(target, mode=0o700, exist_ok=True)
    except (FileExistsError, NotADirectoryError):
        ST.member_skips[fmt + ":path-conflict"] += 1


def extract_zip(path, outdir, fmt):
    members = 0
    nbytes = 0
    with zipfile.ZipFile(path) as zf:
        for info in zf.infolist():
            parts, reason = safe_parts(info.filename)
            if parts is None:
                ST.member_skips[fmt + ":" + reason] += 1
                continue
            mode = info.external_attr >> 16
            if info.is_dir():
                make_dir_member(outdir, parts, fmt)
                continue
            if mode and stat.S_ISLNK(mode):
                ST.member_skips[fmt + ":link"] += 1
                continue
            if mode and not (stat.S_ISREG(mode) or stat.S_IFMT(mode) == 0):
                ST.member_skips[fmt + ":special"] += 1
                continue
            if info.flag_bits & 0x1:
                ST.member_skips[fmt + ":encrypted"] += 1
                continue
            with zf.open(info) as src:
                w = write_member(outdir, parts, src, fmt)
            if w is not None:
                members += 1
                nbytes += w
    return members, nbytes


def extract_tar(path, outdir, mode, fmt):
    members = 0
    nbytes = 0
    with tarfile.open(path, mode) as tf:
        for m in tf:
            parts, reason = safe_parts(m.name)
            if parts is None:
                ST.member_skips[fmt + ":" + reason] += 1
                continue
            if m.isdir():
                make_dir_member(outdir, parts, fmt)
                continue
            if m.issym() or m.islnk():
                ST.member_skips[fmt + ":link"] += 1
                continue
            if not m.isreg():
                ST.member_skips[fmt + ":special"] += 1
                continue
            src = tf.extractfile(m)
            if src is None:
                ST.member_skips[fmt + ":unreadable"] += 1
                continue
            with src:
                w = write_member(outdir, parts, src, fmt)
            if w is not None:
                members += 1
                nbytes += w
    return members, nbytes


def single_name(path, suffixes):
    base = os.path.basename(path)
    low = base.lower()
    for s in suffixes:
        if low.endswith(s) and len(base) > len(s):
            return base[: -len(s)]
    return base + ".out"


def extract_single(path, outdir, opener, suffixes, fmt):
    os.makedirs(outdir, mode=0o700, exist_ok=True)
    with opener(path) as src:
        w = write_member(outdir, [single_name(path, suffixes)], src, fmt)
    return (1, w) if w is not None else (0, 0)


@contextlib.contextmanager
def zstd_stream(path):
    """The decompressed bytes of a zstd file: Python's own decoder (3.14 and later) or the tool."""
    try:
        from compression import zstd
    except ImportError:
        zstd = None
    if zstd is not None:
        with zstd.open(path, "rb") as src:
            yield src
        return
    if not ZSTD_TOOL:
        raise RuntimeError("no zstd decoder")
    proc = subprocess.Popen([ZSTD_TOOL, "-d", "-q", "-c", "--", path], stdout=subprocess.PIPE,
                            stderr=subprocess.DEVNULL, stdin=subprocess.DEVNULL, cwd="/")
    try:
        yield proc.stdout
    finally:
        proc.stdout.close()
        code = proc.wait()
    if code != 0:
        raise RuntimeError("the zstd decoder failed")


def new_outdir(path):
    outdir = path + ".extracted"
    n = 0
    while os.path.lexists(outdir):
        n += 1
        outdir = path + ".extracted%d" % n
    return outdir


def open_archive(path, level, rel):
    """Try to open one file as an archive. Returns outdir or None."""
    kind, label = sniff(path)
    if kind is None:
        return None
    if kind == "unsupported":
        ST.not_opened.append({"format": label, "path": rel, "reason": "format not supported", "level": level})
        return None
    if level >= MAX_LEVEL:
        ST.not_opened.append({"format": label, "path": rel, "reason": "depth limit %d reached" % MAX_LEVEL, "level": level})
        return None
    disk_guard()
    outdir = new_outdir(path)
    fmt = label
    members = nbytes = 0
    err = None
    before = collections.Counter(ST.member_skips)
    try:
        if kind == "zip":
            os.makedirs(outdir, mode=0o700)
            members, nbytes = extract_zip(path, outdir, fmt)
        elif kind == "tar":
            os.makedirs(outdir, mode=0o700)
            members, nbytes = extract_tar(path, outdir, "r:", fmt)
        elif kind in ("gzip", "bz2", "xz"):
            tmode = {"gzip": "r:gz", "bz2": "r:bz2", "xz": "r:xz"}[kind]
            is_tar = True
            try:
                with tarfile.open(path, tmode) as probe:
                    probe.next()
            except tarfile.ReadError:
                is_tar = False
            os.makedirs(outdir, mode=0o700)
            if is_tar:
                fmt = {"gzip": "tar.gz", "bz2": "tar.bz2", "xz": "tar.xz"}[kind]
                members, nbytes = extract_tar(path, outdir, tmode, fmt)
            else:
                fmt = kind + "-single"
                opener = {"gzip": gzip.open, "bz2": bz2.open, "xz": lzma.open}[kind]
                sfx = {"gzip": (".gz",), "bz2": (".bz2",), "xz": (".xz", ".lzma")}[kind]
                members, nbytes = extract_single(path, outdir, opener, sfx, fmt)
        elif kind == "zstd":
            fmt = "zstd-single"
            members, nbytes = extract_single(path, outdir, zstd_stream, (".zst", ".zstd"), fmt)
    except ExtractError:
        raise
    except Exception as exc:  # noqa: BLE001 - record and continue; the caller refuses
        err = type(exc).__name__
    if err and members == 0:
        if os.path.isdir(outdir) and not any(files for _d, _s, files in os.walk(outdir)):
            for d, _s, _f in os.walk(outdir, topdown=False):
                os.rmdir(d)
        ST.not_opened.append({"format": fmt, "path": rel, "reason": "could not read: " + err, "level": level})
        return None
    ST.opened[fmt] += 1
    skips = {k: v for k, v in (ST.member_skips - before).items() if v}
    rec = {"path": rel, "format": fmt, "level": level + 1, "members": members, "bytes": nbytes,
           "outdir": os.path.basename(outdir), "skips": skips}
    if err:
        rec["error"] = err
        ST.partial.append({"format": fmt, "path": rel, "reason": "error while reading: " + err, "level": level})
    ST.archives.append(rec)
    return outdir


def walk_files(top):
    out = []
    for dirpath, dirnames, filenames in os.walk(top, followlinks=False):
        dirnames.sort()
        for fn in sorted(filenames):
            p = os.path.join(dirpath, fn)
            if stat.S_ISREG(os.lstat(p).st_mode):
                out.append(p)
    return out


def expand_file(path, out_root):
    """Copy the regular file at path into the new directory out_root and open it, and what it
    holds, at every level. Returns the lists of opened and unopened archives (paths relative to
    out_root) and the sha256 and size of the bytes that were copied, which are the bytes every
    later step reads. Raises ExtractError when the run must stop."""
    global ST
    path, out_root = os.path.abspath(path), os.path.abspath(out_root)
    if os.path.islink(path) or not os.path.isfile(path):
        raise ExtractError("the payload is not a regular file")
    if os.path.lexists(out_root):
        raise ExtractError("the output tree exists")
    os.makedirs(out_root, mode=0o700)
    ST = Stats(out_root)
    disk_guard()
    dest = os.path.join(out_root, os.path.basename(path))
    digest, size = hashlib.sha256(), 0
    fd = os.open(dest, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
    with open(path, "rb") as src, os.fdopen(fd, "wb") as out:
        # The copy is what every later step reads: its digest is the digest of the scanned bytes.
        for block in iter(lambda: src.read(CHUNK), b""):
            digest.update(block)
            size += len(block)
            out.write(block)
    queue = collections.deque([(dest, 0)])
    while queue:
        member, level = queue.popleft()
        outdir = open_archive(member, level, os.path.relpath(member, out_root))
        if outdir and os.path.isdir(outdir):
            for p in walk_files(outdir):
                queue.append((p, level + 1))
    return {"archives": ST.archives, "not_opened": ST.not_opened, "partial": ST.partial,
            "opened_per_format": dict(ST.opened), "sha256": digest.hexdigest(), "bytes": size}
