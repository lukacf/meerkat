"""Upload receipt: written from two passing scan reports, and checked before an upload.

Synthetic reports and receipts only: made-up digests, commits and names. One test also reads a report
that the credential scan itself wrote for a made-up payload, with the stand-in detector of its own
tests.

The check takes what it holds a receipt to from the checkout it runs from: the pins of the target
and the coverage exceptions file, as the checkout's commit holds them. So every test runs a copy of
the check in a made-up checkout (a scratch git repository) whose commit holds made-up pins.
"""
from pathlib import Path
import contextlib
import hashlib
import importlib.util
import io
import json
import os
import re
import shutil
import stat
import subprocess
import sys
import tempfile
import time
import unittest

HERE = Path(__file__).resolve().parent
CHECK = HERE / "release-upload-receipt-check.py"
WRITE = HERE / "release-upload-receipt-write.py"
CONVERTER = HERE / "privacy-rules-from-names.py"
TARGET, OTHER_TARGET = "linux-x86_64", "macos-arm64"
NAME_LIST, RULES = "a1" * 32, "8b" * 32
PINNED, CURRENT = "b2" * 32, "c3" * 32
# The default rules file each made-up build reads with.
PINNED_RULES, CURRENT_RULES = "5e" * 32, "4d" * 32
NON_PUBLISHED = "d5" * 32
RECEIPT_SCHEMA = "release-upload-receipt/4"
# The made-up tools whose proofs the made-up acceptance entries carry, with the entries each proved.
PROOF_TOOLS = {"zq-prover-one": 1, "zq-prover-two": 1}
PINS_NAME = "upload-receipt-pins.json"
PINS_SCHEMA = "release-upload-receipt-check/pins/3"
EXCEPTIONS_NAME = "payload-scan-coverage-exceptions.json"
EMPTY_LIST_NAME = "library-non-published-source-members-empty.json"
# The pins of one target: what a release scan of that target's files must have run with.
PINS = {"name_list_sha256": NAME_LIST, "rules_sha256": RULES, "non_published_list_sha256": NON_PUBLISHED,
        "detector_pinned_sha256": PINNED, "detector_current_sha256": CURRENT,
        "detector_pinned_default_rules_sha256": PINNED_RULES, "detector_current_default_rules_sha256": CURRENT_RULES}
# The pins of the schema before this one: the builds were pinned, the rules file each read with was not.
FORMER_PINS = {name: value for name, value in PINS.items() if "default_rules" not in name}
OTHER_PINS = {name: "f0" * 32 if name == "detector_current_sha256" else "e9" * 32 for name in PINS}
# The name rule of a target: its files hold its name, or they are exactly these names.
HOLDS = "holds-target"
RELEASE = "release"
RELEASE_NAMES = ["RELEASE-MANIFEST.json", "SHA256SUMS", "source-manifest.json",
                 "meerkat_toolkit-<version>-py3-none-any.whl", "rkat-toolkit-<version>.tgz"]
VERSION = "1.2.3"
# Where a receipt states what each pin fixes.
PIN_FIELDS = {"name_list_sha256": "privacy.name_list_sha256", "rules_sha256": "privacy.rules_sha256",
              "non_published_list_sha256": "privacy.non_published_list_sha256",
              "detector_pinned_sha256": "credential.detector_pinned_sha256",
              "detector_current_sha256": "credential.detector_current_sha256",
              "detector_pinned_default_rules_sha256": "credential.detector_pinned_default_rules_sha256",
              "detector_current_default_rules_sha256": "credential.detector_current_default_rules_sha256"}
# Every option that once gave the check a pin, a full report or an acceptance input.
FORMER_OPTIONS = tuple(f"--{name}-{form}" for name in ("name-list", "non-published-list", "detector-pinned", "detector-current",
                                                       "privacy-accepted", "credential-accepted", "credential-pending")
                       for form in ("sha256", "file")) + ("--privacy-report", "--credential-report")
# A made-up coverage exceptions file, as a checkout commits it.
EXCEPTIONS = json.dumps({"schema": "release-payload-privacy-scan/coverage-exceptions/1", "exceptions": []}, indent=2) + "\n"
GIT = ("-c", "user.name=test", "-c", "user.email=test@example.invalid", "-c", "commit.gpgsign=false",
       "-c", "core.hooksPath=/dev/null")
DROP = object()
# The two scan tools of the made-up release, as its commit holds them: made-up files under the
# scans' own names. A report states the sha256 of the tool that wrote it.
SCAN_TOOLS = {"release-payload-privacy-scan.py": b"# a made-up private-text scan tool\n",
              "release-payload-credential-scan.py": b"# a made-up credential scan tool\n"}
PRIVACY_TOOL, CREDENTIAL_TOOL = (hashlib.sha256(data).hexdigest() for data in SCAN_TOOLS.values())


def load(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    value = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(value)
    return value


def sha256(data):
    return hashlib.sha256(data if isinstance(data, bytes) else data.encode()).hexdigest()


def stamp(age_seconds=60):
    return time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime(time.time() - age_seconds))


def git(directory, *args):
    env = {key: value for key, value in os.environ.items() if not key.startswith("GIT_")}
    done = subprocess.run(["git", "-C", str(directory), *GIT, *args], capture_output=True, text=True, env=env)
    if done.returncode != 0:
        raise AssertionError("git %s: %s" % (args[0], done.stderr))
    return done.stdout.strip()


def section(pins, names=HOLDS):
    """One target's part of the pins file: its name rule and its pins."""
    return {"names": names, **pins}


def pins_text(targets=None):
    """The pins file's text for {target: section or pins}; by default the two made-up platform targets
    and the target of the release's own files, which one scan run covers together with the first
    platform's files. A target given as pins alone gets the rule that its files hold its name."""
    targets = {TARGET: PINS, OTHER_TARGET: OTHER_PINS, RELEASE: section(PINS, RELEASE_NAMES)} if targets is None else targets
    targets = {target: value if isinstance(value, dict) and "names" in value else section(value) if isinstance(value, dict) else value
               for target, value in targets.items()}
    return json.dumps({"schema": PINS_SCHEMA, "targets": targets}, indent=2) + "\n"


def scan_tools(root):
    """The two made-up scan tools under scripts/ of a made-up checkout at root."""
    (root / "scripts").mkdir(parents=True, exist_ok=True)
    for name, data in SCAN_TOOLS.items():
        (root / "scripts" / name).write_bytes(data)


def release_commit(root):
    """The made-up release commit as the first commit of a made-up checkout at root: it holds the two
    scan tools and nothing else. Its names, dates and message are fixed, so it is the same commit in
    every checkout these rows make, and the reports of the made-up scans name it."""
    scan_tools(root)
    env = {key: value for key, value in os.environ.items() if not key.startswith("GIT_")}
    env.update(GIT_AUTHOR_DATE="2000-01-01T00:00:00+0000", GIT_COMMITTER_DATE="2000-01-01T00:00:00+0000")
    for args in (("init", "-q"), ("add", "--", "scripts"), ("commit", "-q", "-m", "the made-up release commit")):
        subprocess.run(["git", "-C", str(root), *GIT, *args], check=True, capture_output=True, env=env)
    return git(root, "rev-parse", "HEAD")


def made_up_release_commit():
    root = Path(tempfile.mkdtemp(prefix="receipt-check-release-commit-"))
    try:
        return release_commit(root)
    finally:
        shutil.rmtree(root, True)


COMMIT = made_up_release_commit()


def commit(root):
    git(root, "init", "-q")
    git(root, "add", "-A")
    git(root, "commit", "-q", "-m", "the tool and what it holds receipts to")


def checkout(root, pins=PINS, committed=True, exceptions=EXCEPTIONS):
    """A made-up checkout at root: the made-up release commit (the two scan tools) and, in one commit
    on top of it, a copy of the check and of the writer under scripts/ and, under release/, the pins
    file (pins: the first target's pins as a mapping, the whole file's text, or None for no file) and
    the coverage exceptions file (None for no file). Not committed: the same files in plain
    directories. Returns the check there; the writer is the file beside it."""
    root.mkdir(parents=True)
    if committed:
        if release_commit(root) != COMMIT:
            raise AssertionError("the made-up release commit is not the same commit in every checkout")
    else:
        scan_tools(root)
    (root / "release").mkdir()
    shutil.copyfile(CHECK, root / "scripts" / CHECK.name)
    shutil.copyfile(WRITE, root / "scripts" / WRITE.name)
    if pins is not None:
        text = pins if isinstance(pins, str) else pins_text({TARGET: pins, OTHER_TARGET: OTHER_PINS,
                                                             RELEASE: section(PINS, RELEASE_NAMES)})
        (root / "release" / PINS_NAME).write_text(text)
    if exceptions is not None:
        (root / "release" / EXCEPTIONS_NAME).write_text(exceptions)
    if committed:
        commit(root)
    return root / "scripts" / CHECK.name


def release_file(tool, name):
    return tool.parent.parent / "release" / name


def run(tool, *args, env=None):
    return subprocess.run([sys.executable, "-I", str(tool), *map(str, args)], capture_output=True, text=True,
                          env=None if env is None else {**os.environ, **env})


def change(document, path, value):
    """A copy of document with the field at the dotted path set to value, or removed when value is DROP."""
    copy = json.loads(json.dumps(document))
    target = copy
    *parents, last = path.split(".")
    for key in parents:
        target = target[int(key)] if isinstance(target, list) else target[key]
    if value is DROP:
        del target[last]
    elif isinstance(target, list):
        target[int(last)] = value
    else:
        target[last] = value
    return copy


