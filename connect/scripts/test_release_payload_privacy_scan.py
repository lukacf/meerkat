"""Release payload privacy scan: the pinned engine through every nested container.

Synthetic rules only: the marker below is made up, and no real rule, name or release byte is used.
The candidate rows reuse the inert assembly fixtures; no native build or acceptance runs.
"""
from pathlib import Path
import bz2
import copy
import gzip
import hashlib
import importlib.util
import io
import json
import lzma
import os
import shutil
import subprocess
import sys
import tarfile
import tempfile
import unittest
import zipfile
import zlib
from unittest import mock

HERE = Path(__file__).resolve().parent
WRAPPER = HERE / "release-payload-privacy-scan.py"
ENGINE = HERE / "release-evidence-privacy-scan.py"
CANDIDATE = HERE / "connect-candidate.py"
FIXTURES = HERE / "fixtures/release-payload-privacy-scan"
RULES = FIXTURES / "synthetic-rules.json"
EXEMPTIONS = FIXTURES / "synthetic-exemptions.json"
MARKER = b"zq-synthetic-marker-7f3a"
TARGET = "macos-arm64"


def load(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    value = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(value)
    return value


def tar_bytes(members, compressed=True):
    raw = io.BytesIO()
    with tarfile.open(fileobj=raw, mode="w", format=tarfile.PAX_FORMAT) as archive:
        for name, data in members.items():
            info = tarfile.TarInfo(name)
            info.size = len(data)
            archive.addfile(info, io.BytesIO(data))
    return gzip.compress(raw.getvalue(), mtime=0) if compressed else raw.getvalue()


def wheel_bytes(members):
    raw = io.BytesIO()
    with zipfile.ZipFile(raw, "w", zipfile.ZIP_DEFLATED) as archive:
        for name, data in members.items():
            archive.writestr(zipfile.ZipInfo(name, (1980, 1, 1, 0, 0, 0)), data)
    return raw.getvalue()


def library_bytes(members):
    """A library archive as the product embeds it: library/<member> inside a .tar.gz."""
    return tar_bytes({"library/" + name: data for name, data in members.items()})


def product_bytes(members):
    """A product archive that embeds the library archive among its resources."""
    return tar_bytes({"pkg/bin/tool": b"inert tool\n", "pkg/libexec/archives/library.tar.gz": library_bytes(members)})


def source_archive(text):
    return tar_bytes({"source/notes.txt": text})


def gzip_stream(data, name=None, comment=None, extra=None):
    """One gzip stream of data with the given header fields (RFC 1952)."""
    flags = (4 if extra is not None else 0) | (8 if name is not None else 0) | (16 if comment is not None else 0)
    header = b"\x1f\x8b\x08" + bytes([flags]) + b"\x00\x00\x00\x00\x00\x03"
    if extra is not None:
        header += len(extra).to_bytes(2, "little") + extra
    for field in (name, comment):
        if field is not None:
            header += field + b"\x00"
    deflate = zlib.compressobj(9, zlib.DEFLATED, -zlib.MAX_WBITS)
    body = deflate.compress(data) + deflate.flush()
    return header + body + zlib.crc32(data).to_bytes(4, "little") + (len(data) & 0xFFFFFFFF).to_bytes(4, "little")


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


def run(script, *args):
    return subprocess.run([sys.executable, "-I", str(script), *map(str, args)], capture_output=True, text=True)


def dump(path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value) + "\n")


