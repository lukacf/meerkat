#!/usr/bin/env python3
"""Scan release payload files for private text through every nested container, at any depth.

One engine does all matching and all container checks: release-evidence-privacy-scan.py beside
this file, unchanged and pinned by sha256 below. The engine decodes nested containers 4 levels
deep, does not unpack zstd, and reads no member or stream larger than 1 GiB. This wrapper walks
each payload file with the engine's own traversal (its Scanner, with matching switched off), so it
sees exactly the containers and locations the engine sees, and gives the engine what it would not
enter as extra artifacts of the same engine run, written to a private work directory and scanned
from level 0 (the walk repeats inside them, at any depth):
  - a container nested deeper than the engine's 4 levels: its bytes;
  - a zstd container: its decompressed bytes;
  - a member that is a bare zlib stream (RFC 1950; the engine reads it as text): the content of
    each complete concatenated stream, found by a valid header and a trial inflation bounded by the
    engine's size bound, and what follows the streams when that is a container;
  - gzip, bzip2 or xz streams that decompress past the engine's bound and hold a tar (a large
    product archive): the tar, cut at member boundaries into pieces below the bound. Each piece is
    the original header and data bytes of whole members plus an end-of-archive marker; the pieces
    together are every byte of the tar up to its end, and what follows must be zero bytes. Every
    concatenated stream is read. The header fields of each gzip stream (extra field, name, comment)
    are scanned as a file of their own. Bytes after the last stream that start no further stream,
    and a stream past the bound that holds no tar, refuse the payload.

  python3 -I release-payload-privacy-scan.py --rules RULES --exemptions EXEMPTIONS \\
      [--accepted-findings ACCEPTED] [--coverage-exceptions EXCEPTIONS] --report REPORT --detail DETAIL
      [--refused LISTING] [--non-published-list LIST [--show-names]] [--label NAME] [--proof-only]
      [--work DIR] [--zstd ZSTD] PAYLOAD...

The limit of "every nested container": the containers above are the ones that are recognised. A
member in any other encoding (a deflate stream without a zlib header, a truncated zlib stream, a
zlib stream with a preset dictionary, an encrypted or proprietary form) is read as the raw bytes
it is, and text inside it is not found.

The rules and exemptions are private and never committed (tests use synthetic ones). The run
refuses without them. privacy-rules-from-names.py beside this file writes both from a private
class and token name list.

Review categories: the hits of a category whose name starts with `review_` are counted in the
report (`review_hits`) and never refuse the payload; they cannot be accepted either.

Accepted findings (optional and private, kept beside the rules by the release owner; without the
file nothing is accepted), schema release-payload-privacy-scan/accepted/2:
  {"schema": "release-payload-privacy-scan/accepted/2",
   "accepted": [{"category": C, "rule": R, "sha256": H, "reason": "one line", "accepted_by": "who",
                 "class": "optional, one line", "proof": {optional, any object}}]}
  - category, rule, sha256, reason and accepted_by are required; an entry with any other key than
    these, class and proof refuses the file. The proof object is the proving tool's own (its census,
    its per-position list); it is part of the file, so the report's accepted.sha256 covers it. The
    scan reads one member of it: `tool`, the name of the tool that proved the entry. An entry is
    proved when its proof is an object whose `tool` is a tool's name: a lower-case letter or a
    digit, then lower-case letters, digits and single hyphens, 64 characters at most. The name
    reaches the upload receipt as it is written, so any other text names no tool. The scan does not
    verify a proof; it reports, of the entries it used, how many are not proved and how many each
    tool proved (accepted.used_unproved, accepted.used_by_tool), and an upload receipt is written
    only when no used entry is unproved. A finding is accepted by a tool's proof, not by hand.
  - A hit of category C is accepted only when every rule behind it is listed with C and the sha256
    H of the innermost member that holds the hit: the first sha256 the refused listing shows for it
    (the member's content; for a compressed member its decompressed content). A container around
    that member does not do.
  - The one exception: an entry whose class is PUBLIC-UPSTREAM-ARCHIVE (a third-party archive
    proved equal to its public index file) may name that archive's own sha256 and covers the hits
    inside it. It never covers through a payload file as a whole (the file, or its decompressed
    content).
  - The report counts the accepted hits by binding: accepted.used {member, archive}. An entry is
    used when it accepted a hit or a structural error; an entry that accepted nothing counts nowhere.
  - A structural error (category "structure", rule = the engine's error kind: archive-layout,
    corrupt-archive, unsupported-archive or malformed-json) is accepted only for the exact
    container it was reported in.
  - Nothing else is ever accepted: a member over the engine's size bound (other than a tar stream
    this wrapper cuts into pieces), an input that is missing, unreadable or not a regular file,
    and a zstd container or limit this wrapper could not handle always refuse.

References to members a library leaves out (--non-published-list, private; required for a release
scan): the list is the library packager's (package-library-release.py, schema
package-library-release/non-published-source-members/1). It holds the sha256 of each member the
library does not publish and of each original source snapshot. A payload member, at any depth this
scan opens, whose bytes hold one of those digests (in either case) is a hit of the category
`withheld_member_reference`, rule `withheld-member-reference`: counted, refused, and never accepted.
The report names the list's own sha256 (`non_published_list_sha256`), no listed digest and no
member; --show-names prints the path of each member that holds such a digest on standard error.
A release scan refuses (exit 4) without the list; a --proof-only scan may run without it and its
report then has `non_published_list_sha256` null.
The limit of this check: it compares digests, not paths. A listed digest is read as its 64
hexadecimal characters, in either case, also behind an escape the engine decodes. A member that
names a listed member only by its path, or states a digest in another written form (split across
lines, with separators, shortened, as raw or base64 bytes), is not a hit.

Coverage exceptions (--coverage-exceptions, optional; a committed file, schema
release-payload-privacy-scan/coverage-exceptions/1): the scan passes only when every member was read
at every depth. A member it cannot read (the engine's archive-layout, corrupt-archive,
unsupported-archive or malformed-json error) refuses the payload unless an entry names it exactly:
  {"path": P, "sha256": H, "kind": K, "reason": "one line", "accepted_by": "who"}
P is the member's path inside the payload (the listing's path, or a suffix of it that starts at a
member boundary), H the sha256 of the container the error was reported in, K the error kind. The
report counts the entries used on their own (`coverage_exceptions`). A member over the size bound,
an unreadable input and a container this wrapper could not handle can never be excepted.

The refused listing (--refused, optional, a new file, mode 0600, schema
release-payload-privacy-scan/refused/1) is what a release owner reads to decide what to accept: each
refused hit with its category, rule ids, count, its path inside the payload and the sha256 of the
file or member that holds it followed by the sha256 of each container that encloses it (the payload
file last); each refused error with its kind, path and container sha256. It never holds matched
text: a path that itself matches a rule, as it is or in a decoded form (JSON or percent escapes, as
the engine decodes them), is withheld (`location` null, `location_withheld` true; the digests still
identify the bytes). It names members: keep it with the detail, never upload it.

Locations join a container and a member with `!`. A member whose name holds `!` so that a part of
its location is another location (a member `a!b` beside a member `a`) refuses the payload: a hit in
one could be bound to the other's digest. A payload path or work directory with `!` is refused
before the scan. A name with `!` that no other location shares is scanned as any other.

The report (--report, a new file, schema release-payload-privacy-scan/report/3) holds no matched text
and no member name. It says what ran and on what, so a later check can tell a current report from a
stale or an empty one:
  status              "PASS" or "REFUSED"; nothing else is a status
  reasons             why it is REFUSED, by name: hits-refused, withheld-member-reference,
                      errors-refused, wrapper-failures, payload-changed-during-scan,
                      no-blocking-rules, home-paths-off, exemption-in-blocking-category (empty for PASS)
  release, label      release is false for a --proof-only run, and its label then starts with
                      PROOF-ONLY-; a release report's label is the given --label or null
  generated_at        UTC, when the report was written
  nonce               32 hex digits, random in every run: an upload receipt carries the sha256 of
                      this report, and with the nonce that digest confirms no guess of the rest
  tool_commit         the git commit of the checkout this tool ran from; tool_sha256 is this file's
  rules               {sha256, total, blocking, review_outside_paths}: blocking counts the rules outside
                      review categories; review_outside_paths counts the review rules that are not the
                      converter's path category (`review_home_paths` exactly as `--paths review` writes
                      it), that is the rules of a name class that refuse nothing. An upload receipt
                      is written only when it is 0
  name_list_sha256    the name list digest the rules file declares (privacy-rules-from-names.py), or null
  non_published_list_sha256  the sha256 of the --non-published-list file, or null (proof-only runs)
  exemptions          {sha256, count, blocking}
  accepted            {sha256, count, used {member, archive}, used_unproved, used_by_tool {tool: entries}};
                      coverage_exceptions {sha256, count, used}
  payload             [{name, bytes, sha256}]: the bytes the walk read. The engine reads each file
                      again; when its digest differs the report is REFUSED
  hits, review_hits, errors, containers, coverage, reroots, wrapper_failures: counts
A release scan refuses (exit 4) when the commit of this tool cannot be determined: git must find a
checkout around this file, and this file and the engine must be what its commit holds. --proof-only
is the explicit override for a run that is not a release scan.
A report cannot be PASS when the rules can refuse nothing: no rule outside the review categories, no
home path category (`home_paths` or `review_home_paths`; `--paths off` in the converter), or an
exemption in a category that is not a review category.

Exit status (bit flags, as the engine's): 0 PASS; 1 a refused hit; 2 coverage incomplete (a refused
error, a container this wrapper could not handle, a limit, a payload file that changed during the
scan, rules that can refuse nothing); 3 both; 4 usage or configuration, no report (a missing rules or
exemptions file, a pin mismatch, a malformed accepted-findings file, an undeterminable tool commit,
an engine usage error). The engine's detail (--detail, a new file, mode 0600) holds the matched
strings: it is private, never upload it.
Standard library only, no network; git is run to read the tool's commit.
"""
from __future__ import annotations

