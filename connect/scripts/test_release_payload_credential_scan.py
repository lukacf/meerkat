"""Release payload credential scan: two detector builds through every nested archive, fail closed.

Synthetic values only. The detector builds are not assumed to exist: the tool's own logic runs
against a stand-in the tests write into a temporary directory. It takes the same command line and
writes a report of the same shape, for made-up markers in the scanned tree (two builds: an older
one with the `detect` command and a current one with `dir`, which also knows a second marker).
The markers are put together from pieces and are not a credential of anything. One class runs the
two real builds when the environment names them, and is skipped otherwise.
"""
from pathlib import Path
import binascii
import bz2
import calendar
import gzip
import hashlib
import importlib.util
import io
import json
import lzma
import os
import shutil
import stat
import struct
import subprocess
import sys
import tarfile
import tempfile
import time
import types
import unittest
import uuid
import zipfile
from unittest import mock

HERE = Path(__file__).resolve().parent
TOOL = HERE / "release-payload-credential-scan.py"
EXTRACTOR = HERE / "release-payload-deep-extract.py"
CANDIDATE = HERE / "connect-candidate.py"
SCHEMA_ACCEPTED = "release-payload-credential-scan/accepted/1"
SCHEMA_PENDING = "release-payload-credential-scan/pending/1"

# The stand-in's first marker: a name, "=", and a value. Both builds know it.
MARK_NAME = "zq_fixture_" + "na" + "me"
MARK_VALUE = "zqv" + "3fa9" + "07c1" + "5be2"
MARK = (MARK_NAME + " = " + MARK_VALUE).encode()
RULE = "zq-fixture-pair"
# The second marker: only the current build knows it.
TAG_NAME = "zq_fixture_" + "tag"
TAG_VALUE = "zqt" + "81c4" + "e07a"
TAG = (TAG_NAME + ":" + TAG_VALUE).encode()
TAG_RULE = "zq-fixture-tag"
RULES = [(RULE, MARK_NAME + r"\s*=\s*(zqv[0-9a-f]{12})"), (TAG_RULE, TAG_NAME + r":(zqt[0-9a-f]{8})")]
VALUES = (MARK_VALUE, TAG_VALUE)
# The scan's proof-of-read line, a made-up name and value of its own that every build must report, and
# the stand-in's rule for it.
PROOF_VALUE = "".join(["k3v9", "x1m7", "q5t2", "w8n4", "j6b0", "pd"])
PROOF_LINE = b"fixture " + b"api" + b"_k" + b"ey = " + PROOF_VALUE.encode() + b"\n"
PROOF = ("zq-proof-of-read", "fixture " + "api" + "_k" + "ey = (" + PROOF_VALUE[:4] + "[a-z0-9]{18})")
VALUES += (PROOF_VALUE,)

# The identifier class rows: a made-up record whose one field holds an identifier that is new in
# every run of these tests, and a stand-in rule that reports any text value of a field of that name.
SCHEMA_CLASSES = "release-payload-credential-scan/identifier-classes/1"
CLASSES_NAME = "payload-credential-identifier-classes.json"
CLASS_TOOL = "credential-identifier-class"
IDENT_KEY = "zq_ident_" + "mark"
IDENT_RULE = "zq-fixture-ident"
IDENT_RULES = [(IDENT_RULE, r'zq_ident\w*"\s*:\s*"([^"\n]{4,})"')]
RECORD = """{
  "kind": "made-up record",
  "runs": [
    {
      "outcome": {
        "holder": {
          "chain": [
            {
              %s
              "step": 1
            }
          ]
        }
      }
    }
  ]
}
"""
RECORD_LINE = 9     # the line of RECORD that holds the field
RECORD_PATH = "runs[].outcome.holder.chain[]." + IDENT_KEY


def fresh():
    """A new random identifier (version 4) that holds a letter, so that its upper-case form differs."""
    while True:
        value = str(uuid.uuid4())
        if value != value.upper():
            return value


def pair(value, key=IDENT_KEY):
    return '"%s": "%s",' % (key, value)


def record(value, line=None):
    """The made-up record with the field on line RECORD_LINE, or with that line as given."""
    return (RECORD % (line if line is not None else pair(value))).encode()


def as_text(value):
    """A made-up document as text, one field a line."""
    return (json.dumps(value, indent=2) + "\n").encode()


def class_entry(**more):
    return {"rule": IDENT_RULE, "asset": "product-<version>-<target>.tar.gz", "member": "records/zq/summary.json",
            "key_path": RECORD_PATH, "value_form": "uuid-v4", "accepted_by": "the test", **more}

# The stand-in's default rules file: a global path allowlist, as the real default has.
DEFAULT_RULES = """title = "made-up default rules"

[allowlist]
description = "global allowlist"
paths = [
    '''(^|/)vendor/''',
    '''\\.dist-info/''',
    '''(^|/)package-lock\\.json$''',
    '''\\.zqskip$''',
]
"""

STAND_IN = r'''#!@PYTHON@ -IB
"""A stand-in for a detector build: the same command line, made-up rules for made-up markers.
Like the real builds it honours a path allowlist, an ignore file and a rules file at the scan
root, and an allow comment unless told to ignore it, and it does not read files that start with
a binary signature. Its report carries the matched value in "Secret" on purpose. SKEW moves the
numbers of a finding's position by the amounts it names, or leaves a number out (None)."""
import json
import lzma
import os
import re
import sys
import tomllib

VERSION = @VERSION@
DIR_MODE = @DIR_MODE@
RULES = @RULES@
EMBEDDED = @EMBEDDED@
SKEW = @SKEW@
REVERSED = @REVERSED@
HERE = os.path.dirname(os.path.abspath(__file__))
BINARY = [(0, b"\x7fELF"), (0, b"PK\x03\x04"), (0, b"\x1f\x8b"), (257, b"ustar"), (0, b"%PDF"), (0, b"\x28\xb5\x2f\xfd")]
BINARY += [(0, b"BZh"), (0, b"\xfd7zXZ\x00")]
# Proof of read. The four lines below are rewritten for a build that differs (adapted, in the rows).
MORE_BINARY = []    # kinds of file this build does not read, beyond those above
SKIP_WORD = None    # a file that holds this word anywhere is not read
SKIP_PATH = None    # a file whose path holds this is not read
NAMES = ""          # how a file's name is misreported: replaced (bytes that are no UTF-8), other (a name of no file)
PROOF = ("zq-proof-of-read", "fixture " + "api" + "_k" + "ey = (" + "k3v9" + "[a-z0-9]{18})")    # the rule for the scan's proof-of-read line, or None
PROOF_MODE = ""     # how that line is misreported: in a file of more than one line twice, below or gone; long everywhere
BINARY += MORE_BINARY


def misreported(findings):
    """The findings as this build reports them: the proof-of-read line of a file of more than one
    line twice, one line below, or not at all, when PROOF_MODE says so."""
    out = []
    for finding in findings:
        wrong = ""
        if PROOF and PROOF_MODE and finding["RuleID"] == PROOF[0]:
            with open(finding["File"], "rb") as stream:
                wrong = PROOF_MODE if stream.read().count(b"\n") > 1 else ""
        if wrong == "below":
            finding = dict(finding, StartLine=finding["StartLine"] + 1, EndLine=finding["EndLine"] + 1)
        if PROOF and PROOF_MODE == "long" and finding["RuleID"] == PROOF[0]:    # its end is said to lie a line later
            finding = dict(finding, EndLine=finding["EndLine"] + 1)
        if NAMES == "replaced":     # as a report does that cannot hold bytes that are no UTF-8
            finding = dict(finding, File=re.sub("[%s-%s]" % (chr(0xdc80), chr(0xdcff)), chr(0xfffd), finding["File"]))
        elif NAMES == "other" and os.path.basename(finding["File"]).startswith("zq"):
            finding = dict(finding, File=finding["File"] + ".x")
        out += [finding] * {"gone": 0, "twice": 2}.get(wrong, 1)
    return out


def main():
    argv = sys.argv[1:]
    with open(os.path.join(HERE, "calls.jsonl"), "a") as log:
        log.write(json.dumps(argv) + "\n")
    if argv == ["version"]:
        print(VERSION)
        return 0
    if argv[:2] == ["dir", "--help"]:
        return 0 if DIR_MODE else 1
    if DIR_MODE and argv[:1] == ["dir"]:
        source, rest = argv[1], argv[2:]
    elif not DIR_MODE and argv[:3] == ["detect", "--no-git", "--source"]:
        source, rest = argv[3], argv[4:]
    else:
        sys.stderr.write("unknown command\n")
        return 2
    options, flags = {}, set()
    while rest:
        word = rest.pop(0)
        if word in ("--config", "--log-level", "--report-format", "--report-path", "--exit-code"):
            options[word] = rest.pop(0)
        else:
            flags.add(word)
    if "--redact=100" not in flags or options.get("--report-format") != "json":
        sys.stderr.write("this stand-in runs only redacted, with a JSON report\n")
        return 3
    local = os.path.join(source, ".gitleaks.toml")
    if "--config" in options:
        with open(options["--config"], encoding="utf-8") as stream:
            rules_text = stream.read()
    elif os.path.exists(local):
        with open(local, encoding="utf-8") as stream:
            rules_text = stream.read()
    else:
        rules_text = EMBEDDED
    allowed = [re.compile(p) for p in tomllib.loads(rules_text).get("allowlist", {}).get("paths", [])]
    ignore_file = os.path.exists(os.path.join(source, ".gitleaksignore"))
    # A rules file that lists rules ([[rules]] tables with an id) leaves out every rule it does not list.
    listed = tomllib.loads(rules_text).get("rules")
    rules = [row for row in RULES if listed is None or any(entry.get("id") == row[0] for entry in listed)]
    if PROOF:   # among the build's rules whatever file it is given
        rules.append(PROOF)
    findings = []
    for dirpath, dirnames, filenames in os.walk(source):
        dirnames.sort()
        for name in sorted(filenames):
            path = os.path.join(dirpath, name)
            if any(rx.search(path) for rx in allowed):
                continue
            with open(path, "rb") as stream:
                data = stream.read()
            if any(data[at:at + len(magic)] == magic for at, magic in BINARY) or (SKIP_WORD and SKIP_WORD in data) or (SKIP_PATH and SKIP_PATH in path):
                continue
            text = data.decode("latin-1")
            for rule, pattern in rules:
                for found in re.finditer(pattern, text):
                    begin = text.rfind("\n", 0, found.start()) + 1
                    end = text.find("\n", found.end())
                    end = len(text) if end < 0 else end
                    if ignore_file or ("gitleaks:allow" in text[begin:end] and "--ignore-gitleaks-allow" not in flags):
                        continue
                    findings.append({
                        "RuleID": rule, "Description": "a made-up rule", "File": path,
                        "StartLine": text.count("\n", 0, found.start()) + 1,
                        "EndLine": text.count("\n", 0, found.end()) + 1,
                        "StartColumn": found.start() - begin + 1, "EndColumn": found.end() - begin,
                        "Match": text[found.start():found.start(1)] + "REDACTED", "Secret": found.group(1)})
                    for key, change in SKEW.items():
                        if change is None:
                            del findings[-1][key]
                        else:
                            findings[-1][key] += change
    if REVERSED:
        findings.reverse()
    with open(options["--report-path"], "w", encoding="utf-8") as stream:
        json.dump(misreported(findings), stream)
    return int(options.get("--exit-code", "1")) if findings else 0


sys.exit(main())
'''


def write_build(directory, version, dir_mode, rules, default_rules=DEFAULT_RULES, skew=None, reverse=False):
    """A stand-in build with its default rules file beside it. Returns the build's path. With reverse
    it reports its findings in the opposite order (a real build reads files in no fixed order)."""
    directory.mkdir(parents=True)
    build = directory / "gitleaks"
    text = STAND_IN
    for key, value in (("@PYTHON@", sys.executable), ("@VERSION@", repr(version)), ("@DIR_MODE@", repr(dir_mode)),
                       ("@RULES@", repr(rules)), ("@EMBEDDED@", repr(default_rules)), ("@SKEW@", repr(skew or {})),
                       ("@REVERSED@", repr(bool(reverse)))):
        text = text.replace(key, value)
    build.write_text(text)
    build.chmod(0o755)
    (directory / "gitleaks.toml").write_text(default_rules)
    return build


def write_builds(directory):
    """The two stand-ins: the pinned one knows the first marker, the current one both."""
    return (write_build(directory / "pinned", "0.0.1", False, RULES[:1]),
            write_build(directory / "current", "0.0.2", True, RULES))


def calls(build):
    """The command lines a stand-in was run with."""
    return [json.loads(line) for line in (build.parent / "calls.jsonl").read_text().splitlines()]


def adapted(build, **switches):
    """A written stand-in build that differs in its proof of read: more_binary (kinds of file it does
    not read), skip_word (a word that makes it pass over a file), skip_path (a part of a path that
    does), proof (its rule for the scan's proof-of-read line, or None), proof_mode (how it
    misreports that line), names (how it misreports the name of a file). Returns the build's path."""
    text = build.read_text()
    for name, value in switches.items():
        line = [line for line in text.splitlines() if line.startswith(name.upper() + " = ")]
        assert len(line) == 1, name
        text = text.replace(line[0], "%s = %r" % (name.upper(), value))
    build.write_text(text)
    return build


def tar_bytes(members, compressed=True):
    raw = io.BytesIO()
    with tarfile.open(fileobj=raw, mode="w", format=tarfile.PAX_FORMAT) as archive:
        for name, data in members.items():
            info = tarfile.TarInfo(name)
            if data is None:    # a symbolic link
                info.type, info.linkname = tarfile.SYMTYPE, "elsewhere"
                archive.addfile(info)
                continue
            info.size = len(data)
            archive.addfile(info, io.BytesIO(data))
    return gzip.compress(raw.getvalue(), mtime=0) if compressed else raw.getvalue()


def zip_bytes(members):
    raw = io.BytesIO()
    with zipfile.ZipFile(raw, "w", zipfile.ZIP_DEFLATED) as archive:
        for name, data in members.items():
            archive.writestr(zipfile.ZipInfo(name, (1980, 1, 1, 0, 0, 0)), data)
    return raw.getvalue()


def zstd_compress(data):
    try:
        from compression import zstd  # Python 3.14 and later
        return zstd.compress(data)
    except ImportError:
        pass
    tool = shutil.which("zstd")
    if tool is None:
        return None
    return subprocess.run([tool, "-q", "-c"], input=data, stdout=subprocess.PIPE, check=True).stdout


def sha256(data):
    return hashlib.sha256(data).hexdigest()


def run(script, *args, env=None):
    return subprocess.run([sys.executable, "-I", str(script), *map(str, args)], capture_output=True, text=True, env=env)


def fields(line):
    """The name=value words of a receipt line."""
    return dict(word.split("=", 1) for word in line.split() if "=" in word)


def position(path, data, line, rule=RULE, **more):
    return {"rule": rule, "path": path, "sha256": sha256(data), "line": line, "class": "SYNTHETIC",
            "reason": "a made-up marker of this test", "accepted_by": "the test", **more}


def archive_entry(path, data):
    return {"path": path, "sha256": sha256(data), "index": "pypi", "class": "PUBLIC-UPSTREAM-ARCHIVE",
            "reason": "a made-up archive of this test", "accepted_by": "the test"}


def file_entry(path, data):
    return {"path": path, "sha256": sha256(data), "class": "PUBLIC-UPSTREAM-FILE",
            "reason": "a made-up file of this test", "accepted_by": "the test"}


class Scans(unittest.TestCase):
    """What every class here needs: a scratch directory, payload files and one scan."""

    @classmethod
    def setUpClass(cls):
        cls.tools = Path(tempfile.mkdtemp(prefix="credential-scan-builds-")).resolve()
        cls.addClassCleanup(shutil.rmtree, cls.tools, True)
        cls.pinned, cls.current = write_builds(cls.tools)

    def setUp(self):
        self.root = Path(tempfile.mkdtemp(prefix="credential-scan-test-")).resolve()
        self.addCleanup(shutil.rmtree, self.root, True)
        self.count = 0

    def payload(self, data, name="product-0.0.0-linux-x86_64.tar.gz"):
        self.count += 1
        directory = self.root / f"payload-{self.count}"
        directory.mkdir()
        path = directory / name
        path.write_bytes(data)
        return path

    def product(self, members, **more):
        """A product archive: every member below one top directory, which the paths leave out."""
        return self.payload(tar_bytes({"pkg/" + name: data for name, data in members.items()}), **more)

    def document(self, name, value):
        self.count += 1
        path = self.root / f"{name}-{self.count}.json"
        path.write_text(json.dumps(value) + "\n")
        return path

    def accepted(self, **lists):
        return self.document("accepted", {"schema": SCHEMA_ACCEPTED, **lists})

    def pending(self, positions):
        return self.document("pending", {"schema": SCHEMA_PENDING, "positions": positions})

    def scan(self, *payload, accepted=None, pending=None, name="T", pinned=None, current=None, extra=(), values=VALUES, tool=TOOL):
        self.count += 1
        out = self.root / f"out-{self.count}"
        out.mkdir()
        args = ["--name", name, "--gitleaks-pinned", pinned or self.pinned, "--gitleaks-current", current or self.current,
                "--receipt", out / "receipt.txt", "--report", out / "report.json", "--unaccepted", out / "unaccepted.json",
                "--work", self.root, *extra]
        if accepted is not None:
            args += ["--accepted-findings", accepted]
        if pending is not None:
            args += ["--pending", pending]
        done = run(tool, *args, *payload)
        texts = [path.read_text() if path.exists() else None for path in (out / "receipt.txt", out / "report.json", out / "unaccepted.json")]
        # No output ever holds a matched value, and the private work directory is gone.
        for text in (done.stdout, done.stderr, *texts):
            for value in values:
                self.assertNotIn(value, text or "")
        self.assertEqual(list(self.root.glob("credential-scan-*")), [])
        line = texts[0].splitlines()[0] if texts[0] else None
        if line is not None:
            self.assertEqual(done.stdout, line + "\n")
            self.assertEqual((out / "unaccepted.json").stat().st_mode & 0o777, 0o600)
        return types.SimpleNamespace(
            code=done.returncode, stderr=done.stderr, line=line, receipt=texts[0], fields=fields(line) if line else None,
            report=json.loads(texts[1]) if texts[1] else None, listing=json.loads(texts[2]) if texts[2] else None)

    def counts(self, result, *names):
        return tuple(int(result.fields[name]) for name in names)


