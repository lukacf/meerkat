#!/usr/bin/env python3
"""Check, before an upload, that the exact files have a passing upload receipt. Fails closed.

  python3 -I release-upload-receipt-check.py --target TARGET --receipt RECEIPT \\
      --release-commit COMMIT --version N.N.N [--max-age-hours N] FILE...

FILE... are the files about to be uploaded, as they are now, all of one target: every file of the
receipt, and no other. Nothing may be uploaded unless this tool exits 0.

The receipt (schema release-upload-receipt/3) is written by release-upload-receipt-write.py, where
the scans ran, from the two full scan reports: the private-text report of
release-payload-privacy-scan.py and the credential report of release-payload-credential-scan.py.
It holds exactly what this check reads: the target, the payload rows, and per scan the status
word, release, the time, the tool's commit and sha256, the digests of the inputs the scan ran with,
its facts as true or false (four of the private-text scan, three of the credential scan), the
names of the tools whose proofs the used acceptance entries carry with the entries each proved,
and the sha256 of the full report. This check never reads a full
report, and a full report given as the receipt refuses: a receipt with one field more or less than
its schema is not a receipt. So no category name and no count of rules or findings reaches a place
that only holds receipts.

The check passes only when the receipt
  - exists, is a JSON object of that schema with exactly its fields, is not named PROOF-ONLY, and
    names TARGET;
  - has payload rows that are exactly the FILEs: as many rows as files, each file one row with
    the same name, the same size in bytes and the same sha256 as the file has now, each row one
    file, no name twice and no PROOF-ONLY file. A caller cannot check a part of a receipt;
and, for BOTH scans:
  - status is exactly "PASS" and release is true;
  - generated_at is a UTC time (YYYY-MM-DDTHH:MM:SSZ) no older than 24 hours and not in the
    future (--max-age-hours can make the bound shorter, never longer);
  - tool_commit is the release commit given by the caller (a full commit id): the scan ran from
    the checkout that is being released;
and, for the private-text scan:
  - exit_code is 0 and blocking_rules is true (rules that can refuse nothing prove nothing);
  - name_rules_all_blocking is true: every rule outside the path category could refuse. The rules
    of one list are one exact file only for one set of the converter's options, and a class made
    a review class refuses nothing;
  - no_exemptions is true: the scan ran with no exemption. An exemption switches a match off, and
    the exemptions file is no committed file, so no pin holds it;
  - accepted_only_with_proof is true: every acceptance entry the scan used names the tool that
    proved it. accepted_proof_tools names those tools, each with the number of entries it proved
    (an object of tool names and whole numbers of 1 or more; empty when nothing was accepted).
    A tool's name has one form: a lower-case letter or a digit, then lower-case letters, digits
    and single hyphens, 64 characters at most; a receipt that holds any other text as a name
    refuses. The check does not verify a proof and holds no acceptance file: the names are
    there for whoever reads the receipt before the upload;
  - rules_sha256 is the pinned rules file and name_list_sha256 the pinned name list. Both: a
    report's name list digest is only what its rules file says about itself, so the rules file
    is pinned too;
  - non_published_list_sha256 is the pinned list of members a library leaves out;
  - coverage_exceptions_sha256 is the sha256 of release/payload-scan-coverage-exceptions.json as
    this tool's checkout commits it: a scan that ran with another exceptions file refuses;
and, for the credential scan:
  - detector_pinned_sha256 and detector_current_sha256 are the two pinned builds;
  - nothing_refused, nothing_pending and nothing_unread_unaccepted are true.

Pins. What a receipt is held to is not the caller's to say. It is read from the checkout this
tool runs from: release/upload-receipt-pins.json (beside scripts/), schema
release-upload-receipt-check/pins/2:
  {"schema": ..., "targets": {TARGET: {"names": "holds-target" | [PATTERN, ...],
    "name_list_sha256": HEX|null, "rules_sha256": HEX|null, "non_published_list_sha256": HEX|null,
    "detector_pinned_sha256": HEX|null, "detector_current_sha256": HEX|null}}}
Each target has its own pins. The file holds digests only: a name list, its rules and a list that
names members a library leaves out are private and are in no repository (a target that leaves
nothing out pins the empty list, which names nothing). This tool has no option, environment
variable or other input that gives a pin or a name rule, and it refuses unless
  - it runs from a git checkout, and this file, the pins file and the coverage exceptions file
    are regular files of that checkout, tracked, and byte for byte what the checkout's commit
    holds (a file that is a link, that lies elsewhere or that was edited after the commit
    refuses; no variable of the caller's environment names another repository, and git is told
    to read the commit's own objects, so a replace ref puts no other file in their place);
  - the pins file is of that schema, with exactly those keys;
  - it has pins for TARGET and every one of them is set: a pin that is null or missing refuses
    every upload of that target until a commit sets it;
  - every FILE's name is a name of TARGET and of no other target of the pins file, so the files
    of one target are never judged by another target's pins.

Names. Each target of the pins file states which file names are its own ("names"), and the
file names of every target are held to the version of the release, which the caller gives with
--version in the one form N.N.N (without it nothing passes):
  - "holds-target", a platform target: the name holds the target's name, and every version the
    name holds (a dotted number of three or more parts) is exactly the release's. A name that
    holds no version passes; a platform's file named for another release does not;
  - a list of exact names, the target `release` (the files of a release that hold no platform's
    name): the whole name equals exactly one pattern of the list. A pattern is a file name; at most
    one "<version>" in it stands for the version of the release.
A file passes for TARGET only when TARGET is the one target whose rule its name meets: a name
that also holds another platform's name, or that is an exact name of another target (for any
version), passes under neither. The pins file itself refuses, for every target, when a pattern
holds a platform target's name, occurs twice, or is no plain file name.
So a receipt is held to what a reviewed commit fixed, and the pass line names that commit.

What this check does not prove: that a scan ran. A receipt is a file; one who can write it by hand
can pass the check. It guards against an upload that skipped a scan, used other inputs or changed
a file afterwards.

Output: every reason for a refusal, one per line, then "UPLOAD-CHECK REFUSED"; or one line
  UPLOAD-CHECK PASS files=<n> <file>=<sha256>:<bytes> ... release_commit=<commit>
    receipt=<sha256> privacy_report=<sha256> credential_report=<sha256> target=<target>
    pins=<sha256 of the pins file> pins_commit=<the commit of this tool's checkout>
Exit status: 0 pass, 1 refused, 2 usage error. Reads only. Standard library only, no network.

From Python (an upload tool of the same checkout; the module is loaded by its path):
  check(target, receipt, release_commit, files, *, version=None, max_age_hours=24.0) returns Passed
  (line, target, release_commit, files, receipt_sha256, pins_sha256, pins_commit) or raises Refused
  (reasons: the reason lines) or Usage; it prints nothing. The version is needed as on the command
  line: without it the check refuses. receipt_rows(receipt) gives the rows of a
  receipt for a caller that takes its file list from the receipt, and judges nothing.
"""
from __future__ import annotations

