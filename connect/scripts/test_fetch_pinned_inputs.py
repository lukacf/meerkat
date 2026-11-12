"""The pinned public inputs of a library build: the fetch step and the committed pin file.

No test here uses the network or Cargo, except the one named for it: the fetch runs against a transport
the test supplies, and the crate rows are derived from a fixture lock and a fixture closure listing. The
committed pin file is checked for its shape and for the rows its readers select.
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
RESOLVED_NAME = "library-pins-macos-arm64.resolved.json"


def load(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


FETCH = load("connect_fetch_pinned_inputs", SCRIPT)

WHEEL = b"wheel bytes"
TARBALL = b"npm tarball bytes"
ASSET = b"release asset bytes" * 40
CRATES = {
    ("io-lifetimes", "2.0.4"): b"io-lifetimes two",
    ("io-lifetimes", "3.0.1"): b"io-lifetimes three",
    ("itoa", "1.0.18"): b"itoa",
    ("serde_derive", "1.0.229"): b"serde derive",
}
CRATES_IO = "registry+https://github.com/rust-lang/crates.io-index"


def crate_url(name, version):
    return f"https://static.crates.io/crates/{name}/{name}-{version}.crate"


def lock_text(extra=""):
    packages = ["version = 4\n"]
    for (name, version), data in CRATES.items():
        packages.append(f'[[package]]\nname = "{name}"\nversion = "{version}"\nsource = "{CRATES_IO}"\n'
                        f'checksum = "{hashlib.sha256(data).hexdigest()}"\n')
    packages.append('[[package]]\nname = "toolkit-package"\nversion = "0.9.15"\ndependencies = [\n "itoa",\n]\n')
    packages.append('[[package]]\nname = "forked"\nversion = "0.1.0"\n'
                    'source = "git+https://example.invalid/forked#0123"\n')
    packages.append(f'[[package]]\nname = "unsummed"\nversion = "0.2.0"\nsource = "{CRATES_IO}"\n')
    packages.append('[[package]]\nname = "elsewhere"\nversion = "0.3.0"\n'
                    f'source = "registry+https://registry.example.invalid/index"\nchecksum = "{"5" * 64}"\n')
    return "\n".join(packages) + extra


LOCK = lock_text()
CLOSURE = """\
toolkit-package v0.9.15 (/checkout/connect/crates/toolkit-package)
itoa v1.0.18
io-lifetimes v3.0.1
io-lifetimes v2.0.4
serde_derive v1.0.229 (proc-macro)
itoa v1.0.18 (*)
"""


def pin(kind, name, version, file, url, data, use):
    sha512 = kind == "npm-tarball"
    return {
        "kind": kind, "name": name, "version": version, "file": file, "url": url,
        "sha256": None if sha512 else hashlib.sha256(data).hexdigest(),
        "sha512": hashlib.sha512(data).hexdigest() if sha512 else None,
        "digest_source": "test", "use": use,
    }


def crate_pin(name, version):
    return {
        "kind": "crate", "name": name, "version": version, "file": f"{name}-{version}.crate",
        "url": crate_url(name, version), "sha256": hashlib.sha256(CRATES[(name, version)]).hexdigest(),
        "sha512": None, "digest_source": "root Cargo.lock checksum (the crates.io index cksum)",
        "use": ["tool build provenance"],
    }


NPM, WHEEL_ROW, RELEASE = 0, 1, 2
WHEEL_URL = "https://files.pythonhosted.org/packages/85/dc/agno-3.0.11-py3-none-any.whl"
NPM_URL = "https://registry.npmjs.org/@scope/core/-/core-2.3.0.tgz"
ASSET_URL = ("https://github.com/example/standalone/releases/download/20260901/"
             "cpython-3.11.16%2B20260901-full.tar.zst")
ASSET_STORE = "https://release-assets.githubusercontent.com/github-production-release-asset/1/2?sig=3"


def fixture_rows():
    """The committed rows: every kind but crate."""
    return [
        pin("npm-tarball", "@scope/core", "2.3.0", "core-2.3.0.tgz", NPM_URL, TARBALL, ["activepieces closure"]),
        pin("pypi-wheel", "agno", "3.0.11", "agno-3.0.11-py3-none-any.whl", WHEEL_URL, WHEEL, ["agno closure"]),
        pin("release-download", "cpython", "3.11.16+20260901", "cpython-3.11.16+20260901-full.tar.zst",
            ASSET_URL, ASSET, ["runtime python"]),
    ]


def resolved_rows():
    return [crate_pin(name, version) for name, version in sorted(CRATES)] + fixture_rows()


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
        bodies = {crate_url(name, version): data for (name, version), data in CRATES.items()}
        bodies.update({NPM_URL: TARBALL, WHEEL_URL: WHEEL, ASSET_STORE: ASSET})
        self.answers = {url: [lambda body=body: Response(200, body)] for url, body in bodies.items()}
        self.answers[ASSET_URL] = [lambda: redirect(ASSET_STORE)]
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
    def __init__(self, rows=None, document=None, text=None, closure=CLOSURE, lock=LOCK):
        self._tmp = tempfile.TemporaryDirectory(prefix="connect-pinned-inputs-")
        self.root = Path(self._tmp.name)
        self.pins = self.root / "pins.json"
        if text is None:
            if document is None:
                document = {"schema": "connect-library-pins/1", "target": "macos-arm64",
                            "pins": fixture_rows() if rows is None else rows}
            text = json.dumps(document)
        self.pins.write_text(text, encoding="utf-8")
        self.closure = self.root / "closure.txt"
        self.closure.write_text(closure, encoding="utf-8")
        self.lock = self.root / "Cargo.lock"
        self.lock.write_text(lock, encoding="utf-8")
        self.destination = self.root / "out"
        self.sleeps = []

    def __enter__(self):
        return self

    def __exit__(self, *exc):
        self._tmp.cleanup()

    def arguments(self, *more):
        return ["--pins", str(self.pins), "--closure", str(self.closure), "--lock", str(self.lock), *more]

    def fetch(self, transport=None, destination=None):
        transport = Transport() if transport is None else transport
        return FETCH.fetch(self.pins, self.closure, self.lock, destination or self.destination,
                           transport=transport, sleep=self.sleeps.append), transport

    def refused(self, transport=None, destination=None):
        with unittest.TestCase().assertRaises(FETCH.Refusal) as caught:
            self.fetch(transport, destination)
        return str(caught.exception)


def main(argv, transport=None):
    out, err = io.StringIO(), io.StringIO()
    with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
        code = FETCH.main(argv, transport=Transport() if transport is None else transport,
                          sleep=lambda seconds: None)
    return code, out.getvalue(), err.getvalue()


class FetchTests(unittest.TestCase):
    def test_every_committed_and_derived_pin_is_fetched_checked_and_recorded(self):
        with Workspace() as work:
            record, transport = work.fetch()
            self.assertEqual(sorted(str(path.relative_to(work.destination))
                                    for path in work.destination.rglob("*") if path.is_file()),
                             ["FETCHED.json", "crate/io-lifetimes-2.0.4.crate", "crate/io-lifetimes-3.0.1.crate",
                              "crate/itoa-1.0.18.crate", "crate/serde_derive-1.0.229.crate", RESOLVED_NAME,
                              "npm-tarball/core-2.3.0.tgz", "pypi-wheel/agno-3.0.11-py3-none-any.whl",
                              "release-download/cpython-3.11.16+20260901-full.tar.zst"])
            self.assertEqual((work.destination / "release-download/cpython-3.11.16+20260901-full.tar.zst")
                             .read_bytes(), ASSET)
            self.assertEqual((work.destination / "crate/itoa-1.0.18.crate").read_bytes(),
                             CRATES[("itoa", "1.0.18")])
            # The resolved pin file is what every reader is given as --pins: the committed rows plus
            # the derived crate rows, in the committed file's own schema and rendering.
            resolved_path = work.destination / RESOLVED_NAME
            resolved = FETCH.load_pins(resolved_path, resolved=True)
            self.assertEqual(resolved.rows, resolved_rows())
            self.assertEqual(set(resolved.document), {"schema", "target", "pins"})
            self.assertEqual(resolved_path.read_text(encoding="utf-8"), FETCH.render(resolved.document))
            fetched = json.loads((work.destination / "FETCHED.json").read_text())
            self.assertEqual(fetched, record)
            self.assertEqual(set(fetched), {"schema", "pins_sha256", "lock_sha256", "resolved_pins", "files"})
            self.assertEqual(fetched["schema"], "connect-pinned-inputs/1")
            self.assertEqual(fetched["pins_sha256"], hashlib.sha256(work.pins.read_bytes()).hexdigest())
            self.assertEqual(fetched["lock_sha256"], hashlib.sha256(work.lock.read_bytes()).hexdigest())
            self.assertEqual(fetched["resolved_pins"], {
                "file": RESOLVED_NAME, "rows": 7, "sha256": hashlib.sha256(resolved_path.read_bytes()).hexdigest()})
            self.assertEqual(fetched["files"][:2] + fetched["files"][-1:], [
                {"kind": "crate", "file": "io-lifetimes-2.0.4.crate", "bytes": len(CRATES[("io-lifetimes", "2.0.4")]),
                 "sha256": hashlib.sha256(CRATES[("io-lifetimes", "2.0.4")]).hexdigest()},
                {"kind": "crate", "file": "io-lifetimes-3.0.1.crate", "bytes": len(CRATES[("io-lifetimes", "3.0.1")]),
                 "sha256": hashlib.sha256(CRATES[("io-lifetimes", "3.0.1")]).hexdigest()},
                {"kind": "release-download", "file": "cpython-3.11.16+20260901-full.tar.zst",
                 "bytes": len(ASSET), "sha256": hashlib.sha256(ASSET).hexdigest()},
            ])
            self.assertEqual([(entry["kind"], entry["file"]) for entry in fetched["files"]],
                             [(row["kind"], row["file"]) for row in resolved_rows()])
            self.assertEqual(transport.requests[-2:], [ASSET_URL, ASSET_STORE])
            self.assertEqual(work.sleeps, [])

    def test_other_bytes_are_refused_and_never_moved_into_place(self):
        for kind, url, file in (("pypi-wheel", WHEEL_URL, "agno-3.0.11-py3-none-any.whl"),
                                ("npm-tarball", NPM_URL, "core-2.3.0.tgz"),
                                ("crate", crate_url("itoa", "1.0.18"), "itoa-1.0.18.crate")):
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
            transport = Transport({WHEEL_URL: [lambda: redirect(ASSET_STORE)]})
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
        for name, failure in (("server error", lambda: Response(503)),
                              ("rate limit", lambda: Response(429)),
                              ("connection error", lambda: OSError("connection reset")),
                              ("short body", lambda: Response(200, WHEEL[:4], declared=len(WHEEL)))):
            with self.subTest(name=name), Workspace() as work:
                transport = Transport({WHEEL_URL: [failure, failure, lambda: Response(200, WHEEL)]})
                work.fetch(transport)
                self.assertEqual(transport.requests.count(WHEEL_URL), 3)
                self.assertEqual(len(work.sleeps), 2)
            with self.subTest(name=name, outcome="never recovers"), Workspace() as work:
                transport = Transport({WHEEL_URL: [failure]})
                self.assertIn("agno-3.0.11-py3-none-any.whl", work.refused(transport))
                self.assertEqual(transport.requests.count(WHEEL_URL), FETCH.ATTEMPTS)
                self.assertFalse((work.destination / "FETCHED.json").exists())

    def test_a_missing_file_is_refused_at_once(self):
        for status in (403, 404, 410):
            with self.subTest(status=status), Workspace() as work:
                transport = Transport({WHEEL_URL: [lambda status=status: Response(status)]})
                self.assertIn(str(status), work.refused(transport))
                self.assertEqual(transport.requests.count(WHEEL_URL), 1)

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

    def test_a_closure_the_lock_does_not_explain_stops_the_fetch_before_any_download(self):
        with Workspace(closure=CLOSURE + "unsummed v0.2.0\n") as work:
            transport = Transport()
            self.assertIn("unsummed", work.refused(transport))
            self.assertEqual(transport.requests, [])
            self.assertFalse(work.destination.exists())


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
            self.assertEqual(len(pins.rows), 3)
            self.assertEqual(pins.sha256, hashlib.sha256(work.pins.read_bytes()).hexdigest())

    def test_document_shape(self):
        self.assertIn("schema", self.refused(lambda doc, rows: doc.update(schema="connect-library-pins/2")))
        self.assertIn("key", self.refused(lambda doc, rows: doc.update(extra=1)))
        self.assertIn("target", self.refused(lambda doc, rows: doc.update(target="")))
        self.assertIn("pins", self.refused(lambda doc, rows: doc.update(pins={})))
        self.assertIn("pins", self.refused(lambda doc, rows: doc.update(pins=[])))
        self.assertIn("note", self.refused(lambda doc, rows: doc.update(note=7)))
        self.assertIn("note", self.refused(lambda doc, rows: doc.update(note="")))
        with Workspace(text='{"schema": "connect-library-pins/1", "schema": "connect-library-pins/1", '
                            '"target": "macos-arm64", "pins": []}') as work:
            self.assertIn("duplicate", work.refused())
        with Workspace(text="not json") as work:
            self.assertIn("JSON", work.refused())

    def test_a_note_for_people_is_allowed_and_rendered(self):
        document = {"schema": "connect-library-pins/1", "target": "macos-arm64",
                    "note": "Crate rows are derived.", "pins": fixture_rows()}
        with Workspace(text=FETCH.render(document)) as work:
            pins = FETCH.load_pins(work.pins)
            self.assertEqual(pins.document["note"], "Crate rows are derived.")
            self.assertEqual(work.pins.read_text(encoding="utf-8").splitlines()[:4],
                             ["{", '  "schema": "connect-library-pins/1",', '  "target": "macos-arm64",',
                              '  "note": "Crate rows are derived.",'])
            work.fetch()
            # The note stays with the committed file; the resolved file has the readers' three keys.
            self.assertNotIn("note", json.loads((work.destination / RESOLVED_NAME).read_text()))

    def test_crate_rows_are_never_committed(self):
        def add_crate(doc, rows):
            rows.insert(0, crate_pin("itoa", "1.0.18"))
        reason = self.refused(add_crate)
        self.assertIn("crate", reason)
        self.assertIn("Cargo.lock", reason)
        # The same row is what a resolved file carries.
        document = {"schema": "connect-library-pins/1", "target": "macos-arm64", "pins": resolved_rows()}
        self.assertEqual(FETCH.validate_document(document, resolved=True), resolved_rows())
        with self.assertRaises(FETCH.Refusal):
            FETCH.validate_document(document)
        with self.assertRaises(FETCH.Refusal):
            FETCH.validate_document({**document, "note": "resolved files carry no note"}, resolved=True)

    def test_row_keys_and_kinds(self):
        self.assertIn("key", self.refused(lambda doc, rows: rows[WHEEL_ROW].update(bytes=3)))
        self.assertIn("key", self.refused(lambda doc, rows: rows[WHEEL_ROW].pop("digest_source")))
        self.assertIn("kind", self.refused(lambda doc, rows: rows[WHEEL_ROW].update(kind="git")))
        self.assertIn("name", self.refused(lambda doc, rows: rows[WHEEL_ROW].update(name="")))
        self.assertIn("version", self.refused(lambda doc, rows: rows[WHEEL_ROW].update(version=7)))

    def test_exactly_the_digest_the_index_publishes(self):
        def wheel(**change):
            return lambda doc, rows: rows[WHEEL_ROW].update(change)
        digest = fixture_rows()[WHEEL_ROW]["sha256"]
        self.assertIn("sha256", self.refused(wheel(sha256=None)))
        self.assertIn("sha256", self.refused(wheel(sha256=digest.upper())))
        self.assertIn("sha256", self.refused(wheel(sha256=digest[:-1])))
        self.assertIn("sha512", self.refused(wheel(sha512="0" * 128)))
        self.assertIn("sha512", self.refused(lambda doc, rows: rows[NPM].update(sha512=None)))
        self.assertIn("sha256", self.refused(lambda doc, rows: rows[NPM].update(sha256="0" * 64)))

    def test_labels_are_a_sorted_list_from_the_known_set(self):
        def use(index, value):
            return lambda doc, rows: rows[index].update(use=value)
        self.assertIn("use", self.refused(use(WHEEL_ROW, "agno closure")))
        self.assertIn("use", self.refused(use(WHEEL_ROW, [])))
        self.assertIn("use", self.refused(use(WHEEL_ROW, ["tool history"])))
        self.assertIn("use", self.refused(use(RELEASE, ["runtime node", "acquisition node"])))
        self.assertIn("use", self.refused(use(RELEASE, ["runtime node", "runtime node"])))
        self.assertEqual(FETCH.LABELS, frozenset({
            "agno closure", "services closure", "activepieces closure", "component build tool",
            "component build backend", "tool build provenance", "runtime python", "runtime node",
            "acquisition node", "acquisition npm", "runtime pip", "official github", "official playwright"}))

    def test_rows_are_sorted_and_unique_by_kind_and_file(self):
        self.assertIn("sorted", self.refused(lambda doc, rows: rows.reverse()))

        def duplicate(doc, rows):
            rows.insert(WHEEL_ROW + 1, dict(rows[WHEEL_ROW], name="agno2"))
        self.assertIn("twice", self.refused(duplicate))

    def test_file_names_cannot_leave_their_directory(self):
        for file in ("../agno.whl", "sub/agno.whl", ".hidden", "", "FETCHED.json", "a\\b", "a b.whl",
                     RESOLVED_NAME):
            with self.subTest(file=file):
                self.assertIn("file", self.refused(lambda doc, rows, file=file: rows[WHEEL_ROW].update(file=file)))

    def test_urls_are_https_on_the_hosts_of_their_kind(self):
        def url(index, value):
            return lambda doc, rows: rows[index].update(url=value)
        wheel = "agno-3.0.11-py3-none-any.whl"
        cases = [
            url(WHEEL_ROW, f"http://files.pythonhosted.org/packages/{wheel}"),
            url(WHEEL_ROW, f"https://github.com/example/x/releases/download/1/{wheel}"),
            url(WHEEL_ROW, f"https://files.pythonhosted.org.example.invalid/packages/{wheel}"),
            url(WHEEL_ROW, f"https://user@files.pythonhosted.org/packages/{wheel}"),
            url(WHEEL_ROW, f"https://files.pythonhosted.org:8443/packages/{wheel}"),
            url(WHEEL_ROW, f"https://files.pythonhosted.org/packages/{wheel}?x=1"),
            url(WHEEL_ROW, f"https://files.pythonhosted.org/packages/{wheel}#x"),
            url(WHEEL_ROW, "https://files.pythonhosted.org/packages/other-3.0.11-py3-none-any.whl"),
            url(WHEEL_ROW, f"https://static.crates.io/crates/agno/{wheel}"),
            url(NPM, "https://files.pythonhosted.org/packages/core-2.3.0.tgz"),
            # github.com serves many things; only a release asset path is a pin.
            url(RELEASE, "https://github.com/example/standalone/raw/main/cpython-3.11.16%2B20260901-full.tar.zst"),
            url(RELEASE, "https://raw.githubusercontent.com/example/standalone/cpython-3.11.16%2B20260901-full.tar.zst"),
            url(RELEASE, "https://release-assets.githubusercontent.com/cpython-3.11.16%2B20260901-full.tar.zst"),
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
                rows[RELEASE] = pin("release-download", "tool", "1", file, url_value, ASSET, ["runtime node"])
                with Workspace(rows=rows) as work:
                    transport = Transport({url_value: [lambda: Response(200, ASSET)]})
                    work.fetch(transport)
                    self.assertEqual((work.destination / "release-download" / file).read_bytes(), ASSET)


class CommandLineTests(unittest.TestCase):
    def test_success_prints_one_json_line(self):
        with Workspace() as work:
            code, out, _ = main(work.arguments("--destination", str(work.destination)))
            self.assertEqual(code, 0)
            self.assertEqual(out.count("\n"), 1)
            summary = json.loads(out)
            self.assertEqual((summary["files"], summary["destination"]), (7, str(work.destination)))
            self.assertEqual(summary["pins_sha256"], hashlib.sha256(work.pins.read_bytes()).hexdigest())
            self.assertEqual(summary["resolved_pins"], str(work.destination / RESOLVED_NAME))
            self.assertEqual(summary["resolved_pins_sha256"],
                             hashlib.sha256((work.destination / RESOLVED_NAME).read_bytes()).hexdigest())
            self.assertEqual(summary["bytes"],
                             len(TARBALL) + len(WHEEL) + len(ASSET) + sum(len(data) for data in CRATES.values()))

    def test_a_refusal_is_one_line_on_stderr_and_exit_status_2(self):
        with Workspace() as work:
            work.destination.mkdir()
            code, out, err = main(work.arguments("--destination", str(work.destination)))
            self.assertEqual((code, out), (2, ""))
            self.assertEqual(err.count("\n"), 1)
            self.assertTrue(err.startswith("pinned input fetch refused: "), err)

    def test_resolve_writes_only_the_resolved_pin_file(self):
        with Workspace() as work:
            output = work.root / "resolved.json"
            transport = Transport()
            code, out, _ = main(work.arguments("--resolve", str(output)), transport)
            self.assertEqual(code, 0)
            self.assertEqual(transport.requests, [])
            self.assertFalse(work.destination.exists())
            self.assertEqual(FETCH.load_pins(output, resolved=True).rows, resolved_rows())
            self.assertEqual(json.loads(out), {
                "resolved_pins": str(output), "rows": 7,
                "resolved_pins_sha256": hashlib.sha256(output.read_bytes()).hexdigest()})
            # A fetch writes the same bytes.
            work.fetch()
            self.assertEqual((work.destination / RESOLVED_NAME).read_bytes(), output.read_bytes())
            # An existing output is never replaced.
            code, _, err = main(work.arguments("--resolve", str(output)))
            self.assertEqual(code, 2)
            self.assertIn("exists", err)

    def test_every_mode_needs_the_closure_and_the_lock(self):
        with Workspace() as work:
            pins, closure, lock = str(work.pins), str(work.closure), str(work.lock)
            destination = ["--destination", str(work.destination)]
            for argv in (["--pins", pins, *destination],
                         ["--pins", pins, "--closure", closure, *destination],
                         ["--pins", pins, "--lock", lock, *destination],
                         ["--closure", closure, "--lock", lock, *destination],
                         work.arguments(),
                         work.arguments(*destination, "--resolve", str(work.root / "resolved.json"))):
                with self.subTest(argv=argv):
                    transport = Transport()
                    code, out, err = main(argv, transport)
                    self.assertEqual((code, out), (2, ""))
                    self.assertTrue(err.startswith("pinned input fetch refused: "), err)
                    self.assertEqual(transport.requests, [])
                    self.assertFalse(work.destination.exists())


class CrateRowTests(unittest.TestCase):
    """Crate rows are never committed: they are the tool closure Cargo prints, with the lock's checksums."""

    def test_rows_come_from_the_closure_listing_and_the_lock(self):
        rows = FETCH.crate_rows(CLOSURE, LOCK)
        self.assertEqual(rows, [crate_pin(name, version) for name, version in sorted(CRATES)])
        self.assertEqual(rows[2], {
            "kind": "crate", "name": "itoa", "version": "1.0.18", "file": "itoa-1.0.18.crate",
            "url": "https://static.crates.io/crates/itoa/itoa-1.0.18.crate",
            "sha256": hashlib.sha256(CRATES[("itoa", "1.0.18")]).hexdigest(), "sha512": None,
            "digest_source": "root Cargo.lock checksum (the crates.io index cksum)",
            "use": ["tool build provenance"]})

    def test_a_registry_crate_without_a_lock_checksum_is_an_error(self):
        with self.assertRaises(FETCH.Refusal) as caught:
            FETCH.crate_rows(CLOSURE + "unsummed v0.2.0\n", LOCK)
        self.assertIn("unsummed 0.2.0", str(caught.exception))
        self.assertIn("checksum", str(caught.exception))

    def test_a_source_that_is_not_crates_io_is_an_error(self):
        for name, line in (("a git source", "forked v0.1.0\n"),
                           ("a git source as Cargo prints it", "forked v0.1.0 (https://example.invalid/forked#0123)\n"),
                           ("another registry", "elsewhere v0.3.0\n"),
                           ("another registry as Cargo prints it", "elsewhere v0.3.0 (registry `example`)\n")):
            with self.subTest(name=name), self.assertRaises(FETCH.Refusal):
                FETCH.crate_rows(CLOSURE + line, LOCK)

    def test_a_listing_the_lock_does_not_explain_is_refused(self):
        for name, closure in (("unknown shape", CLOSURE + "itoa 1.0.18\n"),
                              ("not in the lock", CLOSURE + "ryu v1.0.0\n"),
                              ("another version", CLOSURE + "itoa v1.0.19\n"),
                              ("a path crate with a registry entry", CLOSURE + "itoa v1.0.18 (/checkout/itoa)\n"),
                              ("a registry line for a workspace crate", CLOSURE + "toolkit-package v0.9.15\n"),
                              ("only workspace crates", CLOSURE.splitlines(keepends=True)[0]),
                              ("empty", "")):
            with self.subTest(name=name), self.assertRaises(FETCH.Refusal):
                FETCH.crate_rows(closure, LOCK)
        with self.assertRaises(FETCH.Refusal):
            FETCH.crate_rows(CLOSURE, "not a lock")
        twice = lock_text(f'\n[[package]]\nname = "itoa"\nversion = "1.0.18"\nsource = "{CRATES_IO}"\n'
                          f'checksum = "{"6" * 64}"\n')
        with self.assertRaises(FETCH.Refusal):
            FETCH.crate_rows(CLOSURE, twice)

    def test_the_closure_command_is_the_tool_closure_for_macos_arm64(self):
        self.assertEqual(FETCH.TOOL_CLOSURE_ARGUMENTS, (
            "-p", "toolkit-package", "--features", "local-recipes,local-artifacts",
            "--target", "aarch64-apple-darwin", "-e", "normal,build", "--prefix", "none", "--format", "{p}"))