class PayloadScan(unittest.TestCase):
    def setUp(self):
        self.root = Path(tempfile.mkdtemp(prefix="payload-scan-test-")).resolve()
        self.addCleanup(shutil.rmtree, self.root, True)
        self.count = 0

    def payload(self, data, name="meerkat-connect-0.0.0-linux-x86_64.tar.gz"):
        self.count += 1
        directory = self.root / f"payload-{self.count}"
        directory.mkdir()
        path = directory / name
        path.write_bytes(data)
        return path

    def accepted(self, entries, by="test"):
        """An accepted-findings file; each entry is accepted by `by` unless it says otherwise (None: no key)."""
        self.count += 1
        path = self.root / f"accepted-{self.count}.json"
        if by is not None:
            entries = [{"accepted_by": by, **entry} for entry in entries]
        dump(path, {"schema": "release-payload-privacy-scan/accepted/2", "accepted": entries})
        return path

    def non_published(self, members=(), snapshots=(), **changes):
        """A private list of members a library leaves out, as the library packager reads it: only the
        digests matter to the scan."""
        self.count += 1
        path = self.root / f"non-published-{self.count}.json"
        dump(path, {"schema": "package-library-release/non-published-source-members/1",
                    "trees": {"selected/provenance/source/": [
                        {"name": f"crates/sample/withheld-{n}.txt", "sha256": value} for n, value in enumerate(members)]},
                    "original_snapshots": {"selected/provenance/demo/source.tar.gz": sorted(snapshots)} if snapshots else {},
                    **changes})
        return path

    def scan(self, *payload, accepted=None, rules=RULES, exemptions=EXEMPTIONS, wrapper=WRAPPER, release=False,
             extra=()):
        """One scan. Not a release scan unless release is set: the tool under test is rarely a clean commit."""
        self.count += 1
        report, detail = self.root / f"report-{self.count}.json", self.root / f"detail-{self.count}.json"
        args = ["--report", report, "--detail", detail, "--work", self.root, *extra]
        if not release:
            args += ["--proof-only"]
        elif "--non-published-list" not in extra:
            args += ["--non-published-list", self.non_published()]
        if rules is not None:
            args += ["--rules", rules]
        if exemptions is not None:
            args += ["--exemptions", exemptions]
        if accepted is not None:
            args += ["--accepted-findings", accepted]
        done = run(wrapper, *args, *payload)
        self.last = done
        value = json.loads(report.read_text()) if report.exists() else None
        if value is not None:
            self.assertNotIn(MARKER.decode(), report.read_text())
            self.assertEqual(detail.stat().st_mode & 0o777, 0o600)
            self.assertIn(value["status"], ("PASS", "REFUSED"))
            self.assertEqual(value["status"] == "PASS", done.returncode == 0)
            self.assertEqual(value["reasons"] == [], value["status"] == "PASS")
        return done.returncode, value

    def rules_file(self, categories, **extra):
        self.count += 1
        path = self.root / f"rules-{self.count}.json"
        dump(path, {"schema": "release-evidence-privacy-scan/rules/1", "categories": categories, **extra})
        return path

    def exemptions_file(self, entries):
        self.count += 1
        path = self.root / f"exemptions-{self.count}.json"
        dump(path, {"schema": "release-evidence-privacy-scan/exemptions/1", "exemptions": entries})
        return path

    def tool_copy(self, name, repository=True, bound=None):
        """The wrapper and the engine in a directory of their own, committed to a repository there. With
        bound, the copy's engine reads no member or stream larger than that and the copy pins that engine."""
        directory = self.root / name / "scripts"
        directory.mkdir(parents=True)
        for path in (WRAPPER, ENGINE):
            shutil.copyfile(path, directory / path.name)
        if bound is not None:
            engine = ENGINE.read_text()
            self.assertEqual(engine.count("MAX_MEMBER_BYTES = 1 << 30\n"), 1)
            lowered = engine.replace("MAX_MEMBER_BYTES = 1 << 30\n", f"MAX_MEMBER_BYTES = {bound}\n").encode()
            (directory / ENGINE.name).write_bytes(lowered)
            wrapper = WRAPPER.read_text()
            pin = hashlib.sha256(ENGINE.read_bytes()).hexdigest()
            self.assertEqual(wrapper.count(pin), 1)
            (directory / WRAPPER.name).write_text(wrapper.replace(pin, hashlib.sha256(lowered).hexdigest()))
        if repository:
            for command in (["init", "-q"], ["add", "."],
                            ["-c", "user.name=test", "-c", "user.email=test@example.invalid", "commit", "-q", "-m", "tools"]):
                subprocess.run(["git", "-C", str(directory.parent), *command], check=True, capture_output=True)
        return directory / WRAPPER.name

    def test_the_report_names_the_time_the_tool_the_rules_and_the_scanned_bytes(self):
        data = product_bytes({"selected/notes.txt": b"public notes\n"})
        payload = self.payload(data)
        code, report = self.scan(payload)
        self.assertEqual((code, report["status"], report["reasons"]), (0, "PASS", []))
        self.assertEqual(report["schema"], "release-payload-privacy-scan/report/3")
        self.assertRegex(report["generated_at"], r"\A20[0-9]{2}-[01][0-9]-[0-3][0-9]T[0-2][0-9]:[0-5][0-9]:[0-6][0-9]Z\Z")
        self.assertEqual(report["tool_sha256"], hashlib.sha256(WRAPPER.read_bytes()).hexdigest())
        self.assertEqual(report["rules"], {"sha256": hashlib.sha256(RULES.read_bytes()).hexdigest(),
                                           "total": 2, "blocking": 2, "review_outside_paths": 0})
        self.assertIsNone(report["name_list_sha256"])
        self.assertEqual(report["exemptions"], {"sha256": hashlib.sha256(EXEMPTIONS.read_bytes()).hexdigest(),
                                                "count": 0, "blocking": 0})
        self.assertEqual(report["accepted"], {"sha256": None, "count": 0, "used": {"member": 0, "archive": 0},
                                              "used_unproved": 0, "used_by_tool": {}})
        self.assertEqual(report["coverage_exceptions"], {"sha256": None, "count": 0, "used": {}})
        self.assertIsNone(report["non_published_list_sha256"])
        self.assertEqual(report["payload"], [{"name": payload.name, "bytes": len(data),
                                              "sha256": hashlib.sha256(data).hexdigest()}])
        accepted = self.accepted([{"category": "synthetic_marker", "rule": "synthetic-marker-1", "sha256": "0" * 64,
                                   "reason": "synthetic"}])
        names = hashlib.sha256(b"synthetic name list").hexdigest()
        rules = self.rules_file(json.loads(RULES.read_text())["categories"], name_list_sha256=names)
        code, report = self.scan(payload, accepted=accepted, rules=rules)
        self.assertEqual((code, report["name_list_sha256"]), (0, names))
        self.assertEqual(report["accepted"]["sha256"], hashlib.sha256(accepted.read_bytes()).hexdigest())
        self.assertEqual(report["accepted"]["count"], 1)
        code, report = self.scan(payload, rules=self.rules_file(json.loads(RULES.read_text())["categories"],
                                                                name_list_sha256="abc"))
        self.assertEqual((code, report), (4, None))

    def test_two_reports_of_one_scan_have_other_digests(self):
        # A receipt carries the sha256 of the full report. The report holds a random value, so that
        # digest tells nothing about the report's other fields to one who can guess them.
        payload = self.payload(product_bytes({"selected/notes.txt": b"public notes\n"}))
        reports = []
        for _ in range(3):
            code, report = self.scan(payload)
            self.assertEqual((code, report["status"]), (0, "PASS"))
            self.assertRegex(report["nonce"], r"\A[0-9a-f]{32}\Z")
            reports.append(report)
        self.assertEqual(len({report["nonce"] for report in reports}), 3)
        # Nothing else differs between the runs but the time.
        rest = [{key: value for key, value in report.items() if key not in ("nonce", "generated_at")} for report in reports]
        self.assertEqual(rest[0], rest[1])
        self.assertEqual(rest[0], rest[2])
        # A refused report has one too.
        code, report = self.scan(self.payload(product_bytes({"selected/notes.txt": b"notes " + MARKER + b"\n"})))
        self.assertEqual(report["status"], "REFUSED")
        self.assertRegex(report["nonce"], r"\A[0-9a-f]{32}\Z")

    def test_the_status_is_pass_or_refused_and_the_old_words_are_not_emitted(self):
        for text, status in ((b"public notes\n", "PASS"), (b"notes " + MARKER + b"\n", "REFUSED")):
            with self.subTest(status=status):
                code, report = self.scan(self.payload(product_bytes({"selected/notes.txt": text})),
                                         extra=("--refused", self.root / f"words-{status}.json"))
                self.assertEqual(report["status"], status)
                emitted = json.dumps(report) + self.last.stdout + self.last.stderr \
                    + (self.root / f"words-{status}.json").read_text()
                self.assertIn(f": {status}", self.last.stderr)
                # the earlier lower-case status words: never the status, and one of them nowhere at all
                self.assertNotIn(": refused", self.last.stderr)
                self.assertNotIn("clean", emitted.lower())
        self.assertNotRegex(WRAPPER.read_text(), r"""["']clean["']""")

    def test_a_digest_of_a_member_a_library_leaves_out_refuses_at_any_depth(self):
        member, snapshot = hashlib.sha256(b"synthetic withheld member").hexdigest(), \
            hashlib.sha256(b"synthetic original snapshot").hexdigest()
        listed = self.non_published([member], [snapshot])
        wheel = wheel_bytes({"demo/RECORD.json": json.dumps({"files": {"a": member.upper()}}).encode()})
        nested = {"consumers/demo-0.1-py3-none-any.whl": wheel,
                  "selected/provenance/demo/source.json": ("archive " + snapshot + "\n").encode(),
                  "selected/notes.txt": b"public notes\n"}
        payload = self.payload(product_bytes(nested))
        code, report = self.scan(payload, extra=("--non-published-list", listed))
        self.assertEqual((code, report["status"]), (1, "REFUSED"))
        self.assertEqual(report["hits"]["refused"], {"withheld_member_reference": 2})
        self.assertEqual(report["reasons"], ["hits-refused", "withheld-member-reference"])
        self.assertEqual(report["non_published_list_sha256"], hashlib.sha256(listed.read_bytes()).hexdigest())
        # the rule counts are those of the given rules; the report names no digest and the output no member
        self.assertEqual(report["rules"]["total"], 2)
        emitted = json.dumps(report) + self.last.stdout + self.last.stderr
        for text in (member, snapshot, "RECORD.json", "source.json"):
            self.assertNotIn(text, emitted.lower() if len(text) == 64 else emitted)
        # the members that hold a reference are named on request only
        code, report = self.scan(payload, extra=("--non-published-list", listed, "--show-names"))
        named = [line for line in self.last.stderr.splitlines() if line.startswith("withheld-member-reference\t")]
        self.assertEqual(len(named), 2)
        self.assertTrue(any(line.endswith("demo/RECORD.json") for line in named))
        self.assertTrue(any(line.endswith("selected/provenance/demo/source.json") for line in named))
        self.assertNotIn(member, self.last.stderr.lower())
        # such a hit cannot be accepted, and the same payload without a reference passes
        entry = {"category": "withheld_member_reference", "rule": "withheld-member-reference", "sha256": "0" * 64,
                 "reason": "synthetic"}
        self.assertEqual(self.scan(payload, accepted=self.accepted([entry]), extra=("--non-published-list", listed)),
                         (4, None))
        code, report = self.scan(self.payload(product_bytes({"selected/notes.txt": b"public notes\n"})),
                                 extra=("--non-published-list", listed))
        self.assertEqual((code, report["status"]), (0, "PASS"))
        # a list without a member is a list: nothing is looked for
        code, report = self.scan(payload, extra=("--non-published-list", self.non_published()))
        self.assertEqual((code, report["status"]), (0, "PASS"))

    def test_a_release_scan_needs_the_list_of_members_left_out(self):
        payload = self.payload(product_bytes({"selected/notes.txt": b"public notes\n"}))
        tool = self.tool_copy("release-list")
        self.count += 1
        report = self.root / f"report-{self.count}.json"
        done = run(tool, "--rules", RULES, "--exemptions", EXEMPTIONS, "--report", report,
                   "--detail", self.root / f"detail-{self.count}.json", "--work", self.root, payload)
        self.assertEqual((done.returncode, report.exists()), (4, False))
        self.assertIn("--non-published-list", done.stderr)
        code, value = self.scan(payload, wrapper=tool, release=True)
        self.assertEqual((code, value["status"], value["release"]), (0, "PASS", True))
        self.assertRegex(value["non_published_list_sha256"], r"\A[0-9a-f]{64}\Z")
        # a list that is not in the packager's form, and rules that already use the category, are refused
        good = json.loads(self.non_published(["1" * 64]).read_text())
        for label, changes in (("schema", {"schema": "another/1"}), ("trees", {"trees": []}),
                               ("digest", {"trees": {"selected/provenance/source/": [{"name": "a", "sha256": "12"}]}}),
                               ("snapshots", {"original_snapshots": {"x": "1" * 64}})):
            with self.subTest(list=label):
                self.assertEqual(self.scan(payload, extra=("--non-published-list", self.non_published(**changes))),
                                 (4, None))
        missing = {key: value for key, value in good.items() if key != "original_snapshots"}
        path = self.root / "non-published-missing-key.json"
        dump(path, missing)
        self.assertEqual(self.scan(payload, extra=("--non-published-list", path)), (4, None))
        taken = self.rules_file({**json.loads(RULES.read_text())["categories"], "withheld_member_reference": {
            "rules": [{"id": "other", "kind": "token", "value": "zq-other-token"}]}})
        self.assertEqual(self.scan(payload, rules=taken, extra=("--non-published-list", self.non_published(["1" * 64]))),
                         (4, None))

    def test_rules_that_can_refuse_nothing_cannot_pass(self):
        payload = self.payload(product_bytes({"selected/notes.txt": b"notes " + MARKER + b"\n"}))
        marker = {"rules": [{"id": "synthetic-marker-1", "kind": "token", "value": MARKER.decode()}]}
        home = {"rules": [{"id": "home-paths", "kind": "regex", "value": r"/(?:home|Users)/[^/\s]+/"}]}
        cases = (
            ("no-blocking-rules", self.rules_file({"review_marker": marker, "review_home_paths": home}), EXEMPTIONS, 2),
            ("home-paths-off", self.rules_file({"synthetic_marker": marker}), EXEMPTIONS, 3),
            ("exemption-in-blocking-category", RULES, self.exemptions_file(
                [{"category": "synthetic_marker", "match": MARKER.decode(), "reason": "synthetic"}]), 2),
        )
        for reason, rules, exemptions, expected in cases:
            with self.subTest(reason=reason):
                code, report = self.scan(payload, rules=rules, exemptions=exemptions)
                self.assertEqual((code, report["status"]), (expected, "REFUSED"))
                self.assertIn(reason, report["reasons"])
        # an exemption in a review category does not weaken a blocking rule
        rules = self.rules_file({"synthetic_marker": marker, "home_paths": home, "review_note": {
            "rules": [{"id": "review-note-1", "kind": "token", "value": "zq-review-note-51c2"}]}})
        exemptions = self.exemptions_file([{"category": "review_note", "match": "zq-review-note-51c2", "reason": "synthetic"}])
        code, report = self.scan(self.payload(product_bytes({"selected/notes.txt": b"public\n"})),
                                 rules=rules, exemptions=exemptions)
        self.assertEqual((code, report["status"], report["exemptions"]["count"], report["exemptions"]["blocking"]),
                         (0, "PASS", 1, 0))
        self.assertEqual(report["rules"]["total"], 3)
        self.assertEqual(report["rules"]["blocking"], 2)

    def test_only_the_converters_own_path_rule_is_the_path_category(self):
        # The report counts the review rules outside the path category, so a later step can require that
        # every rule of a name class is blocking. The path category is the one the converter writes with
        # --paths review, by its exact content: a class of names under that category's name is counted.
        converter = load("converter_for_path_category_test", HERE / "privacy-rules-from-names.py")
        built_in = converter.build([("product", "zqmadeupname")], set(), "review")["categories"]["review_home_paths"]
        self.assertEqual(load("wrapper_for_path_category_test", WRAPPER).REVIEW_PATH_CATEGORY, built_in)
        payload = self.payload(product_bytes({"selected/notes.txt": b"public notes\n"}))
        marker = {"rules": [{"id": "synthetic-marker-1", "kind": "token", "value": MARKER.decode()}]}
        name = {"id": "review-note-1", "kind": "token", "value": "zq-review-note-51c2"}
        path_rule = built_in["rules"][0]
        cases = (
            ("the converter's path category", {"review_home_paths": built_in}, (2, 1, 0)),
            ("a name beside the path rule", {"review_home_paths": {"rules": [path_rule, name]}}, (3, 1, 2)),
            ("a name with the path rule's id", {"review_home_paths": {"rules": [dict(name, id="home-paths")]}}, (2, 1, 1)),
            ("another pattern under the path rule's id", {"review_home_paths": {"rules": [dict(path_rule, value="/(?:home)/zq[a-z]+/")]}},
             (2, 1, 1)),
            ("one key more in the category", {"review_home_paths": {**built_in, "note": "made up"}}, None),
            ("the path rule under another review name", {"home_paths": built_in, "review_paths": {"rules": [dict(path_rule, id="paths-2")]}},
             (3, 2, 1)),
            ("a review class beside blocking paths", {"home_paths": built_in, "review_note": {"rules": [name]}}, (3, 2, 1)),
            ("a review class beside review paths", {"review_home_paths": built_in, "review_note": {"rules": [name, dict(name, id="review-note-2", value="zq-other-note-88d1")]}},
             (4, 1, 2)),
        )
        for label, categories, expected in cases:
            with self.subTest(rules=label):
                rules = self.rules_file({"synthetic_marker": marker, **categories})
                code, report = self.scan(payload, rules=rules)
                if expected is None:
                    # Not the converter's category: its rule counts as any review rule, if the engine takes the file.
                    self.assertTrue(report is None or report["rules"]["review_outside_paths"] == 1, report)
                    continue
                self.assertEqual((code, report["status"]), (0, "PASS"))
                self.assertEqual(report["rules"], {"sha256": hashlib.sha256(rules.read_bytes()).hexdigest(), "total": expected[0],
                                                   "blocking": expected[1], "review_outside_paths": expected[2]})

    def test_a_release_scan_names_the_commit_of_the_tool_it_ran_from(self):
        payload = self.payload(product_bytes({"selected/notes.txt": b"public notes\n"}))
        tool = self.tool_copy("committed")
        head = subprocess.run(["git", "-C", str(tool.parent), "rev-parse", "HEAD"], check=True,
                              capture_output=True, text=True).stdout.strip()
        code, report = self.scan(payload, wrapper=tool, release=True, extra=("--label", "candidate-1"))
        self.assertEqual((code, report["status"], report["release"], report["label"], report["tool_commit"]),
                         (0, "PASS", True, "candidate-1", head))
        # a tool that is not the committed one, or is in no repository, cannot make a release report
        with open(tool, "a") as stream:
            stream.write("\n# changed\n")
        code, report = self.scan(payload, wrapper=tool, release=True)
        self.assertEqual((code, report), (4, None))
        self.assertIn("--proof-only", self.last.stderr)
        loose = self.tool_copy("loose", repository=False)
        code, report = self.scan(payload, wrapper=loose, release=True)
        self.assertEqual((code, report), (4, None))
        # the explicit override marks the report: it is not a release report and its label says so
        code, report = self.scan(payload, wrapper=loose, extra=("--label", "candidate-1"))
        self.assertEqual((code, report["status"], report["release"], report["label"], report["tool_commit"]),
                         (0, "PASS", False, "PROOF-ONLY-candidate-1", None))
        code, report = self.scan(payload, wrapper=loose)
        self.assertEqual(report["label"], "PROOF-ONLY-run")
        code, report = self.scan(payload, wrapper=self.tool_copy("named"), release=True, extra=("--label", "PROOF-ONLY-x"))
        self.assertEqual((code, report), (4, None))

    def test_a_payload_replaced_during_the_scan_is_refused(self):
        wrapper = load("payload_scan_replaced", WRAPPER)
        data = product_bytes({"selected/notes.txt": b"public notes\n"})
        payload = self.payload(data)
        report = self.root / "replaced-report.json"
        real_run = subprocess.run

        def replacing(command, *args, **kwargs):
            if command[0] == sys.executable:  # the engine run: the walk has read the payload by now
                payload.write_bytes(product_bytes({"selected/notes.txt": b"other public notes\n"}))
            return real_run(command, *args, **kwargs)

        argv = [str(WRAPPER), "--proof-only", "--rules", str(RULES), "--exemptions", str(EXEMPTIONS), "--report",
                str(report), "--detail", str(self.root / "replaced-detail.json"), "--work", str(self.root), str(payload)]
        with mock.patch.object(wrapper.subprocess, "run", replacing), mock.patch.object(sys, "argv", argv), \
                mock.patch.object(sys, "stderr", io.StringIO()):
            code = wrapper.main()
        value = json.loads(report.read_text())
        self.assertEqual((code, value["status"]), (2, "REFUSED"))
        self.assertIn("payload-changed-during-scan", value["reasons"])
        self.assertEqual(value["payload"][0]["sha256"], hashlib.sha256(data).hexdigest())

    def engine_alone(self, payload):
        self.count += 1
        report, detail = self.root / f"engine-{self.count}.json", self.root / f"engine-detail-{self.count}.json"
        done = run(ENGINE, "--rules", RULES, "--exemptions", EXEMPTIONS, "--report", report, "--detail", detail, payload)
        return done.returncode, json.loads(report.read_text())

    def test_clean_nested_product_passes(self):
        code, report = self.scan(self.payload(product_bytes(
            {"selected/provenance/demo/source.tar.gz": source_archive(b"public notes\n")})))
        self.assertEqual((code, report["status"]), (0, "PASS"))
        self.assertEqual(report["hits"], {"accepted": {}, "refused": {}})

    def test_marker_in_source_archive_in_library_in_product_is_refused(self):
        code, report = self.scan(self.payload(product_bytes(
            {"selected/provenance/demo/source.tar.gz": source_archive(b"notes " + MARKER + b"\n")})))
        self.assertEqual((code, report["status"]), (1, "REFUSED"))
        self.assertEqual(report["hits"]["refused"], {"synthetic_marker": 1})

    def test_a_review_category_is_counted_and_never_refused_or_accepted(self):
        rules = self.rules_file({
            **json.loads(RULES.read_text())["categories"],
            "review_note": {"rules": [{"id": "review-note-1", "kind": "token", "value": "zq-review-note-51c2"}]}})
        nested = {"selected/provenance/demo/source.tar.gz": source_archive(b"a zq-review-note-51c2 line\n"),
                  "consumers/demo.whl": wheel_bytes({"demo/GUIDE.md": b"zq-review-note-51c2 again\n"})}
        code, report = self.scan(self.payload(product_bytes(nested)), rules=rules)
        self.assertEqual((code, report["status"]), (0, "PASS"))
        self.assertEqual(report["hits"], {"accepted": {}, "refused": {}})
        self.assertEqual(report["review_hits"], {"review_note": 2})
        nested["selected/notes.txt"] = b"notes " + MARKER + b"\n"
        code, report = self.scan(self.payload(product_bytes(nested)), rules=rules)
        self.assertEqual((code, report["status"]), (1, "REFUSED"))
        self.assertEqual(report["hits"]["refused"], {"synthetic_marker": 1})
        self.assertEqual(report["review_hits"], {"review_note": 2})

    def test_a_report_without_review_categories_has_an_empty_review_count(self):
        code, report = self.scan(self.payload(product_bytes({"selected/notes.txt": b"public notes\n"})))
        self.assertEqual((code, report["review_hits"]), (0, {}))

    def test_the_refused_listing_names_rules_and_file_digests_and_no_matched_text(self):
        text = b"notes " + MARKER + b"\n"
        inner = source_archive(text)
        payload = self.payload(product_bytes({"selected/provenance/demo/source.tar.gz": inner}))
        listing = self.root / "refused-listing.json"
        report, detail = self.root / "listing-report.json", self.root / "listing-detail.json"
        done = run(WRAPPER, "--proof-only", "--rules", RULES, "--exemptions", EXEMPTIONS, "--report", report, "--detail", detail,
                   "--refused", listing, "--work", self.root, payload)
        self.assertEqual(done.returncode, 1, done.stderr)
        self.assertEqual(listing.stat().st_mode & 0o777, 0o600)
        self.assertNotIn(MARKER.decode(), listing.read_text())
        value = json.loads(listing.read_text())
        self.assertEqual(value["schema"], "release-payload-privacy-scan/refused/1")
        self.assertEqual(len(value["hits"]), 1)
        hit = value["hits"][0]
        self.assertEqual((hit["category"], hit["rules"], hit["count"]), ("synthetic_marker", ["synthetic-marker-1"], 1))
        # the file that holds the finding first, then each container that encloses it, the payload last
        self.assertEqual(hit["sha256"][0], hashlib.sha256(text).hexdigest())
        self.assertIn(hashlib.sha256(inner).hexdigest(), hit["sha256"])
        self.assertEqual(hit["sha256"][-1], hashlib.sha256(payload.read_bytes()).hexdigest())
        self.assertTrue(hit["location"].endswith("source/notes.txt"))
        # accepting the listed finding in its innermost file makes the same payload clean, with an empty listing
        accepted = self.accepted([{"category": hit["category"], "rule": hit["rules"][0], "sha256": hit["sha256"][0],
                                   "reason": "synthetic"}])
        again = self.root / "refused-listing-2.json"
        done = run(WRAPPER, "--proof-only", "--rules", RULES, "--exemptions", EXEMPTIONS, "--accepted-findings", accepted,
                   "--report", self.root / "listing-report-2.json", "--detail", self.root / "listing-detail-2.json",
                   "--refused", again, "--work", self.root, payload)
        self.assertEqual(done.returncode, 0, done.stderr)
        self.assertEqual(json.loads(again.read_text())["hits"], [])

    def test_the_refused_listing_names_a_refused_error_with_its_container_digest(self):
        broken = b"\x1f\x8b\x08\x00 not a gzip stream"
        payload = self.payload(product_bytes({"selected/provenance/demo/source.tar.gz": broken}))
        listing = self.root / "refused-errors.json"
        done = run(WRAPPER, "--proof-only", "--rules", RULES, "--exemptions", EXEMPTIONS, "--report", self.root / "errors-report.json",
                   "--detail", self.root / "errors-detail.json", "--refused", listing, "--work", self.root, payload)
        self.assertEqual(done.returncode, 2, done.stderr)
        errors = json.loads(listing.read_text())["errors"]
        self.assertEqual(len(errors), 1)
        self.assertEqual(errors[0]["container_sha256"], hashlib.sha256(broken).hexdigest())
        self.assertIn(errors[0]["kind"], ("corrupt-archive", "archive-layout", "unsupported-archive"))

    def test_the_refused_listing_withholds_a_path_that_matches_a_rule(self):
        name = "selected/" + MARKER.decode() + "/notes.txt"
        payload = self.payload(product_bytes({name: b"notes " + MARKER + b"\n"}))
        listing = self.root / "refused-names.json"
        done = run(WRAPPER, "--proof-only", "--rules", RULES, "--exemptions", EXEMPTIONS, "--report", self.root / "names-report.json",
                   "--detail", self.root / "names-detail.json", "--refused", listing, "--work", self.root, payload)
        self.assertEqual(done.returncode, 1, done.stderr)
        self.assertNotIn(MARKER.decode(), listing.read_text())
        self.assertNotIn(MARKER.decode(), done.stdout + done.stderr)
        hits = json.loads(listing.read_text())["hits"]
        self.assertTrue(hits)
        # the engine reports a hit in a member's name at its container and a hit in its content at the
        # member: the second path holds the matching name, so it is withheld
        self.assertTrue(any(hit["location_withheld"] for hit in hits))
        for hit in hits:
            self.assertEqual(hit["location"] is None, hit["location_withheld"])
            self.assertTrue(hit["sha256"])

    def test_the_refused_listing_withholds_a_path_that_matches_a_rule_in_an_encoded_form(self):
        encoded = MARKER.decode().replace("-", "%2D", 1)
        self.assertNotIn(MARKER.decode(), encoded)
        payload = self.payload(product_bytes({"selected/" + encoded + "/notes.txt": b"public notes\n"}))
        listing = self.root / "refused-encoded.json"
        code, report = self.scan(payload, extra=("--refused", listing))
        # the name is a hit at the member and in the raw tar header block that holds it
        self.assertEqual((code, report["hits"]["refused"]), (1, {"synthetic_marker": 2}))
        self.assertNotIn(encoded, listing.read_text())
        self.assertNotIn(encoded, self.last.stdout + self.last.stderr)
        hits = json.loads(listing.read_text())["hits"]
        self.assertEqual(sorted(hit["location_withheld"] for hit in hits), [False, True])
        for hit in hits:
            self.assertEqual(hit["location"] is None, hit["location_withheld"])

    def test_a_member_name_that_makes_a_location_ambiguous_refuses(self):
        # locations join a container and a member with "!": the member "a!b" beside a member "a" reads
        # like a member "b" inside "a"
        for members in ({"selected/a": b"public notes\n", "selected/a!b": b"more public notes\n"},
                        {"selected/a!b": b"more public notes\n", "selected/a": tar_bytes({"b": b"public notes\n"})}):
            with self.subTest(members=sorted(members)):
                code, report = self.scan(self.payload(product_bytes(members)))
                self.assertEqual((code, report["status"]), (2, "REFUSED"))
                self.assertIn("wrapper-failures", report["reasons"])
                self.assertGreaterEqual(report["wrapper_failures"], 1)
        # the separator in a name that no other location shares is no ambiguity
        code, report = self.scan(self.payload(product_bytes({"selected/a!b": b"public notes\n"})))
        self.assertEqual((code, report["status"], report["wrapper_failures"]), (0, "PASS", 0))
        # a payload file name with the separator is refused before the scan
        code, report = self.scan(self.payload(product_bytes({"notes.txt": b"public\n"}), name="product!1.tar.gz"))
        self.assertEqual((code, report), (4, None))

    def test_a_span_matched_by_two_rules_needs_both_accepted(self):
        text = b"notes " + MARKER + b"\n"
        categories = json.loads(RULES.read_text())["categories"]
        categories["synthetic_marker"]["rules"].append(
            {"id": "synthetic-part-2", "kind": "literal", "value": MARKER.decode()[3:-5]})
        rules = self.rules_file(categories)
        payload = self.payload(product_bytes({"selected/notes.txt": text}))
        entry = {"category": "synthetic_marker", "sha256": hashlib.sha256(text).hexdigest(), "reason": "synthetic"}
        one = self.accepted([{**entry, "rule": "synthetic-marker-1"}])
        both = self.accepted([{**entry, "rule": "synthetic-marker-1"}, {**entry, "rule": "synthetic-part-2"}])
        code, report = self.scan(payload, rules=rules, accepted=one)
        self.assertEqual((code, report["hits"]), (1, {"accepted": {}, "refused": {"synthetic_marker": 1}}))
        code, report = self.scan(payload, rules=rules, accepted=both)
        self.assertEqual((code, report["hits"]), (0, {"accepted": {"synthetic_marker": 1}, "refused": {}}))

    def exceptions(self, entries):
        self.count += 1
        path = self.root / f"exceptions-{self.count}.json"
        dump(path, {"schema": "release-payload-privacy-scan/coverage-exceptions/1", "exceptions": entries})
        return path

    def scan_with_exceptions(self, payload, exceptions):
        self.count += 1
        report, listing = self.root / f"x-report-{self.count}.json", self.root / f"x-refused-{self.count}.json"
        done = run(WRAPPER, "--proof-only", "--rules", RULES, "--exemptions", EXEMPTIONS, "--coverage-exceptions", exceptions,
                   "--report", report, "--detail", self.root / f"x-detail-{self.count}.json", "--refused", listing,
                   "--work", self.root, payload)
        return done.returncode, (json.loads(report.read_text()) if report.exists() else None), \
            (json.loads(listing.read_text()) if listing.exists() else None)

    def test_an_unread_member_refuses_unless_a_coverage_exception_names_its_exact_bytes(self):
        broken = b"\x1f\x8b\x08\x00 not a gzip stream"
        member = "selected/provenance/demo/source.tar.gz"
        payload = self.payload(product_bytes({member: broken}))
        code, report, listing = self.scan_with_exceptions(payload, self.exceptions([]))
        self.assertEqual((code, report["status"]), (2, "REFUSED"))
        self.assertEqual(report["coverage_exceptions"]["used"], {})
        error = listing["errors"][0]
        entry = {"path": member, "sha256": error["container_sha256"], "kind": error["kind"],
                 "reason": "synthetic fixture", "accepted_by": "test"}
        code, report, listing = self.scan_with_exceptions(payload, self.exceptions([entry]))
        self.assertEqual((code, report["status"], report["errors"]["refused"]), (0, "PASS", {}))
        self.assertEqual(report["coverage_exceptions"]["used"], {error["kind"]: 1})
        self.assertEqual(report["coverage_exceptions"]["count"], 1)
        self.assertEqual(listing["errors"], [])
        # the same error in other bytes, at another path, or named with another kind, still refuses
        other_kind = next(kind for kind in ("archive-layout", "corrupt-archive") if kind != error["kind"])
        for change in ({"sha256": hashlib.sha256(b"other").hexdigest()}, {"path": "selected/other/source.tar.gz"},
                       {"kind": other_kind}):
            code, report, _ = self.scan_with_exceptions(payload, self.exceptions([{**entry, **change}]))
            self.assertEqual((code, report["errors"]["refused"]), (2, {error["kind"]: 1}))

    def test_a_coverage_exception_needs_every_field_and_cannot_cover_a_size_bound(self):
        payload = self.payload(product_bytes({"selected/notes.txt": b"public notes\n"}))
        good = {"path": "a/b.tar", "sha256": "0" * 64, "kind": "archive-layout", "reason": "r", "accepted_by": "t"}
        for bad in ({key: value for key, value in good.items() if key != "accepted_by"}, {**good, "accepted_by": " "},
                    {**good, "kind": "member-too-large"}, {**good, "sha256": "0" * 63}, {**good, "path": ""}):
            code, report, _ = self.scan_with_exceptions(payload, self.exceptions([bad]))
            self.assertEqual((code, report), (4, None))

    def test_the_committed_coverage_exceptions_are_valid_and_the_candidate_scan_uses_them(self):
        wrapper = load("payload_scan_exceptions", WRAPPER)
        committed = HERE.parent / "release/payload-scan-coverage-exceptions.json"
        self.assertIsInstance(wrapper.read_exceptions(committed)[0], list)
        candidate = load("candidate_for_exceptions", CANDIDATE)
        self.assertEqual(candidate.COVERAGE_EXCEPTIONS, committed)

    def test_marker_beyond_the_engine_depth_is_found_by_the_wrapper(self):
        inner = tar_bytes({"inner/deep.txt.gz": gzip.compress(b"deep " + MARKER + b"\n", mtime=0)})
        payload = self.payload(product_bytes({"selected/provenance/demo/source.tar.gz": tar_bytes({"source/inner.tar.gz": inner})}))
        code, report = self.engine_alone(payload)
        self.assertEqual((code, report["hit_total"]), (2, 0))
        self.assertEqual([row["kind"] for row in report["errors"]], ["depth-limit"])
        code, report = self.scan(payload)
        self.assertEqual(code, 1)
        self.assertEqual(report["hits"]["refused"], {"synthetic_marker": 1})
        self.assertEqual(report["errors"], {"accepted": {}, "covered": {"depth-limit": 1}, "refused": {}})
        self.assertEqual(report["reroots"]["depth"], 1)

    def test_marker_inside_zstd_is_found_by_the_wrapper(self):
        compressed = zstd_compress(tar_bytes({"data/notes.txt": b"zst " + MARKER + b"\n"}, compressed=False))
        if compressed is None:
            self.skipTest("no zstd encoder on this host")
        payload = self.payload(product_bytes({"selected/provenance/demo/data.tar.zst": compressed}))
        code, report = self.engine_alone(payload)
        self.assertEqual((code, report["hit_total"]), (2, 0))
        code, report = self.scan(payload)
        self.assertEqual(code, 1)
        self.assertEqual(report["hits"]["refused"], {"synthetic_marker": 1})
        self.assertEqual(report["errors"]["covered"], {"unsupported-archive": 1})

    def test_a_bare_zlib_stream_is_inflated_and_its_content_scanned(self):
        text = b"notes " + MARKER + b"\n"
        cases = {
            "text": zlib.compress(text),
            "fastest-level": zlib.compress(text, 1),
            "tar": zlib.compress(tar_bytes({"data/notes.txt": text}, compressed=False)),
            "second-stream": zlib.compress(b"public notes\n") + zlib.compress(text),
            "nested": zlib.compress(zlib.compress(text)),
            "container-after-the-stream": zlib.compress(b"public notes\n") + gzip.compress(text, mtime=0),
        }
        for label, data in cases.items():
            with self.subTest(content=label):
                payload = self.payload(product_bytes({"selected/objects/blob": data}))
                listing = self.root / f"zlib-{label}.json"
                code, report = self.scan(payload, extra=("--refused", listing))
                self.assertEqual((code, report["hits"]["refused"]), (1, {"synthetic_marker": 1}))
                self.assertGreaterEqual(report["reroots"]["zlib"], 1)
                self.assertEqual(report["wrapper_failures"], 0)
                hit = json.loads(listing.read_text())["hits"][0]
                self.assertIn("selected/objects/blob", hit["location"])
                if label == "text":
                    # the innermost holder is the inflated content: an accepted finding names its digest
                    self.assertEqual(hit["sha256"][0], hashlib.sha256(text).hexdigest())
                    self.assertEqual(hit["sha256"][1], hashlib.sha256(data).hexdigest())
        payload = self.payload(product_bytes({"selected/objects/blob": zlib.compress(text)}))
        entry = {"category": "synthetic_marker", "rule": "synthetic-marker-1", "reason": "synthetic",
                 "sha256": hashlib.sha256(text).hexdigest()}
        code, report = self.scan(payload, accepted=self.accepted([entry]))
        self.assertEqual((code, report["hits"]), (0, {"accepted": {"synthetic_marker": 1}, "refused": {}}))

    def test_content_that_only_begins_like_a_zlib_stream_is_read_as_it_is(self):
        # both begin with a valid zlib header (78 5e, 48 4b) and are no stream
        for text in (b"x^2 + y^2 = 1\n", b"HKEY_SYNTHETIC\\Software\\Demo\n"):
            with self.subTest(text=text[:4]):
                code, report = self.scan(self.payload(product_bytes({"selected/notes.txt": text})))
                self.assertEqual((code, report["status"]), (0, "PASS"))
                self.assertEqual((report["reroots"]["zlib"], report["wrapper_failures"]), (0, 0))
                code, report = self.scan(self.payload(product_bytes({"selected/notes.txt": text + MARKER + b"\n"})))
                self.assertEqual((code, report["hits"]["refused"]), (1, {"synthetic_marker": 1}))
        # The stated limit: an encoding the scan does not recognise is read as raw bytes. A deflate
        # stream without the zlib header is one.
        raw = zlib.compressobj(9, zlib.DEFLATED, -zlib.MAX_WBITS)
        data = raw.compress(b"notes " + MARKER + b"\n") + raw.flush()
        code, report = self.scan(self.payload(product_bytes({"selected/objects/blob": data})))
        self.assertEqual((code, report["reroots"]["zlib"]), (0, 0))

    def test_undecodable_nested_containers_refuse(self):
        for broken in (source_archive(b"public notes\n")[:-20], b"7z\xbc\xaf\x27\x1c" + b"\x00" * 64):
            with self.subTest(head=broken[:4]):
                code, report = self.scan(self.payload(product_bytes({"selected/provenance/demo/source.tar.gz": broken})))
                self.assertEqual((code, report["status"]), (2, "REFUSED"))
                self.assertTrue(report["errors"]["refused"])

    def test_missing_rules_or_exemptions_refuse_without_a_report(self):
        payload = self.payload(product_bytes({"notes.txt": b"public\n"}))
        for missing in ("rules", "exemptions"):
            with self.subTest(missing=missing):
                code, report = self.scan(payload, **{missing: None})
                self.assertEqual((code, report), (4, None))

    def test_without_an_accepted_file_nothing_is_accepted(self):
        payload = self.payload(product_bytes({"notes.txt": b"notes " + MARKER + b"\n"}))
        for accepted in (None, self.accepted([])):
            with self.subTest(accepted=accepted):
                code, report = self.scan(payload, accepted=accepted)
                self.assertEqual((code, report["hits"]["accepted"]), (1, {}))

    def test_an_accepted_finding_is_exact_to_the_file_digest(self):
        texts = (b"third party " + MARKER + b"\n", b"third party " + MARKER + b" again\n")
        first, second = (wheel_bytes({"thirdparty/data.txt": text}) for text in texts)
        entry = {"category": "synthetic_marker", "rule": "synthetic-marker-1",
                 "sha256": hashlib.sha256(texts[0]).hexdigest(), "reason": "synthetic third-party bytes"}
        accepted = self.accepted([entry])
        name = "consumers/demo/thirdparty-1.0-py3-none-any.whl"
        code, report = self.scan(self.payload(product_bytes({name: first})), accepted=accepted)
        self.assertEqual((code, report["hits"]), (0, {"accepted": {"synthetic_marker": 1}, "refused": {}}))
        self.assertEqual(report["accepted"]["used"], {"member": 1, "archive": 0})
        code, report = self.scan(self.payload(product_bytes({name: second})), accepted=accepted)
        self.assertEqual((code, report["hits"]), (1, {"accepted": {}, "refused": {"synthetic_marker": 1}}))
        other_rule = self.accepted([{**entry, "rule": "another-rule"}])
        code, report = self.scan(self.payload(product_bytes({name: first})), accepted=other_rule)
        self.assertEqual(code, 1)
        other_category = self.accepted([{**entry, "category": "another_category"}])
        code, report = self.scan(self.payload(product_bytes({name: first})), accepted=other_category)
        self.assertEqual(code, 1)

    def test_an_accepted_finding_binds_to_the_innermost_member_that_holds_it(self):
        texts = [b"member %d " % n + MARKER + b"\n" for n in range(3)]
        wheel = wheel_bytes({"thirdparty/data-%d.txt" % n: text for n, text in enumerate(texts[:2])})
        library = library_bytes({"consumers/demo/thirdparty-1.0-py3-none-any.whl": wheel, "selected/notes.txt": texts[2]})
        product = tar_bytes({"pkg/bin/tool": b"inert tool\n", "pkg/libexec/archives/library.tar.gz": library})
        payload = self.payload(product)
        entry = {"category": "synthetic_marker", "rule": "synthetic-marker-1", "reason": "synthetic"}

        def scan(*digests, **keys):
            return self.scan(payload, accepted=self.accepted(
                [{**entry, **keys, "sha256": hashlib.sha256(data).hexdigest()} for data in digests]))

        # one entry for the payload file, for the library or for the wheel accepts nothing inside them
        for label, data in (("payload", product), ("library", library), ("wheel", wheel)):
            with self.subTest(enclosing=label):
                code, report = scan(data)
                self.assertEqual((code, report["hits"]), (1, {"accepted": {}, "refused": {"synthetic_marker": 3}}))
        # each member is accepted by its own digest only
        code, report = scan(texts[0])
        self.assertEqual((code, report["hits"]), (1, {"accepted": {"synthetic_marker": 1}, "refused": {"synthetic_marker": 2}}))
        code, report = scan(*texts)
        self.assertEqual((code, report["hits"]), (0, {"accepted": {"synthetic_marker": 3}, "refused": {}}))
        self.assertEqual(report["accepted"]["used"], {"member": 3, "archive": 0})

    def test_a_public_upstream_archive_entry_covers_the_hits_inside_that_archive_only(self):
        texts = [b"member %d " % n + MARKER + b"\n" for n in range(3)]
        wheel = wheel_bytes({"thirdparty/data-%d.txt" % n: text for n, text in enumerate(texts[:2])})
        library = library_bytes({"consumers/demo/thirdparty-1.0-py3-none-any.whl": wheel, "selected/notes.txt": texts[2]})
        product = tar_bytes({"pkg/bin/tool": b"inert tool\n", "pkg/libexec/archives/library.tar.gz": library})
        payload = self.payload(product)
        entry = {"category": "synthetic_marker", "rule": "synthetic-marker-1", "reason": "synthetic",
                 "class": "PUBLIC-UPSTREAM-ARCHIVE"}

        def scan(data, **keys):
            return self.scan(payload, accepted=self.accepted([{**entry, **keys, "sha256": hashlib.sha256(data).hexdigest()}]))

        code, report = scan(wheel)
        self.assertEqual((code, report["hits"]), (1, {"accepted": {"synthetic_marker": 2}, "refused": {"synthetic_marker": 1}}))
        self.assertEqual(report["accepted"]["used"], {"member": 0, "archive": 2})
        # never a top-level payload file, whatever the class says
        code, report = scan(product)
        self.assertEqual((code, report["hits"]["accepted"]), (1, {}))
        # another class keeps the innermost binding
        code, report = scan(wheel, **{"class": "PUBLIC-SOURCE"})
        self.assertEqual((code, report["hits"]["accepted"]), (1, {}))
        # a payload file that is itself the member that holds the hit is its own innermost holder
        loose = self.payload(texts[2], name="notes.txt")
        code, report = self.scan(loose, accepted=self.accepted(
            [{**entry, "class": "PUBLIC-SOURCE", "sha256": hashlib.sha256(texts[2]).hexdigest()}]))
        self.assertEqual((code, report["hits"]), (0, {"accepted": {"synthetic_marker": 1}, "refused": {}}))

    def test_the_report_counts_the_used_entries_without_a_proof_and_names_the_proof_tools(self):
        # An acceptance entry is proved when it names the tool that proved it: a proof object whose
        # `tool` is a tool's name. The scan reads no more of a proof than that. Its report counts
        # the entries it used that are not proved, and the used entries by proof tool, so a later step
        # can require that nothing was accepted by hand. An entry that accepted nothing counts nowhere.
        texts = [b"member %d " % n + MARKER + b"\n" for n in range(3)]
        payload = self.payload(product_bytes({"consumers/a.txt": texts[0], "consumers/b.txt": texts[1], "consumers/c.txt": texts[2]}))
        base = {"category": "synthetic_marker", "rule": "synthetic-marker-1", "reason": "synthetic"}
        prover = {"tool": "made-up-prover"}

        def entry(data, **keys):
            return {**base, "sha256": hashlib.sha256(data).hexdigest(), **keys}

        def accepted_part(target, *entries, expected=(0, "PASS")):
            code, report = self.scan(target, accepted=self.accepted(list(entries)))
            self.assertEqual((code, report["status"]), expected)
            self.assertEqual(set(report["accepted"]), {"sha256", "count", "used", "used_unproved", "used_by_tool"})
            return report["accepted"]["used_unproved"], report["accepted"]["used_by_tool"]

        self.assertEqual(accepted_part(payload, entry(texts[0], proof=prover), entry(texts[1], proof={**prover, "census": {"made-up": 1}}),
                                       entry(texts[2], proof={"tool": "other-prover"})), (0, {"made-up-prover": 2, "other-prover": 1}))
        for label, keys in (("no proof", {}), ("a proof that names no tool", {"proof": {"census": {"made-up": 1}}}),
                            ("an empty tool", {"proof": {"tool": ""}}), ("a blank tool", {"proof": {"tool": "  "}}),
                            ("a number", {"proof": {"tool": 7}}), ("a list", {"proof": {"tool": ["made-up-prover"]}}),
                            ("null", {"proof": {"tool": None}}), ("true", {"proof": {"tool": True}}),
                            # A tool's name has one form: a lower-case letter or a digit, then lower-case
                            # letters, digits and single hyphens, 64 characters at most. Any other text
                            # names no tool: the name reaches the upload receipt as it is written.
                            ("words", {"proof": {"tool": "made up prover"}}), ("a sentence", {"proof": {"tool": "accepted by hand."}}),
                            ("a path", {"proof": {"tool": "tools/made-up-prover.py"}}), ("a file name", {"proof": {"tool": "prover.py"}}),
                            ("an upper-case letter", {"proof": {"tool": "Made-up-prover"}}), ("an underscore", {"proof": {"tool": "made_up"}}),
                            ("two hyphens in a row", {"proof": {"tool": "made--up"}}), ("a hyphen first", {"proof": {"tool": "-prover"}}),
                            ("a space before it", {"proof": {"tool": " made-up-prover"}}), ("a line break after it", {"proof": {"tool": "made-up-prover\n"}}),
                            ("two lines", {"proof": {"tool": "made-up\nprover"}}), ("another alphabet's letter", {"proof": {"tool": "pr" + chr(0xf6) + "ver"}}),
                            ("65 characters", {"proof": {"tool": "p" * 65}}), ("very many characters", {"proof": {"tool": "p" * 100000}})):
            with self.subTest(unproved=label):
                self.assertEqual(accepted_part(payload, entry(texts[0], **keys), entry(texts[1], proof=prover), entry(texts[2], proof=prover)),
                                 (1, {"made-up-prover": 2}))
        self.assertEqual(accepted_part(payload, entry(texts[0]), entry(texts[1]), entry(texts[2])), (3, {}))
        # Names of that form: one character, a digit first, 64 characters, the code identifier tool's own.
        self.assertEqual(accepted_part(payload, entry(texts[0], proof={"tool": "7"}), entry(texts[1], proof={"tool": "p" * 64}),
                                       entry(texts[2], proof={"tool": "release-code-identifier-entry"})),
                         (0, {"7": 1, "p" * 64: 1, "release-code-identifier-entry": 1}))
        # Entries that accept nothing, proved or not, and a proved entry beside an unproved one for the same member.
        self.assertEqual(accepted_part(payload, entry(texts[0], proof=prover), entry(texts[1], proof=prover), entry(texts[2], proof=prover),
                                       entry(b"bytes that are not in the payload"), entry(b"nor these", proof={"tool": "unused-prover"}),
                                       {**entry(texts[0]), "rule": "another-rule"}), (0, {"made-up-prover": 3}))
        self.assertEqual(accepted_part(payload, entry(texts[0], proof=prover), entry(texts[0], reason="a second entry for the same bytes"),
                                       entry(texts[1], proof=prover), entry(texts[2], proof=prover)), (1, {"made-up-prover": 3}))
        # An entry that still leaves a hit refused is used all the same.
        self.assertEqual(accepted_part(payload, entry(texts[0]), entry(texts[1], proof=prover), expected=(1, "REFUSED")),
                         (1, {"made-up-prover": 1}))
        # An archive entry is one entry, however many hits it covers.
        wheel = wheel_bytes({"thirdparty/data-0.txt": texts[0], "thirdparty/data-1.txt": texts[1]})
        nested = self.payload(product_bytes({"consumers/demo/thirdparty-1.0-py3-none-any.whl": wheel}))
        archive = {"class": "PUBLIC-UPSTREAM-ARCHIVE"}
        self.assertEqual(accepted_part(nested, entry(wheel, **archive)), (1, {}))
        self.assertEqual(accepted_part(nested, entry(wheel, **archive, proof={"tool": "archive-prover"})), (0, {"archive-prover": 1}))
        self.assertEqual(accepted_part(nested, entry(wheel, **archive, proof={"tool": "archive-prover"}), entry(texts[0])),
                         (1, {"archive-prover": 1}))
        # An accepted structural error is a used entry too.
        broken = source_archive(b"public notes\n")[:-20]
        damaged = self.payload(product_bytes({"selected/provenance/demo/source.tar.gz": broken}))
        structure = {"category": "structure", "rule": "corrupt-archive", "sha256": hashlib.sha256(broken).hexdigest(), "reason": "synthetic"}
        self.assertEqual(accepted_part(damaged, structure), (1, {}))
        self.assertEqual(accepted_part(damaged, {**structure, "proof": {"tool": "structure-prover"}}), (0, {"structure-prover": 1}))
        # Without a file, and with a file that accepts nothing here.
        code, report = self.scan(self.payload(product_bytes({"notes.txt": b"public\n"})))
        self.assertEqual((report["accepted"]["used_unproved"], report["accepted"]["used_by_tool"]), (0, {}))
        self.assertEqual(accepted_part(self.payload(product_bytes({"notes.txt": b"public\n"})), entry(texts[0])), (0, {}))

    def test_an_accepted_structural_error_is_exact_to_the_container_digest(self):
        broken = source_archive(b"public notes\n")[:-20]
        payload = self.payload(product_bytes({"selected/provenance/demo/source.tar.gz": broken}))
        for digest, expected in ((hashlib.sha256(broken).hexdigest(), 0), ("0" * 64, 2)):
            with self.subTest(expected=expected):
                accepted = self.accepted([{"category": "structure", "rule": "corrupt-archive", "sha256": digest,
                                           "reason": "synthetic damaged third-party archive"}])
                code, report = self.scan(payload, accepted=accepted)
                self.assertEqual(code, expected)

    def test_malformed_accepted_files_are_refused(self):
        payload = self.payload(product_bytes({"notes.txt": b"public\n"}))
        good = {"category": "synthetic_marker", "rule": "synthetic-marker-1", "sha256": "0" * 64, "reason": "x",
                "accepted_by": "release owner, synthetic"}
        for entry in ({**good, "sha256": "ab"}, {**good, "reason": ""}, {**good, "accepted_by": " "},
                      {**good, "accepted_by": "two\nlines"}, {**good, "category": "structure", "rule": "member-too-large"},
                      {key: value for key, value in good.items() if key != "reason"},
                      {key: value for key, value in good.items() if key != "accepted_by"},
                      {**good, "class": ""}, {**good, "class": 3}, {**good, "proof": "text"}, {**good, "proof": []},
                      {**good, "note": "a key the schema does not have"}):
            with self.subTest(entry=entry):
                code, report = self.scan(payload, accepted=self.accepted([entry], by=None))
                self.assertEqual((code, report), (4, None))
        # the earlier schema, whose entries name nobody, is refused as a whole
        earlier = self.root / "accepted-earlier.json"
        dump(earlier, {"schema": "release-payload-privacy-scan/accepted/1", "accepted": []})
        self.assertEqual(self.scan(payload, accepted=earlier), (4, None))
        # class and proof are optional; the proof is opaque to the scan and counted in the file's digest
        proof = {"census": {"synthetic-class": 2}, "positions": [{"offset": 12, "context_sha256": "1" * 64}]}
        full = self.accepted([{**good, "class": "COMPILED-FILE-CONTEXT", "proof": proof}], by=None)
        code, report = self.scan(payload, accepted=full)
        self.assertEqual((code, report["status"], report["accepted"]["count"]), (0, "PASS", 1))
        self.assertEqual(report["accepted"]["sha256"], hashlib.sha256(full.read_bytes()).hexdigest())

    def test_a_changed_engine_is_refused(self):
        copies = self.root / "copies"
        copies.mkdir()
        shutil.copyfile(WRAPPER, copies / WRAPPER.name)
        (copies / ENGINE.name).write_bytes(ENGINE.read_bytes() + b"\n# changed\n")
        code, report = self.scan(self.payload(product_bytes({"notes.txt": b"public\n"})), wrapper=copies / WRAPPER.name)
        self.assertEqual((code, report), (4, None))

    BOUND = 1 << 16

    def bounded_scan(self, data, name="meerkat-connect-gateway-0.0.0-linux-x86_64.tar.gz"):
        """Scan with a copy of the tools whose engine bound is 64 KiB, so the split path runs on small data."""
        if not hasattr(self, "bounded"):
            self.bounded = self.tool_copy("bounded", repository=False, bound=self.BOUND)
        self.assertLess(len(data), self.BOUND)
        return self.scan(self.payload(data, name=name), wrapper=self.bounded, extra=("--piece-bytes", "16384"))

    def large_tar(self, extra=None):
        """A tar past the lowered bound: zero-filled members, then the given ones."""
        members = {f"pkg/zeros-{n}.bin": bytes(8192) for n in range(12)}
        members.update(extra or {"pkg/notes.txt": b"public notes\n"})
        raw = tar_bytes(members, compressed=False)
        self.assertGreater(len(raw), self.BOUND)
        return raw

    def test_a_stream_past_the_bound_is_cut_and_scanned_with_a_lowered_bound(self):
        marked = self.large_tar({"pkg/notes.txt": b"notes " + MARKER + b"\n"})
        for label, data in (("gzip", gzip.compress(marked, mtime=0)), ("bzip2", bz2.compress(marked)),
                            ("xz", lzma.compress(marked, format=lzma.FORMAT_XZ))):
            with self.subTest(stream=label):
                code, report = self.bounded_scan(data)
                self.assertEqual((code, report["hits"]["refused"]), (1, {"synthetic_marker": 1}))
                self.assertEqual(report["errors"], {"accepted": {}, "covered": {"member-too-large": 1}, "refused": {}})
                self.assertEqual((report["reroots"]["split"], report["wrapper_failures"]), (1, 0))
        code, report = self.bounded_scan(gzip.compress(self.large_tar(), mtime=0))
        self.assertEqual((code, report["status"], report["reroots"]["split"]), (0, "PASS", 1))

    def test_the_gzip_header_fields_of_a_stream_past_the_bound_are_scanned(self):
        raw = self.large_tar()
        half = len(raw) // 2 // 512 * 512
        cases = {
            "comment": gzip_stream(raw, comment=b"built for " + MARKER),
            "name": gzip_stream(raw, name=MARKER + b".tar"),
            "extra": gzip_stream(raw, extra=b"zq\x18\x00" + MARKER),
            "second-stream-comment": gzip_stream(raw[:half]) + gzip_stream(raw[half:], comment=MARKER),
        }
        for label, data in cases.items():
            with self.subTest(field=label):
                code, report = self.bounded_scan(data)
                self.assertEqual((code, report["hits"]["refused"]), (1, {"synthetic_marker": 1}))
                self.assertEqual(report["errors"]["covered"], {"member-too-large": 1})
                self.assertEqual(report["wrapper_failures"], 0)
        # the same fields on a stream below the bound are the engine's own to scan
        small = tar_bytes({"pkg/notes.txt": b"public notes\n"}, compressed=False)
        code, report = self.bounded_scan(gzip_stream(small, comment=b"built for " + MARKER))
        self.assertEqual((code, report["hits"]["refused"], report["reroots"]["split"]), (1, {"synthetic_marker": 1}, 0))
        # a header without a field, on a stream past the bound, adds nothing to refuse
        code, report = self.bounded_scan(gzip_stream(raw))
        self.assertEqual((code, report["status"]), (0, "PASS"))

    def test_a_stream_past_the_bound_that_cannot_be_cut_whole_refuses(self):
        raw = self.large_tar()
        cases = {
            # bytes that are not zero after the last member of the tar
            "non-zero-tail": gzip.compress(raw + b"trailing " + MARKER + b"\n" + bytes(512), mtime=0),
            # bytes after the last stream that start no further stream
            "data-after-the-stream": gzip.compress(raw, mtime=0) + b"trailing " + MARKER,
            # a stream that holds no tar
            "not-a-tar": gzip.compress(b"plain text " + MARKER + b"\n" + bytes(2 * self.BOUND), mtime=0),
        }
        for label, data in cases.items():
            with self.subTest(case=label):
                code, report = self.bounded_scan(data)
                self.assertEqual((code, report["status"], report["wrapper_failures"]), (2, "REFUSED", 1))
                self.assertIn("wrapper-failures", report["reasons"])
                # the stream stays unread, so its size error is not counted as covered
                self.assertEqual(report["errors"]["covered"], {})
                self.assertEqual(report["errors"]["refused"], {"member-too-large": 1})

    def test_a_container_the_wrapper_fails_to_decode_refuses(self):
        damaged = b"\x28\xb5\x2f\xfd" + b"not a zstd frame " * 8
        code, report = self.scan(self.payload(product_bytes({"selected/provenance/demo/data.tar.zst": damaged})))
        self.assertEqual((code, report["status"], report["wrapper_failures"]), (2, "REFUSED", 1))
        self.assertEqual(report["errors"]["covered"], {})
        self.assertEqual(report["errors"]["refused"], {"unsupported-archive": 1})
        self.assertEqual(report["reroots"]["zstd"], 0)

    def test_a_tar_is_cut_at_member_boundaries_into_pieces_that_keep_every_byte(self):
        wrapper = load("payload_scan_split", WRAPPER)
        members = {f"pkg/file-{n}.bin": bytes([n]) * (700 + 300 * n) for n in range(1, 7)}
        members["pkg/notes.txt"] = b"notes " + MARKER + b"\n"
        raw = tar_bytes(members, compressed=False)
        source = self.root / "big.tar"
        source.write_bytes(raw)
        pieces, zero_tail = wrapper.split_tar(source, 3000, self.root / "big")
        self.assertTrue(zero_tail)
        self.assertGreater(len(pieces), 2)
        bodies = [piece.read_bytes() for piece in pieces]
        for body in bodies:
            self.assertEqual(body[-1024:], b"\x00" * 1024)
        joined = b"".join(body[:-1024] for body in bodies)
        self.assertEqual(raw[:len(joined)], joined)
        self.assertEqual(raw[len(joined):].count(0), len(raw) - len(joined))
        seen = {}
        for piece in pieces:
            with tarfile.open(piece, "r:") as archive:
                for member in archive:
                    seen[member.name] = archive.extractfile(member).read()
        self.assertEqual(seen, members)
        (self.root / "tail.tar").write_bytes(raw + b"trailing bytes")
        self.assertFalse(wrapper.split_tar(self.root / "tail.tar", 3000, self.root / "tail")[1])

    @unittest.skipUnless(os.environ.get("RELEASE_PAYLOAD_SCAN_LARGE") == "1",
                         "writes and scans about 1.3 GiB; set RELEASE_PAYLOAD_SCAN_LARGE=1")
    def test_a_product_stream_past_the_engine_bound_is_cut_and_scanned(self):
        payload = self.root / "payload-large" / "meerkat-connect-gateway-0.0.0-linux-x86_64.tar.gz"
        payload.parent.mkdir()
        block = 250 << 20

        class Zeros:
            def __init__(self, size):
                self.left = size

            def read(self, n=-1):
                n = self.left if n is None or n < 0 else min(n, self.left)
                self.left -= n
                return bytes(n)

        with gzip.open(payload, "wb", compresslevel=1) as stream:
            with tarfile.open(fileobj=stream, mode="w", format=tarfile.PAX_FORMAT) as archive:
                for n in range(5):
                    info = tarfile.TarInfo(f"pkg/zeros-{n}.bin")
                    info.size = block
                    archive.addfile(info, Zeros(block))
                info = tarfile.TarInfo("pkg/notes.txt")
                info.size = len(b"notes " + MARKER + b"\n")
                archive.addfile(info, io.BytesIO(b"notes " + MARKER + b"\n"))
        code, report = self.engine_alone(payload)
        self.assertEqual((code, report["hit_total"]), (2, 0))
        self.assertEqual([row["kind"] for row in report["errors"]], ["member-too-large"])
        code, report = self.scan(payload)
        self.assertEqual(code, 1)
        self.assertEqual(report["hits"]["refused"], {"synthetic_marker": 1})
        self.assertEqual(report["errors"], {"accepted": {}, "covered": {"member-too-large": 1}, "refused": {}})
        self.assertEqual(report["reroots"]["split"], 1)