import argparse
import calendar
import hashlib
import json
import os
import re
import subprocess
import sys
import time
from pathlib import Path
from typing import NamedTuple

RECEIPT_SCHEMA = "release-upload-receipt/3"
HEX64 = re.compile(r"[0-9a-f]{64}\Z")
COMMIT = re.compile(r"[0-9a-f]{40}([0-9a-f]{24})?\Z")
STAMP = re.compile(r"[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}Z\Z")
PROOF_ONLY = "PROOF-ONLY"
FUTURE_SECONDS = 300
MAX_AGE_HOURS = 24.0
TOOL = Path(__file__).resolve()
RELEASE = TOOL.parent.parent / "release"
PINS_FILE = RELEASE / "upload-receipt-pins.json"
EXCEPTIONS_FILE = RELEASE / "payload-scan-coverage-exceptions.json"
PINS_SCHEMA = "release-upload-receipt-check/pins/2"
PINS = ("name_list_sha256", "rules_sha256", "non_published_list_sha256", "detector_pinned_sha256",
        "detector_current_sha256")
# The name rule of a target: its files hold its name, or they are exactly the names of a list.
HOLDS_TARGET = "holds-target"
TARGET_NAME = re.compile(r"[a-z0-9][a-z0-9_-]{0,63}\Z")
# The one form of a release's version, and its place in a pattern of exact names.
VERSION = re.compile(r"[0-9]+\.[0-9]+\.[0-9]+\Z")
VERSION_PLACE = "<version>"
# A version in a platform's file name: a dotted number of three or more parts, as long as it runs.
VERSIONS_HELD = re.compile(r"[0-9]+(?:\.[0-9]+){2,}")
PATTERN = re.compile(r"[A-Za-z0-9](?:[A-Za-z0-9._-]|<version>)*\Z")
# The fields of a receipt: of the document, of each scan's part, of a payload row. Exactly these.
RECEIPT = ("schema", "target", "payload", "privacy", "credential")
SCAN = ("status", "release", "generated_at", "tool_commit", "tool_sha256", "report_sha256")
PRIVACY = (*SCAN, "exit_code", "rules_sha256", "name_list_sha256", "non_published_list_sha256",
           "coverage_exceptions_sha256", "blocking_rules", "name_rules_all_blocking", "no_exemptions",
           "accepted_only_with_proof", "accepted_proof_tools")