class CredentialScan(Scans):
    def test_a_clean_payload_passes_and_the_receipt_names_every_file(self):
        wheel = zip_bytes({"mod/a.py": b"x = 1\n"})
        product = self.product({"bin/tool": b"inert\n", "lib/library.tar.gz": tar_bytes({"library/wheel-1.0.whl": wheel})})
        checksum = self.payload(b"0" * 64 + b"  " + product.name.encode() + b"\n", name=product.name + ".sha256")
        result = self.scan(product, checksum, name="X0")
        self.assertEqual((result.code, result.stderr), (0, ""))
        builds = "builds=0.0.1,0.0.2 build_sha256=%s,%s" % (sha256(self.pinned.read_bytes()), sha256(self.current.read_bytes()))
        self.assertTrue(result.line.startswith(
            "CREDENTIAL-LEG PASS X0 files=2 %s=%s %s=%s accepted=none pending=none %s findings=0 " % (
                product.name, sha256(product.read_bytes()), checksum.name, sha256(checksum.read_bytes()), builds)), result.line)
        self.assertEqual(self.counts(result, "refused", "pending_owner", "unread", "unread_unaccepted"), (0, 0, 0, 0))
        self.assertEqual((result.report["status"], result.report["receipt"]), ("PASS", result.line))
        self.assertEqual([(row["name"], row["sha256"], row["archives_opened"]) for row in result.report["payload"]],
                         [(product.name, sha256(product.read_bytes()), 3), (checksum.name, sha256(checksum.read_bytes()), 0)])
        self.assertEqual(result.listing["positions"] + result.listing["unread"] + result.listing["unopened"], [])
        # Every run of a build is redacted, ignores allow comments, and uses that build's own command.
        for build, command in ((self.pinned, ["detect", "--no-git", "--source"]), (self.current, ["dir"])):
            runs = [argv for argv in calls(build) if argv != ["version"] and argv != ["dir", "--help"]]
            self.assertTrue(runs)
            for argv in runs:
                self.assertEqual(argv[:len(command)], command)
                self.assertIn("--redact=100", argv)
                self.assertIn("--ignore-gitleaks-allow", argv)

    def test_a_finding_refuses_and_no_output_holds_the_value(self):
        notes = b"first line\n" + MARK + b"\n"
        product = self.product({"notes.txt": notes})
        result = self.scan(product)
        self.assertEqual(result.code, 1)
        self.assertTrue(result.line.startswith("CREDENTIAL-LEG REFUSED T files=1 %s=%s " % (product.name, sha256(product.read_bytes()))))
        self.assertEqual(self.counts(result, "findings", "refused", "pending_owner", "unread_unaccepted"), (1, 1, 0, 0))
        self.assertIn("| p0 | %s | notes.txt | 2 | 1 | no |" % RULE, result.receipt)
        self.assertEqual(result.report["status"], "REFUSED")
        self.assertEqual(result.listing["positions"], [{"payload": product.name, "rule": RULE, "path": "notes.txt",
                                                        "sha256": sha256(notes), "line": 2, "count": 1, "pending": False}])

    def test_a_finding_of_one_build_only_refuses(self):
        result = self.scan(self.product({"notes.txt": TAG + b"\n"}))
        self.assertEqual((result.code, self.counts(result, "findings", "refused")), (1, (1, 1)))
        self.assertIn("| p0 | %s | notes.txt | 1 | 1 | no |" % TAG_RULE, result.receipt)

    def test_an_acceptance_is_bound_to_path_member_digest_rule_and_line(self):
        notes = b"first line\n" + MARK + b"\n"
        entry = position("notes.txt", notes, 2)
        accepted = self.accepted(positions=[entry])
        result = self.scan(self.product({"notes.txt": notes}), accepted=accepted)
        self.assertEqual((result.code, self.counts(result, "findings", "accepted_positions", "refused")), (0, (1, 1, 0)))
        self.assertEqual(result.fields["accepted"], sha256(accepted.read_bytes()))
        # The same finding in other bytes, on another line, or at another path refuses.
        for members in ({"notes.txt": b"first line.\n" + MARK + b"\n"},
                        {"notes.txt": b"first line\n\n" + MARK + b"\n"},
                        {"other.txt": notes}):
            result = self.scan(self.product(members), accepted=accepted)
            self.assertEqual((result.code, self.counts(result, "accepted_positions", "refused")), (1, (0, 1)), members)
        result = self.scan(self.product({"notes.txt": notes, "copy.txt": notes}), accepted=accepted)
        self.assertEqual((result.code, self.counts(result, "accepted_positions", "refused")), (1, (1, 1)))
        # An entry that differs in one field accepts nothing.
        for key, value in (("rule", TAG_RULE), ("path", "other.txt"), ("line", 3), ("sha256", "0" * 64), ("count", 2)):
            result = self.scan(self.product({"notes.txt": notes}), accepted=self.accepted(positions=[{**entry, key: value}]))
            self.assertEqual((result.code, self.counts(result, "accepted_positions", "refused")), (1, (0, 1)), key)
        # Without an acceptance file nothing is accepted.
        result = self.scan(self.product({"notes.txt": notes}))
        self.assertEqual((result.code, result.fields["accepted"]), (1, "none"))

    def test_an_entry_without_accepted_by_is_a_usage_error(self):
        notes = MARK + b"\n"
        product = self.product({"notes.txt": notes})
        whole = {"positions": position("notes.txt", notes, 1), "archives": archive_entry("lib.whl", b"x"),
                 "files": file_entry("notes.txt", notes),
                 "unopened": {"path": "odd.7z", "sha256": "0" * 64, "reason": "made up", "accepted_by": "the test"}}
        for kind, entry in whole.items():
            for broken in ({k: v for k, v in entry.items() if k != "accepted_by"}, {**entry, "accepted_by": "  "}):
                result = self.scan(product, accepted=self.accepted(**{kind: [broken]}))
                self.assertEqual((result.code, result.receipt), (2, None), kind)
                self.assertIn("accepted_by", result.stderr)
        for document in ({"schema": "another/accepted/1", "positions": []}, {"schema": SCHEMA_ACCEPTED, "accepted": []},
                         {"schema": SCHEMA_ACCEPTED, "positions": [{**whole["positions"], "line": "1"}]}, []):
            result = self.scan(product, accepted=self.document("accepted", document))
            self.assertEqual((result.code, result.receipt), (2, None), document)
        result = self.scan(product, pending=self.document("pending", {"schema": SCHEMA_ACCEPTED, "positions": []}))
        self.assertEqual((result.code, result.receipt), (2, None))

    def test_a_pending_entry_never_accepts_and_is_marked(self):
        notes = b"first line\n" + MARK + b"\n"
        product = self.product({"notes.txt": notes})
        entry = {"rule": RULE, "path": "notes.txt", "line": 2, "sha256": sha256(notes), "note": "waiting for its owner"}
        pending = self.pending([entry])
        result = self.scan(product, pending=pending)
        self.assertEqual((result.code, self.counts(result, "refused", "pending_owner")), (1, (1, 1)))
        self.assertEqual(result.fields["pending"], sha256(pending.read_bytes()))
        self.assertIn("| p0 | %s | notes.txt | 2 | 1 | yes |" % RULE, result.receipt)
        self.assertTrue(result.listing["positions"][0]["pending"])
        # A pending position refuses even when an acceptance entry matches it.
        result = self.scan(product, pending=pending, accepted=self.accepted(positions=[position("notes.txt", notes, 2)]))
        self.assertEqual((result.code, self.counts(result, "accepted_positions", "refused", "pending_owner")), (1, (0, 1, 1)))
        # A pending entry for another position marks nothing.
        result = self.scan(product, pending=self.pending([{**entry, "line": 9}]))
        self.assertEqual((result.code, self.counts(result, "refused", "pending_owner")), (1, (1, 0)))
        self.assertIn("| p0 | %s | notes.txt | 2 | 1 | no |" % RULE, result.receipt)

    def test_an_archive_entry_covers_findings_inside_that_exact_archive_only(self):
        wheel = zip_bytes({"mod/a.py": MARK + b"\n"})
        accepted = self.accepted(archives=[archive_entry("lib/wheel-1.0.whl", wheel)])
        result = self.scan(self.product({"lib/wheel-1.0.whl": wheel, "readme.txt": b"plain\n"}), accepted=accepted)
        self.assertEqual((result.code, self.counts(result, "findings", "accepted_in_archives", "refused")), (0, (1, 1, 0)))
        # The same finding outside the archive, and in an archive of other bytes at the same path, refuses.
        result = self.scan(self.product({"lib/wheel-1.0.whl": wheel, "outside.txt": MARK + b"\n"}), accepted=accepted)
        self.assertEqual((result.code, self.counts(result, "accepted_in_archives", "refused")), (1, (1, 1)))
        self.assertIn("| p0 | %s | outside.txt | 1 | 1 | no |" % RULE, result.receipt)
        other = zip_bytes({"mod/a.py": MARK + b"\n", "mod/b.py": b"y = 2\n"})
        result = self.scan(self.product({"lib/wheel-1.0.whl": other}), accepted=accepted)
        self.assertEqual((result.code, self.counts(result, "accepted_in_archives", "refused")), (1, (0, 1)))
        self.assertIn("| p0 | %s | lib/wheel-1.0.whl!mod/a.py | 1 | 1 | no |" % RULE, result.receipt)

    def test_nested_archives_are_opened_at_every_level(self):
        inner = tar_bytes({"deep/notes.txt": MARK + b"\n"}, compressed=False)
        library = tar_bytes({"library/mid.zip": zip_bytes({"data/inner.tar": inner})})
        result = self.scan(self.product({"lib/library.tar.gz": library}))
        self.assertEqual((result.code, self.counts(result, "findings", "refused")), (1, (1, 1)))
        self.assertIn("| p0 | %s | lib/library.tar.gz!library/mid.zip!data/inner.tar!deep/notes.txt | 1 | 1 | no |" % RULE, result.receipt)
        self.assertEqual(result.report["payload"][0]["archives_opened"], 4)
        self.assertEqual(result.listing["positions"][0]["sha256"], sha256(MARK + b"\n"))

    def test_a_zstd_archive_is_opened(self):
        packed = zstd_compress(tar_bytes({"inner/notes.txt": MARK + b"\n"}, compressed=False))
        if packed is None:
            self.skipTest("no zstd encoder on this host")
        result = self.scan(self.product({"data.tar.zst": packed}))
        self.assertEqual((result.code, self.counts(result, "findings", "refused")), (1, (1, 1)))
        self.assertIn("| p0 | %s | data.tar.zst!data.tar!inner/notes.txt | 1 | 1 | no |" % RULE, result.receipt)

    def test_an_archive_that_cannot_be_opened_refuses_unless_its_bytes_are_listed(self):
        broken = b"\x1f\x8b" + b"not a gzip stream at all"
        odd = b"7z\xbc\xaf\x27\x1c" + b"\x00" * 40
        product = self.product({"data/broken.gz": broken, "data/odd.7z": odd})
        result = self.scan(product)
        self.assertEqual((result.code, self.counts(result, "findings", "refused", "unread")), (1, (0, 2, 2)))
        for name in ("data/broken.gz", "data/odd.7z"):
            self.assertIn("| p0 | unopened-archive | %s | 0 | 1 | no |" % name, result.receipt)
        self.assertEqual([(row["path"], row["sha256"]) for row in result.listing["unopened"]],
                         [("data/broken.gz", sha256(broken)), ("data/odd.7z", sha256(odd))])
        listed = [{"path": path, "sha256": sha256(data), "reason": "made up", "accepted_by": "the test"}
                  for path, data in (("data/broken.gz", broken), ("data/odd.7z", odd))]
        result = self.scan(product, accepted=self.accepted(unopened=listed))
        self.assertEqual((result.code, self.counts(result, "refused", "unread", "unread_unaccepted")), (0, (0, 2, 0)))
        result = self.scan(product, accepted=self.accepted(unopened=[listed[0], {**listed[1], "sha256": "0" * 64}]))
        self.assertEqual((result.code, self.counts(result, "refused")), (1, (1,)))
        # A payload file that is itself an archive and cannot be opened refuses under its own name.
        whole = self.payload(broken, name="broken-0.0.0.tar.gz")
        result = self.scan(whole)
        self.assertEqual(result.code, 1)
        self.assertIn("| p0 | unopened-archive | broken-0.0.0.tar.gz | 0 | 1 | no |", result.receipt)

    def test_compiled_files_are_read_one_printable_run_at_a_time(self):
        head = b"\x7fELF\x02\x01\x01" + b"\x00" * 57
        name, value, tail = b"made-up run " + MARK_NAME.encode() + b" =", MARK_VALUE.encode(), b" tail of the run" + b"\x00" * 16
        # The name ends one printable run and the value starts the next: no finding, in either build.
        two_runs = head + name + b"\x00\x02" + value + tail
        result = self.scan(self.product({"bin/tool": two_runs}))
        self.assertEqual((result.code, self.counts(result, "findings", "refused", "unread_unaccepted")), (0, (0, 0, 0)))
        self.assertIn("method artifact: 2 findings", result.receipt)
        self.assertEqual((result.report["elf_files"], result.report["method_artifacts"]), (1, 2))
        for label in ("0.0.1", "0.0.2"):
            raw = result.report["payload"][0]["raw_findings"]
            self.assertEqual((raw[label], raw[label + " ELF strings (runs joined by line)"], raw[label + " ELF strings (each run alone)"]),
                             (0, 1, 0))
        # The runs the joined finding touches went to each build as units of their own.
        for build in (self.pinned, self.current):
            self.assertTrue(any("/strings-isolated-" in word for argv in calls(build) for word in argv))
        # The same bytes inside one run are reported, at the run's byte offset.
        one_run = head + name + b" " + value + tail
        product = self.product({"bin/tool": one_run})
        result = self.scan(product)
        self.assertEqual((result.code, self.counts(result, "findings", "refused")), (1, (1, 1)))
        self.assertIn("| p0 | %s | bin/tool#strings | %d | 1 | no |" % (RULE, len(head)), result.receipt)
        self.assertIn("method artifact: 0 findings", result.receipt)
        self.assertEqual(result.listing["positions"][0]["sha256"], sha256(one_run))
        result = self.scan(product, accepted=self.accepted(positions=[position("bin/tool#strings", one_run, len(head))]))
        self.assertEqual((result.code, self.counts(result, "accepted_positions", "refused")), (0, (1, 0)))
        result = self.scan(product, accepted=self.accepted(positions=[position("bin/tool#strings", one_run, len(head) + 1)]))
        self.assertEqual((result.code, self.counts(result, "accepted_positions", "refused")), (1, (0, 1)))
        result = self.scan(product, accepted=self.accepted(files=[file_entry("bin/tool", one_run)]))
        self.assertEqual((result.code, self.counts(result, "accepted_in_files", "refused")), (0, (1, 0)))

    def test_no_output_gives_a_size_for_a_finding(self):
        head = b"\x7fELF\x02\x01\x01" + b"\x00" * 57
        two_runs = head + b"made-up run " + MARK_NAME.encode() + b" =" + b"\x00\x02" + MARK_VALUE.encode() + b" tail of the run" + b"\x00" * 16
        result = self.scan(self.product({"notes.txt": MARK + b"\n", "bin/tool": two_runs}))
        # A position is rule, path, member digest, line and count: nothing that bounds what matched.
        self.assertEqual({key for row in result.listing["positions"] for key in row},
                         {"payload", "rule", "path", "sha256", "line", "count", "pending"})

        def keys(value):
            if isinstance(value, dict):
                for key, inner in value.items():
                    yield key
                    yield from keys(inner)
            elif isinstance(value, list):
                for inner in value:
                    yield from keys(inner)

        for name in set(keys(result.report)) | set(keys(result.listing)):
            for word in ("length", "width", "span", "column", "end", "match", "secret"):
                self.assertNotIn(word, name.lower().split("_"), name)
        # The method artifact names the run it starts in, and not where the next run starts.
        self.assertIn("| payload | build | ELF path | rule | offset of the run |\n", result.receipt)
        self.assertNotIn("next run", result.receipt)

    def test_vendor_dist_info_and_lock_file_paths_are_read(self):
        result = self.scan(self.product({"vendor/a.txt": MARK + b"\n", "demo-1.0.dist-info/RECORD": MARK + b"\n",
                                         "web/package-lock.json": MARK + b"\n"}))
        self.assertEqual((result.code, self.counts(result, "findings", "refused", "unread")), (1, (3, 3, 0)))
        self.assertEqual([row["allowlist_lines_removed"] for row in result.report["detector_builds"]], [3, 3])

    def test_the_report_names_time_tool_commit_scanned_bytes_builds_and_inputs(self):
        notes = MARK + b"\n"
        product = self.product({"notes.txt": notes})
        accepted = self.accepted(positions=[{**position("notes.txt", notes, 1), "proof": {"kind": "made up", "steps": 2}}])
        pending = self.pending([{"rule": RULE, "path": "other.txt", "line": 1, "sha256": "0" * 64}])
        before = time.time()
        report = self.scan(product, accepted=accepted, pending=pending).report
        self.assertEqual((report["schema"], report["status"]), ("release-payload-credential-scan/report/1", "PASS"))
        self.assertEqual((report["release"], report["name"], report["label"], report["tool_sha256"]),
                         (True, "T", "T", sha256(TOOL.read_bytes())))
        stamp = calendar.timegm(time.strptime(report["generated_at"], "%Y-%m-%dT%H:%M:%SZ"))
        self.assertTrue(before - 2 <= stamp <= time.time() + 2, report["generated_at"])
        self.assertEqual([(row["name"], row["bytes"], row["sha256"]) for row in report["payload"]],
                         [(product.name, product.stat().st_size, sha256(product.read_bytes()))])
        self.assertEqual([(row["role"], row["version"], row["sha256"]) for row in report["detector_builds"]],
                         [("pinned", "0.0.1", sha256(self.pinned.read_bytes())), ("current", "0.0.2", sha256(self.current.read_bytes()))])
        self.assertEqual((report["accepted"]["count"], report["accepted"]["sha256"]), (1, sha256(accepted.read_bytes())))
        self.assertEqual(report["pending"], {"count": 1, "sha256": sha256(pending.read_bytes())})
        report = self.scan(product).report
        self.assertEqual((report["status"], report["accepted"]["count"], report["accepted"]["sha256"], report["pending"]),
                         ("REFUSED", 0, None, {"count": 0, "sha256": None}))
        # A proof is an object or the entry is refused.
        broken = self.accepted(positions=[{**position("notes.txt", notes, 1), "proof": "because"}])
        self.assertEqual(self.scan(product, accepted=broken).code, 2)

    def test_a_proof_only_run_is_named_so_and_is_no_release_report(self):
        product = self.product({"notes.txt": b"plain\n"})
        result = self.scan(product, extra=["--proof-only"])
        self.assertEqual(result.code, 0)
        self.assertTrue(result.line.startswith("CREDENTIAL-LEG PASS PROOF-ONLY-T files=1 "), result.line)
        self.assertEqual((result.report["release"], result.report["name"], result.report["label"], result.report["status"]),
                         (False, "PROOF-ONLY-T", "PROOF-ONLY-T", "PASS"))
        # Only the flag gives that name: a release run cannot be passed off under it, nor the reverse.
        result = self.scan(product, name="PROOF-ONLY-T")
        self.assertEqual((result.code, result.receipt), (2, None))
        self.assertIn("usage error", result.stderr)

    def test_the_tool_commit_is_the_checkout_commit_only_while_the_tool_files_are_unchanged(self):
        if shutil.which("git") is None:
            self.skipTest("no git on this host")
        checkout = self.root / "checkout"
        (checkout / "scripts").mkdir(parents=True)
        copies = [checkout / "scripts" / path.name for path in (TOOL, EXTRACTOR)]
        for source, copy in zip((TOOL, EXTRACTOR), copies):
            shutil.copyfile(source, copy)

        def git(*args):
            done = subprocess.run(["git", "-C", str(checkout), "-c", "user.name=test", "-c", "user.email=test@example.invalid",
                                   "-c", "commit.gpgsign=false", *args], capture_output=True, text=True)
            self.assertEqual(done.returncode, 0, done.stderr)
            return done.stdout.strip()

        def commit_of_a_scan():
            self.count += 1
            report = self.root / f"commit-report-{self.count}.json"
            done = run(copies[0], "--name", "T", "--gitleaks-pinned", self.pinned, "--gitleaks-current", self.current,
                       "--receipt", self.root / f"commit-receipt-{self.count}.txt", "--report", report, "--work", self.root,
                       self.product({"notes.txt": b"plain\n"}))
            self.assertEqual(done.returncode, 0, done.stderr)
            return json.loads(report.read_text())["tool_commit"]

        git("init", "-q")
        self.assertIsNone(commit_of_a_scan())           # no commit yet
        git("add", "scripts/" + copies[0].name)
        git("commit", "-q", "-m", "the tool alone")
        self.assertIsNone(commit_of_a_scan())           # the extractor is not tracked
        git("add", "scripts/" + copies[1].name)
        git("commit", "-q", "-m", "the tool and its extractor")
        head = git("rev-parse", "HEAD")
        self.assertRegex(head, r"\A[0-9a-f]{40}\Z")
        self.assertEqual(commit_of_a_scan(), head)
        with open(copies[1], "a") as stream:
            stream.write("# changed after the commit\n")
        self.assertIsNone(commit_of_a_scan())           # a changed tool file is not that commit's tool

    def test_a_file_no_build_reads_refuses_unless_an_entry_names_its_bytes(self):
        document, lock = b"%PDF-1.4\n" + MARK + b"\n", MARK + b"\n"
        product = self.product({"doc/guide.pdf": document, "deps.zqskip": lock})
        result = self.scan(product)
        self.assertEqual((result.code, result.line.split()[1]), (1, "REFUSED"))
        self.assertEqual(self.counts(result, "findings", "refused", "unread", "unread_unaccepted"), (0, 0, 2, 2))
        self.assertEqual([(row["path"], row["sha256"]) for row in result.listing["unread"]],
                         [("deps.zqskip", sha256(lock)), ("doc/guide.pdf", sha256(document))])
        entries = [file_entry("doc/guide.pdf", document), file_entry("deps.zqskip", lock)]
        result = self.scan(product, accepted=self.accepted(files=entries))
        self.assertEqual((result.code, self.counts(result, "unread", "unread_unaccepted")), (0, (2, 0)))
        result = self.scan(product, accepted=self.accepted(files=[entries[0], {**entries[1], "sha256": "0" * 64}]))
        self.assertEqual((result.code, self.counts(result, "unread", "unread_unaccepted")), (1, (2, 1)))
        # Inside an accepted archive the unread file is covered with the archive.
        wheel = zip_bytes({"doc/guide.pdf": document})
        product = self.product({"lib/wheel-1.0.whl": wheel})
        self.assertEqual(self.scan(product).code, 1)
        result = self.scan(product, accepted=self.accepted(archives=[archive_entry("lib/wheel-1.0.whl", wheel)]))
        self.assertEqual((result.code, self.counts(result, "unread", "unread_unaccepted")), (0, (1, 0)))

    def test_a_file_only_one_build_reads_is_unread_too(self):
        # The unread rule is per build: a file one build's path allowlist skips needs an entry for its
        # exact bytes even though the other build reads it.
        more = DEFAULT_RULES.replace("    '''\\.zqskip$''',\n", "    '''\\.zqskip$''',\n    '''\\.zqonly$''',\n")
        self.assertNotEqual(more, DEFAULT_RULES)
        current = write_build(self.root / "current-more", "0.0.3", True, RULES, more)
        notes = b"plain\n"
        product = self.product({"notes.zqonly": notes})
        result = self.scan(product, current=current)
        self.assertEqual((result.code, self.counts(result, "findings", "refused", "unread", "unread_unaccepted")), (1, (0, 0, 1, 1)))
        self.assertEqual([(row["path"], row["sha256"]) for row in result.listing["unread"]], [("notes.zqonly", sha256(notes))])
        self.assertIn("one build", result.listing["unread"][0]["reason"])
        result = self.scan(product, current=current, accepted=self.accepted(files=[file_entry("notes.zqonly", notes)]))
        self.assertEqual((result.code, self.counts(result, "unread", "unread_unaccepted")), (0, (1, 0)))
        # The entry binds bytes, not the path; and the same bytes under a name no build skips pass.
        other = self.accepted(files=[{**file_entry("notes.zqonly", notes), "sha256": "0" * 64}])
        result = self.scan(product, current=current, accepted=other)
        self.assertEqual((result.code, self.counts(result, "unread", "unread_unaccepted")), (1, (1, 1)))
        result = self.scan(self.product({"notes.txt": notes}), current=current)
        self.assertEqual((result.code, self.counts(result, "findings", "refused", "unread", "unread_unaccepted")), (0, (0, 0, 0, 0)))
        # Inside an accepted archive it is covered with the archive.
        wheel = zip_bytes({"data/notes.zqonly": notes})
        inside = self.product({"lib/wheel-1.0.whl": wheel})
        self.assertEqual(self.scan(inside, current=current).code, 1)
        result = self.scan(inside, current=current, accepted=self.accepted(archives=[archive_entry("lib/wheel-1.0.whl", wheel)]))
        self.assertEqual((result.code, self.counts(result, "unread", "unread_unaccepted")), (0, (1, 0)))
        # A finding the reading build reports there still needs its own acceptance or the file entry.
        marked = self.product({"notes.zqonly": MARK + b"\n"})
        result = self.scan(marked, current=current)
        self.assertEqual((result.code, self.counts(result, "findings", "refused", "unread_unaccepted")), (1, (1, 1, 1)))
        result = self.scan(marked, current=current, accepted=self.accepted(files=[file_entry("notes.zqonly", MARK + b"\n")]))
        self.assertEqual((result.code, self.counts(result, "accepted_in_files", "refused", "unread_unaccepted")), (0, (1, 0, 0)))

    def test_a_member_that_is_not_written_out_refuses(self):
        library = tar_bytes({"library/ok.txt": b"plain\n", "../outside.txt": MARK + b"\n"})
        product = self.product({"lib.tar.gz": library})
        result = self.scan(product)
        # The member is not written out, which refuses on its own; its bytes are no written member's
        # content, so their printable runs are read with the other such bytes of the archive.
        self.assertEqual((result.code, self.counts(result, "findings", "refused", "unread", "unread_unaccepted")), (1, (1, 1, 1, 1)))
        self.assertEqual([row["path"] for row in result.listing["positions"]], ["lib.tar.gz!!outside/tar-headers-and-gaps"])
        self.assertEqual(result.listing["members_not_written"],
                         [{"payload": product.name, "archive": "lib.tar.gz", "sha256": sha256(library), "reason": "dotdot", "count": 1}])
        result = self.scan(product, accepted=self.accepted(archives=[archive_entry("lib.tar.gz", library)]))
        self.assertEqual((result.code, self.counts(result, "unread", "unread_unaccepted")), (0, (1, 0)))
        # A link holds no content: it is counted as a skip and refuses nothing.
        result = self.scan(self.product({"lib.tar.gz": tar_bytes({"library/ok.txt": b"plain\n", "library/link": None})}))
        self.assertEqual((result.code, self.counts(result, "unread")), (0, (0,)))
        self.assertEqual(result.report["payload"][0]["member_skips"], {"tar.gz:link": 1})

    def test_a_payload_cannot_switch_a_finding_off(self):
        allow_all = b'[allowlist]\npaths = [".*"]\n'
        payload = self.payload(tar_bytes({
            "pkg/notes.txt": MARK + b"  # gitleaks:allow\n", "pkg/.gitleaksignore": b"anything\n", "pkg/.gitleaks.toml": allow_all,
            ".gitleaksignore": b"anything\n", ".gitleaks.toml": allow_all}))
        result = self.scan(payload)
        self.assertEqual((result.code, self.counts(result, "findings", "refused")), (1, (1, 1)))
        self.assertIn("| p0 | %s | pkg/notes.txt | 1 | 1 | no |" % RULE, result.receipt)

    def test_a_default_rules_file_that_is_not_the_builds_own_stops_the_scan(self):
        other = self.root / "other-default.toml"
        other.write_text(DEFAULT_RULES.replace("    '''\\.zqskip$''',\n", ""))
        self.assertNotEqual(other.read_text(), DEFAULT_RULES)
        product = self.product({"deps.zqskip": MARK + b"\n"})
        for role in ("pinned", "current"):
            result = self.scan(product, extra=["--default-config-" + role, other])
            self.assertEqual((result.code, result.receipt), (3, None))
            self.assertTrue(result.stderr.startswith("CREDENTIAL-LEG REFUSED tool-failure: "), result.stderr)

    def test_the_two_builds_are_different_files_given_by_absolute_path(self):
        product = self.product({"notes.txt": b"plain\n"})
        alone = self.root / "alone" / "gitleaks"
        alone.parent.mkdir()
        shutil.copyfile(self.current, alone)
        alone.chmod(0o755)
        for pinned, current in ((os.path.relpath(self.pinned), self.current), (self.pinned, self.pinned),
                                (self.pinned, self.root / "absent"), (self.pinned, alone)):
            result = self.scan(product, pinned=pinned, current=current)
            self.assertEqual((result.code, result.receipt), (2, None), (pinned, current))
            self.assertIn("usage error", result.stderr)
        missing = [arg for arg in ("--name", "T", "--gitleaks-pinned", self.pinned, "--receipt", self.root / "r.txt",
                                   "--report", self.root / "r.json", product)]
        self.assertEqual(run(TOOL, *missing).returncode, 2)

    def test_payloads_are_regular_files_and_outputs_are_new(self):
        product = self.product({"notes.txt": b"plain\n"})
        link = self.root / "link.tar.gz"
        link.symlink_to(product)
        for payload in ((), (product.parent,), (link,), (product, self.payload(b"x\n", name=product.name))):
            result = self.scan(*payload)
            self.assertEqual((result.code, result.receipt), (2, None), payload)
            self.assertIn("usage error", result.stderr)
        common = ["--name", "T", "--gitleaks-pinned", self.pinned, "--gitleaks-current", self.current]
        receipt = self.root / "kept.txt"
        receipt.write_text("kept\n")
        done = run(TOOL, *common, "--receipt", receipt, "--report", self.root / "new.json", product)
        self.assertEqual((done.returncode, receipt.read_text(), (self.root / "new.json").exists()), (2, "kept\n", False))


RESOLVED = (0, 1, 1, 0)     # exit code, findings, resolved by class, refused
REFUSED = (1, 1, 0, 1)