class CandidatePrivacyScan(unittest.TestCase):
    """connect-candidate.py: the local release-mode step and assemble-cli's scan of its product. Both are
    release scans: they pass the list of members a library leaves out (--non-published-list, the
    non_published_list argument of assemble_cli) and read the status PASS."""

    def setUp(self):
        self.root = Path(tempfile.mkdtemp(prefix="candidate-scan-test-")).resolve()
        self.addCleanup(shutil.rmtree, self.root, True)
        self.listed = self.root / "non-published.json"
        dump(self.listed, {"schema": "package-library-release/non-published-source-members/1", "trees": {},
                           "original_snapshots": {}})

    def test_privacy_scan_requires_rules_and_refuses_the_marker(self):
        clean = self.root / "clean" / "meerkat-connect-gateway-0.0.0-linux-x86_64.tar.gz"
        marked = self.root / "marked" / "meerkat-connect-0.0.0-linux-x86_64.tar.gz"
        for path, text in ((clean, b"public\n"), (marked, b"notes " + MARKER + b"\n")):
            path.parent.mkdir()
            path.write_bytes(product_bytes({"selected/provenance/demo/source.tar.gz": source_archive(text)}))
        done = run(CANDIDATE, "privacy-scan", "--exemptions", EXEMPTIONS, "--output", self.root / "no-rules", clean)
        self.assertNotEqual(done.returncode, 0)
        self.assertIn("--rules", done.stderr)
        self.assertFalse((self.root / "no-rules").exists())
        done = run(CANDIDATE, "privacy-scan", "--rules", RULES, "--exemptions", EXEMPTIONS,
                   "--non-published-list", self.listed, "--output", self.root / "refused", clean, marked)
        self.assertNotEqual(done.returncode, 0)
        report = json.loads((self.root / "refused/report.json").read_text())
        self.assertEqual((report["status"], report["hits"]["refused"]), ("REFUSED", {"synthetic_marker": 1}))
        self.assertEqual([row["name"] for row in report["payload"]], [clean.name, marked.name])
        done = run(CANDIDATE, "privacy-scan", "--rules", RULES, "--exemptions", EXEMPTIONS,
                   "--non-published-list", self.listed, "--output", self.root / "clean-scan", clean)
        self.assertEqual(done.returncode, 0, done.stderr)
        self.assertEqual(json.loads((self.root / "clean-scan/report.json").read_text())["status"], "PASS")
        self.assertEqual((self.root / "clean-scan/detail.json").stat().st_mode & 0o777, 0o600)
        self.assertEqual(json.loads((self.root / "clean-scan/refused.json").read_text())["hits"], [])
        self.assertEqual(len(json.loads((self.root / "refused/refused.json").read_text())["hits"]), 1)

    def producer(self, library):
        """The CLI producer of test_connect_candidate_cli, with a real nested library archive."""
        p = load("candidate_payload_scan", CANDIDATE)
        f = load("inert_assembler_payload_scan", HERE / "test_assemble_connect_product.py")
        root = self.root / ("producer-" + hashlib.sha256(library).hexdigest()[:8])
        root.mkdir()
        original = f.InertInputs(root, TARGET)
        embedded = original.resources / "archives/inert-library.tar.gz"
        for directory in (original.resources, embedded.parent):
            directory.chmod(0o755)
        embedded.chmod(0o644)
        embedded.write_bytes(library)
        original.select_explicit()
        work = root / "producer"
        cli = work / "cli"
        cli.mkdir(parents=True)
        resources = work / "resources/build.json"
        resources.parent.mkdir()
        shutil.copyfile(original.resource_record, resources)
        shutil.copyfile(original.component, resources.parent / ("meerkat-connect-resources-" + TARGET + ".tar.gz"))
        original.resource_record = resources
        binary = cli / "binary/meerkat-connect"
        binary.parent.mkdir()
        shutil.copyfile(original.binary, binary)
        original.binary = binary
        original.record_value["binary"] = p.binding(binary)
        build = cli / "build-record.json"
        dump(build, original.record_value)
        dump(cli / "stage/staging.json", {"candidate": original.composition_value["candidate"],
             "build_record_sha256": p.digest(build), "resource_build_record_sha256": p.digest(resources)})
        shutil.copyfile(original.composition, cli / "installed-composition.json")
        pending = copy.deepcopy(original.shutdown_value)
        pending["disposition"] = "pending"
        dump(cli / "native-shutdown-review.pending.json", pending)
        value = {"target": TARGET, "connect_version": "1.2.3"}
        dump(work / "inputs-selection.json", value)

        def recorded(recorder, name, argv, **kwargs):
            original.record = Path(argv[argv.index("--build-record") + 1])
            original.assemble(destination=Path(argv[argv.index("--destination") + 1]), release_version="1.2.3")
            assert released == [original.record.read_bytes()]
            return {}, None

        # A versioned assembly also requires the monorepo source identity of the
        # checkout it runs in (the assembler's own tests cover that check). This
        # synthetic producer has no Git checkout, so a stand-in records the call,
        # as in test_connect_candidate_cli.
        released = []

        def release_source(raw):
            released.append(raw)
            return {"repository": "lukacf/meerkat", "revision": "a" * 40, "tree": "b" * 40,
                    "source_manifest_sha256": p.digest(original.source)}

        patches = (mock.patch.object(p, "cli_producer_inputs", lambda supplied: (value, original.source, resources)),
                   mock.patch.object(p.Recorder, "run", recorded),
                   mock.patch.object(f.ASSEMBLER, "checked_release_source", release_source))
        for patch in patches:
            patch.start()
            self.addCleanup(patch.stop)
        return p, original, work, root / "final"

    def test_assemble_cli_without_rules_refuses_before_assembly(self):
        p, original, work, output = self.producer(library_bytes(
            {"selected/provenance/demo/source.tar.gz": source_archive(b"notes " + MARKER + b"\n")}))
        with self.assertRaises(ValueError) as refused:
            p.assemble_cli(work, original.shutdown, p.digest(original.shutdown), output)
        self.assertIn("--rules", str(refused.exception))
        self.assertFalse(output.exists())

    def test_assemble_cli_refuses_a_product_with_the_nested_marker(self):
        p, original, work, output = self.producer(library_bytes(
            {"selected/provenance/demo/source.tar.gz": source_archive(b"notes " + MARKER + b"\n")}))
        with self.assertRaises(ValueError) as refused:
            p.assemble_cli(work, original.shutdown, p.digest(original.shutdown), output,
                           rules=RULES, exemptions=EXEMPTIONS, non_published_list=self.listed)
        self.assertIn("privacy scan", str(refused.exception))
        self.assertFalse((output / "assembled").exists())
        report = json.loads((output / "privacy-scan/report.json").read_text())
        self.assertEqual((report["status"], report["hits"]["refused"]), ("REFUSED", {"synthetic_marker": 1}))

    def test_assemble_cli_keeps_a_clean_product_with_its_report(self):
        p, original, work, output = self.producer(library_bytes(
            {"selected/provenance/demo/source.tar.gz": source_archive(b"public notes\n")}))
        p.assemble_cli(work, original.shutdown, p.digest(original.shutdown), output, rules=RULES, exemptions=EXEMPTIONS,
                       non_published_list=self.listed)
        final = p.read_json(output / "assembled/assembly.json")
        report = json.loads((output / "privacy-scan/report.json").read_text())
        self.assertEqual(report["status"], "PASS")
        self.assertEqual([row["name"] for row in report["payload"]], [final["archive"], final["archive"] + ".sha256"])
        self.assertEqual(report["payload"][0]["sha256"], final["archive_sha256"])


