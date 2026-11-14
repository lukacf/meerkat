#!/usr/bin/env python3
"""Scan release payload files for credential patterns before upload. The scan fails closed.

  python3 -I release-payload-credential-scan.py --name NAME \\
      --gitleaks-pinned /ABSOLUTE/gitleaks --gitleaks-current /ABSOLUTE/gitleaks \\
      [--default-config-pinned TOML] [--default-config-current TOML] \\
      [--pinned-version LABEL] [--current-version LABEL] \\
      [--accepted-findings ACCEPTED] [--pending PENDING] \\
      --receipt RECEIPT --report REPORT [--unaccepted LISTING] [--work DIR] PAYLOAD...

What it does
  1. Deep extraction of every payload file (release-payload-deep-extract.py beside this file):
     every nested archive, at every level. An archive that cannot be opened refuses the run
     unless the acceptance file lists its exact bytes. An archive's bytes that are no content of
     a member written out are scanned too, at paths below ARCHIVE!!outside/ (!outside/ for the
     payload file itself): before-start and after-end (of a zip or a tar) as files, the rest
     (headers, comments, padding, gaps) as its runs of 8 or more printable ASCII bytes and tabs
     in headers-and-gaps; a tar inside a compressed file has tar-after-end and
     tar-headers-and-gaps. The builds read these files from a directory of their own, under
     numbered names, so that no path allowlist skips them for the path of their archive; one
     that starts with a binary signature is read by its printable runs, like a compiled file
     (item 3). None of them is ever unread. A path allowlist that matches one of the numbered
     files the scan writes for the builds (these, the files of item 2 that are read once more,
     the runs of item 3) stops the scan, and so does one of the lines that item 2 removes: the
     build's embedded default has them. A member
     whose name holds "!", or a member file whose
     name ends with "#strings", would print like another path: it is not written out, which
     refuses.
  2. Two builds of the gitleaks detector, a pinned older one and a current one, each given by
     absolute path, read the extracted tree with their own default rules minus the global path
     allowlist lines that mention "vendor", "dist-info" or "lock" (lock files). Each build's
     default rules file is
     gitleaks.toml beside the build, or the file given with --default-config-...; a run with the
     build's embedded default must give the same result over every directory a build reads of
     every payload (the scan root, the bytes outside members, the runs of compiled files, the
     runs read alone), or the scan stops. The embedded default passes over the paths of the
     removed lines, so the files the scan reads only for their removal are given to the build
     once more for this, from a directory of their own under numbered names. The
     builds always run with --redact and --ignore-gitleaks-allow, from an empty working directory,
     with the payload one level below the scan root: no comment, ignore file or rules file inside
     a payload can switch a finding off.
  3. Compiled files (ELF) are covered by their printable runs (8 or more printable ASCII bytes).
     Every run a finding touches is scanned again as a unit of its own, and only those findings
     count, so no rule can join the end of one run to the start of the next. Such a finding is
     reported at PATH#strings with the byte offset of the run as its line. The file of the
     runs begins with the first run, and a run scanned again is its own file from the first
     byte, so a rule that reports only at the first byte of a file can report them. Only a
     file that would start like a binary signature (no build reads it) has something before:
     a line of plain words before the runs, a line break before a run scanned again.
  4. Every finding is a position: rule, path inside the payload ("!" at archive boundaries), line,
     and the sha256 of the member file that holds it. A position without an acceptance bound to
     exactly those bytes REFUSES, unless the tool itself proves it to be of a committed identifier
     class (below).

Identifier classes (release/payload-credential-identifier-classes.json of the checkout this tool
runs from, a committed file; without the file, or with no entry in it, there is no class and every
output is as without this section), schema release-payload-credential-scan/identifier-classes/1:
  classes: [{rule, asset, member, key_path, value_form, accepted_by}]
  A record field that holds a random identifier is new in every build, so no acceptance can be
  bound to its bytes. An entry declares such a field: the detector rule id; the payload file name
  (asset) and the path inside it (member); the JSON key path of the field (keys joined by dots,
  a list step written [] after its key: a.b[].c); the form of the value ("uuid-v4"); and who
  accepted the class. In asset and member <version> stands for three numbers joined by dots
  (no leading zero) and <target> for a target name of two parts with one hyphen between them
  (as linux-x86_64); every other character stands for itself, and a pattern holds a letter or a
  digit of its own beside its places. A finding is resolved by an entry only when this tool
  proves, on the member's bytes, all of:
    1. the member is one JSON document (UTF-8, no key twice in an object, no constant JSON lacks);
    2. the finding's line holds one field and its text value, and that value is the value at
       exactly the declared key path (any index at a list step, no step more or less);
    3. the whole value is a canonical lower-case version 4 UUID and nothing else;
    4. every finding the builds report at that position starts and ends on that line, at columns
       of the line, and the line holds nothing but the field's key, the colon, the value and at
       most one comma: what a span can cover there is the declared key and the proved value;
    5. rule, payload file name and member path are the entry's, and every finding of the
       position lies in the one file that is read (two files that would print as one path
       are not one member).
  Anything else refuses as before. Each finding stands on its own proof. A pending position
  refuses whatever a class proves; an acceptance bound to the bytes counts before a class, and
  neither changes what the other covers. A pending entry is bound to the member's bytes, and such
  a member is new in every build: it holds for the build it was written for, so the pending row
  of a field is removed in the step that enables its class entry. A list that is not of the
  schema, or holds a key twice, is a usage error. The list in force is the commit's: when the
  file is not, byte for byte, what the checkout's commit holds, tool_commit is null, and the
  report states the list's sha256 for the receipt writer to compare. Lines are counted as the
  builds count them: from 1, split at line feeds.

Acceptance file (--accepted-findings, optional and private; without it nothing is accepted),
schema release-payload-credential-scan/accepted/1:
  positions: [{rule, path, sha256, line, class, reason, accepted_by, count?}]
             sha256 is the member file's; line is the line, or the run's byte offset for a
             PATH#strings position; count (default 1) is the number of findings there.
  archives:  [{path, sha256, index (pypi|crates.io|npm), class "PUBLIC-UPSTREAM-ARCHIVE", reason,
               accepted_by}]   everything inside that exact archive, proved byte-equal to the
             public upstream archive.
  files:     [{path, sha256, class "PUBLIC-UPSTREAM-FILE", reason, accepted_by}]   that exact
             file, proved byte-equal to a public upstream file: its findings, and the file itself
             when no build reads it.
  unopened:  [{path, sha256, reason, accepted_by}]   an archive that cannot be opened.
  Every entry needs a non-empty reason and a non-empty accepted_by; the tool has no approver of
  its own, and an entry without one is a usage error. An entry may carry a proof object (what
  was proved and how); the tool keeps it with the entry and does not read it.
Pending list (--pending, optional), schema release-payload-credential-scan/pending/1:
  positions: [{rule, path, line, sha256, note?, count?}]
  It NEVER accepts. A position on it refuses even when an acceptance entry matches, and the
  receipt marks it "pending owner".

Unread rule, per build: a file may be unread by a detector build only when it is byte-equal to a
pinned upstream member; anything else unread REFUSES. A file that starts with a binary signature
and is not an opened archive or an ELF file, a file that a build's remaining path allowlist
skips (both builds, or one build while the other reads it), a file for which a build gives no
proof of read (below), an archive that could not be opened,
and an archive member that was not written out (an encrypted, unreadable or unsafe-path member)
must each be covered by an acceptance entry for those exact bytes: a files entry, an unopened
entry, or an archives entry for an enclosing archive.

Proof of read: no list of the scan decides whether a build read a file. Each build reads every
file it is handed twice (a text member, text among the bytes outside members, a file of runs, a
run scanned again): first as it is, which keeps what every rule means at the first byte and at
the end of the text; then with a line of the scan's own as its last line, a made-up name and
value that the build's rules report (the line is cut off again afterwards). A finding of both
readings counts once. The build has read the file when it reports that line there exactly as it
reports the line alone (measured once per build); this speaks for the first reading too, since a
build takes or passes over a file for its path and its leading bytes. A file without that is
read again behind a line of plain words, twice in the same way, its findings at their own lines;
without the line there too it is unread by that build. The receipt and the report count per
build: files handed, read as they are, read behind the line, unread; readings of both kinds.

Receipt (--receipt, a new file; its first line is also printed):
  CREDENTIAL-LEG PASS|REFUSED <name> files=<n> <file>=<sha256> ... accepted=<sha256|none>
    pending=<sha256|none> builds=<version>,<version> build_sha256=<sha256>,<sha256> findings=<n>
    accepted_positions=<n> accepted_in_archives=<n> accepted_in_files=<n> refused=<n>
    pending_owner=<n> unread=<n> unread_unaccepted=<n>
    and, only when the class list holds an entry: class_tool=<name> class_resolved=<n>
PASS only when refused, pending_owner and unread_unaccepted are all 0.
Report (--report, a new file, JSON, schema release-payload-credential-scan/report/1): what an
upload check reads. status "PASS" or "REFUSED"; release (false for a --proof-only run, whose name
and label start with PROOF-ONLY-) and label; generated_at (UTC); tool_sha256; tool_commit (the commit of
the checkout this tool ran from, null when its files are not that commit's); payload [{name,
bytes, sha256}] of the bytes that were scanned; detector_builds [{role, version, sha256}];
accepted and pending {count, sha256}; the counts; and, only when the class list holds an entry,
findings.class_resolved, the number of findings a class resolved (so that findings.total is the
sum of the accepted, the refused and that number), and identifier_classes {tool, resolved,
list_sha256}: the name of the proof, the same number, and the sha256 of the class list in force.
Of a finding resolved by class the receipt and the report hold that count and nothing else: no
rule, path or line (by_rule counts the other findings; the receipt's rule table has one line for
them that names no rule). The receipt and the report
hold counts, rule ids, paths and positions only. The listing
(--unaccepted, optional, a new file, mode 0600) names every refused position and unread file with
its sha256, which is what an acceptance entry needs. No matched value, no part of one and no line
content is printed, logged or written to any of them. The detectors' redacted reports stay in the
private work directory, which is removed at the end unless --keep-work is given.

Exit status: 0 PASS, 1 REFUSED, 2 usage error, 3 tool failure (also a refusal).
Python 3.11 or later, standard library only, no network.
"""
import argparse
import collections
import functools
import hashlib
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import time
import tomllib
import traceback
import types