class IdentifierClasses(Scans):
    """A finding is resolved by an entry of the committed class list only when the tool itself proves
    what the entry declares. Every row first shows the proof for a well-formed record, then what
    refuses. The list is the file release/payload-credential-identifier-classes.json of the checkout
    the tool runs from, so each row runs a copy of the tool beside a list of its own."""

    MEMBER = "records/zq/summary.json"

    @classmethod
    def setUpClass(cls):
        super().setUpClass()
        cls.ident_pinned = write_build(cls.tools / "ident-pinned", "0.0.1", False, IDENT_RULES)
        cls.ident_current = write_build(cls.tools / "ident-current", "0.0.2", True, IDENT_RULES + RULES)

    def setUp(self):
        super().setUp()
        self.ident = fresh()
        self.good = record(self.ident)
        self.builds = {"pinned": self.ident_pinned, "current": self.ident_current, "values": (self.ident,)}
        self.tool = self.checkout([class_entry()])

    def checkout(self, classes=None, text=None):
        """The tool and its extractor in scripts/ of a directory of their own and, in release/ beside
        it, a class list of these entries or a file of this text. Returns the tool's path there."""
        self.count += 1
        directory = self.root / f"checkout-{self.count}"
        (directory / "scripts").mkdir(parents=True)
        for source in (TOOL, EXTRACTOR):
            shutil.copyfile(source, directory / "scripts" / source.name)
        if classes is not None:
            text = json.dumps({"schema": SCHEMA_CLASSES, "classes": classes}, indent=2) + "\n"
        if text is not None:
            (directory / "release").mkdir()
            (directory / "release" / CLASSES_NAME).write_text(text)
        return directory / "scripts" / TOOL.name

    def outcome(self, result):
        self.assertIsNotNone(result.line, result.stderr)
        return (result.code, *(int(result.fields.get(key, -1)) for key in ("findings", "class_resolved", "refused")))

    def proof(self, tool, resolved):
        """What a report says of the class proof: its name, the number it resolved, and the sha256 of
        the class list that was in force (the file beside the tool)."""
        return {"tool": CLASS_TOOL, "resolved": resolved,
                "list_sha256": sha256((tool.parent.parent / "release" / CLASSES_NAME).read_bytes())}

    def rule_table(self, receipt):
        """The rows of a receipt's rule table: {the first cell: the numbers of the other cells}."""
        lines, rows = receipt.splitlines(), {}
        start = lines.index("| rule | findings | accepted position | accepted in archive | accepted in file | refused |") + 2
        for line in lines[start:]:
            if not line.startswith("|"):
                break
            cells = [cell.strip() for cell in line.strip("|").split("|")]
            rows[cells[0]] = tuple(int(cell) for cell in cells[1:])
        return rows

    def module(self):
        """The tool's own functions, for the rows that ask the proof directly."""
        sys.dont_write_bytecode = True
        spec = importlib.util.spec_from_file_location("credential_scan_identifier_classes", TOOL)
        tool = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(tool)
        return tool

    def classed(self, members, classes=None, name="product-0.0.0-linux-x86_64.tar.gz", **more):
        """(exit code, findings, resolved by class, refused) of one scan of a product with these members
        by the tool beside a class list of these entries: the one entry of class_entry() when none are given."""
        tool = self.tool if classes is None else self.checkout(classes)
        return self.outcome(self.scan(self.product(members, name=name), tool=tool, **{**self.builds, **more}))

    def test_a_class_entry_resolves_a_finding_the_tool_proves_and_an_empty_list_changes_nothing(self):
        product = self.product({self.MEMBER: self.good, "notes.txt": b"plain\n"})
        # Without a list, and with a list of no entry, the finding refuses and every output is as it was.
        absent = self.scan(product, tool=self.checkout(), **self.builds)
        empty = self.scan(product, tool=self.checkout([]), **self.builds)
        for result in (absent, empty):
            self.assertEqual((result.code, self.counts(result, "findings", "refused")), (1, (1, 1)))
            for word in ("class_resolved", "class_tool", "identifier_classes"):
                self.assertNotIn(word, result.receipt + json.dumps(result.report))
        self.assertEqual((absent.receipt, absent.listing), (empty.receipt, empty.listing))
        self.assertEqual({**absent.report, "generated_at": None}, {**empty.report, "generated_at": None})
        resolved = self.scan(product, tool=self.tool, **self.builds)
        self.assertEqual((resolved.code, resolved.stderr), (0, ""))
        self.assertEqual(self.counts(resolved, "findings", "class_resolved", "refused", "accepted_positions"), (1, 1, 0, 0))
        self.assertTrue(resolved.line.endswith(" class_tool=%s class_resolved=1" % CLASS_TOOL), resolved.line)
        # Up to those two words it is the first line of a run without a list that refused nothing.
        self.assertEqual(resolved.line.rsplit(" ", 2)[0],
                         absent.line.replace(" REFUSED ", " PASS ", 1).replace(" refused=1 ", " refused=0 ", 1))
        report = resolved.report
        self.assertEqual((report["status"], report["identifier_classes"]), ("PASS", self.proof(self.tool, 1)))
        self.assertEqual({key for key in report if key not in absent.report}, {"identifier_classes"})
        self.assertEqual((report["findings"]["total"], report["findings"]["refused"], report["findings"]["by_rule"]), (1, 0, {}))
        self.assertEqual(resolved.listing["positions"], [])
        # The receipt and the report state the tool's name and a count: no path, field or rule of what was resolved.
        for text in (resolved.receipt, json.dumps(report)):
            for word in (self.MEMBER, "summary", IDENT_KEY, "holder", IDENT_RULE, "uuid"):
                self.assertNotIn(word, text)
        # A list with an entry that meets nothing says so: the tool's name and the count 0.
        result = self.scan(self.product({"notes.txt": b"plain\n"}), tool=self.tool, **self.builds)
        self.assertEqual((result.code, result.report["identifier_classes"]), (0, self.proof(self.tool, 0)))
        self.assertTrue(result.line.endswith(" class_tool=%s class_resolved=0" % CLASS_TOOL), result.line)

    def test_findings_a_class_resolved_stay_in_the_numbers_of_the_report_and_the_receipt(self):
        parts = ("accepted_positions", "accepted_in_archives", "accepted_in_files", "refused", "class_resolved")

        def numbers(members, tool):
            result = self.scan(self.product(members), tool=tool, **self.builds)
            found = result.report["findings"]
            # One integer under one name, beside the counts that were there: no rule id and no path with it.
            self.assertEqual(set(found), {"total", "positions", "pending_owner", "by_rule", *parts})
            self.assertIs(type(found["class_resolved"]), int)
            # No finding leaves the numbers: the total is the sum of where the findings went.
            self.assertEqual(found["total"], sum(found[part] for part in parts))
            # The receipt's first line and the proof's own entry of the report hold the same integer.
            self.assertEqual(int(result.fields["class_resolved"]), found["class_resolved"])
            self.assertEqual(result.report["identifier_classes"], self.proof(tool, found["class_resolved"]))
            # The receipt's rule table has one line for them, without a rule's name, so that its findings add up too.
            table = self.rule_table(result.receipt)
            self.assertEqual(table["resolved by class (no rule named)"], (found["class_resolved"], 0, 0, 0, 0))
            self.assertEqual(sum(row[0] for row in table.values()), found["total"])
            return result.code, found["total"], found["class_resolved"], found["refused"], found["by_rule"]

        self.assertEqual(numbers({"notes.txt": b"plain\n"}, self.tool), (0, 0, 0, 0, {}))
        self.assertEqual(numbers({self.MEMBER: record(self.ident.upper())}, self.tool), (1, 1, 0, 1, {IDENT_RULE: {"refused": 1}}))
        self.assertEqual(numbers({self.MEMBER: self.good}, self.tool), (0, 1, 1, 0, {}))
        self.assertEqual(numbers({self.MEMBER: self.good, "records/zq/copy.json": self.good}, self.tool),
                         (1, 2, 1, 1, {IDENT_RULE: {"refused": 1}}))
        # Without a list, and with a list of no entry, the report has no such count: it is as it was.
        for tool in (self.checkout(), self.checkout([])):
            result = self.scan(self.product({self.MEMBER: self.good}), tool=tool, **self.builds)
            self.assertEqual(set(result.report["findings"]), {"total", "positions", "pending_owner", "by_rule", *parts[:-1]})
            self.assertNotIn("class_resolved", result.fields)
            self.assertEqual(list(self.rule_table(result.receipt)), [IDENT_RULE])

    def test_an_entry_names_its_payload_file(self):
        # An entry without the payload file name is not an entry that meets every payload file: it is
        # no entry of the schema, and the run stops whatever the payload is called.
        entry = class_entry()
        del entry["asset"]
        tool = self.checkout([entry])
        for name in ("product-0.0.0-linux-x86_64.tar.gz", "other-0.0.0-linux-x86_64.tar.gz"):
            for members in ({self.MEMBER: self.good}, {"notes.txt": b"plain\n"}):
                result = self.scan(self.product(members, name=name), tool=tool, **self.builds)
                self.assertEqual((result.code, result.receipt, result.report), (2, None, None), name)
                self.assertIn("identifier class list", result.stderr)
        # An empty name and a name that is only a place for any text are no payload file name either.
        for value in ("", "<any>", "*", ".*"):
            result = self.scan(self.product({self.MEMBER: self.good}), tool=self.checkout([class_entry(asset=value)]), **self.builds)
            self.assertEqual((result.code, result.receipt), (2, None), value)
        # Nor is a name, or a member path, that is places alone: one of its own letters or digits is needed.
        for key, values in (("asset", ("<target>", "<version>", "<version>-<target>", "<target>.<target>", "-<target>.")),
                            ("member", ("<target>", "<target>/<target>", "<version>/<target>", "<target>!<target>/<version>"))):
            for value in values:
                tool = self.checkout([class_entry(**{key: value})])
                result = self.scan(self.product({self.MEMBER: self.good}), tool=tool, **self.builds)
                self.assertEqual((result.code, result.receipt), (2, None), (key, value))
                self.assertIn("identifier class list", result.stderr)

    def test_only_a_canonical_version_4_identifier_is_proved(self):
        ident = self.ident
        self.assertEqual(self.classed({self.MEMBER: self.good}), RESOLVED)
        others = {
            "version 1": ident[:14] + "1" + ident[15:],
            "version 7": ident[:14] + "7" + ident[15:],
            "a variant that version 4 does not have": ident[:19] + "c" + ident[20:],
            "another such variant": ident[:19] + "0" + ident[20:],
            "upper case": ident.upper(),
            "braces": "{" + ident + "}",
            "a space before": " " + ident,
            "a space after": ident + " ",
            "text before": "id-" + ident,
            "text after": ident + "-1",
            "two in a row": ident + ident,
            "a name before": "urn:uuid:" + ident,
            "no hyphens": ident.replace("-", ""),
            "one character less": ident[:-1],
            "all zero": "00000000-0000-0000-0000-000000000000",
        }
        for name, value in others.items():
            self.assertEqual(self.classed({self.MEMBER: record(value)}), REFUSED, name)

    def test_the_value_sits_at_exactly_the_declared_key_path(self):
        self.assertEqual(self.classed({self.MEMBER: self.good}), RESOLVED)
        whole = json.loads(self.good)
        chain = whole["runs"][0]["outcome"]["holder"]["chain"]
        others = {
            "a sibling field": record(self.ident, pair(self.ident, "zq_ident_other")),
            "one level deeper": as_text({"outer": whole}),
            "inside a list": as_text([whole]),
            "one level less": as_text({"runs": [{"outcome": {"chain": chain}}]}),
            "without the first level": as_text(whole["runs"][0]),
            "under another parent": as_text({"runs": [{"outcome": {"keeper": {"chain": chain}}}]}),
            "at the top": as_text({IDENT_KEY: self.ident, "step": 1}),
            "an object where a list is declared": as_text({"runs": [{"outcome": {"holder": {"chain": chain[0]}}}]}),
            "a list where an object is declared": as_text({"runs": [{"outcome": {"holder": [{"chain": chain}]}}]}),
            "a list in a list": as_text({"runs": [[{"outcome": {"holder": {"chain": chain}}}]]}),
            "one level below the declared field": as_text({"runs": [{"outcome": {"holder": {"chain": [{IDENT_KEY: chain[0]}]}}}]}),
            "objects with a key where the lists are declared": as_text(
                {"runs": {"first": {"outcome": {"holder": {"chain": {"0": chain[0]}}}}}}),
            "an object with a key where one list is declared": as_text({"runs": [{"outcome": {"holder": {"chain": {"first": chain[0]}}}}]}),
        }
        for name, member in others.items():
            self.assertEqual(self.classed({self.MEMBER: member}), REFUSED, name)
        # The declared path is compared whole: an end or a start of the real path, the path without a
        # list step or with one more, and a longer path resolve nothing.
        for path in ("holder.chain[]." + IDENT_KEY, "chain[]." + IDENT_KEY, IDENT_KEY, "runs.outcome.holder.chain." + IDENT_KEY,
                     "runs[].outcome.holder.chain." + IDENT_KEY, "runs[].outcome.holder.chain[][]." + IDENT_KEY,
                     "outer." + RECORD_PATH, RECORD_PATH + ".more", "runs[].outcome.holder.chain", "runs[].outcome", "runs"):
            self.assertEqual(self.classed({self.MEMBER: self.good}, classes=[class_entry(key_path=path)]), REFUSED, path)

    def test_the_line_holds_the_field_and_its_value_and_nothing_else(self):
        self.assertEqual(self.classed({self.MEMBER: self.good}), RESOLVED)
        # The field as the last of its object, without a comma, and a record with CR LF line ends are proved too.
        last = as_text({"runs": [{"outcome": {"holder": {"chain": [{"step": 1, IDENT_KEY: self.ident}]}}}]})
        self.assertEqual(self.classed({self.MEMBER: last}), RESOLVED)
        self.assertEqual(self.classed({self.MEMBER: self.good.replace(b"\n", b"\r\n")}), RESOLVED)
        field = pair(self.ident)
        others = {
            "a second text value after it": record(self.ident, field + ' "note": "made up",'),
            "a second text value before it": record(self.ident, '"note": "made up", ' + field),
            "a number after it": record(self.ident, field + ' "count": 2,'),
            "the end of its object and the start of the next": record(self.ident, field + ' "step": 2 }, {'),
            "the whole document on one line": json.dumps(json.loads(self.good)).encode() + b"\n",
            "the value on the next line": record(self.ident, '"%s":\n"%s",' % (IDENT_KEY, self.ident)),
        }
        for name, member in others.items():
            json.loads(member)      # each is a JSON document: the line is what refuses
            self.assertEqual(self.classed({self.MEMBER: member}), REFUSED, name)

    def test_the_member_is_one_json_document_without_a_repeated_key(self):
        self.assertEqual(self.classed({self.MEMBER: self.good}), RESOLVED)
        second = fresh()
        others = {
            "words after the document": (self.good + b"made-up words\n", 1),
            "a second document": (self.good + self.good, 2),
            "an unclosed document": (self.good.rstrip()[:-1], 1),
            "a comment": (self.good.replace(b'"step": 1', b'"step": 1 // made up'), 1),
            "a constant that JSON does not have": (self.good.replace(b'"step": 1', b'"step": Infinity'), 1),
            "another such constant": (self.good.replace(b'"step": 1', b'"step": NaN'), 1),
            "a byte that is no UTF-8": (self.good.replace(b"made-up record", b"made-up \xff record"), 1),
            "a byte order mark": (b"\xef\xbb\xbf" + self.good, 1),
            "the field twice in its object": (record(self.ident, pair(self.ident) + "\n" + pair(second)), 2),
            "another key twice in its object": (self.good.replace(b'"step": 1', b'"step": 1,\n"step": 1'), 1),
            "a key twice in another object": (self.good.replace(b'  "kind"', b'  "more": {"a": 1, "a": 2},\n  "kind"'), 1),
        }
        for name, (member, findings) in others.items():
            self.assertNotEqual(member, self.good, name)
            self.assertEqual(self.classed({self.MEMBER: member}, values=(self.ident, second)), (1, findings, 0, findings), name)

    def test_rule_asset_and_member_path_are_the_entrys(self):
        good = self.good
        self.assertEqual(self.classed({self.MEMBER: good}), RESOLVED)
        # The same record at another path, or inside a nested archive.
        for members in ({"records/zq/other.json": good}, {"records/summary.json": good}, {"more/" + self.MEMBER: good},
                        {self.MEMBER + ".txt": good}, {"records/zq/summaryxjson": good},
                        {"lib.tar.gz": tar_bytes({self.MEMBER: good})}, {"lib.zip": zip_bytes({self.MEMBER: good})}):
            self.assertEqual(self.classed(members), REFUSED, sorted(members))
        # The same record in a payload file of another name. A dot of the entry's name is a dot, a version
        # is three numbers without a leading zero, and a target is two parts with one hyphen between them.
        for name in ("other-0.0.0-linux-x86_64.tar.gz", "product-0.0-linux-x86_64.tar.gz", "product-0.0.0-Linux.tar.gz",
                     "a-product-0.0.0-linux-x86_64.tar.gz", "product-v0.0.0-linux-x86_64.tar.gz",
                     "product-0.0.0-linux-x86_64xtar.gz", "product-00.0.0-linux-x86_64.tar.gz", "product-0.01.0-linux-x86_64.tar.gz",
                     "product-0.0.0-rc1-linux-x86_64.tar.gz", "product-0.0.0-linux.tar.gz", "product-0.0.0-linux-x86_64-more.tar.gz"):
            self.assertEqual(self.classed({self.MEMBER: good}, name=name), REFUSED, name)
        # The name is that of the payload file that holds the finding, not of another file of the same run.
        right, wrong = "product-0.0.0-linux-x86_64.tar.gz", "other-0.0.0-linux-x86_64.tar.gz"
        for holder, expected in ((wrong, REFUSED), (right, RESOLVED)):
            for names in ((right, wrong), (wrong, right)):
                files = [self.product({self.MEMBER: good} if name == holder else {"notes.txt": b"plain\n"}, name=name) for name in names]
                self.assertEqual(self.outcome(self.scan(*files, tool=self.tool, **self.builds)), expected, (holder, names))
        # An entry that differs in one of the three, or names only a part of the name or the path, resolves nothing.
        for key, value in (("rule", TAG_RULE), ("rule", IDENT_RULE + "-b"), ("rule", IDENT_RULE[:-1]),
                           ("member", "records/zq/other.json"), ("member", "summary.json"), ("member", "zq/summary.json"),
                           ("member", "records/zq/summary"), ("member", "records/zq"), ("asset", "other-<version>-<target>.tar.gz"),
                           ("asset", "product-<version>.tar.gz"), ("asset", "product-0.0.1-<target>.tar.gz"),
                           ("asset", "product-<version>-<target>.tar"), ("asset", "roduct-<version>-<target>.tar.gz"),
                           ("asset", "product-<version>-<target>xtar.gz")):
            self.assertEqual(self.classed({self.MEMBER: good}, classes=[class_entry(**{key: value})]), REFUSED, (key, value))
        # A name without a place is that name, and a place of a member path stands for a version or a target.
        for value in ("product-0.0.0-linux-x86_64.tar.gz", "product-<version>-linux-x86_64.tar.gz"):
            self.assertEqual(self.classed({self.MEMBER: good}, classes=[class_entry(asset=value)]), RESOLVED, value)
        placed = [class_entry(member="records-<version>/<target>/summary.json")]
        self.assertEqual(self.classed({"records-0.0.0/linux-x86_64/summary.json": good}, classes=placed), RESOLVED)
        for path in ("records-0.0/linux-x86_64/summary.json", "records-0.0.0/more/linux/summary.json", "records-0.0.0/Linux/summary.json",
                     "records-00.0.0/linux-x86_64/summary.json", "records-0.0.0/linux/summary.json",
                     "records-0.0.0/linux-x86_64-more/summary.json"):
            self.assertEqual(self.classed({path: good}, classes=placed), REFUSED, path)
        # A second rule that reports the same line is not the entry's rule: its finding refuses.
        both = write_build(self.root / "both", "0.0.3", True, IDENT_RULES + [(IDENT_RULE + "-b", IDENT_RULES[0][1])])
        self.assertEqual(self.classed({self.MEMBER: good}, current=both), (1, 2, 1, 1))

    def test_each_finding_is_resolved_on_its_own_proof(self):
        second = fresh()
        both = (self.ident, second)

        def chained(*values, **more):
            return as_text({"runs": [{"outcome": {"holder": {"chain": [
                {"step": step, IDENT_KEY: value, **more} for step, value in enumerate(values)]}}}]})

        # Two fields that are both proved, in one list and in two.
        self.assertEqual(self.classed({self.MEMBER: chained(*both)}, values=both), (0, 2, 2, 0))
        two_runs = as_text({"runs": [{"outcome": {"holder": {"chain": [{"step": 1, IDENT_KEY: value}]}}} for value in both]})
        self.assertEqual(self.classed({self.MEMBER: two_runs}, values=both), (0, 2, 2, 0))
        # One of the two values is not of the form: it refuses at its own line, and the other is resolved.
        member = chained(self.ident, second.upper())
        result = self.scan(self.product({self.MEMBER: member}), tool=self.tool, **{**self.builds, "values": both})
        self.assertEqual(self.outcome(result), (1, 2, 1, 1))
        line = 1 + [number for number, text in enumerate(member.split(b"\n")) if second.upper().encode() in text][0]
        self.assertEqual([(row["path"], row["line"]) for row in result.listing["positions"]], [(self.MEMBER, line)])
        # A sibling field with an identifier of the form, and the same record at a second path, are not the entry's.
        self.assertEqual(self.classed({self.MEMBER: chained(self.ident, zq_ident_other=second)}, values=both), (1, 2, 1, 1))
        self.assertEqual(self.classed({self.MEMBER: self.good, "records/zq/copy.json": self.good}), (1, 2, 1, 1))

    def test_every_reported_span_lies_on_the_line(self):
        self.assertEqual(self.classed({self.MEMBER: self.good}), RESOLVED)
        line = self.good.split(b"\n")[RECORD_LINE - 1]
        first, last = line.index(b"zq_ident") + 1, line.rindex(b'"') + 1    # the columns the stand-in reports
        skews = {
            "the span ends on the next line": {"EndLine": 1},
            "the span ends on an earlier line": {"EndLine": -1},
            "no end line": {"EndLine": None},
            "no start column": {"StartColumn": None},
            "no end column": {"EndColumn": None},
            "the span starts before the line": {"StartColumn": -first},
            "the span starts after the line": {"StartColumn": len(line) + 2 - first},
            "the span ends after the line": {"EndColumn": len(line) + 2 - last},
            "the span ends before the line": {"EndColumn": -last},
        }
        for number, (name, skew) in enumerate(skews.items()):
            for role, version in (("pinned", "0.0.1"), ("current", "0.0.2")):
                build = write_build(self.root / f"skew-{number}-{role}", version, role == "current", IDENT_RULES, skew=skew)
                self.assertEqual(self.classed({self.MEMBER: self.good}, **{role: build}), REFUSED, (name, role))
        # The first column and the one after the last byte are on the line (a build may count a later line's columns from 2).
        edge = write_build(self.root / "edge", "0.0.2", True, IDENT_RULES,
                           skew={"StartColumn": 1 - first, "EndColumn": len(line) + 1 - last})
        self.assertEqual(self.classed({self.MEMBER: self.good}, current=edge), RESOLVED)

    def test_the_proof_needs_a_span_of_every_finding_and_a_line_of_the_member(self):
        sys.dont_write_bytecode = True
        spec = importlib.util.spec_from_file_location("credential_scan_identifier_classes", TOOL)
        tool = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(tool)
        (entry,) = tool.validate_classes({"schema": SCHEMA_CLASSES, "classes": [class_entry()]})
        width = len(self.good.split(b"\n")[RECORD_LINE - 1])
        span = (RECORD_LINE, 1, width)
        self.assertIs(tool.class_proves(entry, self.good, RECORD_LINE, [span]), True)
        self.assertIs(tool.class_proves(entry, self.good, RECORD_LINE, [span, span]), True)
        for spans in ([], [span, (RECORD_LINE + 1, 1, width)], [(RECORD_LINE, True, width)], [(RECORD_LINE, 1.0, width)],
                      [(float(RECORD_LINE), 1, width)], [(RECORD_LINE, "1", width)], [(RECORD_LINE, 1, None)], [(None, 1, width)]):
            self.assertIs(tool.class_proves(entry, self.good, RECORD_LINE, spans), False, spans)
        for line in (0, -1, RECORD_LINE - 1, RECORD_LINE + 1, len(self.good.split(b"\n")), 10 ** 6):
            self.assertIs(tool.class_proves(entry, self.good, line, [(line, 1, 1)]), False, line)

    def test_a_list_that_is_not_of_its_schema_stops_every_run(self):
        clean, marked = self.product({"notes.txt": b"plain\n"}), self.product({self.MEMBER: self.good})
        entry = class_entry()

        def stopped(text, why):
            tool = self.checkout(text=text)
            for product in (clean, marked):
                result = self.scan(product, tool=tool, **self.builds)
                self.assertEqual((result.code, result.receipt, result.report, result.listing), (2, None, None, None), why)
                self.assertIn("identifier class list", result.stderr, why)

        entries = [{key: value for key, value in entry.items() if key != missing} for missing in entry]
        entries += [{**entry, "note": "made up"}, {**entry, "proof": {}}, {**entry, "sha256": "0" * 64}, "text", [], None]
        for key, values in (
                ("value_form", ("uuid", "uuid-v1", "UUID-V4", "uuid-v4 ", "any", "", 4, None, ["uuid-v4"])),
                ("accepted_by", ("", "  ", 3, None)),
                ("rule", ("", " ", "a rule", 5, None)),
                ("asset", ("", "product-<name>.tar.gz", "<version", "product/<version>.tar.gz", "product *.tar.gz", 7)),
                ("member", ("", "/records/zq/summary.json", "records/<name>/summary.json", "records/../summary.json",
                            "records//summary.json", "records/zq/", "records/./summary.json", "records/zq/summary .json", 7)),
                ("key_path", ("", ".", "[]", "[]." + IDENT_KEY, IDENT_KEY + "[]", "runs..chain", "runs.", ".runs", "runs.chain-a",
                              "runs.[].chain", "runs[0].chain", "runs.*.chain", "runs[] .chain", "runs[", 3, ["runs"]))):
            entries += [{**entry, key: value} for value in values]
        for broken in entries:
            stopped(json.dumps({"schema": SCHEMA_CLASSES, "classes": [broken]}), broken)
        for whole in ([], "text", {"schema": "another/identifier-classes/1", "classes": []}, {"schema": SCHEMA_CLASSES},
                      {"classes": []}, {"schema": SCHEMA_CLASSES, "entries": []}, {"schema": SCHEMA_CLASSES, "classes": {}},
                      {"schema": SCHEMA_CLASSES, "classes": [], "more": 1}, {"schema": SCHEMA_CLASSES, "classes": [entry, entry]}):
            stopped(json.dumps(whole), whole)
        for text in ("", "not a document\n", json.dumps({"schema": SCHEMA_CLASSES, "classes": []}) + "}"):
            stopped(text, text)

    def ordered(self, reverse):
        """The two stand-in builds of the identifier rule, reporting their findings as they walk the
        tree or in the opposite order (a real build reads files in no fixed order)."""
        self.count += 1
        return {role: write_build(self.root / f"order-{self.count}-{role}", version, role == "current", IDENT_RULES, reverse=reverse)
                for role, version in (("pinned", "0.0.1"), ("current", "0.0.2"))}

    def said(self, result):
        """What a scan said: its exit code, the words of its first line but for the digests of the two
        builds that ran it, and its listing."""
        self.assertIsNotNone(result.line, result.stderr)
        return result.code, {key: value for key, value in result.fields.items() if key != "build_sha256"}, result.listing

    def two_files(self):
        """A product with an archive and, beside it, a directory named like the archive and the
        separator: (the path both files would print as, the archive, the bytes of its member, the bytes
        of the directory's file, the product, the two identifiers). Each file holds an identifier of
        its own that the class would prove of the member."""
        inside, other = "inner.tar!" + self.MEMBER, fresh()
        archive, beside = tar_bytes({self.MEMBER: self.good}, compressed=False), record(other)
        self.assertNotEqual(beside, self.good)
        return inside, archive, self.good, beside, self.product({"inner.tar": archive, inside: beside}), (self.ident, other)

    def test_two_files_that_would_print_as_one_path_are_never_one_member(self):
        # A directory whose name holds the archive separator, beside an archive of that name: a file of
        # the directory would print as the path of a member of the archive. It is not written out, for
        # its name, so that path names the member of the archive and nothing else. The class reads that
        # one file. The finding in the directory's file is resolved by nothing, whichever of the two a
        # build reports first, though the file holds an identifier the class would prove of the member.
        inside, archive, member, beside, product, values = self.two_files()
        entry = [class_entry(member=inside)]
        # The member of the archive alone is the entry's member.
        self.assertEqual(self.classed({"inner.tar": archive}, classes=entry), RESOLVED)
        seen = []
        for reverse in (False, True):
            result = self.scan(product, tool=self.checkout(entry), **{**self.builds, **self.ordered(reverse), "values": values})
            # Two findings. The member's is resolved; the other one refuses, and with it the payload.
            self.assertEqual(self.outcome(result), (1, 2, 1, 1), reverse)
            self.assertEqual((result.report["status"], self.counts(result, "unread", "unread_unaccepted")), ("REFUSED", (1, 1)), reverse)
            self.assertEqual(result.listing["members_not_written"], [
                {"payload": product.name, "archive": product.name, "sha256": sha256(product.read_bytes()),
                 "reason": "separator-in-name", "count": 1}], reverse)
            # The refused finding does not stand at the member's path, and its file is neither of the two:
            # it lies among the bytes of the payload that are no written member's.
            self.assertEqual([(row["rule"], row["path"], row["count"]) for row in result.listing["positions"]],
                             [(IDENT_RULE, OUTSIDE + "tar-headers-and-gaps", 1)], reverse)
            self.assertNotIn(result.listing["positions"][0]["sha256"], (sha256(member), sha256(beside)), reverse)
            seen.append(self.said(result))
        self.assertEqual(seen[0], seen[1])      # the same outcome in both report orders
        # The same asked of the proof itself: a position whose findings lie in two files is not proved by
        # reading one of them, even when both hold the same bytes.
        tool = self.module()
        (ready,) = tool.validate_classes({"schema": SCHEMA_CLASSES, "classes": [class_entry()]})
        one, two = self.root / "one.json", self.root / "two.json"
        for path in (one, two):
            path.write_bytes(self.good)

        def resolves(*files):
            rec = {"member": str(one), "spans": [(str(file), RECORD_LINE, 1, 1) for file in files]}
            return tool.class_resolves(ready, "product-0.0.0-linux-x86_64.tar.gz", IDENT_RULE, self.MEMBER, rec, RECORD_LINE)

        self.assertIs(resolves(one), True)
        self.assertIs(resolves(one, one), True)
        for files in ((one, two), (two, one), (two,), (two, two)):
            self.assertIs(resolves(*files), False, files)

    def test_no_acceptance_and_no_class_passes_a_file_that_would_print_as_the_path_of_an_archive_member(self):
        # The payload of the row above under every acceptance that names the path or the bytes of the
        # archive's member, without a class and with the entry for that path, in both report orders. It
        # never passes; the outcome does not depend on the order; an acceptance or the class takes the
        # member's finding and no more; the finding in the directory's file is refused every time.
        inside, archive, member, beside, product, values = self.two_files()
        bound, wrong = (position(inside, data, RECORD_LINE, rule=IDENT_RULE) for data in (member, beside))
        # What is accepted, and the member's finding as (accepted position, in archive, in file).
        cases = {
            "nothing": ({}, (0, 0, 0)),
            "a files entry": ({"files": [file_entry(inside, member)]}, (0, 0, 1)),
            "an archives entry for the archive": ({"archives": [archive_entry("inner.tar", archive)]}, (0, 1, 0)),
            "a positions entry, count 1": ({"positions": [bound]}, (1, 0, 0)),
            "a positions entry, count 2": ({"positions": [{**bound, "count": 2}]}, (0, 0, 0)),
            "a files entry for the bytes of the directory's file": ({"files": [file_entry(inside, beside)]}, (0, 0, 0)),
            "a positions entry for those bytes, count 1": ({"positions": [wrong]}, (0, 0, 0)),
            "a positions entry for those bytes, count 2": ({"positions": [{**wrong, "count": 2}]}, (0, 0, 0)),
            "entries for the bytes of both": ({"files": [file_entry(inside, member), file_entry(inside, beside)],
                                               "positions": [bound, wrong]}, (1, 0, 0)),
        }
        orders = [self.ordered(reverse) for reverse in (False, True)]
        tools = {False: self.checkout(), True: self.checkout([class_entry(member=inside)])}
        names = ("findings", "accepted_positions", "accepted_in_archives", "accepted_in_files", "refused", "unread", "unread_unaccepted")
        for name, (lists, taken) in cases.items():
            accepted = self.accepted(**lists) if lists else None
            for classed, tool in tools.items():
                results = [self.scan(product, tool=tool, accepted=accepted, **{**self.builds, **builds, "values": values})
                           for builds in orders]
                note = (name, classed)
                self.assertEqual(self.said(results[0]), self.said(results[1]), note)    # the same in both report orders
                result = results[0]
                self.assertEqual((result.code, result.report["status"], result.stderr), (1, "REFUSED", ""), note)
                by_class = 1 if classed and not any(taken) else 0
                self.assertEqual(self.counts(result, *names), (2, *taken, 2 - sum(taken) - by_class, 1, 1), note)
                self.assertEqual(int(result.fields.get("class_resolved", 0)), by_class, note)
                # The finding in the directory's file is among the refused, in the payload's bytes that are
                # no written member's; the member's own is there when nothing took it.
                refused = [(row["rule"], row["path"], row["count"], row["line"], row["sha256"]) for row in result.listing["positions"]]
                own = [] if any(taken) or by_class else [(IDENT_RULE, inside, 1, RECORD_LINE, sha256(member))]
                self.assertEqual(refused[0][:3], (IDENT_RULE, OUTSIDE + "tar-headers-and-gaps", 1), note)
                self.assertEqual(refused[1:], own, note)
                self.assertNotIn(sha256(beside), [row[4] for row in refused], note)

    def test_a_files_entry_for_an_archive_member_accepts_no_finding_in_a_directory_named_like_the_archive(self):
        # No class here. A files entry is bound to the bytes of one file: the member of the archive. A
        # finding in the file of a directory that would print as that member's path is not accepted with
        # it, whichever of the two a build reports first, and the payload does not pass.
        inside, archive, member, beside, product, values = self.two_files()
        tool = self.checkout()
        for data, taken in ((member, 1), (beside, 0)):
            accepted = self.accepted(files=[file_entry(inside, data)])
            results = [self.scan(product, tool=tool, accepted=accepted, **{**self.builds, **self.ordered(reverse), "values": values})
                       for reverse in (False, True)]
            self.assertEqual([result.code for result in results], [1, 1], taken)        # it passes in neither order
            self.assertEqual(self.said(results[0]), self.said(results[1]), taken)
            for result in results:
                self.assertEqual(self.counts(result, "findings", "accepted_in_files", "refused"), (2, taken, 2 - taken), taken)
                self.assertNotIn("class_resolved", result.fields)
        # Without the directory the entry accepts the member's finding and the payload passes.
        result = self.scan(self.product({"inner.tar": archive}), tool=tool, accepted=self.accepted(files=[file_entry(inside, member)]),
                           **self.builds)
        self.assertEqual((result.code, self.counts(result, "findings", "accepted_in_files", "refused")), (0, (1, 1, 0)))

    def test_a_member_name_with_the_separator_refuses_by_the_name_alone(self):
        # A lone member whose name holds the separator, with no archive beside it. It is not written out
        # whatever it holds, so nothing that names its path is ever asked: no class entry and no
        # acceptance of a position or of a file at that path passes the payload, in either report order.
        name, plain = "lone!" + self.MEMBER, {"notes.txt": b"plain\n"}
        entry = [class_entry(member=name)]
        orders = [self.ordered(reverse) for reverse in (False, True)]
        holds = {"an identifier the class would prove": self.good, "another document": as_text({"kind": "made up"}), "plain text": b"plain\n"}
        rows = []
        for what, data in holds.items():
            product = self.product({name: data, **plain})
            lists = ({}, {"files": [file_entry(name, data)]}, {"positions": [position(name, data, RECORD_LINE, rule=IDENT_RULE)]},
                     {"files": [file_entry(name, data)], "positions": [position(name, data, RECORD_LINE, rule=IDENT_RULE)]})
            for accepted in lists:
                for tool in (self.checkout(), self.checkout(entry)):
                    results = [self.scan(product, tool=tool, accepted=self.accepted(**accepted) if accepted else None,
                                         **{**self.builds, **builds}) for builds in orders]
                    note = (what, sorted(accepted))
                    self.assertEqual(self.said(results[0]), self.said(results[1]), note)
                    result = results[0]
                    self.assertEqual((result.code, result.report["status"], result.stderr), (1, "REFUSED", ""), note)
                    self.assertEqual(self.counts(result, "accepted_positions", "accepted_in_archives", "accepted_in_files",
                                                 "unread", "unread_unaccepted"), (0, 0, 0, 1, 1), note)
                    self.assertEqual(int(result.fields.get("class_resolved", 0)), 0, note)
                    self.assertEqual(self.counts(result, "findings"), self.counts(result, "refused"), note)
                    self.assertNotIn(name, [row["path"] for row in result.listing["positions"] + result.listing["unread"]], note)
                    (row,) = result.listing["members_not_written"]
                    self.assertEqual(row["sha256"], sha256(product.read_bytes()), note)
                    rows.append({key: value for key, value in row.items() if key not in ("sha256", "payload")})
            # Under a name without the separator the same bytes are a member like any other.
            expected = RESOLVED if data == self.good else (0, 0, 0, 0)
            self.assertEqual(self.classed({name.replace("!", "-"): data, **plain}, classes=[class_entry(member=name.replace("!", "-"))]),
                             expected, what)
        # What is said of the member is the same whatever it holds: the reason is its name.
        self.assertEqual({json.dumps(row, sort_keys=True) for row in rows},
                         {json.dumps({"archive": "product-0.0.0-linux-x86_64.tar.gz", "reason": "separator-in-name", "count": 1}, sort_keys=True)})

    def test_a_whole_archive_acceptance_covers_a_member_refused_for_its_name_and_asks_nothing_keyed_by_a_path(self):
        # An archives entry accepts one exact archive, by its digest, with everything inside it: also a
        # member that is not written out, whatever the reason, a name with the separator included. That
        # is meant. Under such an entry nothing inside is asked by its printed path, so two files that
        # would print as one path cannot share an entry there; and nothing but the bytes of the
        # enclosing archive gives it (no entry at a path does: the rows above).
        inside, archive, member, beside, product, values = self.two_files()
        lone = self.product({"lone!" + self.MEMBER: b"plain\n", "notes.txt": b"plain\n"})
        orders = [self.ordered(reverse) for reverse in (False, True)]
        names = ("findings", "accepted_positions", "accepted_in_archives", "accepted_in_files", "refused", "unread", "unread_unaccepted")
        for payload, findings in ((product, 2), (lone, 0)):
            whole = archive_entry(payload.name, payload.read_bytes())
            for tool in (self.checkout(), self.checkout([class_entry(member=inside)])):
                results = [self.scan(payload, tool=tool, accepted=self.accepted(archives=[whole]), **{**self.builds, **builds, "values": values})
                           for builds in orders]
                self.assertEqual(self.said(results[0]), self.said(results[1]), findings)
                result = results[0]
                self.assertEqual((result.code, result.report["status"]), (0, "PASS"), findings)
                # Every finding is accepted with the archive; none by a position, a file or the class.
                self.assertEqual(self.counts(result, *names), (findings, 0, findings, 0, 0, 1, 0), findings)
                self.assertEqual(int(result.fields.get("class_resolved", 0)), 0, findings)
                self.assertEqual(result.listing["members_not_written"], [])
            # The entry is the enclosing archive's bytes: with another digest, or for the archive beside
            # the member, the member stays refused.
            for entry in ({**whole, "sha256": "0" * 64}, archive_entry("inner.tar", archive)):
                result = self.scan(payload, tool=self.checkout(), accepted=self.accepted(archives=[entry]),
                                   **{**self.builds, "values": values})
                self.assertEqual((result.code, self.counts(result, "unread", "unread_unaccepted")), (1, (1, 1)), findings)
                self.assertEqual([row["reason"] for row in result.listing["members_not_written"]], ["separator-in-name"], findings)

    def test_the_class_reads_one_file_where_an_extractor_writes_such_a_name_out(self):
        # That such a name is not written out is the extractor's doing. The class does not lean on it:
        # beside an extractor that writes every name out, the two files print as one path, one position
        # holds the findings of both, and the class resolves neither, whichever a build reports first,
        # though each file alone holds what it would prove.
        inside, archive, member, beside, product, values = self.two_files()
        tool = self.checkout([class_entry(member=inside)])
        with open(tool.parent / EXTRACTOR.name, "a") as stream:
            stream.write("\n\ndef printed_parts(parts, directory):\n    return (parts, None)\n")
        seen = []
        for reverse in (False, True):
            result = self.scan(product, tool=tool, **{**self.builds, **self.ordered(reverse), "values": values})
            self.assertEqual(self.outcome(result), (1, 2, 0, 2), reverse)
            self.assertEqual(self.counts(result, "unread"), (0,), reverse)      # both files were written out
            self.assertEqual([(row["path"], row["line"], row["count"]) for row in result.listing["positions"]],
                             [(inside, RECORD_LINE, 2)], reverse)
            seen.append(result.listing["positions"][0]["sha256"])
        # The one position is filed under the file a build reported first: the member in one order, the
        # directory's file in the other. In neither is the other file's finding proved with it.
        self.assertEqual(sorted(seen), sorted((sha256(member), sha256(beside))))
        # Each file alone, beside that extractor, is the entry's member and is resolved.
        self.assertEqual(self.outcome(self.scan(self.product({"inner.tar": archive}), tool=tool, **self.builds)), RESOLVED)
        self.assertEqual(self.outcome(self.scan(self.product({inside: beside}), tool=tool, **{**self.builds, "values": values})), RESOLVED)

    def bound_files(self):
        """A committed checkout of the tool, its extractor and a class list: (the checkout, a function
        that runs git there, a function that gives the tool commit of a scan from it, the commit, and
        an edit for each of the three files the tool commit is bound to)."""
        tool = self.checkout([class_entry()])
        checkout = tool.parent.parent

        def git(*args):
            done = subprocess.run(["git", "-C", str(checkout), "-c", "user.name=test", "-c", "user.email=test@example.invalid",
                                   "-c", "commit.gpgsign=false", "-c", "core.safecrlf=false", *args], capture_output=True, text=True)
            self.assertEqual(done.returncode, 0, done.stderr)
            return done.stdout.rstrip("\n")

        def commit_of_a_scan():
            result = self.scan(self.product({self.MEMBER: self.good}), tool=tool, **self.builds)
            self.assertEqual(self.outcome(result), RESOLVED)
            return result.report["tool_commit"]

        git("init", "-q")
        git("add", "-A")
        git("commit", "-q", "-m", "the tools and the list")
        head = git("rev-parse", "HEAD")
        self.assertEqual(commit_of_a_scan(), head)
        edits = {"release/" + CLASSES_NAME: lambda text: text.replace('"the test"', '"another test"'),
                 "scripts/" + TOOL.name: lambda text: text + "\n# changed after the commit\n",
                 "scripts/" + EXTRACTOR.name: lambda text: text + "\n# changed after the commit\n"}
        return checkout, git, commit_of_a_scan, head, edits

    def test_a_staged_edit_of_a_bound_file_is_not_the_commits(self):
        # The measure is the commit's file, not git's index: a bound file whose edit is staged is not the
        # commit's, and a file with the commit's bytes is, whatever is staged for it.
        if shutil.which("git") is None:
            self.skipTest("no git on this host")
        checkout, git, commit_of_a_scan, head, edits = self.bound_files()
        for name, edit in edits.items():
            path = checkout / name
            text = path.read_text()
            path.write_text(edit(text))
            self.assertNotEqual(path.read_text(), text, name)
            git("add", "--", name)
            self.assertEqual(git("status", "--porcelain"), "M  " + name)    # staged, and the file is what the index holds
            self.assertIsNone(commit_of_a_scan(), name)
            path.write_text(text)       # the commit's bytes again; the index still holds the edit
            self.assertEqual(git("status", "--porcelain"), "MM " + name)
            self.assertEqual(commit_of_a_scan(), head, name)
            git("add", "--", name)
            self.assertEqual(git("status", "--porcelain"), "")

    def test_a_bound_file_that_only_a_git_filter_makes_the_commits_is_not_the_commits(self):
        # git can be told to change a file's bytes on their way in (an attribute for line ends, a filter
        # of the repository); a file that holds other bytes is then hashed to the commit's object. The
        # tool compares the bytes as they are in the file: what it runs and reads is not passed through git.
        if shutil.which("git") is None:
            self.skipTest("no git on this host")
        checkout, git, commit_of_a_scan, head, edits = self.bound_files()
        (checkout / ".git" / "info").mkdir(exist_ok=True)
        (checkout / ".git" / "info" / "attributes").write_text("*.json text\n*.py text\n")
        self.assertEqual(commit_of_a_scan(), head)
        for name in edits:
            path = checkout / name
            raw = path.read_bytes()
            self.assertNotIn(b"\r", raw, name)
            path.write_bytes(raw.replace(b"\n", b"\r\n"))       # the same lines, each with another line end
            held = git("rev-parse", "HEAD:" + name)
            # Hashed through git's filters the file is the commit's object; hashed as it is, it is not.
            self.assertEqual(git("hash-object", "--", name), held, name)
            self.assertNotEqual(git("hash-object", "--no-filters", "--", name), held, name)
            self.assertIsNone(commit_of_a_scan(), name)
            path.write_bytes(raw)
            self.assertEqual(commit_of_a_scan(), head, name)

    def test_a_list_that_repeats_a_key_stops_every_run(self):
        good = json.dumps({"schema": SCHEMA_CLASSES, "classes": [class_entry()]})
        texts = {
            "classes twice, the first without an entry": good.replace('"classes": [', '"classes": [], "classes": [', 1),
            "classes twice, the last without an entry": good[:-1] + ', "classes": []}',
            "schema twice": good.replace('"schema":', '"schema": "another/identifier-classes/1", "schema":', 1),
            "the key path of an entry twice": good.replace('"key_path":', '"key_path": "runs", "key_path":', 1),
            "accepted_by of an entry twice": good.replace('"accepted_by":', '"accepted_by": "", "accepted_by":', 1),
        }
        for name, text in texts.items():
            self.assertNotEqual(text, good, name)
            json.loads(text)        # a reader that keeps the last of two keys takes each of these
            tool = self.checkout(text=text)
            for members in ({self.MEMBER: self.good}, {"notes.txt": b"plain\n"}):
                result = self.scan(self.product(members), tool=tool, **self.builds)
                self.assertEqual((result.code, result.receipt, result.report), (2, None, None), name)
                self.assertIn("identifier class list", result.stderr, name)

    def test_a_long_field_line_is_decided(self):
        # No number of blanks around the comma of a field line keeps the proof from its answer. The row
        # states the answers; a proof that does not come to one does not end.
        tool = self.module()
        (ready,) = tool.validate_classes({"schema": SCHEMA_CLASSES, "classes": [class_entry()]})
        line = self.good.split(b"\n")[RECORD_LINE - 1]
        field, blanks = line.rstrip(b","), b" " * 1000000

        def proved(text):
            member = self.good.replace(line, text, 1)
            self.assertNotEqual(member, self.good)
            return tool.class_proves(ready, member, RECORD_LINE, [(RECORD_LINE, 1, len(text) + 1)])

        self.assertIs(tool.class_proves(ready, self.good, RECORD_LINE, [(RECORD_LINE, 1, len(line) + 1)]), True)
        self.assertIs(proved(field + blanks + b"," + blanks), True)
        self.assertIs(proved(blanks + field + blanks + b","), True)
        # The same line with something after the blanks is not the field alone.
        self.assertIs(proved(field + blanks + b"," + blanks + b'"more": 1,'), False)
        self.assertIs(proved(field + blanks + b"," + blanks + b"x"), False)
        self.assertIs(proved(field + blanks + blanks + b"x"), False)

    def test_a_file_changed_behind_the_index_is_not_the_commits(self):
        # git can be told not to look at a file (two bits of its index); it then reports no change. The
        # tool compares each file it is bound to (itself, its extractor, the class list) byte for byte
        # with what the commit holds, and does not ask the index.
        if shutil.which("git") is None:
            self.skipTest("no git on this host")
        tool = self.checkout([class_entry()])
        checkout = tool.parent.parent

        def git(*args):
            done = subprocess.run(["git", "-C", str(checkout), "-c", "user.name=test", "-c", "user.email=test@example.invalid",
                                   "-c", "commit.gpgsign=false", *args], capture_output=True, text=True)
            self.assertEqual(done.returncode, 0, done.stderr)
            return done.stdout.strip()

        def commit_of_a_scan():
            result = self.scan(self.product({self.MEMBER: self.good}), tool=tool, **self.builds)
            self.assertEqual(self.outcome(result), RESOLVED)
            return result.report["tool_commit"]

        git("init", "-q")
        git("add", "-A")
        git("commit", "-q", "-m", "the tools and the list")
        head = git("rev-parse", "HEAD")
        self.assertEqual(commit_of_a_scan(), head)
        edits = {"release/" + CLASSES_NAME: lambda text: text.replace('"the test"', '"another test"'),
                 "scripts/" + TOOL.name: lambda text: text + "\n# changed behind the index\n",
                 "scripts/" + EXTRACTOR.name: lambda text: text + "\n# changed behind the index\n"}
        for bit in ("assume-unchanged", "skip-worktree"):
            for name, edit in edits.items():
                path = checkout / name
                text = path.read_text()
                git("update-index", "--" + bit, "--", name)
                path.write_text(edit(text))
                self.assertNotEqual(path.read_text(), text, (bit, name))
                self.assertEqual(git("status", "--porcelain"), "", (bit, name))     # git reports no change
                self.assertIsNone(commit_of_a_scan(), (bit, name))
                path.write_text(text)
                self.assertEqual(commit_of_a_scan(), head, (bit, name))
                git("update-index", "--no-" + bit, "--", name)

    def test_a_pending_entry_refuses_and_an_acceptance_counts_before_a_class(self):
        names = ("findings", "accepted_positions", "accepted_in_files", "class_resolved", "refused", "pending_owner")

        def counted(member, **inputs):
            result = self.scan(self.product({self.MEMBER: member}), tool=self.tool, **self.builds, **inputs)
            self.assertIsNotNone(result.line, result.stderr)
            return (result.code, *(int(result.fields.get(name, -1)) for name in names))

        self.assertEqual(counted(self.good), (0, 1, 0, 0, 1, 0, 0))
        bound = position(self.MEMBER, self.good, RECORD_LINE, rule=IDENT_RULE)
        # An acceptance bound to the bytes counts as it always did, and the class counts nothing.
        self.assertEqual(counted(self.good, accepted=self.accepted(positions=[bound])), (0, 1, 1, 0, 0, 0, 0))
        self.assertEqual(counted(self.good, accepted=self.accepted(files=[file_entry(self.MEMBER, self.good)])), (0, 1, 0, 1, 0, 0, 0))
        # An acceptance for other bytes, another line or another rule accepts nothing: the class resolves on its own proof.
        for key, value in (("sha256", "0" * 64), ("line", RECORD_LINE + 1), ("rule", TAG_RULE)):
            self.assertEqual(counted(self.good, accepted=self.accepted(positions=[{**bound, key: value}])), (0, 1, 0, 0, 1, 0, 0), key)
        # A pending entry refuses whatever the class proves, with and without an acceptance.
        waiting = self.pending([{"rule": IDENT_RULE, "path": self.MEMBER, "line": RECORD_LINE, "sha256": sha256(self.good)}])
        self.assertEqual(counted(self.good, pending=waiting), (1, 1, 0, 0, 0, 1, 1))
        self.assertEqual(counted(self.good, pending=waiting, accepted=self.accepted(positions=[bound])), (1, 1, 0, 0, 0, 1, 1))
        # A class takes nothing from an acceptance and adds nothing to it: a value the tool does not prove is
        # accepted by its bytes as before, and an acceptance of one position leaves a second unproved one refused.
        other = record(self.ident.upper())
        self.assertEqual(counted(other), (1, 1, 0, 0, 0, 1, 0))
        self.assertEqual(counted(other, accepted=self.accepted(positions=[position(self.MEMBER, other, RECORD_LINE, rule=IDENT_RULE)])),
                         (0, 1, 1, 0, 0, 0, 0))
        two = record(self.ident, pair(self.ident.upper()) + "\n" + pair(self.ident.upper(), "zq_ident_other"))
        self.assertEqual(counted(two, accepted=self.accepted(positions=[position(self.MEMBER, two, RECORD_LINE, rule=IDENT_RULE)])),
                         (1, 2, 1, 0, 0, 1, 0))

    def test_a_class_list_that_is_not_the_commits_leaves_the_tool_without_a_commit(self):
        if shutil.which("git") is None:
            self.skipTest("no git on this host")
        tool = self.checkout([class_entry()])
        checkout, listed = tool.parent.parent, tool.parent.parent / "release" / CLASSES_NAME

        def git(*args):
            done = subprocess.run(["git", "-C", str(checkout), "-c", "user.name=test", "-c", "user.email=test@example.invalid",
                                   "-c", "commit.gpgsign=false", *args], capture_output=True, text=True)
            self.assertEqual(done.returncode, 0, done.stderr)
            return done.stdout.strip()

        def commit_of_a_scan():
            result = self.scan(self.product({self.MEMBER: self.good}), tool=tool, **self.builds)
            self.assertEqual(self.outcome(result), RESOLVED)
            return result.report["tool_commit"]

        git("init", "-q")
        git("add", "scripts")
        git("commit", "-q", "-m", "the tools alone")
        self.assertIsNone(commit_of_a_scan())           # the list is not tracked
        git("add", "release")
        git("commit", "-q", "-m", "the tools and the list")
        head = git("rev-parse", "HEAD")
        self.assertEqual(commit_of_a_scan(), head)
        text = listed.read_text()
        listed.write_text(text.replace('"the test"', '"another test"'))
        self.assertNotEqual(listed.read_text(), text)
        self.assertIsNone(commit_of_a_scan())           # a changed list is not that commit's list
        listed.write_text(text)
        self.assertEqual(commit_of_a_scan(), head)