class ReleaseScanGitEnvironment(unittest.TestCase):
    """A release scan finds its commit from where the tool is. Git variables in the caller's
    environment do not choose it. The helpers are those of PayloadScan; none of its rows runs here."""

    setUp = PayloadScan.setUp
    payload = PayloadScan.payload
    non_published = PayloadScan.non_published
    scan = PayloadScan.scan
    tool_copy = PayloadScan.tool_copy

    def git(self, tool, *command):
        done = subprocess.run(["git", "-C", str(tool.parent.parent), "-c", "user.name=test",
                               "-c", "user.email=test@example.invalid", "-c", "commit.gpgsign=false", *command],
                              check=True, capture_output=True, text=True)
        return done.stdout.strip()

    def test_git_variables_of_the_caller_do_not_choose_the_commit_a_release_scan_names(self):
        payload = self.payload(product_bytes({"selected/notes.txt": b"public notes\n"}))
        committed = self.tool_copy("committed")
        loose = self.tool_copy("loose", repository=False)
        # Another repository that holds the same two files, at a commit of its own.
        elsewhere = self.tool_copy("elsewhere")
        self.git(elsewhere, "commit", "-q", "--allow-empty", "-m", "another commit")
        head, other = self.git(committed, "rev-parse", "HEAD"), self.git(elsewhere, "rev-parse", "HEAD")
        self.assertNotEqual(head, other)
        variables = {"GIT_DIR": str(elsewhere.parent.parent / ".git"), "GIT_WORK_TREE": str(loose.parent.parent)}
        with mock.patch.dict(os.environ, variables):
            # A tool in no repository stays without a commit: the other repository's is not taken.
            self.assertEqual(self.scan(payload, wrapper=loose, release=True), (4, None))
            # A committed tool names its own checkout's commit.
            code, report = self.scan(payload, wrapper=committed, release=True)
        self.assertEqual((code, report["status"], report["release"], report["tool_commit"]), (0, "PASS", True, head))
        with mock.patch.dict(os.environ, {"GIT_DIR": str(self.root / "no-such-repository")}):
            code, report = self.scan(payload, wrapper=committed, release=True)
        self.assertEqual((code, report["tool_commit"]), (0, head))

    def test_a_replace_ref_does_not_make_an_edited_tool_the_commits(self):
        # git can be told to read another object in place of one (a replace ref). The scan asks for the
        # commit's own objects, so a tool edited after the commit is not that commit's tool.
        payload = self.payload(product_bytes({"selected/notes.txt": b"public notes\n"}))
        tool = self.tool_copy("replaced")
        head = self.git(tool, "rev-parse", "HEAD")
        with open(tool, "a") as stream:
            stream.write("\n# edited after the commit\n")
        self.git(tool, "add", "-A")
        other = self.git(tool, "commit-tree", self.git(tool, "write-tree"), "-m", "the edited tool")
        self.git(tool, "replace", head, other)
        # Asked without more, git now reads the other commit as the checkout's: nothing looks changed.
        self.assertEqual((self.git(tool, "rev-parse", "HEAD"), self.git(tool, "status", "--porcelain")), (head, ""))
        self.assertEqual(self.scan(payload, wrapper=tool, release=True), (4, None))
        # Without the replace ref the same checkout refuses for the same reason, and the committed tool passes.
        self.git(tool, "replace", "-d", head)
        self.assertEqual(self.scan(payload, wrapper=tool, release=True), (4, None))
        code, report = self.scan(payload, wrapper=self.tool_copy("not-replaced"), release=True)
        self.assertEqual((code, report["status"]), (0, "PASS"))


