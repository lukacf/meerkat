"""Release payload credential scan: two detector builds through every nested archive, fail closed.

Synthetic values only. The detector builds are not assumed to exist: the tool's own logic runs
against a stand-in the tests write into a temporary directory. It takes the same command line and
writes a report of the same shape, for made-up markers in the scanned tree (two builds: an older
one with the `detect` command and a current one with `dir`, which also knows a second marker).
The markers are put together from pieces and are not a credential of anything. One class runs the
two real builds when the environment names them, and is skipped otherwise.
"""
from pathlib import Path
import calendar
import gzip
import hashlib
import importlib.util
import io
import json
import os
import shutil
import subprocess
import sys
import tarfile
import tempfile
import time
import types
import unittest
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
a binary signature. Its report carries the matched value in "Secret" on purpose."""
import json
import os
import re
import sys
import tomllib

VERSION = @VERSION@
DIR_MODE = @DIR_MODE@
RULES = @RULES@
EMBEDDED = @EMBEDDED@
HERE = os.path.dirname(os.path.abspath(__file__))
BINARY = [(0, b"\x7fELF"), (0, b"PK\x03\x04"), (0, b"\x1f\x8b"), (257, b"ustar"), (0, b"%PDF"), (0, b"\x28\xb5\x2f\xfd")]


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
    findings = []
    for dirpath, dirnames, filenames in os.walk(source):
        dirnames.sort()
        for name in sorted(filenames):
            path = os.path.join(dirpath, name)
            if any(rx.search(path) for rx in allowed):
                continue
            with open(path, "rb") as stream:
                data = stream.read()
            if any(data[at:at + len(magic)] == magic for at, magic in BINARY):
                continue
            text = data.decode("latin-1")
            for rule, pattern in RULES:
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
    with open(options["--report-path"], "w", encoding="utf-8") as stream:
        json.dump(findings, stream)
    return int(options.get("--exit-code", "1")) if findings else 0


sys.exit(main())
'''


def write_build(directory, version, dir_mode, rules, default_rules=DEFAULT_RULES):
    """A stand-in build with its default rules file beside it. Returns the build's path."""
    directory.mkdir(parents=True)
    build = directory / "gitleaks"
    text = STAND_IN
    for key, value in (("@PYTHON@", sys.executable), ("@VERSION@", repr(version)), ("@DIR_MODE@", repr(dir_mode)),
                       ("@RULES@", repr(rules)), ("@EMBEDDED@", repr(default_rules))):
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

    def scan(self, *payload, accepted=None, pending=None, name="T", pinned=None, current=None, extra=(), values=VALUES):
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
        done = run(TOOL, *args, *payload)
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
        self.assertEqual((result.code, self.counts(result, "findings", "refused", "unread", "unread_unaccepted")), (1, (0, 0, 1, 1)))
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


if __name__ == "__main__":
    unittest.main()