class CandidateCredentialScan(Scans):
    """connect-candidate.py credential-scan: the local release-mode step."""

    def test_credential_scan_requires_both_builds_and_refuses_the_marker(self):
        clean = self.product({"notes.txt": b"plain\n"}, name="meerkat-connect-gateway-0.0.0-linux-x86_64.tar.gz")
        marked = self.product({"notes.txt": MARK + b"\n"}, name="meerkat-connect-0.0.0-linux-x86_64.tar.gz")
        done = run(CANDIDATE, "credential-scan", "--gitleaks-current", self.current, "--output", self.root / "one-build", clean)
        self.assertNotEqual(done.returncode, 0)
        self.assertIn("--gitleaks-pinned", done.stderr)
        self.assertFalse((self.root / "one-build").exists())
        builds = ["--gitleaks-pinned", self.pinned, "--gitleaks-current", self.current]
        done = run(CANDIDATE, "credential-scan", *builds, "--output", self.root / "refused", clean, marked)
        self.assertNotEqual(done.returncode, 0)
        self.assertNotIn(MARK_VALUE, done.stdout + done.stderr)
        line = (self.root / "refused/receipt.txt").read_text().splitlines()[0]
        self.assertTrue(line.startswith("CREDENTIAL-LEG REFUSED candidate files=2 %s=%s %s=%s " % (
            clean.name, sha256(clean.read_bytes()), marked.name, sha256(marked.read_bytes()))), line)
        self.assertEqual(json.loads((self.root / "refused/report.json").read_text())["status"], "REFUSED")
        listing = self.root / "refused/unaccepted.json"
        self.assertEqual((len(json.loads(listing.read_text())["positions"]), listing.stat().st_mode & 0o777), (1, 0o600))
        for path in (self.root / "refused").iterdir():
            self.assertNotIn(MARK_VALUE, path.read_text())
        # An acceptance and a pending list go through, and the run can be named.
        notes = MARK + b"\n"
        accepted = self.accepted(positions=[position("notes.txt", notes, 1)])
        done = run(CANDIDATE, "credential-scan", *builds, "--accepted-findings", accepted, "--name", "X0",
                   "--output", self.root / "accepted", clean, marked)
        self.assertEqual(done.returncode, 0, done.stderr)
        line = (self.root / "accepted/receipt.txt").read_text().splitlines()[0]
        self.assertTrue(line.startswith("CREDENTIAL-LEG PASS X0 files=2 "), line)
        self.assertEqual((fields(line)["accepted"], fields(line)["accepted_positions"]), (sha256(accepted.read_bytes()), "1"))
        pending = self.pending([{"rule": RULE, "path": "notes.txt", "line": 1, "sha256": sha256(notes)}])
        done = run(CANDIDATE, "credential-scan", *builds, "--accepted-findings", accepted, "--pending", pending,
                   "--output", self.root / "pending", clean, marked)
        self.assertNotEqual(done.returncode, 0)
        line = (self.root / "pending/receipt.txt").read_text().splitlines()[0]
        self.assertEqual((fields(line)["pending"], fields(line)["pending_owner"]), (sha256(pending.read_bytes()), "1"))
        # A used output directory and a payload for another command are refused.
        done = run(CANDIDATE, "credential-scan", *builds, "--output", self.root / "accepted", clean)
        self.assertNotEqual(done.returncode, 0)
        self.assertNotEqual(run(CANDIDATE, "fetch-built", clean).returncode, 0)


class CandidatePrivacyScanStatus(Scans):
    """connect-candidate.py privacy-scan reads the private-text report's status word."""

    def test_privacy_scan_accepts_only_a_report_that_says_pass(self):
        spec = importlib.util.spec_from_file_location("candidate_scan_status", CANDIDATE)
        candidate = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(candidate)
        payload = self.product({"notes.txt": b"plain\n"})
        rules = self.document("rules", {})
        for status, code, accepted in (("PASS", 0, True), ("clean", 0, False), ("REFUSED", 0, False), ("PASS", 1, False),
                                       ("pass", 0, False)):
            output = self.root / ("privacy-%s-%d" % (status, code))

            def scan(command, **kwargs):
                # The release scan gets the list of members a library leaves out.
                self.assertEqual(command[command.index("--non-published-list") + 1], str(rules))
                Path(command[command.index("--report") + 1]).write_text(json.dumps({"status": status}))
                return types.SimpleNamespace(returncode=code)

            with mock.patch.object(candidate.subprocess, "run", scan):
                inputs = candidate.privacy_inputs(rules, rules, None, rules)
                if accepted:
                    self.assertEqual(candidate.privacy_scan([payload], inputs, output)["status"], "PASS")
                else:
                    with self.assertRaises(ValueError, msg=(status, code)):
                        candidate.privacy_scan([payload], inputs, output)
        # Release mode refuses without the list, after it has asked for the rules.
        with self.assertRaisesRegex(ValueError, "--non-published-list"):
            candidate.privacy_inputs(rules, rules, None, None)
        with self.assertRaisesRegex(ValueError, "--rules"):
            candidate.privacy_inputs(None, rules, None, None)
        done = run(CANDIDATE, "privacy-scan", "--rules", rules, "--exemptions", rules, "--output", self.root / "no-list", payload)
        self.assertNotEqual(done.returncode, 0)
        self.assertIn("--non-published-list", done.stderr)
        self.assertFalse((self.root / "no-list").exists())


@unittest.skipUnless(os.environ.get("CONNECT_GITLEAKS_PINNED") and os.environ.get("CONNECT_GITLEAKS_CURRENT"),
                     "set CONNECT_GITLEAKS_PINNED and CONNECT_GITLEAKS_CURRENT to the two real builds")
class RealBuilds(Scans):
    """The same tool with the two real builds, named by the environment: CONNECT_GITLEAKS_PINNED and
    CONNECT_GITLEAKS_CURRENT (absolute paths), and for a build without gitleaks.toml beside it
    CONNECT_GITLEAKS_PINNED_CONFIG and CONNECT_GITLEAKS_CURRENT_CONFIG (its default rules file)."""

    # A made-up name and value that the builds' default rules report.
    NAME = b"fixture " + b"api" + b"_k" + b"ey ="
    VALUE = "".join(["k3v9", "x1m7", "q5t2", "w8n4", "j6b0", "pd"])

    def real(self, *payload):
        extra = []
        for role in ("pinned", "current"):
            config = os.environ.get("CONNECT_GITLEAKS_%s_CONFIG" % role.upper())
            if config:
                extra += ["--default-config-" + role, config]
        return self.scan(*payload, pinned=os.environ["CONNECT_GITLEAKS_PINNED"], current=os.environ["CONNECT_GITLEAKS_CURRENT"],
                         extra=extra, values=(self.VALUE,))

    def test_the_real_builds_report_a_made_up_pair_and_respect_the_run_rule(self):
        value = self.VALUE.encode()
        result = self.real(self.product({"settings.txt": b"plain line\n" + self.NAME + b" " + value + b"\n"}))
        self.assertEqual(result.code, 1, result.stderr)
        self.assertGreaterEqual(self.counts(result, "refused")[0], 1)
        self.assertEqual({row["path"] for row in result.listing["positions"]}, {"settings.txt"})
        head, tail = b"\x7fELF\x02\x01\x01" + b"\x00" * 57, b" end of the fixture run" + b"\x00" * 16
        result = self.real(self.product({"bin/tool": head + self.NAME + b"\x00\x02" + value + tail}))
        self.assertEqual((result.code, self.counts(result, "findings", "refused")), (0, (0, 0)), result.stderr)
        result = self.real(self.product({"bin/tool": head + self.NAME + b" " + value + tail}))
        self.assertEqual(result.code, 1, result.stderr)
        self.assertEqual({row["path"] for row in result.listing["positions"]}, {"bin/tool#strings"})
        self.assertEqual({row["line"] for row in result.listing["positions"]}, {len(head)})
        result = self.real(self.product({"notes.txt": b"plain\n"}))
        self.assertEqual((result.code, self.counts(result, "findings", "refused", "unread_unaccepted")), (0, (0, 0, 0)), result.stderr)

    def test_a_file_one_real_build_skips_by_its_name_refuses_unless_its_bytes_are_accepted(self):
        # Plain words under a name with an image extension: one build's default path allowlist skips it.
        body = b"made-up words for the unread rule, nothing that looks like a pair\n" * 4
        product = self.product({"art/picture.png": body})
        result = self.real(product)
        self.assertEqual((result.code, self.counts(result, "findings", "refused")), (1, (0, 0)), result.stderr)
        self.assertEqual([(row["path"], row["sha256"]) for row in result.listing["unread"]], [("art/picture.png", sha256(body))])
        result = self.real(self.product({"art/picture.txt": body}))
        self.assertEqual((result.code, self.counts(result, "unread", "unread_unaccepted")), (0, (0, 0)), result.stderr)
        self.count += 1
        accepted = self.accepted(files=[file_entry("art/picture.png", body)])
        extra = ["--accepted-findings", accepted]
        for role in ("pinned", "current"):
            config = os.environ.get("CONNECT_GITLEAKS_%s_CONFIG" % role.upper())
            if config:
                extra += ["--default-config-" + role, config]
        result = self.scan(product, pinned=os.environ["CONNECT_GITLEAKS_PINNED"], current=os.environ["CONNECT_GITLEAKS_CURRENT"],
                           extra=extra, values=(self.VALUE,))
        self.assertEqual((result.code, self.counts(result, "unread", "unread_unaccepted")), (0, (1, 0)), result.stderr)


class ToolCommitGitEnvironment(Scans):
    """The tool commit is found from where the tool is. Git variables in the caller's environment do
    not choose it."""

    def checkout(self, name, repository=True):
        """The tool and its extractor in scripts/ of a directory of their own, committed there or not."""
        directory = self.root / name
        (directory / "scripts").mkdir(parents=True)
        for source in (TOOL, EXTRACTOR):
            shutil.copyfile(source, directory / "scripts" / source.name)
        if repository:
            self.git(directory, "init", "-q")
            self.git(directory, "add", ".")
            self.git(directory, "commit", "-q", "-m", "tools")
        return directory

    def git(self, directory, *args):
        done = subprocess.run(["git", "-C", str(directory), "-c", "user.name=test", "-c", "user.email=test@example.invalid",
                               "-c", "commit.gpgsign=false", *args], capture_output=True, text=True)
        self.assertEqual(done.returncode, 0, done.stderr)
        return done.stdout.strip()

    def commit_of_a_scan(self, directory, **variables):
        self.count += 1
        report = self.root / f"commit-report-{self.count}.json"
        done = run(directory / "scripts" / TOOL.name, "--name", "T", "--gitleaks-pinned", self.pinned,
                   "--gitleaks-current", self.current, "--receipt", self.root / f"commit-receipt-{self.count}.txt",
                   "--report", report, "--work", self.root, self.product({"notes.txt": b"plain\n"}),
                   env={**os.environ, **variables})
        self.assertEqual(done.returncode, 0, done.stderr)
        return json.loads(report.read_text())["tool_commit"]

    def test_git_variables_of_the_caller_do_not_choose_the_tool_commit(self):
        if shutil.which("git") is None:
            self.skipTest("no git on this host")
        committed, loose = self.checkout("committed"), self.checkout("loose", repository=False)
        # Another repository that holds the same two files, at a commit of its own.
        elsewhere = self.checkout("elsewhere")
        self.git(elsewhere, "commit", "-q", "--allow-empty", "-m", "another commit")
        head, other = self.git(committed, "rev-parse", "HEAD"), self.git(elsewhere, "rev-parse", "HEAD")
        self.assertNotEqual(head, other)
        variables = {"GIT_DIR": str(elsewhere / ".git"), "GIT_WORK_TREE": str(loose)}
        # A tool in no repository stays without a commit: the other repository's is not taken.
        self.assertIsNone(self.commit_of_a_scan(loose, **variables))
        # A committed tool names its own checkout's commit.
        self.assertEqual(self.commit_of_a_scan(committed, **variables), head)
        self.assertEqual(self.commit_of_a_scan(committed, GIT_DIR=str(self.root / "no-such-repository")), head)
        self.assertEqual(self.commit_of_a_scan(committed), head)

    def test_a_replace_ref_does_not_make_an_edited_tool_the_commits(self):
        # git can be told to read another object in place of one (a replace ref). The scan asks for the
        # commit's own objects, so a tool edited after the commit is not that commit's tool.
        if shutil.which("git") is None:
            self.skipTest("no git on this host")
        checkout = self.checkout("replaced")
        head = self.git(checkout, "rev-parse", "HEAD")
        self.assertEqual(self.commit_of_a_scan(checkout), head)
        with open(checkout / "scripts" / TOOL.name, "a") as stream:
            stream.write("\n# edited after the commit\n")
        self.git(checkout, "add", "-A")
        other = self.git(checkout, "commit-tree", self.git(checkout, "write-tree"), "-m", "the edited tool")
        self.git(checkout, "replace", head, other)
        # Asked without more, git now reads the other commit as the checkout's: nothing looks changed.
        self.assertEqual((self.git(checkout, "rev-parse", "HEAD"), self.git(checkout, "status", "--porcelain")), (head, ""))
        self.assertIsNone(self.commit_of_a_scan(checkout))


# The directory, inside an opened archive, of the bytes of that archive that are no member's content.
OUTSIDE = "!outside/"


def zip_plain(members, comment=b"", between=b"", header=b""):
    """A zip without compression. between is written after the first member's record, where no member's
    content lies and no directory entry points; comment is the archive's comment; header stands in an
    extra field of the first member's local header, and of that header alone."""
    raw = io.BytesIO()
    with zipfile.ZipFile(raw, "w", zipfile.ZIP_STORED) as archive:
        for number, (name, data) in enumerate(members.items()):
            info = zipfile.ZipInfo(name, (1980, 1, 1, 0, 0, 0))
            if number == 0 and header:
                info.extra = (0x7a71).to_bytes(2, "little") + len(header).to_bytes(2, "little") + header
            archive.writestr(info, data)
            if number == 0 and between:
                archive.fp.write(between)
                archive.start_dir = archive.fp.tell()
        archive.comment = comment
    packed = raw.getvalue()
    if header:      # the directory entry repeats the extra field: there it is blanked
        at = packed.rindex(header)
        packed = packed[:at] + b"\x00" * len(header) + packed[at + len(header):]
    return packed


def tar_with_padding(name, data, padding):
    """A tar of one member whose last block is filled up with padding and not with zero bytes."""
    raw = bytearray(tar_bytes({name: data}, compressed=False))
    with tarfile.open(fileobj=io.BytesIO(bytes(raw))) as archive:
        at = archive.getmember(name).offset_data + len(data)
    raw[at:at + len(padding)] = padding
    return bytes(raw)


def gzip_with_comment(data, comment):
    """A gzip stream whose header holds a comment field."""
    packed = bytearray(gzip.compress(data, mtime=0))
    packed[3] |= 16
    return bytes(packed[:10]) + comment + b"\x00" + bytes(packed[10:])


def zstd_skippable(data):
    """A zstd skippable frame: a decoder passes over it and gives nothing."""
    return (0x184D2A50).to_bytes(4, "little") + len(data).to_bytes(4, "little") + data