class ReferenceCheckLimits(unittest.TestCase):
    """What the check for references to members a library leaves out reads, and what it does not: it
    compares the listed digests as hexadecimal text. The helpers are those of PayloadScan."""

    setUp = PayloadScan.setUp
    payload = PayloadScan.payload
    non_published = PayloadScan.non_published
    scan = PayloadScan.scan

    def test_a_listed_digest_is_found_as_hexadecimal_text_and_a_listed_path_is_not_looked_for(self):
        import base64
        raw = hashlib.sha256(b"synthetic withheld member").digest()
        digest = raw.hex()
        listed = self.non_published([digest])
        name = json.loads(listed.read_text())["trees"]["selected/provenance/source/"][0]["name"]
        self.assertEqual(name, "crates/sample/withheld-0.txt")
        # Read: the 64 characters, in either case, as they are or behind an escape the engine decodes.
        found = {"selected/lower.txt": ("pinned " + digest + "\n").encode(),
                 "selected/upper.txt": ("pinned " + digest.upper() + "\n").encode(),
                 "selected/escaped.json": json.dumps({"sha256": digest}).replace("a", "\\u0061").encode()}
        self.assertIn(b"\\u0061", found["selected/escaped.json"])
        code, report = self.scan(self.payload(product_bytes(found)), extra=("--non-published-list", listed))
        self.assertEqual((code, report["status"]), (1, "REFUSED"))
        self.assertEqual(report["hits"]["refused"], {"withheld_member_reference": len(found)})
        # Not read (a stated limit): the member's path alone, and a digest in any other written form.
        pairs = ":".join(digest[index:index + 2] for index in range(0, 64, 2))
        other = {"selected/path-only.txt": ("see " + name + "\n").encode(),
                 "selected/split.txt": (digest[:32] + "\n" + digest[32:] + "\n").encode(),
                 "selected/pairs.txt": pairs.encode() + b"\n",
                 "selected/shortened.txt": digest[:40].encode() + b"\n",
                 "selected/base64.txt": base64.b64encode(raw) + b"\n",
                 "selected/raw.bin": b"\x00" + raw + b"\x00"}
        code, report = self.scan(self.payload(product_bytes(other)), extra=("--non-published-list", listed))
        self.assertEqual((code, report["status"], report["hits"]["refused"]), (0, "PASS", {}))


if __name__ == "__main__":
    unittest.main()