CREDENTIAL = (*SCAN, "detector_pinned_sha256", "detector_current_sha256", "nothing_refused", "nothing_pending",
              "nothing_unread_unaccepted")
ROW = ("name", "bytes", "sha256")
# The one form of a proof tool's name, as the private-text scan and the receipt writer hold it: a
# lower-case letter or a digit, then lower-case letters, digits and single hyphens, 64 at most.
PROOF_TOOL = re.compile(r"(?!.*--)[a-z0-9][a-z0-9-]{0,63}\Z")


class Usage(Exception):
    """An argument that cannot be used: the command line's exit status 2."""


class Refused(Exception):
    """The check refuses. reasons holds every reason, one line each; nothing may be uploaded."""

    def __init__(self, reasons):
        self.reasons = [reasons] if isinstance(reasons, str) else list(reasons)
        super().__init__("; ".join(self.reasons))


class Passed(NamedTuple):
    """What a passing check states: the pass line and its parts."""
    line: str
    target: str
    release_commit: str
    files: tuple  # (name, bytes, sha256) of every file, in the order given
    receipt_sha256: str
    pins_sha256: str
    pins_commit: str


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with open(path, "rb") as stream:
        for block in iter(lambda: stream.read(1 << 20), b""):
            digest.update(block)
    return digest.hexdigest()


def count(value) -> bool:
    return isinstance(value, int) and not isinstance(value, bool) and value >= 0


def is_digest(value) -> bool:
    return isinstance(value, str) and HEX64.match(value) is not None


def git(*arguments: str) -> bytes | None:
    """What a git command prints, run in this tool's directory; None when it fails. No GIT_ variable of
    the caller's environment reaches it, so the repository is the one this file lies in, and git
    reads the commit's own objects, not objects a replace ref puts in their place."""
    environment = {key: value for key, value in os.environ.items() if not key.startswith("GIT_")}
    try:
        done = subprocess.run(["git", "--no-replace-objects", "-C", str(TOOL.parent), *arguments], stdin=subprocess.DEVNULL,
                              stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, env=environment, check=False)
    except OSError:
        return None
    return done.stdout if done.returncode == 0 else None


def unique(pairs):
    value = dict(pairs)
    if len(value) != len(pairs):
        raise ValueError("a key twice")
    return value