class BytesOutsideMembers(Scans):
    """An archive reader opens an archive that holds more than its members: bytes before the first
    member, between two members, in a header or a comment, and after the end. Both builds read those
    too, at every level: what stands before or after the archive as a file of its own, the rest by
    its printable runs. The paths name the archive and the place."""

    def refused_at(self, result):
        """(rule, path) of every refused position of a refused scan."""
        self.assertEqual(result.code, 1, result.stderr)
        return sorted({(row["rule"], row["path"]) for row in result.listing["positions"]})

    def passes(self, result, note=None):
        self.assertEqual((result.code, self.counts(result, "findings", "refused", "unread")), (0, (0, 0, 0)), note)

    def test_bytes_after_the_end_of_a_zip_are_scanned(self):
        tail = b"\n" + MARK + b"\n"
        clean = zip_bytes({"pkg/notes.txt": b"plain\n"})
        archive = clean + tail
        with zipfile.ZipFile(io.BytesIO(archive)) as reader:    # the reader takes it and gives the member only
            self.assertEqual(reader.namelist(), ["pkg/notes.txt"])
        # The payload file itself.
        result = self.scan(self.payload(archive, name="product-0.0.0.zip"))
        self.assertEqual(self.refused_at(result), [(RULE, OUTSIDE + "after-end")])
        self.assertEqual(self.counts(result, "findings", "refused"), (1, 1))
        self.assertIn("| p0 | %s | %safter-end | 2 | 1 | no |" % (RULE, OUTSIDE), result.receipt)
        self.assertEqual(result.listing["positions"][0]["sha256"], sha256(tail))
        # One and two levels down.
        result = self.scan(self.product({"lib/inner.zip": archive}))
        self.assertEqual(self.refused_at(result), [(RULE, "lib/inner.zip!" + OUTSIDE + "after-end")])
        result = self.scan(self.product({"lib/outer.tar.gz": tar_bytes({"deep/inner.zip": archive})}))
        self.assertEqual(self.refused_at(result), [(RULE, "lib/outer.tar.gz!deep/inner.zip!" + OUTSIDE + "after-end")])
        # The archive without those bytes passes.
        self.passes(self.scan(self.payload(clean, name="product-0.0.0.zip")))
        self.passes(self.scan(self.product({"lib/inner.zip": clean})))

    def test_bytes_before_a_zip_between_its_members_and_in_its_comment_are_scanned(self):
        members = {"pkg/notes.txt": b"plain\n", "pkg/other.txt": b"plain too\n"}
        cases = (("before-start", b"#!/bin/sh\n" + MARK + b"\n" + zip_plain(members)),
                 ("headers-and-gaps", zip_plain(members, between=b"\x00" + MARK + b"\x00")),
                 ("headers-and-gaps", zip_plain(members, header=MARK)),
                 ("headers-and-gaps", zip_plain(members, comment=b"a comment, " + MARK)))
        for number, (place, archive) in enumerate(cases):
            with zipfile.ZipFile(io.BytesIO(archive)) as reader:
                self.assertEqual(reader.namelist(), list(members))
            result = self.scan(self.payload(archive, name="product-0.0.0.zip"))
            self.assertEqual(self.refused_at(result), [(RULE, OUTSIDE + place)], number)
            result = self.scan(self.product({"lib/inner.zip": archive}))
            # One level down the archive file stays in the tree. The one that starts with text is read
            # as a file too, so the bytes before its first record are reported there as well.
            also = [(RULE, "lib/inner.zip")] if place == "before-start" else []
            self.assertEqual(self.refused_at(result), also + [(RULE, "lib/inner.zip!" + OUTSIDE + place)], number)
        self.passes(self.scan(self.payload(zip_plain(members, between=b"\x00" * 40, comment=b"a comment"), name="product-0.0.0.zip")))

    def test_bytes_after_the_end_of_a_tar_and_in_its_padding_are_scanned(self):
        plain = tar_bytes({"pkg/notes.txt": b"plain\n"}, compressed=False)
        cases = (("after-end", plain + MARK + b"\n"), ("headers-and-gaps", tar_with_padding("pkg/notes.txt", b"plain\n", MARK)))
        for place, archive in cases:
            with tarfile.open(fileobj=io.BytesIO(archive)) as reader:
                self.assertEqual(reader.getnames(), ["pkg/notes.txt"])
            result = self.scan(self.payload(archive, name="product-0.0.0.tar"))
            self.assertEqual(self.refused_at(result), [(RULE, OUTSIDE + place)], place)
            result = self.scan(self.product({"lib/inner.tar": archive}))
            self.assertEqual(self.refused_at(result), [(RULE, "lib/inner.tar!" + OUTSIDE + place)], place)
            # Inside a compressed stream they are named as the tar's.
            for pack in (lambda data: gzip.compress(data, mtime=0), bz2.compress, lzma.compress):
                result = self.scan(self.payload(pack(archive)))
                self.assertEqual(self.refused_at(result), [(RULE, OUTSIDE + "tar-" + place)], place)

    def test_bytes_after_the_last_stream_of_a_compressed_file_refuse(self):
        # A compressed file is its streams and nothing else. Bytes after the last stream that are not
        # zero bytes and start no further stream make it a file that is not opened: such bytes can hold
        # a stream after any other byte, where no reader would look for it.
        notes = b"plain\n"
        pack = tar_bytes({"deep/notes.txt": notes}, compressed=False)
        packed = {"notes.txt.gz": gzip.compress(notes, mtime=0), "notes.txt.bz2": bz2.compress(notes), "notes.txt.xz": lzma.compress(notes),
                  "pack.tar.gz": gzip.compress(pack, mtime=0), "pack.tar.bz2": bz2.compress(pack), "pack.tar.xz": lzma.compress(pack)}
        if zstd_compress(notes) is not None:
            packed["notes.txt.zst"] = zstd_compress(notes)
        tails = (MARK + b"\n", b"x" + gzip.compress(MARK + b"\n", mtime=0), b"\x00" * 8 + b"x" + gzip.compress(MARK + b"\n", mtime=0))
        for name, data in packed.items():
            for number, tail in enumerate(tails):
                result = self.scan(self.product({"data/" + name: data + tail}))
                self.assertEqual((result.code, self.counts(result, "findings", "refused", "unread")), (1, (0, 1, 1)), (name, number))
                self.assertEqual([row["path"] for row in result.listing["unopened"]], ["data/" + name], (name, number))
            # Zero bytes after a stream are padding, and the file without them is read as before.
            self.passes(self.scan(self.product({"data/" + name: data + b"\x00" * 8})), name)
            self.passes(self.scan(self.product({"data/" + name: data})), name)
        # A second stream after the padding is read: its content is the member's.
        result = self.scan(self.product({"data/notes.txt.gz": packed["notes.txt.gz"] + b"\x00" * 8 + gzip.compress(MARK + b"\n", mtime=0)}))
        self.assertEqual(self.refused_at(result), [(RULE, "data/notes.txt.gz!notes.txt")])
        for name, pack_again in (("notes.txt.bz2", bz2.compress), ("notes.txt.xz", lzma.compress)):
            result = self.scan(self.product({"data/" + name: packed[name] + pack_again(MARK + b"\n")}))
            self.assertEqual(self.refused_at(result), [(RULE, "data/%s!notes.txt" % name)], name)
        # A gzip member whose check is not its content's is no stream that was read: the file is not opened.
        broken = packed["notes.txt.gz"][:-8] + b"\x00" * 4 + packed["notes.txt.gz"][-4:]
        result = self.scan(self.product({"data/notes.txt.gz": broken}))
        self.assertEqual((result.code, self.counts(result, "findings", "refused", "unread")), (1, (0, 1, 1)))
        self.assertEqual([row["path"] for row in result.listing["unopened"]], ["data/notes.txt.gz"])

    def test_zero_bytes_between_two_zstd_frames_are_padding_with_either_decoder(self):
        # The same file gives the same result with Python's own decoder (3.14 and later) and with the
        # zstd executable (which reads no zero bytes between frames: it is given the frames alone).
        first, second = zstd_compress(b"first line\n"), zstd_compress(MARK + b"\n")
        if first is None:
            self.skipTest("no zstd encoder on this host")
        for number, packed in enumerate((first + b"\x00" * 8 + second, first + b"\x00" * 3 + zstd_skippable(b"made-up") + b"\x00" + second + b"\x00" * 5)):
            result = self.scan(self.product({"data/notes.txt.zst": packed}))
            self.assertEqual((result.code, [(row["rule"], row["path"], row["line"]) for row in result.listing["positions"]]),
                             (1, [(RULE, "data/notes.txt.zst!notes.txt", 2)]), number)

    def test_a_tab_does_not_split_a_run(self):
        # A tab is text to the builds: a name, a tab, the sign and the value are one run.
        tabbed = MARK_NAME.encode() + b"\t=\t" + MARK_VALUE.encode()
        members = {"pkg/notes.txt": b"plain\n"}
        result = self.scan(self.product({"notes.txt": tabbed + b"\n"}))
        self.assertEqual(self.refused_at(result), [(RULE, "notes.txt")])
        result = self.scan(self.payload(zip_plain(members, comment=b"a comment, " + tabbed), name="product-0.0.0.zip"))
        self.assertEqual(self.refused_at(result), [(RULE, OUTSIDE + "headers-and-gaps")])
        result = self.scan(self.product({"data/notes.txt.gz": gzip_with_comment(b"plain\n", b"a comment, " + tabbed)}))
        self.assertEqual(self.refused_at(result), [(RULE, "data/notes.txt.gz!" + OUTSIDE + "headers-and-gaps")])

    def test_a_gzip_comment_and_a_zstd_skippable_frame_are_scanned(self):
        notes = b"plain\n"
        pack = tar_bytes({"deep/notes.txt": notes}, compressed=False)
        for name, data in (("notes.txt.gz", notes), ("pack.tar.gz", pack)):
            commented = gzip_with_comment(data, b"a comment, " + MARK)
            self.assertEqual(gzip.decompress(commented), data)
            result = self.scan(self.product({"data/" + name: commented}))
            self.assertEqual(self.refused_at(result), [(RULE, "data/%s!%sheaders-and-gaps" % (name, OUTSIDE))], name)
        frame = zstd_compress(notes)
        if frame is None:
            self.skipTest("no zstd encoder on this host")
        for number, data in enumerate((frame + zstd_skippable(MARK), frame + zstd_skippable(MARK) + frame)):
            result = self.scan(self.product({"data/notes.txt.zst": data}))
            self.assertEqual(self.refused_at(result), [(RULE, "data/notes.txt.zst!" + OUTSIDE + "headers-and-gaps")], number)
        self.passes(self.scan(self.product({"data/notes.txt.zst": frame + zstd_skippable(b"nothing to find here") + frame})))

    def test_an_archive_before_or_after_an_archive_is_opened(self):
        hidden = {"deep/notes.txt": MARK + b"\n"}
        clean = zip_bytes({"mod/a.py": b"x = 1\n"})
        plain = tar_bytes({"mod/a.py": b"x = 1\n"}, compressed=False)
        cases = (("lib/inner.zip", clean + tar_bytes(hidden), "after-end"),
                 ("lib/inner.zip", zip_bytes(hidden) + clean, "before-start"),
                 ("lib/inner.tar", plain + tar_bytes(hidden, compressed=False), "after-end"),
                 ("lib/inner.tar.gz", gzip.compress(plain + tar_bytes(hidden, compressed=False), mtime=0), "tar-after-end"))
        for name, archive, place in cases:
            result = self.scan(self.product({name: archive}))
            self.assertEqual(self.refused_at(result), [(RULE, "%s!%s%s!deep/notes.txt" % (name, OUTSIDE, place))], (name, place))
            self.assertEqual(result.listing["positions"][0]["sha256"], sha256(MARK + b"\n"))

    def test_zero_bytes_outside_members_make_no_scan_file(self):
        plain = tar_bytes({"pkg/notes.txt": b"plain\n"}, compressed=False)
        for archive in (plain, plain + b"\x00" * 4096):
            result = self.scan(self.payload(archive, name="product-0.0.0.tar"))
            self.passes(result)
            # The member, and the printable runs of the tar's headers: no file for the padding or the end blocks.
            self.assertEqual(result.report["payload"][0]["files"], 2)
        # Bytes without a printable run of eight make no file either.
        result = self.scan(self.payload(gzip.compress(b"plain\n", mtime=0), name="notes-0.0.0.txt.gz"))
        self.passes(result)
        self.assertEqual(result.report["payload"][0]["files"], 1)

    def test_bytes_outside_members_with_a_binary_signature_are_read_by_their_runs(self):
        # The builds do not read a file that starts with a binary signature. A member of that kind is
        # unread and refuses; bytes outside members are no member with a proof of its own, so they are
        # read like a compiled file, by their printable runs, and are never unread.
        tail = b"%PDF-1.4\n" + MARK + b"\n"
        product = self.product({"lib/inner.zip": zip_bytes({"mod/a.py": b"x = 1\n"}) + tail})
        path = "lib/inner.zip!" + OUTSIDE + "after-end"
        result = self.scan(product)
        self.assertEqual((result.code, self.counts(result, "findings", "refused", "unread", "unread_unaccepted")), (1, (1, 1, 0, 0)))
        self.assertEqual([(row["rule"], row["path"], row["line"], row["sha256"]) for row in result.listing["positions"]],
                         [(RULE, path + "#strings", tail.index(MARK), sha256(tail))])
        self.assertEqual(result.listing["unread"], [])
        for lists in ({"files": [file_entry(path, tail)]}, {"positions": [position(path + "#strings", tail, tail.index(MARK))]}):
            result = self.scan(product, accepted=self.accepted(**lists))
            self.assertEqual((result.code, self.counts(result, "findings", "refused", "unread")), (0, (1, 0, 0)), list(lists))
        # The marker in the run that starts with the signature: the run itself starts like a file no build reads.
        tail = b"%PDF-1.4 " + MARK + b"\n"
        result = self.scan(self.product({"lib/inner.zip": zip_bytes({"mod/a.py": b"x = 1\n"}) + tail}))
        self.assertEqual([(row["rule"], row["path"], row["line"]) for row in result.listing["positions"]], [(RULE, path + "#strings", 0)])
        # A compiled file before a zip is read the same way.
        stub = b"\x7fELF\x02\x01\x01" + b"\x00" * 57 + b"made-up run " + MARK + b" end of the run" + b"\x00" * 16
        result = self.scan(self.product({"lib/inner.zip": stub + zip_bytes({"mod/a.py": b"x = 1\n"})}))
        self.assertEqual([(row["rule"], row["path"], row["line"]) for row in result.listing["positions"]],
                         [(RULE, "lib/inner.zip!" + OUTSIDE + "before-start#strings", 64)])
        self.assertEqual(result.listing["unread"], [])

    def test_the_runs_of_bytes_with_a_binary_signature_are_the_runs_they_are(self):
        # The line of plain words before the runs of such bytes, and the line break before a run that is
        # read alone, change no run: each is found at its own place, two runs are not read as one, and
        # none is lost.
        name, value = MARK_NAME.encode() + b" =", MARK_VALUE.encode()
        tail = (b"%PDF-1.4 " + MARK + b"\x00\x01"                              # the run that starts with the signature
                + b"second run " + MARK + b" end of it\x00\x02"                # a run in the middle
                + b"third run " + name + b"\x00\x02" + value + b" the value starts a run of its own\x00\x03"
                + b"last run " + MARK)                                          # the run that ends the file
        product = self.product({"lib/inner.zip": zip_bytes({"mod/a.py": b"x = 1\n"}) + tail})
        path = "lib/inner.zip!" + OUTSIDE + "after-end#strings"
        result = self.scan(product)
        self.assertEqual([(row["rule"], row["path"], row["line"], row["count"], row["sha256"]) for row in result.listing["positions"]],
                         [(RULE, path, at, 1, sha256(tail)) for at in (0, tail.index(b"second run"), tail.index(b"last run"))])
        # Three findings and no more: the name at the end of one run and the value at the start of the
        # next are none, in either build.
        self.assertEqual(self.counts(result, "findings", "refused", "unread"), (3, 3, 0))

    def test_an_acceptance_covers_bytes_outside_members_with_their_archive_or_by_their_bytes(self):
        tail = b"\n" + MARK + b"\n"
        wheel = zip_bytes({"mod/a.py": b"x = 1\n"}) + tail
        path = "lib/wheel-1.0.whl!" + OUTSIDE + "after-end"
        entries = ({"archives": [archive_entry("lib/wheel-1.0.whl", wheel)]}, {"positions": [position(path, tail, 2)]})
        product = self.product({"lib/wheel-1.0.whl": wheel})
        self.assertEqual(self.refused_at(self.scan(product)), [(RULE, path)])
        for lists, word in zip(entries, ("accepted_in_archives", "accepted_positions")):
            result = self.scan(product, accepted=self.accepted(**lists))
            self.assertEqual((result.code, self.counts(result, "findings", word, "refused")), (0, (1, 1, 0)), word)
            # Other bytes after the end are another archive and other bytes at that path.
            other = self.product({"lib/wheel-1.0.whl": wheel + MARK + b"\n"})
            result = self.scan(other, accepted=self.accepted(**lists))
            self.assertEqual((result.code, self.counts(result, word)), (1, (0,)), word)


class BytesOutsideMembersUnderAnyPath(Scans):
    """The bytes outside an archive's members are no member: no proof says where they come from. Both
    builds read them whatever path the archive lies at, also a path that a build's allowlist skips."""

    MORE = DEFAULT_RULES.replace("    '''\\.zqskip$''',\n", "    '''\\.zqskip$''',\n    '''(^|/)zqthird/''',\n")

    def builds(self, name, *roles):
        """The two builds; those named skip every path below a directory zqthird."""
        self.assertNotEqual(self.MORE, DEFAULT_RULES)
        return {"pinned": write_build(self.root / (name + "-pinned"), "0.0.5", False, RULES[:1], self.MORE if "pinned" in roles else DEFAULT_RULES),
                "current": write_build(self.root / (name + "-current"), "0.0.6", True, RULES, self.MORE if "current" in roles else DEFAULT_RULES)}

    def test_bytes_outside_members_are_read_below_a_path_that_a_build_skips(self):
        members = {"pkg/notes.txt": b"plain\n"}
        plain = tar_bytes(members, compressed=False)
        cases = (("inner.zip", zip_plain(members) + b"\n" + MARK + b"\n", "after-end"),
                 ("inner.zip", zip_plain(members, comment=b"a comment, " + MARK), "headers-and-gaps"),
                 ("inner.zip", b"#!/bin/sh\n" + MARK + b"\n" + zip_plain(members), "before-start"),
                 ("inner.tar", tar_with_padding("pkg/notes.txt", b"plain\n", MARK), "headers-and-gaps"),
                 ("inner.tar", plain + MARK + b"\n", "after-end"),
                 ("inner.tar.gz", gzip.compress(tar_with_padding("pkg/notes.txt", b"plain\n", MARK), mtime=0), "tar-headers-and-gaps"))
        for number, roles in enumerate((("pinned", "current"), ("current",))):
            builds = self.builds("skip-%d" % number, *roles)
            for name, archive, place in cases:
                result = self.scan(self.product({"zqthird/" + name: archive}), **builds)
                path = "zqthird/%s!%s%s" % (name, OUTSIDE, place)
                self.assertEqual(result.code, 1, (roles, name, place))
                self.assertIn((RULE, path, 1), [(row["rule"], row["path"], row["count"]) for row in result.listing["positions"]], (roles, name, place))
                # What is unread there is the archive's member (and an archive file that starts with
                # text), never its bytes outside members.
                self.assertEqual([row["path"] for row in result.listing["unread"] if OUTSIDE in row["path"]], [], (roles, name, place))

    def test_a_clean_archive_below_such_a_path_has_no_unread_file_of_its_own(self):
        members = {"pkg/notes.txt": b"plain words\n", "pkg/other.txt": b"more plain words\n"}
        builds = self.builds("clean", "pinned", "current")
        for name, archive in (("inner.zip", zip_plain(members, comment=b"a plain comment of the archive") + b"plain words after the end\n"),
                              ("inner.tar", tar_bytes(members, compressed=False) + b"plain words after the end\n")):
            product = self.product({"zqthird/" + name: archive})
            result = self.scan(product, **builds)
            self.assertEqual((result.code, self.counts(result, "findings", "unread", "unread_unaccepted")), (1, (0, 2, 2)), name)
            self.assertEqual([row["path"] for row in result.listing["unread"]],
                             ["zqthird/%s!pkg/notes.txt" % name, "zqthird/%s!pkg/other.txt" % name], name)
            # With its members accepted nothing of the archive is left unread.
            entries = [file_entry("zqthird/%s!%s" % (name, member), data) for member, data in members.items()]
            result = self.scan(product, accepted=self.accepted(files=entries), **builds)
            self.assertEqual((result.code, self.counts(result, "findings", "refused", "unread", "unread_unaccepted")), (0, (0, 0, 2, 0)), name)


class NumberedFilesOfTheScan(Scans):
    """The scan gives the builds some bytes as files of its own, under numbered names: the bytes outside
    members, the runs of a compiled file, a run that is read alone. A build's path allowlist that
    skips such a name would leave them read by no build, and nothing would say so: the scan stops."""

    ELF = b"\x7fELF\x02\x01\x01" + b"\x00" * 57

    def builds(self, name, pattern):
        """The two builds, with one more pattern on the path allowlist of each."""
        more = DEFAULT_RULES.replace("    '''\\.zqskip$''',\n", "    '''\\.zqskip$''',\n    '''%s''',\n" % pattern)
        self.assertNotEqual(more, DEFAULT_RULES)
        return {"pinned": write_build(self.root / (name + "-pinned"), "0.0.9", False, RULES[:1], more),
                "current": write_build(self.root / (name + "-current"), "0.0.10", True, RULES, more)}

    def test_a_path_allowlist_that_skips_a_numbered_file_stops_the_scan(self):
        run = b"made-up run " + MARK + b" end of the run"
        text = b"#!/bin/sh\n" + MARK + b"\n"
        archive = text + zip_plain({"pkg/notes.txt": b"plain\n"}) + text
        # The archive file starts with text and is read as a file too: its two markers are found there as well.
        cases = (("bytes outside members", r"/t/00001\.txt$", {"lib/inner.zip": archive}, 4),
                 ("runs of a compiled file", r"/t/00001\.txt$", {"bin/first": self.ELF + run + b"\x00" * 16, "bin/second": self.ELF + run + b"\x00" * 24}, 2),
                 ("a run that is read alone", r"/t/00000-\d{8}\.txt$", {"bin/tool": self.ELF + run + b"\x00" * 16}, 1))
        for number, (note, pattern, files, markers) in enumerate(cases):
            product = self.product(files)
            # With the builds' own allowlist every marker is found.
            result = self.scan(product)
            self.assertEqual((result.code, sum(row["count"] for row in result.listing["positions"])), (1, markers), note)
            result = self.scan(product, **self.builds("numbered-%d" % number, pattern))
            self.assertEqual((result.code, result.receipt), (2, None), note)
            self.assertIn("path-allowlist pattern", result.stderr, note)
        # A pattern that matches no file of the scan's own stops nothing.
        result = self.scan(self.product({"bin/tool": self.ELF + run + b"\x00" * 16}), **self.builds("numbered-none", r"/t/00007\.txt$"))
        self.assertEqual((result.code, [(row["rule"], row["path"]) for row in result.listing["positions"]]), (1, [(RULE, "bin/tool#strings")]))


