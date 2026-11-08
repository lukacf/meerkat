#!/usr/bin/env python3
"""Refuse private text in public release evidence and package metadata before upload.

One scanner for every producer (Linux and macOS): the same scanner, rules and exemptions bytes,
bound into each report by sha256.

  python3 -I release-evidence-privacy-scan.py \\
      --rules release-evidence-privacy-rules.json \\
      --exemptions release-evidence-privacy-exemptions.json \\
      --report <public report.json> --detail <private detail.json> \\
      [--require-json PATTERN ...] \\
      [--expect-scanner-sha256 HEX] [--expect-rules-sha256 HEX] [--expect-exemptions-sha256 HEX] \\
      ARTIFACT [ARTIFACT ...]

ARTIFACT is a file, a directory (walked; a symbolic link inside it is a scan error, never followed),
a tar archive (plain, or gzip, bzip2 or xz compressed: .tar.gz, .tgz, .crate, sdists) or a zip archive
(.whl, .zip, also with data before it, as in self-extracting files). Archives and compressed streams
are recognized by their bytes and scanned member by member, nested ones too (4 levels), and every
byte is accounted for, fail-closed: what a format's structure does not account for is a scan error.
Scanned: data before or after a zip, every concatenated tar archive and gzip, bzip2 or xz stream, data
after a tar's end or after the last compressed stream, non-zero tar block and extension padding, every
gzip header (name, comment, extra field), zip names, comments and extra fields, tar names, links,
owners and pax records. Validated: each zip local header against its central record, data descriptors,
zip entry data (decoded here, so bytes after a compressed stream are found), every tar header block
(fields end in NUL padding, numbers are octal, unused areas are zero) and every pax or GNU long-name
record. Every raw tar header block and extension body, and the raw name of every zip local and
central record, is also scanned as text in occurrence order, before any parser override or cut, as well
as the final parsed fields. Refused: bytes between zip entries, a zip directory entry that holds data, a
zip entry name that holds a NUL byte, ZIP64, encryption,
unknown zip flags and methods, unknown tar types, base-256 tar numbers, tar members without data that
declare a size, reserved gzip flags, and zstd, 7z, xar, rar, lz4, lzip, rpm, deb, dmg and Unix compress
containers. A refused region that still holds readable bytes is scanned too. Nothing is extracted to
disk. Every container gets a typed outcome: decoded (every byte accounted for), refused (a scan error
says why) or unsupported (also a scan error). COVERAGE.md maps every region to its treatment and test.

What is scanned: every file and member name, symlink and hard-link target, tar user and group name
and pax header, zip comment and extra field, and every file and member content. Content that parses
as JSON (or as JSON lines) is decoded and scanned key by key and value by value (duplicate keys
included), so escaped slashes and \\uXXXX escapes in keys and values cannot hide anything; other
content is scanned as text. Every scanned string is also scanned in its decoded forms: JSON-style
escapes undone and percent-encoding undone, each repeated, two rounds deep at any size. Binary
content is scanned as UTF-8 text with replacement characters.

Declared JSON: a top-level file artifact named *.json, *.jsonl or *.ndjson (also before a .gz, .bz2
or .xz suffix), and every file or member whose path or base name matches a --require-json pattern
(fnmatch, case-sensitive), must parse as UTF-8 JSON: one document, or one per non-empty line for a
.jsonl or .ndjson name, after any gzip, bzip2 or xz layers. A declared JSON that is empty, truncated,
malformed, not UTF-8, nested beyond the parser's limit, or a zip, tar or other container is a scan
error, and its content is still scanned. Other content is scanned as JSON when it parses and as text
when it does not.

Outputs:
  --report  PUBLIC-SAFE JSON (schema release-evidence-privacy-scan/report/2): per-category hit counts,
            coverage (files, members, bytes, strings), typed container outcomes, scan errors by kind,
            the sha256 of the scanner, rules and exemptions, and each artifact's binding: its path (the
            normalized argument when it is relative and does not start with "..", otherwise its base
            name) and sha256 (for a directory, the tree sha256). It never holds a matched string: a
            path that matches a rule is shortened to its base name, a base name that matches (a hit) is
            replaced by "artifact-<index>", and the serialized report is scanned before it is written.
  --detail  PRIVATE JSON (mode 0600, schema release-evidence-privacy-scan/detail/2): every match with
            its category, rule, location and decoded form,
            every container outcome and every scan error with its message. Never upload it.
Before anything is written or removed, each output path is checked against every input: the
artifacts, the files inside scanned directories, the rules and exemptions files and this scanner. The
same path, a symbolic link to an input, a hard link of an input, or a path inside a scanned directory
is a usage error. Outputs are written to new, uniquely named staging files next to them and then
renamed into place; once the paths pass, an existing file at either path is removed first, so a
failed run leaves neither behind. Inputs are only ever read.

Exit status (bit flags): 0 clean; 1 a hit; 2 a scan error (coverage incomplete: a missing,
unreadable, corrupt or unsupported input, a refused container structure, a malformed declared JSON, a
symbolic link inside a directory, a member over 1 GiB, or containers nested more than 4 deep); 3 both;
4 a usage or configuration error (bad arguments, rules or exemptions, an output that aliases an input,
a --expect-*-sha256 mismatch: nothing is scanned), a report that would itself match a rule, or an
internal error. No report is written on exit 4.

Rules (private JSON, schema release-evidence-privacy-scan/rules/1): categories, each a list of rules
{id, kind: literal | token | regex, value, ignore_case, prefilter?}. A literal matches anywhere; a token
only as a whole token (not next to [A-Za-z0-9_-]); a regex is a Python regular expression, run only
when one of its prefilter strings (if given) occurs in the case-folded text, so each of its matches
must contain one. Overlapping matches of one category count once. Exemptions (schema
release-evidence-privacy-scan/exemptions/1): {category, match, reason}; a match whose exact text equals
an exempt match of its category is not a hit.
"""

from __future__ import annotations

import argparse
import bz2
import fnmatch
import hashlib
import io
import json
import lzma
import os
import platform
import re
import stat
import struct
import sys
import tarfile
import tempfile
import traceback
import urllib.parse
import zipfile
import zlib
from datetime import datetime, timezone

SCANNER_NAME = "release-evidence-privacy-scan.py"
SCHEMA_REPORT = "release-evidence-privacy-scan/report/2"
SCHEMA_DETAIL = "release-evidence-privacy-scan/detail/2"
SCHEMA_RULES = "release-evidence-privacy-scan/rules/1"
SCHEMA_EXEMPTIONS = "release-evidence-privacy-scan/exemptions/1"
EXIT_HITS, EXIT_ERRORS, EXIT_USAGE = 1, 2, 4
MAX_MEMBER_BYTES = 1 << 30
MAX_DEPTH = 4
DECODE_ROUNDS = 2
ID = re.compile(r"^[a-z][a-z0-9_]{0,63}$")
RULE_ID = re.compile(r"^[A-Za-z0-9][A-Za-z0-9_.-]{0,95}$")
TOKEN_BEFORE, TOKEN_AFTER = r"(?<![A-Za-z0-9_-])", r"(?![A-Za-z0-9_-])"
JSON_ESCAPE = re.compile(r'\\(?:u([0-9a-fA-F]{4})|(["\\/bfnrt]))')
SIMPLE_ESCAPES = {'"': '"', "\\": "\\", "/": "/", "b": "\b", "f": "\f", "n": "\n", "r": "\r", "t": "\t"}
SURROGATE = re.compile("[\ud800-\udfff]")
JSON_NAMES = (".json", ".jsonl", ".ndjson")
JSON_LINES_NAMES = (".jsonl", ".ndjson")
COMPRESSION_SUFFIXES = (".gz", ".bz2", ".xz")
SIMPLE_KEY = re.compile(r"^[A-Za-z0-9_]+$")
NONZERO = re.compile(rb"[^\x00]")
RESULTS = {0: "clean", EXIT_HITS: "hits", EXIT_ERRORS: "errors", EXIT_HITS | EXIT_ERRORS: "hits-and-errors"}
OUTCOMES = ("decoded", "refused", "unsupported")
TAR_BLOCK = 512
ZIP_MAGIC = (b"PK\x03\x04", b"PK\x05\x06", b"PK\x07\x08")
ZIP_LOCAL, ZIP_END, ZIP64_END, ZIP64_LOCATOR = b"PK\x03\x04", b"PK\x05\x06", b"PK\x06\x06", b"PK\x06\x07"
ZIP_END_SEARCH = 22 + 0xFFFF
ZIP_DESCRIPTOR_SIZES = (12, 16, 20, 24)
STREAM_MAGIC = {"gzip": b"\x1f\x8b", "bzip2": b"BZh", "xz": b"\xfd7zXZ\x00"}
# Containers whose bytes cannot be read as text and that this scanner does not unpack.
OPAQUE_MAGIC = (
    (b"\x28\xb5\x2f\xfd", "zstd"), (b"7z\xbc\xaf\x27\x1c", "7z"), (b"xar!", "xar"), (b"Rar!\x1a\x07", "rar"),
    (b"\x04\x22\x4d\x18", "lz4"), (b"LZIP", "lzip"), (b"\xed\xab\xee\xdb", "rpm"), (b"!<arch>\ndebian-binary", "deb"),
    (b"\x1f\x9d", "compress"),
)
ZIP_DESCRIPTOR = b"PK\x07\x08"
ZIP_ALLOWED_FLAGS = 0x0002 | 0x0004 | 0x0008 | 0x0800  # deflate options, data descriptor, UTF-8 names
TAR_POSIX_MAGIC, TAR_GNU_MAGIC = b"ustar\x0000", b"ustar  \x00"
TAR_V7_STRINGS = ((0, 100), (157, 100))  # name, linkname
TAR_GNU_STRINGS = TAR_V7_STRINGS + ((265, 32), (297, 32))  # uname, gname
TAR_POSIX_STRINGS = TAR_GNU_STRINGS + ((345, 155),)  # prefix
TAR_V7_NUMBERS = ((100, 8), (108, 8), (116, 8), (124, 12), (136, 12), (148, 8))  # mode uid gid size mtime chksum
TAR_NUMBERS = TAR_V7_NUMBERS + ((329, 8), (337, 8))  # devmajor devminor
TAR_NUMBER = re.compile(rb"[ \x00]*[0-7]*[ \x00]*\Z")
TAR_PAX_RECORD = re.compile(rb"([0-9]+) ")
TAR_FIELDS = ((0, 100), (100, 8), (108, 8), (116, 8), (124, 12), (136, 12), (148, 8), (156, 1), (157, 100),
              (257, 6), (263, 2), (265, 32), (297, 32), (329, 8), (337, 8), (345, 155), (500, 12))
