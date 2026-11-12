"""The pinned public inputs of a library build: the fetch step and the committed pin file.

No test here uses the network: the fetch runs against a transport the test supplies. The committed pin
file is checked for its shape, for the rows its readers select, and against the root Cargo.lock.
"""
from __future__ import annotations

import contextlib
import hashlib
import importlib.util
import io
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest import mock

ROOT = Path(__file__).resolve().parents[1]
REPOSITORY = ROOT.parent
SCRIPT = ROOT / "scripts/fetch-pinned-inputs.py"
PINS = ROOT / "release/library-pins-macos-arm64.json"


def load(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


FETCH = load("connect_fetch_pinned_inputs", SCRIPT)

WHEEL = b"wheel bytes"
TARBALL = b"npm tarball bytes"
CRATE = b"crate bytes"
ASSET = b"release asset bytes" * 40


def pin(kind, name, version, file, url, data, use):
    sha512 = kind == "npm-tarball"
    return {
        "kind": kind, "name": name, "version": version, "file": file, "url": url,
        "sha256": None if sha512 else hashlib.sha256(data).hexdigest(),
        "sha512": hashlib.sha512(data).hexdigest() if sha512 else None,
        "digest_source": "test", "use": use,
    }


def fixture_rows():
    return [
        pin("crate", "itoa", "1.0.18", "itoa-1.0.18.crate",
            "https://static.crates.io/crates/itoa/itoa-1.0.18.crate", CRATE, ["tool build provenance"]),
        pin("npm-tarball", "@scope/core", "2.3.0", "core-2.3.0.tgz",
            "https://registry.npmjs.org/@scope/core/-/core-2.3.0.tgz", TARBALL, ["activepieces closure"]),
        pin("pypi-wheel", "agno", "3.0.11", "agno-3.0.11-py3-none-any.whl",
            "https://files.pythonhosted.org/packages/85/dc/agno-3.0.11-py3-none-any.whl", WHEEL,
            ["agno closure"]),
        pin("release-download", "cpython", "3.11.16+20260901", "cpython-3.11.16+20260901-full.tar.zst",
            "https://github.com/example/standalone/releases/download/20260901/cpython-3.11.16%2B20260901-full.tar.zst",
            ASSET, ["runtime python"]),
    ]


BODIES = {
    "https://static.crates.io/crates/itoa/itoa-1.0.18.crate": CRATE,
    "https://registry.npmjs.org/@scope/core/-/core-2.3.0.tgz": TARBALL,
    "https://files.pythonhosted.org/packages/85/dc/agno-3.0.11-py3-none-any.whl": WHEEL,
}
ASSET_URL = ("https://github.com/example/standalone/releases/download/20260901/"
             "cpython-3.11.16%2B20260901-full.tar.zst")
ASSET_STORE = "https://release-assets.githubusercontent.com/github-production-release-asset/1/2?sig=3"


class Response:
    def __init__(self, status, body=b"", headers=None, declared=None):
        self.status = status
        self.headers = dict(headers or {})
        if status == 200 and "Content-Length" not in self.headers and declared is not False:
            self.headers["Content-Length"] = str(len(body) if declared is None else declared)
        self._body = io.BytesIO(body)
        self.closed = False

    def read(self, size):
        return self._body.read(size)

    def close(self):
        self.closed = True


def redirect(location, status=302):
    return Response(status, headers={"Location": location})


class Transport:
    """Serves each URL from a list of answers (the last one stays); records every request."""

    def __init__(self, answers=None):
        self.answers = {url: [lambda body=body: Response(200, body)] for url, body in BODIES.items()}
        self.answers[ASSET_URL] = [lambda: redirect(ASSET_STORE)]
        self.answers[ASSET_STORE] = [lambda: Response(200, ASSET)]
        for url, sequence in (answers or {}).items():
            self.answers[url] = list(sequence)
        self.requests = []

    def __call__(self, url):
        self.requests.append(url)
        sequence = self.answers[url]
        answer = sequence.pop(0) if len(sequence) > 1 else sequence[0]
        result = answer()
        if isinstance(result, BaseException):
            raise result
        return result


class Workspace:
    def __init__(self, rows=None, document=None, text=None):
        self._tmp = tempfile.TemporaryDirectory(prefix="connect-pinned-inputs-")
        self.root = Path(self._tmp.name)
        self.pins = self.root / "pins.json"
        if text is None:
            if document is None:
                document = {"schema": "connect-library-pins/1", "target": "macos-arm64",
                            "pins": fixture_rows() if rows is None else rows}
            text = json.dumps(document)
        self.pins.write_text(text, encoding="utf-8")
        self.destination = self.root / "out"
        self.sleeps = []

    def __enter__(self):
        return self

    def __exit__(self, *exc):
        self._tmp.cleanup()

    def fetch(self, transport=None, destination=None):
        transport = Transport() if transport is None else transport
        return FETCH.fetch(self.pins, destination or self.destination, transport=transport,
                           sleep=self.sleeps.append), transport

    def refused(self, transport=None, destination=None):
        with unittest.TestCase().assertRaises(FETCH.Refusal) as caught:
            self.fetch(transport, destination)
        return str(caught.exception)


class FetchTests(unittest.TestCase):
    def test_every_pin_is_fetched_checked_and_recorded(self):
        with Workspace() as work:
            record, transport = work.fetch()
            self.assertEqual(sorted(str(path.relative_to(work.destination))
                                    for path in work.destination.rglob("*") if path.is_file()),
                             ["FETCHED.json", "crate/itoa-1.0.18.crate", "npm-tarball/core-2.3.0.tgz",
                              "pypi-wheel/agno-3.0.11-py3-none-any.whl",
                              "release-download/cpython-3.11.16+20260901-full.tar.zst"])
            self.assertEqual((work.destination / "release-download/cpython-3.11.16+20260901-full.tar.zst")
                             .read_bytes(), ASSET)
            fetched = json.loads((work.destination / "FETCHED.json").read_text())
            self.assertEqual(fetched, record)
            self.assertEqual(fetched["schema"], "connect-pinned-inputs/1")
            self.assertEqual(fetched["pins_sha256"], hashlib.sha256(work.pins.read_bytes()).hexdigest())
            self.assertEqual(fetched["files"], [
                {"kind": "crate", "file": "itoa-1.0.18.crate", "bytes": len(CRATE),
                 "sha256": hashlib.sha256(CRATE).hexdigest()},
                {"kind": "npm-tarball", "file": "core-2.3.0.tgz", "bytes": len(TARBALL),
                 "sha256": hashlib.sha256(TARBALL).hexdigest()},
                {"kind": "pypi-wheel", "file": "agno-3.0.11-py3-none-any.whl", "bytes": len(WHEEL),
                 "sha256": hashlib.sha256(WHEEL).hexdigest()},
                {"kind": "release-download", "file": "cpython-3.11.16+20260901-full.tar.zst",
                 "bytes": len(ASSET), "sha256": hashlib.sha256(ASSET).hexdigest()},
            ])
            self.assertEqual(transport.requests[-2:], [ASSET_URL, ASSET_STORE])
            self.assertEqual(work.sleeps, [])

    def test_other_bytes_are_refused_and_never_moved_into_place(self):
        for kind, url, file in (("pypi-wheel", "https://files.pythonhosted.org/packages/85/dc/"
                                 "agno-3.0.11-py3-none-any.whl", "agno-3.0.11-py3-none-any.whl"),
                                ("npm-tarball", "https://registry.npmjs.org/@scope/core/-/core-2.3.0.tgz",
                                 "core-2.3.0.tgz")):
            with self.subTest(kind=kind), Workspace() as work:
                transport = Transport({url: [lambda: Response(200, b"other bytes")]})
                reason = work.refused(transport)
                self.assertIn(file, reason)
                self.assertIn("digest", reason)
                self.assertFalse((work.destination / kind / file).exists())
                self.assertFalse((work.destination / "FETCHED.json").exists())
                # A digest difference is a refusal, not a transient failure.
                self.assertEqual(transport.requests.count(url), 1)

    def test_a_redirect_stays_in_the_host_family_of_its_pin(self):
        cases = {
            "another host": [lambda: redirect("https://downloads.example.invalid/asset")],
            "plain http": [lambda: redirect("http://release-assets.githubusercontent.com/asset")],
            "no location": [lambda: Response(302)],
        }
        for name, answers in cases.items():
            with self.subTest(name=name), Workspace() as work:
                transport = Transport({ASSET_URL: answers})
                self.assertIn("redirect", work.refused(transport))
                self.assertEqual([url for url in transport.requests if "example.invalid" in url], [])
        with Workspace() as work:
            wheel = "https://files.pythonhosted.org/packages/85/dc/agno-3.0.11-py3-none-any.whl"
            transport = Transport({wheel: [lambda: redirect(ASSET_STORE)]})
            self.assertIn("redirect", work.refused(transport))
            self.assertNotIn(ASSET_STORE, transport.requests)
        with Workspace() as work:
            transport = Transport({ASSET_URL: [lambda: redirect(ASSET_URL)]})
            self.assertIn("redirect", work.refused(transport))
            self.assertLessEqual(transport.requests.count(ASSET_URL), 6)

    def test_a_redirect_inside_the_same_host_and_the_other_asset_store_are_followed(self):
        other_store = "https://objects.githubusercontent.com/github-production-release-asset/9"
        moved = "https://github.com/example/standalone/releases/download/20260901/moved"
        with Workspace() as work:
            transport = Transport({ASSET_URL: [lambda: redirect(moved, 301)],
                                   moved: [lambda: redirect(other_store, 307)],
                                   other_store: [lambda: Response(200, ASSET)]})
            work.fetch(transport)
            self.assertEqual(transport.requests[-3:], [ASSET_URL, moved, other_store])

    def test_a_file_over_the_size_limit_is_refused(self):
        with Workspace() as work:
            transport = Transport({ASSET_STORE: [lambda: Response(200, b"", declared=FETCH.MAX_BYTES + 1)]})
            self.assertIn("2 GiB", work.refused(transport))
        with Workspace() as work, mock.patch.object(FETCH, "MAX_BYTES", len(ASSET) - 1):
            transport = Transport({ASSET_STORE: [lambda: Response(200, ASSET, declared=False)]})
            self.assertIn("2 GiB", work.refused(transport))
            self.assertFalse((work.destination / "release-download").exists()
                             and any((work.destination / "release-download").iterdir()))

    def test_a_transient_failure_is_retried_a_bounded_number_of_times(self):
        wheel = "https://files.pythonhosted.org/packages/85/dc/agno-3.0.11-py3-none-any.whl"
        for name, failure in (("server error", lambda: Response(503)),
                              ("rate limit", lambda: Response(429)),
                              ("connection error", lambda: OSError("connection reset")),
                              ("short body", lambda: Response(200, WHEEL[:4], declared=len(WHEEL)))):
            with self.subTest(name=name), Workspace() as work:
                transport = Transport({wheel: [failure, failure, lambda: Response(200, WHEEL)]})
                work.fetch(transport)
                self.assertEqual(transport.requests.count(wheel), 3)
                self.assertEqual(len(work.sleeps), 2)
            with self.subTest(name=name, outcome="never recovers"), Workspace() as work:
                transport = Transport({wheel: [failure]})
                self.assertIn("agno-3.0.11-py3-none-any.whl", work.refused(transport))
                self.assertEqual(transport.requests.count(wheel), FETCH.ATTEMPTS)
                self.assertFalse((work.destination / "FETCHED.json").exists())

    def test_a_missing_file_is_refused_at_once(self):
        wheel = "https://files.pythonhosted.org/packages/85/dc/agno-3.0.11-py3-none-any.whl"
        for status in (403, 404, 410):
            with self.subTest(status=status), Workspace() as work:
                transport = Transport({wheel: [lambda status=status: Response(status)]})
                self.assertIn(str(status), work.refused(transport))
                self.assertEqual(transport.requests.count(wheel), 1)

    def test_the_destination_must_be_a_new_absolute_directory_with_a_parent(self):
        with Workspace() as work:
            work.destination.mkdir()
            self.assertIn("exists", work.refused())
        with Workspace() as work:
            self.assertIn("parent", work.refused(destination=work.root / "missing" / "out"))
        with Workspace() as work:
            self.assertIn("absolute", work.refused(destination=Path("relative-out")))
        with Workspace() as work:
            (work.root / "real").mkdir()
            (work.root / "link").symlink_to(work.root / "real")
            self.assertIn("parent", work.refused(destination=work.root / "link" / "out"))


class PinFileShapeTests(unittest.TestCase):
    def refused(self, change):
        rows = fixture_rows()
        document = {"schema": "connect-library-pins/1", "target": "macos-arm64", "pins": rows}
        change(document, rows)
        with Workspace(document=document) as work:
            transport = Transport()
            reason = work.refused(transport)
            self.assertEqual(transport.requests, [], "a refused pin file must not start any download")
            self.assertFalse(work.destination.exists())
            return reason

    def test_the_fixture_is_accepted(self):
        with Workspace() as work:
            pins = FETCH.load_pins(work.pins)
            self.assertEqual(len(pins.rows), 4)
            self.assertEqual(pins.sha256, hashlib.sha256(work.pins.read_bytes()).hexdigest())

    def test_document_shape(self):
        self.assertIn("schema", self.refused(lambda doc, rows: doc.update(schema="connect-library-pins/2")))
        self.assertIn("key", self.refused(lambda doc, rows: doc.update(extra=1)))
        self.assertIn("target", self.refused(lambda doc, rows: doc.update(target="")))
        self.assertIn("pins", self.refused(lambda doc, rows: doc.update(pins={})))
        self.assertIn("pins", self.refused(lambda doc, rows: doc.update(pins=[])))
        with Workspace(text='{"schema": "connect-library-pins/1", "schema": "connect-library-pins/1", '
                            '"target": "macos-arm64", "pins": []}') as work:
            self.assertIn("duplicate", work.refused())
        with Workspace(text="not json") as work:
            self.assertIn("JSON", work.refused())

    def test_row_keys_and_kinds(self):
        self.assertIn("key", self.refused(lambda doc, rows: rows[0].update(bytes=3)))
        self.assertIn("key", self.refused(lambda doc, rows: rows[0].pop("digest_source")))
        self.assertIn("kind", self.refused(lambda doc, rows: rows[0].update(kind="git")))
        self.assertIn("name", self.refused(lambda doc, rows: rows[0].update(name="")))
        self.assertIn("version", self.refused(lambda doc, rows: rows[0].update(version=7)))

    def test_exactly_the_digest_the_index_publishes(self):
        self.assertIn("sha256", self.refused(lambda doc, rows: rows[0].update(sha256=None)))
        self.assertIn("sha256", self.refused(lambda doc, rows: rows[0].update(sha256=rows[0]["sha256"].upper())))
        self.assertIn("sha256", self.refused(lambda doc, rows: rows[0].update(sha256=rows[0]["sha256"][:-1])))
        self.assertIn("sha512", self.refused(lambda doc, rows: rows[0].update(sha512="0" * 128)))
        self.assertIn("sha512", self.refused(lambda doc, rows: rows[1].update(sha512=None)))
        self.assertIn("sha256", self.refused(lambda doc, rows: rows[1].update(sha256="0" * 64)))

    def test_labels_are_a_sorted_list_from_the_known_set(self):
        self.assertIn("use", self.refused(lambda doc, rows: rows[0].update(use="tool build provenance")))
        self.assertIn("use", self.refused(lambda doc, rows: rows[0].update(use=[])))
        self.assertIn("use", self.refused(lambda doc, rows: rows[0].update(use=["tool history"])))
        self.assertIn("use", self.refused(lambda doc, rows: rows[3].update(use=["runtime node", "acquisition node"])))
        self.assertIn("use", self.refused(lambda doc, rows: rows[3].update(use=["runtime node", "runtime node"])))
        self.assertEqual(FETCH.LABELS, frozenset({
            "agno closure", "services closure", "activepieces closure", "component build tool",
            "component build backend", "tool build provenance", "runtime python", "runtime node",
            "acquisition node", "acquisition npm", "runtime pip", "official github", "official playwright"}))

    def test_rows_are_sorted_and_unique_by_kind_and_file(self):
        self.assertIn("sorted", self.refused(lambda doc, rows: rows.reverse()))

        def duplicate(doc, rows):
            copy = dict(rows[2], name="agno2")
            rows.insert(3, copy)
        self.assertIn("twice", self.refused(duplicate))

    def test_file_names_cannot_leave_their_directory(self):
        for file in ("../itoa-1.0.18.crate", "sub/itoa-1.0.18.crate", ".hidden", "", "FETCHED.json", "a\\b",
                     "a b.crate"):
            with self.subTest(file=file):
                self.assertIn("file", self.refused(lambda doc, rows, file=file: rows[0].update(file=file)))

    def test_urls_are_https_on_the_hosts_of_their_kind(self):
        def url(index, value):
            return lambda doc, rows: rows[index].update(url=value)
        cases = [
            url(0, "http://static.crates.io/crates/itoa/itoa-1.0.18.crate"),
            url(0, "https://github.com/example/x/releases/download/1/itoa-1.0.18.crate"),
            url(0, "https://static.crates.io.example.invalid/crates/itoa/itoa-1.0.18.crate"),
            url(0, "https://user@static.crates.io/crates/itoa/itoa-1.0.18.crate"),
            url(0, "https://static.crates.io:8443/crates/itoa/itoa-1.0.18.crate"),
            url(0, "https://static.crates.io/crates/itoa/itoa-1.0.18.crate?x=1"),
            url(0, "https://static.crates.io/crates/itoa/itoa-1.0.18.crate#x"),
            url(0, "https://static.crates.io/crates/itoa/other-1.0.18.crate"),
            url(1, "https://files.pythonhosted.org/packages/core-2.3.0.tgz"),
            url(2, "https://registry.npmjs.org/agno/-/agno-3.0.11-py3-none-any.whl"),
            # github.com serves many things; only a release asset path is a pin.
            url(3, "https://github.com/example/standalone/raw/main/cpython-3.11.16%2B20260901-full.tar.zst"),
            url(3, "https://raw.githubusercontent.com/example/standalone/cpython-3.11.16%2B20260901-full.tar.zst"),
            url(3, "https://release-assets.githubusercontent.com/cpython-3.11.16%2B20260901-full.tar.zst"),
        ]
        for case in cases:
            with self.subTest(case=cases.index(case)):
                self.assertIn("url", self.refused(case))

    def test_the_publisher_hosts_of_release_downloads(self):
        for url_value, file in (("https://nodejs.org/dist/v24.19.0/node-v24.19.0-darwin-arm64.tar.gz",
                                 "node-v24.19.0-darwin-arm64.tar.gz"),
                                ("https://bootstrap.pypa.io/pip/zipapp/pip-26.2.1.pyz", "pip-26.2.1.pyz")):
            with self.subTest(file=file):
                rows = fixture_rows()
                rows[3] = pin("release-download", "tool", "1", file, url_value, ASSET, ["runtime node"])
                with Workspace(rows=rows) as work:
                    transport = Transport({url_value: [lambda: Response(200, ASSET)]})
                    work.fetch(transport)
                    self.assertEqual((work.destination / "release-download" / file).read_bytes(), ASSET)


class CommandLineTests(unittest.TestCase):
    def run_main(self, argv, transport):
        out, err = io.StringIO(), io.StringIO()
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            code = FETCH.main(argv, transport=transport, sleep=lambda seconds: None)
        return code, out.getvalue(), err.getvalue()

    def test_success_prints_one_json_line(self):
        with Workspace() as work:
            code, out, _ = self.run_main(["--pins", str(work.pins), "--destination", str(work.destination)],
                                         Transport())
            self.assertEqual(code, 0)
            self.assertEqual(out.count("\n"), 1)
            summary = json.loads(out)
            self.assertEqual((summary["files"], summary["destination"]), (4, str(work.destination)))
            self.assertEqual(summary["pins_sha256"], hashlib.sha256(work.pins.read_bytes()).hexdigest())
            self.assertEqual(summary["bytes"], len(CRATE) + len(TARBALL) + len(WHEEL) + len(ASSET))

    def test_a_refusal_is_one_line_on_stderr_and_exit_status_2(self):
        with Workspace() as work:
            work.destination.mkdir()
            code, out, err = self.run_main(["--pins", str(work.pins), "--destination", str(work.destination)],
                                           Transport())
            self.assertEqual((code, out), (2, ""))
            self.assertEqual(err.count("\n"), 1)
            self.assertTrue(err.startswith("pinned input fetch refused: "), err)


LOCK = """\
version = 4

[[package]]
name = "itoa"
version = "1.0.18"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "{itoa}"

[[package]]
name = "io-lifetimes"
version = "2.0.4"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "{old}"

[[package]]
name = "io-lifetimes"
version = "3.0.1"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "{new}"

[[package]]
name = "serde_derive"
version = "1.0.229"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "{derive}"

[[package]]
name = "toolkit-package"
version = "0.9.15"
dependencies = [
 "itoa",
]

[[package]]
name = "forked"
version = "0.1.0"
source = "git+https://example.invalid/forked#0123"
""".format(itoa="1" * 64, old="2" * 64, new="3" * 64, derive="4" * 64)

CLOSURE = """\
toolkit-package v0.9.15 (/checkout/connect/crates/toolkit-package)
itoa v1.0.18
io-lifetimes v3.0.1
io-lifetimes v2.0.4
serde_derive v1.0.229 (proc-macro)
itoa v1.0.18 (*)
"""


class CrateRowTests(unittest.TestCase):
    """The crate rows are generated: the tool closure Cargo prints, with the lock's checksums."""

    def test_rows_come_from_the_closure_listing_and_the_lock(self):
        rows = FETCH.crate_rows(CLOSURE, LOCK)
        self.assertEqual([(row["name"], row["version"], row["sha256"]) for row in rows],
                         [("io-lifetimes", "2.0.4", "2" * 64), ("io-lifetimes", "3.0.1", "3" * 64),
                          ("itoa", "1.0.18", "1" * 64), ("serde_derive", "1.0.229", "4" * 64)])
        self.assertEqual(rows[2], {
            "kind": "crate", "name": "itoa", "version": "1.0.18", "file": "itoa-1.0.18.crate",
            "url": "https://static.crates.io/crates/itoa/itoa-1.0.18.crate", "sha256": "1" * 64,
            "sha512": None, "digest_source": "root Cargo.lock checksum (the crates.io index cksum)",
            "use": ["tool build provenance"]})

    def test_a_listing_the_lock_does_not_explain_is_refused(self):
        for name, closure in (("unknown shape", CLOSURE + "itoa 1.0.18\n"),
                              ("not in the lock", CLOSURE + "ryu v1.0.0\n"),
                              ("another version", CLOSURE + "itoa v1.0.19\n"),
                              ("a git source", CLOSURE + "forked v0.1.0\n"),
                              ("a workspace crate with a registry entry", CLOSURE + "itoa v1.0.18 (/checkout/itoa)\n"),
                              ("a registry line for a workspace crate", CLOSURE + "toolkit-package v0.9.15\n"),
                              ("empty", "")):
            with self.subTest(name=name), self.assertRaises(FETCH.Refusal):
                FETCH.crate_rows(closure, LOCK)

    def synced(self, rows):
        document = {"schema": "connect-library-pins/1", "target": "macos-arm64", "pins": rows}
        work = Workspace(text=FETCH.render(document))
        (work.root / "Cargo.lock").write_text(LOCK, encoding="utf-8")
        (work.root / "closure.txt").write_text(CLOSURE, encoding="utf-8")
        return work

    def crate_mode(self, work, mode):
        out, err = io.StringIO(), io.StringIO()
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            code = FETCH.main(["--pins", str(work.pins), mode, "--closure", str(work.root / "closure.txt"),
                               "--lock", str(work.root / "Cargo.lock")], transport=Transport())
        return code, out.getvalue(), err.getvalue()

    def test_check_reports_stale_crate_rows_and_write_replaces_exactly_them(self):
        stale = fixture_rows()
        with self.synced(stale) as work:
            before = work.pins.read_bytes()
            code, _, err = self.crate_mode(work, "--check-crate-rows")
            self.assertEqual(code, 1)
            self.assertIn("io-lifetimes-2.0.4.crate", err)
            self.assertIn("--write-crate-rows", err)
            self.assertEqual(work.pins.read_bytes(), before)
            code, _, _ = self.crate_mode(work, "--write-crate-rows")
            self.assertEqual(code, 0)
            written = FETCH.load_pins(work.pins)
            self.assertEqual([row for row in written.rows if row["kind"] == "crate"],
                             FETCH.crate_rows(CLOSURE, LOCK))
            self.assertEqual([row for row in written.rows if row["kind"] != "crate"], stale[1:])
            self.assertEqual(work.pins.read_text(), FETCH.render(written.document))
            self.assertEqual(self.crate_mode(work, "--check-crate-rows")[0], 0)
            self.assertFalse(work.destination.exists())

    def test_the_crate_modes_need_both_inputs_and_no_destination(self):
        with self.synced(fixture_rows()) as work:
            for argv in (["--pins", str(work.pins), "--check-crate-rows"],
                         ["--pins", str(work.pins), "--check-crate-rows", "--closure", str(work.root / "closure.txt")],
                         ["--pins", str(work.pins), "--write-crate-rows", "--closure", str(work.root / "closure.txt"),
                          "--lock", str(work.root / "Cargo.lock"), "--destination", str(work.destination)],
                         ["--pins", str(work.pins)]):
                with self.subTest(argv=argv[2:]):
                    out, err = io.StringIO(), io.StringIO()
                    with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
                        code = FETCH.main(argv, transport=Transport())
                    self.assertEqual(code, 2)
                    self.assertTrue(err.getvalue().startswith("pinned input fetch refused: "))


def rows_with(rows, label):
    return [row for row in rows if label in row["use"]]


class CommittedPinFileTests(unittest.TestCase):
    """connect/release/library-pins-macos-arm64.json as its readers select from it."""

    @classmethod
    def setUpClass(cls):
        cls.pins = FETCH.load_pins(PINS)
        cls.rows = cls.pins.rows
        cls.lock = (REPOSITORY / "Cargo.lock").read_text(encoding="utf-8")

    def test_the_file_is_the_canonical_rendering_for_macos_arm64(self):
        self.assertEqual(self.pins.document["target"], "macos-arm64")
        self.assertEqual(PINS.read_text(encoding="utf-8"), FETCH.render(self.pins.document))

    def test_the_resource_rows_are_selected_by_kind_and_file(self):
        by_key = {(row["kind"], row["file"]): row for row in self.rows}
        for key, labels in (
                (("release-download", "cpython-3.11.16+20260901-aarch64-apple-darwin-pgo+lto-full.tar.zst"),
                 ["runtime python"]),
                (("release-download", "node-v24.19.0-darwin-arm64.tar.gz"), ["acquisition node", "runtime node"]),
                (("release-download", "pip-26.2.1.pyz"), ["runtime pip"]),
                (("npm-tarball", "npm-12.2.0.tgz"), ["acquisition npm"])):
            with self.subTest(file=key[1]):
                self.assertEqual(by_key[key]["use"], labels)
        self.assertIn("our own", by_key[("release-download", "pip-26.2.1.pyz")]["digest_source"])

    def test_each_producer_label_selects_what_the_producer_expects(self):
        for label in ("runtime python", "runtime node", "runtime pip", "acquisition node", "acquisition npm"):
            with self.subTest(label=label):
                self.assertEqual(len(rows_with(self.rows, label)), 1)
        tools = rows_with(self.rows, "component build tool")
        self.assertEqual(sorted(row["name"] for row in tools),
                         ["build", "packaging", "pyproject_hooks", "setuptools", "wheel"])
        self.assertEqual({row["kind"] for row in tools}, {"pypi-wheel"})

    def test_the_component_backends_are_the_two_exact_setuptools_versions(self):
        backends = rows_with(self.rows, "component build backend")
        self.assertEqual([(row["name"], row["version"], row["file"]) for row in backends],
                         [("setuptools", "82.0.0", "setuptools-82.0.0-py3-none-any.whl"),
                          ("setuptools", "84.0.0", "setuptools-84.0.0-py3-none-any.whl")])

    def test_the_official_component_rows(self):
        github = rows_with(self.rows, "official github")
        self.assertEqual([(row["kind"], row["file"], row["version"]) for row in github],
                         [("release-download", "github-mcp-server_Darwin_arm64.tar.gz", "2.0.3")])
        playwright = rows_with(self.rows, "official playwright")
        self.assertEqual([(row["name"], row["version"]) for row in playwright],
                         [("@playwright/mcp", "0.0.83"), ("playwright", "1.64.0-alpha-1790635538000"),
                          ("playwright-core", "1.64.0-alpha-1790635538000")])
        self.assertEqual({row["kind"] for row in playwright}, {"npm-tarball"})

    def test_every_consumer_closure_row_is_a_wheel_or_an_npm_archive(self):
        for label, kind in (("agno closure", "pypi-wheel"), ("services closure", "pypi-wheel"),
                            ("activepieces closure", "npm-tarball")):
            with self.subTest(label=label):
                selected = rows_with(self.rows, label)
                self.assertTrue(selected)
                self.assertEqual({row["kind"] for row in selected}, {kind})

    def test_crate_rows_and_only_they_are_tool_build_provenance(self):
        crates = [row for row in self.rows if row["kind"] == "crate"]
        self.assertTrue(crates)
        self.assertEqual(crates, rows_with(self.rows, "tool build provenance"))
        self.assertEqual({tuple(row["use"]) for row in crates}, {("tool build provenance",)})

    def test_every_crate_row_is_a_package_of_the_root_lock_with_its_checksum(self):
        # Runs everywhere (no Cargo): a lock change that moves a tool dependency to another version
        # leaves a row the lock no longer has, and this fails until the rows are written again with
        # `fetch-pinned-inputs.py --write-crate-rows` (the message of the check names the command).
        import tomllib
        packages = {}
        for package in tomllib.loads(self.lock)["package"]:
            packages.setdefault((package["name"], package["version"]), []).append(package)
        for row in (row for row in self.rows if row["kind"] == "crate"):
            with self.subTest(crate=row["file"]):
                entries = packages.get((row["name"], row["version"]), [])
                self.assertEqual(len(entries), 1, "not a package of the root Cargo.lock: stale crate rows")
                self.assertEqual(entries[0].get("source"), "registry+https://github.com/rust-lang/crates.io-index")
                self.assertEqual(entries[0].get("checksum"), row["sha256"])

    @unittest.skipUnless(os.environ.get("CONNECT_PINS_TOOL_CLOSURE") == "1",
                         "needs Cargo and the registry: set CONNECT_PINS_TOOL_CLOSURE=1 (the macOS stage run does)")
    def test_the_crate_rows_equal_the_tool_closure_of_the_current_lock(self):
        result = subprocess.run(
            [str(ROOT / "scripts/repo-cargo"), "tree", "--locked", "--manifest-path",
             str(REPOSITORY / "Cargo.toml"), *FETCH.TOOL_CLOSURE_ARGUMENTS],
            cwd=REPOSITORY, capture_output=True, text=True, stdin=subprocess.DEVNULL)
        self.assertEqual(result.returncode, 0, result.stderr[-2000:])
        expected = FETCH.crate_rows(result.stdout, self.lock)
        committed = [row for row in self.rows if row["kind"] == "crate"]
        self.assertEqual([row["file"] for row in committed], [row["file"] for row in expected],
                         "stale crate rows: run fetch-pinned-inputs.py --write-crate-rows")
        self.assertEqual(committed, expected)


if __name__ == "__main__":
    unittest.main()