class ProofOfRead(Scans):
    """No list of the scan decides whether a build read a file. While a build runs, every file handed to
    it ends with a line of the scan's own that the build reports. A file whose line a build does not
    report is read again behind a line of plain words, and is unread by that build when its line is
    missing there too."""

    ELF = b"\x7fELF\x02\x01\x01" + b"\x00" * 57
    KIND = b"{\\rtf1 "      # how a kind of file starts that the scan's list does not hold
    NEVER = b"zq-never-read"

    def builds(self, name, pinned=None, **more):
        """The two builds; more is how both differ from the usual ones, pinned how the pinned one alone does."""
        return {"pinned": adapted(write_build(self.root / (name + "-pinned"), "0.0.13", False, RULES[:1]), **(more if pinned is None else pinned)),
                "current": adapted(write_build(self.root / (name + "-current"), "0.0.14", True, RULES), **more)}

    def positions(self, result):
        return [(row["rule"], row["path"], row["line"], row["count"]) for row in result.listing["positions"]]

    def test_a_kind_of_file_outside_the_list_is_read_behind_the_line(self):
        # Both builds read no file that starts like the kind. The scan does not know the kind.
        builds = self.builds("kind", more_binary=[(0, self.KIND[:5])])
        text = self.KIND + b"made-up words\n" + MARK + b"\nlast words, and no line break"
        words, later = self.KIND + b"made-up words", b"made-up run " + MARK + b" end of the run"
        first = self.ELF + words + b"\x00\x02" + later + b"\x00" * 16
        alone = self.ELF + b"plain made-up words" + b"\x00\x02" + self.KIND + MARK + b"\x00" * 16
        after = self.KIND + b"made-up words\n" + MARK + b"\n"
        cases = (("a text member", {"doc/notes.rtf": text}, (RULE, "doc/notes.rtf", 2, 1), text),
                 ("the file of a compiled file's runs", {"bin/tool": first}, (RULE, "bin/tool#strings", len(self.ELF) + len(words) + 2, 1), first),
                 ("a run that is read alone", {"bin/tool": alone}, (RULE, "bin/tool#strings", len(self.ELF) + 19 + 2, 1), alone),
                 ("text after the end of an archive", {"lib/inner.zip": zip_plain({"pkg/notes.txt": b"plain\n"}) + after},
                  (RULE, "lib/inner.zip!" + OUTSIDE + "after-end", 2, 1), after))
        for note, files, found, holder in cases:
            result = self.scan(self.product(files), **builds)
            self.assertEqual((result.code, self.positions(result)), (1, [found]), note)
            # The file is the member's own bytes again: the scan's line is cut off after the builds ran.
            self.assertEqual([row["sha256"] for row in result.listing["positions"]], [sha256(holder)], note)
            self.assertEqual((self.counts(result, "unread", "unread_unaccepted"), result.report["method_artifacts"]), ((0, 0), 0), note)
            for label in ("0.0.13", "0.0.14"):
                self.assertGreaterEqual(result.report["read_proof"][label]["read_behind_the_line"], 1, note)
                self.assertEqual(result.report["read_proof"][label]["unread"], 0, note)
        # The usual builds read the same files as they are: nothing goes behind the line.
        result = self.scan(self.product({"doc/notes.rtf": text}))
        self.assertEqual((result.code, self.positions(result)), (1, [(RULE, "doc/notes.rtf", 2, 1)]))
        self.assertEqual([proof["read_behind_the_line"] for proof in result.report["read_proof"].values()], [0, 0])

    def test_a_file_that_a_build_reads_nowhere_is_unread(self):
        body = b"made-up words, " + self.NEVER + b"\n" + MARK + b"\n"
        product = self.product({"doc/notes.txt": body, "doc/other.txt": b"plain\n"})
        # Both builds read no file that holds the word, wherever it stands and whatever stands before.
        result = self.scan(product, **self.builds("never", skip_word=self.NEVER))
        self.assertEqual((result.code, self.positions(result)), (1, []))
        self.assertEqual([(row["path"], row["sha256"], row["reason"]) for row in result.listing["unread"]],
                         [("doc/notes.txt", sha256(body), "no proof that a build read it (both builds)")])
        self.assertEqual(self.counts(result, "unread", "unread_unaccepted"), (1, 1))
        proofs = result.report["read_proof"]
        self.assertEqual(sorted(proofs), ["0.0.13", "0.0.14"])
        for proof in proofs.values():
            self.assertEqual((proof["read_behind_the_line"], proof["unread"]), (0, 1))
            self.assertEqual(proof["handed"], proof["read_as_it_is"] + 1)
        self.assertIn("Proof of read", result.receipt)
        # One build alone: the other build reads the file and reports its marker; the file still refuses.
        result = self.scan(product, **self.builds("never-one", pinned={"skip_word": self.NEVER}))
        self.assertEqual((result.code, self.positions(result)), (1, [(RULE, "doc/notes.txt", 2, 1)]))
        self.assertEqual([(row["path"], row["reason"]) for row in result.listing["unread"]],
                         [("doc/notes.txt", "no proof that a build read it (one build; the other build read it)")])
        self.assertEqual([proofs["unread"] for _label, proofs in sorted(result.report["read_proof"].items())], [1, 0])
        # An acceptance of the position does not cover the file: it is still unread by a build, and refuses.
        result = self.scan(product, accepted=self.accepted(positions=[position("doc/notes.txt", body, 2)]),
                           **self.builds("never-position", pinned={"skip_word": self.NEVER}))
        self.assertEqual((result.code, self.counts(result, "accepted_positions", "refused", "unread", "unread_unaccepted")), (1, (1, 0, 1, 1)))
        # Only the file's exact bytes, accepted, let it pass (the unread rule as it is). The receipt's
        # first line shows both counts either way: the unread files, and those without an acceptance.
        clean = b"made-up words, " + self.NEVER + b"\n"
        product = self.product({"doc/notes.txt": clean})
        builds = self.builds("never-accepted", skip_word=self.NEVER)
        result = self.scan(product, **builds)
        self.assertEqual((result.code, self.counts(result, "unread", "unread_unaccepted")), (1, (1, 1)))
        result = self.scan(product, accepted=self.accepted(files=[file_entry("doc/notes.txt", clean)]), **builds)
        self.assertEqual((result.code, self.counts(result, "unread", "unread_unaccepted")), (0, (1, 0)))
        self.assertEqual([proof["unread"] for proof in result.report["read_proof"].values()], [1, 1])

    def test_every_kind_of_handed_file_without_proof_is_listed_unread(self):
        # A word that makes both builds pass over a file, in a run of a compiled file (so in the file
        # of its runs) and in text after the end of an archive; and builds that read no run that
        # stands alone. Each is listed at the member it comes from, and refuses.
        later = b"made-up run " + MARK + b" end of the run"
        tool = self.ELF + b"made-up run with " + self.NEVER + b"\x00\x02" + later + b"\x00" * 16
        after = b"made-up words, " + self.NEVER + b"\n" + MARK + b"\n"
        marked = self.ELF + later + b"\x00" * 16
        cases = (("a file of runs", {"bin/tool": tool}, {"skip_word": self.NEVER}, "bin/tool", tool),
                 ("text after the end of an archive", {"lib/inner.zip": zip_plain({"pkg/notes.txt": b"plain\n"}) + after},
                  {"skip_word": self.NEVER}, "lib/inner.zip!" + OUTSIDE + "after-end", after),
                 ("a run that is read alone", {"bin/tool": marked}, {"skip_path": "strings-isolated"}, "bin/tool", marked))
        for number, (note, files, switches, path, holder) in enumerate(cases):
            result = self.scan(self.product(files), **self.builds("listed-%d" % number, **switches))
            self.assertEqual((result.code, self.positions(result)), (1, []), note)
            self.assertEqual([(row["path"], row["sha256"], row["reason"]) for row in result.listing["unread"]],
                             [(path, sha256(holder), "no proof that a build read it (both builds)")], note)

    def test_a_proof_line_that_is_doubled_moved_or_missing_proves_nothing(self):
        body = b"plain words\nmore plain words\n"
        for mode in ("twice", "below", "gone"):
            result = self.scan(self.product({"doc/notes.txt": body}), **self.builds("mode-" + mode, proof_mode=mode))
            self.assertEqual(result.code, 1, mode)
            self.assertIn(("doc/notes.txt", sha256(body), "no proof that a build read it (both builds)"),
                          [(row["path"], row["sha256"], row["reason"]) for row in result.listing["unread"]], mode)
            self.assertGreaterEqual(min(proof["unread"] for proof in result.report["read_proof"].values()), 1, mode)
        # A file with a finding and without its proof line is read again too: the finding stands once.
        marked = b"plain words\n" + MARK + b"\nmore plain words\n"
        result = self.scan(self.product({"doc/notes.txt": marked}), **self.builds("mode-gone-marked", proof_mode="gone"))
        self.assertEqual((result.code, [row for row in self.positions(result) if row[0] == RULE]), (1, [(RULE, "doc/notes.txt", 2, 1)]))
        self.assertIn("doc/notes.txt", [row["path"] for row in result.listing["unread"]])
        # A build that states the line's end one line later still proves: the line is the file's last,
        # so a finding that starts on it lies in the scan's own line whatever end the build states.
        result = self.scan(self.product({"doc/notes.txt": marked}), **self.builds("mode-long", proof_mode="long"))
        self.assertEqual((result.code, self.positions(result), self.counts(result, "unread")), (1, [(RULE, "doc/notes.txt", 2, 1)], (0,)))
        self.assertEqual([proof["read_behind_the_line"] for proof in result.report["read_proof"].values()], [0, 0])

    def test_a_member_that_ends_with_the_proof_line_keeps_its_own_finding(self):
        for ending in (b"\n", b""):
            body = b"plain words\n" + PROOF_LINE[:-1] + ending
            result = self.scan(self.product({"doc/notes.txt": body}))
            self.assertEqual((result.code, self.positions(result)), (1, [(PROOF[0], "doc/notes.txt", 2, 1)]), ending)
            self.assertEqual((self.counts(result, "unread"), [proof["unread"] for proof in result.report["read_proof"].values()]), ((0,), [0, 0]), ending)

    def test_a_finding_of_the_reading_without_the_proof_line_stands(self):
        # Every file is read twice by each build: as it is, and with the scan's line after it. A rule
        # that reports only at the end of the text reports in the first reading alone; a finding of
        # both readings counts once.
        end_value = "zqe" + "12ab" + "34cd"
        end_rule = ("zq-fixture-end", "zq_fixture_" + "end" + r":(zqe[0-9a-f]{8})\Z")
        builds = {"pinned": write_build(self.root / "end-pinned", "0.0.15", False, RULES[:1] + [end_rule]),
                  "current": write_build(self.root / "end-current", "0.0.16", True, RULES + [end_rule]), "values": VALUES + (end_value,)}
        ending = ("zq_fixture_" + "end:" + end_value).encode()
        text = b"plain words\n" + MARK + b"\n" + ending       # no line break at its end
        tool = self.ELF + b"made-up run " + MARK + b" end of the run" + b"\x00\x02" + ending + b"\x00" * 16
        result = self.scan(self.product({"doc/notes.txt": text, "bin/tool": tool}), **builds)
        self.assertEqual((result.code, self.positions(result)), (1, [
            ("zq-fixture-end", "doc/notes.txt", 3, 1), (RULE, "bin/tool#strings", len(self.ELF), 1), (RULE, "doc/notes.txt", 2, 1)]))
        self.assertEqual(self.counts(result, "unread"), (0,))
        for proof in result.report["read_proof"].values():
            self.assertEqual((proof["unread"], proof["read_behind_the_line"]), (0, 0))
            self.assertGreaterEqual(proof["readings"], 3)
            self.assertEqual(proof["readings"], proof["proof_readings"])
        # The same for a file that had to be read behind the line of plain words: it is read there
        # as it is first, and the value at the end of its text is reported at its own line.
        builds = {"pinned": adapted(write_build(self.root / "end-kind-pinned", "0.0.19", False, RULES[:1] + [end_rule]), more_binary=[(0, self.KIND[:5])]),
                  "current": adapted(write_build(self.root / "end-kind-current", "0.0.20", True, RULES + [end_rule]), more_binary=[(0, self.KIND[:5])]),
                  "values": VALUES + (end_value,)}
        text = self.KIND + b"made-up words\n" + MARK + b"\n" + ending
        result = self.scan(self.product({"doc/notes.rtf": text}), **builds)
        self.assertEqual((result.code, self.positions(result)), (1, [("zq-fixture-end", "doc/notes.rtf", 3, 1), (RULE, "doc/notes.rtf", 2, 1)]))
        self.assertEqual([proof["unread"] for proof in result.report["read_proof"].values()], [0, 0])

    def test_a_member_whose_name_a_build_does_not_give_back_keeps_its_proof_and_its_findings(self):
        # A tar member whose name is no UTF-8 (the byte 0xe9, as tarfile and the disk carry it). Both
        # builds report the file with a replacement character where the byte stands, as a report does
        # that cannot hold the name's bytes. The scan knows the file all the same: before a build reads
        # the tree it notes that this file is known by its name without what is not printable ASCII.
        name = "doc/caf" + chr(0xdce9) + ".txt"
        builds = self.builds("names", names="replaced")
        clean, marked = b"plain words\n", b"plain words\n" + MARK + b"\n"
        result = self.scan(self.product({name: clean, "doc/notes.txt": b"plain\n"}), **builds)
        self.assertEqual((result.code, result.stderr), (0, ""))
        self.assertEqual(self.counts(result, "findings", "unread"), (0, 0))
        self.assertEqual([(proof["read_behind_the_line"], proof["unread"]) for proof in result.report["read_proof"].values()], [(0, 0), (0, 0)])
        result = self.scan(self.product({name: marked}), **builds)
        self.assertEqual((result.code, result.stderr), (1, ""))
        self.assertEqual((result.code, [(row["rule"], row["line"], row["count"], row["sha256"]) for row in result.listing["positions"]]),
                         (1, [(RULE, 2, 1, sha256(marked))]))
        self.assertEqual(result.listing["positions"][0]["path"].encode("utf-8", "surrogateescape"), b"doc/caf\xe9.txt")
        self.assertIn("| doc/caf\\xe9.txt |", result.receipt)     # the receipt is text: the byte stands there as its escape
        self.assertEqual([proof["read_behind_the_line"] for proof in result.report["read_proof"].values()], [0, 0])
        # The same for such a byte in the name of a directory.
        inside = "d" + chr(0xdce9) + "r/notes.txt"
        result = self.scan(self.product({inside: marked}), **builds)
        self.assertEqual((result.code, [(row["rule"], row["line"], row["count"], row["sha256"]) for row in result.listing["positions"]]),
                         (1, [(RULE, 2, 1, sha256(marked))]))
        self.assertEqual(result.listing["positions"][0]["path"].encode("utf-8", "surrogateescape"), b"d\xe9r/notes.txt")
        # A second member whose own name is what the report says of the first: the reported path
        # exists, and it is the other file. The scan stops before a build reads the tree: a report
        # could mean either file, and it makes no position at the other member. The same for two
        # members whose names differ only in such bytes, for a plain name that is the first's without
        # the byte, and for a UTF-8 name with a letter outside ASCII in the byte's place: the form
        # leaves out everything that is not printable ASCII, so the stop is wider than the names a
        # report would really confuse.
        said = "doc/caf" + chr(0xfffd) + ".txt"
        for other in (said, "doc/caf" + chr(0xdce8) + ".txt", "doc/caf.txt", "doc/caf" + chr(0xe9) + ".txt"):
            result = self.scan(self.product({name: marked, other: clean}), **builds)
            self.assertEqual((result.code, result.receipt), (3, None), ascii(other))
            self.assertIn("cannot be told apart in a build's report", result.stderr, ascii(other))
        # The stop comes before any build reads anything of the payload.
        fresh = self.builds("names-early", names="replaced")
        self.assertEqual(self.scan(self.product({name: marked, said: clean}), **fresh).code, 3)
        self.assertEqual([argv for build in (fresh["pinned"], fresh["current"]) for argv in calls(build) if "--report-path" in argv], [])
        # Two such members in two directories, each with a marker: each position at its own path,
        # line and digest.
        one, two = b"one\n" + MARK + b"\n", b"two words\nmore\n" + MARK + b"\n"
        result = self.scan(self.product({"a/caf" + chr(0xdce9) + ".txt": one, "b/caf" + chr(0xdce9) + ".txt": two}), **builds)
        self.assertEqual((result.code, [(row["path"].encode("utf-8", "surrogateescape"), row["line"], row["sha256"])
                                        for row in result.listing["positions"]]),
                         (1, [(b"a/caf\xe9.txt", 2, sha256(one)), (b"b/caf\xe9.txt", 3, sha256(two))]))
        # Two members whose names are UTF-8 and differ only in a letter outside ASCII are told apart
        # by their own names, as before.
        first, second = "doc/caf" + chr(0xe9) + ".txt", "doc/caf" + chr(0xe8) + ".txt"
        result = self.scan(self.product({first: marked, second: clean}), **builds)
        self.assertEqual((result.code, [(row["rule"], row["path"], row["line"], row["count"]) for row in result.listing["positions"]]),
                         (1, [(RULE, first, 2, 1)]))
        # A build that reports a finding at a path that is no file of the tree stops the scan in its
        # own words: no position is made at a path the tree does not hold.
        result = self.scan(self.product({"doc/zq-notes.txt": marked}), **self.builds("names-other", names="other"))
        self.assertEqual((result.code, result.receipt), (3, None))
        self.assertIn("a path that is no file of the directory it read", result.stderr)

    def test_a_file_one_build_passes_over_for_its_path_is_proven_for_the_other(self):
        # The current build passes over the path (its allowlist), the pinned build over the kind of
        # file. The pinned build is handed the file, shows no proof, and reads it behind the line.
        more = DEFAULT_RULES.replace("    '''\\.zqskip$''',\n", "    '''\\.zqskip$''',\n    '''(^|/)zqthird/''',\n")
        self.assertNotEqual(more, DEFAULT_RULES)
        builds = {"pinned": adapted(write_build(self.root / "path-pinned", "0.0.17", False, RULES[:1]), more_binary=[(0, self.KIND[:5])]),
                  "current": write_build(self.root / "path-current", "0.0.18", True, RULES, more)}
        text = self.KIND + b"made-up words\n" + MARK + b"\n"
        result = self.scan(self.product({"zqthird/notes.txt": text}), **builds)
        self.assertEqual((result.code, self.positions(result)), (1, [(RULE, "zqthird/notes.txt", 2, 1)]))
        self.assertEqual([(row["path"], row["reason"]) for row in result.listing["unread"]],
                         [("zqthird/notes.txt", "on one build's remaining path allowlist (the other build reads it)")])

    def test_a_build_that_does_not_report_the_proof_line_stops_the_scan(self):
        result = self.scan(self.product({"doc/notes.txt": b"plain\n"}), **self.builds("none", proof=None))
        self.assertEqual((result.code, result.receipt), (3, None))
        self.assertIn("proof-of-read line", result.stderr)

    def test_a_kept_work_directory_holds_no_proof_line_after_a_failure(self):
        # The scan stops on builds that do not report the proof-of-read line, after it has put the
        # line at the end of the tree's copies. Kept, the copies are the members' own bytes again.
        builds = self.builds("kept", proof=None)
        body = b"plain words\nmore plain words, and no line break"
        out = self.root / "kept-out"
        out.mkdir()
        done = run(TOOL, "--name", "T", "--gitleaks-pinned", builds["pinned"], "--gitleaks-current", builds["current"],
                   "--receipt", out / "receipt.txt", "--report", out / "report.json", "--unaccepted", out / "unaccepted.json",
                   "--work", self.root, "--keep-work", self.product({"doc/notes.txt": body}))
        kept = list(self.root.glob("credential-scan-*"))
        self.assertEqual((done.returncode, len(kept)), (3, 1), done.stderr)
        copies = [path for path in (kept[0] / "p0").rglob("*") if path.is_file()]
        self.assertEqual([path.read_bytes() for path in copies if path.name == "notes.txt"], [body])
        self.assertEqual([str(path.relative_to(kept[0])) for path in copies if path.read_bytes().endswith(PROOF_LINE)], [])
        shutil.rmtree(kept[0])

    def test_a_large_file_with_many_findings_shows_its_proof_line(self):
        lines = [b"made-up words of a long file, line %06d" % number for number in range(60000)]
        for number in range(0, 60000, 200):
            lines[number] = MARK
        body = b"\n".join(lines) + b"\n"
        self.assertGreater(len(body), 2 * 1024 * 1024)
        result = self.scan(self.product({"doc/long.txt": body, "doc/empty.txt": b""}))
        self.assertEqual((result.code, len(result.listing["positions"]), self.counts(result, "unread")), (1, 300, (0,)))
        self.assertEqual({row["line"] for row in result.listing["positions"]}, set(range(1, 60001, 200)))
        self.assertEqual([(proof["read_behind_the_line"], proof["unread"]) for proof in result.report["read_proof"].values()], [(0, 0), (0, 0)])


class ReceiptPaths(Scans):
    """The receipt is a text that a person reads. A member's path stands there in a form of its own and
    on one line, whatever its name holds; the listing and the report keep the path as it is."""

    def test_a_path_prints_in_a_form_of_its_own_and_on_one_line(self):
        marked = b"plain words\n" + MARK + b"\n"
        # A byte that is no UTF-8; a name of plain characters that spells such a byte's escape; a line
        # break and a carriage return in a name; the sign of a table's column; the escape sign.
        byte, spelled = "doc/caf" + chr(0xdce9) + ".txt", "doc/caf\\xe9.txt"
        broken, column, sign = "doc/two\nlines\r.txt", "doc/a|b.txt", "doc/back\\slash.txt"
        names = [byte, spelled, broken, column, sign]
        result = self.scan(self.product({name: marked for name in names}))
        self.assertEqual((result.code, sorted(row["path"] for row in result.listing["positions"])), (1, sorted(names)))
        rows = [line.split(" | ") for line in result.receipt.splitlines() if line.startswith("| p0 | " + RULE + " | ")]
        self.assertEqual([len(row) for row in rows], [6] * 5)
        self.assertEqual(sorted(row[2] for row in rows), sorted([
            "doc/caf\\xe9.txt", "doc/caf\\\\xe9.txt", "doc/two\\x0alines\\x0d.txt", "doc/a\\x7cb.txt", "doc/back\\\\slash.txt"]))
        # No name starts a line of the receipt.
        self.assertEqual([line for line in result.receipt.splitlines() if line.startswith(("lines", ".txt"))], [])


def zip_by_hand(rows, again=(), before=b"", stray=()):
    """A zip put together by hand. A row is (name, method, body, content, compressed size, size, flags)
    and may end with a file mode: body stands in the file after the local header, the directory's
    check value is that of content, and the two sizes are what the directory and the local header
    state. again adds directory entries for a record that has one: (number of the row, content,
    compressed size, size). before stands before the first record. stray adds directory entries
    (name, offset) of no size for a place in the file, whatever stands there."""
    out, directory = before, []
    for name, method, body, content, packed, size, flags, *mode in rows:
        directory.append((name, method, flags, len(out), content, packed, size, mode[0] if mode else 0))
        out += struct.pack("<4sHHHHHIIIHH", b"PK\x03\x04", 20, flags, method, 0, 0x21, binascii.crc32(content), packed, size,
                           len(name), 0) + name + body
    for number, content, packed, size in again:
        directory.append((*directory[number][:4], content, packed, size, directory[number][7]))
    for name, at in stray:
        directory.append((name, 0, 0, at, b"", 0, 0, 0))
    central = b"".join(
        struct.pack("<4sHHHHHHIIIHHHHHII", b"PK\x01\x02", 0x0314, 20, flags, method, 0, 0x21, binascii.crc32(content), packed, size,
                    len(name), 0, 0, 0, 0, mode << 16, at) + name
        for name, method, flags, at, content, packed, size, mode in directory)
    return out + central + struct.pack("<4sHHHHIIH", b"PK\x05\x06", 0, 0, len(directory), len(directory), len(central), len(out), 0)


def deflate_plain(data, last=True):
    """One block of a deflate stream that holds data as it is (a block without compression)."""
    return bytes([last]) + struct.pack("<HH", len(data), len(data) ^ 0xffff) + data


class OneWay:
    """A file that can only be written to, one byte after the other: zipfile then writes each member's
    sizes after its data (a data descriptor) and not into its local header."""

    def __init__(self):
        self.data = bytearray()

    def write(self, block):
        self.data += block
        return len(block)

    def flush(self):
        pass


class ZipMemberSizes(Scans):
    """zipfile gives of a member what the sizes of its directory entry say, and finds no fault when
    the member's data holds more. Such data is no content that was read: the archive is not opened."""

    NAME = b"pkg/notes.txt"
    VISIBLE = b"plain words\n" * 4
    HIDDEN = b"\n" + MARK + b"\n"

    def not_opened(self, archive, note):
        """zipfile reads the archive without a fault and gives the visible bytes; the scan does not
        open it, as the payload file and one level down, and reports nothing else."""
        with zipfile.ZipFile(io.BytesIO(archive)) as reader:
            self.assertEqual((reader.read(self.NAME.decode()), reader.testzip()), (self.VISIBLE, None), note)
        self.assertIn(MARK, archive, note)
        whole = self.payload(archive, name="product-0.0.0.zip")
        for product, path in ((whole, whole.name), (self.product({"lib/inner.zip": archive}), "lib/inner.zip")):
            with self.subTest(case=note, archive=path):
                result = self.scan(product)
                self.assertEqual((result.code, self.counts(result, "findings", "refused", "unread", "unread_unaccepted")),
                                 (1, (0, 1, 1, 0)))
                self.assertEqual([(row["path"], row["sha256"]) for row in result.listing["unopened"]], [(path, sha256(archive))])
                self.assertIn("| p0 | unopened-archive | %s | 0 | 1 | no |" % path, result.receipt)

    def test_a_stored_member_whose_data_is_longer_than_its_size_is_not_opened(self):
        body = self.VISIBLE + self.HIDDEN
        self.not_opened(zip_by_hand([(self.NAME, 0, body, self.VISIBLE, len(body), len(self.VISIBLE), 0)]), "stored")

    def test_a_deflated_member_whose_sizes_are_not_its_streams_is_not_opened(self):
        first, both = deflate_plain(self.VISIBLE, last=False), deflate_plain(self.VISIBLE, last=False) + deflate_plain(self.HIDDEN)
        cases = (
            # Bytes after the end of the stream, inside the compressed size.
            ("after the stream", deflate_plain(self.VISIBLE) + self.HIDDEN, None),
            # A stream that decodes to more than the size: its second block is never given.
            ("a longer stream", both, None),
            # The same stream, of which the compressed size covers the first block only.
            ("a stream past the compressed size", both, len(first)))
        for note, body, packed in cases:
            packed = len(body) if packed is None else packed
            self.not_opened(zip_by_hand([(self.NAME, 8, body, self.VISIBLE, packed, len(self.VISIBLE), 0)]), note)

    def test_a_member_of_another_method_is_read_with_the_bytes_outside_members(self):
        # Of a method other than stored and deflated the scan cannot say which bytes were decoded: the
        # member is written out, and its bytes in the archive are read by their printable runs as well.
        body = bz2.compress(self.VISIBLE) + self.HIDDEN
        archive = zip_by_hand([(self.NAME, 12, body, self.VISIBLE, len(body), len(self.VISIBLE), 0)])
        with zipfile.ZipFile(io.BytesIO(archive)) as reader:
            self.assertEqual((reader.read(self.NAME.decode()), reader.testzip()), (self.VISIBLE, None))
        result = self.scan(self.payload(archive, name="product-0.0.0.zip"))
        self.assertEqual((result.code, [(row["rule"], row["path"]) for row in result.listing["positions"]]),
                         (1, [(RULE, OUTSIDE + "headers-and-gaps")]))
        result = self.scan(self.product({"lib/inner.zip": archive}))
        self.assertEqual((result.code, [(row["rule"], row["path"]) for row in result.listing["positions"]]),
                         (1, [(RULE, "lib/inner.zip!" + OUTSIDE + "headers-and-gaps")]))

    def test_sizes_after_the_data_zip64_sizes_and_other_methods_are_read_as_before(self):
        marked = {"pkg/notes.txt": b"first line\n" + MARK + b"\n", "pkg/other.txt": b"plain too\n" * 200}
        clean = {"pkg/notes.txt": b"plain\n" * 200, "pkg/other.txt": b"plain too\n"}

        def one_way(method):
            def write(members):
                raw = OneWay()
                with zipfile.ZipFile(raw, "w", method) as archive:
                    for name, data in members.items():
                        archive.writestr(zipfile.ZipInfo(name, (1980, 1, 1, 0, 0, 0)), data, compress_type=method)
                return bytes(raw.data)
            return write

        def zip64(members):
            raw = io.BytesIO()
            with zipfile.ZipFile(raw, "w", zipfile.ZIP_DEFLATED) as archive:
                for name, data in members.items():
                    info = zipfile.ZipInfo(name, (1980, 1, 1, 0, 0, 0))
                    info.compress_type = zipfile.ZIP_DEFLATED
                    with archive.open(info, "w", force_zip64=True) as stream:
                        stream.write(data)
            return raw.getvalue()

        def method(number):
            def write(members):
                raw = io.BytesIO()
                with zipfile.ZipFile(raw, "w", number) as archive:
                    for name, data in members.items():
                        archive.writestr(zipfile.ZipInfo(name, (1980, 1, 1, 0, 0, 0)), data, compress_type=number)
                return raw.getvalue()
            return write

        writers = (("sizes after stored data", one_way(zipfile.ZIP_STORED)), ("sizes after deflated data", one_way(zipfile.ZIP_DEFLATED)),
                   ("zip64 sizes", zip64), ("bzip2", method(zipfile.ZIP_BZIP2)), ("lzma", method(zipfile.ZIP_LZMA)))
        for note, write in writers:
            archive = write(clean)
            if note.startswith("sizes after"):
                self.assertEqual(archive[6] & 8, 8, note)       # the flag of a data descriptor
            if note == "zip64 sizes":
                self.assertIn(b"\x01\x00\x10\x00", archive, note)   # the extra field of zip64 sizes
            for product in (self.payload(archive, name="product-0.0.0.zip"), self.product({"lib/inner.zip": archive})):
                result = self.scan(product)
                self.assertEqual((result.code, self.counts(result, "findings", "refused", "unread")), (0, (0, 0, 0)), note)
            result = self.scan(self.payload(write(marked), name="product-0.0.0.zip"))
            self.assertEqual((result.code, [(row["rule"], row["path"], row["line"]) for row in result.listing["positions"]]),
                             (1, [(RULE, "notes.txt", 2)]), note)

    def test_an_encrypted_member_and_a_record_listed_twice_end_in_scanned_bytes_or_a_refusal(self):
        # A member with the flag of an encrypted member is not written out; its bytes are read by their runs.
        body = b"made-up words " + MARK
        archive = zip_by_hand([(b"pkg/plain.txt", 0, b"plain\n", b"plain\n", 6, 6, 0), (self.NAME, 0, body, body, len(body), len(body), 1)])
        product = self.product({"lib/inner.zip": archive})
        result = self.scan(product)
        self.assertEqual((result.code, self.counts(result, "unread", "unread_unaccepted")), (1, (1, 1)))
        self.assertEqual(result.listing["members_not_written"], [
            {"payload": product.name, "archive": "lib/inner.zip", "sha256": sha256(archive), "reason": "encrypted", "count": 1}])
        self.assertEqual([(row["rule"], row["path"]) for row in result.listing["positions"]],
                         [(RULE, "lib/inner.zip!" + OUTSIDE + "headers-and-gaps")])
        # One record that the directory lists twice, with two sizes that both end before the marker:
        # the archive is not opened, or the bytes after the shorter sizes are read by their runs.
        body = self.VISIBLE + self.HIDDEN
        archive = zip_by_hand([(self.NAME, 0, body, self.VISIBLE, len(self.VISIBLE), len(self.VISIBLE), 0)],
                              again=[(0, self.VISIBLE[:12], 12, 12)])
        for product, path in ((self.payload(archive, name="product-0.0.0.zip"), "product-0.0.0.zip"),
                              (self.product({"lib/inner.zip": archive}), "lib/inner.zip")):
            result = self.scan(product)
            self.assertEqual(result.code, 1, path)
            self.assertTrue([row["path"] for row in result.listing["unopened"]] == [path]
                            or [row["rule"] for row in result.listing["positions"]] == [RULE], path)


def zip64_end(members, comment=b"", sector=b"", record_size=None, total=None, locator=b"PK\x06\x07", last=b"PK\x05\x06"):
    """A zip whose end is written the zip64 way: the zip64 end record (with sector as its data
    sector), its locator, and an end record whose own numbers say "look there", with the archive's
    comment. record_size is the size the zip64 end record states of itself, total its 8 bytes for
    the number of entries, locator and last the signatures of the locator and of the end record."""
    plain = zip_plain(members)
    directory, end = plain.index(b"PK\x01\x02"), plain.rindex(b"PK\x05\x06")
    number = len(members).to_bytes(8, "little")
    size = 44 + len(sector) if record_size is None else record_size
    record = struct.pack("<4sQHHII", b"PK\x06\x06", size, 45, 45, 0, 0) + number + (number if total is None else total) \
        + struct.pack("<QQ", end - directory, directory) + sector
    return plain[:end] + record + locator + struct.pack("<IQI", 0, end, 1) \
        + last + struct.pack("<HHHHIIH", 0, 0, 0xffff, 0xffff, 0xffffffff, 0xffffffff, len(comment)) + comment


class Zip64End(Scans):
    """The end of a zip is the end of its end record's comment, also when a zip64 end record and its
    locator stand before the end record."""

    MEMBERS = {"pkg/notes.txt": b"plain words\n"}

    def both(self, archive):
        """The archive as the payload file and one level down, with the start of its paths."""
        return ((self.payload(archive, name="product-0.0.0.zip"), "product-0.0.0.zip", ""),
                (self.product({"lib/inner.zip": archive}), "lib/inner.zip", "lib/inner.zip!"))

    def read(self, archive, comment):
        with zipfile.ZipFile(io.BytesIO(archive)) as reader:
            self.assertEqual((reader.namelist(), reader.comment, reader.testzip()), (list(self.MEMBERS), comment, None))

    def test_a_long_comment_of_a_zip64_archive_is_read_whole(self):
        # 76 bytes before the comment's end is where the zip64 end record, its locator and the end
        # record end when they are counted from the zip64 end record: the marker stands across it.
        comment = b"A" * 50 + MARK + b"B" * 60
        self.assertTrue(50 < len(comment) - 76 < 50 + len(MARK))
        archive = zip64_end(self.MEMBERS, comment)
        self.read(archive, comment)
        for product, _path, start in self.both(archive):
            result = self.scan(product)
            self.assertEqual((result.code, [(row["rule"], row["path"]) for row in result.listing["positions"]]),
                             (1, [(RULE, start + OUTSIDE + "headers-and-gaps")]), start)
        # The same comment at the end of a zip without the zip64 records.
        result = self.scan(self.payload(zip_plain(self.MEMBERS, comment=comment), name="product-0.0.0.zip"))
        self.assertEqual([(row["rule"], row["path"]) for row in result.listing["positions"]], [(RULE, OUTSIDE + "headers-and-gaps")])
        # The data sector of the zip64 end record is inside the archive too.
        archive = zip64_end(self.MEMBERS, sector=b"\x00" + MARK + b"\x00")
        self.read(archive, b"")
        for product, _path, start in self.both(archive):
            result = self.scan(product)
            self.assertEqual([(row["rule"], row["path"]) for row in result.listing["positions"]],
                             [(RULE, start + OUTSIDE + "headers-and-gaps")], start)

    def test_a_zip64_archive_has_nothing_after_its_end_but_what_follows_its_comment(self):
        for comment in (b"", b"a plain note", b"a plain note " * 9):
            archive = zip64_end(self.MEMBERS, comment)
            self.read(archive, comment)
            result = self.scan(self.payload(archive, name="product-0.0.0.zip"))
            self.assertEqual((result.code, self.counts(result, "findings", "refused", "unread")), (0, (0, 0, 0)), comment)
            # The member and the runs of the archive's records: no file of bytes after the end.
            self.assertEqual(result.report["payload"][0]["files"], 2, comment)
            tail = b"\n" + MARK + b"\n"
            for product, _path, start in self.both(archive + tail):
                result = self.scan(product)
                self.assertEqual([(row["rule"], row["path"], row["sha256"]) for row in result.listing["positions"]],
                                 [(RULE, start + OUTSIDE + "after-end", sha256(tail))], (comment, start))

    def test_a_zip64_archive_with_text_before_it_is_opened(self):
        # The form of an archive that unpacks itself: text first, then the archive, here with zip64 records.
        text, comment = b"#!/bin/sh\n" + MARK + b"\n", b"a plain note"
        archive = text + zip64_end(self.MEMBERS, comment)
        self.read(archive, comment)
        for product, path, start in self.both(archive):
            result = self.scan(product)
            self.assertEqual(result.listing["unopened"], [], path)
            # One level down the archive file, which starts with text, is read as a file too.
            also = [(RULE, path, None)] if start else []
            self.assertEqual([(row["rule"], row["path"], row["sha256"] if OUTSIDE in row["path"] else None) for row in result.listing["positions"]],
                             also + [(RULE, start + OUTSIDE + "before-start", sha256(text))], path)

    def test_a_zip64_archive_whose_locator_is_damaged_is_not_opened(self):
        for product, path, _start in self.both(zip64_end(self.MEMBERS, locator=b"PK\x06\x08")):
            result = self.scan(product)
            self.assertEqual((result.code, self.counts(result, "findings", "refused", "unread")), (1, (0, 1, 1)), path)
            self.assertEqual([row["path"] for row in result.listing["unopened"]], [path], path)

    def test_the_end_is_found_from_the_records_themselves(self):
        # The way from the zip64 end record to the end record, asked of the extractor itself: the record's
        # own 8-byte size leads to the locator, the locator to the end record, and each must be there.
        spec = importlib.util.spec_from_file_location("release_payload_deep_extract_zip64_rows", EXTRACTOR)
        deep = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(deep)
        path = self.root / "end.zip"

        def end(archive, comment=b""):
            path.write_bytes(archive)
            at = archive.find(b"PK\x06\x06")
            with open(path, "rb") as source:
                return deep.zip_end(source, at if at >= 0 else archive.rindex(b"PK\x05\x06"), comment)

        comment, sector = b"a plain note", b"\x00made-up bytes of a data sector\x00"
        for archive in (zip_plain(self.MEMBERS, comment=comment), zip64_end(self.MEMBERS, comment),
                        zip64_end(self.MEMBERS, comment, sector=sector)):
            self.assertEqual(end(archive, comment), len(archive))
            self.assertEqual(end(archive + b"bytes after the end", comment), len(archive))
        broken = (
            ("no locator where the size leads (a size past the file, as 4 bytes the true size)", zip64_end(self.MEMBERS, record_size=44 + (1 << 32))),
            ("no locator where the size leads (a size too small)", zip64_end(self.MEMBERS, record_size=24)),
            ("no locator where the size leads, and an end record's signature 20 bytes on",
             zip64_end(self.MEMBERS, record_size=0, total=b"PK\x05\x06" + b"\x00" * 4)),
            ("a locator without its signature", zip64_end(self.MEMBERS, locator=b"PK\x06\x08")),
            ("an end record without its signature", zip64_end(self.MEMBERS, last=b"PK\x05\x07")),
            ("a size that leaves out the data sector", zip64_end(self.MEMBERS, sector=sector, record_size=44)))
        for note, archive in broken:
            with self.assertRaises(ValueError, msg=note):
                end(archive)
        # Neither record at the place given.
        path.write_bytes(zip_plain(self.MEMBERS))
        with open(path, "rb") as source, self.assertRaises(ValueError):
            deep.zip_end(source, 0, b"")