class Names:
    """The name rules of a pins file: {target: "holds-target" or its list of exact names}."""

    def __init__(self, rules: dict):
        self.rules = rules

    @staticmethod
    def equals(pattern: str, name: str, version: str | None) -> bool:
        """Whether name is the pattern's name for the version; for any version when none is given."""
        head, place, tail = pattern.partition(VERSION_PLACE)
        if not place:
            return name == pattern
        if len(name) < len(head) + len(tail) or not name.startswith(head) or not name.endswith(tail):
            return False
        middle = name[len(head):len(name) - len(tail)]
        return middle == version if version is not None else VERSION.match(middle) is not None

    def homes(self, name: str, target: str, version: str | None) -> set[str]:
        """Every target whose rule the file name meets: target's own rule with the given version,
        another target's rule for any version. A file passes for target when this is {target}. A name
        that holds target meets its rule only when every version the name holds is the given one."""
        found = set()
        for other, rule in self.rules.items():
            if not isinstance(rule, list):
                met = other in name and (other != target or all(held == version for held in VERSIONS_HELD.findall(name)))
            elif other == target:
                met = sum(self.equals(pattern, name, version) for pattern in rule) == 1
            else:
                met = any(self.equals(pattern, name, None) for pattern in rule)
            if met:
                found.add(other)
        return found


class Checkout:
    """The commit of the checkout this tool runs from, and the files it holds there."""

    def __init__(self):
        top, head = git("rev-parse", "--show-toplevel"), git("rev-parse", "--verify", "HEAD^{commit}")
        commit = (head or b"").decode("ascii", "replace").strip()
        if top is None or not COMMIT.match(commit):
            raise Refused("pins: this tool does not run from a git checkout with a commit, and the pins are a commit's")
        self.top, self.commit = Path(os.fsdecode(top.rstrip(b"\r\n"))).resolve(), commit
        # The check itself must be the commit's, or the commit does not vouch for it.
        self.committed(TOOL)

    def committed(self, path: Path) -> bytes:
        """The bytes of a file of this checkout that are, byte for byte, what its commit holds at that
        path. A link, a file below a linked directory and a file the commit does not track never are:
        the commit holds no such bytes there."""
        name = "release/" + path.name if path.parent == RELEASE else "this tool"
        problem = Refused(f"pins: {name} is not a regular file that this tool's checkout tracks, "
                          "byte for byte as its commit holds it")
        try:
            relative = path.relative_to(self.top)
            raw = path.read_bytes()
        except (OSError, ValueError):
            raise problem from None
        if git("cat-file", "blob", f"{self.commit}:{relative.as_posix()}") != raw:
            raise problem
        return raw

    def blob_sha256(self, commit: str, path: Path) -> str | None:
        """The sha256 of the file at path as the given commit holds it in this checkout; None when the
        checkout has no such commit or that commit no such file. For the receipt writer, which holds
        a report's tool to the scan file of the commit the report names."""
        try:
            relative = path.relative_to(self.top)
        except ValueError:
            return None
        raw = git("cat-file", "blob", f"{commit}:{relative.as_posix()}")
        return None if raw is None else hashlib.sha256(raw).hexdigest()

    def sections(self) -> tuple[dict, str]:
        """({target: its name rule and pins}, the sha256 of the pins file). The file is refused as a whole
        when it is not of its schema or when its name rules could give one file name two targets."""
        raw = self.committed(PINS_FILE)
        problem = Refused(f"pins: release/{PINS_FILE.name} is not a {PINS_SCHEMA} document: "
                          '{"schema", "targets": {TARGET: {names, ' + ", ".join(PINS) + "}}}, names "
                          f'"{HOLDS_TARGET}" or a list of exact file names, each pin 64 lower-case hex digits or null')
        try:
            value = json.loads(raw, object_pairs_hook=unique)
        except ValueError:
            raise problem from None
        if not isinstance(value, dict) or set(value) != {"schema", "targets"} or value["schema"] != PINS_SCHEMA \
                or not isinstance(value["targets"], dict) or not value["targets"]:
            raise problem
        patterns: list[str] = []
        for target, section in value["targets"].items():
            if not TARGET_NAME.match(target) or not isinstance(section, dict) or set(section) != {"names", *PINS} or not all(
                    section[pin] is None or is_digest(section[pin]) for pin in PINS):
                raise problem
            rule = section["names"]
            if isinstance(rule, str) and rule == HOLDS_TARGET:
                continue
            if not isinstance(rule, list) or not rule or not all(
                    isinstance(pattern, str) and PATTERN.match(pattern) and pattern.count(VERSION_PLACE) <= 1 for pattern in rule):
                raise problem
            patterns += rule
        holders = [target for target, section in value["targets"].items() if not isinstance(section["names"], list)]
        if len(set(patterns)) != len(patterns):
            raise Refused(f"pins: release/{PINS_FILE.name} states an exact file name twice: no file name is a name of two targets")
        if any(target in pattern for target in holders for pattern in patterns):
            raise Refused(f"pins: release/{PINS_FILE.name} states an exact file name that holds a platform target's name: "
                          "no file name is a name of two targets")
        return value["targets"], hashlib.sha256(raw).hexdigest()

    def pins(self, target: str) -> tuple[dict, str, Names]:
        """(the pins of target, the sha256 of the pins file, the name rules of every target)."""
        sections, digest = self.sections()
        if target not in sections:
            raise Refused(f"pins: release/{PINS_FILE.name} has no pins for the target {target}")
        unset = [name for name in PINS if sections[target][name] is None]
        if unset:
            raise Refused(f"pins: {', '.join(unset)} of the target {target} not set (null) in release/{PINS_FILE.name}: "
                          "no upload of this target passes until a commit sets every pin")
        return sections[target], digest, Names({name: section["names"] for name, section in sections.items()})


