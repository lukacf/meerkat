#!/usr/bin/env python3
"""Deep extraction for release-payload-credential-scan.py (a library; it has no command line).

expand_file(PAYLOAD, NEW_TREE) writes a "deep tree": a copy of the payload file and, for every
archive found by its head, the members in a sibling directory named "<archive name>.extracted/",
again and again up to MAX_LEVEL levels. Formats: zip (also a zip appended to another file), tar
(with the "ustar" mark, or of the old form: a first header whose check holds), gzip, bzip2, xz and
zstd (also one that starts with a skippable frame), each as a tar or as a single stream. zstd
needs Python 3.14 or a zstd executable; either gives the same result.

The input is untrusted data and nothing from it is executed. A member with an absolute path, a
".." component, a name that would print like another path (printed_parts), a tar link, a tar
member of another kind (a pipe, a device) and a zip directory entry that holds data is not written;
it is counted per archive under a reason ("skips"). A tar member that is no regular file, no link
and no directory and has a size holds data: it is written like a regular member when tarfile gives
its data (a type it does not know), and counted under a reason of its own when it gives none. A zip
entry with the mode of a link or of a special file is no member at its
place either; its data (a link's target) is read as text all the same: written as a regular file
link-NNNNN or special-NNNNN in OUTSIDE (below). No link is ever made, and nothing is followed. An
archive that cannot be read, is read only in part, has a
format this file does not open, or sits deeper than MAX_LEVEL is listed ("not_opened", "partial"):
the caller decides, and the credential scan refuses. Only names, counts and sizes leave this file,
never content; a reader's warning is an error of its archive, never a line on standard error.

A reader opens an archive that holds more than its members. Every byte of an opened archive that is
not the content of a member that was written out is written too, into the directory OUTSIDE inside
the archive's ".extracted" directory:
  before-start      the bytes before the first record of a zip, as they are
  after-end         the bytes after the end of a zip (its end record and comment) or a tar (the
                    block where its reader stopped), as they are, without their leading zero
                    bytes
  headers-and-gaps  of every other such byte (headers, names, comments, extra fields, padding, gaps
                    between members, a member that was not written out, the header and trailer of
                    a gzip member, a zstd skippable frame): the runs of 8 or more printable ASCII
                    bytes and tabs, one run on a line
A tar inside a compressed file names its own as tar-after-end and tar-headers-and-gaps. A range of
zero bytes alone and a range without such a run make no file. before-start and after-end are files
like any member: an archive there is opened. No member can stand in OUTSIDE: the name holds the
separator "!", and a member whose name holds it is not written out.

What counts as a member's content is what was decoded, and it is checked, not taken from the
archive's word. zip: of a stored member its data, whose length must be the size written; of a
deflated member its stream, which must end where the compressed size ends and decode to the size
written; of a member of another method nothing (its bytes are among the runs as well). Every
directory entry must name a place where a local header of the entry's own name stands, and the end
of the archive is found from its end records themselves. A zip that fails one of these is not
opened. A record that no directory entry names is among the runs, also when a whole archive stands
there. A compressed
file is its streams and zero padding: any other byte after the last stream makes it a file that
is not opened.
Standard library only, no network.
"""
import bz2
import collections
import hashlib
import lzma
import os
import re
import shutil
import stat
import subprocess
import tarfile
import warnings
import zipfile
import zlib

MAX_LEVEL = 32
MAX_MEMBER_BYTES = 4 << 30
MAX_TOTAL_BYTES = 60 << 30
# Refuse to go on when the file system of the tree has less free space than this, or (when set)
# is fuller than this percentage. The caller may change both before a run.
MIN_FREE_BYTES = 1 << 30
DISK_STOP_PERCENT = None
CHUNK = 1 << 20
ZSTD_TOOL = shutil.which("zstd", path="/usr/bin:/bin:/usr/local/bin")
# What joins an archive and a member in a printed path, and what follows the path of a compiled file
# for a position in its printable runs. A member's name gives neither (printed_parts).
SEPARATOR = "!"
STRINGS_MARK = "#strings"
# The directory, inside an opened archive's directory, of the archive's bytes that are no member's
# content. Its name holds the separator, so no member is written there.
OUTSIDE = SEPARATOR + "outside"
# A run is printable ASCII bytes and tabs (a tab is text to the builds, and must not cut a value from its name).
PRINTABLE = b"\t" + bytes(range(0x20, 0x7f))
RUNS = re.compile(rb"[\t\x20-\x7e]{8,}")
RUNS_HEAD = b"runs of 8 or more printable ASCII bytes among the bytes of an archive that are no member's content, one run on a line\n"
# A run longer than a block is written in pieces; two pieces share this many bytes.
RUN_OVERLAP = 4096
STREAM_MAGIC = {"gzip": b"\x1f\x8b", "bz2": b"BZh", "xz": b"\xfd7zXZ\x00", "zstd": b"\x28\xb5\x2f\xfd"}

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