class Fixture(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.base = Path(tempfile.mkdtemp(prefix="receipt-check-checkout-")).resolve()
        cls.addClassCleanup(shutil.rmtree, cls.base, True)
        # The checkout most tests run the check from: its commit holds the made-up pins of two targets.
        cls.tool = checkout(cls.base / "checkout")

    def setUp(self):
        self.root = Path(tempfile.mkdtemp(prefix="receipt-check-test-")).resolve()
        self.addCleanup(shutil.rmtree, self.root, True)
        self.count = 0
        staging = self.root / "staging"
        staging.mkdir()
        self.files = []
        for name, data in ((f"product-{VERSION}-{TARGET}.tar.gz", b"made-up product bytes\n"),
                           (f"product-{VERSION}-{TARGET}.tar.gz.sha256", b"made-up checksum line\n")):
            (staging / name).write_bytes(data)
            self.files.append(staging / name)

    def checkout(self, pins=PINS, committed=True, exceptions=EXCEPTIONS):
        """Another made-up checkout, of this test alone. Returns the check there."""
        self.count += 1
        return checkout(self.root / f"checkout-{self.count}", pins, committed, exceptions)

    def payload(self, files=None):
        return [{"name": path.name, "bytes": path.stat().st_size, "sha256": sha256(path.read_bytes())}
                for path in (self.files if files is None else files)]

    def named(self, *names, directory="named"):
        """Made-up files with these names, in a directory of their own."""
        (self.root / directory).mkdir(exist_ok=True)
        files = []
        for name in names:
            files.append(self.root / directory / name)
            files[-1].write_bytes(("made-up bytes of " + name + "\n").encode())
        return files

    def release_files(self, version=VERSION):
        """The five files of a release that hold no platform's name, for a version."""
        return self.named(*(name.replace("<version>", version) for name in RELEASE_NAMES), directory="release-" + version)

    # The two full reports, as the scans write them: with what a receipt leaves behind.
    def privacy(self):
        return {"schema": "release-payload-privacy-scan/report/3", "status": "PASS", "reasons": [], "release": True,
                "nonce": "0f" * 16,
                "label": None, "generated_at": stamp(), "tool_commit": COMMIT, "tool_sha256": PRIVACY_TOOL,
                # The engine saw hits (1) and errors (2): the report shows where each of them went.
                "engine": {"sha256": "6f" * 32, "exit_code": 3},
                "rules": {"sha256": RULES, "total": 6, "blocking": 5, "review_outside_paths": 0}, "name_list_sha256": NAME_LIST,
                "non_published_list_sha256": NON_PUBLISHED,
                "exemptions": {"count": 0, "sha256": "d4" * 32, "blocking": 0},
                "accepted": {"count": 2, "sha256": "e5" * 32, "used": {"member": 2, "archive": 0}, "used_unproved": 0,
                             "used_by_tool": dict(PROOF_TOOLS)},
                "coverage_exceptions": {"count": 0, "sha256": sha256(EXCEPTIONS), "used": {}}, "payload": self.payload(),
                "hits": {"refused": {}, "accepted": {"madeupclass": 2}}, "review_hits": {"review_madeupclass": 4},
                "errors": {"covered": {"made-up-kind": 1}, "accepted": {"corrupt-archive": 1}, "refused": {}}, "containers": 3,
                "coverage": {"files": 2, "members": 9, "bytes": 44},
                "reroots": {"depth": 0}, "wrapper_failures": 0, "exit_code": 0}

    def credential(self):
        return {"schema": "release-payload-credential-scan/report/1", "name": "X0", "label": "X0", "release": True,
                "status": "PASS", "generated_at": stamp(), "tool_commit": COMMIT, "tool_sha256": CREDENTIAL_TOOL,
                "receipt": "CREDENTIAL-LEG PASS X0 files=2", "payload": self.payload(),
                "detector_builds": [{"role": "pinned", "version": "0.0.1", "sha256": PINNED, "default_rules_sha256": PINNED_RULES},
                                    {"role": "current", "version": "0.0.2", "sha256": CURRENT, "default_rules_sha256": CURRENT_RULES}],
                "accepted": {"count": 3, "sha256": "a7" * 32, "positions": 3}, "pending": {"count": 0, "sha256": None},
                "findings": {"total": 3, "refused": 0, "pending_owner": 0, "by_rule": {"made-up-rule": {"accepted position": 3}}},
                "unread": {"total": 4, "unaccepted": 0, "files": {"made-up reason": 4}},
                "read_proof": {version: {"handed": 9, "read_as_it_is": 8, "read_behind_the_line": 1, "unread": 0, "readings": 2,
                                         "proof_readings": 2} for version in ("0.0.1", "0.0.2")}}

    def receipt(self, files=None):
        """A passing receipt for the files, as the writer writes it for reports that ran with the class
        checkout's pins."""
        scan = {"status": "PASS", "release": True, "generated_at": stamp(), "tool_commit": COMMIT}
        return {"schema": RECEIPT_SCHEMA, "target": TARGET, "payload": self.payload(files),
                "privacy": {**scan, "tool_sha256": PRIVACY_TOOL, "report_sha256": "1b" * 32, "exit_code": 0, "rules_sha256": RULES,
                            "name_list_sha256": NAME_LIST, "non_published_list_sha256": NON_PUBLISHED,
                            "coverage_exceptions_sha256": sha256(EXCEPTIONS), "blocking_rules": True,
                            "name_rules_all_blocking": True, "no_exemptions": True, "accepted_only_with_proof": True,
                            "accepted_proof_tools": dict(PROOF_TOOLS)},
                "credential": {**scan, "tool_sha256": CREDENTIAL_TOOL, "report_sha256": "2c" * 32, "detector_pinned_sha256": PINNED,
                               "detector_current_sha256": CURRENT, "detector_pinned_default_rules_sha256": PINNED_RULES,
                               "detector_current_default_rules_sha256": CURRENT_RULES, "nothing_refused": True, "nothing_pending": True,
                               "nothing_unread_unaccepted": True}}

    def write(self, name, value):
        self.count += 1
        path = self.root / f"{name}-{self.count}.json"
        path.write_text(value if isinstance(value, str) else json.dumps(value) + "\n")
        return path

    def check(self, receipt=None, files=None, commit=COMMIT, target=TARGET, extra=(), tool=None, env=None, version=VERSION):
        """Run the check of a checkout (the class's, unless tool is given), for the made-up release's
        version unless another or none (None) is given. No pin is ever passed."""
        receipt = self.receipt() if receipt is None else receipt
        receipt = receipt if isinstance(receipt, Path) else self.write("upload-receipt", receipt)
        commit = [] if commit is None else ["--release-commit", commit]
        target = [] if target is None else ["--target", target]
        version = [] if version is None else ["--version", version]
        return run(self.tool if tool is None else tool, "--receipt", receipt, *commit, *target, *version, *extra,
                   *(self.files if files is None else files), env=env)

    def passed(self, done):
        self.assertEqual((done.returncode, done.stderr), (0, ""), done.stdout)
        self.assertTrue(done.stdout.strip().splitlines()[-1].startswith("UPLOAD-CHECK PASS files="), done.stdout)

    def refused(self, done, *words):
        self.assertEqual(done.returncode, 1, done.stdout + done.stderr)
        self.assertNotIn("UPLOAD-CHECK PASS", done.stdout)
        self.assertIn("UPLOAD-CHECK REFUSED", done.stdout)
        for word in words:
            self.assertIn(word, done.stdout)

    def usage(self, done):
        self.assertEqual(done.returncode, 2, done.stdout + done.stderr)
        self.assertNotIn("UPLOAD-CHECK PASS", done.stdout)


class ReceiptCheck(Fixture):
    def test_a_passing_receipt_for_the_exact_files_passes(self):
        receipt = self.write("upload-receipt", self.receipt())
        done = self.check(receipt)
        self.assertEqual((done.returncode, done.stderr), (0, ""), done.stdout)
        line = done.stdout.strip().splitlines()[-1]
        self.assertTrue(line.startswith("UPLOAD-CHECK PASS files=2 "), line)
        for path in self.files:
            self.assertIn("%s=%s:%d" % (path.name, sha256(path.read_bytes()), path.stat().st_size), line)
        # The pass line names the release commit, the receipt and the two full reports it was written
        # from, and what judged it: the target, the pins file's digest and the commit of the checkout
        # the pins were read from.
        head = git(self.tool.parent, "rev-parse", "HEAD")
        self.assertRegex(head, r"\A[0-9a-f]{40}\Z")
        for word in ("release_commit=" + COMMIT, "receipt=" + sha256(receipt.read_bytes()), "privacy_report=" + "1b" * 32,
                     "credential_report=" + "2c" * 32, "target=" + TARGET, "pins=" + sha256(pins_text()), "pins_commit=" + head):
            self.assertIn(word, line.split())

    def test_the_files_are_exactly_the_receipts_rows(self):
        # Same count, each file one row, each row one file. A caller cannot check a part of a receipt,
        # and a receipt cannot vouch for a file that is not given.
        other = {"name": f"other-{TARGET}.bin", "bytes": 3, "sha256": "9" * 64}
        more = change(self.receipt(), "payload", self.payload() + [other])
        self.refused(self.check(more), "receipt", "payload", other["name"])
        for part in (self.files[:1], self.files[1:]):
            left_out = [path.name for path in self.files if path not in part]
            self.refused(self.check(files=part), "receipt", "payload", *left_out)
            done = self.check(self.receipt(part), files=part)
            self.passed(done)
            self.assertIn("files=1", done.stdout.split())
        # A row twice, with the same or with other bytes, is not one row for one file.
        rows = self.payload()
        for twice in (rows + rows[:1], rows + [{**rows[0], "bytes": rows[0]["bytes"] + 1}], rows[:1] + rows[:1]):
            self.refused(self.check(change(self.receipt(), "payload", twice)), "receipt", "payload")
        # As many rows as files, one of them for another file.
        self.refused(self.check(change(self.receipt(), "payload", rows[:1] + [other])), "receipt", "payload", self.files[1].name)

    def test_each_scan_must_say_pass_and_be_a_release_run(self):
        for which in ("privacy", "credential"):
            for status in ("clean", "REFUSED", "pass", "PROOF-ONLY", "", None, True):
                self.refused(self.check(change(self.receipt(), which + ".status", status)), which, "status")
            # A run that was not a release run is no receipt, whatever its status says.
            for value in (False, None, "true", 1):
                self.refused(self.check(change(self.receipt(), which + ".release", value)), which, "release")
        for value in (1, 4, None, False, "0"):
            self.refused(self.check(change(self.receipt(), "privacy.exit_code", value)), "privacy", "exit_code")

    def test_a_missing_unreadable_or_proof_only_receipt_refuses(self):
        self.refused(self.check(self.root / "absent.json"), "receipt", "missing")
        self.refused(self.check(self.write("broken", "{not json")), "receipt", "JSON")
        self.refused(self.check(self.write("list", [])), "receipt")
        self.refused(self.check(self.write("twice", json.dumps(self.receipt()).replace('"target"', '"target": "%s", "target"' % TARGET, 1))),
                     "receipt", "JSON")
        # A receipt file named PROOF-ONLY anywhere in its name, and one that names such a file.
        for name in ("PROOF-ONLY-upload-receipt.json", "upload-receipt-PROOF-ONLY.json"):
            proof = self.root / name
            proof.write_text(json.dumps(self.receipt()))
            self.refused(self.check(proof), "receipt", "PROOF-ONLY")
        named = [{**row, "name": "PROOF-ONLY-" + row["name"]} for row in self.payload()]
        self.refused(self.check(change(self.receipt(), "payload", named)), "receipt", "PROOF-ONLY")
        for schema in ("release-upload-receipt/1", "release-upload-receipt/2", "release-upload-receipt/3", "release-upload-receipt/5",
                       "release-payload-privacy-scan/report/3", None):
            self.refused(self.check(change(self.receipt(), "schema", schema)), "receipt", RECEIPT_SCHEMA)
        # A receipt as it was written before the two newer facts existed is not one.
        former = change(change(self.receipt(), "schema", "release-upload-receipt/2"), "privacy.accepted_only_with_proof", DROP)
        former = change(former, "privacy.accepted_proof_tools", DROP)
        self.refused(self.check(former), "receipt", RECEIPT_SCHEMA)
        # Nor is one as it was written before a receipt stated the rules file each build read with:
        # under its own schema, and under this one without those two digests.
        former = change(change(self.receipt(), "credential.detector_pinned_default_rules_sha256", DROP),
                        "credential.detector_current_default_rules_sha256", DROP)
        self.refused(self.check(change(former, "schema", "release-upload-receipt/3")), "receipt", RECEIPT_SCHEMA)
        self.refused(self.check(former), "credential", "not exactly the fields")

    def test_a_receipt_holds_exactly_its_fields_and_a_full_report_is_none(self):
        receipt = self.receipt()
        # Nothing rides along: one field more, anywhere, and it is not a receipt.
        for path, value in (("label", "X0"), ("privacy.rules", {"total": 7, "blocking": 5}), ("privacy.hits", {"madeupclass": 2}),
                            ("privacy.exemptions", {"count": 0}), ("credential.findings", {"total": 3, "refused": 0}),
                            ("credential.accepted", {"count": 3}), ("payload.0.path", "made-up/place")):
            with self.subTest(more=path):
                self.refused(self.check(change(receipt, path, value)))
        # One field less, and it is not one either.
        for path in ("target", "payload", "privacy", "credential", "privacy.status", "privacy.release", "privacy.exit_code",
                     "privacy.generated_at", "privacy.tool_commit", "privacy.tool_sha256", "privacy.report_sha256",
                     "privacy.rules_sha256", "privacy.name_list_sha256", "privacy.non_published_list_sha256",
                     "privacy.coverage_exceptions_sha256", "privacy.blocking_rules", "privacy.name_rules_all_blocking",
                     "privacy.no_exemptions", "privacy.accepted_only_with_proof", "privacy.accepted_proof_tools",
                     "credential.status", "credential.release",
                     "credential.generated_at", "credential.tool_commit", "credential.tool_sha256", "credential.report_sha256",
                     "credential.detector_pinned_sha256", "credential.detector_current_sha256",
                     "credential.detector_pinned_default_rules_sha256", "credential.detector_current_default_rules_sha256",
                     "credential.nothing_refused",
                     "credential.nothing_pending", "credential.nothing_unread_unaccepted", "payload.0.sha256", "payload.1.bytes"):
            with self.subTest(less=path):
                self.refused(self.check(change(receipt, path, DROP)))
        for field in ("tool_sha256", "report_sha256"):
            self.refused(self.check(change(receipt, "privacy." + field, "short")), "privacy", field)
            self.refused(self.check(change(receipt, "credential." + field, None)), "credential", field)
        # A full report is not a receipt: as the document, or in a scan's place.
        for report in (self.privacy(), self.credential()):
            self.refused(self.check(report), "receipt", "full report")
        self.refused(self.check(change(receipt, "privacy", self.privacy())), "privacy", "full report")
        self.refused(self.check(change(receipt, "credential", self.credential())), "credential", "full report")
        self.refused(self.check(change(change(receipt, "privacy", receipt["credential"]), "credential", receipt["privacy"])))
        # The former options for full reports and acceptance inputs are gone from the check.
        for option in ("--privacy-report", "--credential-report", "--privacy-accepted-file"):
            done = self.check(extra=[option, self.write("privacy-report", self.privacy())])
            self.usage(done)
            self.assertIn("unrecognized arguments", done.stderr)

    def test_a_stale_or_undated_receipt_refuses(self):
        for which in ("privacy", "credential"):
            for value in (stamp(25 * 3600), stamp(-3600), "2026-01-01", "yesterday", None, 0):
                self.refused(self.check(change(self.receipt(), which + ".generated_at", value)), which, "generated_at")
            two_hours = change(self.receipt(), which + ".generated_at", stamp(2 * 3600))
            self.assertEqual(self.check(two_hours).returncode, 0)
            self.refused(self.check(two_hours, extra=["--max-age-hours", "1"]), which, "generated_at")

    def test_the_age_bound_can_be_made_shorter_and_never_longer(self):
        # The bound is the tool's: a caller may ask for less than 24 hours, never for more.
        old = change(change(self.receipt(), "privacy.generated_at", stamp(30 * 3600)), "credential.generated_at", stamp(30 * 3600))
        self.refused(self.check(old), "generated_at")
        for hours in ("24.5", "48", "8784", "0", "-1", "nan", "inf"):
            self.usage(self.check(old, extra=["--max-age-hours", hours]))
            self.usage(self.check(extra=["--max-age-hours", hours]))
        self.passed(self.check(extra=["--max-age-hours", "24"]))

    def test_the_tool_commit_must_be_the_release_commit(self):
        for which in ("privacy", "credential"):
            for value in ("d" * 40, COMMIT[:12], COMMIT.upper(), None):
                self.refused(self.check(change(self.receipt(), which + ".tool_commit", value)), which, "tool_commit")
        # The release commit is the caller's to name, as a full id.
        self.usage(self.check(commit=None))
        self.usage(self.check(commit=COMMIT[:12]))

    def test_the_files_are_bound_by_name_size_and_digest(self):
        receipt = self.receipt()
        for path, value in (("payload.0.sha256", "0" * 64), ("payload.0.bytes", 1), ("payload.1.name", "other.sha256"),
                            ("payload", self.payload()[:1]), ("payload", []), ("payload.0.bytes", "22"), ("payload", "rows")):
            self.refused(self.check(change(receipt, path, value)), "receipt", "payload")
        # A file changed after the scans: the receipt's rows are not the file as it is now.
        before = self.receipt()
        with open(self.files[0], "ab") as stream:
            stream.write(b"changed\n")
        self.refused(self.check(before), "receipt", "payload", self.files[0].name)
        # The unchanged file alone is not that receipt's files either; a receipt of its own passes.
        self.refused(self.check(before, files=self.files[1:]), "receipt", "payload", self.files[0].name)
        self.passed(self.check(self.receipt(self.files[1:]), files=self.files[1:]))
        # A file no receipt row names.
        added = self.files[0].with_name(f"added-{TARGET}.bin")
        added.write_bytes(b"not scanned\n")
        self.refused(self.check(self.receipt(), files=[*self.files, added]), "receipt", "payload", added.name)

    def test_the_private_text_scan_needs_blocking_rules_and_the_pinned_lists(self):
        receipt = self.receipt()
        for value in (False, None, 1, "true", 5):
            self.refused(self.check(change(receipt, "privacy.blocking_rules", value)), "privacy", "blocking_rules")
        # Every rule of a name class was blocking: a review class refuses nothing, so its names would pass.
        for value in (False, None, 1, 0, "true"):
            self.refused(self.check(change(receipt, "privacy.name_rules_all_blocking", value)), "privacy", "name_rules_all_blocking")
        # The scan ran with no exemption: an exemption switches a match off, and no pin holds its file.
        for value in (False, None, 1, 0, "true"):
            self.refused(self.check(change(receipt, "privacy.no_exemptions", value)), "privacy", "no_exemptions")
        # Every acceptance entry the scan used names the tool that proved it, and the receipt names those
        # tools with the entries each proved, for whoever reads it before the upload.
        for value in (False, None, 1, 0, "true"):
            self.refused(self.check(change(receipt, "privacy.accepted_only_with_proof", value)), "privacy", "accepted_only_with_proof")
        for value in (None, [], "zq-prover-one", 2, {"": 1}, {"  ": 1}, {"zq-prover-one": 0}, {"zq-prover-one": "1"},
                      {"zq-prover-one": True}, {"zq-prover-one": -1}, {"zq-prover-one": 1.5}, {"zq-prover-one": None},
                      # A name outside the one form of a proof tool's name, alone or beside a name of that form.
                      {"a tool with a longer name": 1, "zq-prover-two": 2}, {"accepted by hand.": 1}, {"tools/zq-prover.py": 1},
                      {"zq-prover.py": 1}, {"Zq-prover-one": 1}, {"zq_prover_one": 1}, {"zq--prover": 1}, {"-zq-prover": 1},
                      {" zq-prover-one": 1}, {"zq-prover-one\n": 1}, {"zq-prover\none": 1}, {"zq-pr" + chr(0xf6) + "ver": 1}, {"z" * 65: 1},
                      {"z" * 100000: 1}):
            with self.subTest(tools=value if len(str(value)) < 200 else "a long name"):
                self.refused(self.check(change(receipt, "privacy.accepted_proof_tools", value)), "privacy", "accepted_proof_tools")
        for value in ({}, {"zq-prover-one": 40}, {"a-tool-with-a-longer-name": 1, "zq-prover-two": 2}, {"7": 1, "z" * 64: 1}):
            self.passed(self.check(change(receipt, "privacy.accepted_proof_tools", value)))
        for field in ("name_list_sha256", "rules_sha256", "non_published_list_sha256"):
            for value in ("0" * 64, None, "none", True):
                self.refused(self.check(change(receipt, "privacy." + field, value)), "privacy", field)

    def test_rules_that_only_declare_the_pinned_name_list_refuse(self):
        # A rules file states the digest of the name list it was written from, and a report repeats what
        # the rules file says. So the rules file itself is pinned: one made-up blocking rule under the
        # pinned list's digest is not the rules of that list.
        names = self.root / "names.txt"
        names.write_text("product\tzqmadeupname\nreview-product\tzqothername\n")
        derived = []
        for index in (1, 2):
            rules = self.root / f"rules-{index}.json"
            done = run(CONVERTER, "--names", names, "--rules", rules, "--exemptions", self.root / f"exemptions-{index}.json")
            self.assertEqual(done.returncode, 0, done.stderr)
            derived.append(rules.read_bytes())
        # The rules of a list are one exact file: the pin can be stated for a given list.
        self.assertEqual(derived[0], derived[1])
        declared = json.loads(derived[0])["name_list_sha256"]
        self.assertEqual(declared, sha256(names.read_bytes()))
        tool = self.checkout({**PINS, "name_list_sha256": declared, "rules_sha256": sha256(derived[0])})
        # What a release scan reports for those rules, and for one made-up rule that declares the list.
        report = change(change(self.privacy(), "name_list_sha256", declared), "rules.sha256", sha256(derived[0]))
        made_up = json.dumps({"schema": json.loads(derived[0])["schema"], "name_list_sha256": declared, "categories": {
            "product": {"rules": [{"id": "product-1", "kind": "literal", "value": "nothing-in-the-payload"}]}}}).encode()
        forged = change(report, "rules", {"sha256": sha256(made_up), "total": 1, "blocking": 1, "review_outside_paths": 0})
        self.assertEqual(forged["name_list_sha256"], declared)
        receipts = []
        for label, value in (("derived", report), ("made-up", forged)):
            receipts.append(self.root / f"upload-receipt-{label}.json")
            done = run(tool.with_name(WRITE.name), "--version", VERSION, "--target", TARGET, "--privacy-report", self.write("privacy-report", value),
                       "--credential-report", self.write("credential-report", self.credential()), "--receipt", receipts[-1])
            self.assertEqual(done.returncode, 0, done.stdout + done.stderr)
        self.passed(self.check(receipts[0], tool=tool))
        self.refused(self.check(receipts[1], tool=tool), "privacy", "rules_sha256")

    def test_the_coverage_exceptions_are_the_checkouts_committed_file(self):
        # A scan that ran with another exceptions file is not this checkout's scan.
        receipt = self.receipt()
        for value in ("0" * 64, None):
            self.refused(self.check(change(receipt, "privacy.coverage_exceptions_sha256", value)), "privacy", "coverage_exceptions")
        other = EXCEPTIONS.replace("[]", json.dumps([{"path": "made-up/member", "sha256": "1" * 64, "kind": "archive-layout",
                                                      "reason": "made up", "accepted_by": "nobody"}]))
        scanned_with_other = change(receipt, "privacy.coverage_exceptions_sha256", sha256(other))
        self.refused(self.check(scanned_with_other), "privacy", "coverage_exceptions")
        # In a checkout that commits that file, the same receipt passes and the first one refuses.
        tool = self.checkout(exceptions=other)
        self.passed(self.check(scanned_with_other, tool=tool))
        self.refused(self.check(tool=tool), "privacy", "coverage_exceptions")
        # The file is read as the commit holds it: one edited after the commit, or absent, refuses.
        release_file(tool, EXCEPTIONS_NAME).write_text(EXCEPTIONS)
        self.refused(self.check(tool=tool), EXCEPTIONS_NAME)
        self.refused(self.check(scanned_with_other, tool=tool), EXCEPTIONS_NAME)
        self.refused(self.check(tool=self.checkout(exceptions=None)), EXCEPTIONS_NAME)

    def test_the_credential_scan_needs_the_pinned_builds_and_nothing_refused(self):
        receipt = self.receipt()
        swapped = change(change(receipt, "credential.detector_pinned_sha256", CURRENT), "credential.detector_current_sha256", PINNED)
        for value in (change(receipt, "credential.detector_pinned_sha256", "0" * 64),
                      change(receipt, "credential.detector_current_sha256", "0" * 64),
                      change(receipt, "credential.detector_current_sha256", None), swapped):
            self.refused(self.check(value), "credential", "detector")
        for field in ("nothing_refused", "nothing_pending", "nothing_unread_unaccepted"):
            for value in (False, None, 0, 1, "true"):
                self.refused(self.check(change(receipt, "credential." + field, value)), "credential", field)

    def test_the_credential_scan_needs_the_pinned_default_rules_file_of_each_build(self):
        # A build's digest does not say which rules file the scan gave it, and a file that lacks a
        # rule finds less. A receipt of a scan in which a build read with another rules file refuses,
        # and so does one that states no digest there, a build's digest, or the two the other way round.
        receipt = self.receipt()
        fields = ("credential.detector_pinned_default_rules_sha256", "credential.detector_current_default_rules_sha256")
        swapped = change(change(receipt, fields[0], CURRENT_RULES), fields[1], PINNED_RULES)
        for number, value in enumerate([change(receipt, field, other) for field in fields for other in ("0" * 64, None, "", PINNED)]
                                       + [swapped]):
            with self.subTest(case=number):
                done = self.check(value)
                self.refused(done, "credential", "default rules files")
                # The builds are the pinned ones in every case: the one reason is the rules file.
                self.assertNotIn("are not the two pinned builds", done.stdout)
                self.assertEqual(len(done.stdout.strip().splitlines()), 2, done.stdout)

    def test_the_pins_are_the_ones_the_checkouts_commit_holds(self):
        # Each receipt field is held to the committed pin of the target: a checkout whose commit pins
        # another digest refuses the receipt the first checkout passes, and passes the one it refuses.
        self.passed(self.check())
        for name, path in PIN_FIELDS.items():
            with self.subTest(pin=name):
                which, field = path.split(".")
                other = self.checkout({**PINS, name: "0" * 64})
                self.refused(self.check(tool=other), which, field.replace("_sha256", "").split("_")[0])
                receipt = change(self.receipt(), path, "0" * 64)
                self.passed(self.check(receipt, tool=other))
                self.refused(self.check(receipt), which)

    def test_each_target_has_its_own_pins(self):
        # The receipt of one target does not pass under another target's pins, and the files name their target.
        other_files = []
        for path in self.files:
            other_files.append(path.with_name(path.name.replace(TARGET, OTHER_TARGET)))
            other_files[-1].write_bytes(path.read_bytes())
        receipt = change(self.receipt(other_files), "target", OTHER_TARGET)
        self.refused(self.check(receipt, files=other_files, target=OTHER_TARGET),
                     "name_list_sha256", "rules_sha256", "non_published_list_sha256", "detector")
        for name, path in PIN_FIELDS.items():
            receipt = change(receipt, path, OTHER_PINS[name])
        done = self.check(receipt, files=other_files, target=OTHER_TARGET)
        self.passed(done)
        self.assertIn("target=" + OTHER_TARGET, done.stdout.split())
        # The receipt names its target: it is not another target's receipt.
        self.refused(self.check(change(receipt, "target", TARGET), files=other_files, target=OTHER_TARGET), "receipt", "target")
        self.refused(self.check(change(self.receipt(), "target", OTHER_TARGET)), "receipt", "target")
        # A file that does not name the target it is checked for, or that names another target too.
        self.refused(self.check(target=OTHER_TARGET), "target", self.files[0].name)
        self.refused(self.check(receipt, files=other_files, target=TARGET), "target")
        both = self.files[0].with_name(f"product-{TARGET}-{OTHER_TARGET}.tar.gz")
        both.write_bytes(b"x")
        self.refused(self.check(self.receipt([both]), files=[both]), "target", both.name)
        # The target is required, and it is one the pins file has.
        self.usage(self.check(target=None))
        self.refused(self.check(target="plan9-mips"), "pins", "plan9-mips")
        # A target whose pins are not set refuses, while the other target of the same file passes.
        tool = self.checkout(pins_text({TARGET: PINS, OTHER_TARGET: {name: None for name in PINS}}))
        self.passed(self.check(tool=tool))
        self.refused(self.check(receipt, files=other_files, target=OTHER_TARGET, tool=tool), "pins")

    def test_a_pin_that_is_null_or_missing_refuses(self):
        for name, path in PIN_FIELDS.items():
            with self.subTest(pin=name):
                tool = self.checkout({**PINS, name: None})
                self.refused(self.check(tool=tool), "pins", name)
                # A receipt that states nothing there either does not meet a pin that is not set.
                self.refused(self.check(change(self.receipt(), path, None), tool=tool), "pins", name)
                missing = self.checkout({key: value for key, value in PINS.items() if key != name})
                self.refused(self.check(tool=missing), "pins")
        # A file whose lists are not set, as the file was committed before a pins commit set them:
        # nothing passes, whatever the receipt says about its own lists.
        unset = {**PINS, "name_list_sha256": None, "rules_sha256": None, "non_published_list_sha256": None}
        receipt = self.receipt()
        for field in ("name_list_sha256", "rules_sha256", "non_published_list_sha256"):
            receipt = change(receipt, "privacy." + field, None)
        self.refused(self.check(receipt, tool=self.checkout(unset)),
                     "pins", "name_list_sha256", "rules_sha256", "non_published_list_sha256")
        self.refused(self.check(tool=self.checkout({name: None for name in PINS})), "pins")

    def test_no_option_or_variable_gives_a_pin(self):
        unset = self.checkout({name: None for name in PINS})
        elsewhere = self.root / "pins-elsewhere.json"
        elsewhere.write_text(pins_text())
        values = {option: (PINS.get(option[2:].replace("-", "_"), "1" * 64) if option.endswith("-sha256") else self.files[0])
                  for option in FORMER_OPTIONS}
        values.update({"--rules-sha256": RULES, "--pins": elsewhere, "--pins-file": elsewhere, "--pins-sha256": sha256(pins_text()),
                       "--coverage-exceptions": elsewhere, "--coverage-exceptions-sha256": sha256(EXCEPTIONS)})
        for option, value in values.items():
            with self.subTest(option=option):
                for tool in (self.tool, unset):
                    done = self.check(tool=tool, extra=[option, value])
                    self.usage(done)
                    self.assertIn("unrecognized arguments", done.stderr)
        # Nor does the environment.
        env = {name.upper(): value for name, value in PINS.items()}
        env.update({"UPLOAD_RECEIPT_PINS": str(elsewhere), "RELEASE_UPLOAD_RECEIPT_PINS": str(elsewhere)})
        self.refused(self.check(tool=unset, env=env), "pins")
        self.passed(self.check(env={name.upper(): "0" * 64 for name in PINS}))

    def test_a_pins_file_that_is_not_the_checkouts_commit_is_not_read(self):
        elsewhere = self.root / "elsewhere"
        elsewhere.mkdir()
        (elsewhere / PINS_NAME).write_text(pins_text())
        (elsewhere / EXCEPTIONS_NAME).write_text(EXCEPTIONS)
        unset = {**PINS, "name_list_sha256": None}
        # A link to a file outside the checkout, committed as a link.
        tool = self.checkout(None, committed=False)
        release_file(tool, PINS_NAME).symlink_to(elsewhere / PINS_NAME)
        commit(tool.parent.parent)
        self.refused(self.check(tool=tool), "pins")
        # The release directory itself a link to a directory outside.
        tool = self.checkout(None, committed=False, exceptions=None)
        release_file(tool, PINS_NAME).parent.rmdir()
        release_file(tool, PINS_NAME).parent.symlink_to(elsewhere, target_is_directory=True)
        commit(tool.parent.parent)
        self.assertTrue(release_file(tool, PINS_NAME).is_file())
        self.refused(self.check(tool=tool), "pins")
        # A file the commit does not hold: written after the commit, then staged as well.
        tool = self.checkout(None)
        release_file(tool, PINS_NAME).write_text(pins_text())
        self.refused(self.check(tool=tool), "pins")
        git(tool.parent, "add", "-A")
        self.refused(self.check(tool=tool), "pins")
        # A file edited after the commit: pins set in the work tree only, and pins changed there.
        tool = self.checkout(unset)
        self.refused(self.check(tool=tool), "pins", "name_list_sha256")
        release_file(tool, PINS_NAME).write_text(pins_text())
        self.refused(self.check(tool=tool), "pins")
        tool = self.checkout()
        self.passed(self.check(tool=tool))
        release_file(tool, PINS_NAME).write_text(pins_text({TARGET: {**PINS, "name_list_sha256": "0" * 64}}))
        self.refused(self.check(change(self.receipt(), "privacy.name_list_sha256", "0" * 64), tool=tool), "pins")
        git(tool.parent.parent, "checkout", "--", ".")
        self.passed(self.check(tool=tool))
        # The tool itself edited after the commit: the commit no longer vouches for the check.
        with open(tool, "a") as stream:
            stream.write("# edited after the commit\n")
        self.refused(self.check(tool=tool), "pins")
        # No checkout at all: the tool and a pins file in plain directories.
        self.refused(self.check(tool=self.checkout(committed=False)), "pins")
        # Another repository that the environment names is not the tool's checkout.
        good = self.tool.parent.parent
        tool = self.checkout(unset)
        self.refused(self.check(tool=tool, env={"GIT_DIR": str(good / ".git"), "GIT_WORK_TREE": str(good)}), "pins", "name_list_sha256")
        bad = tool.parent.parent
        self.passed(self.check(env={"GIT_DIR": str(bad / ".git"), "GIT_WORK_TREE": str(bad)}))

    def test_a_replace_ref_gives_the_check_no_other_pins(self):
        # git can be told to read another object in place of one (a replace ref). The check asks for the
        # commit's own objects: pins that no commit of the checkout holds are not read.
        unset = {**PINS, "name_list_sha256": None}
        # The pins file's blob replaced by one with every pin set, and the work tree's file made that one.
        tool = self.checkout(unset)
        repository, pins = tool.parent.parent, release_file(tool, PINS_NAME)
        self.refused(self.check(tool=tool), "pins", "name_list_sha256")
        committed = git(repository, "rev-parse", "HEAD:release/" + PINS_NAME)
        pins.write_text(pins_text())
        git(repository, "replace", committed, git(repository, "hash-object", "-w", "--", pins))
        self.assertEqual(git(repository, "cat-file", "blob", "HEAD:release/" + PINS_NAME), pins_text().strip())
        self.refused(self.check(tool=tool), "pins")
        # The commit itself replaced by one whose tree holds the set pins.
        tool = self.checkout(unset)
        repository, pins = tool.parent.parent, release_file(tool, PINS_NAME)
        head = git(repository, "rev-parse", "HEAD")
        pins.write_text(pins_text())
        git(repository, "add", "-A")
        git(repository, "replace", head, git(repository, "commit-tree", git(repository, "write-tree"), "-m", "pins set"))
        self.assertEqual((git(repository, "rev-parse", "HEAD"), git(repository, "status", "--porcelain")), (head, ""))
        self.assertEqual(git(repository, "cat-file", "blob", "HEAD:release/" + PINS_NAME), pins_text().strip())
        self.refused(self.check(tool=tool), "pins")
        # The same for the tool: an edited check that a replaced commit holds is not the commit's check.
        tool = self.checkout()
        repository = tool.parent.parent
        head = git(repository, "rev-parse", "HEAD")
        self.passed(self.check(tool=tool))
        with open(tool, "a") as stream:
            stream.write("# edited after the commit\n")
        git(repository, "add", "-A")
        git(repository, "replace", head, git(repository, "commit-tree", git(repository, "write-tree"), "-m", "edited"))
        self.assertEqual(git(repository, "status", "--porcelain"), "")
        self.refused(self.check(tool=tool), "pins")

    def test_the_pins_file_must_be_of_its_schema(self):
        one = section(PINS)
        whole = {"schema": PINS_SCHEMA, "targets": {TARGET: one}}
        cases = {"another schema": json.dumps({**whole, "schema": "release-upload-receipt-check/pins/0"}),
                 "the former schema": json.dumps({**whole, "schema": "release-upload-receipt-check/pins/1"}),
                 "the former file": json.dumps({"schema": "release-upload-receipt-check/pins/1", "targets": {TARGET: PINS}}),
                 # The schema before this one, which pinned the builds and not the rules file each reads with:
                 # as it was, under this schema's name, and with this schema's pins under its own.
                 "the schema before this one": json.dumps({"schema": "release-upload-receipt-check/pins/2",
                                                           "targets": {TARGET: section(FORMER_PINS)}}),
                 "no pin for the rules files": json.dumps({**whole, "targets": {TARGET: section(FORMER_PINS)}}),
                 "the name of the schema before": json.dumps({**whole, "schema": "release-upload-receipt-check/pins/2"}),
                 "no schema": json.dumps({"targets": {TARGET: one}}),
                 "one key more": json.dumps({**whole, "max_age_hours": 1000}),
                 "pins beside the targets": json.dumps({**whole, **PINS}),
                 "one pin more": json.dumps({**whole, "targets": {TARGET: {**one, "exemptions_sha256": "1" * 64}}}),
                 "targets as a list": json.dumps({**whole, "targets": [one]}),
                 "a target that is no mapping": json.dumps({**whole, "targets": {TARGET: NAME_LIST}}),
                 "short digest": json.dumps({**whole, "targets": {TARGET: {**one, "name_list_sha256": "a1"}}}),
                 "upper case": json.dumps({**whole, "targets": {TARGET: {**one, "detector_pinned_sha256": PINNED.upper()}}}),
                 "a number": json.dumps({**whole, "targets": {TARGET: {**one, "detector_current_sha256": 7}}}),
                 "true": json.dumps({**whole, "targets": {TARGET: {**one, "non_published_list_sha256": True}}}),
                 "a list": json.dumps([whole]),
                 "a target twice": pins_text({TARGET: PINS}).replace('"targets": {', '"targets": {"%s": {}, ' % TARGET, 1),
                 "a pin twice": pins_text({TARGET: PINS}).replace(
                     '"name_list_sha256"', '"name_list_sha256": "%s", "name_list_sha256"' % NAME_LIST, 1),
                 "not JSON": "{not json", "empty": ""}
        for label, text in cases.items():
            with self.subTest(pins=label):
                self.refused(self.check(tool=self.checkout(text)), "pins")
        # One target alone is a whole file.
        self.passed(self.check(tool=self.checkout(pins_text({TARGET: PINS}))))

    def test_the_repository_commits_one_pins_file_of_that_schema(self):
        release = HERE.parent / "release"
        pins = json.loads((release / PINS_NAME).read_text())
        self.assertEqual((set(pins), pins["schema"]), ({"schema", "targets"}, PINS_SCHEMA))
        # A section for every target a library or a candidate is built for, and one for the release's
        # own files; a name rule, digests and nulls only.
        self.assertEqual(set(pins["targets"]), {TARGET, OTHER_TARGET, RELEASE})
        for target, values in pins["targets"].items():
            self.assertEqual(set(values), {"names", *PINS}, target)
            for name in PINS:
                self.assertTrue(values[name] is None or re.fullmatch(r"[0-9a-f]{64}", values[name]), (target, name))
            self.assertEqual(values["names"], RELEASE_NAMES if target == RELEASE else HOLDS)
        # The Linux target and the release's own files have every pin set, each a digest: a commit
        # before the release commit sets them from the lane's inputs. The macOS target's pins are not
        # set, so nothing of that target passes. That a pin that is null or missing refuses, for a
        # platform target, for another target of the same file and for the release's own files, is
        # held by test_a_pin_that_is_null_or_missing_refuses, test_each_target_has_its_own_pins and
        # test_the_release_target_has_its_own_pins.
        for target in (TARGET, RELEASE):
            for name in PINS:
                self.assertRegex(pins["targets"][target][name] or "", r"\A[0-9a-f]{64}\Z", (target, name))
        self.assertEqual(set(pins["targets"][OTHER_TARGET][name] for name in PINS), {None})
        self.assertEqual([path.name for path in release.glob("*pins*")], [PINS_NAME])
        # The file the check compares the scans' coverage exceptions with is committed beside it.
        self.assertTrue((release / EXCEPTIONS_NAME).is_file())

    def test_the_empty_list_of_left_out_members_is_a_public_file_that_can_be_pinned(self):
        # A target that leaves nothing out is scanned and built with the empty list. That list names
        # nothing, so it is committed, a hosted job may hold it, and its digest is an ordinary pin.
        empty = HERE.parent / "release" / EMPTY_LIST_NAME
        packager = load("package_library_release_for_receipts", HERE / "package-library-release.py")
        self.assertEqual(empty.read_bytes(), packager.encode_source_member_lists({}, {}))
        lists = packager.read_non_published_list(empty.read_bytes())
        self.assertEqual((lists.names(), lists.digests()), (frozenset(), frozenset()))
        digest = sha256(empty.read_bytes())
        tool = self.checkout({**PINS, "non_published_list_sha256": digest})
        self.passed(self.check(change(self.receipt(), "privacy.non_published_list_sha256", digest), tool=tool))
        self.refused(self.check(tool=tool), "privacy", "non_published_list_sha256")

    def test_upload_files_are_regular_files_with_distinct_names(self):
        link = self.root / self.files[0].name.replace("product", "link")
        link.symlink_to(self.files[0])
        twin = self.root / self.files[0].name
        twin.write_bytes(self.files[0].read_bytes())
        for files in ((), (self.root,), (link,), (self.files[0], twin), (self.root / f"absent-{TARGET}.tar.gz",)):
            done = self.check(files=files)
            self.assertEqual(done.returncode, 2, files)
            self.assertIn("usage error", done.stderr)
            self.assertNotIn("UPLOAD-CHECK PASS", done.stdout)


class NameRules(Fixture):
    """Which target a file is judged under is the pins file's to say: each target has a committed name
    rule, and no file name passes under two targets."""

    def judged(self, files, target, version=VERSION, tool=None):
        receipt = change(self.receipt(files), "target", target)
        return self.check(receipt, files=files, target=target, tool=tool, version=version)

    def test_the_release_target_passes_exactly_its_names_for_the_given_version(self):
        files = self.release_files()
        done = self.judged(files, RELEASE, VERSION)
        self.passed(done)
        for word in ("files=5", "target=" + RELEASE):
            self.assertIn(word, done.stdout.split())
        # Each of its names alone is a whole receipt too.
        for path in files:
            self.passed(self.judged([path], RELEASE, VERSION))
        # The version is the caller's to give, in one strict form; without it nothing of this target passes.
        self.refused(self.judged(files, RELEASE, None), "files", "--version")
        self.refused(self.judged(files[:1], RELEASE, None), "files", "--version")
        for version in ("v" + VERSION, "1.2", "1.2.3.4", VERSION + "-rc1", VERSION + "+build", " " + VERSION, "1.2.x", ""):
            self.usage(self.judged(files, RELEASE, version))
        # A file named for another version is not a name of this release.
        versioned = [path for path in files if VERSION in path.name]
        self.assertEqual(len(versioned), 2)
        self.refused(self.judged(files, RELEASE, "1.2.4"), "files", *(path.name for path in versioned))
        other = self.release_files("1.2.4")
        self.passed(self.judged(other, RELEASE, "1.2.4"))
        self.refused(self.judged(other, RELEASE, VERSION), "files", *(path.name for path in other if "1.2.4" in path.name))
        mixed = [path for path in files if VERSION not in path.name] + [path for path in other if "1.2.4" in path.name]
        self.refused(self.judged(mixed, RELEASE, VERSION), "files")
        # Any other name: another case, a longer or shorter name, a name of no pattern.
        for name in ("release-manifest.json", "SHA256SUMS.txt", "XSHA256SUMS", "source-manifest.json.sig", "notes.txt",
                     f"meerkat_toolkit-{VERSION}-py3-none-any.whl.asc", f"rkat-toolkit-{VERSION}.tar.gz", f"rkat-toolkit-{VERSION}",
                     "rkat-toolkit-<version>.tgz", f"rkat-toolkit-{VERSION}{VERSION}.tgz", f"other-{RELEASE}.bin"):
            with self.subTest(name=name):
                extra = self.named(name)
                self.refused(self.judged(extra, RELEASE, VERSION), "files", name)
                self.refused(self.judged(files + extra, RELEASE, VERSION), "files", name)

    def test_no_file_passes_under_two_targets(self):
        release, platform = self.release_files(), self.files
        # A platform's file is not a file of the release target, nor the other way round, and the
        # version means nothing where the names hold a target.
        self.refused(self.judged(platform, RELEASE, VERSION), "files", platform[0].name)
        self.refused(self.judged(release, TARGET), "files", release[0].name)
        self.refused(self.judged(release + platform, RELEASE, VERSION), "files", platform[0].name)
        self.refused(self.judged(release + platform, TARGET), "files", release[0].name)
        self.passed(self.judged(platform, TARGET))
        self.refused(self.judged(platform, TARGET, None), "files", "--version")
        # The two evidence files of a platform hold its name: they are that platform's files.
        evidence = self.named(f"product-{VERSION}-{TARGET}.evidence.json", f"product-{VERSION}-{TARGET}.evidence.tar.gz")
        self.passed(self.judged(platform + evidence, TARGET))
        self.refused(self.judged(evidence, RELEASE, VERSION), "files", evidence[0].name)
        self.refused(self.judged(evidence, OTHER_TARGET), "files", evidence[0].name)
        # A platform target whose name a version can hold: a file that holds that name and is an exact
        # name of the release target passes under neither.
        digits = self.checkout(pins_text({"7": PINS, TARGET: PINS, RELEASE: section(PINS, RELEASE_NAMES)}))
        both = self.named("rkat-toolkit-1.7.3.tgz")
        self.refused(self.judged(both, "7", "1.7.3", tool=digits), "files", both[0].name)
        self.refused(self.judged(both, RELEASE, "1.7.3", tool=digits), "files", both[0].name)
        self.passed(self.judged(self.named("product-7.bin"), "7", tool=digits))
        self.passed(self.judged(self.named("rkat-toolkit-1.2.3.tgz"), RELEASE, VERSION, tool=digits))
        # Two targets with exact names: a name that one's pattern and the other's literal name both
        # give passes under neither.
        twice = self.checkout(pins_text({TARGET: PINS, RELEASE: section(PINS, RELEASE_NAMES),
                                         "extras": section(PINS, ["rkat-toolkit-9.9.9.tgz", "extra-notes.txt"])}))
        clash = self.named("rkat-toolkit-9.9.9.tgz")
        self.refused(self.judged(clash, RELEASE, "9.9.9", tool=twice), "files", clash[0].name)
        self.refused(self.judged(clash, "extras", "9.9.9", tool=twice), "files", clash[0].name)
        self.passed(self.judged(self.named("extra-notes.txt"), "extras", VERSION, tool=twice))
        self.passed(self.judged(self.release_files(), RELEASE, VERSION, tool=twice))
        # One list that gives one name in two ways: that name is not exactly one of its names.
        double = self.checkout(pins_text({TARGET: PINS, RELEASE: section(PINS, RELEASE_NAMES + ["rkat-toolkit-9.9.9.tgz"])}))
        self.refused(self.judged(clash, RELEASE, "9.9.9", tool=double), "files", clash[0].name)
        self.passed(self.judged(self.named("rkat-toolkit-1.2.3.tgz"), RELEASE, VERSION, tool=double))

    def test_the_release_target_has_its_own_pins(self):
        files = self.release_files()
        # The pins of the release target are its section's, not a platform's.
        own = self.checkout(pins_text({TARGET: PINS, RELEASE: section(OTHER_PINS, RELEASE_NAMES)}))
        self.refused(self.judged(files, RELEASE, VERSION, tool=own), "name_list_sha256", "rules_sha256", "detector")
        self.passed(self.check(tool=own))
        receipt = change(self.receipt(files), "target", RELEASE)
        for name, path in PIN_FIELDS.items():
            receipt = change(receipt, path, OTHER_PINS[name])
        self.passed(self.check(receipt, files=files, target=RELEASE, tool=own))
        # Not set: nothing of the release's own files passes, while the platform's do.
        unset = self.checkout(pins_text({TARGET: PINS, RELEASE: section({name: None for name in PINS}, RELEASE_NAMES)}))
        self.refused(self.judged(files, RELEASE, VERSION, tool=unset), "pins", RELEASE)
        self.passed(self.check(tool=unset))
        # A pins file without that target has no rule for such a file at all.
        none = self.checkout(pins_text({TARGET: PINS, OTHER_TARGET: OTHER_PINS}))
        self.refused(self.judged(files, RELEASE, VERSION, tool=none), "pins", RELEASE)

    def test_the_pins_file_states_one_name_rule_for_every_target(self):
        def refuses(label, targets, release_too=False):
            with self.subTest(pins=label):
                tool = self.checkout(targets if isinstance(targets, str) else pins_text(targets))
                # The file is refused as a whole: for a platform target, whichever section is at fault.
                self.refused(self.check(tool=tool), "pins")
                if release_too:
                    self.refused(self.judged(self.release_files(), RELEASE, VERSION, tool=tool), "pins")

        release = section(PINS, RELEASE_NAMES)
        for label, names in (("none", None), ("another word", "any"), ("a near word", "holds_target"), ("empty", ""),
                             ("an empty list", []), ("a number in the list", ["SHA256SUMS", 7]), ("a mapping", {"SHA256SUMS": True}),
                             ("true", True)):
            refuses("names " + label, {TARGET: section(PINS, names), RELEASE: release})
            refuses("release names " + label, {TARGET: PINS, RELEASE: section(PINS, names)}, release_too=label == "none")
        refuses("no name rule", json.dumps({"schema": PINS_SCHEMA, "targets": {TARGET: section(PINS), RELEASE: PINS}}), release_too=True)
        # A pattern is one file name, with at most one place for the version.
        for pattern in ("dir/SHA256SUMS", "../SHA256SUMS", "SHA256SUMS ", "", ".", "..", ".hidden", "-option", "a*.tgz", "a?.tgz",
                        "rkat-toolkit-<version>-<version>.tgz", "rkat-toolkit-<ver>.tgz", "rkat-toolkit-<version.tgz",
                        "rkat-toolkit-[0-9]+.tgz", "<version>", "a\\b"):
            refuses("pattern " + repr(pattern), {TARGET: PINS, RELEASE: section(PINS, RELEASE_NAMES + [pattern])})
        # A pattern that holds a platform target's name would give a file two targets.
        for pattern in (f"notes-{TARGET}.txt", OTHER_TARGET, f"rkat-toolkit-<version>-{OTHER_TARGET}.tgz"):
            refuses("pattern " + pattern, {TARGET: PINS, OTHER_TARGET: OTHER_PINS, RELEASE: section(PINS, RELEASE_NAMES + [pattern])})
        # A pattern twice: in one list, or in the lists of two targets.
        refuses("a pattern twice", {TARGET: PINS, RELEASE: section(PINS, RELEASE_NAMES + RELEASE_NAMES[:1])})
        refuses("a pattern of two targets", {TARGET: PINS, RELEASE: release, "extras": section(PINS, ["notes.txt", RELEASE_NAMES[3]])})
        # A target's name is a plain lower-case word.
        for name in ("Linux", "linux x86", "", "-linux", "linux/x86", "a" * 65):
            refuses("target " + repr(name), {TARGET: PINS, RELEASE: release, name: PINS})
        # What passes: names that are free of all of that.
        fine = self.checkout(pins_text({TARGET: PINS, RELEASE: section(PINS, ["SHA256SUMS", "a.b_c-<version>.whl"])}))
        self.passed(self.check(tool=fine))
        self.passed(self.judged(self.named("SHA256SUMS", f"a.b_c-{VERSION}.whl"), RELEASE, VERSION, tool=fine))
        self.refused(self.judged(self.named("RELEASE-MANIFEST.json"), RELEASE, VERSION, tool=fine), "files")

    def test_a_platform_file_named_for_another_version_refuses(self):
        # The version goes with every target. A platform's file names hold the target; a name that
        # also holds a version holds the release's, so the files of one release pass for no other.
        def platform(*names):
            return self.named(*names, directory="versions")

        own = platform(f"product-{VERSION}-{TARGET}.tar.gz", f"product-{VERSION}-{TARGET}.evidence.json")
        self.passed(self.judged(own, TARGET))
        self.refused(self.judged(own, TARGET, "1.2.4"), "files", own[0].name, own[1].name)
        self.refused(self.judged(own, TARGET, None), "files", "--version")
        self.usage(self.judged(own, TARGET, "v" + VERSION))
        other = platform(f"product-9.9.9-{TARGET}.tar.gz")
        self.refused(self.judged(other, TARGET), "files", other[0].name)
        self.refused(self.judged(own + other, TARGET), "files", other[0].name)
        self.passed(self.judged(other, TARGET, "9.9.9"))
        # Any dotted number of three or more parts is a version: it must be the given one, wherever
        # it stands and however often. A number of two parts, and no number at all, are no version:
        # the candidate's own files hold none.
        for name, passes in ((f"product-{VERSION}.4-{TARGET}.bin", False), (f"product-{VERSION}.0-{TARGET}.bin", False),
                             (f"product-1{VERSION}-{TARGET}.bin", False), (f"product-{VERSION}0-{TARGET}.bin", False),
                             (f"product-{VERSION}-from-0.9.0-{TARGET}.bin", False), (f"0.9.0-product-{TARGET}.bin", False),
                             (f"product-v{VERSION}-{TARGET}.bin", True), (f"product-{VERSION}-{TARGET}-{VERSION}.bin", True),
                             (f"product_{VERSION}_{TARGET}.bin", True), (f"product-1.2-{TARGET}.bin", True),
                             (f"connect-{TARGET}-candidate.tar.gz", True), (f"connect-{TARGET}-provenance.jsonl", True),
                             (f"product-py3.11-{TARGET}.bin", True)):
            with self.subTest(name=name):
                (self.passed if passes else self.refused)(self.judged(platform(name), TARGET))

    def test_the_function_takes_the_version_as_the_command_line_does(self):
        module = load("receipt_check_module_for_names", self.tool)
        files = self.release_files()
        receipt = self.write("upload-receipt", change(self.receipt(files), "target", RELEASE))
        self.assertIsInstance(module.check(RELEASE, receipt, COMMIT, files, version=VERSION), module.Passed)
        for version, error in ((None, module.Refused), ("1.2.4", module.Refused), ("v" + VERSION, module.Usage), (123, module.Usage)):
            with self.subTest(version=version), self.assertRaises(error):
                module.check(RELEASE, receipt, COMMIT, files, version=version)
        platform = self.write("upload-receipt", self.receipt())
        self.assertIsInstance(module.check(TARGET, platform, COMMIT, self.files, version=VERSION), module.Passed)
        for version, error in ((None, module.Refused), ("9.9.9", module.Refused), ("v" + VERSION, module.Usage)):
            with self.subTest(platform=version), self.assertRaises(error):
                module.check(TARGET, platform, COMMIT, self.files, version=version)


class ReceiptCheckFunction(Fixture):
    """The check as a function of the module, for an upload tool of the same checkout: the same
    judgement as the command line, as a value or an exception, with nothing printed."""

    def module(self, tool=None):
        self.count += 1
        return load(f"receipt_check_module_{self.count}", self.tool if tool is None else tool)

    def call(self, function, *args, **options):
        """(what the function returned or raised, what it printed)."""
        if getattr(function, "__name__", "") == "check":
            options.setdefault("version", VERSION)
        printed = io.StringIO()
        with contextlib.redirect_stdout(printed), contextlib.redirect_stderr(printed):
            try:
                value = function(*args, **options)
            except Exception as error:  # the caller tells which
                value = error
        return value, printed.getvalue()

    def test_the_function_returns_what_the_command_line_prints(self):
        module = self.module()
        receipt = self.write("upload-receipt", self.receipt())
        passed, printed = self.call(module.check, TARGET, receipt, COMMIT, self.files)
        self.assertIsInstance(passed, module.Passed)
        self.assertEqual(printed, "")
        done = self.check(receipt)
        self.passed(done)
        self.assertEqual(passed.line, done.stdout.strip())
        self.assertEqual(passed._fields, ("line", "target", "release_commit", "files", "receipt_sha256", "pins_sha256",
                                          "pins_commit"))
        self.assertEqual((passed.target, passed.release_commit, passed.receipt_sha256, passed.pins_sha256, passed.pins_commit),
                         (TARGET, COMMIT, sha256(receipt.read_bytes()), sha256(pins_text()), git(self.tool.parent, "rev-parse", "HEAD")))
        self.assertEqual(passed.files, tuple((path.name, path.stat().st_size, sha256(path.read_bytes())) for path in self.files))
        # The age bound is the same argument, and can only shorten.
        self.assertIsInstance(self.call(module.check, TARGET, receipt, COMMIT, self.files, max_age_hours=1)[0], module.Passed)
        self.assertIsInstance(self.call(module.check, TARGET, receipt, COMMIT, self.files, max_age_hours=48)[0], module.Usage)

    def test_the_function_raises_the_reasons_of_a_refusal(self):
        module = self.module()
        self.assertTrue(issubclass(module.Refused, Exception) and issubclass(module.Usage, Exception))
        cases = {"no pass": change(change(self.receipt(), "privacy.status", "REFUSED"), "credential.nothing_refused", False),
                 "a part": change(self.receipt(), "payload", self.payload() + [{"name": f"more-{TARGET}.bin", "bytes": 1, "sha256": "7" * 64}]),
                 "a full report": self.privacy()}
        for label, value in cases.items():
            with self.subTest(case=label):
                receipt = self.write("upload-receipt", value)
                error, printed = self.call(module.check, TARGET, receipt, COMMIT, self.files)
                self.assertIsInstance(error, module.Refused)
                self.assertEqual(printed, "")
                done = self.check(receipt)
                self.refused(done)
                # The reasons are the command line's reason lines, in its order.
                self.assertEqual(error.reasons, done.stdout.strip().splitlines()[:-1])
                self.assertTrue(error.reasons)
                self.assertEqual(str(error), "; ".join(error.reasons))
        # The pins are the commit's of the checkout the module file lies in, wherever the caller stands.
        unset = self.module(self.checkout({**PINS, "rules_sha256": None}))
        receipt = self.write("upload-receipt", self.receipt())
        error, _ = self.call(unset.check, TARGET, receipt, COMMIT, self.files)
        self.assertIsInstance(error, unset.Refused)
        self.assertTrue(any(reason.startswith("pins: ") and "rules_sha256" in reason for reason in error.reasons), error.reasons)
        # What the command line calls a usage error is another exception, and never a pass.
        link = self.root / f"link-{TARGET}.bin"
        link.symlink_to(self.files[0])
        for arguments in ((TARGET, receipt, COMMIT[:12], self.files), (TARGET, receipt, COMMIT, []),
                          (TARGET, receipt, COMMIT, [link]), (TARGET, receipt, COMMIT, [self.files[0], self.files[0]])):
            error, printed = self.call(module.check, *arguments)
            self.assertIsInstance(error, module.Usage)
            self.assertEqual(printed, "")

    def test_the_rows_of_a_receipt_can_be_read_for_a_file_list(self):
        module = self.module()
        receipt = self.write("upload-receipt", self.receipt())
        rows, printed = self.call(module.receipt_rows, receipt)
        self.assertEqual(rows, tuple((path.name, path.stat().st_size, sha256(path.read_bytes())) for path in self.files))
        self.assertEqual(printed, "")
        twice = self.payload() + self.payload()[:1]
        for value in (self.privacy(), [], "{not json", change(self.receipt(), "payload", twice), change(self.receipt(), "payload", []),
                      change(self.receipt(), "payload.0.bytes", "3"), change(self.receipt(), "label", "X0"),
                      change(self.receipt(), "schema", "release-upload-receipt/0")):
            error, _ = self.call(module.receipt_rows, self.write("not-a-receipt", value))
            self.assertIsInstance(error, module.Refused)
        self.assertIsInstance(self.call(module.receipt_rows, self.root / "absent.json")[0], module.Refused)
        # Reading the rows judges nothing: a receipt that does not pass still has rows.
        stale = self.write("upload-receipt", change(self.receipt(), "privacy.status", "REFUSED"))
        self.assertEqual(self.call(module.receipt_rows, stale)[0], rows)
        self.assertIsInstance(self.call(module.check, TARGET, stale, COMMIT, self.files)[0], module.Refused)


class ReceiptWrite(Fixture):
    """The receipt is written, where the scans ran, from the two full reports, and holds none of what
    they hold beyond what the check reads."""

    def written(self, privacy=None, credential=None, target=TARGET, extra=(), name=None, tool=None, version=VERSION):
        """Run the writer of a checkout (the class's, unless the check of another is given) for one
        target: (the run, the receipt's path)."""
        privacy = self.privacy() if privacy is None else privacy
        credential = self.credential() if credential is None else credential
        privacy = privacy if isinstance(privacy, Path) else self.write("privacy-report", privacy)
        credential = credential if isinstance(credential, Path) else self.write("credential-report", credential)
        self.count += 1
        receipt = self.root / (name or f"written-receipt-{self.count}.json")
        version = [] if version is None else ["--version", version]
        return run((self.tool if tool is None else tool).with_name(WRITE.name), "--target", target, "--privacy-report", privacy,
                   "--credential-report", credential, "--receipt", receipt, *version, *extra), receipt

    def written_for(self, files, targets, version=VERSION, tool=None, extra=(), privacy_files=None):
        """Run the writer once for one pair of reports over the files and for several targets, in pairs
        of --target and --receipt: (the run, {target: its receipt's path})."""
        privacy = self.write("privacy-report", change(self.privacy(), "payload", self.payload(files if privacy_files is None else privacy_files)))
        credential = self.write("credential-report", change(self.credential(), "payload", self.payload(files)))
        receipts, pairs = {}, []
        for target in targets:
            self.count += 1
            receipts[target] = self.root / f"written-receipt-{target}-{self.count}.json"
            pairs += ["--target", target, "--receipt", receipts[target]]
        version = [] if version is None else ["--version", version]
        return run((self.tool if tool is None else tool).with_name(WRITE.name), "--privacy-report", privacy,
                   "--credential-report", credential, *version, *pairs, *extra), receipts

    def none_written(self, done, receipts, *words, code=1):
        self.assertEqual(done.returncode, code, done.stdout + done.stderr)
        self.assertNotIn("UPLOAD-RECEIPT WRITTEN", done.stdout)
        self.assertEqual([path.name for path in receipts.values() if path.exists()], [])
        if code == 1:
            self.assertIn("UPLOAD-RECEIPT REFUSED", done.stdout)
        for word in words:
            self.assertIn(word, done.stdout)

    def not_written(self, privacy=None, credential=None, words=(), **options):
        done, receipt = self.written(privacy, credential, **options)
        self.assertEqual(done.returncode, 1, done.stdout + done.stderr)
        self.assertIn("UPLOAD-RECEIPT REFUSED", done.stdout)
        self.assertNotIn("UPLOAD-RECEIPT WRITTEN", done.stdout)
        self.assertFalse(receipt.exists())
        for word in words:
            self.assertIn(word, done.stdout)

    def test_a_receipt_is_written_from_two_passing_release_reports(self):
        privacy, credential = self.write("privacy-report", self.privacy()), self.write("credential-report", self.credential())
        done, receipt = self.written(privacy, credential)
        self.assertEqual((done.returncode, done.stderr), (0, ""), done.stdout)
        raw = receipt.read_bytes()
        self.assertEqual(done.stdout, "UPLOAD-RECEIPT WRITTEN target=%s files=2 receipt_sha256=%s\n" % (TARGET, sha256(raw)))
        self.assertEqual(stat.S_IMODE(receipt.stat().st_mode) & 0o133, 0)
        reports = [json.loads(path.read_text()) for path in (privacy, credential)]
        scan = {"status": "PASS", "release": True, "tool_commit": COMMIT}
        self.assertEqual(json.loads(raw), {
            "schema": RECEIPT_SCHEMA, "target": TARGET, "payload": self.payload(),
            "privacy": {**scan, "generated_at": reports[0]["generated_at"], "tool_sha256": PRIVACY_TOOL, "exit_code": 0,
                        "report_sha256": sha256(privacy.read_bytes()), "rules_sha256": RULES, "name_list_sha256": NAME_LIST,
                        "non_published_list_sha256": NON_PUBLISHED, "coverage_exceptions_sha256": sha256(EXCEPTIONS),
                        "blocking_rules": True, "name_rules_all_blocking": True, "no_exemptions": True,
                        "accepted_only_with_proof": True, "accepted_proof_tools": PROOF_TOOLS},
            "credential": {**scan, "generated_at": reports[1]["generated_at"], "tool_sha256": CREDENTIAL_TOOL,
                           "report_sha256": sha256(credential.read_bytes()), "detector_pinned_sha256": PINNED,
                           "detector_current_sha256": CURRENT, "detector_pinned_default_rules_sha256": PINNED_RULES,
                           "detector_current_default_rules_sha256": CURRENT_RULES, "nothing_refused": True, "nothing_pending": True,
                           "nothing_unread_unaccepted": True}})
        # Nothing else of the full reports is in it: no category or rule name, no count, no label.
        text = raw.decode()
        for word in ("madeupclass", "made-up", "X0", "CREDENTIAL-LEG", "total", "count", "hits", "by_rule", "reasons",
                     '"exemptions"', "label", "version", "engine", "e5" * 32, "a7" * 32, "d4" * 32, "6f" * 32):
            self.assertNotIn(word, text, word)
        self.assertFalse(any(isinstance(value, int) and not isinstance(value, bool)
                             for part in ("privacy", "credential") for key, value in json.loads(raw)[part].items()
                             if key != "exit_code"))
        # The check passes with exactly that file, and names it and the two reports it was written from.
        checked = self.check(receipt)
        self.passed(checked)
        for word in ("receipt=" + sha256(raw), "privacy_report=" + sha256(privacy.read_bytes()),
                     "credential_report=" + sha256(credential.read_bytes())):
            self.assertIn(word, checked.stdout.split())

    def test_no_receipt_is_written_from_a_refused_or_proof_only_report(self):
        for which in ("privacy", "credential"):
            base = getattr(self, which)()
            for status in ("REFUSED", "clean", "pass", "PROOF-ONLY", "", None, True, DROP):
                self.not_written(**{which: change(base, "status", status)}, words=(which, "status"))
            for value in (False, None, "true", 1, DROP):
                self.not_written(**{which: change(base, "release", value)}, words=(which, "release"))
            self.not_written(**{which: change(base, "label", "PROOF-ONLY-clean")}, words=(which, "PROOF-ONLY"))
            proof = self.root / ("PROOF-ONLY-%s-report.json" % which)
            proof.write_text(json.dumps(base))
            self.not_written(**{which: proof}, words=(which, "PROOF-ONLY"))
            self.not_written(**{which: self.root / "absent.json"}, words=(which, "missing"))
            self.not_written(**{which: self.write("broken", "{not json")}, words=(which, "JSON"))
            self.not_written(**{which: self.write("list", [])}, words=(which,))
            for path, value in (("generated_at", "yesterday"), ("generated_at", DROP), ("tool_commit", COMMIT[:12]),
                                ("tool_commit", None), ("tool_sha256", DROP), ("schema", "release-upload-receipt/1")):
                self.not_written(**{which: change(base, path, value)}, words=(which, path))
        # A proof-only private-text report: not a release run, named so, and without the list of left-out members.
        proof = change(change(change(self.privacy(), "non_published_list_sha256", None), "release", False), "label", "PROOF-ONLY-clean")
        self.not_written(privacy=proof, words=("privacy", "release", "PROOF-ONLY", "non_published_list_sha256"))
        self.not_written(credential=change(self.credential(), "name", "PROOF-ONLY-clean"), words=("credential", "PROOF-ONLY"))
        # The two reports the other way round.
        self.not_written(privacy=self.credential(), credential=self.privacy(), words=("privacy", "credential", "schema"))

    def test_no_receipt_is_written_from_a_report_the_check_would_refuse_by_its_own_content(self):
        privacy, credential = self.privacy(), self.credential()
        for path, value, word in (("exit_code", 1, "exit_code"), ("exit_code", 4, "exit_code"), ("exit_code", None, "exit_code"),
                                  ("exit_code", False, "exit_code"), ("exit_code", DROP, "exit_code"),
                                  ("rules.blocking", 0, "rules"), ("rules.blocking", "5", "rules"), ("rules.blocking", True, "rules"),
                                  ("rules.total", 4, "rules"), ("rules", DROP, "rules"), ("rules.sha256", None, "rules"),
                                  ("rules.sha256", DROP, "rules"), ("name_list_sha256", None, "name_list_sha256"),
                                  ("name_list_sha256", DROP, "name_list_sha256"),
                                  ("non_published_list_sha256", None, "non_published_list_sha256"),
                                  ("non_published_list_sha256", "none", "non_published_list_sha256"),
                                  ("coverage_exceptions.sha256", None, "coverage_exceptions"),
                                  ("coverage_exceptions", DROP, "coverage_exceptions"), ("exemptions", DROP, "exemptions"),
                                  ("accepted.count", -1, "accepted"), ("accepted.sha256", "short", "accepted"),
                                  # Without the scan's random value the report's digest is a guessing aid.
                                  ("nonce", DROP, "nonce"), ("nonce", None, "nonce"), ("nonce", "0f" * 15, "nonce"),
                                  ("nonce", "0F" * 16, "nonce"), ("nonce", 7, "nonce")):
            with self.subTest(privacy=path, value=value):
                self.not_written(privacy=change(privacy, path, value), words=("privacy", word))
        swapped = [dict(credential["detector_builds"][0], role="current"), dict(credential["detector_builds"][1], role="pinned")]
        for path, value, word in (("findings.refused", 1, "refused"), ("findings.refused", "0", "refused"),
                                  ("findings.refused", DROP, "refused"), ("findings.pending_owner", 2, "pending_owner"),
                                  ("unread.unaccepted", 1, "unaccepted"), ("unread", DROP, "unaccepted"),
                                  ("detector_builds", credential["detector_builds"][:1], "detector_builds"),
                                  ("detector_builds", DROP, "detector_builds"), ("detector_builds.1.role", "pinned", "detector_builds"),
                                  ("detector_builds.0.sha256", None, "detector_builds"), ("accepted", DROP, "accepted"),
                                  # A report that does not state the rules file a build read with.
                                  ("detector_builds.0.default_rules_sha256", DROP, "default_rules_sha256"),
                                  ("detector_builds.1.default_rules_sha256", None, "default_rules_sha256"),
                                  ("detector_builds.1.default_rules_sha256", "short", "default_rules_sha256"),
                                  ("pending.count", DROP, "pending")):
            with self.subTest(credential=path, value=value):
                self.not_written(credential=change(credential, path, value), words=("credential", word))
        # The roles decide which build is which, not the order of the rows.
        done, receipt = self.written(credential=change(credential, "detector_builds", swapped))
        self.assertEqual(done.returncode, 0, done.stdout)
        written = json.loads(receipt.read_text())["credential"]
        self.assertEqual((written["detector_pinned_sha256"], written["detector_current_sha256"]), (CURRENT, PINNED))
        self.assertEqual((written["detector_pinned_default_rules_sha256"], written["detector_current_default_rules_sha256"]),
                         (CURRENT_RULES, PINNED_RULES))
        self.refused(self.check(receipt), "credential", "detector")

    def test_a_receipt_of_a_scan_that_read_with_another_rules_file_is_refused_by_the_check(self):
        # The writer holds no pin: it states the rules file each build read with, as the report does.
        # The check holds that to the pins of its checkout, for each build.
        for row, role in enumerate(("pinned", "current")):
            with self.subTest(build=role):
                other = change(self.credential(), "detector_builds.%d.default_rules_sha256" % row, "0" * 64)
                done, receipt = self.written(credential=other)
                self.assertEqual(done.returncode, 0, done.stdout)
                self.assertEqual(json.loads(receipt.read_text())["credential"]["detector_%s_default_rules_sha256" % role], "0" * 64)
                checked = self.check(receipt)
                self.refused(checked, "credential", "default rules files")
                self.assertNotIn("are not the two pinned builds", checked.stdout)
                # A checkout whose commit pins that rules file passes the same receipt.
                self.passed(self.check(receipt, tool=self.checkout({**PINS, "detector_%s_default_rules_sha256" % role: "0" * 64})))

    def test_no_receipt_is_written_when_a_build_gives_no_proof_of_read_for_a_file(self):
        # A file that one build was handed and left unread for lack of proof of read, and an
        # acceptance that covers its bytes: the scan says PASS (one unread file, none without an
        # acceptance). For an upload no entry covers such a file: no receipt.
        reason = "no proof that a build read it (one build; the other build read it)"
        proof = {"handed": 9, "read_as_it_is": 9, "read_behind_the_line": 0, "unread": 0, "readings": 2, "proof_readings": 2}
        for left in ("0.0.1", "0.0.2"):
            proofs = {version: dict(proof, read_as_it_is=8, unread=1) if version == left else dict(proof) for version in ("0.0.1", "0.0.2")}
            covered = change(change(self.credential(), "unread", {"total": 1, "unaccepted": 0, "files": {reason: 1}}), "read_proof", proofs)
            with self.subTest(left=left):
                self.not_written(credential=covered, words=("credential", "read_proof"))
        # A report that does not count such files for each of its two builds gives no receipt either:
        # no counts at all (a scan from before the proof of read), the count of one build alone, a
        # count under another build's name, a count that is no number.
        credential = self.credential()
        both = credential["read_proof"]
        for note, value in (("no counts", DROP), ("no counts", None), ("no counts", {}), ("one build", {"0.0.1": both["0.0.1"]}),
                            ("another build", {"0.0.1": both["0.0.1"], "0.0.3": both["0.0.2"]}),
                            ("a third build", {**both, "0.0.3": both["0.0.2"]}),
                            ("no number", {**both, "0.0.2": dict(both["0.0.2"], unread="0")}),
                            ("no number", {**both, "0.0.2": dict(both["0.0.2"], unread=False)}),
                            ("no number", {**both, "0.0.2": dict(both["0.0.2"], unread=-1)}),
                            ("no count", {**both, "0.0.2": {key: value for key, value in both["0.0.2"].items() if key != "unread"}}),
                            ("no count", {**both, "0.0.2": 0})):
            with self.subTest(note=note, value=value):
                self.not_written(credential=change(credential, "read_proof", value), words=("credential", "read_proof"))
        # The other counts of the proof of read refuse nothing: files read behind the line are read.
        done, _receipt = self.written(credential=change(credential, "read_proof", {
            version: dict(proof, read_as_it_is=0, read_behind_the_line=9) for version in both}))
        self.assertEqual(done.returncode, 0, done.stdout)

    def test_the_receipt_names_the_files_both_reports_name_with_the_same_bytes(self):
        extra = {"name": f"extra-{TARGET}.bin", "bytes": 3, "sha256": "9" * 64}
        # A file only one report names: no receipt at all, not a receipt without that row. Both scans
        # read every file, or nothing is vouched for.
        self.not_written(privacy=change(self.privacy(), "payload", self.payload() + [extra]), words=("payload", extra["name"]))
        self.not_written(credential=change(self.credential(), "payload", self.payload() + [extra]), words=("payload", extra["name"]))
        self.not_written(privacy=change(self.privacy(), "payload", self.payload()[:1]), words=("payload", self.files[1].name))
        both = change(self.credential(), "payload", self.payload() + [extra])
        done, receipt = self.written(privacy=change(self.privacy(), "payload", self.payload() + [extra]), credential=both)
        self.assertEqual(done.returncode, 0, done.stdout)
        self.assertEqual(json.loads(receipt.read_text())["payload"], sorted(self.payload() + [extra], key=lambda row: row["name"]))
        # The same name with other bytes in the two reports, no file in common, or rows that are none.
        for path, value in (("payload.0.sha256", "0" * 64), ("payload.0.bytes", 1)):
            self.not_written(credential=change(self.credential(), path, value), words=("payload", self.files[0].name))
        self.not_written(credential=change(self.credential(), "payload", [extra]), words=("payload", "no file in common"))
        named = [{**row, "name": "PROOF-ONLY-" + row["name"]} for row in self.payload()]
        for value in ([], DROP, "rows", self.payload() + self.payload()[:1], named, [{"name": "x"}]):
            self.not_written(privacy=change(self.privacy(), "payload", value), words=("privacy", "payload"))
        # A receipt whose rows are not the files as they are at the upload refuses at the check.
        done, receipt = self.written()
        self.passed(self.check(receipt))
        with open(self.files[1], "ab") as stream:
            stream.write(b"changed after the scans\n")
        self.refused(self.check(receipt), "receipt", "payload", self.files[1].name)

    def test_one_pair_of_reports_gives_the_receipt_of_every_target_of_a_staging_directory(self):
        # A release's staging directory: a platform's archives and its two evidence files, and the five
        # files that hold no platform's name. Both scans read all of them in one run each.
        evidence = self.named(f"product-{VERSION}-{TARGET}.evidence.json", f"product-{VERSION}-{TARGET}.evidence.tar.gz")
        platform, release = self.files + evidence, self.release_files()
        staged = platform + release
        done, receipts = self.written_for(staged, [TARGET, RELEASE], VERSION)
        self.assertEqual((done.returncode, done.stderr), (0, ""), done.stdout)
        # One line per receipt, in the order given; each receipt holds exactly its target's rows.
        written = {target: json.loads(path.read_text()) for target, path in receipts.items()}
        self.assertEqual(done.stdout.splitlines(), [
            "UPLOAD-RECEIPT WRITTEN target=%s files=%d receipt_sha256=%s" % (target, count, sha256(receipts[target].read_bytes()))
            for target, count in ((TARGET, 4), (RELEASE, 5))])
        for target, files in ((TARGET, platform), (RELEASE, release)):
            self.assertEqual(written[target]["target"], target)
            self.assertEqual(written[target]["payload"], sorted(self.payload(files), key=lambda row: row["name"]))
        # The same two reports stand behind both.
        self.assertEqual({key: value for key, value in written[TARGET].items() if key not in ("target", "payload")},
                         {key: value for key, value in written[RELEASE].items() if key not in ("target", "payload")})
        # Each passes the check with exactly its files, and with no part or mix of them.
        self.passed(self.check(receipts[TARGET], files=platform))
        self.passed(self.check(receipts[RELEASE], files=release, target=RELEASE))
        self.refused(self.check(receipts[TARGET], files=self.files), "receipt", "payload", evidence[0].name)
        self.refused(self.check(receipts[TARGET], files=staged), "files", release[0].name)
        self.refused(self.check(receipts[RELEASE], files=staged, target=RELEASE), "files", platform[0].name)
        # The order of the pairs is the caller's; the rows are the same.
        done, swapped = self.written_for(staged, [RELEASE, TARGET], VERSION)
        self.assertEqual([line.split()[2] for line in done.stdout.splitlines()], ["target=" + RELEASE, "target=" + TARGET])
        self.assertEqual(json.loads(swapped[TARGET].read_text())["payload"], written[TARGET]["payload"])

        # All or nothing. A staged file that is a name of none of the targets given: no receipt at all.
        for name in ("notes.txt", f"product-0.0.0-{OTHER_TARGET}.tar.gz", f"rkat-toolkit-{VERSION}.tar.gz", "sha256sums"):
            with self.subTest(stray=name):
                stray = self.named(name, directory="stray")
                self.none_written(*self.written_for(staged + stray, [TARGET, RELEASE], VERSION), "payload", name)
        # The release's own files are no rows of a platform's receipt, and are not left out silently.
        self.none_written(*self.written_for(staged, [TARGET]), "payload", release[0].name)
        self.none_written(*self.written_for(staged, [RELEASE], VERSION), "payload", platform[0].name)
        # A target that gets no row.
        self.none_written(*self.written_for(platform, [TARGET, RELEASE], VERSION), "payload", RELEASE)
        self.none_written(*self.written_for(release, [TARGET, RELEASE], VERSION), "payload", TARGET)
        # A file one report alone names, among many.
        self.none_written(*self.written_for(staged, [TARGET, RELEASE], VERSION, privacy_files=staged[:-1]), "payload", staged[-1].name)
        self.none_written(*self.written_for(staged[1:], [TARGET, RELEASE], VERSION, privacy_files=staged), "payload", staged[0].name)
        # The version: needed for exact names, in its one form, and it means nothing without them.
        self.none_written(*self.written_for(staged, [TARGET, RELEASE], None), "--version")
        self.none_written(*self.written_for(staged, [TARGET, RELEASE], "1.2.4"), "payload", f"rkat-toolkit-{VERSION}.tgz")
        self.none_written(*self.written_for(platform, [TARGET], None), "--version")
        self.none_written(*self.written_for(release, [RELEASE], None), "--version")
        self.none_written(*self.written_for(staged, [TARGET, RELEASE], "v" + VERSION), code=2)
        # The pairs: as many receipts as targets, each target and each file once.
        other = self.root / "another-receipt.json"
        self.none_written(*self.written_for(staged, [TARGET, RELEASE], VERSION, extra=["--receipt", other]), code=2)
        self.none_written(*self.written_for(staged, [TARGET, RELEASE], VERSION, extra=["--target", OTHER_TARGET]), code=2)
        self.none_written(*self.written_for(staged, [TARGET, RELEASE], VERSION, extra=["--target", TARGET, "--receipt", other]), code=2)
        self.assertFalse(other.exists())
        done, receipts = self.written_for(staged, [TARGET, RELEASE], VERSION)
        first = receipts[TARGET]
        done = run(self.tool.with_name(WRITE.name), "--privacy-report", self.write("privacy-report", change(self.privacy(), "payload", self.payload(staged))),
                   "--credential-report", self.write("credential-report", change(self.credential(), "payload", self.payload(staged))),
                   "--version", VERSION, "--target", TARGET, "--receipt", other, "--target", RELEASE, "--receipt", other)
        self.assertEqual((done.returncode, other.exists()), (2, False))
        # A receipt that exists already: nothing is written, not even the other target's receipt.
        before = first.read_bytes()
        done = run(self.tool.with_name(WRITE.name), "--privacy-report", self.write("privacy-report", change(self.privacy(), "payload", self.payload(staged))),
                   "--credential-report", self.write("credential-report", change(self.credential(), "payload", self.payload(staged))),
                   "--version", VERSION, "--target", RELEASE, "--receipt", other, "--target", TARGET, "--receipt", first)
        self.assertEqual((done.returncode, other.exists(), first.read_bytes()), (2, False, before))
        self.assertNotIn("UPLOAD-RECEIPT WRITTEN", done.stdout)

    def test_the_writer_sorts_the_files_by_the_committed_name_rules(self):
        staged = self.files + self.release_files()
        # A target the pins file does not have, and a pins file that is not a commit's or not of its schema.
        self.none_written(*self.written_for(self.files, ["plan9-mips"]), "pins", "plan9-mips")
        none = self.checkout(pins_text({TARGET: PINS}))
        self.none_written(*self.written_for(staged, [TARGET, RELEASE], VERSION, tool=none), "pins", RELEASE)
        edited = self.checkout()
        release_file(edited, PINS_NAME).write_text(pins_text({TARGET: PINS, RELEASE: section(PINS, RELEASE_NAMES + ["notes.txt"])}))
        self.none_written(*self.written_for(staged + self.named("notes.txt"), [TARGET, RELEASE], VERSION, tool=edited), "pins")
        self.none_written(*self.written_for(self.files, [TARGET], tool=self.checkout(committed=False)), "pins")
        broken = self.checkout(pins_text({TARGET: PINS, RELEASE: section(PINS, RELEASE_NAMES + [f"notes-{TARGET}.txt"])}))
        self.none_written(*self.written_for(self.files, [TARGET], tool=broken), "pins")
        # The rules are the commit's: another commit's list gives a file another target, or none.
        wider = self.checkout(pins_text({TARGET: PINS, RELEASE: section(PINS, RELEASE_NAMES + ["notes.txt"])}))
        done, receipts = self.written_for(staged + self.named("notes.txt"), [TARGET, RELEASE], VERSION, tool=wider)
        self.assertEqual(done.returncode, 0, done.stdout + done.stderr)
        self.assertIn("notes.txt", [row["name"] for row in json.loads(receipts[RELEASE].read_text())["payload"]])
        # A file that is a name of two targets gets no row, whichever of them is given: the writer
        # holds a name to exactly one target of the whole pins file, as the check does.
        digits = self.checkout(pins_text({"7": PINS, TARGET: PINS, RELEASE: section(PINS, RELEASE_NAMES)}))
        seven = self.release_files("1.7.3")
        held = [path.name for path in seven if "7" in path.name]
        self.assertEqual(held, ["meerkat_toolkit-1.7.3-py3-none-any.whl", "rkat-toolkit-1.7.3.tgz"])
        self.none_written(*self.written_for(seven, [RELEASE], "1.7.3", tool=digits), "payload", *held)
        self.none_written(*self.written_for(seven, [RELEASE, "7"], "1.7.3", tool=digits), "payload", *held)
        both = self.named(f"product-{TARGET}-{OTHER_TARGET}.tar.gz")
        self.none_written(*self.written_for(self.files + both, [TARGET]), "payload", both[0].name)
        self.none_written(*self.written_for(both, [OTHER_TARGET]), "payload", both[0].name)
        done, receipts = self.written_for(self.release_files(), [RELEASE], VERSION, tool=digits)
        self.assertEqual(done.returncode, 0, done.stdout + done.stderr)
        # A platform's file named for another version than the release's is a file of no target given.
        foreign = self.named(f"product-9.9.9-{TARGET}.tar.gz")
        self.none_written(*self.written_for(self.files + foreign, [TARGET]), "payload", foreign[0].name)
        self.none_written(*self.written_for(self.files, [TARGET], "9.9.9"), "payload", self.files[0].name)
        done, receipts = self.written_for(foreign, [TARGET], "9.9.9")
        self.assertEqual(done.returncode, 0, done.stdout + done.stderr)
        # Whether a pin is set is the check's to say, not the writer's: the receipt is written and refuses there.
        unset = self.checkout({**PINS, "name_list_sha256": None})
        done, receipts = self.written_for(self.files, [TARGET], tool=unset)
        self.assertEqual(done.returncode, 0, done.stdout + done.stderr)
        self.refused(self.check(receipts[TARGET], tool=unset), "pins", "name_list_sha256")

    def test_no_receipt_is_written_unless_every_rule_of_a_name_class_is_blocking(self):
        # A review class refuses nothing: its names pass a scan. Only the path category may be a review
        # category, and the report says how many review rules lie outside it.
        privacy = self.privacy()
        for value in (1, 2, None, DROP, "0", False, True, -1):
            with self.subTest(review_outside_paths=value):
                self.not_written(privacy=change(privacy, "rules.review_outside_paths", value), words=("privacy", "rules", "review"))
        # Counts that cannot stand together: more review rules than the one path rule, with none outside.
        for path, value in (("rules.blocking", 4), ("rules.total", 8)):
            self.not_written(privacy=change(privacy, path, value), words=("privacy", "rules", "review"))
        # A report of the scan as it was before it counted them shows nothing, so nothing is written.
        self.not_written(privacy=change(privacy, "rules", {"sha256": RULES, "total": 6, "blocking": 5}), words=("privacy", "rules", "review"))
        # Blocking paths as well, or review paths alone: both are written, and the receipt says so as a fact.
        for blocking in (6, 5):
            done, receipt = self.written(privacy=change(privacy, "rules.blocking", blocking))
            self.assertEqual(done.returncode, 0, done.stdout + done.stderr)
            self.assertIs(json.loads(receipt.read_text())["privacy"]["name_rules_all_blocking"], True)
            self.passed(self.check(receipt))

    def test_no_receipt_is_written_from_a_scan_that_ran_with_an_exemption(self):
        # An exemption switches a match off. The exemptions file is no committed file and no pin holds
        # it, so a release scan runs with none, and the receipt says so as a fact.
        privacy = self.privacy()
        for count in (1, 2, 40):
            with self.subTest(exemptions=count):
                self.not_written(privacy=change(privacy, "exemptions.count", count), words=("privacy", "exemptions"))
        for path, value in (("exemptions.count", "0"), ("exemptions.count", None), ("exemptions.count", False),
                            ("exemptions.count", DROP), ("exemptions", DROP), ("exemptions", 0), ("exemptions", [])):
            self.not_written(privacy=change(privacy, path, value), words=("privacy", "exemptions"))
        done, receipt = self.written()
        self.assertEqual(done.returncode, 0, done.stdout + done.stderr)
        self.assertIs(json.loads(receipt.read_text())["privacy"]["no_exemptions"], True)
        self.assertNotIn("d4" * 32, receipt.read_text())

    def test_no_receipt_is_written_from_a_report_that_says_pass_and_shows_a_refusal(self):
        # The writer compares what it reads and does not take the status word for it: a damaged or
        # mixed report that says PASS beside a reason, a refused hit or error, an exemption in a
        # blocking category or a failure of the walk gives no receipt.
        privacy = self.privacy()
        self.assertEqual((privacy["status"], privacy["exit_code"]), ("PASS", 0))
        for path, value, word in (
                ("reasons", ["hits-refused"], "reasons"), ("reasons", ["no-blocking-rules", "home-paths-off"], "reasons"),
                ("reasons", DROP, "reasons"), ("reasons", None, "reasons"), ("reasons", "", "reasons"), ("reasons", {}, "reasons"),
                ("hits.refused", {"madeupclass": 1}, "hits.refused"), ("hits.refused", {"withheld_member_reference": 2}, "hits.refused"),
                ("hits.refused", DROP, "hits.refused"), ("hits.refused", 0, "hits.refused"), ("hits.refused", [], "hits.refused"),
                ("hits", DROP, "hits.refused"), ("hits", [], "hits.refused"),
                ("errors.refused", {"corrupt-archive": 1}, "errors.refused"), ("errors.refused", DROP, "errors.refused"),
                ("errors", DROP, "errors.refused"), ("errors", {}, "errors.refused"),
                ("exemptions.blocking", 1, "exemptions.blocking"), ("exemptions.blocking", DROP, "exemptions.blocking"),
                ("exemptions.blocking", None, "exemptions.blocking"), ("exemptions.blocking", "0", "exemptions.blocking"),
                ("wrapper_failures", 1, "wrapper_failures"), ("wrapper_failures", DROP, "wrapper_failures"),
                ("wrapper_failures", None, "wrapper_failures"), ("wrapper_failures", False, "wrapper_failures"),
                ("wrapper_failures", [], "wrapper_failures")):
            with self.subTest(privacy=path, value=value):
                self.not_written(privacy=change(privacy, path, value), words=("privacy", word))
        # What was accepted, covered or only counted for review refuses nothing, and is not in the receipt.
        done, receipt = self.written(privacy=change(change(privacy, "hits.accepted", {"madeupclass": 40}), "review_hits", {"review_home_paths": 9}))
        self.assertEqual(done.returncode, 0, done.stdout + done.stderr)
        self.passed(self.check(receipt))

    def test_no_receipt_is_written_when_the_engines_exit_code_does_not_stand_with_the_report(self):
        # The engine's exit code says whether it saw a hit (1) and whether it saw an error (2). A
        # passing report shows where each went: hits accepted or counted for review, errors covered,
        # accepted or excepted. A code that says more, or less, than the report shows is not that report's.
        privacy = self.privacy()
        self.assertEqual(privacy["engine"]["exit_code"], 3)
        for value in (0, 1, 2, 4, 7, -1, "3", None, True, 3.0, DROP):
            with self.subTest(exit_code=value):
                self.not_written(privacy=change(privacy, "engine.exit_code", value), words=("privacy", "engine"))
        for value in (DROP, [], 3, "3"):
            self.not_written(privacy=change(privacy, "engine", value), words=("privacy", "engine"))
        # What the report shows decides what the code must be.
        quiet = privacy
        for path, value in (("hits.accepted", {}), ("review_hits", {}), ("errors.covered", {}), ("errors.accepted", {}),
                            ("accepted.used", {"member": 0, "archive": 0}), ("accepted.used_by_tool", {})):
            quiet = change(quiet, path, value)
        proved = change(quiet, "accepted.used_by_tool", {"zq-prover-one": 1})
        shows = {"nothing": (quiet, 0),
                 "an accepted hit": (change(proved, "hits.accepted", {"madeupclass": 1}), 1),
                 "a review hit": (change(quiet, "review_hits", {"review_home_paths": 2}), 1),
                 "a covered error": (change(quiet, "errors.covered", {"depth-limit": 1}), 2),
                 "an accepted error": (change(proved, "errors.accepted", {"corrupt-archive": 1}), 2),
                 "an excepted error": (change(quiet, "coverage_exceptions.used", {"archive-layout": 1}), 2),
                 "a hit and an error": (change(change(quiet, "review_hits", {"review_home_paths": 1}), "errors.covered", {"depth-limit": 2}), 3)}
        for label, (report, expected) in shows.items():
            for code in (0, 1, 2, 3):
                with self.subTest(report=label, exit_code=code):
                    value = change(report, "engine.exit_code", code)
                    if code == expected:
                        done, _ = self.written(privacy=value)
                        self.assertEqual(done.returncode, 0, done.stdout + done.stderr)
                    else:
                        self.not_written(privacy=value, words=("privacy", "engine"))
        # Counts that are no counts cannot be compared.
        for path, value in (("hits.accepted", {"madeupclass": "2"}), ("review_hits", []), ("errors.covered", {"depth-limit": -1}),
                            ("errors.accepted", DROP), ("coverage_exceptions.used", None), ("review_hits", DROP)):
            self.not_written(privacy=change(privacy, path, value), words=("privacy", "engine"))

    def test_no_receipt_is_written_unless_every_used_acceptance_entry_names_its_proof_tool(self):
        # A finding is accepted only by a tool's proof: every acceptance entry the scan used carries a
        # proof object that names its tool. The report counts the used entries that do not, and the
        # used entries by tool; the receipt states the fact and carries the tools with their counts.
        privacy = self.privacy()
        for value in (1, 2, None, DROP, "0", False, True, -1):
            with self.subTest(used_unproved=value):
                self.not_written(privacy=change(privacy, "accepted.used_unproved", value), words=("privacy", "accepted", "proof"))
        for value in (DROP, None, [], "zq-prover-one", {"": 2}, {"  ": 2}, {"zq-prover-one": 0, "zq-prover-two": 2}, {"zq-prover-one": "2"},
                      {"zq-prover-one": True}, {"zq-prover-one": -1}, {"zq-prover-one": 1.0},
                      # more used entries than the file holds, and none used while something was accepted
                      {"zq-prover-one": 3}, {"zq-prover-one": 2, "zq-prover-two": 1}, {},
                      # a name outside the one form of a proof tool's name: it would reach the receipt as written
                      {"a tool with a longer name": 1, "zq-prover-two": 1}, {"accepted by hand.": 2}, {"tools/zq-prover.py": 2},
                      {"zq-prover.py": 2}, {"Zq-prover-one": 2}, {"zq_prover_one": 2}, {"zq--prover": 2}, {"-zq-prover": 2},
                      {" zq-prover-one": 2}, {"zq-prover-one\n": 2}, {"zq-prover\none": 2}, {"zq-pr" + chr(0xf6) + "ver": 2}, {"z" * 65: 2},
                      {"z" * 100000: 2}):
            with self.subTest(used_by_tool=value if len(str(value)) < 200 else "a long name"):
                self.not_written(privacy=change(privacy, "accepted.used_by_tool", value), words=("privacy", "accepted", "proof"))
        # Names of that form: one character, a digit first, 64 characters.
        done, receipt = self.written(privacy=change(privacy, "accepted.used_by_tool", {"7": 1, "z" * 64: 1}))
        self.assertEqual(done.returncode, 0, done.stdout + done.stderr)
        self.assertEqual(json.loads(receipt.read_text())["privacy"]["accepted_proof_tools"], {"7": 1, "z" * 64: 1})
        self.passed(self.check(receipt))
        # A report of the scan as it was before it counted them shows nothing, so nothing is written.
        older = change(privacy, "accepted", {"count": 2, "sha256": "e5" * 32, "used": {"member": 2, "archive": 0}})
        self.not_written(privacy=older, words=("privacy", "accepted", "proof"))
        done, receipt = self.written()
        self.assertEqual(done.returncode, 0, done.stdout + done.stderr)
        written = json.loads(receipt.read_text())["privacy"]
        self.assertIs(written["accepted_only_with_proof"], True)
        self.assertEqual(written["accepted_proof_tools"], PROOF_TOOLS)
        self.passed(self.check(receipt))
        # Nothing accepted: no tool, and a tool named for nothing is not that report's.
        quiet = privacy
        for path, value in (("hits.accepted", {}), ("errors.accepted", {}), ("accepted.used", {"member": 0, "archive": 0}),
                            ("engine.exit_code", 3)):
            quiet = change(quiet, path, value)
        self.not_written(privacy=quiet, words=("privacy", "accepted", "proof"))
        done, receipt = self.written(privacy=change(quiet, "accepted.used_by_tool", {}))
        self.assertEqual(done.returncode, 0, done.stdout + done.stderr)
        self.assertEqual(json.loads(receipt.read_text())["privacy"]["accepted_proof_tools"], {})
        self.passed(self.check(receipt))

    def test_no_receipt_is_written_unless_each_scans_own_file_is_its_commits(self):
        # A report states the commit its scan tool ran from and the sha256 of that tool. The writer runs
        # where the scans ran: it compares that digest with the scan's own file as that commit holds
        # it in the writer's checkout. A report of another tool, or of a commit the checkout does not
        # hold, gives no receipt.
        names = dict(zip(("privacy", "credential"), SCAN_TOOLS))
        for which, name in names.items():
            base = getattr(self, which)()
            for digest in ("9a" * 32, sha256(b"# another scan tool\n")):
                self.not_written(**{which: change(base, "tool_sha256", digest)}, words=(which, "tool_sha256", name))
            for commit_id in ("d" * 40, COMMIT[:-1] + ("0" if COMMIT[-1] != "0" else "1")):
                self.not_written(**{which: change(base, "tool_commit", commit_id)}, words=(which, "tool_commit", name))
        # Each report is held to its own scan's file: the two digests the other way round are neither's.
        self.not_written(privacy=change(self.privacy(), "tool_sha256", CREDENTIAL_TOOL),
                         credential=change(self.credential(), "tool_sha256", PRIVACY_TOOL), words=("privacy", "credential", "tool_sha256"))
        # The commit decides, not the checkout's head and not the work tree. A later commit changes the
        # private-text scan's file: reports of the release commit still stand, a report that names
        # the later commit must state the later file.
        tool = self.checkout()
        repository, changed = tool.parent.parent, b"# the private-text scan tool, changed in a later commit\n"
        (repository / "scripts" / names["privacy"]).write_bytes(changed)
        git(repository, "add", "-A")
        git(repository, "commit", "-q", "-m", "a later commit changes a scan tool")
        later = git(repository, "rev-parse", "HEAD")
        self.assertEqual(self.written(tool=tool)[0].returncode, 0)
        moved = change(self.privacy(), "tool_commit", later)
        self.not_written(privacy=moved, tool=tool, words=("privacy", "tool_sha256", names["privacy"]))
        self.assertEqual(self.written(privacy=change(moved, "tool_sha256", sha256(changed)), tool=tool)[0].returncode, 0)
        self.not_written(privacy=change(self.privacy(), "tool_sha256", sha256(changed)), tool=tool, words=("privacy", "tool_sha256"))
        (repository / "scripts" / names["credential"]).write_bytes(b"# edited in the work tree, in no commit\n")
        self.assertEqual(self.written(tool=tool)[0].returncode, 0)
        self.not_written(credential=change(self.credential(), "tool_sha256", sha256(b"# edited in the work tree, in no commit\n")),
                         tool=tool, words=("credential", "tool_sha256"))
        # A replace ref puts no other file in the commit's place.
        other = self.checkout()
        repository = other.parent.parent
        (repository / "scripts" / names["privacy"]).write_bytes(changed)
        git(repository, "replace", git(repository, "rev-parse", COMMIT + ":scripts/" + names["privacy"]),
            git(repository, "hash-object", "-w", "--", repository / "scripts" / names["privacy"]))
        self.assertEqual(git(repository, "cat-file", "blob", COMMIT + ":scripts/" + names["privacy"]), changed.decode().strip())
        self.not_written(privacy=change(self.privacy(), "tool_sha256", sha256(changed)), tool=other, words=("privacy", "tool_sha256"))
        self.assertEqual(self.written(tool=other)[0].returncode, 0)

    def test_no_receipt_is_written_unless_the_class_list_of_a_credential_report_is_its_commits(self):
        # A credential report that names an identifier class proof states the sha256 of the class list
        # that was in force. The writer compares it with the list as the report's commit holds it in
        # the writer's checkout, as it does for the scan's own file. A report without that part
        # resolved nothing by class and is taken as before (every other row of this class).
        name = "payload-credential-identifier-classes.json"
        listed = b'{"made-up": "class list"}\n'
        tool = self.checkout()
        repository = tool.parent.parent
        (repository / "release" / name).write_bytes(listed)
        git(repository, "add", "-A")
        git(repository, "commit", "-q", "-m", "a later commit holds a class list")
        later = git(repository, "rev-parse", "HEAD")
        proof = {"tool": "credential-identifier-class", "resolved": 1, "list_sha256": sha256(listed)}
        report = change(change(self.credential(), "tool_commit", later), "identifier_classes", proof)
        done, receipt = self.written(credential=report, tool=tool)
        self.assertEqual(done.returncode, 0, done.stdout + done.stderr)
        # The receipt holds nothing of it: its fields are the ones it had.
        self.assertEqual(set(json.loads(receipt.read_text())["credential"]), set(self.receipt()["credential"]))
        # Another list, no digest, or a part that is no such statement: no receipt.
        for value in ("9a" * 32, sha256(b'{"another": "class list"}\n'), sha256(listed).upper(), None, 7, "text", DROP):
            self.not_written(credential=change(report, "identifier_classes.list_sha256", value), tool=tool,
                             words=("credential", "identifier_classes", name))
        for value in ("text", [], None, True, 3):
            self.not_written(credential=change(report, "identifier_classes", value), tool=tool, words=("credential", "identifier_classes"))
        # A commit that holds no class list vouches for none: the release commit of these rows holds none.
        self.not_written(credential=change(report, "tool_commit", COMMIT), tool=tool, words=("credential", "identifier_classes", name))
        # The commit decides, not the work tree, and not what git's index is told to overlook: a report
        # of a scan that ran with an edited list states the edited list, and gets no receipt.
        changed = b'{"made-up": "class list, edited in the work tree"}\n'
        git(repository, "update-index", "--assume-unchanged", "--", "release/" + name)
        (repository / "release" / name).write_bytes(changed)
        self.assertEqual(git(repository, "status", "--porcelain"), "")
        self.not_written(credential=change(report, "identifier_classes.list_sha256", sha256(changed)), tool=tool,
                         words=("credential", "identifier_classes", name))
        self.assertEqual(self.written(credential=report, tool=tool)[0].returncode, 0)

    def test_acceptance_inputs_can_be_bound_to_the_present_files(self):
        accepted = self.root / "credential-accepted.json"
        accepted.write_bytes(b"{}\n")
        credential = change(self.credential(), "accepted.sha256", sha256(accepted.read_bytes()))
        absent = self.root / "absent.json"
        ok = ["--credential-accepted-file", accepted, "--credential-pending-file", absent]
        self.assertEqual(self.written(credential=credential, extra=ok)[0].returncode, 0)
        accepted.write_bytes(b"{\"changed\": 1}\n")
        self.not_written(credential=credential, extra=ok, words=("credential", "accepted"))
        self.not_written(extra=["--credential-accepted-file", absent], words=("credential", "accepted"))
        pending = change(change(self.credential(), "pending.sha256", "1" * 64), "pending.count", 1)
        self.not_written(credential=pending, extra=["--credential-pending-file", absent], words=("credential", "pending"))
        self.assertEqual(self.written(credential=pending, extra=["--credential-pending-sha256", "1" * 64])[0].returncode, 0)
        self.not_written(extra=["--privacy-accepted-sha256", "0" * 64], words=("privacy", "accepted"))
        self.assertEqual(self.written(extra=["--privacy-accepted-sha256", "e5" * 32])[0].returncode, 0)
        self.assertEqual(self.written(privacy=change(self.privacy(), "accepted.sha256", None),
                                      extra=["--privacy-accepted-sha256", "none"])[0].returncode, 0)
        # One binding in two forms, and a digest that is none, are usage errors; nothing is written.
        for extra in (["--privacy-accepted-sha256", "e5" * 32, "--privacy-accepted-file", accepted], ["--privacy-accepted-sha256", "short"]):
            done, receipt = self.written(extra=extra)
            self.assertEqual((done.returncode, receipt.exists()), (2, False))

    def test_the_receipt_is_a_new_file_and_never_named_proof_only(self):
        done, receipt = self.written()
        self.assertEqual(done.returncode, 0)
        before = receipt.read_bytes()
        again, _ = self.written(name=receipt.name)
        self.assertEqual((again.returncode, receipt.read_bytes()), (2, before))
        done, receipt = self.written(name="PROOF-ONLY-upload-receipt.json")
        self.assertEqual((done.returncode, receipt.exists()), (2, False))
        done, receipt = self.written(target="Not A Target")
        self.assertEqual((done.returncode, receipt.exists()), (2, False))

    def test_a_report_the_credential_scan_wrote_is_accepted_only_for_its_own_commit(self):
        fixtures = load("credential_scan_fixtures", HERE / "test_release_payload_credential_scan.py")
        pinned, current = fixtures.write_builds(self.root / "builds")
        # One checkout holds the scan, the writer, the check and the pins of the two builds the scan
        # runs with and of the default rules file beside each: the commit that is released is its head.
        tool = self.checkout({**PINS, "detector_pinned_sha256": sha256(Path(pinned).read_bytes()),
                              "detector_current_sha256": sha256(Path(current).read_bytes()),
                              "detector_pinned_default_rules_sha256": sha256(Path(pinned).with_name("gitleaks.toml").read_bytes()),
                              "detector_current_default_rules_sha256": sha256(Path(current).with_name("gitleaks.toml").read_bytes())})
        scanner = tool.parent.parent
        for source in (fixtures.TOOL, fixtures.EXTRACTOR):
            shutil.copyfile(source, scanner / "scripts" / source.name)
        git(scanner, "add", "-A")
        git(scanner, "commit", "-q", "-m", "the credential scan and its extractor")
        head = git(scanner, "rev-parse", "HEAD")
        product = self.files[0]
        product.write_bytes(fixtures.tar_bytes({"pkg/notes.txt": b"plain\n"}))

        def scanned(name, *extra):
            """The report of a scan of the files by the checkout's scan, with the two stand-in builds."""
            report = self.root / (name + "-report.json")
            done = subprocess.run([sys.executable, "-I", str(scanner / "scripts" / fixtures.TOOL.name), "--name", "X0",
                                   "--gitleaks-pinned", str(pinned), "--gitleaks-current", str(current),
                                   "--receipt", str(self.root / (name + "-receipt.txt")), "--report", str(report),
                                   "--work", str(self.root), *extra, *map(str, self.files)], capture_output=True, text=True)
            self.assertEqual(done.returncode, 0, done.stderr)
            return report

        report = scanned("credential")
        self.assertEqual(json.loads(report.read_text())["tool_commit"], head)
        self.assertEqual(json.loads(report.read_text())["tool_sha256"], sha256(fixtures.TOOL.read_bytes()))
        done, receipt = self.written(privacy=change(self.privacy(), "tool_commit", head), credential=report, tool=tool)
        self.assertEqual(done.returncode, 0, done.stdout + done.stderr)
        # The checkout judges it: it pins the two builds the scan ran with, and their rules files.
        self.passed(self.check(receipt, commit=head, tool=tool))
        # The writer of a checkout that does not hold that commit cannot tell whose tool wrote the report.
        self.not_written(privacy=change(self.privacy(), "tool_commit", head), credential=report, words=("tool_commit",))
        self.refused(self.check(receipt, commit="d" * 40, tool=tool), "tool_commit")
        self.refused(self.check(receipt, commit=head), "credential", "detector")
        # The same scan with another rules file for one build, one that finds what the build's own
        # finds here (the same text and a comment): the scan passes, and the receipt written from its
        # report is refused. The builds are the pinned ones; the rules file one of them read with is not.
        other = self.root / "other-rules.toml"
        other.write_text(fixtures.DEFAULT_RULES + "# the same rules, other bytes\n")
        report = scanned("other-rules", "--default-config-current", str(other))
        done, receipt = self.written(privacy=change(self.privacy(), "tool_commit", head), credential=report, tool=tool)
        self.assertEqual(done.returncode, 0, done.stdout + done.stderr)
        checked = self.check(receipt, commit=head, tool=tool)
        self.refused(checked, "credential", "default rules files")
        self.assertNotIn("are not the two pinned builds", checked.stdout)


if __name__ == "__main__":
    unittest.main()