TAR_MEMBER_TYPES = (b"0", b"\x00", b"1", b"2", b"3", b"4", b"5", b"6", b"7")
TAR_EXTENSION_TYPES = (b"x", b"g", b"L", b"K")


class ConfigError(Exception):
    """A usage or configuration error: exit 4 before anything is scanned."""


class Refusal(Exception):
    """A container form this scanner does not accept: a typed scan error (error kind, message)."""

    def __init__(self, kind: str, message: str):
        super().__init__(message)
        self.kind = kind


class Pairs(list):
    """A JSON object kept as its (key, value) pairs, duplicate keys included."""


def sha256_bytes(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def sha256_file(path: str) -> str:
    digest = hashlib.sha256()
    with open(path, "rb") as handle:
        for block in iter(lambda: handle.read(1 << 20), b""):
            digest.update(block)
    return digest.hexdigest()


# ---------------------------------------------------------------------------
# Rules and exemptions.


class Rules:
    def __init__(self, rules_doc: object, exemptions_doc: object):
        if not isinstance(rules_doc, dict) or rules_doc.get("schema") != SCHEMA_RULES:
            raise ConfigError(f"the rules file is not a {SCHEMA_RULES} document")
        categories = rules_doc.get("categories")
        if not isinstance(categories, dict) or not categories:
            raise ConfigError("the rules file has no categories")
        self.rules_id = str(rules_doc.get("rules_id", ""))
        self.categories: list[str] = []
        # (category, rule id, pattern, needles, folded): a rule runs its regex only when one of its needles
        # occurs in the text (in the case-folded text when folded). A literal or token rule's needle is its
        # value; a regex rule's needles are its prefilter, compared case-folded (every match must contain
        # one); a regex rule without a prefilter always runs.
        self.compiled: list[tuple[str, str, re.Pattern, tuple[str, ...] | None, bool]] = []
        seen_ids = set()
        for category, body in categories.items():
            if not ID.match(category):
                raise ConfigError(f"category id {category!r} is not [a-z][a-z0-9_]*")
            rules = body.get("rules") if isinstance(body, dict) else None
            if not isinstance(rules, list) or not rules:
                raise ConfigError(f"category {category} has no rules")
            self.categories.append(category)
            for rule in rules:
                if not isinstance(rule, dict):
                    raise ConfigError(f"a rule of {category} is not an object")
                rule_id, kind, value = rule.get("id"), rule.get("kind"), rule.get("value")
                if not isinstance(rule_id, str) or not RULE_ID.match(rule_id) or rule_id in seen_ids:
                    raise ConfigError(f"a rule of {category} has a missing, malformed or repeated id")
                seen_ids.add(rule_id)
                if kind not in ("literal", "token", "regex") or not isinstance(value, str) or not value:
                    raise ConfigError(f"rule {rule_id} needs kind literal|token|regex and a value")
                ignore_case = rule.get("ignore_case", False)
                if not isinstance(ignore_case, bool):
                    raise ConfigError(f"rule {rule_id}: ignore_case must be true or false")
                if kind == "literal":
                    source = re.escape(value)
                elif kind == "token":
                    source = TOKEN_BEFORE + re.escape(value) + TOKEN_AFTER
                else:
                    source = value
                try:
                    pattern = re.compile(source, re.IGNORECASE if ignore_case else 0)
                except re.error as error:
                    raise ConfigError(f"rule {rule_id}: {error}") from None
                if pattern.search(""):
                    raise ConfigError(f"rule {rule_id} matches the empty string")
                prefilter = rule.get("prefilter")
                if prefilter is not None and (not isinstance(prefilter, list) or not prefilter or not all(
                        isinstance(item, str) and item for item in prefilter)):
                    raise ConfigError(f"rule {rule_id}: prefilter must be a list of non-empty strings")
                if kind != "regex":
                    needles = (value.casefold(),) if ignore_case else (value,)
                    folded_needles = ignore_case
                elif prefilter is not None:
                    needles = tuple(item.casefold() for item in prefilter)
                    folded_needles = True
                else:
                    needles, folded_needles = None, False
                self.compiled.append((category, rule_id, pattern, needles, folded_needles))
        self.rule_count = len(self.compiled)
        if not isinstance(exemptions_doc, dict) or exemptions_doc.get("schema") != SCHEMA_EXEMPTIONS:
            raise ConfigError(f"the exemptions file is not a {SCHEMA_EXEMPTIONS} document")
        entries = exemptions_doc.get("exemptions")
        if not isinstance(entries, list):
            raise ConfigError("the exemptions file has no exemptions list")
        self.exemptions_id = str(exemptions_doc.get("exemptions_id", ""))
        self.exempt: dict[str, set[str]] = {category: set() for category in self.categories}
        for entry in entries:
            if not isinstance(entry, dict):
                raise ConfigError("an exemption is not an object")
            category, match, reason = entry.get("category"), entry.get("match"), entry.get("reason")
            if category not in self.exempt:
                raise ConfigError(f"an exemption names the unknown category {category!r}")
            if not isinstance(match, str) or not match:
                raise ConfigError(f"an exemption of {category} has no match")
            if not isinstance(reason, str) or not reason.strip():
                raise ConfigError(f"an exemption of {category} has no reason")
            self.exempt[category].add(match)
        self.exemption_count = len(entries)

    def find(self, text: str) -> dict[str, list[tuple[int, int, list[str]]]]:
        """Per category, the merged spans of its non-exempt matches, each with the rule ids behind it."""
        spans: dict[str, list[tuple[int, int, str]]] = {}
        folded = None
        for category, rule_id, pattern, needles, folded_needles in self.compiled:
            if needles is not None:
                if folded_needles:
                    if folded is None:
                        folded = text.casefold()
                    if not any(needle in folded for needle in needles):
                        continue
                elif not any(needle in text for needle in needles):
                    continue
            exempt = self.exempt[category]
            for match in pattern.finditer(text):
                if match.end() > match.start() and match.group(0) not in exempt:
                    spans.setdefault(category, []).append((match.start(), match.end(), rule_id))
        merged: dict[str, list[tuple[int, int, list[str]]]] = {}
        for category, items in spans.items():
            items.sort()
            out: list[tuple[int, int, list[str]]] = []
            for start, end, rule_id in items:
                if out and start < out[-1][1]:
                    last = out[-1]
                    rules = last[2] if rule_id in last[2] else last[2] + [rule_id]
                    out[-1] = (last[0], max(last[1], end), rules)
                else:
                    out.append((start, end, [rule_id]))
            merged[category] = out
        return merged


# ---------------------------------------------------------------------------
# Decoded forms.


def json_unescape(text: str) -> str:
    if "\\" not in text:
        return text

    def replace(match: re.Match) -> str:
        if match.group(1) is not None:
            return chr(int(match.group(1), 16))
        return SIMPLE_ESCAPES[match.group(2)]

    out = JSON_ESCAPE.sub(replace, text)
    if SURROGATE.search(out):
        try:
            # Join the UTF-16 surrogate pairs that \u escapes produced.
            out = out.encode("utf-16-le", "surrogatepass").decode("utf-16-le")
        except UnicodeError:
            pass
    return out


def percent_decode(text: str) -> str:
    if "%" not in text:
        return text
    return urllib.parse.unquote(text, errors="replace")


def text_digest(text: str) -> bytes:
    """A sha1 of the text, encoded slice by slice so a large text is never copied whole."""
    digest = hashlib.sha1()
    for start in range(0, len(text), 1 << 20):
        digest.update(text[start:start + (1 << 20)].encode("utf-8", "surrogatepass"))
    return digest.digest()


def decoded_forms(text: str):
    """Yield (form, text): the raw text, then each new text that JSON or percent decoding produces,
    DECODE_ROUNDS rounds deep whatever the size of the text."""
    seen = {text_digest(text)}
    yield "raw", text
    frontier = [("raw", text)]
    for _ in range(DECODE_ROUNDS):
        following = []
        for name, value in frontier:
            for label, decoder in (("json", json_unescape), ("percent", percent_decode)):
                out = decoder(value)
                if out is value:
                    continue
                key = text_digest(out)
                if key in seen:
                    continue
                seen.add(key)
                form = label if name == "raw" else f"{name}+{label}"
                yield form, out
                following.append((form, out))
        frontier = following


def parse_json(text: str) -> tuple[bool, object, str | None]:
    try:
        return True, json.loads(text, object_pairs_hook=Pairs), None
    except ValueError as error:
        return False, None, str(error)
    except RecursionError:
        return False, None, "nested deeper than the JSON parser accepts"


# ---------------------------------------------------------------------------
# The scan.


class Scanner:
    def __init__(self, rules: Rules, require_json: list[str] | None = None):
        self.rules = rules
        self.require_json = list(require_json or [])
        self.hits = {category: 0 for category in rules.categories}
        self.coverage = {
            "artifacts": 0, "files": 0, "directories": 0, "archives": 0, "members": 0, "bytes": 0,
            "binary_contents": 0, "json_documents": 0, "json_strings": 0, "text_units": 0,
            "names": 0, "symlinks": 0,
        }
        self.containers: dict[str, dict[str, int]] = {}
        self.errors: dict[tuple[int, str], int] = {}
        self.detail_hits: list[dict] = []
        self.detail_errors: list[dict] = []
        self.detail_containers: list[dict] = []
        self.index = 0

    # ---- recording

    def error(self, kind: str, location: str, message: str) -> None:
        key = (self.index, kind)
        self.errors[key] = self.errors.get(key, 0) + 1
        self.detail_errors.append({"artifact": self.index, "kind": kind, "location": location, "message": message})

    def container(self, location: str, kind: str, outcome: str, reason: str | None = None) -> None:
        """One container's typed outcome: decoded, refused or unsupported."""
        counts = self.containers.setdefault(kind, {name: 0 for name in OUTCOMES})
        counts[outcome] += 1
        self.detail_containers.append({"artifact": self.index, "location": location, "kind": kind,
                                       "outcome": outcome, "reason": reason})

    def scan_string(self, text: str, location: str, where: str) -> int:
        """Scan one string in every decoded form; returns its hit count."""
        self.coverage["text_units"] += 1
        best: dict[tuple[str, str], tuple[int, str, int, list[str]]] = {}
        for form, value in decoded_forms(text):
            for category, spans in self.rules.find(value).items():
                counts: dict[str, list] = {}
                for start, end, rule_ids in spans:
                    matched = value[start:end]
                    row = counts.setdefault(matched, [0, start, rule_ids])
                    row[0] += 1
                for matched, (count, start, rule_ids) in counts.items():
                    previous = best.get((category, matched))
                    if previous is None or count > previous[0]:
                        line = value.count("\n", 0, start) + 1
                        best[(category, matched)] = (count, form, line, rule_ids)
        total = 0
        for (category, matched), (count, form, line, rule_ids) in sorted(best.items()):
            self.hits[category] += count
            total += count
            self.detail_hits.append({
                "artifact": self.index, "location": location, "where": where, "category": category,
                "rules": rule_ids, "form": form, "line": line, "count": count, "match": matched,
            })
        return total

    def scan_name(self, name: str, location: str, kind: str = "name") -> int:
        self.coverage["names"] += 1
        return self.scan_string(name, location, kind)

    def declared(self, path: str, name: str) -> bool:
        """Whether a --require-json pattern declares this file or member JSON."""
        return any(fnmatch.fnmatchcase(path, pattern) or fnmatch.fnmatchcase(name, pattern)
                   for pattern in self.require_json)

    # ---- content

    def scan_json(self, value: object, location: str, path: str) -> None:
        """Every key and string value of a parsed document (iteratively: no recursion limit)."""
        stack = [(value, path)]
        while stack:
            node, node_path = stack.pop()
            if isinstance(node, Pairs):
                children = []
                for key, child in node:
                    self.coverage["json_strings"] += 1
                    self.scan_string(key, location, f"{node_path}<key>")
                    if SIMPLE_KEY.match(key):
                        children.append((child, f"{node_path}.{key}"))
                    else:
                        children.append((child, f"{node_path}[{json.dumps(key)}]"))
                stack.extend(reversed(children))
            elif isinstance(node, list):
                stack.extend(reversed([(child, f"{node_path}[{index}]") for index, child in enumerate(node)]))
            elif isinstance(node, str):
                self.coverage["json_strings"] += 1
                self.scan_string(node, location, node_path)

    def scan_content(self, data: bytes, name: str, location: str, depth: int, required: bool = False) -> None:
        """One file or member's bytes: a container, JSON or text. required: a declared JSON."""
        self.coverage["bytes"] += len(data)
        kind = archive_kind(data)
        if required and kind is not None and kind not in STREAM_MAGIC:
            # A zip, a tar or an unsupported container never satisfies a JSON declaration; it is still scanned.
            self.error("malformed-json", location, f"declared JSON is a {kind.replace('opaque-', '')} container")
            required = False
        if kind is not None and kind.startswith("opaque-"):
            self.container(location, kind[7:], "unsupported", "a container this scanner does not unpack")
            self.error("unsupported-archive", location, f"{kind[7:]} container: not unpacked, not scanned")
            return
        if kind is not None:
            if depth >= MAX_DEPTH:
                self.container(location, kind, "refused", f"nested deeper than {MAX_DEPTH}")
                self.error("depth-limit", location, f"archive nested deeper than {MAX_DEPTH}")
                return
            self.scan_archive(kind, data, name, location, depth + 1, required)
            return
        if data[:4096].find(b"\x00") >= 0 and not data.startswith((b"\xff\xfe", b"\xfe\xff")):
            self.coverage["binary_contents"] += 1
        text = decode_text(data)
        if self.try_json(data, text, name, location, required):
            return
        self.scan_string(text, location, "text")

    def try_json(self, data: bytes, text: str, name: str, location: str, required: bool) -> bool:
        """Scan the content as JSON when it parses, and say whether it did. A declared JSON that does not
        parse is a scan error; the caller then scans its raw text."""
        lowered = name.lower()
        for suffix in COMPRESSION_SUFFIXES:
            if lowered.endswith(suffix):
                lowered = lowered[:-len(suffix)]
                break
        lines_only = lowered.endswith(JSON_LINES_NAMES)
        body = text[1:] if text.startswith("\ufeff") else text
        stripped = body.lstrip(" \t\r\n")
        if required:
            if not strict_utf8(data):
                self.error("malformed-json", location, "declared JSON is not UTF-8")
                return False
            if not stripped.strip():
                self.error("malformed-json", location, "declared JSON is empty")
                return False
        elif not stripped or (stripped[0] not in "{[" and not lowered.endswith(JSON_NAMES)):
            return False
        if not lines_only:
            ok, value, problem = parse_json(stripped)
            if ok:
                self.coverage["json_documents"] += 1
                self.scan_json(value, location, "$")
                return True
            if required:
                self.error("malformed-json", location, f"declared JSON does not parse: {problem}")
                return False
        parsed = []
        for number, line in enumerate(body.split("\n"), 1):
            if not line.strip():
                continue
            ok, value, problem = parse_json(line)
            if not ok:
                if required:
                    self.error("malformed-json", location, f"declared JSON line {number} does not parse: {problem}")
                return False
            parsed.append((number, value))
        if not parsed:
            return False
        self.coverage["json_documents"] += len(parsed)
        for number, value in parsed:
            self.scan_json(value, location, f"line {number}: $")
        return True

    # ---- containers

    def scan_archive(self, kind: str, data: bytes, name: str, location: str, depth: int, required: bool) -> None:
        self.coverage["archives"] += 1
        if kind == "zip":
            self.scan_zip(data, location, depth)
        elif kind == "tar":
            self.scan_tar_data(data, location, depth, probe=False)
        else:
            self.scan_compressed(kind, data, name, location, depth, required)

    def scan_extra(self, data: bytes, location: str, where: str, depth: int) -> None:
        """Bytes outside every member and record of a container: scanned as content of their own."""
        self.coverage["members"] += 1
        self.scan_content(data, f"<{where}>", f"{location}!<{where}>", depth)

    def scan_compressed(self, kind: str, data: bytes, name: str, location: str, depth: int, required: bool) -> None:
        """Every concatenated gzip, bzip2 or xz stream, then what they hold: a tar or other content."""
        try:
            inner, headers, rest = decompress_streams(kind, data)
        except Refusal as refusal:
            self.container(location, kind, "refused", str(refusal))
            self.error(refusal.kind, location, f"{kind}: {refusal}")
            return
        except OverflowError:
            self.container(location, kind, "refused", f"decompresses past {MAX_MEMBER_BYTES} bytes")
            self.error("member-too-large", location, f"{kind} stream decompresses past {MAX_MEMBER_BYTES} bytes")
            return
        except (OSError, EOFError, ValueError, zlib.error, lzma.LZMAError) as error:
            self.container(location, kind, "refused", f"damaged or truncated stream: {error}")
            self.error("corrupt-archive", location, f"{kind}: {error}")
            return
        for header in headers:
            self.container(location, kind, "decoded")
            if header:  # a gzip header: name, comment, extra field and the fixed fields
                self.scan_name(decode_text(header), location, f"{kind} header")
        if rest:
            self.scan_extra(rest, location, f"data after the {kind} stream", depth)
        if self.scan_tar_data(inner, location, depth, probe=True):
            if required:
                self.error("malformed-json", location, f"declared JSON decodes to a tar archive")
            return
        inner_name = name
        for suffix in COMPRESSION_SUFFIXES:
            if inner_name.lower().endswith(suffix):
                inner_name = inner_name[:-len(suffix)]
                break
        self.coverage["members"] += 1
        self.scan_content(inner, inner_name, f"{location}!<{kind}-decompressed>", depth, required)

    def scan_tar_data(self, data: bytes, location: str, depth: int, probe: bool) -> bool:
        """Every tar archive concatenated in data, then whatever follows them. With probe, return False
        (nothing recorded) when data does not start with a tar header."""
        stream = io.BytesIO(data)
        offset, part = 0, 0
        while True:
            here = location if part == 0 else f"{location}!<tar part {part + 1}>"
            if part > 0:
                # tarfile reads an invalid header after offset 0 as the end of an empty archive.
                try:
                    tarfile.TarInfo.frombuf(data[offset:offset + TAR_BLOCK], tarfile.ENCODING, "surrogateescape")
                except (tarfile.HeaderError, ValueError):
                    self.scan_extra(data[offset:], location, "data after the tar", depth)
                    return True
            stream.seek(offset)
            try:
                archive = tarfile.open(fileobj=stream, mode="r:")
            except (tarfile.TarError, EOFError, OSError, ValueError) as error:
                if part == 0 and probe:
                    return False
                if part == 0:
                    self.container(here, "tar", "refused", f"not a readable tar: {error}")
                    self.error("corrupt-archive", here, f"tar: {error}")
                    self.scan_string(decode_text(data), here, "text")  # refused, and still read as raw text
                    return True
                self.scan_extra(data[offset:], location, "data after the tar", depth)
                return True
            with archive:
                end, problems = self.scan_tar(archive, data, offset, here, depth)
            for kind, problem in problems:
                self.error(kind, here, problem)
            if end is None or problems:
                self.container(here, "tar", "refused", problems[0][1] if problems else "a member could not be read")
                if end is None:
                    return True
            else:
                self.container(here, "tar", "decoded")
            following = NONZERO.search(data, end)
            if following is None:
                return True  # only zero padding follows
            start = following.start()
            if start % TAR_BLOCK != 0 or start <= offset:
                self.scan_extra(data[start:], location, "data after the tar", depth)
                return True
            offset, part = start, part + 1

    def tar_blocks(self, data: bytes, start: int, stop: int, location: str, depth: int, member: bool) -> list:
        """Scan and validate the tar header blocks in data[start:stop]: extension headers (pax, GNU long
        names and links, global pax), then, with member, the member's own header as the last block. Every
        raw header block and every extension body is scanned as text in occurrence order, before the tar
        parser applies any override, so a value that a later record or field replaces still counts. Non-zero
        padding after an extension's data is scanned. Returns [(error kind, problem)]."""
        problems: list = []
        position = start
        while position < stop:
            block = data[position:position + TAR_BLOCK]
            if len(block) < TAR_BLOCK:
                return problems + [("corrupt-archive", "a truncated tar header")]
            self.scan_string(tar_header_text(block), f"{location}!<tar header at {position}>", "tar header block")
            problems += tar_header_problems(block)
            typeflag = block[156:157]
            if typeflag in TAR_EXTENSION_TYPES:
                try:
                    size = tarfile.nti(block[124:136])
                except (tarfile.HeaderError, ValueError):
                    return problems + [("archive-layout", "a tar extension header with an invalid size")]
                stored = -(-size // TAR_BLOCK) * TAR_BLOCK
                body = data[position + TAR_BLOCK:position + TAR_BLOCK + size]
                self.scan_string(decode_text(body), f"{location}!<tar extension data at {position}>",
                                 "tar extension data")
                problems += tar_extension_problems(typeflag, body)
                padding = data[position + TAR_BLOCK + size:position + TAR_BLOCK + stored]
                if NONZERO.search(padding):
                    self.scan_extra(padding, location, "tar header padding", depth)
                position += TAR_BLOCK + stored
                continue
            if not member:
                return problems + [("archive-layout", "tar header blocks that belong to no member")]
            if position + TAR_BLOCK != stop:
                return problems + [("archive-layout", "unexpected blocks between a tar header and its data")]
            if typeflag not in TAR_MEMBER_TYPES:
                problems.append(("unsupported-archive", f"tar member type {typeflag.decode('latin-1')!r}"))
            return problems
        if member:
            problems.append(("archive-layout", "a tar member without its own header"))
        return problems

    def scan_tar(self, archive: tarfile.TarFile, data: bytes, start: int, location: str,
                 depth: int) -> tuple[int | None, list]:
        """The members of one tar archive starting at start: (where it ended, or None when a member could not
        be read; [(error kind, problem)] for header bytes this scanner does not accept)."""
        problems: list = []
        expected = start
        while True:
            try:
                member = archive.next()
            except (tarfile.TarError, EOFError, OSError, zlib.error, lzma.LZMAError, ValueError) as error:
                self.error("corrupt-archive", location, f"tar member: {error}")
                return None, problems
            if member is None:
                break
            self.coverage["members"] += 1
            where = f"{location}!{member.name}"
            problems += self.tar_blocks(data, expected, member.offset_data, location, depth, True)
            if not member.isreg() and member.type in tarfile.SUPPORTED_TYPES and member.size:
                problems.append(("archive-layout", f"the tar member {member.name!r} has no data but declares a size"))
            data_blocks = -(-member.size // TAR_BLOCK) * TAR_BLOCK
            if member.isreg() or member.type not in tarfile.SUPPORTED_TYPES:
                expected = member.offset_data + data_blocks
            else:
                expected = member.offset_data
            self.scan_name(member.name, where)
            for text, kind in ((member.linkname, "link target"), (member.uname, "tar user"),
                               (member.gname, "tar group")):
                if text:
                    self.scan_name(text, where, kind)
            applied = {"path": member.name, "linkpath": member.linkname, "uname": member.uname,
                       "gname": member.gname}
            for key, value in sorted((member.pax_headers or {}).items()):
                if applied.get(key) == value or archive.pax_headers.get(key) == value:
                    continue  # scanned above as the member field it sets, or once as a global header
                self.scan_name(str(key), where, "pax header key")
                self.scan_name(str(value), where, "pax header value")
            if member.issym() or member.islnk():
                self.coverage["symlinks"] += 1
            if not member.isfile():
                continue
            if member.size > MAX_MEMBER_BYTES:
                self.error("member-too-large", where, f"{member.size} bytes")
                return None, problems
            try:
                handle = archive.extractfile(member)
                content = handle.read() if handle is not None else b""
            except (tarfile.TarError, EOFError, OSError, zlib.error, lzma.LZMAError, ValueError) as error:
                self.error("corrupt-archive", where, f"tar member content: {error}")
                return None, problems
            base = member.name.rsplit("/", 1)[-1]
            self.scan_content(content, member.name, where, depth, self.declared(member.name, base))
            padding = data[member.offset_data + member.size:member.offset_data + data_blocks]
            if NONZERO.search(padding):
                self.scan_extra(padding, where, "block padding", depth)
        problems += self.tar_blocks(data, expected, archive.offset, location, depth, False)
        for key, value in sorted((archive.pax_headers or {}).items()):
            self.scan_name(str(key), location, "pax global header key")
            self.scan_name(str(value), location, "pax global header value")
        return archive.offset, problems

    def scan_zip(self, data: bytes, location: str, depth: int) -> None:
        try:
            archive = zipfile.ZipFile(io.BytesIO(data))
        except (zipfile.BadZipFile, zipfile.LargeZipFile, OSError, ValueError, EOFError) as error:
            self.container(location, "zip", "refused", f"not a readable zip: {error}")
            self.error("corrupt-archive", location, f"zip: {error}")
            self.scan_string(decode_text(data), location, "text")  # refused, and still read as raw text
            return
        refused = None
        with archive:
            prefix_end, suffix_start, problems, entry_problems, local_extras, data_starts, local_names = \
                zip_layout(data, archive)
            if prefix_end > 0:
                self.scan_extra(data[:prefix_end], location, "data before the zip", depth)
            if archive.comment:
                self.scan_name(decode_text(archive.comment), location, "zip comment")
            for info in archive.infolist():
                self.coverage["members"] += 1
                where = f"{location}!{info.filename}"
                self.scan_name(info.filename, where)
                # The raw names of the central record and the local header too: the parsed name stops at
                # a NUL.
                self.scan_name(info.orig_filename, where, "zip central name")
                local_name = local_names.get(info.header_offset)
                if local_name is not None:
                    self.scan_name(local_name.decode("utf-8" if info.flag_bits & 0x800 else "cp437", "replace"),
                                   where, "zip local name")
                if info.comment:
                    self.scan_name(decode_text(info.comment), where, "zip member comment")
                if info.extra:
                    self.scan_name(decode_text(info.extra), where, "zip extra field")
                local_extra = local_extras.get(info.header_offset)
                if local_extra and local_extra != info.extra:
                    self.scan_name(decode_text(local_extra), where, "zip local extra field")
                own = entry_problems.get(info.header_offset, [])
                for kind, problem in own:
                    self.error(kind, where, problem)
                    refused = refused or problem
                if any(kind == "unsupported-archive" for kind, _ in own):
                    continue  # its bytes cannot be decoded
                if info.file_size > MAX_MEMBER_BYTES:
                    self.error("member-too-large", where, f"{info.file_size} bytes")
                    refused = refused or "a member is over the size limit"
                    continue
                leftover = b""
                try:
                    if info.header_offset in data_starts:
                        content, leftover = zip_member_content(data, info, data_starts[info.header_offset])
                    else:  # past a layout problem (already refused): read what zipfile can
                        content = archive.read(info)
                except Refusal as refusal:
                    self.error(refusal.kind, where, f"zip member: {refusal}")
                    refused = refused or str(refusal)
                    continue
                except OverflowError:
                    self.error("member-too-large", where, f"decompresses past {MAX_MEMBER_BYTES} bytes")
                    refused = refused or "a member is over the size limit"
                    continue
                except (NotImplementedError, RuntimeError) as error:  # a compression method or encryption
                    self.error("unsupported-archive", where, f"zip member: {error}")
                    refused = refused or "a member uses an unsupported compression method or encryption"
                    continue
                except (zipfile.BadZipFile, zipfile.LargeZipFile, OSError, zlib.error, lzma.LZMAError,
                        EOFError, ValueError) as error:
                    self.error("corrupt-archive", where, f"zip member: {error}")
                    refused = refused or "a member could not be read"
                    continue
                if leftover:
                    # Bytes inside the entry after its compressed stream: refused, and still scanned.
                    self.error("archive-layout", where, "bytes after the end of a zip entry's compressed data")
                    refused = refused or "bytes after the end of an entry's compressed data"
                    self.scan_extra(leftover, where, "bytes after the compressed data", depth)
                if info.is_dir():
                    if not content:
                        continue
                    # A directory entry holds no data: one that does is refused, and its bytes are scanned.
                    self.error("archive-layout", where, "a zip directory entry holds data")
                    refused = refused or "a directory entry holds data"
                    self.scan_content(content, info.filename, where, depth)
                    continue
                base = info.filename.rsplit("/", 1)[-1]
                self.scan_content(content, info.filename, where, depth, self.declared(info.filename, base))
            if suffix_start < len(data):
                self.scan_extra(data[suffix_start:], location, "data after the zip", depth)
        for kind, problem in problems:
            self.error(kind, location, problem)
            refused = refused or problem
        self.container(location, "zip", "refused" if refused else "decoded", refused)

    # ---- artifacts

    def matches_any(self, text: str) -> bool:
        """Whether any rule matches the text in any decoded form (nothing is counted)."""
        return any(self.rules.find(value) for _, value in decoded_forms(text))

    def scan_artifact(self, index: int, path: str) -> dict:
        self.index = index
        self.coverage["artifacts"] += 1
        normal = os.path.normpath(path)
        base = os.path.basename(normal) or normal
        entry: dict = {"index": index}
        # The base name is usually the uploaded name: a match in it is a hit. The rest of a relative
        # path is local layout: it is only kept out of the report when it matches.
        name_hits = self.scan_name(base, path, "artifact name")
        shown = normal.replace(os.sep, "/")
        if os.path.isabs(normal) or shown == ".." or shown.startswith("../") or self.matches_any(shown):
            shown = base
        if name_hits:
            entry["path"], entry["path_redacted"] = f"artifact-{index}", True
        else:
            entry["path"] = shown
        try:
            status = os.stat(path)
        except OSError as error:
            entry["kind"] = "missing"
            self.error("missing", path, str(error))
            return entry
        if stat.S_ISDIR(status.st_mode):
            entry["kind"] = "directory"
            entry.update(self.scan_directory(path))
            return entry
        if not stat.S_ISREG(status.st_mode):
            entry["kind"] = "unsupported"
            self.error("unsupported", path, "not a regular file or a directory")
            return entry
        entry["kind"] = "file"
        if status.st_size > MAX_MEMBER_BYTES:
            entry["size"] = status.st_size
            entry["sha256"] = sha256_file(path)
            self.error("member-too-large", path, f"{status.st_size} bytes")
            return entry
        try:
            with open(path, "rb") as handle:
                data = handle.read()
        except OSError as error:
            self.error("unreadable", path, str(error))
            return entry
        self.coverage["files"] += 1
        entry["size"] = len(data)
        entry["sha256"] = sha256_bytes(data)
        required = is_json_name(base) or self.declared(shown, base) or self.declared(normal, base)
        self.scan_content(data, base, path, 0, required)
        return entry

    def scan_directory(self, root: str) -> dict:
        self.coverage["directories"] += 1
        lines, files, symlinks = [], 0, 0

        def failed(error: OSError) -> None:
            self.error("unreadable", getattr(error, "filename", root) or root, str(error))

        for current, dirs, names in os.walk(root, topdown=True, onerror=failed, followlinks=False):
            dirs.sort()
            relative_dir = os.path.relpath(current, root)
            for name in sorted(dirs) + sorted(names):
                full = os.path.join(current, name)
                relative = name if relative_dir == "." else os.path.join(relative_dir, name)
                relative = relative.replace(os.sep, "/")
                where = f"{root}!{relative}"
                self.scan_name(relative, where)
                try:
                    status = os.lstat(full)
                except OSError as error:
                    self.error("unreadable", where, str(error))
                    continue
                if stat.S_ISLNK(status.st_mode):
                    # An uploader may follow it and upload bytes this scan never read.
                    symlinks += 1
                    self.coverage["symlinks"] += 1
                    self.error("symlink", where, "symbolic link inside a directory: not followed")
                    try:
                        self.scan_name(os.readlink(full), where, "link target")
                    except OSError as error:
                        self.error("unreadable", where, str(error))
                    continue
                if name in dirs:
                    continue
                if not stat.S_ISREG(status.st_mode):
                    self.error("unsupported", where, "not a regular file")
                    continue
                if status.st_size > MAX_MEMBER_BYTES:
                    self.error("member-too-large", where, f"{status.st_size} bytes")
                    continue
                try:
                    with open(full, "rb") as handle:
                        data = handle.read()
                except OSError as error:
                    self.error("unreadable", where, str(error))
                    continue
                files += 1
                self.coverage["files"] += 1
                digest = sha256_bytes(data)
                lines.append(f"{digest}  {relative}\n")
                self.scan_content(data, name, where, 0, self.declared(relative, name))
        lines.sort(key=lambda line: line[66:])
        return {"files": files, "symlinks": symlinks,
                "tree_sha256": sha256_bytes("".join(lines).encode("utf-8", "surrogateescape"))}

    def container_counts(self) -> dict:
        return {kind: dict(counts) for kind, counts in sorted(self.containers.items())}


# ---------------------------------------------------------------------------
# Formats.


def is_json_name(name: str) -> bool:
    lowered = name.lower()
    for suffix in COMPRESSION_SUFFIXES:
        if lowered.endswith(suffix):
            lowered = lowered[:-len(suffix)]
            break
    return lowered.endswith(JSON_NAMES)


def strict_utf8(data: bytes) -> bool:
    try:
        data.decode("utf-8")
    except UnicodeDecodeError:
        return False
    return True


def archive_kind(data: bytes) -> str | None:
    """zip, gzip, bzip2, xz or tar; "opaque-<kind>" for a container this scanner cannot read; else None."""
    for magic, kind in OPAQUE_MAGIC:
        if data.startswith(magic):
            return f"opaque-{kind}"
    if len(data) >= 512 and data[-512:-508] == b"koly":
        return "opaque-dmg"
    if data[:4] in ZIP_MAGIC:
        return "zip"
    for kind, magic in STREAM_MAGIC.items():
        if data.startswith(magic):
            return kind
    if len(data) >= 262 and data[257:262] == b"ustar":
        return "tar"
    if prefixed_zip(data):
        return "zip"
    return None


def prefixed_zip(data: bytes) -> bool:
    """A zip after other data (a self-extracting stub): its end record is near the end and it opens."""
    if data.find(ZIP_END, max(0, len(data) - ZIP_END_SEARCH)) < 0:
        return False
    try:
        with zipfile.ZipFile(io.BytesIO(data)) as archive:
            archive.infolist()
    except (zipfile.BadZipFile, zipfile.LargeZipFile, OSError, ValueError, EOFError, struct.error):
        return False
    return True


def zip64_extra(extra: bytes) -> bool:
    """Whether a zip extra field holds a ZIP64 record (header id 1)."""
    position = 0
    while position + 4 <= len(extra):
        tag, size = struct.unpack("<HH", extra[position:position + 4])
        if tag == 0x0001:
            return True
        position += 4 + size
    return False


def zip_layout(data: bytes, archive: zipfile.ZipFile) -> tuple[int, int, list, dict, dict]:
    """Account for every byte of a zip: (end of the data before the first entry, start of the data after
    the end record and its comment, [(error kind, problem)] for the archive, {header offset: [(error kind,
    problem)]} per entry, {header offset: local extra field}). Each local header must match its central
    directory record, a data descriptor must match it too, and nothing may sit between entries, before the
    central directory or before its end record. ZIP64, encryption, unknown flags and compression methods
    other than stored, deflate, bzip2 and lzma are refused."""
    problems: list = []
    entry_problems: dict = {}
    local_extras: dict = {}
    local_names: dict = {}
    data_starts: dict = {}
    end = data.rfind(ZIP_END, max(0, len(data) - ZIP_END_SEARCH))
    if end < 0 or end + 22 > len(data):
        return 0, len(data), [("corrupt-archive", "no end-of-central-directory record")], {}, {}, {}, {}
    disk, directory_disk, disk_entries, entries, directory_size, directory_offset, comment_length = \
        struct.unpack("<HHHHIIH", data[end + 4:end + 22])
    suffix_start = min(len(data), end + 22 + comment_length)
    zip64 = (0xFFFF in (disk, directory_disk, disk_entries, entries)
             or 0xFFFFFFFF in (directory_size, directory_offset)
             or (end >= 20 and data[end - 20:end - 16] == ZIP64_LOCATOR))
    if zip64:
        problems.append(("unsupported-archive", "a ZIP64 archive"))
    elif disk or directory_disk or disk_entries != entries:
        problems.append(("unsupported-archive", "a zip that spans several disks"))
    elif entries != len(archive.infolist()):
        problems.append(("archive-layout", "the end record's entry count differs from the central directory"))
    infos = sorted(archive.infolist(), key=lambda info: info.header_offset)
    start_dir = archive.start_dir
    prefix_end = infos[0].header_offset if infos else start_dir
    position = prefix_end
    for index, info in enumerate(infos):
        own = entry_problems.setdefault(info.header_offset, [])
        if info.header_offset != position:
            problems.append(("archive-layout",
                             f"{info.header_offset - position} bytes at offset {position} belong to no zip entry"))
            break
        header = data[position:position + 30]
        if len(header) < 30 or header[:4] != ZIP_LOCAL:
            problems.append(("archive-layout", f"no local header at offset {position}"))
            break
        version, flags, method, dos_time, dos_date, crc, compressed, size, name_length, extra_length = \
            struct.unpack("<HHHHHIIIHH", header[4:30])
        year, month, day, hour, minute, second = info.date_time
        central_time = (hour << 11 | minute << 5 | second // 2, (year - 1980) << 9 | month << 5 | day)
        raw_name = data[position + 30:position + 30 + name_length]
        local_extra = data[position + 30 + name_length:position + 30 + name_length + extra_length]
        local_extras[info.header_offset] = local_extra
        local_names[info.header_offset] = raw_name
        data_starts[info.header_offset] = position + 30 + name_length + extra_length
        central_name = info.orig_filename.encode("utf-8" if info.flag_bits & 0x800 else "cp437")
        if b"\x00" in raw_name or b"\x00" in central_name:
            # zipfile cuts a name at its first NUL: what follows would never reach the parsed name.
            own.append(("archive-layout", "a zip entry name holds a NUL byte"))
        either = flags | info.flag_bits
        if either & ~ZIP_ALLOWED_FLAGS & 0xFFFF:
            own.append(("unsupported-archive",
                        "an encrypted zip entry" if either & 0x0001 else f"zip entry flags {either:#06x}"))
        if zip64_extra(local_extra) or zip64_extra(info.extra):
            own.append(("unsupported-archive", "a ZIP64 zip entry"))
        if (flags != info.flag_bits or method != info.compress_type or raw_name != central_name
                or version != info.extract_version or (dos_time, dos_date) != central_time):
            own.append(("archive-layout", "the local header differs from the central directory record"))
        recorded = (info.CRC, info.compress_size, info.file_size)
        if (crc, compressed, size) != recorded and not (flags & 0x08 and (crc, compressed, size) == (0, 0, 0)):
            own.append(("archive-layout", "the local header CRC or sizes differ from the central directory record"))
        position += 30 + name_length + extra_length + info.compress_size
        if flags & 0x08:
            following = infos[index + 1].header_offset if index + 1 < len(infos) else start_dir
            descriptor = data[position:following]
            if descriptor[:4] == ZIP_DESCRIPTOR:
                descriptor = descriptor[4:]
            if len(descriptor) != 12 or struct.unpack("<III", descriptor) != recorded:
                problems.append(("archive-layout",
                                 f"no valid data descriptor after the zip entry at offset {info.header_offset}"))
                break
            position = following
    else:
        if position != start_dir:
            problems.append(("archive-layout",
                             f"{start_dir - position} bytes between the last zip entry and the central directory"))
    if not zip64 and start_dir + directory_size != end:
        problems.append(("archive-layout",
                         f"{end - start_dir - directory_size} bytes between the central directory and its end record"))
    return prefix_end, suffix_start, problems, entry_problems, local_extras, data_starts, local_names


def zip_member_content(data: bytes, info: zipfile.ZipInfo, start: int) -> tuple[bytes, bytes]:
    """Decode one zip entry's data here rather than through zipfile, so that every compressed byte is
    accounted for: (the content, any bytes after the end of the compressed stream inside the entry). Raises
    Refusal for an unsupported form, ValueError for damaged data or a size or CRC mismatch, and
    OverflowError past MAX_MEMBER_BYTES."""
    compressed = data[start:start + info.compress_size]
    if len(compressed) != info.compress_size:
        raise ValueError("the zip entry's data is truncated")
    leftover = b""
    if info.compress_type == zipfile.ZIP_STORED:
        content = compressed
    else:
        if info.compress_type == zipfile.ZIP_DEFLATED:
            decompressor = zlib.decompressobj(-zlib.MAX_WBITS)
        elif info.compress_type == zipfile.ZIP_BZIP2:
            decompressor = bz2.BZ2Decompressor()
        elif info.compress_type != zipfile.ZIP_LZMA:
            raise Refusal("unsupported-archive", f"zip compression method {info.compress_type}")
        else:  # lzma: version, properties, then a raw LZMA1 stream that must carry its end marker
            if not info.flag_bits & 0x0002:
                raise Refusal("unsupported-archive", "an lzma zip entry without an end marker")
            if len(compressed) < 4:
                raise ValueError("the zip entry's lzma header is truncated")
            size = struct.unpack("<H", compressed[2:4])[0]
            lzma1 = lzma._decode_filter_properties(lzma.FILTER_LZMA1, compressed[4:4 + size])
            decompressor = lzma.LZMADecompressor(lzma.FORMAT_RAW, filters=[lzma1])
            compressed = compressed[4 + size:]
        content = decompressor.decompress(compressed, MAX_MEMBER_BYTES + 1)
        if len(content) > MAX_MEMBER_BYTES:
            raise OverflowError("zip entry")
        if not decompressor.eof:
            raise ValueError("the zip entry's compressed data ends before its end marker")
        leftover = decompressor.unused_data
    if len(content) != info.file_size or zlib.crc32(content) & 0xFFFFFFFF != info.CRC:
        raise ValueError("the zip entry's size or CRC does not match its record")
    return content, leftover


def tar_header_text(block: bytes) -> str:
    """One raw tar header block as text, field by field with a NUL between fields (the type flag must not
    run into the link name), every byte of every field included."""
    return "\x00".join(decode_text(block[start:start + length]) for start, length in TAR_FIELDS)


def tar_header_problems(block: bytes) -> list:
    """Every byte of one 512-byte tar header block: string fields end in NUL padding, number fields hold
    octal digits, spaces and NULs only, and unused areas are zero. Returns [(error kind, problem)]."""
    magic = block[257:265]
    zero_areas: tuple = ()
    if magic == TAR_POSIX_MAGIC:
        strings, numbers, zero_areas = TAR_GNU_STRINGS, TAR_NUMBERS, ((500, 12),)
        prefix = block[345:500]
        end = prefix.find(b"\x00")
        if 0 <= end < 131 and block[476:500].strip(b"\x00"):
            # The star convention (also node-tar): access and change times as numbers in the field's last
            # 24 bytes after a prefix of at most 130 bytes.
            numbers = numbers + ((476, 12), (488, 12))
            zero_areas = zero_areas + ((345 + end, 131 - end),)
        else:
            strings = strings + ((345, 155),)
    elif magic == TAR_GNU_MAGIC:
        # GNU: access, change and offset times and the real size are numbers; the rest of the area is unused
        # (sparse members are refused by type).
        strings, numbers = TAR_GNU_STRINGS, TAR_NUMBERS + ((345, 12), (357, 12), (369, 12), (483, 12))
        zero_areas = ((381, 102), (495, 17))
    elif not magic.strip(b"\x00"):
        strings, numbers, zero_areas = TAR_V7_STRINGS, TAR_V7_NUMBERS, ((257, 255),)
    else:
        return [("unsupported-archive", "a tar header of unknown format")]
    problems = []
    for start, length in numbers:
        field = block[start:start + length]
        if field[:1] in (b"\x80", b"\xff"):
            problems.append(("unsupported-archive", "a base-256 number in a tar header"))
        elif not TAR_NUMBER.match(field):
            problems.append(("archive-layout", "a tar number field holds other bytes"))
    for start, length in strings:
        field = block[start:start + length]
        end = field.find(b"\x00")
        if end >= 0 and field[end:].strip(b"\x00"):
            problems.append(("archive-layout", "bytes after the end of a tar header field"))
    if any(block[start:start + length].strip(b"\x00") for start, length in zero_areas):
        problems.append(("archive-layout", "non-zero bytes in an unused tar header area"))
    return problems


def tar_extension_problems(typeflag: bytes, body: bytes) -> list:
    """A pax header must be exactly a sequence of "LENGTH key=value\\n" records; a GNU long name or link
    is the name, then NUL padding only."""
    if typeflag in (b"x", b"g"):
        position = 0
        while position < len(body):
            match = TAR_PAX_RECORD.match(body, position)
            length = int(match.group(1)) if match else 0
            record = body[position:position + length]
            if (not match or length <= len(match.group(0)) or len(record) != length or not record.endswith(b"\n")
                    or b"=" not in record[len(match.group(0)):]):
                return [("archive-layout", "a malformed pax header record")]
            position += length
        return []
    end = body.find(b"\x00")
    if end >= 0 and body[end:].strip(b"\x00"):
        return [("archive-layout", "bytes after the end of a GNU long name")]
    return []


def new_decompressor(kind: str):
    if kind == "gzip":
        return zlib.decompressobj(16 + zlib.MAX_WBITS)
    if kind == "bzip2":
        return bz2.BZ2Decompressor()
    return lzma.LZMADecompressor(format=lzma.FORMAT_XZ)


def gzip_header_end(data: bytes, start: int) -> int:
    """Where the gzip member header at start ends (its name, comment and extra field included). Raises
    Refusal for a header form this scanner does not accept and ValueError for a truncated one."""
    header = data[start:start + 10]
    if len(header) < 10:
        raise ValueError("the gzip header is truncated")
    if header[2] != 8:
        raise Refusal("unsupported-archive", "a gzip compression method other than deflate")
    flags = header[3]
    if flags & 0xE0:
        raise Refusal("unsupported-archive", "a gzip header with reserved flags set")
    position = start + 10
    if flags & 0x04:  # FEXTRA
        if len(data) < position + 2:
            raise ValueError("the gzip header is truncated")
        position += 2 + struct.unpack("<H", data[position:position + 2])[0]
    for flag in (0x08, 0x10):  # FNAME, FCOMMENT: zero-terminated
        if flags & flag:
            end = data.find(b"\x00", position)
            if end < 0:
                raise ValueError("the gzip header is truncated")
            position = end + 1
    if flags & 0x02:  # FHCRC (zlib checks it)
        position += 2
    if position > len(data):
        raise ValueError("the gzip header is truncated")
    return position


def decompress_streams(kind: str, data: bytes) -> tuple[bytes, list, bytes]:
    """Every concatenated stream of one kind: (the decompressed bytes, one header per stream (the gzip
    header bytes; empty for bzip2 and xz, which carry no metadata), the bytes after the last stream that
    start no further stream; zero padding between or after streams is skipped). Raises Refusal for a
    header form this scanner does not accept, ValueError for a damaged or truncated stream and
    OverflowError past MAX_MEMBER_BYTES."""
    magic = STREAM_MAGIC[kind]
    pieces: list[bytes] = []
    headers: list[bytes] = []
    total, position = 0, 0
    while True:
        headers.append(data[position:gzip_header_end(data, position)] if kind == "gzip" else b"")
        decompressor = new_decompressor(kind)
        pending = data[position:]
        while True:
            budget = MAX_MEMBER_BYTES + 1 - total
            piece = decompressor.decompress(pending, budget)
            pending = decompressor.unconsumed_tail if kind == "gzip" else b""
            total += len(piece)
            pieces.append(piece)
            if total > MAX_MEMBER_BYTES:
                raise OverflowError(kind)
            if decompressor.eof:
                break
            if kind == "gzip":
                if not pending:
                    raise ValueError("the stream ends before its end marker")
            elif decompressor.needs_input:
                raise ValueError("the stream ends before its end marker")
        rest = decompressor.unused_data
        following = NONZERO.search(rest)
        if following is None:
            return b"".join(pieces), headers, b""
        rest = rest[following.start():]
        if not rest.startswith(magic):
            return b"".join(pieces), headers, rest
        position = len(data) - len(rest)


def decode_text(data: bytes) -> str:
    if data.startswith((b"\xff\xfe", b"\xfe\xff")):
        return data.decode("utf-16", errors="replace")
    return data.decode("utf-8", errors="replace")


# ---------------------------------------------------------------------------
# Inputs and outputs.


def unique_pairs(pairs: list) -> dict:
    keys = [key for key, _ in pairs]
    if len(set(keys)) != len(keys):
        raise ValueError("an object repeats a key")
    return dict(pairs)


def load_json(path: str, label: str) -> tuple[object, str]:
    try:
        with open(path, "rb") as handle:
            raw = handle.read()
    except OSError as error:
        raise ConfigError(f"cannot read the {label} file: {error}") from None
    try:
        return json.loads(raw.decode("utf-8"), object_pairs_hook=unique_pairs), sha256_bytes(raw)
    except (UnicodeDecodeError, ValueError) as error:
        raise ConfigError(f"the {label} file is not JSON: {error}") from None


def identity(path: str) -> tuple[int, int] | None:
    try:
        status = os.stat(path)
    except OSError:
        return None
    return status.st_dev, status.st_ino


class Inputs:
    """Everything the scan reads: paths, files (by device and inode) and scanned directories."""

    def __init__(self, paths: list[str]):
        self.paths = set()
        self.files = set()
        self.directories = set()
        for path in paths:
            self.paths.add(os.path.realpath(path))
            try:
                status = os.stat(path)
            except OSError:
                continue
            if stat.S_ISDIR(status.st_mode):
                self.directories.add((status.st_dev, status.st_ino))
                for current, _, names in os.walk(path, followlinks=False):
                    for name in names:
                        try:
                            entry = os.lstat(os.path.join(current, name))
                        except OSError:
                            continue
                        if stat.S_ISREG(entry.st_mode):
                            self.files.add((entry.st_dev, entry.st_ino))
            elif stat.S_ISREG(status.st_mode):
                self.files.add((status.st_dev, status.st_ino))


def check_destination(label: str, path: str, inputs: Inputs) -> None:
    """Refuse an output path that is, links to or lies inside anything the scan reads."""
    absolute = os.path.abspath(path)
    parent = os.path.dirname(absolute)
    if not os.path.isdir(parent):
        raise ConfigError(f"the {label} directory does not exist")
    if os.path.realpath(absolute) in inputs.paths:
        raise ConfigError(f"the {label} path is a scanned artifact, the rules or exemptions file or the scanner")
    ancestor = os.path.realpath(parent)
    while True:
        if identity(ancestor) in inputs.directories:
            raise ConfigError(f"the {label} path is inside a scanned directory")
        upper = os.path.dirname(ancestor)
        if upper == ancestor:
            break
        ancestor = upper
    if os.path.lexists(absolute):
        if os.path.isdir(absolute):
            raise ConfigError(f"the {label} path is a directory")
        if identity(absolute) in inputs.files:  # the same file: a hard link, or a symbolic link to one
            raise ConfigError(f"the {label} path is the same file as an input")


def same_destination(first: str, second: str) -> bool:
    if os.path.realpath(first) == os.path.realpath(second):
        return True
    one, two = identity(first), identity(second)
    return one is not None and one == two


def write_output(path: str, text: str, mode: int) -> None:
    """Write through a new, uniquely named staging file next to the output, then rename it into place."""
    absolute = os.path.abspath(path)
    directory, base = os.path.split(absolute)
    descriptor, staging = tempfile.mkstemp(prefix=f".{base}.", suffix=".partial", dir=directory)
    try:
        with os.fdopen(descriptor, "w", encoding="utf-8") as handle:
            handle.write(text)
            handle.flush()
            os.fsync(handle.fileno())
        os.chmod(staging, mode)
        os.replace(staging, absolute)
    except BaseException:
        try:
            os.unlink(staging)
        except OSError:
            pass
        raise


# ---------------------------------------------------------------------------
# Command line.


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--rules", required=True)
    parser.add_argument("--exemptions", required=True)
    parser.add_argument("--report", required=True, help="the PUBLIC-SAFE JSON report")
    parser.add_argument("--detail", required=True, help="the PRIVATE detail JSON (mode 0600)")
    parser.add_argument("--require-json", action="append", default=[], metavar="PATTERN",
                        help="declare matching files and members JSON (fnmatch on the path or base name)")
    parser.add_argument("--expect-scanner-sha256")
    parser.add_argument("--expect-rules-sha256")
    parser.add_argument("--expect-exemptions-sha256")
    parser.add_argument("artifacts", nargs="+", metavar="ARTIFACT")
    try:
        args = parser.parse_args(argv)
    except SystemExit as exit_:
        return EXIT_USAGE if exit_.code else 0
    try:
        scanner_path = os.path.abspath(__file__)
        inputs = Inputs(list(args.artifacts) + [args.rules, args.exemptions, scanner_path])
        check_destination("report", args.report, inputs)
        check_destination("PRIVATE detail", args.detail, inputs)
        if same_destination(args.report, args.detail):
            raise ConfigError("--report and --detail must be different files")
        if any(not pattern for pattern in args.require_json):
            raise ConfigError("an empty --require-json pattern")
        # A failed run must never leave an earlier run's outputs behind.
        for output in (args.report, args.detail):
            if os.path.lexists(output):
                try:
                    os.unlink(output)
                except OSError as error:
                    raise ConfigError(f"cannot replace an output: {error}") from None
        scanner_sha256 = sha256_file(scanner_path)
        rules_doc, rules_sha256 = load_json(args.rules, "rules")
        exemptions_doc, exemptions_sha256 = load_json(args.exemptions, "exemptions")
        for label, actual, expected in (("scanner", scanner_sha256, args.expect_scanner_sha256),
                                        ("rules", rules_sha256, args.expect_rules_sha256),
                                        ("exemptions", exemptions_sha256, args.expect_exemptions_sha256)):
            if expected is not None and expected.lower() != actual:
                raise ConfigError(f"the {label} sha256 is {actual}, not the expected {expected}")
        rules = Rules(rules_doc, exemptions_doc)
    except ConfigError as error:
        print(f"release-evidence-privacy-scan: {error}", file=sys.stderr)
        return EXIT_USAGE

    scanner = Scanner(rules, args.require_json)
    artifacts = [scanner.scan_artifact(index, path) for index, path in enumerate(args.artifacts)]
    hit_total = sum(scanner.hits.values())
    error_total = sum(scanner.errors.values())
    code = (EXIT_HITS if hit_total else 0) | (EXIT_ERRORS if error_total else 0)
    result = RESULTS[code]
    report = {
        "schema": SCHEMA_REPORT,
        "generated_at": datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
        "result": result,
        "exit_code": code,
        "scanner": {"name": SCANNER_NAME, "sha256": scanner_sha256, "python": platform.python_version()},
        "rules": {"schema": SCHEMA_RULES, "rules_id": rules.rules_id, "sha256": rules_sha256,
                  "categories": rules.categories, "rule_count": rules.rule_count},
        "exemptions": {"schema": SCHEMA_EXEMPTIONS, "exemptions_id": rules.exemptions_id,
                       "sha256": exemptions_sha256, "count": rules.exemption_count},
        "artifacts": artifacts,
        "coverage": scanner.coverage,
        "containers": scanner.container_counts(),
        "hits": scanner.hits,
        "hit_total": hit_total,
        "errors": [{"artifact": index, "kind": kind, "count": count}
                   for (index, kind), count in sorted(scanner.errors.items())],
        "error_total": error_total,
    }
    report_text = json.dumps(report, indent=1, sort_keys=False, ensure_ascii=True) + "\n"
    # The report is public: refuse to write one that a rule matches.
    self_check = Scanner(rules)
    leaked = self_check.scan_string(report_text, "<report>", "report")
    detail = {
        "schema": SCHEMA_DETAIL,
        "private": "PRIVATE: matched strings and local paths; never upload.",
        "report_sha256": None if leaked else sha256_bytes(report_text.encode("utf-8")),
        "artifacts": [{"index": index, "path": path} for index, path in enumerate(args.artifacts)],
        "hits": scanner.detail_hits,
        "containers": scanner.detail_containers,
        "errors": scanner.detail_errors,
        "report_self_check_hits": self_check.detail_hits,
    }
    try:
        write_output(args.detail, json.dumps(detail, indent=1, ensure_ascii=True) + "\n", 0o600)
        if leaked:
            print("release-evidence-privacy-scan: the report itself matched a rule; no report written "
                  "(see the detail file)", file=sys.stderr)
            return EXIT_USAGE
        write_output(args.report, report_text, 0o644)
    except OSError as error:
        print(f"release-evidence-privacy-scan: cannot write an output: {error}", file=sys.stderr)
        return EXIT_USAGE
    summary = ", ".join(f"{category} {count}" for category, count in scanner.hits.items())
    print(f"release-evidence-privacy-scan: {result}: {hit_total} hit(s) [{summary}], {error_total} scan "
          f"error(s); {scanner.coverage['files']} files, {scanner.coverage['members']} members, "
          f"{scanner.coverage['bytes']} bytes")
    return code


def run() -> int:
    """main(), with a crash reported as exit 4 rather than Python's 1 (which means a hit)."""
    try:
        return main()
    except Exception as error:  # an internal error must not look like a result
        frame = traceback.extract_tb(error.__traceback__)[-1]
        print(f"release-evidence-privacy-scan: internal error {type(error).__name__} in {frame.name} line "
              f"{frame.lineno}; no report written", file=sys.stderr)
        return EXIT_USAGE


if __name__ == "__main__":
    sys.exit(run())