def tar_head(head):
    """Whether the first bytes of a file are those of a tar: the "ustar" mark, or (a tar of the old
    form has no mark) a first header whose check holds."""
    if head[257:262] == b"ustar":
        return True
    if len(head) < 512:
        return False
    try:
        tarfile.TarInfo.frombuf(head[:512], "utf-8", "surrogateescape")
        return True
    except tarfile.HeaderError:
        return False


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
    # A zstd file starts with a frame or with a skippable frame (which a decoder passes over).
    if head[:4] == b"\x28\xb5\x2f\xfd" or (len(head) >= 4 and head[1:4] == b"\x2a\x4d\x18" and 0x50 <= head[0] <= 0x5f):
        return ("zstd", "zstd")
    if tar_head(head):
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


def printed_parts(parts, directory):
    """The one place that decides whether a member's name can be printed as a path of its own:
    (parts, None), or (None, reason) for a name that would print like another path. The separator
    joins an archive and a member, so a name that holds it prints like a member of an archive; the
    strings mark after a file's name prints like the runs of a compiled file of the name before it.
    Such a member is not written out."""
    if any(SEPARATOR in part for part in parts):
        return (None, "separator-in-name")
    if not directory and parts[-1].endswith(STRINGS_MARK):
        return (None, "strings-mark-in-name")
    return (parts, None)


def safe_parts(name, directory=False):
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
    return printed_parts(parts, directory)


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


def count(nbytes):
    ST.total_bytes += nbytes
    if ST.total_bytes > MAX_TOTAL_BYTES:
        raise ExtractError("total extracted bytes exceed the cap")


def fresh(path):
    """path, or path with a number, whichever does not exist yet."""
    name, n = path, 0
    while os.path.lexists(name):
        n += 1
        name = path + "%d" % n
    return name


def first_nonzero(src, at, end):
    """The offset of the first byte in [at, end) of the open file that is not zero, or end."""
    src.seek(at)
    while at < end:
        block = src.read(min(CHUNK, end - at))
        if not block:
            raise ValueError("the file is shorter than its layout")
        rest = block.lstrip(b"\x00")
        if rest:
            return at + len(block) - len(rest)
        at += len(block)
    return end


def gaps(claimed, start, end):
    """The ranges of [start, end) that no claimed range covers, in order."""
    at = start
    for a, b in sorted(claimed):
        if at < min(a, end):
            yield at, min(a, end)
        at = max(at, b)
    if at < end:
        yield at, end


def runs_of(src, ranges):
    """Every run of 8 or more printable ASCII bytes in the ranges of the open file. No run joins two
    ranges. A run longer than a block comes in pieces that share RUN_OVERLAP bytes, so that nothing
    shorter is cut in two; a piece always holds bytes the piece before it did not."""
    for at, end in ranges:
        src.seek(at)
        held, told = b"", 0     # told: how many bytes at the start of held an earlier piece gave already
        while at < end:
            block = src.read(min(CHUNK, end - at))
            if not block:
                raise ValueError("the file is shorter than its layout")
            at += len(block)
            data = held + block
            body = data.rstrip(PRINTABLE)
            if body:
                found = RUNS.findall(body)
                if told >= 8 and told == len(held) and block[:1] not in PRINTABLE:
                    found = found[1:]   # the long run ended with the block before: its last bytes were given
                yield from found
                told = 0
            held = data[len(body):]
            if len(held) > CHUNK:
                yield held
                held = held[-RUN_OVERLAP:]
                told = len(held)
        if len(held) >= 8 and len(held) > told:
            yield held