def scan(label: str, value, fields: tuple, commit: str, max_age: float, problems: list) -> dict | None:
    """What both scans' parts of a receipt share. Returns the part, or None when it is not one."""
    if not isinstance(value, dict) or set(value) != set(fields):
        problems.append(f"{label}: not exactly the fields of a receipt (a full report is not a receipt)")
        return None
    if value["status"] != "PASS" or not isinstance(value["status"], str):
        problems.append(f"{label}: status is not PASS")
    if value["release"] is not True:
        problems.append(f"{label}: release is not true (a run that is not a release run is no receipt)")
    stamp = value["generated_at"]
    if not isinstance(stamp, str) or not STAMP.match(stamp):
        problems.append(f"{label}: generated_at is missing or not a UTC time")
    else:
        try:
            age = time.time() - calendar.timegm(time.strptime(stamp, "%Y-%m-%dT%H:%M:%SZ"))
        except ValueError:
            age = None
        if age is None or age > max_age or age < -FUTURE_SECONDS:
            problems.append(f"{label}: generated_at is stale, in the future or not a time")
    if value["tool_commit"] != commit:
        problems.append(f"{label}: tool_commit is not the release commit")
    for field in ("tool_sha256", "report_sha256"):
        if not is_digest(value[field]):
            problems.append(f"{label}: {field} is not a sha256")
    return value


def read_receipt(path: Path, problems: list) -> tuple[dict | None, bytes, list | None]:
    """(the receipt, its bytes, its rows as (name, bytes, sha256) in its order). The receipt is None when
    the file is not a receipt document at all, the rows when its payload is not a list of rows; each
    with its reason in problems. A reason may also stand beside rows (a name twice, a PROOF-ONLY name)."""
    if PROOF_ONLY in path.name:
        problems.append(f"receipt: the receipt file is named {PROOF_ONLY}")
    if not path.is_file():
        problems.append("receipt: the receipt is missing")
        return None, b"", None
    try:
        raw = path.read_bytes()
        receipt = json.loads(raw, object_pairs_hook=unique)
    except (OSError, ValueError):
        problems.append("receipt: the receipt is not readable JSON")
        return None, b"", None
    if not (isinstance(receipt, dict) and set(receipt) == set(RECEIPT) and receipt["schema"] == RECEIPT_SCHEMA):
        problems.append(f"receipt: not a {RECEIPT_SCHEMA} document with exactly its fields (a full report is not a receipt)")
        return None, raw, None
    payload = receipt["payload"]
    if not isinstance(payload, list) or not payload or not all(
            isinstance(row, dict) and set(row) == set(ROW) and isinstance(row["name"], str) and count(row["bytes"])
            and is_digest(row["sha256"]) for row in payload):
        problems.append("receipt: payload does not list files with exactly name, bytes and sha256")
        return receipt, raw, None
    names = [row["name"] for row in payload]
    if any(name.startswith(PROOF_ONLY) for name in names):
        problems.append(f"receipt: payload names a {PROOF_ONLY} file")
    if len(set(names)) != len(names):
        problems.append("receipt: payload names a file twice")
    return receipt, raw, [(row["name"], row["bytes"], row["sha256"]) for row in payload]