HERE = os.path.dirname(os.path.abspath(__file__))
EXTRACTOR = os.path.join(HERE, "release-payload-deep-extract.py")
SCHEMA_ACCEPTED = "release-payload-credential-scan/accepted/1"
SCHEMA_PENDING = "release-payload-credential-scan/pending/1"
SCHEMA_REPORT = "release-payload-credential-scan/report/1"
SCHEMA_UNACCEPTED = "release-payload-credential-scan/unaccepted/1"
HEX64 = re.compile(r"^[0-9a-f]{64}$")
LEAK_EXIT = 42
# A file that starts with one of these is not text; the builds do not read it.
BIN_MAGICS = [(0, b"\x7fELF"), (0, b"%PDF"), (0, b"SQLite format 3\x00"), (0, b"\x00asm"), (0, b"MZ"),
              (0, b"\xca\xfe\xba\xbe"), (0, b"\xcf\xfa\xed\xfe"), (0, b"wOFF"), (0, b"wOF2"),
              (0, b"PK\x03\x04"), (0, b"\x1f\x8b"), (0, b"BZh"), (0, b"\xfd7zXZ\x00"), (0, b"\x28\xb5\x2f\xfd"),
              (257, b"ustar"), (0, b"7z\xbc\xaf\x27\x1c"), (0, b"Rar!"), (0, b"!<arch>\n")]
# What the scan writes first in a file of runs that would start like one of them (see signature_led).
PLAIN_LINE = b"printable runs of a file, one run on a line\n"
# The scan's proof-of-read line: a made-up name and value that the default rules of both builds report.
# It is put together from pieces, so that no scanner of this file meets it whole.
PROOF_LINE = b"fixture " + b"api" + b"_k" + b"ey = " + "".join(["k3v9", "x1m7", "q5t2", "w8n4", "j6b0", "pd"]).encode() + b"\n"
STRINGS_RX = re.compile(rb"[\x20-\x7e]{8,}")
STRINGS = "#strings"
PROOF_ONLY = "PROOF-ONLY"
# Reasons a member was not written out that leave no content unread.
NO_CONTENT_SKIPS = ("link", "special", "duplicate-name-renamed")
UNOPENED_RULE = "unopened-archive"
SCHEMA_CLASSES = "release-payload-credential-scan/identifier-classes/1"
# The committed class list of the checkout this tool runs from. Without the file there is no class.
CLASSES_FILE = os.path.join(os.path.dirname(HERE), "release", "payload-credential-identifier-classes.json")
CLASS_TOOL = "credential-identifier-class"
# The value forms an entry can declare. The whole value is of the form, and nothing else is in it.
CLASS_FORMS = {"uuid-v4": re.compile(rb"[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}")}
# The places of an asset or member pattern. Every other character of a pattern is itself. A version is
# three numbers without a leading zero. A target is two parts with one hyphen between them, as the
# target names of release/upload-receipt-pins.json are (a payload file's name holds its target's name).
CLASS_PLACES = {"<version>": r"(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)", "<target>": r"[a-z0-9]+-[a-z0-9_]+"}
# A line that holds one field and its text value, at most one comma after it, and nothing else. No part
# of it can be read in two ways, so a line is decided in time linear in its length.
CLASS_LINE = re.compile(rb'[ \t\r]*"[A-Za-z_][A-Za-z0-9_]*"[ \t\r]*:[ \t\r]*"([^"\\\x00-\x1f]*)"[ \t\r]*(?:,[ \t\r]*)?')
CLASS_MARK = b"identifier-class-proof-mark"
ANY_INDEX = object()    # a list step of a key path: any index


class Usage(Exception):
    pass


class Failure(Exception):
    pass


def signature_led(head):
    """Whether a file that starts with head starts like a binary signature: the builds read no such
    file. The one place that asks it: of a member file, of bytes outside members, and of the files
    the scan itself writes for the builds (a file of runs, a run that is read alone)."""
    return any(head[off:off + len(m)] == m for off, m in BIN_MAGICS)


# What a build's report may give back of a file's name in another form than the disk has it.
PLAIN_NAME = re.compile(r"[^\x20-\x7e]+")


def lead_of(start, unit):
    """What the scan writes before bytes that begin with start, in a file it writes for the builds:
    nothing when the file would start like no binary signature, so that its first byte is the
    bytes' own; else unit, as often as it takes for the file to start like none. Once is not always
    enough: a signature can lie further in, where one unit moves other bytes to its place."""
    lead = b""
    while signature_led(lead + start):
        lead += unit
        if len(lead) > 4096:
            raise Failure("a file of runs starts like a binary signature whatever stands before it")
    return lead


def sha256_file(path):
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def need(cond, msg):
    if not cond:
        raise Usage(msg)


def tool_commit():
    """The commit of the checkout this tool runs from, when this file, the extractor and the class
    list (when there is one) are, byte for byte, what that commit holds; otherwise None, and no
    upload check accepts the report. Each file's own bytes are compared with the commit's: git's
    index is not asked, so a file it is told to overlook is still compared. The checkout is
    found from this file's directory: git variables of the caller's environment are not passed on,
    and git is told to read the commit's own objects, not objects a replace ref puts in their place."""
    environment = {key: value for key, value in os.environ.items() if not key.startswith("GIT_")}

    def git(*args):
        try:
            done = subprocess.run(["git", "--no-replace-objects", "-C", HERE, *args], stdout=subprocess.PIPE,
                                  stderr=subprocess.DEVNULL, stdin=subprocess.DEVNULL, env=environment, check=False)
        except OSError:
            return 1, ""
        return done.returncode, done.stdout.decode("utf-8", "replace").strip()
    code, head = git("rev-parse", "--verify", "HEAD^{commit}")
    files = [os.path.abspath(__file__), EXTRACTOR]
    if os.path.lexists(CLASSES_FILE):
        files.append(CLASSES_FILE)
    if code != 0 or not re.fullmatch(r"[0-9a-f]{40}([0-9a-f]{24})?", head):
        return None
    for path in files:
        tracked, name = git("ls-files", "--full-name", "--error-unmatch", "--", path)
        held = git("rev-parse", "--verify", "--quiet", "%s:%s" % (head, name)) if tracked == 0 else (1, "")
        here = git("hash-object", "--no-filters", "--", path)
        if held[0] != 0 or here[0] != 0 or not held[1] or held[1] != here[1]:
            return None
    return head


def load_json(path, what):
    """The document and the sha256 of the bytes it was read from."""
    try:
        with open(path, "rb") as f:
            raw = f.read()
        return json.loads(raw), hashlib.sha256(raw).hexdigest()
    except (OSError, ValueError) as exc:
        raise Usage("%s is not readable JSON (%s)" % (what, type(exc).__name__))


def entry(e, where, keys, texts):
    need(isinstance(e, dict) and set(e) <= keys | {"proof"}, where + ": unknown or missing structure")
    need(isinstance(e.get("proof", {}), dict), where + ": proof must be an object")
    for k in texts:
        need(isinstance(e.get(k), str) and e[k].strip(), where + ": %s must be a non-empty string" % k)
    need(isinstance(e.get("sha256"), str) and HEX64.match(e["sha256"]), where + ": sha256 must be 64 lower-hex")


def positive(value):
    return isinstance(value, int) and not isinstance(value, bool) and value >= 1


def validate_accepted(doc):
    need(isinstance(doc, dict) and doc.get("schema") == SCHEMA_ACCEPTED, "acceptance file: schema must be %s" % SCHEMA_ACCEPTED)
    need(set(doc) <= {"schema", "positions", "archives", "files", "unopened", "note"}, "acceptance file: unknown top-level key")
    pos, arch, files, unop = (doc.get(k, []) for k in ("positions", "archives", "files", "unopened"))
    need(all(isinstance(x, list) for x in (pos, arch, files, unop)), "acceptance file: positions, archives, files and unopened must be lists")
    for n, e in enumerate(pos):
        w = "acceptance positions[%d]" % n
        entry(e, w, {"rule", "path", "sha256", "line", "class", "reason", "accepted_by", "count"},
              ("rule", "path", "class", "reason", "accepted_by"))
        need(isinstance(e.get("line"), int) and not isinstance(e.get("line"), bool) and e["line"] >= 0, w + ": line must be an integer")
        need(positive(e.get("count", 1)), w + ": count must be a positive integer")
    for n, e in enumerate(arch):
        w = "acceptance archives[%d]" % n
        entry(e, w, {"path", "sha256", "index", "class", "reason", "accepted_by"}, ("path", "reason", "accepted_by"))
        need(e.get("index") in ("pypi", "crates.io", "npm"), w + ": index must be pypi, crates.io or npm")
        need(e.get("class") == "PUBLIC-UPSTREAM-ARCHIVE", w + ": class must be PUBLIC-UPSTREAM-ARCHIVE")
    for n, e in enumerate(files):
        w = "acceptance files[%d]" % n
        entry(e, w, {"path", "sha256", "class", "reason", "accepted_by"}, ("path", "reason", "accepted_by"))
        need(e.get("class") == "PUBLIC-UPSTREAM-FILE", w + ": class must be PUBLIC-UPSTREAM-FILE")
    for n, e in enumerate(unop):
        entry(e, "acceptance unopened[%d]" % n, {"path", "sha256", "reason", "accepted_by"}, ("path", "reason", "accepted_by"))
    return pos, arch, files, unop


def validate_pending(doc):
    need(isinstance(doc, dict) and doc.get("schema") == SCHEMA_PENDING, "pending list: schema must be %s" % SCHEMA_PENDING)
    need(set(doc) <= {"schema", "positions", "note"}, "pending list: unknown top-level key")
    pos = doc.get("positions", [])
    need(isinstance(pos, list), "pending list: positions must be a list")
    for n, e in enumerate(pos):
        w = "pending positions[%d]" % n
        entry(e, w, {"rule", "path", "line", "sha256", "note", "count"}, ("rule", "path"))
        need(isinstance(e.get("line"), int) and not isinstance(e.get("line"), bool), w + ": line must be an integer")
        need(positive(e.get("count", 1)), w + ": count must be a positive integer")
    return pos