def write_outside(outdir, source, claimed, start, end, label=""):
    """Write what the opened archive file source holds beside the content of its written members,
    into OUTSIDE below outdir (see the head of this file). claimed are the ranges of source that
    were decoded into written members; [start, end) is the archive as its reader read it."""
    size = os.path.getsize(source)
    if not 0 <= start <= end <= size or any(not start <= a <= b <= end for a, b in claimed):
        raise ValueError("the layout of the archive is not known")
    target = os.path.join(outdir, OUTSIDE)

    def new(name):
        os.makedirs(target, mode=0o700, exist_ok=True)
        return os.fdopen(os.open(os.path.join(target, label + name), os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600), "wb")

    with open(source, "rb") as src:
        for name, a, b in (("before-start", 0, start), ("after-end", end, size)):
            a = first_nonzero(src, a, b)
            if a == b:
                continue
            src.seek(a)
            with new(name) as out:
                while a < b:
                    block = src.read(min(CHUNK, b - a))
                    if not block:
                        raise ValueError("the file is shorter than its layout")
                    a += len(block)
                    count(len(block))
                    out.write(block)
        out = None
        try:
            for run in runs_of(src, gaps(claimed, start, end)):
                if out is None:
                    out = new("headers-and-gaps")
                    out.write(RUNS_HEAD)
                count(len(run) + 1)
                out.write(run + b"\n")
        finally:
            if out is not None:
                out.close()


def zip_decoded(raw, info, written):
    """The range of the open zip file that zipfile decoded into the written bytes of the member
    info, or None for a method other than stored and deflated (this file cannot say which bytes
    such a method decodes: nothing is claimed, and the member's bytes in the archive are read with
    the archive's other bytes). zipfile gives what the sizes of the directory entry say and finds
    no fault when the data holds more or the stream says otherwise. Here that raises: data that
    is longer than a stored member's size, and a deflate stream that does not end where the
    compressed size ends or does not decode to exactly the written bytes."""
    raw.seek(info.header_offset + 26)
    lengths = raw.read(4)
    data = info.header_offset + 30 + int.from_bytes(lengths[:2], "little") + int.from_bytes(lengths[2:], "little")
    if info.compress_type == zipfile.ZIP_STORED:
        if info.compress_size != written:
            raise ValueError("a stored zip member whose size is not its data's")
        return data, data + written
    if info.compress_type != zipfile.ZIP_DEFLATED:
        return None
    decoded = [0]

    def counted(block):
        decoded[0] += len(block)
        if decoded[0] > written:
            raise ValueError("a deflated zip member whose stream decodes to more than its size")

    raw.seek(data)
    stop = through(zlib.decompressobj(-15), raw, counted)
    if stop != data + info.compress_size or decoded[0] != written:
        raise ValueError("a deflated zip member whose sizes are not its stream's")
    return data, stop


def zip_end(raw, at, comment):
    """The offset after a zip's end record and its comment. at is the place zipfile reports for the
    end record. For a zip64 archive that is the place of the zip64 end record: from there the
    record's own size leads to its locator and the locator to the end record. A record that is
    not where it must be raises."""
    def signature(magic):
        raw.seek(at)
        if raw.read(4) != magic:
            raise ValueError("an end record of a zip that is not where it must be")

    raw.seek(at)
    if raw.read(4) == b"PK\x06\x06":
        at += 12 + int.from_bytes(raw.read(8), "little")
        signature(b"PK\x06\x07")
        at += 20
    signature(b"PK\x05\x06")
    return at + zipfile.sizeEndCentDir + len(comment)


def extract_zip(path, outdir, fmt):
    members = 0
    nbytes = 0
    claimed = []
    numbers = collections.Counter()
    with zipfile.ZipFile(path) as zf, open(path, "rb") as raw:
        infos = zf.infolist()
        # The archive starts at the lowest place a directory entry names. zipfile looks at that place
        # only for a member it reads, so every entry is asked here: a local header must stand there,
        # and it must carry the entry's own name, byte for byte (the name of the directory entry as
        # zipfile decoded it, in the encoding its flag states).
        for info in infos:
            raw.seek(info.header_offset)
            head = raw.read(30)
            if len(head) < 30 or head[:4] != b"PK\x03\x04":
                raise ValueError("a zip directory entry that names no record")
            own = info.orig_filename.encode("utf-8" if info.flag_bits & 0x800 else "cp437")
            if raw.read(int.from_bytes(head[26:28], "little")) != own:
                raise ValueError("a zip directory entry at a record of another name")
        for info in infos:
            parts, reason = safe_parts(info.filename, info.is_dir())
            if parts is None:
                ST.member_skips[fmt + ":" + reason] += 1
                continue
            # Only the entry of a directory has no data; one that has is not written out.
            if info.is_dir():
                if info.compress_size or info.file_size:
                    ST.member_skips[fmt + ":directory-with-data"] += 1
                    continue
                make_dir_member(outdir, parts, fmt)
                continue
            if info.flag_bits & 0x1:
                ST.member_skips[fmt + ":encrypted"] += 1
                continue
            mode = info.external_attr >> 16
            kind = "link" if mode and stat.S_ISLNK(mode) else \
                "special" if mode and not (stat.S_ISREG(mode) or stat.S_IFMT(mode) == 0) else None
            with zf.open(info) as src:
                if kind is None:
                    w = write_member(outdir, parts, src, fmt)
                else:
                    # An entry with the mode of a link or of a special file is no member at its place,
                    # and counted so. Its data (a link's target) is content all the same: it is read as
                    # text, written as a regular file among the bytes outside members under a name
                    # that is a number. No link is made, no name of the entry is used, nothing is followed.
                    ST.member_skips[fmt + ":" + kind] += 1
                    w = write_member(os.path.join(outdir, OUTSIDE), ["%s-%05d" % (kind, numbers[kind])], src, fmt)
                    numbers[kind] += 1
            if w is not None:
                members += kind is None
                nbytes += w
                decoded = zip_decoded(raw, info, w)
                if decoded is not None:
                    claimed.append(decoded)
        # The archive, as its reader found it: from its first record to the end of its end record's comment.
        record = zipfile._EndRecData(raw)
        end = zip_end(raw, record[zipfile._ECD_LOCATION], record[zipfile._ECD_COMMENT])
        write_outside(outdir, path, claimed, min([info.header_offset for info in infos] + [zf.start_dir]), end)
    return members, nbytes