def receipt_rows(receipt: Path) -> tuple:
    """The rows (name, bytes, sha256) of a receipt file, in its order: for a caller that takes its
    file list from the receipt. Raises Refused when the file is not a receipt with well-formed rows,
    each name once. It judges nothing else: only check says whether an upload may happen."""
    problems: list[str] = []
    _, _, rows = read_receipt(receipt, problems)
    if problems or rows is None:
        raise Refused(problems)
    return tuple(rows)


def check(target: str, receipt: Path, release_commit: str, files, *, version: str | None = None,
          max_age_hours: float = MAX_AGE_HOURS) -> Passed:
    """The check, for a caller in Python: the same judgement as the command line. Returns Passed, or
    raises Refused with every reason (nothing may be uploaded) or Usage for an argument that cannot
    be used. Prints nothing and reads only; the files are read as they are at the call."""
    files = [Path(path) for path in files]
    if not isinstance(release_commit, str) or not COMMIT.match(release_commit):
        raise Usage("--release-commit must be a full lower-case commit id")
    if version is not None and (not isinstance(version, str) or not VERSION.match(version)):
        raise Usage("--version must be the release's version in the form N.N.N")
    if isinstance(max_age_hours, bool) or not isinstance(max_age_hours, (int, float)) or not 0 < max_age_hours <= MAX_AGE_HOURS:
        raise Usage(f"--max-age-hours must be positive and at most {MAX_AGE_HOURS:g}: it can only shorten the bound")
    if not files:
        raise Usage("at least one file to upload is required")
    for path in files:
        if path.is_symlink() or not path.is_file():
            raise Usage(f"not a regular file: {path.name}")
    if len({path.name for path in files}) != len(files):
        raise Usage("two files to upload have the same name")
    files = [(path.name, path.stat().st_size, sha256_file(path)) for path in files]
    problems: list[str] = []
    max_age = max_age_hours * 3600

    # What the receipt is held to: this checkout's commit, never the caller.
    pins = pins_sha256 = pins_commit = exceptions_sha256 = names = None
    try:
        checkout = Checkout()
        pins_commit = checkout.commit
        try:
            exceptions_sha256 = hashlib.sha256(checkout.committed(EXCEPTIONS_FILE)).hexdigest()
        except Refused as error:
            problems.extend(error.reasons)
        pins, pins_sha256, names = checkout.pins(target)
    except Refused as error:
        problems.extend(error.reasons)
    # Which names are the target's is the pins file's to say; no name passes under two targets.
    if version is None:
        problems.append("files: --version is required: the file names of every target are held to the release's version")
    elif names is not None:
        for name, _, _ in files:
            others = sorted(names.homes(name, target, version) ^ {target})
            if others:
                problems.append(f"files: {name} is not a file name of the target {target} and of no other target"
                                + f" for the version {version}"
                                + (f" (it is one of the target {', '.join(other for other in others if other != target)})"
                                   if others != [target] else ""))

    document, raw, rows = read_receipt(Path(receipt), problems)
    privacy = credential = None
    if document is not None:
        if document["target"] != target:
            problems.append(f"receipt: target is not {target}")
        if rows is not None:
            for name, size, digest in files:
                if (name, size, digest) not in rows:
                    problems.append(f"receipt: payload does not name {name} at its present size {size} and sha256 {digest}")
            # The other way round: no row without its file. The files are exactly the rows.
            given = {name for name, _, _ in files}
            for name in sorted({name for name, _, _ in rows} - given):
                problems.append(f"receipt: payload names {name}, which is not among the files given "
                                "(the files must be exactly the receipt's rows)")
        privacy = scan("privacy scan", document["privacy"], PRIVACY, release_commit, max_age, problems)
        credential = scan("credential scan", document["credential"], CREDENTIAL, release_commit, max_age, problems)

    if privacy is not None:
        label = "privacy scan"
        if privacy["exit_code"] != 0 or isinstance(privacy["exit_code"], bool):
            problems.append(f"{label}: exit_code is not 0")
        if privacy["blocking_rules"] is not True:
            problems.append(f"{label}: blocking_rules is not true (the rules showed no blocking rule)")
        if privacy["name_rules_all_blocking"] is not True:
            problems.append(f"{label}: name_rules_all_blocking is not true (a rule outside the path category could refuse nothing)")
        if privacy["no_exemptions"] is not True:
            problems.append(f"{label}: no_exemptions is not true (the scan ran with an exemption, which no pin holds)")
        if privacy["accepted_only_with_proof"] is not True:
            problems.append(f"{label}: accepted_only_with_proof is not true (a used acceptance entry names no proof tool)")
        tools = privacy["accepted_proof_tools"]
        if not isinstance(tools, dict) or not all(isinstance(name, str) and PROOF_TOOL.match(name) and count(number) and number >= 1
                                                  for name, number in tools.items()):
            problems.append(f"{label}: accepted_proof_tools does not name tools, each with the number of entries it proved "
                            "(a tool's name: lower-case letters, digits and single hyphens, 64 at most)")
        if pins is not None:
            if privacy["rules_sha256"] != pins["rules_sha256"]:
                problems.append(f"{label}: rules_sha256 is not the pinned rules file")
            if privacy["name_list_sha256"] != pins["name_list_sha256"]:
                problems.append(f"{label}: name_list_sha256 is not the pinned name list")
            if privacy["non_published_list_sha256"] != pins["non_published_list_sha256"]:
                problems.append(f"{label}: non_published_list_sha256 is not the pinned list of left-out members")
        if exceptions_sha256 is not None and privacy["coverage_exceptions_sha256"] != exceptions_sha256:
            problems.append(f"{label}: coverage_exceptions_sha256 is not the coverage exceptions file this checkout commits")

    if credential is not None:
        label = "credential scan"
        if pins is not None and (credential["detector_pinned_sha256"], credential["detector_current_sha256"]) != (
                pins["detector_pinned_sha256"], pins["detector_current_sha256"]):
            problems.append(f"{label}: detector_pinned_sha256 and detector_current_sha256 are not the two pinned builds")
        for field in ("nothing_refused", "nothing_pending", "nothing_unread_unaccepted"):
            if credential[field] is not True:
                problems.append(f"{label}: {field} is not true")

    if problems:
        raise Refused(problems)
    receipt_sha256 = hashlib.sha256(raw).hexdigest()
    line = ("UPLOAD-CHECK PASS files=%d %s release_commit=%s receipt=%s privacy_report=%s credential_report=%s "
            "target=%s pins=%s pins_commit=%s" % (
                len(files), " ".join("%s=%s:%d" % (name, digest, size) for name, size, digest in files), release_commit,
                receipt_sha256, privacy["report_sha256"], credential["report_sha256"], target, pins_sha256, pins_commit))
    return Passed(line, target, release_commit, tuple(files), receipt_sha256, pins_sha256, pins_commit)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter,
                                     allow_abbrev=False)
    parser.add_argument("--target", required=True, help="the target of the files; its pins are read from the checkout")
    parser.add_argument("--receipt", type=Path, required=True, help="the upload receipt of the files")
    parser.add_argument("--release-commit", required=True, help="the full id of the commit being released")
    parser.add_argument("--version", help="the release's version, N.N.N: the file names of every target are held to it")
    parser.add_argument("--max-age-hours", type=float, default=MAX_AGE_HOURS)
    parser.add_argument("files", nargs="*", type=Path)
    args = parser.parse_args()
    try:
        passed = check(args.target, args.receipt, args.release_commit, args.files, version=args.version,
                       max_age_hours=args.max_age_hours)
    except Usage as error:
        print(f"release-upload-receipt-check: usage error: {error}", file=sys.stderr)
        return 2
    except Refused as error:
        for reason in error.reasons:
            print(reason)
        print(f"UPLOAD-CHECK REFUSED files={len(args.files)}: {len(error.reasons)} reason(s); nothing may be uploaded")
        return 1
    print(passed.line)
    return 0


if __name__ == "__main__":
    sys.exit(main())