class ZipStartAndEnd(Scans):
    """Where a zip starts and ends decides which of its bytes are a file before it, a file after it,
    and runs in between. Both places are records of the archive, and are held to be."""

    MEMBERS = {"pkg/notes.txt": b"plain words\n"}

    def both(self, archive):
        return ((self.payload(archive, name="product-0.0.0.zip"), "product-0.0.0.zip", ""),
                (self.product({"lib/inner.zip": archive}), "lib/inner.zip", "lib/inner.zip!"))

    def test_bytes_after_the_comment_of_a_zip_are_the_file_after_its_end(self):
        tail = b"\n" + MARK + b"\n"
        for comment in (b"a plain note", b"a plain note, " + TAG):
            archive = zip_plain(self.MEMBERS, comment=comment) + tail
            with zipfile.ZipFile(io.BytesIO(archive)) as reader:
                self.assertEqual((reader.namelist(), reader.comment), (list(self.MEMBERS), comment))
            for product, _path, start in self.both(archive):
                result = self.scan(product)
                found = [(RULE, start + OUTSIDE + "after-end", sha256(tail))]
                if TAG in comment:      # the comment itself is among the runs, and not in the file after the end
                    found.append((TAG_RULE, start + OUTSIDE + "headers-and-gaps", None))
                self.assertEqual([(row["rule"], row["path"], row["sha256"] if row["path"].endswith("after-end") else None)
                                  for row in result.listing["positions"]], found, (comment, start))

    def test_bytes_before_a_zip_without_members_are_the_file_before_its_start(self):
        text = b"#!/bin/sh\n" + MARK + b"\n"
        empty = zip_by_hand([])
        with zipfile.ZipFile(io.BytesIO(text + empty)) as reader:
            self.assertEqual(reader.namelist(), [])
        for product, _path, start in self.both(text + empty):
            result = self.scan(product)
            self.assertIn((RULE, start + OUTSIDE + "before-start", sha256(text)),
                          [(row["rule"], row["path"], row["sha256"]) for row in result.listing["positions"]], start)
        result = self.scan(self.payload(b"plain words before the archive\n" + empty, name="product-0.0.0.zip"))
        self.assertEqual((result.code, self.counts(result, "findings", "refused", "unread")), (0, (0, 0, 0)))

    def test_a_directory_entry_that_names_no_record_is_not_opened(self):
        # The lowest place a directory entry names is where the archive starts. zipfile reads the place
        # of an entry only when it reads the member, so an entry of a directory can name any place: here
        # one in the middle of the text before the archive, which would cut that text in two files.
        before = b"plain words, " + MARK + b", more plain words\n"
        row = (b"pkg/notes.txt", 0, b"plain words\n", b"plain words\n", 12, 12, 0)
        archive = zip_by_hand([row], before=before, stray=[(b"pkg/sub/", before.index(MARK) + 8)])
        with zipfile.ZipFile(io.BytesIO(archive)) as reader:
            self.assertEqual((reader.namelist(), reader.read("pkg/notes.txt")), (["pkg/notes.txt", "pkg/sub/"], b"plain words\n"))
        for product, path, _start in self.both(archive):
            result = self.scan(product)
            # Not opened; and the file, which starts with text, is read as the file it is.
            self.assertEqual((result.code, self.counts(result, "findings", "refused", "unread")), (1, (1, 2, 1)), path)
            self.assertEqual([row["path"] for row in result.listing["unopened"]], [path], path)
            self.assertEqual([(row["rule"], row["path"], row["line"]) for row in result.listing["positions"]], [(RULE, path, 1)], path)
        # A place that holds the entry's own name where a record would hold it, and no record's signature.
        named = (b"plain words before a zip, " * 2)[:26] + struct.pack("<HH", 8, 0) + b"pkg/sub/ and more plain words\n"
        for product, path, _start in self.both(zip_by_hand([row], before=named, stray=[(b"pkg/sub/", 0)])):
            result = self.scan(product)
            self.assertEqual([row["path"] for row in result.listing["unopened"]], [path], path)
        # With every entry at its record the same text is the file before the start.
        for product, _path, start in self.both(zip_by_hand([row], before=before)):
            result = self.scan(product)
            self.assertIn((RULE, start + OUTSIDE + "before-start"), [(row["rule"], row["path"]) for row in result.listing["positions"]])

    def test_a_directory_entry_at_a_record_of_another_name_is_not_opened(self):
        # A whole archive stands before the zip, and a directory entry of no size names that archive's
        # first record: a record does stand there, of another name. The zip would start at 0, the
        # archive before it would be no file before the start and would never be opened.
        words = b"made-up words, " * 30
        raw = io.BytesIO()
        with zipfile.ZipFile(raw, "w") as first:
            first.writestr(zipfile.ZipInfo("deep/notes.txt", (1980, 1, 1, 0, 0, 0)), words + b"\n" + MARK + b"\n" + words,
                           compress_type=zipfile.ZIP_DEFLATED)
        first = raw.getvalue()
        self.assertNotIn(MARK, first)
        row = (b"pkg/notes.txt", 0, b"plain words\n", b"plain words\n", 12, 12, 0)
        archive = zip_by_hand([row], before=first, stray=[(b"pkg/sub/", 0)])
        with zipfile.ZipFile(io.BytesIO(archive)) as reader:
            self.assertEqual((reader.namelist(), reader.read("pkg/notes.txt")), (["pkg/notes.txt", "pkg/sub/"], b"plain words\n"))
        for product, path, _start in self.both(archive):
            result = self.scan(product)
            self.assertEqual(result.code, 1, path)
            self.assertEqual(([row["path"] for row in result.listing["unopened"]], result.listing["positions"]), ([path], []), path)
        # With every entry at its own record the archive before the zip is opened as the file before the start.
        for product, _path, start in self.both(zip_by_hand([row], before=first)):
            result = self.scan(product)
            self.assertEqual((result.code, [(row["rule"], row["path"], row["line"]) for row in result.listing["positions"]]),
                             (1, [(RULE, start + OUTSIDE + "before-start!deep/notes.txt", 2)]), start)
        # An entry of a directory at a record of its own name is a directory, as before.
        own = zip_by_hand([row, (b"pkg/sub/", 0, b"", b"", 0, 0, 0)], before=first)
        result = self.scan(self.payload(own, name="product-0.0.0.zip"))
        self.assertEqual([row["path"] for row in result.listing["unopened"]], [])
        # A name outside ASCII is the entry's own name in the encoding the entry's flag states.
        name = "pkg/caf" + chr(0xe9) + ".txt"
        raw = io.BytesIO()
        with zipfile.ZipFile(raw, "w") as flagged:
            flagged.writestr(zipfile.ZipInfo(name, (1980, 1, 1, 0, 0, 0)), b"plain words\n")
        self.assertIn(name.encode(), raw.getvalue())
        for archive in (raw.getvalue(), zip_by_hand([(b"pkg/caf\x82.txt", 0, b"plain words\n", b"plain words\n", 12, 12, 0)])):
            with zipfile.ZipFile(io.BytesIO(archive)) as reader:
                self.assertEqual(reader.namelist(), [name])
            result = self.scan(self.payload(archive, name="product-0.0.0.zip"))
            self.assertEqual((result.code, self.counts(result, "findings", "refused", "unread")), (0, (0, 0, 0)))


class ZipEntryKinds(Scans):
    """A zip entry is a name and data, whatever its mode says. The data of an entry with the mode of a
    link (the link's target) or of a special file is read as text: written as a regular file among
    the bytes outside members, under a name that is a number, and nothing is followed. An entry of a
    directory has no data; one that has is not written out, which refuses."""

    WORDS = b"made-up words, " * 30
    CONTENT = WORDS + b"\n" + MARK + b"\n" + WORDS
    LINK, PIPE, DEVICE = stat.S_IFLNK | 0o777, stat.S_IFIFO | 0o644, stat.S_IFCHR | 0o600

    def both(self, archive):
        return ((self.payload(archive, name="product-0.0.0.zip"), "product-0.0.0.zip", ""),
                (self.product({"lib/inner.zip": archive}), "lib/inner.zip", "lib/inner.zip!"))

    def with_entry(self, name, data, mode, method=zipfile.ZIP_STORED):
        """A zip of one plain member and one entry of the mode given."""
        raw = io.BytesIO()
        with zipfile.ZipFile(raw, "w", method) as archive:
            archive.writestr(zipfile.ZipInfo("pkg/plain.txt", (1980, 1, 1, 0, 0, 0)), b"plain\n")
            info = zipfile.ZipInfo(name, (1980, 1, 1, 0, 0, 0))
            info.external_attr = mode << 16
            archive.writestr(info, data, compress_type=method)
        return raw.getvalue()

    def test_the_data_of_a_link_or_special_entry_is_read_as_a_file(self):
        for mode, kind in ((self.LINK, "link"), (self.PIPE, "special"), (self.DEVICE, "special")):
            for method in (zipfile.ZIP_STORED, zipfile.ZIP_DEFLATED):
                for product, _path, start in self.both(self.with_entry("pkg/entry", self.CONTENT, mode, method)):
                    result = self.scan(product)
                    self.assertEqual((result.code, [(row["rule"], row["path"], row["line"], row["sha256"]) for row in result.listing["positions"]]),
                                     (1, [(RULE, "%s%s%s-00000" % (start, OUTSIDE, kind), 2, sha256(self.CONTENT))]), (oct(mode), method, start))
                    # The entry is no member at its place, as before; it refuses nothing by itself.
                    self.assertEqual(result.report["payload"][0]["member_skips"], {"zip:" + kind: 1}, (oct(mode), method))
                    self.assertEqual((result.listing["members_not_written"], result.listing["unread"]), ([], []))
        # Each such entry has a number of its own, in the order of the directory.
        raw = io.BytesIO()
        with zipfile.ZipFile(raw, "w") as archive:
            for number, data in enumerate((b"plain target\n", MARK + b"\n", TAG + b"\n")):
                info = zipfile.ZipInfo("pkg/link-%d" % number, (1980, 1, 1, 0, 0, 0))
                info.external_attr = self.LINK << 16
                archive.writestr(info, data)
        result = self.scan(self.payload(raw.getvalue(), name="product-0.0.0.zip"))
        self.assertEqual([(row["rule"], row["path"]) for row in result.listing["positions"]],
                         [(RULE, OUTSIDE + "link-00001"), (TAG_RULE, OUTSIDE + "link-00002")])

    def test_a_link_entry_makes_no_link_and_no_file_of_its_name_and_nothing_is_followed(self):
        spec = importlib.util.spec_from_file_location("release_payload_deep_extract_link_rows", EXTRACTOR)
        deep = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(deep)
        # A file outside the tree that holds the marker, and links that name it: by its whole path and
        # by a path that climbs out of the tree.
        target = self.root / "zq-made-up-target.txt"
        target.write_bytes(MARK + b"\n")
        for number, text in enumerate((str(target).encode(), b"../" * 12 + str(target).encode().lstrip(b"/"))):
            archive = self.with_entry("pkg/zq-made-up-link", text, self.LINK)
            source = self.root / ("links-%d.zip" % number)
            source.write_bytes(archive)
            tree = self.root / ("tree-%d" % number)
            deep.expand_file(str(source), str(tree))
            files = {str(path.relative_to(tree)): path for path in tree.rglob("*")}
            # Nothing in the tree is a link, nothing is named after the entry, and the target's bytes are nowhere.
            self.assertEqual([name for name, path in files.items() if path.is_symlink() or not (path.is_file() or path.is_dir())], [])
            self.assertEqual([name for name in files if "zq-made-up-link" in name], [])
            written = files["links-%d.zip.extracted/%slink-00000" % (number, OUTSIDE)]
            self.assertEqual((written.read_bytes(), written.stat().st_mode & 0o777), (text, 0o600))
            self.assertEqual([name for name, path in files.items() if path.is_file() and MARK in path.read_bytes()], [])
            # The scan reads the link's target as the text it is: a path, with no marker in it.
            result = self.scan(self.product({"lib/inner.zip": archive}))
            self.assertEqual((result.code, self.counts(result, "findings", "refused", "unread")), (0, (0, 0, 0)), number)

    def test_the_name_of_a_link_entry_goes_through_the_name_check(self):
        for name, reason in (("pkg/a!b", "separator-in-name"), ("pkg/tool#strings", "strings-mark-in-name"), ("../outside", "dotdot")):
            archive = self.with_entry(name, b"plain target\n", self.LINK)
            product = self.product({"lib/inner.zip": archive})
            result = self.scan(product)
            self.assertEqual((result.code, self.counts(result, "unread", "unread_unaccepted")), (1, (1, 1)), name)
            self.assertEqual(result.listing["members_not_written"], [
                {"payload": product.name, "archive": "lib/inner.zip", "sha256": sha256(archive), "reason": reason, "count": 1}], name)

    def test_the_data_of_a_link_entry_is_read_below_a_path_that_a_build_skips(self):
        more = DEFAULT_RULES.replace("    '''\\.zqskip$''',\n", "    '''\\.zqskip$''',\n    '''(^|/)zqthird/''',\n")
        builds = {"pinned": write_build(self.root / "link-pinned", "0.0.7", False, RULES[:1], more),
                  "current": write_build(self.root / "link-current", "0.0.8", True, RULES, more)}
        result = self.scan(self.product({"zqthird/inner.zip": self.with_entry("pkg/entry", self.CONTENT, self.LINK)}), **builds)
        self.assertEqual([(row["rule"], row["path"], row["count"]) for row in result.listing["positions"]],
                         [(RULE, "zqthird/inner.zip!" + OUTSIDE + "link-00000", 1)])
        self.assertEqual([row["path"] for row in result.listing["unread"]], ["zqthird/inner.zip!pkg/plain.txt"])

    def test_a_directory_entry_with_data_is_not_written_out(self):
        stream = gzip.compress(self.CONTENT, mtime=0)[10:-8]    # the deflate stream of a gzip member
        self.assertNotIn(MARK, stream)
        plain = (b"pkg/plain.txt", 0, b"plain\n", b"plain\n", 6, 6, 0)
        cases = (("deflated", (b"pkg/sub/", 8, stream, self.CONTENT, len(stream), len(self.CONTENT), 0)),
                 ("stored", (b"pkg/sub/", 0, self.WORDS, self.WORDS, len(self.WORDS), len(self.WORDS), 0)),
                 ("a size and no data", (b"pkg/sub/", 0, b"", b"", 0, 5, 0)))
        for note, entry in cases:
            archive = zip_by_hand([plain, entry])
            with zipfile.ZipFile(io.BytesIO(archive)) as reader:
                self.assertEqual(reader.namelist(), ["pkg/plain.txt", "pkg/sub/"], note)
            for product, path, _start in self.both(archive):
                result = self.scan(product)
                self.assertEqual((result.code, self.counts(result, "findings", "unread", "unread_unaccepted")), (1, (0, 1, 1)), (note, path))
                self.assertEqual([(row["archive"], row["sha256"], row["reason"], row["count"]) for row in result.listing["members_not_written"]],
                                 [(path, sha256(archive), "directory-with-data", 1)], (note, path))
        # A directory entry without data is a directory, as before.
        archive = zip_by_hand([plain, (b"pkg/sub/", 0, b"", b"", 0, 0, 0)])
        result = self.scan(self.payload(archive, name="product-0.0.0.zip"))
        self.assertEqual((result.code, self.counts(result, "findings", "refused", "unread")), (0, (0, 0, 0)))

    def test_a_record_listed_twice_is_not_opened_and_no_output_names_its_member(self):
        # zipfile reads a record that two directory entries name, and says so in a warning that holds
        # the member's name. A warning of a reader is a fault of the archive here, and nothing of an
        # archive's names is written to the scan's standard error.
        body = b"plain words\n" * 4
        archive = zip_by_hand([(b"pkg/zq-made-up-member.txt", 0, body, body, len(body), len(body), 0)], again=[(0, body, len(body), len(body))])
        for product, path, _start in self.both(archive):
            result = self.scan(product)
            self.assertEqual(result.stderr, "", path)
            self.assertEqual((result.code, self.counts(result, "findings", "refused", "unread")), (1, (0, 1, 1)), path)
            self.assertEqual([row["path"] for row in result.listing["unopened"]], [path], path)


def tar_old(members):
    """A tar of the old form: its headers hold no "ustar" mark (and so nothing at byte 257)."""
    raw = bytearray(tar_bytes(members, compressed=False))
    with tarfile.open(fileobj=io.BytesIO(bytes(raw))) as archive:
        headers = [member.offset for member in archive]
    for at in headers:
        raw[at + 257:at + 265] = b"\x00" * 8
        raw[at + 148:at + 156] = b" " * 8
        raw[at + 148:at + 156] = b"%06o\x00 " % sum(raw[at:at + 512])
    return bytes(raw)


class ContainersByTheirHead(Scans):
    """A container is known by its head. Two heads that the usual tools open were plain files to the
    scan: a zstd file that starts with a skippable frame, and a tar without the "ustar" mark."""

    def test_a_zstd_file_that_starts_with_a_skippable_frame_is_opened(self):
        # The marker is compressed once more inside, so that no build finds it in the file as it is.
        frame = zstd_compress(gzip.compress(b"first line\n" + MARK + b"\n", mtime=0))
        if frame is None:
            self.skipTest("no zstd encoder on this host")
        self.assertNotIn(MARK, frame)
        packed = zstd_skippable(b"made-up bytes") + frame
        result = self.scan(self.payload(packed, name="notes-0.0.0.txt.gz.zst"))
        self.assertEqual((result.code, [(row["rule"], row["path"], row["line"]) for row in result.listing["positions"]]),
                         (1, [(RULE, "notes-0.0.0.txt.gz!notes-0.0.0.txt", 2)]))
        result = self.scan(self.product({"data/notes.txt.gz.zst": packed}))
        self.assertEqual((result.code, [(row["rule"], row["path"], row["line"]) for row in result.listing["positions"]]),
                         (1, [(RULE, "data/notes.txt.gz.zst!notes.txt.gz!notes.txt", 2)]))
        # Skippable frames alone hold no stream: the file is not opened.
        alone = self.product({"data/notes.txt.zst": zstd_skippable(b"made-up bytes") + zstd_skippable(b"more made-up bytes")})
        result = self.scan(alone)
        self.assertEqual((result.code, self.counts(result, "findings", "refused", "unread")), (1, (0, 1, 1)))
        self.assertEqual([row["path"] for row in result.listing["unopened"]], ["data/notes.txt.zst"])

    def test_a_tar_without_the_ustar_mark_is_opened(self):
        packed = gzip.compress(b"first line\n" + MARK + b"\n", mtime=0)
        archive = tar_old({"pkg/notes.txt.gz": packed, "pkg/plain.txt": b"plain\n"})
        self.assertNotIn(b"ustar", archive)
        with tarfile.open(fileobj=io.BytesIO(archive)) as reader:
            self.assertEqual(reader.getnames(), ["pkg/notes.txt.gz", "pkg/plain.txt"])
        result = self.scan(self.payload(archive, name="product-0.0.0.tar"))
        self.assertEqual((result.code, [(row["rule"], row["path"], row["line"]) for row in result.listing["positions"]]),
                         (1, [(RULE, "notes.txt.gz!notes.txt", 2)]))
        result = self.scan(self.product({"lib/old.tar": archive}))
        self.assertEqual((result.code, [(row["rule"], row["path"], row["line"]) for row in result.listing["positions"]]),
                         (1, [(RULE, "lib/old.tar!pkg/notes.txt.gz!notes.txt", 2)]))
        # Its bytes outside members are read like those of any tar. (One level down the archive file
        # stays in the tree, and a file without a mark is read as a file too.)
        result = self.scan(self.product({"lib/old.tar": tar_old({"pkg/plain.txt": b"plain\n"}) + MARK + b"\n"}))
        self.assertEqual([(row["rule"], row["path"]) for row in result.listing["positions"]],
                         [(RULE, "lib/old.tar"), (RULE, "lib/old.tar!" + OUTSIDE + "after-end")])
        # Text that is no tar header stays a file.
        result = self.scan(self.product({"notes.txt": b"plain words\n" * 60}))
        self.assertEqual((result.code, result.report["payload"][0]["archives_opened"]), (0, 1))


class TarMemberKinds(Scans):
    """A tar member that is no regular file, no link and no directory can hold data all the same. Data
    that tarfile gives is read like a regular member's; data it does not give refuses."""

    def with_member(self, name, data, kind):
        """A tar of one plain member and one member of the type given, with data."""
        raw = io.BytesIO()
        with tarfile.open(fileobj=raw, mode="w", format=tarfile.PAX_FORMAT) as archive:
            for member, content, sort in (("pkg/plain.txt", b"plain\n", tarfile.REGTYPE), (name, data, kind)):
                info = tarfile.TarInfo(member)
                info.type, info.size = sort, len(content)
                archive.addfile(info, io.BytesIO(content))
        return raw.getvalue()

    def test_a_member_of_an_unknown_type_is_read_like_a_regular_member(self):
        packed = gzip.compress(b"first line\n" + MARK + b"\n", mtime=0)
        archive = self.with_member("pkg/odd.gz", packed, b"Z")
        with tarfile.open(fileobj=io.BytesIO(archive)) as reader:
            self.assertEqual((reader.getnames(), reader.extractfile("pkg/odd.gz").read()), (["pkg/plain.txt", "pkg/odd.gz"], packed))
        for product, path in ((self.payload(archive, name="product-0.0.0.tar"), "odd.gz!odd"),
                              (self.product({"lib/inner.tar": archive}), "lib/inner.tar!pkg/odd.gz!odd")):
            result = self.scan(product)
            self.assertEqual((result.code, [(row["rule"], row["path"], row["line"]) for row in result.listing["positions"]]),
                             (1, [(RULE, path, 2)]), path)
            self.assertEqual(result.report["payload"][0]["member_skips"], {}, path)

    def test_a_special_member_with_data_that_tarfile_does_not_give_refuses(self):
        # A named pipe with a size and data blocks: tarfile gives no data for it and reads on at the
        # block after its header. The data here is a deflate stream, which no build finds anything in.
        stream = gzip.compress(b"made-up words, " * 30 + MARK + b"\n", mtime=0)[10:-8]
        self.assertNotIn(MARK, stream)
        archive = self.with_member("pkg/pipe", stream, tarfile.FIFOTYPE)
        for product, path in ((self.payload(archive, name="product-0.0.0.tar"), "product-0.0.0.tar"),
                              (self.product({"lib/inner.tar": archive}), "lib/inner.tar")):
            result = self.scan(product)
            self.assertEqual(result.code, 1, path)
            self.assertEqual([(row["archive"], row["sha256"], row["reason"], row["count"]) for row in result.listing["members_not_written"]],
                             [(path, sha256(archive), "special-with-data", 1)], path)
        # Without data such a member holds nothing, is counted as before and refuses nothing.
        result = self.scan(self.payload(self.with_member("pkg/pipe", b"", tarfile.FIFOTYPE), name="product-0.0.0.tar"))
        self.assertEqual((result.code, self.counts(result, "findings", "refused", "unread")), (0, (0, 0, 0)))
        self.assertEqual(result.report["payload"][0]["member_skips"], {"tar:special": 1})


class CompiledFileRuns(Scans):
    """The printable runs of a compiled file are read from a file of their own. A first run that starts
    like a binary signature would make that a file no build reads, and every run of it would be lost."""

    ELF = b"\x7fELF\x02\x01\x01" + b"\x00" * 57

    def test_a_first_run_that_starts_like_a_signature_hides_no_run(self):
        later = b"made-up run " + MARK + b" end of the run"
        for first in (b"plain made-up words", b"%PDF-1.4 made-up words", b"BZh91AY&SY made-up words"):
            tool = self.ELF + first + b"\x00\x02" + later + b"\x00" * 16
            product = self.product({"bin/tool": tool})
            result = self.scan(product)
            place = len(self.ELF) + len(first) + 2
            self.assertEqual((result.code, [(row["rule"], row["path"], row["line"], row["count"], row["sha256"]) for row in result.listing["positions"]]),
                             (1, [(RULE, "bin/tool#strings", place, 1, sha256(tool))]), first)
            self.assertEqual([proof["read_behind_the_line"] for proof in result.report["read_proof"].values()], [0, 0], first)
            # A position is the run's own place in the file, whatever stands first in the file of the runs.
            accepted = self.accepted(positions=[position("bin/tool#strings", tool, place)])
            result = self.scan(product, accepted=accepted)
            self.assertEqual((result.code, self.counts(result, "findings", "accepted_positions", "refused")), (0, (1, 1, 0)), first)

    # A third made-up marker, for a rule that reports only at the first byte of the file a build reads.
    START_NAME = "zq_fixture_" + "start"
    START_VALUE = "zqs" + "51d0" + "9be2"
    START = (START_NAME + "=" + START_VALUE).encode()
    START_RULE = "zq-fixture-start"

    def anchored(self, name):
        """The two builds with one more rule each, which reports only at the first byte of a file."""
        rules = RULES + [(self.START_RULE, r"\A" + self.START_NAME + "=(zqs[0-9a-f]{8})")]
        return {"pinned": write_build(self.root / (name + "-pinned"), "0.0.11", False, rules[:1] + rules[2:]),
                "current": write_build(self.root / (name + "-current"), "0.0.12", True, rules),
                "values": (*VALUES, self.START_VALUE)}

    def test_a_rule_that_reports_only_at_the_first_byte_reports_the_first_run(self):
        # The first run of a compiled file stands at the first byte of the file of its runs, and a run
        # that is read alone at the first byte of its own file, when it starts like no signature.
        builds = self.anchored("first")
        tool = self.ELF + self.START + b"\x00\x01" + b"a second made-up run" + b"\x00" * 9
        product = self.product({"bin/tool": tool, "doc/notes.txt": self.START + b"\nplain words\n"})
        result = self.scan(product, **builds)
        self.assertEqual((result.code, [(row["rule"], row["path"], row["line"], row["count"]) for row in result.listing["positions"]]),
                         (1, [(self.START_RULE, "bin/tool#strings", len(self.ELF), 1), (self.START_RULE, "doc/notes.txt", 1, 1)]))
        self.assertEqual(result.report["method_artifacts"], 0)
        accepted = self.accepted(positions=[position("bin/tool#strings", tool, len(self.ELF), rule=self.START_RULE),
                                            position("doc/notes.txt", self.START + b"\nplain words\n", 1, rule=self.START_RULE)])
        result = self.scan(product, accepted=accepted, **builds)
        self.assertEqual((result.code, self.counts(result, "findings", "accepted_positions", "refused")), (0, (2, 2, 0)))

    def test_a_signature_further_in_hides_no_run(self):
        # The tar mark is a signature at a place further in (byte 257). A file of runs, and a run that is
        # read alone, with the mark there is read by no build; and what the scan writes before such a
        # file must not move other bytes to that place.
        line = b"printable runs of a file, one run on a line"
        self.assertIn(line, TOOL.read_bytes())

        def run_with(*places):
            run = bytearray(b"m" * 300)
            for place in places:
                run[place:place + 5] = b"ustar"
            return bytes(run)

        later = b"made-up run " + MARK + b" end of the run"
        every = run_with(*range(257, 0, -len(line) - 1))    # a mark wherever a further line would move one to that place
        both = b"%PDF-1.4 " + run_with(256)[9:] + b" " + MARK   # read alone, one line break would move its mark to that place
        for note, first, own in (("the mark at its place", run_with(257), 0),
                                 ("a second mark that one line would move to that place", run_with(257, 257 - len(line) - 1), 0),
                                 ("a mark for every line that would be written", every, 0),
                                 ("the mark and the marker in one run", run_with(257) + b" " + MARK, 1),
                                 ("a signature at the first byte and the mark one byte before its place, in a run with the marker", both, 1)):
            tool = self.ELF + first + b"\x00\x02" + later + b"\x00" * 16
            result = self.scan(self.product({"bin/tool": tool}))
            found = [(RULE, "bin/tool#strings", len(self.ELF), 1)] * own + [(RULE, "bin/tool#strings", len(self.ELF) + len(first) + 2, 1)]
            self.assertEqual((result.code, [(row["rule"], row["path"], row["line"], row["count"]) for row in result.listing["positions"]]),
                             (1, found), note)
            self.assertEqual(result.report["method_artifacts"], 0, note)
            # What the scan writes before such a file lets both builds read it as it is: nothing has
            # to be read again behind a line of plain words.
            self.assertEqual([proof["read_behind_the_line"] for proof in result.report["read_proof"].values()], [0, 0], note)

    def test_a_run_below_the_line_of_plain_words_keeps_its_place(self):
        # The file of the runs starts with the line of plain words here, and the marker stands in the
        # first of two runs: its place is that run's own, not the next run's.
        first = b"%PDF-1.4 made-up words " + MARK
        tool = self.ELF + first + b"\x00\x02" + b"a second made-up run, plain" + b"\x00" * 16
        result = self.scan(self.product({"bin/tool": tool}))
        self.assertEqual((result.code, [(row["rule"], row["path"], row["line"], row["count"]) for row in result.listing["positions"]]),
                         (1, [(RULE, "bin/tool#strings", len(self.ELF), 1)]))

    def test_a_rule_that_reports_only_at_the_first_byte_reports_a_run_that_is_read_alone(self):
        # The second run is the value; a finding of another rule in the third run has it read alone.
        builds = self.anchored("alone")
        first, third = b"plain made-up words", b"made-up run " + MARK + b" end of the run"
        tool = self.ELF + first + b"\x00\x02" + self.START + b"\x00\x03" + third + b"\x00" * 16
        second = len(self.ELF) + len(first) + 2
        result = self.scan(self.product({"bin/tool": tool}), **builds)
        self.assertEqual((result.code, [(row["rule"], row["path"], row["line"], row["count"]) for row in result.listing["positions"]]),
                         (1, [(RULE, "bin/tool#strings", second + len(self.START) + 2, 1), (self.START_RULE, "bin/tool#strings", second, 1)]))