def extract_tar(path, outdir, fmt, label=""):
    members = 0
    nbytes = 0
    claimed = []
    with tarfile.open(path, "r:") as tf:
        for m in tf:
            parts, reason = safe_parts(m.name, m.isdir())
            if parts is None:
                ST.member_skips[fmt + ":" + reason] += 1
                continue
            if m.isdir():
                make_dir_member(outdir, parts, fmt)
                continue
            if m.issym() or m.islnk():
                ST.member_skips[fmt + ":link"] += 1
                continue
            # A member that is no regular file, no link and no directory holds nothing when it has no
            # size. With a size it holds data: tarfile gives the data of a type it does not know, and
            # that is read like a regular member's; it gives none for a pipe or a device, and such a
            # member is counted under a reason of its own, one that holds content.
            if not m.isreg() and not m.size:
                ST.member_skips[fmt + ":special"] += 1
                continue
            src = tf.extractfile(m)
            if src is None:
                ST.member_skips[fmt + (":unreadable" if m.isreg() else ":special-with-data")] += 1
                continue
            with src:
                w = write_member(outdir, parts, src, fmt)
            if w is not None:
                members += 1
                nbytes += w
                if m.sparse is None:
                    claimed.append((m.offset_data, m.offset_data + m.size))
        end = tf.offset     # where the reader stopped: the end blocks, a block that is no header, or the end of the file
    write_outside(outdir, path, claimed, 0, min(end, os.path.getsize(path)), label)
    return members, nbytes


def single_name(path, suffixes):
    base = os.path.basename(path)
    low = base.lower()
    for s in suffixes:
        if low.endswith(s) and len(base) > len(s):
            return base[: -len(s)]
    return base + ".out"


def through(decoder, src, sink):
    """Feed the open file, from where it stands, to the decoder until the decoder's stream ends.
    Returns the offset of the first byte after the stream."""
    tail = b""
    while not decoder.eof:
        block = tail or (src.read(CHUNK) if getattr(decoder, "needs_input", True) else b"")
        data = decoder.decompress(block, CHUNK)
        if not data and not block and not decoder.eof:
            raise EOFError("the stream ends before its end marker")
        sink(data)
        tail = getattr(decoder, "unconsumed_tail", b"")
    return src.tell() - len(decoder.unused_data)


def gzip_body(src, at):
    """The offset of the compressed data of the gzip member at offset at: after its header and the
    header's extra field, name, comment and check (RFC 1952)."""
    src.seek(at)
    head = src.read(10)
    if len(head) != 10 or head[:3] != b"\x1f\x8b\x08" or head[3] & 0xe0:
        raise ValueError("no gzip member")
    at += 10
    if head[3] & 4:
        extra = src.read(2)
        if len(extra) != 2:
            raise ValueError("a gzip header that ends early")
        at += 2 + int.from_bytes(extra, "little")
    for flag in (8, 16):
        while head[3] & flag:
            src.seek(at)
            block = src.read(4096)
            if not block:
                raise ValueError("a gzip header that ends early")
            cut = block.find(b"\x00")
            at += len(block) if cut < 0 else cut + 1
            if cut >= 0:
                break
    return at + (2 if head[3] & 2 else 0)