import argparse
import datetime
import hashlib
import json
import os
import re
import secrets
import shutil
import subprocess
import sys
import tarfile
import tempfile
import types
import zlib
from pathlib import Path

HERE = Path(__file__).resolve().parent
ENGINE = HERE / "release-evidence-privacy-scan.py"
# The vendored engine, byte for byte.
ENGINE_SHA256 = "9405cacc8e3f4d7e0aea5a1ec2087bf50565425d9dbb5e9d5b199a5366f19485"
SCHEMA_REPORT = "release-payload-privacy-scan/report/3"
SCHEMA_ACCEPTED = "release-payload-privacy-scan/accepted/2"
SCHEMA_REFUSED = "release-payload-privacy-scan/refused/1"
SCHEMA_EXCEPTIONS = "release-payload-privacy-scan/coverage-exceptions/1"
# The library packager's private list of members a library leaves out (package-library-release.py).
SCHEMA_NON_PUBLISHED = "package-library-release/non-published-source-members/1"
# The category and the one rule id of a payload member that holds a digest of that list.
WITHHELD_CATEGORY, WITHHELD_RULE = "withheld_member_reference", "withheld-member-reference"
STRUCTURE = "structure"
# An accepted finding of this class may name the digest of an archive that encloses the hit.
ARCHIVE_CLASS = "PUBLIC-UPSTREAM-ARCHIVE"
ACCEPTED_REQUIRED = {"category", "rule", "sha256", "reason", "accepted_by"}
ACCEPTED_OPTIONAL = {"class", "proof"}
# The one form of a proof tool's name: a lower-case letter or a digit, then lower-case letters,
# digits and single hyphens, 64 characters at most. The receipt writer and the receipt check hold
# the same form.
PROOF_TOOL = re.compile(r"(?!.*--)[a-z0-9][a-z0-9-]{0,63}")
# What the engine calls the content of a compressed stream that holds no tar.
DECOMPRESSED = re.compile(r"!<(?:gzip|bzip2|xz)-decompressed>\Z")
# A category whose name starts with this prefix is counted and reported, never refused or accepted.
REVIEW_PREFIX = "review_"
# The home directory path category privacy-rules-from-names.py writes (blocking, or as a review category).
HOME_PATH_CATEGORIES = ("home_paths", REVIEW_PREFIX + "home_paths")
# That category exactly as the converter writes it with --paths review: the one review category that
# is not a class of names. Any other content under its name counts as review rules of a name class.
REVIEW_PATH_CATEGORY = {"rules": [{"id": "home-paths", "kind": "regex", "value": r"/(?:home|Users)/[^/\s\"'<>]+/",
                                   "prefilter": ["/home/", "/users/"]}]}
PASS, REFUSED = "PASS", "REFUSED"
# The label of a report that is not a release report starts with this.
PROOF_PREFIX = "PROOF-ONLY-"
LABEL = re.compile(r"[A-Za-z0-9][A-Za-z0-9._-]{0,79}\Z")
ACCEPTABLE_ERRORS = ("archive-layout", "corrupt-archive", "unsupported-archive", "malformed-json")
HEX64 = re.compile(r"[0-9a-f]{64}\Z")
EXIT_HITS, EXIT_COVERAGE, EXIT_USAGE = 1, 2, 4
TAR_BLOCK = 512
CHUNK = 1 << 20
GZIP_FIXED_HEADER = 10