def rows_with(rows, label):
    return [row for row in rows if label in row["use"]]


class CommittedPinFileTests(unittest.TestCase):
    """connect/release/library-pins-macos-arm64.json as its readers select from it."""

    @classmethod
    def setUpClass(cls):
        cls.pins = FETCH.load_pins(PINS)
        cls.rows = cls.pins.rows

    def test_the_file_is_the_canonical_rendering_for_macos_arm64(self):
        self.assertEqual(self.pins.document["target"], "macos-arm64")
        self.assertEqual(PINS.read_text(encoding="utf-8"), FETCH.render(self.pins.document))

    def test_no_crate_row_is_committed_and_the_note_says_where_they_come_from(self):
        self.assertEqual({row["kind"] for row in self.rows}, {"npm-tarball", "pypi-wheel", "release-download"})
        self.assertEqual(rows_with(self.rows, "tool build provenance"), [])
        note = self.pins.document["note"]
        for words in ("Cargo.lock", "fetch-pinned-inputs.py", RESOLVED_NAME):
            self.assertIn(words, note)

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

    def test_the_macos_producer_selection_binds_exactly_this_pin_file(self):
        # The producer refuses every macOS step unless its committed selection names this file's digest,
        # so a pin change without the selection (or the other way round) fails here first.
        selection = json.loads((ROOT / "release/connect-release-inputs-macos-arm64.json").read_text(encoding="utf-8"))
        library = selection["library"]
        self.assertEqual(library["state"], "selected")
        self.assertEqual(library["pins"], {"path": "release/library-pins-macos-arm64.json",
                                           "sha256": hashlib.sha256(PINS.read_bytes()).hexdigest()})
        self.assertEqual(library["pins"]["sha256"], self.pins.sha256)
        self.assertNotIn("PENDING", library["reason"])

    @unittest.skipUnless(os.environ.get("CONNECT_PINS_TOOL_CLOSURE") == "1",
                         "needs Cargo and the registry: set CONNECT_PINS_TOOL_CLOSURE=1 (the macOS stage run does)")
    def test_the_tool_closure_cargo_prints_resolves_against_the_current_lock(self):
        result = subprocess.run(
            [str(ROOT / "scripts/repo-cargo"), "tree", "--locked", "--manifest-path",
             str(REPOSITORY / "Cargo.toml"), *FETCH.TOOL_CLOSURE_ARGUMENTS],
            cwd=REPOSITORY, capture_output=True, text=True, stdin=subprocess.DEVNULL)
        self.assertEqual(result.returncode, 0, result.stderr[-2000:])
        lock = (REPOSITORY / "Cargo.lock").read_text(encoding="utf-8")
        crates = FETCH.crate_rows(result.stdout, lock)
        self.assertGreater(len(crates), 50)
        document = FETCH.resolve(self.pins, crates)
        self.assertEqual(FETCH.validate_document(document, resolved=True),
                         crates + self.rows)
        self.assertEqual(rows_with(document["pins"], "tool build provenance"), crates)


if __name__ == "__main__":
    unittest.main()