def zstd_frame_end(src, at, size):
    """The offset after the zstd frame at offset at, found from the frame's own headers without a
    decoder (RFC 8878): the frame header, every block header, and the content check."""
    src.seek(at)
    head = src.read(5)
    if len(head) != 5 or head[:4] != STREAM_MAGIC["zstd"] or head[4] & 0x08:
        raise ValueError("no zstd frame")
    descriptor = head[4]
    single = descriptor >> 5 & 1
    at += 5 + (0 if single else 1) + (0, 1, 2, 4)[descriptor & 3] + (single, 2, 4, 8)[descriptor >> 6]
    while True:
        src.seek(at)
        block = src.read(3)
        if len(block) != 3 or block[0] >> 1 & 3 == 3:
            raise ValueError("a zstd frame that ends early or holds a block of no known kind")
        word = int.from_bytes(block, "little")
        at += 3 + (1 if word >> 1 & 3 == 1 else word >> 3)
        if word & 1:
            break
    at += 4 if descriptor & 4 else 0
    if at > size:
        raise ValueError("a zstd frame that ends early")
    return at


def zstd_tool(path, frames, sink):
    """Decode the zstd frames of path, given as ranges, with the zstd executable. The executable reads
    a file from its first byte to its last and takes no zero byte between two frames, so when the
    frames are not the whole file it is given a file of the frames alone."""
    if not ZSTD_TOOL:
        raise RuntimeError("no zstd decoder")
    whole = path
    try:
        if [at for at, _end in frames] != [0] + [end for _at, end in frames[:-1]] or frames[-1][1] != os.path.getsize(path):
            whole = fresh(path + ".frames")
            with open(path, "rb") as src, os.fdopen(os.open(whole, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600), "wb") as out:
                for at, end in frames:
                    src.seek(at)
                    while at < end:
                        block = src.read(min(CHUNK, end - at))
                        if not block:
                            raise ValueError("the file is shorter than its layout")
                        at += len(block)
                        out.write(block)
        proc = subprocess.Popen([ZSTD_TOOL, "-d", "-q", "-c", "--", whole], stdout=subprocess.PIPE,
                                stderr=subprocess.DEVNULL, stdin=subprocess.DEVNULL, cwd="/")
        try:
            for block in iter(lambda: proc.stdout.read(CHUNK), b""):
                sink(block)
        finally:
            proc.stdout.close()
            code = proc.wait()
        if code != 0:
            raise RuntimeError("the zstd decoder failed")
    finally:
        if whole != path:
            os.unlink(whole)


def decode_streams(path, kind, sink):
    """Decode every stream of a gzip, bzip2, xz or zstd file, one after the other, into sink(block).
    Zero bytes between two streams and after the last are padding; any other byte that starts no
    stream raises. Returns (claimed, end): the ranges of the file that were decoded (of a gzip
    member its compressed data, without header and trailer; no part of a zstd skippable frame), and
    the offset after the last stream."""
    try:
        from compression import zstd    # Python 3.14 and later
    except ImportError:
        zstd = None
    size = os.path.getsize(path)
    claimed, end = [], 0
    with open(path, "rb") as src:
        while True:
            at = first_nonzero(src, end, size)
            src.seek(at)
            head = src.read(8)
            if kind == "zstd" and len(head) == 8 and head[1:4] == b"\x2a\x4d\x18" and 0x50 <= head[0] <= 0x5f:
                # A skippable frame: a decoder passes over its bytes. They are claimed by no member.
                end = at + 8 + int.from_bytes(head[4:], "little")
                if end > size:
                    raise ValueError("a zstd skippable frame that ends early")
            elif not head.startswith(STREAM_MAGIC[kind]):
                break
            elif kind == "gzip":
                check = [0, 0]

                def checked(block):
                    check[0], check[1] = zlib.crc32(block, check[0]), check[1] + len(block)
                    sink(block)

                body = gzip_body(src, at)
                src.seek(body)
                stop = through(zlib.decompressobj(-15), src, checked)
                src.seek(stop)
                if src.read(8) != check[0].to_bytes(4, "little") + (check[1] & 0xffffffff).to_bytes(4, "little"):
                    raise ValueError("a gzip member whose check or length is not its content's")
                claimed.append((body, stop))
                end = stop + 8
            elif kind == "zstd":
                end = zstd_frame_end(src, at, size)
                if zstd is not None:
                    src.seek(at)
                    if through(zstd.ZstdDecompressor(), src, sink) != end:
                        raise ValueError("a zstd frame that does not end where its headers say")
                claimed.append((at, end))
            else:
                src.seek(at)
                end = through(bz2.BZ2Decompressor() if kind == "bz2" else lzma.LZMADecompressor(lzma.FORMAT_XZ), src, sink)
                claimed.append((at, end))
    if not claimed:
        raise ValueError("no stream")
    if at != size:
        # Such bytes are not given to the builds as a file: they can hold a stream after any other
        # byte, where no reader looks for one. The file is not one this file reads.
        raise ValueError("bytes after the last stream that are no padding and start no stream")
    if kind == "zstd" and zstd is None:
        zstd_tool(path, claimed, sink)
    return claimed, end


