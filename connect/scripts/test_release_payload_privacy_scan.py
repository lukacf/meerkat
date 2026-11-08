"""Release payload privacy scan: the pinned engine through every nested container.

Synthetic rules only: the marker below is made up, and no real rule, name or release byte is used.
The candidate rows reuse the inert assembly fixtures; no native build or acceptance runs.
"""
from pathlib import Path
import copy
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
import unittest
import zipfile
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

    def accepted(self, entries):
        self.count += 1
        path = self.root / f"accepted-{self.count}.json"
        dump(path, {"schema": "release-payload-privacy-scan/accepted/1", "accepted": entries})
        return path

    def scan(self, *payload, accepted=None, rules=RULES, exemptions=EXEMPTIONS, wrapper=WRAPPER):
        self.count += 1
        report, detail = self.root / f"report-{self.count}.json", self.root / f"detail-{self.count}.json"
        args = ["--report", report, "--detail", detail, "--work", self.root]
        if rules is not None:
            args += ["--rules", rules]
        if exemptions is not None:
            args += ["--exemptions", exemptions]
        if accepted is not None:
            args += ["--accepted-findings", accepted]
        done = run(wrapper, *args, *payload)
        value = json.loads(report.read_text()) if report.exists() else None
        if value is not None:
            self.assertNotIn(MARKER.decode(), report.read_text())
            self.assertEqual(detail.stat().st_mode & 0o777, 0o600)
        return done.returncode, value

    def engine_alone(self, payload):
        self.count += 1
        report, detail = self.root / f"engine-{self.count}.json", self.root / f"engine-detail-{self.count}.json"
        done = run(ENGINE, "--rules", RULES, "--exemptions", EXEMPTIONS, "--report", report, "--detail", detail, payload)
        return done.returncode, json.loads(report.read_text())

    def test_clean_nested_product_passes(self):
        code, report = self.scan(self.payload(product_bytes(
            {"selected/provenance/demo/source.tar.gz": source_archive(b"public notes\n")})))
        self.assertEqual((code, report["status"]), (0, "clean"))
        self.assertEqual(report["hits"], {"accepted": {}, "refused": {}})

    def test_marker_in_source_archive_in_library_in_product_is_refused(self):
        code, report = self.scan(self.payload(product_bytes(
            {"selected/provenance/demo/source.tar.gz": source_archive(b"notes " + MARKER + b"\n")})))
        self.assertEqual((code, report["status"]), (1, "refused"))
        self.assertEqual(report["hits"]["refused"], {"synthetic_marker": 1})

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

    def test_undecodable_nested_containers_refuse(self):
        for broken in (source_archive(b"public notes\n")[:-20], b"7z\xbc\xaf\x27\x1c" + b"\x00" * 64):
            with self.subTest(head=broken[:4]):
                code, report = self.scan(self.payload(product_bytes({"selected/provenance/demo/source.tar.gz": broken})))
                self.assertEqual((code, report["status"]), (2, "refused"))
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
        first = wheel_bytes({"thirdparty/data.txt": b"third party " + MARKER + b"\n"})
        second = wheel_bytes({"thirdparty/data.txt": b"third party " + MARKER + b" again\n"})
        accepted = self.accepted([{"category": "synthetic_marker", "rule": "synthetic-marker-1",
                                   "sha256": hashlib.sha256(first).hexdigest(), "reason": "synthetic third-party bytes"}])
        name = "consumers/demo/thirdparty-1.0-py3-none-any.whl"
        code, report = self.scan(self.payload(product_bytes({name: first})), accepted=accepted)
        self.assertEqual((code, report["hits"]), (0, {"accepted": {"synthetic_marker": 1}, "refused": {}}))
        code, report = self.scan(self.payload(product_bytes({name: second})), accepted=accepted)
        self.assertEqual((code, report["hits"]), (1, {"accepted": {}, "refused": {"synthetic_marker": 1}}))
        other_rule = self.accepted([{"category": "synthetic_marker", "rule": "another-rule",
                                     "sha256": hashlib.sha256(first).hexdigest(), "reason": "synthetic"}])
        code, report = self.scan(self.payload(product_bytes({name: first})), accepted=other_rule)
        self.assertEqual(code, 1)

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
        for entry in ({"category": "synthetic_marker", "rule": "synthetic-marker-1", "sha256": "ab", "reason": "x"},
                      {"category": "synthetic_marker", "rule": "synthetic-marker-1", "sha256": "0" * 64, "reason": ""},
                      {"category": "structure", "rule": "member-too-large", "sha256": "0" * 64, "reason": "x"},
                      {"category": "synthetic_marker", "rule": "synthetic-marker-1", "sha256": "0" * 64}):
            with self.subTest(entry=entry):
                code, report = self.scan(payload, accepted=self.accepted([entry]))
                self.assertEqual((code, report), (4, None))

    def test_a_changed_engine_is_refused(self):
        copies = self.root / "copies"
        copies.mkdir()
        shutil.copyfile(WRAPPER, copies / WRAPPER.name)
        (copies / ENGINE.name).write_bytes(ENGINE.read_bytes() + b"\n# changed\n")
        code, report = self.scan(self.payload(product_bytes({"notes.txt": b"public\n"})), wrapper=copies / WRAPPER.name)
        self.assertEqual((code, report), (4, None))

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
    """connect-candidate.py: the local release-mode step and assemble-cli's scan of its product."""

    def setUp(self):
        self.root = Path(tempfile.mkdtemp(prefix="candidate-scan-test-")).resolve()
        self.addCleanup(shutil.rmtree, self.root, True)

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
                   "--output", self.root / "refused", clean, marked)
        self.assertNotEqual(done.returncode, 0)
        report = json.loads((self.root / "refused/report.json").read_text())
        self.assertEqual((report["status"], report["hits"]["refused"]), ("refused", {"synthetic_marker": 1}))
        self.assertEqual([row["name"] for row in report["payload"]], [clean.name, marked.name])
        done = run(CANDIDATE, "privacy-scan", "--rules", RULES, "--exemptions", EXEMPTIONS,
                   "--output", self.root / "clean-scan", clean)
        self.assertEqual(done.returncode, 0, done.stderr)
        self.assertEqual(json.loads((self.root / "clean-scan/report.json").read_text())["status"], "clean")
        self.assertEqual((self.root / "clean-scan/detail.json").stat().st_mode & 0o777, 0o600)

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
            return {}, None

        patches = (mock.patch.object(p, "cli_producer_inputs", lambda supplied: (value, original.source, resources)),
                   mock.patch.object(p.Recorder, "run", recorded))
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
                           rules=RULES, exemptions=EXEMPTIONS)
        self.assertIn("privacy scan", str(refused.exception))
        self.assertFalse((output / "assembled").exists())
        report = json.loads((output / "privacy-scan/report.json").read_text())
        self.assertEqual((report["status"], report["hits"]["refused"]), ("refused", {"synthetic_marker": 1}))

    def test_assemble_cli_keeps_a_clean_product_with_its_report(self):
        p, original, work, output = self.producer(library_bytes(
            {"selected/provenance/demo/source.tar.gz": source_archive(b"public notes\n")}))
        p.assemble_cli(work, original.shutdown, p.digest(original.shutdown), output, rules=RULES, exemptions=EXEMPTIONS)
        final = p.read_json(output / "assembled/assembly.json")
        report = json.loads((output / "privacy-scan/report.json").read_text())
        self.assertEqual(report["status"], "clean")
        self.assertEqual([row["name"] for row in report["payload"]], [final["archive"], final["archive"] + ".sha256"])
        self.assertEqual(report["payload"][0]["sha256"], final["archive_sha256"])


if __name__ == "__main__":
    unittest.main()