class DefaultRulesOfEveryPayload(Scans):
    """The rules file given for a build is proved to be the build's own default on every payload."""

    def test_a_rules_file_that_lacks_a_rule_stops_the_scan_wherever_the_hit_is(self):
        lacking = self.root / "lacking-default.toml"
        lacking.write_text(DEFAULT_RULES + '\n[[rules]]\nid = "%s"\n' % RULE)
        clean = self.product({"notes.txt": b"plain\n"})
        marked = self.payload(tar_bytes({"pkg/notes.txt": TAG + b"\n"}), name="other-0.0.0-linux-x86_64.tar.gz")
        # With the build's own default the finding refuses, wherever its payload stands.
        for payloads in ((clean, marked), (marked, clean)):
            result = self.scan(*payloads)
            self.assertEqual((result.code, self.counts(result, "findings", "refused")), (1, (1, 1)))
        # Run with the file that lacks the rule, the build that alone knows the marker reports nothing:
        # the scan stops, also when the only hit is in a later payload.
        for payloads in ((marked,), (marked, clean), (clean, marked)):
            result = self.scan(*payloads, extra=["--default-config-current", lacking])
            self.assertEqual((result.code, result.receipt), (3, None), len(payloads))
            self.assertTrue(result.stderr.startswith("CREDENTIAL-LEG REFUSED tool-failure: "), result.stderr)
            self.assertIn("embedded default", result.stderr)

    def test_each_build_runs_with_its_embedded_default_once_for_every_payload(self):
        pinned, current = write_builds(self.root / "builds")
        payloads = [self.payload(tar_bytes({"pkg/notes.txt": b"plain\n"}), name="part%d-0.0.0-linux-x86_64.tar.gz" % number)
                    for number in range(3)]
        result = self.scan(*payloads, pinned=pinned, current=current)
        self.assertEqual((result.code, result.fields["files"]), (0, "3"))
        for build in (pinned, current):
            scans = [argv for argv in calls(build) if "--report-path" in argv]
            embedded = [argv for argv in scans if "--config" not in argv]
            # One run without a rules file for each directory that is read of each payload: its scan
            # root, and the directory of its bytes outside members (a tar's headers here).
            self.assertEqual(len(embedded), 6)
            roots = {argv[1] if argv[0] == "dir" else argv[3] for argv in embedded}
            self.assertEqual((len(roots), sorted({os.path.basename(root) for root in roots})), (6, ["outside", "scan"]))
        # A payload that is no archive has the one directory.
        pinned, current = write_builds(self.root / "builds-plain")
        self.scan(self.payload(b"plain\n", name="notes-0.0.0.txt"), pinned=pinned, current=current)
        for build in (pinned, current):
            self.assertEqual(len([argv for argv in calls(build) if "--report-path" in argv and "--config" not in argv]), 1)

    ELF = b"\x7fELF\x02\x01\x01" + b"\x00" * 57

    def compiled(self, *runs):
        """A compiled file whose printable runs are runs, each ended by bytes that are no text."""
        return self.ELF + b"".join(run + b"\x00\x02" for run in runs) + b"tail of the made-up runs" + b"\x00" * 16

    def test_a_rules_file_that_lacks_a_rule_stops_the_scan_when_the_hit_is_in_a_compiled_file(self):
        # The builds do not read a compiled file where it lies; its printable runs are read from a
        # directory of their own, with the given rules file. That directory is proved too.
        lacking = self.root / "lacking-default.toml"
        lacking.write_text(DEFAULT_RULES + '\n[[rules]]\nid = "%s"\n' % RULE)
        tool = self.compiled(b"made-up run " + TAG + b" end of the run")
        inside = self.payload(tar_bytes({"pkg/bin/tool": tool}), name="tool-0.0.0-linux-x86_64.tar.gz")
        clean = self.product({"notes.txt": b"plain\n"})
        empty = self.payload(b"", name="empty-0.0.0.txt")
        alone = self.payload(tool, name="tool-0.0.0")
        cases = (("alone in the scan", (inside,), "bin/tool#strings"), ("after a clean payload", (clean, inside), "bin/tool#strings"),
                 ("after an empty payload", (empty, inside), "bin/tool#strings"), ("the compiled file is the payload", (alone,), "tool-0.0.0#strings"))
        for note, payloads, path in cases:
            result = self.scan(*payloads)
            self.assertEqual((result.code, [(row["rule"], row["path"]) for row in result.listing["positions"]]), (1, [(TAG_RULE, path)]), note)
            result = self.scan(*payloads, extra=["--default-config-current", lacking])
            self.assertEqual((result.code, result.receipt), (3, None), note)
            self.assertTrue(result.stderr.startswith("CREDENTIAL-LEG REFUSED tool-failure: "), result.stderr)
            self.assertIn("embedded default", result.stderr)

    def test_a_rules_file_is_proved_on_each_run_that_is_read_alone(self):
        # A run that a finding touches is read once more as a file of its own, with the given rules
        # file; only that reading counts. A rule that reports a run only when it is read alone (here:
        # one that wants the run to end the file) has no hit in the first reading with either rules
        # file, so the readings of the runs alone are proved too.
        end_value = "zqe" + "12ab" + "34cd"
        end_rule = ("zq-fixture-end", "zq_fixture_" + "end" + r":(zqe[0-9a-f]{8})\n\Z")
        current = write_build(self.root / "current-end", "0.0.4", True, RULES + [end_rule])
        lacking = self.root / "lacking-default.toml"
        lacking.write_text(DEFAULT_RULES + '\n[[rules]]\nid = "%s"\n' % RULE)
        first = b"made-up run " + MARK + b" end of the run"
        tool = self.compiled(first, ("made-up run zq_fixture_" + "end:" + end_value).encode())
        product = self.product({"bin/tool": tool})
        values = VALUES + (end_value,)
        result = self.scan(product, current=current, values=values)
        self.assertEqual((result.code, [(row["rule"], row["path"], row["line"]) for row in result.listing["positions"]]),
                         (1, [("zq-fixture-end", "bin/tool#strings", len(self.ELF) + len(first) + 2), (RULE, "bin/tool#strings", len(self.ELF))]))
        accepted = self.accepted(positions=[position("bin/tool#strings", tool, len(self.ELF))])
        result = self.scan(product, current=current, values=values, accepted=accepted)
        self.assertEqual((result.code, self.counts(result, "findings", "accepted_positions", "refused")), (1, (2, 1, 1)))
        # With the rules file that lacks that rule the scan stops; it does not pass on the one accepted position.
        result = self.scan(product, current=current, values=values, accepted=accepted, extra=["--default-config-current", lacking])
        self.assertEqual((result.code, result.receipt), (3, None))
        self.assertIn("embedded default", result.stderr)

    def test_the_runs_of_compiled_files_are_proved_only_when_there_are_some(self):
        # The runs of a build over one payload that is a file: the one that measures how the build
        # reports the proof-of-read line, its two readings (as it is, with that line), the two of the
        # proof, and the first reading of the runs. With a compiled file: the second reading of the
        # runs and the two of the proof over them. With a finding in a run: the two readings of the
        # touched runs alone, and the two of the proof over them.
        marked = self.compiled(b"made-up run " + MARK + b" end of the run")
        cases = ((b"plain\n", 6), (self.compiled(b"made-up run without a marker"), 9), (marked, 13))
        for number, (data, runs) in enumerate(cases):
            pinned, current = write_builds(self.root / ("builds-%d" % number))
            self.scan(self.payload(data, name="tool-0.0.0"), pinned=pinned, current=current)
            for build in (pinned, current):
                self.assertEqual(len([argv for argv in calls(build) if "--report-path" in argv]), runs, (runs, build.parent.name))
        # In an archive: four more for the bytes outside its members (their two readings and the proof).
        pinned, current = write_builds(self.root / "builds-archive")
        self.scan(self.product({"bin/tool": marked}), pinned=pinned, current=current)
        for build in (pinned, current):
            self.assertEqual(len([argv for argv in calls(build) if "--report-path" in argv]), 17, build.parent.name)

    def test_a_rules_file_is_proved_over_the_bytes_outside_members(self):
        lacking = self.root / "lacking-default.toml"
        lacking.write_text(DEFAULT_RULES + '\n[[rules]]\nid = "%s"\n' % RULE)
        archive = zip_bytes({"pkg/notes.txt": b"plain\n"}) + b"\n" + TAG + b"\n"
        for product, path in ((self.payload(archive, name="product-0.0.0.zip"), OUTSIDE + "after-end"),
                              (self.product({"lib/inner.zip": archive}), "lib/inner.zip!" + OUTSIDE + "after-end")):
            result = self.scan(product)
            self.assertEqual((result.code, [(row["rule"], row["path"]) for row in result.listing["positions"]]), (1, [(TAG_RULE, path)]))
            result = self.scan(product, extra=["--default-config-current", lacking])
            self.assertEqual((result.code, result.receipt), (3, None), path)
            self.assertIn("embedded default", result.stderr)

    def test_a_rules_file_that_lacks_a_rule_stops_the_scan_when_the_hit_is_at_a_path_of_a_removed_allowlist_line(self):
        # The scan removes the path allowlist lines for vendor, dist-info and lock file paths from the
        # rules it scans with, so it reads those paths. A build's own default skips them, with the
        # given rules file and with the embedded default alike: two runs that both pass over a file
        # say nothing about the rules for it. The rules file is proved over those files too.
        lacking = self.root / "lacking-default.toml"
        lacking.write_text(DEFAULT_RULES + '\n[[rules]]\nid = "%s"\n' % RULE)
        clean = self.product({"notes.txt": b"plain\n"})
        later = self.payload(tar_bytes({"pkg/vendor/notes.txt": TAG + b"\n"}), name="other-0.0.0-linux-x86_64.tar.gz")
        cases = (("a vendor directory", (self.product({"vendor/notes.txt": TAG + b"\n"}),), "vendor/notes.txt"),
                 ("a dist-info directory", (self.product({"thing-1.0.dist-info/notes.txt": TAG + b"\n"}),), "thing-1.0.dist-info/notes.txt"),
                 ("a lock file", (self.product({"package-lock.json": TAG + b"\n"}),), "package-lock.json"),
                 ("a vendor directory of a second payload", (clean, later), "vendor/notes.txt"))
        for note, payloads, path in cases:
            with self.subTest(note):
                # With the build's own default the scan reads the path: the finding refuses there.
                result = self.scan(*payloads)
                self.assertEqual((result.code, [(row["rule"], row["path"]) for row in result.listing["positions"]]), (1, [(TAG_RULE, path)]))
                # With the file that lacks the rule the scan stops: the only hit lies at that path.
                result = self.scan(*payloads, extra=["--default-config-current", lacking])
                self.assertEqual((result.code, result.receipt is None), (3, True))
                self.assertTrue(result.stderr.startswith("CREDENTIAL-LEG REFUSED tool-failure: "), result.stderr)
                self.assertIn("embedded default", result.stderr)


class MemberNames(Scans):
    """A printed path names one file: "!" stands only where an archive is entered and "#strings" only
    after the path of a compiled file. A member whose name would print like another path is not written
    out, and the payload refuses."""

    def test_a_member_name_with_the_archive_separator_refuses(self):
        inside, beside = b"inside\n" + MARK + b"\n", b"beside\n" + MARK + b"\n"
        inner = tar_bytes({"evidence/notes.txt": inside}, compressed=False)
        product = self.product({"inner.tar": inner, "inner.tar!evidence/notes.txt": beside})
        path = "inner.tar!evidence/notes.txt"
        result = self.scan(product)
        self.assertEqual((result.code, self.counts(result, "unread", "unread_unaccepted")), (1, (1, 1)))
        # The one file that prints as that path is the member of the archive. The directory's file is
        # not written out; its bytes are read with the payload's other bytes that are no written member's.
        self.assertEqual([row for row in result.listing["positions"] if row["path"] == path],
                         [{"payload": product.name, "rule": RULE, "path": path, "sha256": sha256(inside), "line": 2, "count": 1,
                           "pending": False}])
        self.assertEqual([row["path"] for row in result.listing["positions"]], [OUTSIDE + "tar-headers-and-gaps", path])
        self.assertEqual(result.listing["members_not_written"], [
            {"payload": product.name, "archive": product.name, "sha256": sha256(product.read_bytes()), "reason": "separator-in-name",
             "count": 1}])
        # No acceptance of a position or of a file at that path passes the payload.
        for data in (inside, beside):
            for count in (1, 2):
                accepted = self.accepted(positions=[position(path, data, 2, count=count)])
                self.assertEqual(self.scan(product, accepted=accepted).code, 1, (data, count))
            self.assertEqual(self.scan(product, accepted=self.accepted(files=[file_entry(path, data)])).code, 1, data)
        self.assertEqual(self.scan(product, accepted=self.accepted(archives=[archive_entry("inner.tar", inner)])).code, 1)
        # A directory's name and a name at a deeper level refuse in the same way, under their archive.
        pack = zip_bytes({"a!b/": b"", "a!b/notes.txt": b"plain\n", "c/d!e.txt": b"plain\n", "c/plain.txt": b"plain\n"})
        deep = self.product({"lib/pack.zip": pack})
        result = self.scan(deep)
        self.assertEqual((result.code, self.counts(result, "findings", "refused", "unread", "unread_unaccepted")), (1, (0, 0, 3, 3)))
        self.assertEqual(result.listing["members_not_written"], [
            {"payload": deep.name, "archive": "lib/pack.zip", "sha256": sha256(pack), "reason": "separator-in-name", "count": 3}])
        # Without such a name the same members pass.
        result = self.scan(self.product({"lib/pack.zip": zip_bytes({"ab/": b"", "ab/notes.txt": b"plain\n", "c/de.txt": b"plain\n"})}))
        self.assertEqual((result.code, self.counts(result, "findings", "refused", "unread")), (0, (0, 0, 0)))

    def test_no_member_stands_where_the_bytes_outside_members_are_written(self):
        marked = MARK + b"\n"
        members = {OUTSIDE + "after-end": marked, OUTSIDE + "headers-and-gaps": marked, "notes.txt": b"plain\n"}
        archives = (("lib/pack.zip", zip_bytes(members)), ("lib/pack.tar", tar_bytes(members, compressed=False)),
                    ("lib/pack.tar.gz", tar_bytes(members)))
        for name, archive in archives:
            for product in (self.product({name: archive}), self.product({"deep/outer.tar": tar_bytes({name: archive}, compressed=False)})):
                result = self.scan(product)
                self.assertEqual(result.code, 1, name)
                self.assertEqual([(row["reason"], row["count"], row["sha256"]) for row in result.listing["members_not_written"]],
                                 [("separator-in-name", 2, sha256(archive))], name)
                # No file of the tree is such a member: what is reported there is the archive's own.
                for row in result.listing["positions"] + result.listing["unread"]:
                    self.assertNotEqual(row["sha256"], sha256(marked), name)
        whole = self.payload(archives[0][1], name="product-0.0.0.zip")
        result = self.scan(whole)
        self.assertEqual((result.code, self.counts(result, "unread", "unread_unaccepted")), (1, (2, 2)))
        self.assertEqual(result.listing["members_not_written"], [
            {"payload": whole.name, "archive": whole.name, "sha256": sha256(whole.read_bytes()), "reason": "separator-in-name", "count": 2}])

    def test_a_member_name_that_ends_like_the_runs_of_a_compiled_file_refuses(self):
        head = b"\x7fELF\x02\x01\x01" + b"\x00" * 57
        tool = head + b"made-up run " + MARK + b" tail of the run" + b"\x00" * 16
        # A text member named like the runs of the compiled file, with the marker on the line whose
        # number is the offset of the run.
        text = b"\n" * (len(head) - 1) + MARK + b"\n"
        product = self.product({"bin/tool": tool, "bin/tool#strings": text})
        result = self.scan(product)
        self.assertEqual((result.code, self.counts(result, "unread", "unread_unaccepted")), (1, (1, 1)))
        self.assertEqual([row for row in result.listing["positions"] if row["path"] == "bin/tool#strings"],
                         [{"payload": product.name, "rule": RULE, "path": "bin/tool#strings", "sha256": sha256(tool), "line": len(head),
                           "count": 1, "pending": False}])
        self.assertEqual([row["path"] for row in result.listing["positions"]], [OUTSIDE + "tar-headers-and-gaps", "bin/tool#strings"])
        self.assertEqual(result.listing["members_not_written"], [
            {"payload": product.name, "archive": product.name, "sha256": sha256(product.read_bytes()), "reason": "strings-mark-in-name",
             "count": 1}])
        for data in (tool, text):
            for count in (1, 2):
                accepted = self.accepted(positions=[position("bin/tool#strings", data, len(head), count=count)])
                self.assertEqual(self.scan(product, accepted=accepted).code, 1, count)
        # The mark inside a name, or as the name of a directory, prints like no other path.
        pack = zip_bytes({"dir#strings/": b"", "dir#strings/notes.txt": b"plain\n"})
        result = self.scan(self.product({"bin/tool#strings.txt": b"plain\n", "bin/dir#strings/notes.txt": b"plain\n", "lib/pack.zip": pack}))
        self.assertEqual((result.code, self.counts(result, "findings", "refused", "unread")), (0, (0, 0, 0)))

    def test_the_one_member_of_a_compressed_file_is_named_through_the_same_check(self):
        # The member of a compressed file is named after the file. That name goes through the one
        # function too: a name that ends with the mark of a compiled file's runs is not written out.
        packed = gzip.compress(b"first line\n" + MARK + b"\n", mtime=0)
        product = self.product({"bin/tool#strings.gz": packed, "bin/plain.txt.gz": gzip.compress(b"plain\n", mtime=0)})
        result = self.scan(product)
        self.assertEqual((result.code, self.counts(result, "findings", "unread", "unread_unaccepted")), (1, (0, 1, 1)))
        self.assertEqual(result.listing["members_not_written"], [
            {"payload": product.name, "archive": "bin/tool#strings.gz", "sha256": sha256(packed), "reason": "strings-mark-in-name", "count": 1}])
        self.assertEqual(result.report["payload"][0]["member_skips"], {"gzip-single:strings-mark-in-name": 1})


class Extractor(unittest.TestCase):
    """The parts of the extractor that the scans above reach only in part."""

    def setUp(self):
        spec = importlib.util.spec_from_file_location("release_payload_deep_extract_rows", EXTRACTOR)
        self.deep = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(self.deep)
        self.root = Path(tempfile.mkdtemp(prefix="deep-extract-test-")).resolve()
        self.addCleanup(shutil.rmtree, self.root, True)

    def test_one_function_decides_whether_a_name_prints_as_a_path_of_its_own(self):
        deep = self.deep
        self.assertEqual((deep.SEPARATOR, deep.STRINGS_MARK, deep.OUTSIDE), ("!", "#strings", OUTSIDE.rstrip("/")))
        cases = ((["a", "b.txt"], False, None), (["a"], True, None),
                 (["a!b", "c.txt"], False, "separator-in-name"), (["a", "b!c.txt"], False, "separator-in-name"),
                 (["a", "b", "!"], False, "separator-in-name"), (["a!b"], True, "separator-in-name"),
                 (["a", "b!"], True, "separator-in-name"), (["!outside", "after-end"], False, "separator-in-name"),
                 (["a", "tool#strings"], False, "strings-mark-in-name"), (["#strings"], False, "strings-mark-in-name"),
                 (["a", "tool#strings"], True, None), (["a#strings", "tool"], False, None), (["a", "tool#strings.txt"], False, None),
                 (["a", "tool#string"], False, None))
        for parts, directory, reason in cases:
            self.assertEqual(deep.printed_parts(parts, directory), (parts, None) if reason is None else (None, reason), (parts, directory))
        # Every member name of a zip and a tar goes through it.
        self.assertEqual(deep.safe_parts("a/./b!c"), (None, "separator-in-name"))
        self.assertEqual(deep.safe_parts("a/tool#strings"), (None, "strings-mark-in-name"))
        self.assertEqual(deep.safe_parts("a/tool#strings/", True), (["a", "tool#strings"], None))
        self.assertEqual(deep.safe_parts("a/b.txt"), (["a", "b.txt"], None))

    def test_runs_join_no_two_ranges_and_a_long_run_comes_in_pieces_that_overlap(self):
        deep = self.deep
        self.assertEqual(list(deep.gaps([(8, 20), (5, 10), (30, 40)], 0, 50)), [(0, 5), (20, 30), (40, 50)])
        self.assertEqual(list(deep.gaps([(0, 50)], 0, 50)), [])
        self.assertEqual(list(deep.gaps([], 3, 9)), [(3, 9)])
        path = self.root / "bytes"
        path.write_bytes(b"\x00" + b"A" * 6 + b"B" * 6 + b"\x01" + b"C" * 8 + b"\x02" + b"D" * 7 + b"\x03")
        with open(path, "rb") as source:
            self.assertEqual(list(deep.runs_of(source, [(0, 30)])), [b"A" * 6 + b"B" * 6, b"C" * 8])
            # Two ranges that touch are still two: six and six bytes are no run of eight.
            self.assertEqual(list(deep.runs_of(source, [(0, 7), (7, 14)])), [])
            self.assertEqual(list(deep.runs_of(source, [(0, 9), (9, 30)])), [b"A" * 6 + b"B" * 2, b"C" * 8])
        # A tab is part of a run; no other byte below a blank is.
        path.write_bytes(b"\x00abc\tdefgh\x00abc\ndefgh\x00abc\rdefgh\x00\t\t\t\t\t\t\t\t\x00")
        with open(path, "rb") as source:
            self.assertEqual(list(deep.runs_of(source, [(0, path.stat().st_size)])), [b"abc\tdefgh", b"\t" * 8])
        # A run longer than a block: every stretch of it up to the overlap stands whole in a piece, and
        # every piece holds bytes the piece before it did not, also when the run ends with a block or
        # with the range.
        deep.CHUNK, deep.RUN_OVERLAP = 64, 16
        whole = bytes(0x21 + byte % 90 for number in range(32) for byte in hashlib.sha256(b"%d" % number).digest())
        more = b"\x00a second run\x00tail of the file"
        for run, after in ((whole, more), (whole[:-1], more), (whole[:-1], b""), (whole, b"")):
            path.write_bytes(b"\x00" + run + after)
            with open(path, "rb") as source:
                pieces = list(deep.runs_of(source, [(0, path.stat().st_size)]))
            if after:
                self.assertEqual(pieces[-2:], [b"a second run", b"tail of the file"])
                del pieces[-2:]
            self.assertGreater(len(pieces), 3)
            for start in range(len(run) - 15):
                self.assertTrue(any(run[start:start + 16] in piece for piece in pieces), start)
            for one, two in zip(pieces, pieces[1:]):
                self.assertNotIn(two, one)
            self.assertEqual(run.count(pieces[-1]), 1)
            self.assertTrue(run.endswith(pieces[-1]))

    def test_a_layout_that_is_not_known_writes_nothing(self):
        deep = self.deep
        deep.ST = deep.Stats(str(self.root))
        path = self.root / "archive"
        path.write_bytes(b"made-up bytes, " * 8)
        size = path.stat().st_size
        for claimed, start, end in (([(10, size + 1)], 0, size), ([(0, 10)], 5, size), ([(20, 10)], 0, size), ([], 0, size + 1),
                                    ([], 60, 50), ([(10, 60)], 0, 50)):
            with self.assertRaises(ValueError, msg=(claimed, start, end)):
                deep.write_outside(str(self.root), str(path), claimed, start, end)
        self.assertEqual(sorted(entry.name for entry in self.root.iterdir()), ["archive"])
        # A known layout writes the three files, and counts their bytes.
        deep.write_outside(str(self.root), str(path), [(40, 60)], 20, 100)
        outside = self.root / OUTSIDE.rstrip("/")
        self.assertEqual(sorted(entry.name for entry in outside.iterdir()), ["after-end", "before-start", "headers-and-gaps"])
        data = path.read_bytes()
        self.assertEqual(((outside / "before-start").read_bytes(), (outside / "after-end").read_bytes()), (data[:20], data[100:]))
        self.assertEqual((outside / "headers-and-gaps").read_bytes().split(b"\n")[1:], [data[20:40], data[60:100], b""])
        self.assertEqual(deep.ST.total_bytes, sum(entry.stat().st_size for entry in outside.iterdir()) - len(deep.RUNS_HEAD))

    def test_the_end_of_a_zstd_frame_is_found_without_a_decoder(self):
        deep = self.deep
        words = b"".join(hashlib.sha256(b"%d" % number).digest() for number in range(12000))
        contents = (b"", b"plain\n", b"plain words, " * 40000, words, words + b"plain words, " * 40000, b"a" * 400000,
                    words + b"\x00" * 300000 + words)
        frames = [zstd_compress(content) for content in contents]
        if frames[0] is None:
            self.skipTest("no zstd encoder on this host")
        path = self.root / "frames"
        for number, frame in enumerate(frames):
            for before, after in ((b"", b""), (b"", b"bytes after the frame"), (frame, frame)):
                path.write_bytes(before + frame + after)
                with open(path, "rb") as source:
                    self.assertEqual(deep.zstd_frame_end(source, len(before), path.stat().st_size), len(before) + len(frame), number)
            path.write_bytes(frame[:-1])
            with open(path, "rb") as source, self.assertRaises(ValueError):
                deep.zstd_frame_end(source, 0, path.stat().st_size)

    def test_the_member_cap_stops_a_stream_while_it_is_decoded(self):
        deep = self.deep
        deep.CHUNK, deep.MAX_MEMBER_BYTES = 1024, 4096
        content = b"plain words, " * 40000
        packers = {"notes.txt.gz": lambda data: gzip.compress(data, mtime=0), "notes.txt.bz2": bz2.compress, "notes.txt.xz": lzma.compress}
        if zstd_compress(b"") is not None:
            packers["notes.txt.zst"] = zstd_compress
        for number, (name, pack) in enumerate(packers.items()):
            source = self.root / name
            source.write_bytes(pack(content))
            tree = self.root / ("tree-%d" % number)
            stats = deep.expand_file(str(source), str(tree))
            self.assertEqual(([row["path"] for row in stats["not_opened"]], stats["archives"]), ([name], []), name)
            # The stream was stopped when it passed the cap, not after it was decoded in full.
            self.assertLess(deep.ST.total_bytes, 4096 + 4 * 1024, name)
            self.assertEqual(sorted(entry.name for entry in tree.iterdir()), [name], name)
        # A tar inside a compressed file is no member: it may be longer than the cap, and its members
        # are held to the cap one by one.
        for number, (size, opened) in enumerate(((3000, True), (5000, False))):
            source = self.root / ("pack-%d.tar.gz" % number)
            source.write_bytes(tar_bytes({"pkg/%d.txt" % member: b"plain words, " * (size // 13) for member in range(4)}))
            stats = deep.expand_file(str(source), str(self.root / ("tar-tree-%d" % number)))
            self.assertEqual(([row["members"] for row in stats["archives"]], len(stats["not_opened"])), (([4], 0) if opened else ([], 1)), size)


if __name__ == "__main__":
    unittest.main()