def extract_streams(path, outdir, kind):
    """A gzip, bzip2, xz or zstd file: its streams decoded into one file, which is then the one
    member, or (gzip, bzip2, xz) a tar whose members are the members. Returns (format, members, bytes)."""
    plain = fresh(path + ".decoded")
    # Whether the decoded bytes are the one member (held to the member cap while they are written) or
    # a tar (a step, not a member): known for zstd from the start, else from the first 512 bytes.
    state = {"single": True if kind == "zstd" else None, "head": b"", "written": 0}
    try:
        with os.fdopen(os.open(plain, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600), "wb") as out:
            def sink(block):
                count(len(block))
                out.write(block)
                if state["written"] // (64 * CHUNK) != (state["written"] + len(block)) // (64 * CHUNK):
                    disk_guard()
                state["written"] += len(block)
                if state["single"] is None:
                    state["head"] += block[:512 - len(state["head"])]
                    if len(state["head"]) == 512:
                        state["single"] = not tar_head(state["head"])
                if state["single"] and state["written"] > MAX_MEMBER_BYTES:
                    raise RuntimeError("member exceeds size cap")

            claimed, end = decode_streams(path, kind, sink)
        size = os.path.getsize(plain)
        is_tar = kind != "zstd" and tar_head(state["head"])
        if is_tar:
            fmt = {"gzip": "tar.gz", "bz2": "tar.bz2", "xz": "tar.xz"}[kind]
            members, nbytes = extract_tar(plain, outdir, fmt, "tar-")
            ST.total_bytes -= size      # the decoded tar was a step, not a member
        else:
            fmt = kind + "-single"
            if size > MAX_MEMBER_BYTES:
                raise RuntimeError("member exceeds size cap")
            suffixes = {"gzip": (".gz",), "bz2": (".bz2",), "xz": (".xz", ".lzma"), "zstd": (".zst", ".zstd")}[kind]
            parts, reason = printed_parts([single_name(path, suffixes)], False)
            if parts is None:
                # Its name would print like another path: it is not written out, and nothing was decoded
                # into a member.
                ST.member_skips[fmt + ":" + reason] += 1
                members, nbytes, claimed = 0, 0, []
            else:
                os.replace(plain, os.path.join(outdir, parts[0]))
                members, nbytes = 1, size
    finally:
        if os.path.lexists(plain):
            os.unlink(plain)
    write_outside(outdir, path, claimed, 0, end)
    return fmt, members, nbytes


def new_outdir(path):
    return fresh(path + ".extracted")


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
            members, nbytes = extract_tar(path, outdir, fmt)
        elif kind in ("gzip", "bz2", "xz", "zstd"):
            fmt = kind + "-single"
            os.makedirs(outdir, mode=0o700)
            fmt, members, nbytes = extract_streams(path, outdir, kind)
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
    # A reader's warning can hold a member's name, and it would be written to standard error. While the
    # archives are read every warning is an error: of the archive when its reader gave it (the archive
    # is then not opened), with its type alone as its message everywhere.
    with warnings.catch_warnings():
        warnings.simplefilter("error")
        # Not the warning of a file left open: it is given where no error can be raised, and would be
        # written with the file's path. It stays unheard, as it is by default.
        warnings.simplefilter("ignore", ResourceWarning)
        while queue:
            member, level = queue.popleft()
            outdir = open_archive(member, level, os.path.relpath(member, out_root))
            if outdir and os.path.isdir(outdir):
                for p in walk_files(outdir):
                    queue.append((p, level + 1))
    return {"archives": ST.archives, "not_opened": ST.not_opened, "partial": ST.partial,
            "opened_per_format": dict(ST.opened), "sha256": digest.hexdigest(), "bytes": size}