class Usage(Exception):
    pass


def split_tar(path: Path, piece_bytes: int, out_prefix: Path) -> tuple[list[Path], bool]:
    """Cut the tar at path into pieces at member boundaries: each piece is the original bytes from the
    end of the previous piece through the end of whole members (header, extension and data blocks),
    followed by an end-of-archive marker, and holds at most piece_bytes of original bytes unless one
    member alone is larger. Returns the piece paths and whether everything after the last member is
    zero bytes. Raises tarfile.TarError, OSError or ValueError for a tar it cannot read."""
    ends = []
    with tarfile.open(path, "r:") as archive:
        for member in archive:
            data_blocks = -(-member.size // TAR_BLOCK) * TAR_BLOCK
            if member.isreg() or member.type not in tarfile.SUPPORTED_TYPES:
                ends.append(member.offset_data + data_blocks)
            else:
                ends.append(member.offset_data)
    if not ends:
        raise ValueError("a tar without members")
    cuts, start, previous = [], 0, None
    for end in ends:
        if previous is not None and end - start > piece_bytes:
            cuts.append((start, previous))
            start = previous
        previous = end
    cuts.append((start, previous))
    pieces = []
    with open(path, "rb") as source:
        for index, (begin, end) in enumerate(cuts):
            piece = out_prefix.with_name(f"{out_prefix.name}.piece{index + 1:03d}.tar")
            source.seek(begin)
            with open(piece, "xb") as target:
                remaining = end - begin
                while remaining:
                    block = source.read(min(remaining, 1 << 20))
                    if not block:
                        raise ValueError("the tar is shorter than its members")
                    target.write(block)
                    remaining -= len(block)
                target.write(b"\x00" * (2 * TAR_BLOCK))
            os.chmod(piece, 0o600)
            pieces.append(piece)
        source.seek(previous)
        zero_tail = True
        while block := source.read(1 << 20):
            if block.count(0) != len(block):
                zero_tail = False
                break
    return pieces, zero_tail


def inflate_streams(engine, kind: str, data: bytes, sink, limit: int) -> list[bytes]:
    """Decompress every concatenated gzip, bzip2 or xz stream of data into sink(block), the way the engine
    reads a stream within its bound but without holding the result. Returns one header per stream (the
    gzip header bytes; empty for bzip2 and xz, which carry no metadata). Zero padding between and after
    streams is skipped. Raises ValueError, the decoder's error or the engine's Refusal for a damaged,
    truncated or unaccepted stream, and RuntimeError past limit bytes or when bytes that start no
    further stream follow the last one."""
    magic = engine.STREAM_MAGIC[kind]
    headers: list[bytes] = []
    position = total = 0
    while True:
        headers.append(data[position:engine.gzip_header_end(data, position)] if kind == "gzip" else b"")
        decompressor = engine.new_decompressor(kind)
        cursor, pending = position, b""
        while not decompressor.eof:
            if pending:
                feed = pending
            elif kind != "gzip" and not decompressor.needs_input:
                feed = b""
            else:
                feed = data[cursor:cursor + CHUNK]
                cursor += len(feed)
                if not feed:
                    raise ValueError("the stream ends before its end marker")
            block = decompressor.decompress(feed, CHUNK)
            pending = decompressor.unconsumed_tail if kind == "gzip" else b""
            total += len(block)
            if total > limit:
                raise RuntimeError("re-root byte limit")
            sink(block)
        following = engine.NONZERO.search(data, cursor - len(decompressor.unused_data))
        if following is None:
            return headers
        if not data.startswith(magic, following.start()):
            raise RuntimeError("bytes after the last stream that start no further stream")
        position = following.start()


def zlib_header(data: bytes, start: int = 0) -> bool:
    """Whether a zlib stream header (RFC 1950: deflate, a window of at most 32 KiB, a valid check value,
    no preset dictionary) stands at start."""
    if len(data) < start + 2:
        return False
    method, flags = data[start], data[start + 1]
    return method & 0x0F == 8 and method >> 4 <= 7 and not flags & 0x20 and (method << 8 | flags) % 31 == 0


def inflate_zlib(data: bytes, limit: int) -> tuple[list[bytes], bytes] | None:
    """A trial inflation of data as bare zlib streams: (the content of each complete concatenated stream,
    the bytes after the last one), or None when data does not begin with a complete zlib stream (text
    that happens to begin like one, or a truncated stream). Raises RuntimeError past limit bytes."""
    contents: list[bytes] = []
    position = total = 0
    while zlib_header(data, position):
        decompressor = zlib.decompressobj()
        try:
            content = decompressor.decompress(data[position:], limit - total + 1)
        except zlib.error:
            break
        if len(content) > limit - total:
            raise RuntimeError("zlib content over the size bound")
        if not decompressor.eof:
            break
        contents.append(content)
        total += len(content)
        position = len(data) - len(decompressor.unused_data)
    return (contents, data[position:]) if contents else None


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with open(path, "rb") as stream:
        for block in iter(lambda: stream.read(1 << 20), b""):
            digest.update(block)
    return digest.hexdigest()


def tool_commit() -> str:
    """The commit of the checkout this tool runs from. Raises Usage when there is none, or when this
    file or the engine is not exactly what that commit holds. The checkout is found from this
    file's directory: git variables of the caller's environment are not passed on, and git is told to
    read the commit's own objects, not objects a replace ref puts in their place."""
    environment = {key: value for key, value in os.environ.items() if not key.startswith("GIT_")}

    def git(*arguments: str) -> str:
        try:
            done = subprocess.run(["git", "--no-replace-objects", "-C", str(HERE), *arguments], stdin=subprocess.DEVNULL,
                                  env=environment, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, check=False)
        except OSError:
            raise Usage("the commit of this tool cannot be determined: git did not run") from None
        if done.returncode != 0:
            raise Usage("the commit of this tool cannot be determined: it is not in a git checkout")
        return done.stdout.decode("utf-8", "replace").strip()

    commit = git("rev-parse", "--verify", "HEAD^{commit}")
    if not re.fullmatch(r"[0-9a-f]{40}(?:[0-9a-f]{24})?", commit):
        raise Usage("the commit of this tool cannot be determined")
    if git("status", "--porcelain", "--", str(Path(__file__).resolve()), str(ENGINE)):
        raise Usage("this tool or its engine is not the file its checkout's commit holds")
    return commit


def load_engine():
    """The pinned engine as a module, compiled from the exact bytes that were hashed."""
    raw = ENGINE.read_bytes()
    if hashlib.sha256(raw).hexdigest() != ENGINE_SHA256:
        raise Usage("the vendored engine differs from its pinned sha256")
    module = types.ModuleType("release_evidence_privacy_scan")
    module.__file__ = str(ENGINE)
    exec(compile(raw, str(ENGINE), "exec"), module.__dict__)
    return module


def read_document(path: Path, label: str) -> tuple[object, str]:
    """A JSON file and the sha256 of the bytes that were parsed."""
    try:
        raw = path.read_bytes()
        return json.loads(raw), hashlib.sha256(raw).hexdigest()
    except (OSError, ValueError) as error:
        raise Usage(f"the {label} file cannot be read: {type(error).__name__}") from None


def proof_tool(entry: dict) -> str | None:
    """The tool an acceptance entry names as the one that proved it: the `tool` of its proof object,
    a name of the one form PROOF_TOOL. None for an entry that names none: no proof, no `tool`, or a
    `tool` that is any other text. The rest of a proof is the tool's own."""
    proof = entry.get("proof")
    tool = proof.get("tool") if isinstance(proof, dict) else None
    return tool if isinstance(tool, str) and PROOF_TOOL.fullmatch(tool) else None


def read_accepted(path: Path | None) -> tuple[dict, str | None, int, list]:
    """({(category, rule): {"member": {sha256, ...}, "archive": {sha256, ...}}}, the file's sha256, its
    number of entries, and per entry (category, rule, sha256, whether it is an ARCHIVE_CLASS entry, its
    proof tool or None)); an absent file accepts nothing. Every entry's digest is in "member";
    "archive" holds the digests of the ARCHIVE_CLASS entries."""
    accepted: dict = {}
    if path is None:
        return accepted, None, 0, []
    value, digest = read_document(path, "accepted-findings")
    if not isinstance(value, dict) or set(value) != {"schema", "accepted"} or value["schema"] != SCHEMA_ACCEPTED \
            or not isinstance(value["accepted"], list):
        raise Usage(f"the accepted-findings file is not a {SCHEMA_ACCEPTED} document")
    entries: list = []
    for entry in value["accepted"]:
        if not isinstance(entry, dict) or not ACCEPTED_REQUIRED <= set(entry) <= ACCEPTED_REQUIRED | ACCEPTED_OPTIONAL \
                or not all(isinstance(entry[key], str) and entry[key].strip() and "\n" not in entry[key]
                           for key in entry if key != "proof") \
                or not HEX64.fullmatch(entry["sha256"]) or not isinstance(entry.get("proof", {}), dict) \
                or entry["category"] == WITHHELD_CATEGORY \
                or (entry["category"] == STRUCTURE and entry["rule"] not in ACCEPTABLE_ERRORS):
            raise Usage("an accepted finding needs category, rule, an exact sha256, a one-line reason and who "
                        "accepted it (accepted_by); class and one proof object are optional")
        sets = accepted.setdefault((entry["category"], entry["rule"]), {"member": set(), "archive": set()})
        sets["member"].add(entry["sha256"])
        if entry.get("class") == ARCHIVE_CLASS:
            sets["archive"].add(entry["sha256"])
        entries.append((entry["category"], entry["rule"], entry["sha256"], entry.get("class") == ARCHIVE_CLASS, proof_tool(entry)))
    return accepted, digest, len(value["accepted"]), entries


def read_exceptions(path: Path | None) -> tuple[list[dict], str | None]:
    """(the coverage exceptions: members the scan cannot read, each bound to its exact bytes; the file's
    sha256)."""
    if path is None:
        return [], None
    value, digest = read_document(path, "coverage exceptions")
    if not isinstance(value, dict) or set(value) != {"schema", "exceptions"} or value["schema"] != SCHEMA_EXCEPTIONS \
            or not isinstance(value["exceptions"], list):
        raise Usage(f"the coverage exceptions file is not a {SCHEMA_EXCEPTIONS} document")
    for entry in value["exceptions"]:
        if not isinstance(entry, dict) or set(entry) != {"path", "sha256", "kind", "reason", "accepted_by"} \
                or not all(isinstance(entry[key], str) and entry[key].strip() for key in entry) \
                or not HEX64.fullmatch(entry["sha256"]) or entry["kind"] not in ACCEPTABLE_ERRORS \
                or any("\n" in entry[key] for key in ("path", "reason", "accepted_by")):
            raise Usage("a coverage exception needs path, an exact sha256, an acceptable error kind, "
                        "a one-line reason and who accepted it")
    return value["exceptions"], digest


def read_non_published(path: Path | None) -> tuple[list[str], str | None]:
    """(every digest of the library packager's private list: the members a library leaves out and the
    original source snapshots; the file's sha256). Only the digests are used here."""
    if path is None:
        return [], None
    value, digest = read_document(path, "non-published list")
    problem = Usage(f"the non-published list is not a {SCHEMA_NON_PUBLISHED} document")
    if not isinstance(value, dict) or set(value) != {"schema", "trees", "original_snapshots"} \
            or value["schema"] != SCHEMA_NON_PUBLISHED or not isinstance(value["trees"], dict) \
            or not isinstance(value["original_snapshots"], dict):
        raise problem
    digests: set[str] = set()
    for rows in value["trees"].values():
        if not isinstance(rows, list) or not all(isinstance(row, dict) and set(row) == {"name", "sha256"} for row in rows):
            raise problem
        digests.update(row["sha256"] for row in rows)
    for rows in value["original_snapshots"].values():
        if not isinstance(rows, list):
            raise problem
        digests.update(rows)
    if not all(isinstance(item, str) and HEX64.fullmatch(item) for item in digests):
        raise problem
    return sorted(digests), digest


def zstd_decompress(data: bytes, zstd_cli: str | None, limit: int) -> bytes:
    try:
        from compression import zstd  # Python 3.14 and later
    except ImportError:
        zstd = None
    if zstd is not None:
        content = zstd.decompress(data)
    elif not zstd_cli:
        raise RuntimeError("no zstd decoder")
    else:
        done = subprocess.run([zstd_cli, "-d", "-c", "-q"], input=data, stdout=subprocess.PIPE,
                              stderr=subprocess.DEVNULL, check=False)
        if done.returncode != 0:
            raise RuntimeError("zstd decoding failed")
        content = done.stdout
    if len(content) > limit:
        raise RuntimeError("zstd content over the size bound")
    return content


def walker_class(engine):
    walk_rules = engine.Rules(
        {"schema": engine.SCHEMA_RULES, "categories": {"walk": {"rules": [
            {"id": "walk", "kind": "literal", "value": "\x00walk-only\x00"}]}}},
        {"schema": engine.SCHEMA_EXEMPTIONS, "exemptions": []})

    class Walker(engine.Scanner):
        """The engine's own traversal with matching switched off: it records each location's bytes
        (sha256), which locations are containers, and re-roots what the engine would not enter."""

        def __init__(self, work: Path, zstd_cli: str | None, max_reroots: int, max_bytes: int, max_nesting: int,
                     piece_bytes: int):
            super().__init__(walk_rules)
            self.work, self.zstd_cli = work, zstd_cli
            self.max_reroots, self.max_bytes, self.max_nesting = max_reroots, max_bytes, max_nesting
            self.piece_bytes = piece_bytes
            self.digests: dict[str, str] = {}
            self.sizes: dict[str, int] = {}
            # Files this wrapper cut or assembled: their digests are not a member's.
            self.synthetic: set[str] = set()
            self.container_locations: set[str] = set()
            self.reroots: dict[str, tuple[str, list[str]]] = {}
            self.origins: dict[str, str] = {}
            self.nesting: dict[str, int] = {}
            self.artifacts: list[str] = []
            self.failures: list[str] = []
            self.reroot_bytes = 0
            self.counter = 0
            self.roots: list[str] = []
            self.root_paths: set[str] = set()
            # (location, name) of each member whose name holds the location separator.
            self.separator_names: list[tuple[str, str]] = []

        def scan_string(self, text, location, where):  # no matching during the walk
            return 0

        def scan_name(self, name, location, kind="name"):
            if kind == "name" and "!" in name:
                self.separator_names.append((location, name))
            return super().scan_name(name, location, kind)

        def ambiguous_names(self) -> int:
            """Members whose name holds "!" so that a part of their location is another location: a hit
            in one could then be bound to the other's digest."""
            known = self.digests.keys() | self.container_locations
            return sum(1 for location, name in self.separator_names if any(
                location[:len(location) - len(name) + index] in known
                for index, character in enumerate(name) if character == "!"))

        def walk(self, index: int, path: str, nesting: int) -> None:
            self.nesting[path] = nesting
            self.root_paths.add(path)
            self.roots.append(path)
            try:
                self.scan_artifact(index, path)
            finally:
                self.roots.pop()

        def scan_content(self, data, name, location, depth, required=False):
            self.digests.setdefault(location, hashlib.sha256(data).hexdigest())
            if depth == 0:
                self.sizes.setdefault(location, len(data))
            kind = engine.archive_kind(data)
            if kind is None:
                # The engine reads it as text. A bare zlib stream is inflated here and scanned as well.
                if zlib_header(data):
                    self.reroot(location, "zlib", name, lambda: self.inflated(data, name))
                return
            if kind == "opaque-zstd":
                self.reroot(location, "zstd", name, lambda: [self.write(name, ".zstd-decompressed", zstd_decompress(
                    data, self.zstd_cli, engine.MAX_MEMBER_BYTES))])
                return
            if kind.startswith("opaque-"):
                return  # the engine reports it as unsupported; it cannot be decoded here either
            self.container_locations.add(location)
            if depth >= engine.MAX_DEPTH:
                self.reroot(location, "depth", name, lambda: [self.write(name, "", data)])
                return
            seen = len(self.detail_errors)
            super().scan_content(data, name, location, depth, required)
            if kind in engine.STREAM_MAGIC and any(
                    error["kind"] == "member-too-large" and error["location"] == location
                    for error in self.detail_errors[seen:]):
                self.reroot(location, "split", name, lambda: self.split(kind, data, name))

        def write(self, name: str, suffix: str, data: bytes) -> Path:
            self.reroot_bytes += len(data)
            if self.reroot_bytes > self.max_bytes:
                raise RuntimeError("re-root byte limit")
            path = self.new_path(name, suffix)
            with open(path, "xb") as stream:
                stream.write(data)
            os.chmod(path, 0o600)
            return path

        def new_path(self, name: str, suffix: str) -> Path:
            self.counter += 1
            base = re.sub(r"[^A-Za-z0-9._-]", "_", name.rsplit("/", 1)[-1])[-80:] or "member"
            if suffix == ".zstd-decompressed":
                base = re.sub(r"\.(zst|zstd)$", "", base)
            return self.work / f"{self.counter:05d}-{base}{suffix}"

        def inflated(self, data: bytes, name: str) -> list[Path]:
            """The content of each bare zlib stream data begins with, and what follows them when that is
            a container; nothing when data is no complete zlib stream."""
            result = inflate_zlib(data, engine.MAX_MEMBER_BYTES)
            if result is None:
                return []
            contents, rest = result
            paths = [self.write(name, ".zlib-inflated", content) for content in contents]
            if engine.archive_kind(rest) is not None or zlib_header(rest):
                # Bytes of the member itself, not a member: the engine has read them as text already.
                paths.append(self.write(name, ".after-zlib", rest))
                self.synthetic.add(str(paths[-1]))
            return paths

        def split(self, kind: str, data: bytes, name: str) -> list[Path]:
            """Streams past the engine's bound that hold a tar: decompress every concatenated stream to
            the work directory and cut the tar into pieces at member boundaries. The header fields of each
            gzip stream (extra field, name, comment) become a file of their own. Anything else is not
            handled."""
            tar_path = self.new_path(name, ".decompressed")
            try:
                with open(tar_path, "xb") as target:
                    headers = inflate_streams(engine, kind, data, target.write, self.max_bytes - self.reroot_bytes)
                with open(tar_path, "rb") as stream:
                    head = stream.read(engine.TAR_BLOCK)
                if len(head) < 262 or head[257:262] != b"ustar":
                    raise RuntimeError("a stream past the size bound that is not a tar")
                paths, zero_tail = split_tar(tar_path, self.piece_bytes, tar_path)
            finally:
                tar_path.unlink(missing_ok=True)
            if not zero_tail:
                raise RuntimeError("non-zero bytes after the last member of a tar past the size bound")
            self.reroot_bytes += sum(piece.stat().st_size for piece in paths)
            for header in headers:
                if len(header) > GZIP_FIXED_HEADER:
                    # The line break keeps the first field from reading as the start of a container.
                    paths.append(self.write(name, ".gzip-header", b"\n" + header[GZIP_FIXED_HEADER:]))
            self.synthetic.update(str(path) for path in paths)
            return paths

        def reroot(self, location: str, reason: str, name: str, produce) -> None:
            if location in self.reroots:
                return
            nesting = self.nesting[self.roots[-1]] + 1
            if len(self.artifacts) >= self.max_reroots or nesting > self.max_nesting:
                self.failures.append("re-root limit")
                return
            try:
                paths = produce()
            except Exception as error:  # fail closed: counted as a container that was not handled
                self.failures.append(f"{reason}: {type(error).__name__}")
                return
            if not paths:
                return  # nothing to enter: nothing is recorded as re-rooted
            self.reroots[location] = (reason, [str(path) for path in paths])
            for path in paths:
                self.origins[str(path)] = location
                self.artifacts.append(str(path))
            for path in paths:
                self.walk(10**6 + self.artifacts.index(str(path)), str(path), nesting)

    return Walker


def prefixes(location: str):
    """location, then each of its '!'-separated ancestors, longest first."""
    yield location
    position = len(location)
    while (position := location.rfind("!", 0, position)) > 0:
        yield location[:position]


def classify(walker, detail: dict, accepted: dict, exceptions: list[dict] = (), matches=None, entries: list = ()) -> dict:
    roots = walker.root_paths

    def top(location: str) -> bool:
        """Whether location is a payload file as a whole: the file, its decompressed content, or a file this
        wrapper wrote from either."""
        while True:
            location = DECOMPRESSED.sub("", location)
            if location not in roots:
                return False
            if location not in walker.origins:
                return True
            location = walker.origins[location]

    def chain(location: str) -> list[tuple[str, bool]]:
        """(sha256, whether it is a payload file as a whole) of the file or member at location, then of each
        container that encloses it. A file this wrapper cut or assembled is no member and is left out."""
        found: list[tuple[str, bool]] = []
        while True:
            root = None
            for prefix in prefixes(location):
                digest = walker.digests.get(prefix)
                if digest is not None and prefix not in walker.synthetic and all(digest != row[0] for row in found):
                    found.append((digest, top(prefix)))
                if prefix in roots:
                    root = prefix
            if root is None or root not in walker.origins:
                return found
            location = walker.origins[root]

    def shown(location: str) -> str:
        """location as a path inside the payload: a re-rooted artifact is replaced by where it came from,
        and the payload file by its name."""
        while True:
            root = next((prefix for prefix in prefixes(location) if prefix in roots), None)
            if root is None:
                return location
            if root not in walker.origins:
                return os.path.basename(root) + location[len(root):]
            location = walker.origins[root] + location[len(root):]

    def container_digest(location: str) -> str | None:
        for prefix in prefixes(location):
            if prefix in walker.container_locations:
                return walker.digests.get(prefix)
        return None

    zstd_unsupported = {c["location"] for c in detail["containers"]
                        if c["kind"] == "zstd" and c["outcome"] == "unsupported"}
    counts = {"hits": {"accepted": {}, "refused": {}}, "review_hits": {},
              "errors": {"covered": {}, "accepted": {}, "refused": {}}, "bindings": {"member": 0, "archive": 0}}
    nothing = {"member": set(), "archive": set()}
    refused_hits: dict[tuple, dict] = {}
    refused_errors: list[dict] = []
    used: dict[str, int] = {}
    # The acceptance entries that accepted something, by their place in the file.
    used_entries: set[int] = set()

    def excepted(kind: str, location: str) -> bool:
        digest, where = container_digest(location), shown(location)
        return any(entry["kind"] == kind and entry["sha256"] == digest
                   and (where == entry["path"] or where.endswith(("/" + entry["path"], "!" + entry["path"])))
                   for entry in exceptions)

    def listed(location: str) -> dict:
        """A path for the listing; withheld when the path itself matches a rule."""
        where = shown(location)
        if matches is not None and matches(where):
            return {"location": None, "location_withheld": True}
        return {"location": where, "location_withheld": False}

    def add(group: dict, key: str, n: int) -> None:
        group[key] = group.get(key, 0) + n

    for hit in detail["hits"]:
        if hit["category"].startswith(REVIEW_PREFIX):
            add(counts["review_hits"], hit["category"], int(hit["count"]))
            continue
        holders = chain(hit["location"])
        digests = [digest for digest, _ in holders]
        # An entry binds to the innermost member that holds the hit. An ARCHIVE_CLASS entry may instead
        # name an archive around it, never a payload file as a whole.
        inner = set(digests[:1])
        archives = {digest for digest, whole in holders if not whole}
        bound = [accepted.get((hit["category"], rule), nothing) for rule in hit["rules"]]
        ok = bool(bound) and all(entry["member"] & inner or entry["archive"] & archives for entry in bound)
        add(counts["hits"]["accepted" if ok else "refused"], hit["category"], int(hit["count"]))
        if ok:
            binding = "member" if all(entry["member"] & inner for entry in bound) else "archive"
            counts["bindings"][binding] += int(hit["count"])
            used_entries.update(place for place, (category, rule, digest, archive, _) in enumerate(entries)
                                if category == hit["category"] and rule in hit["rules"]
                                and (digest in inner or (archive and digest in archives)))
        else:
            key = (hit["category"], tuple(sorted(hit["rules"])), hit["location"])
            row = refused_hits.setdefault(key, {"category": hit["category"], "rules": sorted(hit["rules"]),
                                                "count": 0, **listed(hit["location"]), "sha256": digests})
            row["count"] += int(hit["count"])
    for error in detail["errors"]:
        kind, location = error["kind"], error["location"]
        reason = walker.reroots.get(location, (None, []))[0]
        if (kind, reason) in (("depth-limit", "depth"), ("member-too-large", "split")) or (
                kind == "unsupported-archive" and reason == "zstd" and location in zstd_unsupported):
            add(counts["errors"]["covered"], kind, 1)
        # Only an acceptable kind can be named by an accepted finding or an exception: both files are
        # refused as a whole otherwise (read_accepted, read_exceptions).
        elif container_digest(location) in accepted.get((STRUCTURE, kind), nothing)["member"]:
            add(counts["errors"]["accepted"], kind, 1)
            used_entries.update(place for place, (category, rule, digest, _, _) in enumerate(entries)
                                if (category, rule, digest) == (STRUCTURE, kind, container_digest(location)))
        elif excepted(kind, location):
            add(used, kind, 1)
        else:
            add(counts["errors"]["refused"], kind, 1)
            refused_errors.append({"kind": kind, **listed(location), "container_sha256": container_digest(location)})
    counts["exceptions_used"] = used
    # What the used entries say of their proof: how many name no tool, and how many each tool proved.
    tools = [entries[place][4] for place in sorted(used_entries)]
    counts["proofs"] = {"unproved": tools.count(None),
                        "tools": {tool: tools.count(tool) for tool in sorted({tool for tool in tools if tool is not None})}}
    counts["refused"] = {"schema": SCHEMA_REFUSED, "hits": list(refused_hits.values()), "errors": refused_errors}
    return counts


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--rules", type=Path, help="the private rules file (required)")
    parser.add_argument("--exemptions", type=Path, help="the private exemptions file (required)")
    parser.add_argument("--accepted-findings", type=Path,
                        help="the private accepted-findings file (optional; without it nothing is accepted)")
    parser.add_argument("--report", type=Path, required=True, help="counts-only report (a new file)")
    parser.add_argument("--detail", type=Path, required=True, help="the engine's private detail (a new file, 0600)")
    parser.add_argument("--coverage-exceptions", type=Path,
                        help="members the scan cannot read, each bound to its exact bytes (a committed file)")
    parser.add_argument("--refused", type=Path,
                        help="what was refused, by rule and file digests, without matched text (a new file, 0600)")
    parser.add_argument("--non-published-list", type=Path,
                        help="the library packager's private list of members a library leaves out: a payload "
                             "member that holds one of its digests refuses (required for a release scan)")
    parser.add_argument("--show-names", action="store_true",
                        help="name the payload members that hold such a digest on standard error")
    parser.add_argument("--proof-only", action="store_true",
                        help="not a release scan: the report says so, its label starts with PROOF-ONLY-, "
                             "and the commit of this tool need not be determinable")
    parser.add_argument("--label", help="a name for this run, kept in the report")
    parser.add_argument("--work", type=Path, help="parent of the private work directory")
    parser.add_argument("--keep-work", action="store_true")
    parser.add_argument("--zstd", help="a zstd executable, used when Python has no zstd decoder")
    parser.add_argument("--max-reroots", type=int, default=100000)
    parser.add_argument("--max-reroot-bytes", type=int, default=64 << 30)
    parser.add_argument("--max-nesting", type=int, default=64)
    parser.add_argument("--piece-bytes", type=int, default=256 << 20,
                        help="the target size of the pieces a tar past the engine's bound is cut into")
    parser.add_argument("payload", nargs="*", type=Path)
    args = parser.parse_args()
    try:
        for label, path in (("rules", args.rules), ("exemptions", args.exemptions)):
            if path is None or not path.is_file():
                raise Usage(f"a {label} file is required (--{label}); release scanning refuses without one")
        if args.accepted_findings is not None and not args.accepted_findings.is_file():
            raise Usage("the accepted-findings file does not exist")
        if not args.payload:
            raise Usage("at least one payload file is required")
        for path in args.payload:
            if path.is_symlink() or not path.is_file():
                raise Usage(f"a payload is not a regular file: {path.name}")
        # "!" joins a container and a member in every location.
        if any("!" in str(path.resolve()) for path in (*args.payload, args.work or Path(tempfile.gettempdir()))):
            raise Usage("a payload path or the work directory holds '!', the separator of locations")
        if args.report.exists() or args.detail.exists() or (args.refused is not None and args.refused.exists()):
            raise Usage("the report, detail and refused listing must be new files")
        if args.label is not None and not LABEL.match(args.label):
            raise Usage("--label is 1 to 80 letters, digits, dots, underscores or hyphens")
        if args.proof_only:
            label = args.label or "run"
            label = label if label.startswith(PROOF_PREFIX) else PROOF_PREFIX + label
            try:
                commit = tool_commit()
            except Usage:
                commit = None
        else:
            if args.label is not None and args.label.startswith(PROOF_PREFIX):
                raise Usage(f"a release scan cannot carry a {PROOF_PREFIX} label")
            if args.non_published_list is None:
                raise Usage("a release scan needs --non-published-list: without it nothing is checked for "
                            "digests of members a library leaves out (--proof-only marks a run that is not a "
                            "release scan)")
            label = args.label
            try:
                commit = tool_commit()
            except Usage as error:
                raise Usage(f"{error}; a release scan refuses without it (--proof-only marks a run that is "
                            "not a release scan)") from None
        engine = load_engine()
        if not 0 < args.piece_bytes < engine.MAX_MEMBER_BYTES:
            raise Usage("--piece-bytes must be positive and below the engine's size bound")
        accepted, accepted_sha256, accepted_count, accepted_entries = read_accepted(args.accepted_findings)
        exceptions, exceptions_sha256 = read_exceptions(args.coverage_exceptions)
        try:
            rules_raw, exemptions_raw = args.rules.read_bytes(), args.exemptions.read_bytes()
            rules_doc, exemptions_doc = json.loads(rules_raw), json.loads(exemptions_raw)
            scan_rules = engine.Rules(rules_doc, exemptions_doc)
        except (OSError, ValueError, engine.ConfigError) as error:
            raise Usage(f"the rules or exemptions cannot be used: {type(error).__name__}") from None
        name_list = rules_doc.get("name_list_sha256")
        if name_list is not None and not (isinstance(name_list, str) and HEX64.fullmatch(name_list)):
            raise Usage("the rules file's name_list_sha256 is not a sha256")
        withheld, withheld_sha256 = read_non_published(args.non_published_list)
        if WITHHELD_CATEGORY in scan_rules.categories or WITHHELD_RULE in (row[1] for row in scan_rules.compiled):
            raise Usage(f"the rules use the category or rule id of {WITHHELD_RULE}")
    except Usage as error:
        print(f"release-payload-privacy-scan: {error}", file=sys.stderr)
        return EXIT_USAGE
    work = Path(tempfile.mkdtemp(prefix="payload-scan-", dir=args.work))
    os.chmod(work, 0o700)
    try:
        reroot_dir = work / "reroot"
        reroot_dir.mkdir()
        walker = walker_class(engine)(reroot_dir, args.zstd or shutil.which("zstd"),
                                      args.max_reroots, args.max_reroot_bytes, args.max_nesting, args.piece_bytes)
        payload = [str(path.resolve()) for path in args.payload]
        for index, path in enumerate(payload):
            walker.walk(index, path, 0)
        walker.failures += ["a member name makes a location ambiguous"] * walker.ambiguous_names()
        engine_report = work / "engine-report.json"
        engine_rules = args.rules
        if withheld:
            # The same engine run looks for the listed digests, in either case, at every depth it opens:
            # one more category beside the given rules, in a private copy of them.
            engine_rules = work / "rules-with-references.json"
            rules_raw_engine = json.dumps({**rules_doc, "categories": {**rules_doc["categories"], WITHHELD_CATEGORY: {
                "rules": [{"id": WITHHELD_RULE, "kind": "regex", "ignore_case": True,
                           "value": "(?:" + "|".join(withheld) + ")", "prefilter": withheld}]}}}).encode()
            descriptor = os.open(engine_rules, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
            with os.fdopen(descriptor, "wb") as stream:
                stream.write(rules_raw_engine)
        else:
            rules_raw_engine = rules_raw
        command = [sys.executable, "-I", str(ENGINE), "--rules", str(engine_rules), "--exemptions", str(args.exemptions),
                   "--expect-scanner-sha256", ENGINE_SHA256,
                   "--expect-rules-sha256", hashlib.sha256(rules_raw_engine).hexdigest(),
                   "--expect-exemptions-sha256", hashlib.sha256(exemptions_raw).hexdigest(),
                   "--report", str(engine_report),
                   "--detail", str(args.detail), *payload, *walker.artifacts]
        engine_run = subprocess.run(command, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                                    stderr=subprocess.PIPE, check=False)
        if args.detail.exists():
            os.chmod(args.detail, 0o600)
        if engine_run.returncode == EXIT_USAGE or not engine_report.is_file() or not args.detail.is_file():
            sys.stderr.write(engine_run.stderr.decode("utf-8", "replace"))
            print("release-payload-privacy-scan: the engine refused its arguments or inputs", file=sys.stderr)
            return EXIT_USAGE
        value = json.loads(engine_report.read_text(encoding="utf-8"))
        detail = json.loads(args.detail.read_text(encoding="utf-8"))
        counts = classify(walker, detail, accepted, exceptions, engine.Scanner(scan_rules).matches_any, accepted_entries)
        if args.refused is not None:
            descriptor = os.open(args.refused, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
            with os.fdopen(descriptor, "w", encoding="utf-8") as stream:
                stream.write(json.dumps(counts["refused"], indent=1, sort_keys=True) + "\n")
        refused_hits = sum(counts["hits"]["refused"].values())
        refused_errors = sum(counts["errors"]["refused"].values())
        # The engine read every file again after the walk: both must have read the same bytes. The report
        # names the bytes the walk read.
        scanned = {row["index"]: row for row in value["artifacts"]}
        rows, changed = [], 0
        for index, path in enumerate([*payload, *walker.artifacts]):
            walked, read = walker.digests.get(path), scanned.get(index, {}).get("sha256")
            if walked is not None and walked != read:
                if index < len(payload):
                    changed += 1
                else:
                    walker.failures.append("a re-rooted file changed during the scan")
            if index < len(payload):
                rows.append({"name": args.payload[index].name, "sha256": walked or read,
                             "bytes": walker.sizes.get(path, scanned.get(index, {}).get("size"))})
        blocking = [category for category in scan_rules.categories if not category.startswith(REVIEW_PREFIX)]
        blocking_rules = sum(1 for row in scan_rules.compiled if row[0] in blocking)
        # Review rules that are not the converter's path category: rules of a name class that refuse nothing.
        review_paths = HOME_PATH_CATEGORIES[1] if rules_doc["categories"].get(HOME_PATH_CATEGORIES[1]) == REVIEW_PATH_CATEGORY else None
        review_outside_paths = sum(1 for row in scan_rules.compiled if row[0] not in blocking and row[0] != review_paths)
        blocking_exemptions = sum(1 for entry in exemptions_doc["exemptions"] if entry["category"] in blocking)
        reasons = [name for name, failed in (
            ("hits-refused", refused_hits),
            (WITHHELD_RULE, counts["hits"]["refused"].get(WITHHELD_CATEGORY)),
            ("errors-refused", refused_errors),
            ("wrapper-failures", walker.failures),
            ("payload-changed-during-scan", changed),
            ("no-blocking-rules", not blocking_rules),
            ("home-paths-off", not any(name in scan_rules.categories for name in HOME_PATH_CATEGORIES)),
            ("exemption-in-blocking-category", blocking_exemptions)) if failed]
        code = (EXIT_HITS if refused_hits else 0) | (
            EXIT_COVERAGE if any(name not in ("hits-refused", WITHHELD_RULE) for name in reasons) else 0)
        reroot_reasons = [reason for reason, _ in walker.reroots.values()]
        report = {
            "schema": SCHEMA_REPORT,
            "status": REFUSED if reasons else PASS,
            "reasons": reasons,
            "exit_code": code,
            "release": not args.proof_only,
            "label": label,
            "generated_at": datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
            # 128 random bits: the sha256 of this report, which an upload receipt carries, confirms no
            # guess of the other fields.
            "nonce": secrets.token_hex(16),
            "tool_commit": commit,
            "tool_sha256": sha256_file(Path(__file__).resolve()),
            "engine": {"sha256": ENGINE_SHA256, "exit_code": engine_run.returncode},
            "rules": {"sha256": hashlib.sha256(rules_raw).hexdigest(), "total": scan_rules.rule_count,
                      "blocking": blocking_rules, "review_outside_paths": review_outside_paths},
            "name_list_sha256": name_list,
            "non_published_list_sha256": withheld_sha256,
            "exemptions": {"sha256": hashlib.sha256(exemptions_raw).hexdigest(), "count": scan_rules.exemption_count,
                           "blocking": blocking_exemptions},
            "accepted": {"sha256": accepted_sha256, "count": accepted_count, "used": counts["bindings"],
                         "used_unproved": counts["proofs"]["unproved"], "used_by_tool": counts["proofs"]["tools"]},
            "payload": rows,
            "reroots": {"depth": reroot_reasons.count("depth"), "zstd": reroot_reasons.count("zstd"),
                        "split": reroot_reasons.count("split"), "zlib": reroot_reasons.count("zlib"),
                        "artifacts": len(walker.artifacts),
                        "bytes": walker.reroot_bytes},
            "wrapper_failures": len(walker.failures),
            "containers": value["containers"],
            "coverage": {key: value["coverage"][key] for key in ("files", "members", "bytes")},
            "hits": counts["hits"],
            "review_hits": counts["review_hits"],
            "coverage_exceptions": {"sha256": exceptions_sha256, "count": len(exceptions),
                                    "used": counts["exceptions_used"]},
            "errors": counts["errors"],
        }
        with open(args.report, "x", encoding="utf-8") as stream:
            stream.write(json.dumps(report, indent=2, sort_keys=True) + "\n")
        if args.show_names:
            for row in counts["refused"]["hits"]:
                if row["category"] == WITHHELD_CATEGORY:
                    print(f"{WITHHELD_RULE}\t{row['location'] if row['location'] is not None else '(path withheld)'}",
                          file=sys.stderr)
        print(f"release-payload-privacy-scan: {report['status']}"
              f"{' (' + ', '.join(reasons) + ')' if reasons else ''}: hits refused {refused_hits}, accepted "
              f"{sum(counts['hits']['accepted'].values())}, review {sum(counts['review_hits'].values())}; "
              f"errors refused {refused_errors}, accepted "
              f"{sum(counts['errors']['accepted'].values())}, excepted {sum(counts['exceptions_used'].values())}, "
              f"covered {sum(counts['errors']['covered'].values())}; "
              f"wrapper failures {len(walker.failures)}; re-rooted {len(walker.artifacts)}", file=sys.stderr)
        return code
    finally:
        if not args.keep_work:
            shutil.rmtree(work, ignore_errors=True)


if __name__ == "__main__":
    sys.exit(main())