def class_pattern(text, where, characters):
    """An asset or member pattern as a regular expression: <version> and <target> are places, and
    every other character is itself. A pattern of places alone is none: it holds a letter or a digit
    of its own."""
    need(isinstance(text, str) and re.fullmatch(characters, text), where + " is not a name with the places <version> and <target>")
    parts = re.split("(<[^<>]*>)", text)
    need(all(part in CLASS_PLACES if part.startswith("<") else "<" not in part and ">" not in part for part in parts),
         where + " holds a place other than <version> and <target>")
    need(any(re.search("[A-Za-z0-9]", part) for part in parts if part not in CLASS_PLACES),
         where + " holds no letter or digit of its own beside its places")
    return re.compile("".join(CLASS_PLACES[part] if part in CLASS_PLACES else re.escape(part) for part in parts))


def class_key_path(text, where):
    """A key path as its steps, keys and list steps: a.b[].c is ("a", "b", ANY_INDEX, "c")."""
    need(isinstance(text, str), where + ": key_path must be a string")
    path = []
    for step in text.split("."):
        found = re.fullmatch(r"([A-Za-z_][A-Za-z0-9_]*)((?:\[\])*)", step)
        need(found, where + ": key_path is keys joined by dots, a list step written [] after its key")
        path += [found.group(1)] + [ANY_INDEX] * (len(found.group(2)) // 2)
    need(path[-1] is not ANY_INDEX, where + ": key_path must end with the key of the value")
    return tuple(path)


def validate_classes(doc):
    """The entries of a class list, each made ready for the proof."""
    w = "identifier class list"
    need(isinstance(doc, dict) and doc.get("schema") == SCHEMA_CLASSES, w + ": schema must be %s" % SCHEMA_CLASSES)
    need(set(doc) == {"schema", "classes"} and isinstance(doc["classes"], list), w + ": classes must be a list, the only other key")
    out = []
    for n, e in enumerate(doc["classes"]):
        where = "%s: classes[%d]" % (w, n)
        need(isinstance(e, dict) and set(e) == {"rule", "asset", "member", "key_path", "value_form", "accepted_by"},
             where + ": unknown or missing structure")
        need(isinstance(e["rule"], str) and re.fullmatch(r"[A-Za-z0-9._-]+", e["rule"]), where + ": rule must be a rule id")
        need(isinstance(e["accepted_by"], str) and e["accepted_by"].strip(), where + ": accepted_by must be a non-empty string")
        need(isinstance(e["value_form"], str) and e["value_form"] in CLASS_FORMS,
             where + ": value_form must be one of: " + ", ".join(sorted(CLASS_FORMS)))
        need(isinstance(e["member"], str) and all(part not in ("", ".", "..") for part in re.split("[/!]", e["member"])),
             where + ": member must be a path inside the payload")
        out.append({"rule": e["rule"], "asset": class_pattern(e["asset"], where + ": asset", r"[A-Za-z0-9._+<>-]+"),
                    "member": class_pattern(e["member"], where + ": member", r"[A-Za-z0-9._+<>!/-]+"),
                    "path": class_key_path(e["key_path"], where), "form": CLASS_FORMS[e["value_form"]]})
    need(len({json.dumps(e, sort_keys=True) for e in doc["classes"]}) == len(out), w + ": an entry is listed twice")
    return out


def class_document(raw):
    """The JSON document of these bytes. ValueError when they are not one document in UTF-8, when an
    object holds a key twice, or when they hold a constant JSON does not have."""
    def pairs(items):
        value = dict(items)
        if len(value) != len(items):
            raise ValueError("a key twice")
        return value

    def constant(name):
        raise ValueError(name)

    return json.loads(raw.decode("utf-8"), object_pairs_hook=pairs, parse_constant=constant)


def class_differences(before, after, path, found):
    """Every place where two documents differ, added to found as (key path, value before, value after)."""
    if isinstance(before, dict) and isinstance(after, dict) and list(before) == list(after):
        for key in before:
            class_differences(before[key], after[key], path + (key,), found)
    elif isinstance(before, list) and isinstance(after, list) and len(before) == len(after):
        for one, other in zip(before, after):
            class_differences(one, other, path + (ANY_INDEX,), found)
    elif type(before) is not type(after) or before != after:
        found.append((path, before, after))


@functools.lru_cache(maxsize=8)
def class_fields(raw):
    """What this tool proves of a member's bytes: {line: (bytes of the line, key path of its value, the
    value)} for every line that holds one field and its text value and nothing else. Nothing when
    the bytes are not one JSON document.

    The proof is by reading the member twice: once as it is, and once with every such value replaced
    by a mark of its line. Both must be JSON documents, the same but for the marked values; where
    the mark of a line stands in the second is the key path of that line's value in the first."""
    marks, marked = {}, []
    for number, text in enumerate(raw.split(b"\n"), start=1):
        pair = CLASS_LINE.fullmatch(text)
        if pair is not None:
            mark = CLASS_MARK + b"%d" % number
            marks[mark.decode("ascii")] = (number, len(text), pair.group(1))
            text = text[:pair.start(1)] + mark + text[pair.end(1):]
        marked.append(text)
    found, fields = [], {}
    try:
        class_differences(class_document(raw), class_document(b"\n".join(marked)), (), found)
        for path, _, after in found:
            number, length, value = marks[after]
            fields[number] = (length, path, value)
    except (ValueError, RecursionError, KeyError, TypeError):
        return {}
    return fields


def class_proves(entry, raw, line, spans):
    """Whether this tool proves that a position, a line of these member bytes with the spans the builds
    report for its findings (end line, start column, end column), is the entry's identifier field
    and nothing else. The numbers are those of the tool's docstring."""
    # (1, 2) The member is one JSON document and the line holds one field and its text value.
    field = class_fields(raw).get(line) if type(line) is int else None
    if field is None or not spans:
        return False
    length, path, value = field
    # (4) Every finding starts and ends on the line, at columns of the line. A build counts the columns
    # of a later line from 2, so the column after the last byte is still the line's.
    for span in spans:
        if not all(type(number) is int for number in span) or span[0] != line \
                or not all(1 <= column <= length + 1 for column in span[1:]):
            return False
    # (2) The value sits at exactly the declared key path. (3) The whole value is of the declared form.
    return path == entry["path"] and entry["form"].fullmatch(value) is not None


def class_resolves(entry, asset, rule, path, rec, line):
    """(5) Rule, payload file name and member path are the entry's, and every finding of the position
    lies in the one file that is read (two files of a payload that would print as one path are not
    one member); the rest is proved on that file's bytes."""
    spans = rec.get("spans", ())
    if entry["rule"] != rule or entry["asset"].fullmatch(asset) is None or entry["member"].fullmatch(path) is None \
            or any(span[0] != rec["member"] for span in spans):
        return False
    with open(rec["member"], "rb") as f:
        return class_proves(entry, f.read(), line, [span[1:] for span in spans])


def load_classes():
    """(the entries of the committed class list, the sha256 of its bytes); no entry and None without
    the file. A list that is not one JSON document, or holds a key twice, is no list."""
    if not os.path.lexists(CLASSES_FILE):
        return (), None
    try:
        with open(CLASSES_FILE, "rb") as f:
            raw = f.read()
        doc = class_document(raw)
    except (OSError, ValueError, RecursionError) as exc:
        raise Usage("identifier class list is not readable JSON without a repeated key (%s)" % type(exc).__name__)
    return validate_classes(doc), hashlib.sha256(raw).hexdigest()


def load_extractor():
    with open(EXTRACTOR, "rb") as f:
        raw = f.read()
    module = types.ModuleType("release_payload_deep_extract")
    module.__file__ = EXTRACTOR
    exec(compile(raw, EXTRACTOR, "exec"), module.__dict__)
    return module


def chain_of(rel, outdirs):
    """Tree-relative path -> the path with "!" at archive boundaries, and the enclosing archives
    as (tree-relative path of the archive file, its path with "!")."""
    comps = [c for c in rel.split("/") if c]
    chain, cur, prefix, ancestors = [], [], [], []
    for comp in comps:
        prefix.append(comp)
        cur.append(comp)
        key = "/".join(prefix)
        if key in outdirs:
            cur[-1] = os.path.basename(outdirs[key])
            chain.append("/".join(cur))
            ancestors.append((outdirs[key], "!".join(chain)))
            cur = []
    if cur:
        chain.append("/".join(cur))
    return "!".join(chain), ancestors


def strip_allowlist(default_text):
    """Remove global path-allowlist lines that mention vendor, dist-info or lock (lock files).
    Returns (text, removed)."""
    out, removed = [], 0
    in_global = in_paths = False
    for ln in default_text.splitlines(keepends=True):
        s = ln.strip()
        if s.startswith("["):
            in_global = s in ("[allowlist]", "[[allowlists]]")
            in_paths = False
        elif in_global and re.match(r"^paths\s*=\s*\[", s):
            in_paths = not s.endswith("]")
        elif in_global and in_paths:
            if s.startswith("]"):
                in_paths = False
            elif "vendor" in s or "dist-info" in s or "lock" in s:
                removed += 1
                continue
        out.append(ln)
    return "".join(out), removed


def global_paths(cfg_text):
    try:
        cfg = tomllib.loads(cfg_text)
    except tomllib.TOMLDecodeError:
        raise Failure("a default rules file is not TOML")
    pats = list((cfg.get("allowlist") or {}).get("paths", []))
    for extra in cfg.get("allowlists", []) or []:
        if not extra.get("targetRules"):
            pats.extend(extra.get("paths", []))
    out = []
    for p in pats:
        try:
            out.append(re.compile(p))
        except re.error:
            raise Failure("a detector path-allowlist pattern cannot be evaluated by this tool")
    return out


class Build:
    def __init__(self, role, exe, label, default_cfg):
        self.role, self.exe, self.label, self.default_cfg = role, exe, label, default_cfg
        self.sha256 = sha256_file(exe)
        self.dir_mode = False
        self.cfg = self.default_copy = None
        self.removed = 0
        # The global path allowlist of the rules the build scans with (the removed lines left out), and of
        # its default rules file as it is: what the build's own default passes over.
        self.patterns = []
        self.default_patterns = []


def run_detector(build, source, config, report, logdir, tag, empty_cwd):
    """One run of a build over the directory source. Returns its findings (redacted by the build)."""
    if build.dir_mode:
        cmd = [build.exe, "dir", source]
    else:
        cmd = [build.exe, "detect", "--no-git", "--source", source]
    if config:
        cmd += ["--config", config]
    cmd += ["--redact=100", "--no-banner", "--no-color", "--log-level", "error", "--ignore-gitleaks-allow",
            "--report-format", "json", "--report-path", report, "--exit-code", str(LEAK_EXIT)]
    env = {k: v for k, v in os.environ.items() if k not in ("GITLEAKS_CONFIG", "GITLEAKS_CONFIG_TOML")}
    so, se = os.path.join(logdir, tag + ".stdout"), os.path.join(logdir, tag + ".stderr")
    fo = os.open(so, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    fe = os.open(se, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    try:
        rc = subprocess.run(cmd, stdout=fo, stderr=fe, stdin=subprocess.DEVNULL, cwd=empty_cwd, env=env, check=False).returncode
    finally:
        os.close(fo)
        os.close(fe)
    if os.path.exists(report):
        os.chmod(report, 0o600)
    if rc not in (0, LEAK_EXIT) or not os.path.exists(report) or os.path.getsize(se) != 0:
        raise Failure("the %s build's run %s did not finish cleanly (exit %d)" % (build.role, tag, rc))
    with open(report, "rb") as f:
        data = json.load(f) or []
    if not isinstance(data, list) or not all(isinstance(item, dict) for item in data):
        raise Failure("the %s build's run %s wrote no list of findings" % (build.role, tag))
    return data


def number(item, key):
    value = item.get(key, 0)
    if isinstance(value, bool) or not isinstance(value, int):
        raise Failure("a finding without a line number")
    return value


def prepare_builds(args):
    builds = []
    for role, given, cfg, label in (("pinned", args.gitleaks_pinned, args.default_config_pinned, args.pinned_version),
                                    ("current", args.gitleaks_current, args.default_config_current, args.current_version)):
        need(os.path.isabs(given), "the %s build must be given by absolute path" % role)
        exe = os.path.realpath(given)
        need(os.path.isfile(exe) and os.access(exe, os.X_OK), "the %s build is not an executable file" % role)
        if not cfg:
            cfg = os.path.join(os.path.dirname(given), "gitleaks.toml")
            need(os.path.isfile(cfg), "no gitleaks.toml beside the %s build: pass --default-config-%s" % (role, role))
        need(os.path.isfile(cfg), "the default rules file of the %s build was not found" % role)
        if not label:
            out = subprocess.run([exe, "version"], stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, stdin=subprocess.DEVNULL,
                                 cwd="/", check=False).stdout.decode("utf-8", "replace").strip()
            label = out if re.fullmatch(r"v?[0-9]+\.[0-9]+\.[0-9]+", out) else "unknown-" + sha256_file(exe)[:12]
        need(re.fullmatch(r"[A-Za-z0-9._-]{1,40}", label), "a version label is not printable")
        builds.append(Build(role, exe, label.lstrip("v"), cfg))
    need(builds[0].sha256 != builds[1].sha256, "the pinned and the current build are the same file")
    return builds


def receipt_path(path):
    """A member's path as the receipt prints it. The receipt is a text that a person reads: every path
    has a form of its own there and stands on one line. The escape sign is written twice; a byte
    that is no UTF-8 (Python carries it as a lone surrogate) as \\xNN of the byte; the sign of a
    table's column and every control character of ASCII, a line break among them, as \\xNN; other
    characters that end a line or cannot be written, as \\uNNNN. The listing and the report hold
    the path itself."""
    out = []
    for char in path:
        code = ord(char)
        if char == "\\":
            out.append("\\\\")
        elif 0xdc80 <= code <= 0xdcff:
            out.append("\\x%02x" % (code - 0xdc00))
        elif code < 0x20 or code == 0x7f or char == "|":
            out.append("\\x%02x" % code)
        elif 0x80 <= code <= 0x9f or code in (0x2028, 0x2029) or 0xd800 <= code <= 0xdfff:
            out.append("\\u%04x" % code)
        else:
            out.append(char)
    return "".join(out)


def write_new(path, text):
    # A path of a member can hold bytes that are no UTF-8 (Python carries them as lone surrogates): in a
    # text they are written as their escapes, never as a failure of the write.
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(fd, "w", encoding="utf-8", errors="backslashreplace") as f:
        f.write(text)


class Scan:
    """The state of one run: what the builds found and what nothing read."""

    def __init__(self, work, builds, extractor):
        self.work, self.builds, self.extractor = work, builds, extractor
        self.empty_cwd = os.path.join(work, "empty-cwd")
        self.positions = collections.OrderedDict()
        self.payloads = []
        self.unopened = []     # (payload, path, format, sha256, ancestors, tree)
        self.unread = []       # (payload, path, reason, sha256, ancestors, tree)
        self.skipped = []      # (payload, archive path, sha256, reason, count, ancestors, tree)
        self.artifacts = []
        self.unread_reasons = collections.Counter()
        self.elf_files = 0
        self.sha_cache = {}
        # Proof of read: the size of each file that carries the proof-of-read line while the builds run,
        # the rule ids each build reports for that line alone, and the counts per build.
        self.seals = {}
        self.proof_ids = {}
        self.read_proof = collections.OrderedDict(
            (b.label, collections.OrderedDict((key, 0) for key in (
                "handed", "read_as_it_is", "read_behind_the_line", "unread", "readings", "proof_readings"))) for b in builds)

    def fsha(self, path):
        if path not in self.sha_cache:
            self.sha_cache[path] = sha256_file(path)
        return self.sha_cache[path]

    def default_rules_proved(self, b, root, rdir, tag):
        """The default rules file given for a build must be the build's own default: over root, a run
        with the embedded default and a run with the given file must report the same. It is asked of
        every directory a build reads with the given rules (a payload's scan root, the files of it
        that only the removed allowlist lines would skip, the bytes outside members, the runs of its
        compiled files, the runs read alone): a file that lacks a rule gives the default's result
        wherever that rule has no hit, and wherever both runs pass over the file that holds it."""
        def keyset(items):
            return collections.Counter((str(x.get("RuleID")), str(x.get("File")), x.get("StartLine")) for x in items)
        d1 = run_detector(b, root, None, os.path.join(rdir, "%s%s-embedded-default.report.json" % (b.role, tag)), rdir,
                          b.role + tag + "-embedded-default", self.empty_cwd)
        d2 = run_detector(b, root, b.default_copy, os.path.join(rdir, "%s%s-explicit-default.report.json" % (b.role, tag)), rdir,
                          b.role + tag + "-explicit-default", self.empty_cwd)
        if keyset(d1) != keyset(d2):
            raise Failure("the default rules file given for the %s build does not reproduce its embedded default" % b.role)

    def removed_paths(self, b, paths, pdir):
        """The files of the tree that a build reads only because the scan removed path allowlist lines
        (vendor, dist-info, lock files) from the rules it scans with. The build's own default passes
        over them where they lie, with the embedded default and with the given file alike, so the two
        runs of default_rules_proved over the scan root agree about them whatever the given file
        lacks. Here they are given to the build once more from a directory of their own, under
        numbered names that none of those lines matches (own_file stops the scan when one does), for
        default_rules_proved to ask there. paths are the files the build is handed. Returns the
        directory, or None when there is no such file. The names are links: the same bytes, no second
        copy on the disk."""
        only = [path for path in paths if any(rx.search(path) for rx in b.default_patterns)]
        if not only:
            return None
        root = os.path.join(pdir, "removed-paths-" + b.role)
        os.makedirs(os.path.join(root, "t"), mode=0o700)
        for number_, path in enumerate(only):
            os.link(path, self.own_file(os.path.join(root, "t", "%05d.txt" % number_)))
        return root

    def own_file(self, place):
        """The place of a file the scan writes for the builds under a numbered name of its own (bytes
        outside members, the runs of a compiled file, a run that is read alone, a file of a removed
        allowlist line). A build's path allowlist that matches it would leave the file read by no
        build, and nothing would say so: every such place is asked about, and a match stops the scan.
        The removed lines are asked too: the build's embedded default has them, and a run with it
        that passes over the file proves nothing about the rules file."""
        if any(rx.search(place) for b in self.builds for rx in b.patterns + b.default_patterns):
            raise Usage("a detector path-allowlist pattern matches a file of the scan's own in the work directory")
        return place

    def sealed(self, path):
        """Put the proof-of-read line at the end of a file that is handed to a build, on a line of its
        own, and give the number of that line. The file's own size is kept: unsealed cuts the line
        off again, so that every later reader and every digest sees the file's own bytes."""
        lines, last = 0, b"\n"
        with open(path, "r+b") as f:
            for block in iter(lambda: f.read(1 << 20), b""):
                lines += block.count(b"\n")
                last = block[-1:]
            self.seals[path] = f.tell()
            f.write((b"" if last == b"\n" else b"\n") + PROOF_LINE)
        return lines + (last != b"\n") + 1

    def unsealed(self):
        """Cut every proof-of-read line off again. What is cut must be that line and nothing else."""
        for path, size in self.seals.items():
            with open(path, "r+b") as f:
                f.seek(size)
                if f.read() not in (PROOF_LINE, b"\n" + PROOF_LINE):
                    raise Failure("a file of the scan tree is not as the scan left it")
                f.truncate(size)
        self.seals.clear()

    def proof_rules(self, b):
        """The rule ids a build reports for the proof-of-read line alone, each with its count. Measured
        once per build, with the rules the build scans with: nothing of the scan says which rule of a
        build reports the line. A build that does not report it can prove no read: the scan stops."""
        if b.role not in self.proof_ids:
            root, logs = os.path.join(self.work, "proof-" + b.role), os.path.join(self.work, "proof-" + b.role + "-reports")
            os.makedirs(os.path.join(root, "t"), mode=0o700)
            os.makedirs(logs, mode=0o700)
            write_new(self.own_file(os.path.join(root, "t", "00000.txt")), PROOF_LINE.decode())
            data = run_detector(b, root, b.cfg, os.path.join(logs, "report.json"), logs, "proof", self.empty_cwd)
            ids = collections.Counter(str(it.get("RuleID")) for it in data if number(it, "StartLine") == 1)
            if not ids or sum(ids.values()) != len(data):
                raise Failure("the %s build does not report the scan's proof-of-read line at its line" % b.role)
            self.proof_ids[b.role] = ids
        return self.proof_ids[b.role]

    def reading(self, b, root, rdir, tag, proof=False, names=None):
        """One reading of a directory by a build, with the rules it scans with. Counted per build:
        the readings of files as they are, and the readings with the proof-of-read line.

        names is given for the tree of the members, whose files have their own names (tree_names):
        each reported path is set to the file of the tree it is, or the scan stops. No finding is
        made a position at a path the tree does not hold, and none is dropped."""
        self.read_proof[b.label]["proof_readings" if proof else "readings"] += 1
        data = run_detector(b, root, b.cfg, os.path.join(rdir, "%s%s.report.json" % (b.role, tag)), rdir, b.role + tag, self.empty_cwd)
        if names is not None:
            exact, loose = names
            for it in data:
                path = str(it.get("File", ""))
                if path not in exact:
                    if PLAIN_NAME.sub("", path) not in loose:
                        raise Failure("the %s build reports a finding at a path that is no file of the directory it read" % b.role)
                    it["File"] = loose[PLAIN_NAME.sub("", path)]
        return data

    @staticmethod
    def tree_names(files):
        """How the files of the tree are known in a build's report, made before any build reads the
        tree. A report gives a path back as the disk has it when the path is UTF-8. A name that is
        not (a tar member can carry any bytes) comes back in another form, with other characters in
        the place of those bytes: such a file is known by its path without everything that is not
        printable ASCII, and that form must be its own. When another file of the tree has the same
        form, a report could mean either, and the scan stops before a build runs. Returns the set of
        the paths, and the files that are known by their form.

        This takes it that a build gives back every printable ASCII character of a path and adds
        none. A build that does otherwise never makes such a file pass (its path fits no file, or
        the file shows no proof of read), but it may show a refused position at a sibling with a
        valid name."""
        forms, loose = collections.Counter(PLAIN_NAME.sub("", path) for path in files), {}
        for path in files:
            try:
                path.encode("utf-8")
            except UnicodeEncodeError:
                if forms[PLAIN_NAME.sub("", path)] != 1:
                    raise Failure("two files of a payload cannot be told apart in a build's report (a name that is no UTF-8)")
                loose[PLAIN_NAME.sub("", path)] = path
        return files, loose

    @staticmethod
    def folded(data, more):
        """The findings of two readings of the same files as one list: a finding of both readings,
        the same by file, line and rule, counts once."""
        def key(it):
            return str(it.get("File", "")), str(it.get("RuleID")), number(it, "StartLine")
        have, seen = collections.Counter(key(it) for it in data), collections.Counter()
        for it in more:
            seen[key(it)] += 1
            if seen[key(it)] > have[key(it)]:
                data.append(it)
        return data

    def proven(self, b, data, root, lines, wanted, pdir, rdir, tag, names=None):
        """The second reading of a directory by a build, and the proof of read. data are the findings
        of the first reading, of the files as they are: that reading keeps what every rule means,
        at the first byte and at the end of the text too. Now each file of lines carries the
        proof-of-read line as its last line (lines gives the line's number), and root is read again;
        wanted are the files the build was handed (it may pass over the others for their path).

        A file is proven read when the build reports the line there exactly as it reports the line
        alone: the same rule ids, each as often, starting at that line (the line is the file's last,
        so a finding that starts on it lies in it whatever end the build states); those findings are the scan's own
        and leave the findings. The proof of this reading speaks for the first: a build takes or
        passes over a file for its path and its leading bytes, and those are the same in both. A
        handed file without proof is read again behind a line of plain words (as many as lead_of
        asks), in a directory of its own, twice in the same way; its findings come back at their
        own file and line. Returns the findings of all readings, each once, and the handed files
        that showed their proof line in no reading."""
        ids, counts = self.proof_rules(b), self.read_proof[b.label]

        def split(items, where):
            seen = collections.defaultdict(collections.Counter)
            at_line = []
            for it in items:
                path = str(it.get("File", ""))
                at_line.append(path in where and number(it, "StartLine") == where[path])
                if at_line[-1]:
                    seen[path][str(it.get("RuleID"))] += 1
            good = {path for path in where if seen[path] == ids}
            return good, [it for it, own in zip(items, at_line) if not (own and str(it.get("File", "")) in good)]

        good, more = split(self.reading(b, root, rdir, tag + "-proof", True, names), lines)
        data = self.folded(data, more)
        again = sorted(path for path in wanted if path not in good)
        counts["handed"] += len(wanted)
        counts["read_as_it_is"] += len(wanted) - len(again)
        if not again:
            return data, []
        behind = os.path.join(pdir, "again-%s%s" % (b.role, tag))
        os.makedirs(os.path.join(behind, "t"), mode=0o700)
        back = {}
        for number_, path in enumerate(again):
            place = self.own_file(os.path.join(behind, "t", "%05d.txt" % number_))
            with open(path, "rb") as src, open(os.open(place, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600), "wb") as dst:
                lead = PLAIN_LINE + lead_of(PLAIN_LINE + src.read(600), PLAIN_LINE)
                src.seek(0)
                dst.write(lead)
                shutil.copyfileobj(src, dst)
                dst.truncate(len(lead) + self.seals[path])      # the file's own bytes, without the proof line
            back[place] = (path, len(lead) // len(PLAIN_LINE))
        first = self.reading(b, behind, rdir, tag + "-again")
        self.default_rules_proved(b, behind, rdir, tag + "-again")
        where = {place: self.sealed(place) for place in back}
        good, more = split(self.reading(b, behind, rdir, tag + "-again-proof", True), where)
        counts["read_behind_the_line"] += len(good)
        counts["unread"] += len(again) - len(good)
        mapped = []
        for it in self.folded(first, more):
            if str(it.get("File", "")) not in back:
                raise Failure("a finding lies outside the directory of the files that are read again")
            path, shift = back[str(it.get("File", ""))]
            own = number(it, "StartLine") - shift
            if own < 1:
                raise Failure("a finding in a line of plain words of the scan's own")
            mapped.append(dict(it, File=path, StartLine=own, EndLine=max(own, number(it, "EndLine") - shift)))
        return self.folded(data, mapped), [path for place, (path, _shift) in back.items() if place not in good]

    def payload(self, i, payload):
        """One payload file (read_payload). Whatever happens there, no copy of the scan keeps the
        proof-of-read line: a work directory that is kept after a failure holds the files' own bytes."""
        try:
            self.read_payload(i, payload)
        finally:
            self.unsealed()

    def read_payload(self, i, payload):
        builds, D = self.builds, self.extractor
        pdir = os.path.join(self.work, "p%d" % i)
        scan_root = os.path.join(pdir, "scan")
        os.makedirs(scan_root, mode=0o700)
        tree = os.path.join(scan_root, "t")
        # The bytes outside archive members are read from a directory of their own, under names that hold
        # nothing of a payload; other such bytes wait in a third while the builds run (below).
        oroot, held = os.path.join(pdir, "outside"), os.path.join(pdir, "outside-held")
        for b in builds:
            if any(rx.search(tree + "/x") for rx in b.patterns + b.default_patterns):
                raise Usage("the work directory matches a detector path-allowlist pattern; choose another --work")
        try:
            stats = D.expand_file(payload, tree)
        except D.ExtractError:
            raise Failure("deep extraction stopped (disk guard, size cap or output location)")
        base = os.path.basename(payload)
        digest = stats["sha256"]    # of the copy that was opened and scanned, not of a later read of the file
        self.sha_cache[os.path.join(tree, base)] = digest
        outdirs = {}
        for rec in stats["archives"]:
            d = os.path.dirname(rec["path"])
            outdirs[os.path.join(d, rec["outdir"]) if d else rec["outdir"]] = rec["path"]
        top = next((rec for rec in stats["archives"] if rec["path"] == base), None)
        top_dir = None
        if top is not None:
            # The payload is an opened archive: its members stand for it, at paths below its top directory.
            os.unlink(os.path.join(tree, base))
            inner = os.path.join(tree, top["outdir"])
            names = [name for name in os.listdir(inner) if name != D.OUTSIDE]
            if len(names) == 1 and os.path.isdir(os.path.join(inner, names[0])) and not os.path.islink(os.path.join(inner, names[0])):
                top_dir = names[0]

        def strip(cpath):
            segs = cpath.split("!")
            if top is not None and len(segs) > 1 and segs[0] == base:
                segs = segs[1:]
                if top_dir and segs[0].startswith(top_dir + "/"):
                    segs[0] = segs[0][len(top_dir) + 1:]
            return "!".join(segs)

        def canon(rel):
            """The path inside the payload, and the enclosing archives (the payload file last out)."""
            cpath, ancestors = chain_of(rel, outdirs)
            return strip(cpath), [(arel, acanon if arel == base else strip(acanon)) for arel, acanon in ancestors]

        closed = {rec["path"] for rec in stats["not_opened"] + stats["partial"]}
        for rec in stats["not_opened"] + stats["partial"]:
            cpath, ancestors = canon(rec["path"])
            self.unopened.append((i, cpath, rec.get("format", "?"), self.fsha(os.path.join(tree, rec["path"])), ancestors, tree))
        skips = collections.Counter()
        for rec in stats["archives"]:
            for key, count in rec.get("skips", {}).items():
                skips[key] += count
                reason = key.split(":", 1)[-1]
                if reason not in NO_CONTENT_SKIPS and rec["path"] not in closed:
                    cpath, ancestors = canon(rec["path"])
                    self.skipped.append((i, cpath, self.fsha(os.path.join(tree, rec["path"])), reason, count, ancestors, tree))
        opened = {rec["path"] for rec in stats["archives"]} | closed

        # What no build reads, and the printable runs of the compiled files.
        sroot = os.path.join(pdir, "strings")
        os.makedirs(os.path.join(sroot, "t"), mode=0o700)
        elf_by_sha, elf_by_idx = {}, {}
        files = 0
        # The files of an archive's bytes outside its members: no member, so no acceptance speaks for
        # them. Those the builds read (apart) and those read by their printable runs (binary).
        apart, binary = [], []
        # Proof of read: the builds each file of the tree is handed to, the line of the proof-of-read
        # line in each file of runs, and the files that showed that line to a build in no reading.
        handed, runs_files, unproven, tree_files = {}, [], collections.defaultdict(set), set()
        for dirpath, dirnames, filenames in os.walk(tree, followlinks=False):
            dirnames.sort()
            for fn in sorted(filenames):
                p = os.path.join(dirpath, fn)
                if os.path.islink(p) or not os.path.isfile(p):
                    continue
                files += 1
                tree_files.add(p)
                rel = os.path.relpath(p, tree)
                if rel in opened or os.path.getsize(p) == 0:
                    continue
                with open(p, "rb") as f:
                    head = f.read(600)
                outside = os.path.basename(dirpath) == D.OUTSIDE
                if head[:4] == b"\x7fELF" or (outside and signature_led(head)):
                    # A compiled file, and bytes outside members that start with a binary signature: the
                    # builds do not read them, so their printable runs are read.
                    esha = self.fsha(p)
                    if esha not in elf_by_sha:
                        with open(p, "rb") as f:
                            body = f.read()
                        offs, idx = [], len(elf_by_sha)
                        fd = os.open(self.own_file(os.path.join(sroot, "t", "%05d.txt" % idx)), os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
                        with os.fdopen(fd, "wb") as f:
                            # The file is the runs, one on a line, from its first byte: a rule that reports
                            # only at the first byte of a file can report the first run. Only when the file
                            # would start like a binary signature (a build reads no such file, and every run
                            # would be lost) lines of plain words stand first (lead_of), each a run at no
                            # place of the file. Every other line is a run, and a position is that run's
                            # own place.
                            start = b""
                            for run in STRINGS_RX.finditer(body):
                                start += run.group(0)[:600] + b"\n"
                                if len(start) >= 600:
                                    break
                            lead = lead_of(start, PLAIN_LINE)
                            offs.extend([0] * (len(lead) // len(PLAIN_LINE)))
                            f.write(lead)
                            for run in STRINGS_RX.finditer(body):
                                offs.append(run.start())
                                f.write(run.group(0) + b"\n")
                        del body
                        runs_files.append(os.path.join(sroot, "t", "%05d.txt" % idx))
                        elf_by_sha[esha] = {"idx": idx, "offs": offs, "paths": []}
                        elf_by_idx[idx] = esha
                    elf_by_sha[esha]["paths"].append(rel)
                    self.elf_files += head[:4] == b"\x7fELF"
                    if outside:
                        binary.append(rel)
                    continue
                if outside:
                    apart.append(rel)
                    continue
                if signature_led(head):
                    reason = "binary signature (not an opened archive)"
                else:
                    hit = [any(rx.search(p) for rx in b.patterns) for b in builds]
                    handed[p] = [b for b, passed_over in zip(builds, hit) if not passed_over]
                    if not any(hit):
                        continue
                    # The rule is per build: a file one build skips is unread by that build.
                    reason = "on both builds' remaining path allowlists" if all(hit) else \
                        "on one build's remaining path allowlist (the other build reads it)"
                self.unread_reasons[reason] += 1
                cpath, ancestors = canon(rel)
                self.unread.append((i, cpath, reason, self.fsha(p), ancestors, tree))

        # While the builds run, the bytes outside members are not in the scan tree, where a build's path
        # allowlist can skip them for the path of their archive: those the builds read lie under
        # numbered names in a directory of their own, the binary ones wait aside. Each is put back at
        # its place afterwards, where its digest is taken.
        places = [(rel, self.own_file(os.path.join(oroot, "t", "%05d.txt" % number))) for number, rel in enumerate(apart)] \
            + [(rel, os.path.join(held, "%05d" % number)) for number, rel in enumerate(binary)]
        os.makedirs(os.path.join(oroot, "t"), mode=0o700)
        os.makedirs(held, mode=0o700)
        for rel, place in places:
            os.rename(os.path.join(tree, rel), place)
        rel_of = {place: rel for rel, place in places[:len(apart)]}
        # The files of the tree while the builds read it (those outside members lie elsewhere now), and
        # how a build's report names them: made before any build reads the tree.
        tree_names = self.tree_names(tree_files - {os.path.join(tree, rel) for rel, _place in places})

        rdir = os.path.join(pdir, "reports")
        os.makedirs(rdir, mode=0o700)
        raw = collections.OrderedDict()
        # Every file is read twice by each build. The first reading, here: each file as it is. This is
        # the reading that keeps what every rule means, at the first byte and at the end of the text.
        first = {}
        for b in builds:
            data = self.reading(b, scan_root, rdir, "", names=tree_names)
            self.default_rules_proved(b, scan_root, rdir, "")
            # The build's own default passes over the paths of the removed allowlist lines, in both runs
            # of that comparison: the files the scan reads there are proved from a directory of their own.
            aside = self.removed_paths(b, sorted(path for path, readers in handed.items() if b in readers), pdir)
            if aside:
                self.default_rules_proved(b, aside, rdir, "-removed-paths")
            more = self.reading(b, oroot, rdir, "-outside") if apart else []
            if apart:
                self.default_rules_proved(b, oroot, rdir, "-outside")
            # The printable runs of the ELF files. Pass 1 reads the dumps, one run per line (fast). A rule can
            # join two neighbouring runs across the line break, so pass 2 reads EVERY run a pass-1 finding
            # touches as a file of its own, and only pass-2 findings count. A pass-1 finding that no single
            # run reproduces is recorded as a method artifact.
            sdata = self.reading(b, sroot, rdir, "-strings")
            if elf_by_sha:
                self.default_rules_proved(b, sroot, rdir, "-strings")
            first[b.role] = (data, more, sdata)
        # The second reading: every file that is handed to a build ends with the proof-of-read line.
        tree_lines = {path: self.sealed(path) for path, readers in handed.items() if readers}
        out_lines = {place: self.sealed(place) for place in rel_of}
        runs_lines = {path: self.sealed(path) for path in runs_files}

        def of_runs(path):
            """The compiled files whose runs a numbered file of the scan holds."""
            return elf_by_sha[elf_by_idx[int(os.path.basename(path)[:5])]]["paths"]

        for b in builds:
            data, more, sdata = first.pop(b.role)
            data, lacking = self.proven(b, data, scan_root, tree_lines, [path for path in tree_lines if b in handed[path]], pdir, rdir, "", tree_names)
            raw[b.label] = len(data)
            for path in lacking:
                unproven[os.path.relpath(path, tree)].add(b.label)
            if apart:
                more, lacking = self.proven(b, more, oroot, out_lines, list(out_lines), pdir, rdir, "-outside")
                raw[b.label + " bytes outside members"] = len(more)
                for place in lacking:
                    unproven[rel_of[place]].add(b.label)
                for it in more:
                    found = re.search(r"/t/(\d{5})\.txt$", str(it.get("File", "")))
                    if not found or int(found.group(1)) >= len(apart):
                        raise Failure("a finding in the bytes outside members lies outside their directory")
                    it["File"] = os.path.join(tree, apart[int(found.group(1))])    # its place in the tree
                data += more
            del more
            for it in data:
                fpath = str(it.get("File", ""))
                if not fpath.startswith(tree + "/") or fpath not in tree_files:     # a position is at a file of the tree
                    raise Failure("a finding lies outside the scan tree")
                rel = fpath[len(tree) + 1:]
                cpath, ancestors = canon(rel)
                key = (i, str(it.get("RuleID", "?")), cpath, number(it, "StartLine"))
                rec = self.positions.setdefault(key, {"member": fpath, "ancestors": ancestors, "tree": tree,
                                                      "per_build": collections.Counter(), "strings": False, "spans": []})
                rec["per_build"][b.label] += 1
                rec["spans"].append((fpath, *(it.get(name) for name in ("EndLine", "StartColumn", "EndColumn"))))
            del data
            if runs_lines:
                sdata, lacking = self.proven(b, sdata, sroot, runs_lines, list(runs_lines), pdir, rdir, "-strings")
                for path in lacking:
                    for erel in of_runs(path):
                        unproven[erel].add(b.label)
            raw[b.label + " ELF strings (runs joined by line)"] = len(sdata)
            touched = collections.defaultdict(set)
            joined = []
            for it in sdata:
                mfile = re.search(r"/(\d{5})\.txt$", str(it.get("File", "")))
                if not mfile or int(mfile.group(1)) not in elf_by_idx:
                    raise Failure("a strings finding lies outside the dump directory")
                idx = int(mfile.group(1))
                nruns = len(elf_by_sha[elf_by_idx[idx]]["offs"])
                sl = number(it, "StartLine")
                el = max(sl, number(it, "EndLine"))
                if sl < 1 or sl > nruns:
                    raise Failure("a strings finding has no run")
                lo, hi = max(1, sl - 2), min(nruns, el + 2)
                if hi - lo > 20000:
                    raise Failure("a strings finding spans too many runs to isolate")
                touched[idx].update(range(lo, hi + 1))
                joined.append((idx, str(it.get("RuleID", "?")), sl, lo, hi))
            del sdata
            iso_root = os.path.join(pdir, "strings-isolated-" + b.role)
            os.makedirs(os.path.join(iso_root, "t"), mode=0o700)
            alone = []
            for idx, lines in touched.items():
                with open(os.path.join(sroot, "t", "%05d.txt" % idx), "rb") as f:
                    all_lines = f.read().split(b"\n")
                for ln in sorted(lines):
                    alone.append(self.own_file(os.path.join(iso_root, "t", "%05d-%08d.txt" % (idx, ln))))
                    fd = os.open(alone[-1], os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
                    with os.fdopen(fd, "wb") as f:
                        # The run is the file, from its first byte. Only a run that starts like a binary
                        # signature (a build reads no such file) has line breaks before it (lead_of).
                        run = all_lines[ln - 1]
                        lead = lead_of(run[:600] + b"\n", b"\n")
                        f.write(lead + run + b"\n")
                del all_lines
            idata = []
            if touched:     # read twice, like every file: as it is, then with the proof-of-read line
                idata = self.reading(b, iso_root, rdir, "-strings-isolated")
                self.default_rules_proved(b, iso_root, rdir, "-strings-isolated")
                idata, lacking = self.proven(b, idata, iso_root, {place: self.sealed(place) for place in alone}, alone, pdir, rdir, "-strings-isolated")
                for path in lacking:
                    for erel in of_runs(path):
                        unproven[erel].add(b.label)
            raw[b.label + " ELF strings (each run alone)"] = len(idata)
            iso_hits = set()
            for it in idata:
                mfile = re.search(r"/(\d{5})-(\d{8})\.txt$", str(it.get("File", "")))
                if not mfile or int(mfile.group(1)) not in elf_by_idx:
                    raise Failure("an isolated strings finding lies outside the dump directory")
                idx, ln = int(mfile.group(1)), int(mfile.group(2))
                rule = str(it.get("RuleID", "?"))
                ent = elf_by_sha[elf_by_idx[idx]]
                if ln < 1 or ln > len(ent["offs"]):
                    raise Failure("an isolated strings finding has no run")
                iso_hits.add((idx, rule, ln))
                for erel in ent["paths"]:
                    cpath, ancestors = canon(erel)
                    key = (i, rule, cpath + STRINGS, ent["offs"][ln - 1])
                    rec = self.positions.setdefault(key, {"member": os.path.join(tree, erel), "ancestors": ancestors, "tree": tree,
                                                          "per_build": collections.Counter(), "strings": True})
                    rec["per_build"][b.label] += 1
            del idata
            for idx, rule, sl, lo, hi in joined:
                if not any((idx, rule, ln) in iso_hits for ln in range(lo, hi + 1)):
                    ent = elf_by_sha[elf_by_idx[idx]]
                    for erel in ent["paths"]:
                        self.artifacts.append((i, b.label, canon(erel)[0], rule, ent["offs"][sl - 1]))
        self.unsealed()
        for rel, place in places:
            os.rename(place, os.path.join(tree, rel))
        # A file that showed its proof-of-read line to a build in no reading is unread by that build.
        for rel, labels in sorted(unproven.items()):
            reason = "no proof that a build read it (%s)" % ("both builds" if len(labels) == len(builds) else "one build; the other build read it")
            self.unread_reasons[reason] += 1
            cpath, ancestors = canon(rel)
            self.unread.append((i, cpath, reason, self.fsha(os.path.join(tree, rel)), ancestors, tree))
        self.payloads.append({"name": base, "sha256": digest, "bytes": stats["bytes"], "files": files,
                              "archives_opened": sum(stats["opened_per_format"].values()),
                              "archives_not_opened": len(closed), "member_skips": dict(sorted(skips.items())),
                              "raw_findings": dict(raw)})


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--name", required=True, help="the name of this run in the receipt")
    ap.add_argument("--gitleaks-pinned", required=True, help="absolute path of the pinned older build")
    ap.add_argument("--gitleaks-current", required=True, help="absolute path of the current build")
    ap.add_argument("--default-config-pinned", help="the pinned build's default rules (default: gitleaks.toml beside it)")
    ap.add_argument("--default-config-current", help="the current build's default rules (default: gitleaks.toml beside it)")
    ap.add_argument("--pinned-version", help="a label for a pinned build that does not print its version")
    ap.add_argument("--current-version", help="a label for a current build that does not print its version")
    ap.add_argument("--accepted-findings", help="the private acceptance file (optional; without it nothing is accepted)")
    ap.add_argument("--pending", help="the pending list (optional; it never accepts)")
    ap.add_argument("--receipt", required=True, help="the counts-only receipt (a new file)")
    ap.add_argument("--report", required=True, help="the counts-only report, JSON (a new file)")
    ap.add_argument("--unaccepted", help="what refused, by path, position and sha256, without content (a new file, 0600)")
    ap.add_argument("--work", help="parent of the private work directory")
    ap.add_argument("--keep-work", action="store_true")
    ap.add_argument("--proof-only", action="store_true",
                    help="a run that is no release receipt: its name gets the PROOF-ONLY- prefix and its report says release false")
    ap.add_argument("--min-free-bytes", type=int, default=1 << 30, help="stop when the work file system has less free space")
    ap.add_argument("--disk-stop-percent", type=float, help="stop when the work file system is at least this full")
    ap.add_argument("payload", nargs="*")
    a = ap.parse_args()
    os.umask(0o077)
    need(re.fullmatch(r"[A-Za-z0-9._-]{1,80}", a.name), "--name must match [A-Za-z0-9._-]{1,80}")
    need(not a.name.startswith(PROOF_ONLY), "a name that starts with PROOF-ONLY is given by --proof-only alone")
    if a.proof_only:
        a.name = PROOF_ONLY + "-" + a.name
    need(a.payload, "at least one payload file is required")
    payloads = [os.path.abspath(p) for p in a.payload]
    for p in payloads:
        need(os.path.isfile(p) and not os.path.islink(p), "a payload is not a regular file")
        need(re.fullmatch(r"[A-Za-z0-9._+-]+", os.path.basename(p)), "a payload file name has characters a receipt cannot hold")
    need(len({os.path.basename(p) for p in payloads}) == len(payloads), "two payload files have the same name")
    outputs = [os.path.abspath(p) for p in (a.receipt, a.report, a.unaccepted) if p]
    need(not any(os.path.lexists(p) for p in outputs) and len(set(outputs)) == len(outputs),
         "the receipt, the report and the listing must be new files")
    acc_pos = acc_arch = acc_files = acc_unop = pend = ()
    acc_sha = pend_sha = "none"
    if a.accepted_findings:
        doc, acc_sha = load_json(a.accepted_findings, "acceptance file")
        acc_pos, acc_arch, acc_files, acc_unop = validate_accepted(doc)
    if a.pending:
        doc, pend_sha = load_json(a.pending, "pending list")
        pend = validate_pending(doc)
    classes, classes_sha = load_classes()
    need(os.path.isfile(EXTRACTOR), "release-payload-deep-extract.py is missing beside this file")
    need(a.work is None or os.path.isdir(a.work), "--work must be an existing directory")
    builds = prepare_builds(a)
    started = time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())
    commit = tool_commit()

    work = tempfile.mkdtemp(prefix="credential-scan-", dir=a.work)
    try:
        os.chmod(work, 0o700)
        cfg_dir = os.path.join(work, "config")
        os.makedirs(cfg_dir, mode=0o700)
        extractor = load_extractor()
        extractor.MIN_FREE_BYTES, extractor.DISK_STOP_PERCENT = a.min_free_bytes, a.disk_stop_percent
        scan = Scan(work, builds, extractor)
        os.makedirs(scan.empty_cwd, mode=0o700)
        for b in builds:
            with open(b.default_cfg, "rb") as f:
                raw = f.read()
            text = raw.decode("utf-8")
            b.default_sha256 = hashlib.sha256(raw).hexdigest()
            stripped, b.removed = strip_allowlist(text)
            b.default_copy = os.path.join(cfg_dir, "default-%s.toml" % b.role)
            b.cfg = os.path.join(cfg_dir, "default-minus-removed-paths-%s.toml" % b.role)
            write_new(b.default_copy, text)
            write_new(b.cfg, stripped)
            b.patterns = global_paths(stripped)
            b.default_patterns = global_paths(text)
            b.dir_mode = subprocess.run([b.exe, "dir", "--help"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                                        stdin=subprocess.DEVNULL, cwd=scan.empty_cwd, check=False).returncode == 0
        for i, payload in enumerate(payloads):
            scan.payload(i, payload)
        return decide(a, scan, (acc_pos, acc_arch, acc_files, acc_unop), acc_sha, pend, pend_sha, started, commit, classes, classes_sha)
    finally:
        if not a.keep_work:
            shutil.rmtree(work, ignore_errors=True)


def decide(a, scan, accepted, acc_sha, pend, pend_sha, started, commit, classes, classes_sha):
    acc_pos, acc_arch, acc_files, acc_unop = accepted
    builds = scan.builds

    def in_archive(ancestors, tree):
        return any(e["path"] == acanon and e["sha256"] == scan.fsha(os.path.join(tree, arel))
                   for arel, acanon in ancestors for e in acc_arch)

    def in_file(path, sha):
        return any(e["path"] == path and e["sha256"] == sha for e in acc_files)

    counts = collections.Counter()
    per_rule = collections.defaultdict(collections.Counter)
    refused = []    # (payload, rule, path, line, findings, pending owner, sha256)
    for (i, rule, cpath, line), rec in scan.positions.items():
        msha = scan.fsha(rec["member"])
        count = max(rec["per_build"].values())

        def same(e):
            return e["rule"] == rule and e["path"] == cpath and e["line"] == line and e["sha256"] == msha and e.get("count", 1) == count

        is_pend = any(same(e) for e in pend)
        if is_pend:
            decision = "refused"    # a pending position refuses whatever the acceptance file says
        elif any(same(e) for e in acc_pos):
            decision = "accepted position"
        elif in_archive(rec["ancestors"], rec["tree"]):
            decision = "accepted in archive"
        elif in_file(cpath[:-len(STRINGS)] if rec["strings"] else cpath, msha):
            decision = "accepted in file"
        elif any(class_resolves(e, scan.payloads[i]["name"], rule, cpath, rec, line) for e in classes):
            decision = "resolved by class"  # after every acceptance bound to bytes, and never for a pending position
        else:
            decision = "refused"
        if decision == "refused":
            refused.append((i, rule, cpath, line, count, is_pend, msha))
        counts["findings"] += count
        counts[decision] += count
        if decision != "resolved by class":     # of those the outputs hold one count: no rule, path or line
            per_rule[rule][decision] += count
    unopened_refused = [row for row in scan.unopened
                        if not (any(e["path"] == row[1] and e["sha256"] == row[3] for e in acc_unop)
                                or in_file(row[1], row[3]) or in_archive(row[4], row[5]))]
    for i, cpath, _fmt, usha, _anc, _tree in unopened_refused:
        refused.append((i, UNOPENED_RULE, cpath, 0, 1, False, usha))
    unread_refused = [row for row in scan.unread if not (in_file(row[1], row[3]) or in_archive(row[4], row[5]))]
    skipped_refused = [row for row in scan.skipped
                       if not (any(e["path"] == row[1] and e["sha256"] == row[2] for e in acc_arch) or in_archive(row[5], row[6]))]
    n_refused = sum(r[4] for r in refused)
    n_pending = sum(r[4] for r in refused if r[5])
    n_unread = len(scan.unread) + len(scan.unopened) + sum(row[4] for row in scan.skipped)
    n_unread_unaccepted = len(unread_refused) + sum(row[4] for row in skipped_refused)
    verdict = "PASS" if n_refused == 0 and n_pending == 0 and n_unread_unaccepted == 0 else "REFUSED"

    line1 = "CREDENTIAL-LEG %s %s files=%d %s accepted=%s pending=%s builds=%s build_sha256=%s findings=%d accepted_positions=%d " \
            "accepted_in_archives=%d accepted_in_files=%d refused=%d pending_owner=%d unread=%d unread_unaccepted=%d" % (
                verdict, a.name, len(scan.payloads), " ".join("%s=%s" % (p["name"], p["sha256"]) for p in scan.payloads),
                acc_sha, pend_sha, ",".join(b.label for b in builds), ",".join(b.sha256 for b in builds), counts["findings"],
                counts["accepted position"], counts["accepted in archive"], counts["accepted in file"], n_refused, n_pending,
                n_unread, n_unread_unaccepted)
    if classes:
        line1 += " class_tool=%s class_resolved=%d" % (CLASS_TOOL, counts["resolved by class"])
    out = [line1, ""]
    out.append("counts are findings: a position (rule, path, line) with N findings counts N (the larger of the two builds); "
               "distinct positions: %d; an archive that could not be opened counts as one refusal" % len(scan.positions))
    out.append("")
    out.append("| payload | files in deep tree | archives opened | archives not opened | raw findings per build | member skips |")
    out.append("|---|---|---|---|---|---|")
    for i, p in enumerate(scan.payloads):
        out.append("| p%d %s | %d | %d | %d | %s | %s |" % (
            i, p["name"], p["files"], p["archives_opened"], p["archives_not_opened"],
            ", ".join("%s=%d" % kv for kv in p["raw_findings"].items()),
            "; ".join("%s: %d" % kv for kv in p["member_skips"].items()) or "none"))
    out.append("")
    out.append("| build | role | binary sha256 | allowlist lines removed (vendor, dist-info, lock) | default rules proved against the embedded default |")
    out.append("|---|---|---|---|---|")
    for b in builds:
        out.append("| %s | %s | %s | %d | yes |" % (b.label, b.role, b.sha256, b.removed))
    out.append("")
    out.append("| rule | findings | accepted position | accepted in archive | accepted in file | refused |")
    out.append("|---|---|---|---|---|---|")
    for rule in sorted(per_rule):
        c = per_rule[rule]
        out.append("| %s | %d | %d | %d | %d | %d |" % (rule, sum(c.values()), c["accepted position"], c["accepted in archive"],
                                                    c["accepted in file"], c["refused"]))
    if classes:     # one line for the findings a class resolved, so that the table's findings add up; it names no rule
        out.append("| resolved by class (no rule named) | %d | 0 | 0 | 0 | 0 |" % counts["resolved by class"])
    out.append("")
    out.append("ELF files covered by their printable runs (8 or more printable ASCII bytes, each run a unit of its own): %d" % scan.elf_files)
    out.append("Proof of read, per build (files handed, read as they are, read behind the line of plain words, unread; "
               "readings of directories as they are, with the proof-of-read line): %s" % "; ".join(
                   "%s: %d, %d, %d, %d; %d, %d" % (label, *proof.values()) for label, proof in scan.read_proof.items()))
    out.append("unread files: %s; of them without an acceptance entry: %d" % (
        "; ".join("%s: %d" % kv for kv in sorted(scan.unread_reasons.items())) or "none", len(unread_refused)))
    out.append("archives not opened: %d (without an acceptance entry: %d); members not written out that hold content: %d "
               "(without an acceptance entry: %d)" % (
                   len(scan.unopened), len(unopened_refused), sum(row[4] for row in scan.skipped),
                   sum(row[4] for row in skipped_refused)))
    out.append("acceptance file: %d position entries, %d archive entries, %d file entries, %d unopened entries; "
               "pending entries (never accepting): %d" % (len(acc_pos), len(acc_arch), len(acc_files), len(acc_unop), len(pend)))
    out.append("")
    out.append("method artifact: %d findings that span two runs (reported only when the runs are joined by line; "
               "no single run reproduces them; not counted, not refused)" % len(scan.artifacts))
    if scan.artifacts:
        out.append("")
        out.append("| payload | build | ELF path | rule | offset of the run |")
        out.append("|---|---|---|---|---|")
        for i, blabel, cpath, rule, offset in sorted(scan.artifacts):
            out.append("| p%d | %s | %s | %s | %d |" % (i, blabel, receipt_path(cpath), rule, offset))
    out.append("")
    out.append("| refused: payload | rule | path | line | findings | pending owner |")
    out.append("|---|---|---|---|---|---|")
    for i, rule, cpath, line, count, is_pend, _sha in sorted(refused):
        out.append("| p%d | %s | %s | %d | %d | %s |" % (i, rule, receipt_path(cpath), line, count, "yes" if is_pend else "no"))
    if not refused:
        out.append("| none | | | | | |")
    write_new(os.path.abspath(a.receipt), "\n".join(out) + "\n")

    names = [p["name"] for p in scan.payloads]
    report = {
        "schema": SCHEMA_REPORT, "name": a.name, "label": a.name, "release": not a.proof_only, "status": verdict,
        "generated_at": started, "tool_commit": commit, "tool_sha256": sha256_file(os.path.abspath(__file__)),
        "receipt": line1,
        "payload": scan.payloads,
        "detector_builds": [{"role": b.role, "version": b.label, "sha256": b.sha256, "default_rules_sha256": b.default_sha256,
                             "allowlist_lines_removed": b.removed} for b in builds],
        "accepted": {"count": len(acc_pos) + len(acc_arch) + len(acc_files) + len(acc_unop),
                     "sha256": None if acc_sha == "none" else acc_sha,
                     "positions": len(acc_pos), "archives": len(acc_arch), "files": len(acc_files), "unopened": len(acc_unop)},
        "pending": {"count": len(pend), "sha256": None if pend_sha == "none" else pend_sha},
        "findings": {"total": counts["findings"], "positions": len(scan.positions), "accepted_positions": counts["accepted position"],
                     "accepted_in_archives": counts["accepted in archive"], "accepted_in_files": counts["accepted in file"],
                     "refused": n_refused, "pending_owner": n_pending,
                     "by_rule": {rule: dict(sorted(c.items())) for rule, c in sorted(per_rule.items())}},
        "unread": {"total": n_unread, "unaccepted": n_unread_unaccepted, "files": dict(sorted(scan.unread_reasons.items())),
                   "archives_not_opened": len(scan.unopened), "archives_not_opened_unaccepted": len(unopened_refused),
                   "members_not_written": sum(row[4] for row in scan.skipped)},
        "elf_files": scan.elf_files, "method_artifacts": len(scan.artifacts),
        "read_proof": scan.read_proof,
    }
    if classes:
        # A finding resolved by class stays in the numbers: findings.total is the sum of the accepted, the
        # refused and this one count.
        report["findings"]["class_resolved"] = counts["resolved by class"]
        # The list in force, for the receipt writer: it holds this digest to the list of the report's commit.
        report["identifier_classes"] = {"tool": CLASS_TOOL, "resolved": counts["resolved by class"], "list_sha256": classes_sha}
    write_new(os.path.abspath(a.report), json.dumps(report, indent=2, sort_keys=True) + "\n")
    if a.unaccepted:
        listing = {
            "schema": SCHEMA_UNACCEPTED,
            "positions": [{"payload": names[i], "rule": rule, "path": cpath, "sha256": sha, "line": line, "count": count, "pending": is_pend}
                          for i, rule, cpath, line, count, is_pend, sha in sorted(refused) if rule != UNOPENED_RULE],
            "unopened": [{"payload": names[i], "path": cpath, "sha256": usha, "format": fmt}
                         for i, cpath, fmt, usha, _anc, _tree in sorted(unopened_refused, key=lambda row: row[:4])],
            "unread": [{"payload": names[i], "path": cpath, "sha256": sha, "reason": reason}
                       for i, cpath, reason, sha, _anc, _tree in sorted(unread_refused, key=lambda row: row[:4])],
            "members_not_written": [{"payload": names[i], "archive": cpath, "sha256": sha, "reason": reason, "count": count}
                                    for i, cpath, sha, reason, count, _anc, _tree in sorted(skipped_refused, key=lambda row: row[:5])],
        }
        write_new(os.path.abspath(a.unaccepted), json.dumps(listing, indent=1, sort_keys=True) + "\n")
    print(line1)
    return 0 if verdict == "PASS" else 1


if __name__ == "__main__":
    try:
        sys.exit(main())
    except Usage as exc:
        sys.stderr.write("release-payload-credential-scan: usage error: %s\n" % exc)
        sys.exit(2)
    except Failure as exc:
        sys.stderr.write("CREDENTIAL-LEG REFUSED tool-failure: %s\n" % exc)
        sys.exit(3)
    except SystemExit:
        raise
    except BaseException as exc:  # noqa: BLE001 - never echo exception text (it may hold data)
        tb = traceback.extract_tb(exc.__traceback__)
        sys.stderr.write("CREDENTIAL-LEG REFUSED tool-failure: %s at %s\n" % (
            type(exc).__name__, ", ".join("%s:%d" % (os.path.basename(fr.filename), fr.lineno) for fr in tb[-3:])))
        sys.exit(3)
