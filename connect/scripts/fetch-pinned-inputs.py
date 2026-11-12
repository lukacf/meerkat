#!/usr/bin/env python3
"""Fetch the pinned public inputs of a Connect library build, or keep the pin file's crate rows current.

    python3 -I connect/scripts/fetch-pinned-inputs.py --pins connect/release/library-pins-macos-arm64.json \\
        --destination /private/tmp/cw/pins

The pin file (schema connect-library-pins/1) lists every public index artifact a library build reads:
PyPI wheels, npm archives, crates.io archives and publisher release downloads, each with its URL and the
digest its own index publishes (sha256; sha512 for npm, whose registry publishes no sha256). This step is
the only one that downloads them. It writes DESTINATION/<kind>/<file> for every pin, each checked against
its pinned digest before it is moved into place, then DESTINATION/FETCHED.json:

    {"schema": "connect-pinned-inputs/1", "pins_sha256": "<sha256 of the pin file>",
     "files": [{"kind", "file", "bytes", "sha256"}]}

where sha256 is always computed here from the fetched bytes. A directory without FETCHED.json is a failed
fetch and must not be used. The packagers and the exporter read DESTINATION/<kind>/<file>, never download,
and check each file they read against the pin file again.

Refused (exit status 2, one line on stderr, nothing recorded): a pin file of another shape, a URL that is
not https on a host of its kind (files.pythonhosted.org; registry.npmjs.org; static.crates.io; release
assets of github.com, nodejs.org and bootstrap.pypa.io for release downloads), a redirect out of that
host family (github.com release assets may redirect to GitHub's two asset stores), a file over 2 GiB, any
digest difference, and an existing destination. A server error, a rate limit, a connection error or a
body that ends early is retried a bounded number of times first. Nothing fetched is opened, unpacked or
run here.

The crate rows are generated, not hand-kept: they are the dependency closure of the six native tools for
the target (normal and build edges), with the root Cargo.lock checksums. Print the closure with

    connect/scripts/repo-cargo tree --locked --manifest-path Cargo.toml -p toolkit-package \\
        --features local-recipes,local-artifacts --target aarch64-apple-darwin -e normal,build \\
        --prefix none --format "{p}" > closure.txt

and compare or rewrite the rows (no download, no Cargo):

    python3 -I connect/scripts/fetch-pinned-inputs.py --pins FILE --check-crate-rows \\
        --closure closure.txt --lock Cargo.lock
    python3 -I connect/scripts/fetch-pinned-inputs.py --pins FILE --write-crate-rows \\
        --closure closure.txt --lock Cargo.lock
"""
from __future__ import annotations

import argparse
import collections
import hashlib
import http.client
import json
import os
from pathlib import Path
import re
import stat
import sys
import time
import tomllib
import urllib.error
import urllib.parse
import urllib.request

SCHEMA = "connect-library-pins/1"
FETCHED_SCHEMA = "connect-pinned-inputs/1"
KINDS = ("crate", "npm-tarball", "pypi-wheel", "release-download")
ROW_KEYS = ("kind", "name", "version", "file", "url", "sha256", "sha512", "digest_source", "use")
# The labels readers select by. A label is added here before a row may carry it, so a reader's label
# cannot change by accident.
LABELS = frozenset({
    "agno closure", "services closure", "activepieces closure",
    "component build tool", "component build backend", "tool build provenance",
    "runtime python", "runtime node", "runtime pip", "acquisition node", "acquisition npm",
    "official github", "official playwright",
})
# kind -> origin host -> the other hosts a redirect from that origin may name.
HOSTS = {
    "crate": {"static.crates.io": ()},
    "npm-tarball": {"registry.npmjs.org": ()},
    "pypi-wheel": {"files.pythonhosted.org": ()},
    "release-download": {
        "github.com": ("objects.githubusercontent.com", "release-assets.githubusercontent.com"),
        "nodejs.org": (),
        "bootstrap.pypa.io": (),
    },
}
GITHUB_RELEASE_ASSET = re.compile(r"/[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+/releases/download/[^/]+/[^/]+")
FILE_NAME = re.compile(r"[A-Za-z0-9][A-Za-z0-9._+-]*")
SHA256 = re.compile(r"[0-9a-f]{64}")
SHA512 = re.compile(r"[0-9a-f]{128}")
MAX_BYTES = 2 * 1024 * 1024 * 1024
CHUNK = 1024 * 1024
ATTEMPTS = 4
BACKOFF_SECONDS = 3
MAX_REDIRECTS = 5
TIMEOUT_SECONDS = 60
REDIRECT_STATUSES = (301, 302, 303, 307, 308)
TRANSIENT_STATUSES = (408, 425, 429)
USER_AGENT = "meerkat-connect-pinned-inputs/1"
CRATES_IO = "registry+https://github.com/rust-lang/crates.io-index"
# The arguments after `repo-cargo tree --locked --manifest-path <root>/Cargo.toml`: the six native tools
# are the bins of toolkit-package under these two features, built for macOS arm64.
TOOL_CLOSURE_ARGUMENTS = (
    "-p", "toolkit-package", "--features", "local-recipes,local-artifacts",
    "--target", "aarch64-apple-darwin", "-e", "normal,build", "--prefix", "none", "--format", "{p}",
)
CLOSURE_LINE = re.compile(
    r"(?P<name>[A-Za-z0-9_-]+) v(?P<version>[0-9][0-9A-Za-z.+-]*)"
    r"(?: \(proc-macro\))?(?P<path> \(/[^()]*\))?(?: \(\*\))?")

Pins = collections.namedtuple("Pins", "document rows sha256")


class Refusal(Exception):
    """The step stops; nothing after it may use the destination."""


class Transient(Exception):
    """A failure worth another attempt: the server or the connection, not the pin."""


def _no_duplicates(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise Refusal(f"the pin file has a duplicate key {key!r}")
        result[key] = value
    return result


def _refuse_constant(value):
    raise Refusal(f"the pin file is not JSON ({value})")


def _text(row, key, where):
    if not isinstance(row[key], str) or not row[key]:
        raise Refusal(f"{where}: {key} must be a non-empty string")
    return row[key]


def _check_url(row, where):
    kind, url = row["kind"], row["url"]
    try:
        parts = urllib.parse.urlsplit(url)
        host = parts.hostname
    except ValueError:
        raise Refusal(f"{where}: url is not a URL") from None
    if parts.scheme != "https" or not host or parts.netloc != host or parts.query or parts.fragment:
        raise Refusal(f"{where}: url must be plain https without user, port, query or fragment")
    if host not in HOSTS[kind]:
        raise Refusal(f"{where}: url host {host} is not a host of {kind} pins")
    if host == "github.com" and not GITHUB_RELEASE_ASSET.fullmatch(parts.path):
        raise Refusal(f"{where}: url on github.com is not a release asset")
    if urllib.parse.unquote(parts.path.rsplit("/", 1)[-1]) != row["file"]:
        raise Refusal(f"{where}: url does not end in the file name")


def validate_document(document):
    """The rows of a pin document, or a Refusal that names the first thing wrong with it."""
    if not isinstance(document, dict) or set(document) != {"schema", "target", "pins"}:
        raise Refusal("the pin file keys must be exactly schema, target, pins")
    if document["schema"] != SCHEMA:
        raise Refusal(f"the pin file schema must be {SCHEMA}")
    if not isinstance(document["target"], str) or not re.fullmatch(r"[a-z0-9]+(?:-[a-z0-9]+)*", document["target"]):
        raise Refusal("the pin file target must be a target name")
    rows = document["pins"]
    if not isinstance(rows, list) or not rows:
        raise Refusal("the pin file pins must be a non-empty list")
    seen = set()
    for index, row in enumerate(rows):
        where = f"pin {index}"
        if not isinstance(row, dict) or tuple(sorted(row)) != tuple(sorted(ROW_KEYS)):
            raise Refusal(f"{where}: keys must be exactly {', '.join(ROW_KEYS)}")
        kind = row["kind"]
        if kind not in KINDS:
            raise Refusal(f"{where}: unknown kind {kind!r}")
        file = row["file"]
        if not isinstance(file, str) or not FILE_NAME.fullmatch(file) or file == "FETCHED.json":
            raise Refusal(f"{where}: file must be a plain file name")
        where = f"{where} ({kind}/{file})"
        for key in ("name", "version", "url", "digest_source"):
            _text(row, key, where)
        sha256, sha512 = row["sha256"], row["sha512"]
        if kind == "npm-tarball":
            if sha256 is not None or not isinstance(sha512, str) or not SHA512.fullmatch(sha512):
                raise Refusal(f"{where}: an npm row carries sha512 (128 lowercase hex) and a null sha256")
        elif sha512 is not None or not isinstance(sha256, str) or not SHA256.fullmatch(sha256):
            raise Refusal(f"{where}: a {kind} row carries sha256 (64 lowercase hex) and a null sha512")
        use = row["use"]
        if (not isinstance(use, list) or not use or any(label not in LABELS for label in use)
                or use != sorted(set(use))):
            raise Refusal(f"{where}: use must be a sorted list of distinct known labels")
        _check_url(row, where)
        if (kind, file) in seen:
            raise Refusal(f"{where}: this kind and file occur twice")
        seen.add((kind, file))
    keys = [(row["kind"], row["name"], row["version"], row["file"]) for row in rows]
    if keys != sorted(keys):
        raise Refusal("the pin file rows are not sorted by kind, name, version, file")
    return rows


def load_pins(path):
    try:
        data = Path(path).read_bytes()
    except OSError as error:
        raise Refusal(f"the pin file cannot be read ({type(error).__name__})") from None
    try:
        document = json.loads(data.decode("utf-8"), object_pairs_hook=_no_duplicates,
                              parse_constant=_refuse_constant)
    except (UnicodeDecodeError, json.JSONDecodeError):
        raise Refusal("the pin file is not JSON") from None
    return Pins(document, validate_document(document), hashlib.sha256(data).hexdigest())


def render(document):
    """The pin file's text: one row per line, keys in ROW_KEYS order."""
    rows = document["pins"]
    lines = ["{", f'  "schema": {json.dumps(document["schema"])},',
             f'  "target": {json.dumps(document["target"])},', '  "pins": [']
    for index, row in enumerate(rows):
        ordered = {key: row[key] for key in ROW_KEYS}
        lines.append("    " + json.dumps(ordered, ensure_ascii=True) + ("," if index + 1 < len(rows) else ""))
    lines.extend(["  ]", "}"])
    return "\n".join(lines) + "\n"


class _NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, *args, **kwargs):
        return None


class _Answer:
    def __init__(self, status, headers, body):
        self.status = status
        self.headers = headers
        self._body = body

    def read(self, size):
        return self._body.read(size)

    def close(self):
        self._body.close()


def https_transport(url):
    """One GET without following redirects: the caller decides where a redirect may lead."""
    opener = urllib.request.build_opener(_NoRedirect)
    request = urllib.request.Request(
        url, headers={"User-Agent": USER_AGENT, "Accept": "*/*", "Accept-Encoding": "identity"})
    try:
        response = opener.open(request, timeout=TIMEOUT_SECONDS)
    except urllib.error.HTTPError as error:
        return _Answer(error.code, error.headers, error)
    return _Answer(response.status, response.headers, response)


def _redirect_allowed(kind, origin, url):
    try:
        parts = urllib.parse.urlsplit(url)
        host = parts.hostname
    except ValueError:
        return False
    if parts.scheme != "https" or not host or parts.netloc != host:
        return False
    return host == origin or host in HOSTS[kind][origin]


def _download_once(row, temporary, transport):
    label = f"{row['kind']}/{row['file']}"
    origin = urllib.parse.urlsplit(row["url"]).hostname
    url = row["url"]
    for _ in range(MAX_REDIRECTS + 1):
        try:
            response = transport(url)
        except (OSError, http.client.HTTPException) as error:
            raise Transient(f"connection failed ({type(error).__name__})") from None
        try:
            status = response.status
            if status in REDIRECT_STATUSES:
                location = response.headers.get("Location")
                if not location:
                    raise Refusal(f"{label}: redirect without a location")
                url = urllib.parse.urljoin(url, location)
                if not _redirect_allowed(row["kind"], origin, url):
                    raise Refusal(f"{label}: redirect out of the host family of {origin}")
                continue
            if status in TRANSIENT_STATUSES or 500 <= status < 600:
                raise Transient(f"HTTP {status}")
            if status != 200:
                raise Refusal(f"{label}: HTTP {status}")
            declared = response.headers.get("Content-Length")
            if declared is not None:
                if not re.fullmatch(r"[0-9]+", declared):
                    raise Refusal(f"{label}: unreadable Content-Length")
                declared = int(declared)
                if declared > MAX_BYTES:
                    raise Refusal(f"{label}: over the 2 GiB limit")
            sha256, sha512, size = hashlib.sha256(), hashlib.sha512(), 0
            with open(temporary, "wb") as handle:
                while True:
                    try:
                        chunk = response.read(CHUNK)
                    except (OSError, http.client.HTTPException) as error:
                        raise Transient(f"connection failed ({type(error).__name__})") from None
                    if not chunk:
                        break
                    size += len(chunk)
                    if size > MAX_BYTES:
                        raise Refusal(f"{label}: over the 2 GiB limit")
                    sha256.update(chunk)
                    sha512.update(chunk)
                    handle.write(chunk)
            if declared is not None and size != declared:
                raise Transient("the body ended early")
        finally:
            response.close()
        for name, digest in (("sha256", sha256), ("sha512", sha512)):
            if row[name] is not None and digest.hexdigest() != row[name]:
                raise Refusal(f"{label}: {name} digest differs from the pin (fetched {digest.hexdigest()})")
        return size, sha256.hexdigest()
    raise Refusal(f"{label}: too many redirects")


def _download(row, temporary, transport, sleep):
    last = None
    for attempt in range(ATTEMPTS):
        if attempt:
            sleep(BACKOFF_SECONDS * attempt)
        try:
            return _download_once(row, temporary, transport)
        except Transient as error:
            last = error
    raise Refusal(f"{row['kind']}/{row['file']}: {last} in each of {ATTEMPTS} attempts")


def fetch(pins_path, destination, transport=None, sleep=time.sleep, progress=None):
    """Fetch every pin into a new destination directory and return the FETCHED.json record."""
    destination = Path(destination)
    if not destination.is_absolute():
        raise Refusal("the destination must be an absolute path")
    pins = load_pins(pins_path)
    try:
        parent = os.lstat(destination.parent)
    except OSError:
        parent = None
    if parent is None or not stat.S_ISDIR(parent.st_mode):
        raise Refusal("the destination's parent must be an existing directory, not a link")
    if os.path.lexists(destination):
        raise Refusal("the destination exists; a fetch writes a new directory")
    transport = https_transport if transport is None else transport
    partial = destination / ".partial"
    os.mkdir(destination, 0o755)
    os.mkdir(partial, 0o755)
    files = []
    for index, row in enumerate(pins.rows):
        directory = destination / row["kind"]
        if not directory.exists():
            os.mkdir(directory, 0o755)
            os.chmod(directory, 0o755)
        temporary = partial / f"{index:05d}"
        size, sha256 = _download(row, temporary, transport, sleep)
        os.chmod(temporary, 0o644)
        os.replace(temporary, directory / row["file"])
        files.append({"kind": row["kind"], "file": row["file"], "bytes": size, "sha256": sha256})
        if progress is not None:
            progress(f"fetched {row['kind']}/{row['file']} ({size} bytes)")
    os.rmdir(partial)
    os.chmod(destination, 0o755)
    record = {"schema": FETCHED_SCHEMA, "pins_sha256": pins.sha256, "files": files}
    temporary = destination / ".FETCHED.json"
    temporary.write_text(json.dumps(record, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    os.chmod(temporary, 0o644)
    os.replace(temporary, destination / "FETCHED.json")
    return record


def crate_rows(closure_text, lock_text):
    """The crate rows for a tool closure listing (`cargo tree --prefix none --format "{p}"`) and the root
    Cargo.lock: one row per registry crate, with the lock's checksum. Workspace crates must be lock
    packages without a source; any other line shape or source is refused."""
    try:
        packages = collections.defaultdict(list)
        for package in tomllib.loads(lock_text)["package"]:
            packages[(package["name"], package["version"])].append(package)
    except (tomllib.TOMLDecodeError, KeyError, TypeError):
        raise Refusal("the lock is not a Cargo.lock") from None
    wanted = set()
    for line in closure_text.splitlines():
        match = CLOSURE_LINE.fullmatch(line)
        if not match:
            raise Refusal(f"unsupported closure line {line!r}")
        wanted.add((match["name"], match["version"], match["path"] is not None))
    rows = []
    for name, version, workspace in sorted(wanted):
        entries = packages.get((name, version), [])
        if len(entries) != 1:
            raise Refusal(f"{name} {version}: {len(entries)} packages in the lock")
        entry = entries[0]
        if workspace:
            if "source" in entry:
                raise Refusal(f"{name} {version}: a path crate with a lock source")
            continue
        checksum = entry.get("checksum")
        if entry.get("source") != CRATES_IO or not isinstance(checksum, str) or not SHA256.fullmatch(checksum):
            raise Refusal(f"{name} {version}: not a crates.io package with a lock checksum")
        rows.append({
            "kind": "crate", "name": name, "version": version, "file": f"{name}-{version}.crate",
            "url": f"https://static.crates.io/crates/{name}/{name}-{version}.crate",
            "sha256": checksum, "sha512": None,
            "digest_source": "root Cargo.lock checksum (the crates.io index cksum)",
            "use": ["tool build provenance"],
        })
    if not rows:
        raise Refusal("the closure listing names no registry crate")
    return rows


def _crate_mode(pins_path, closure_path, lock_path, write):
    pins = load_pins(pins_path)
    try:
        closure_text = Path(closure_path).read_text(encoding="utf-8")
        lock_text = Path(lock_path).read_text(encoding="utf-8")
    except (OSError, UnicodeDecodeError) as error:
        raise Refusal(f"the closure listing or the lock cannot be read ({type(error).__name__})") from None
    wanted = crate_rows(closure_text, lock_text)
    current = [row for row in pins.rows if row["kind"] == "crate"]
    if current == wanted:
        print(json.dumps({"crate_rows": len(wanted), "status": "current"}, sort_keys=True))
        return 0
    if not write:
        have = {row["file"]: row for row in current}
        want = {row["file"]: row for row in wanted}
        for file in sorted(set(want) - set(have)):
            print(f"missing crate row: {file}", file=sys.stderr)
        for file in sorted(set(have) - set(want)):
            print(f"crate row outside the closure: {file}", file=sys.stderr)
        for file in sorted(set(have) & set(want)):
            if have[file] != want[file]:
                print(f"crate row differs from the lock: {file}", file=sys.stderr)
        print("the crate rows are stale: run this script with --write-crate-rows and the same "
              "--closure and --lock", file=sys.stderr)
        return 1
    rows = [row for row in pins.rows if row["kind"] != "crate"] + wanted
    rows.sort(key=lambda row: (row["kind"], row["name"], row["version"], row["file"]))
    document = {"schema": pins.document["schema"], "target": pins.document["target"], "pins": rows}
    validate_document(document)
    path = Path(pins_path)
    temporary = path.with_name(path.name + ".new")
    temporary.write_text(render(document), encoding="utf-8")
    os.replace(temporary, path)
    print(json.dumps({"crate_rows": len(wanted), "status": "written"}, sort_keys=True))
    return 0


class _Parser(argparse.ArgumentParser):
    def error(self, message):
        raise Refusal(f"arguments: {message}")


def main(argv=None, transport=None, sleep=time.sleep):
    parser = _Parser(description="Fetch the pinned public inputs of a Connect library build.")
    parser.add_argument("--pins", required=True, help="the pin file (connect-library-pins/1)")
    parser.add_argument("--destination", help="a new absolute directory for the fetched files")
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument("--check-crate-rows", action="store_true",
                      help="compare the crate rows with --closure and --lock; exit 1 when they differ")
    mode.add_argument("--write-crate-rows", action="store_true",
                      help="rewrite the crate rows from --closure and --lock")
    parser.add_argument("--closure", help="the tool closure listing Cargo printed (see the module text)")
    parser.add_argument("--lock", help="the root Cargo.lock")
    try:
        arguments = parser.parse_args(argv)
        if arguments.check_crate_rows or arguments.write_crate_rows:
            if not arguments.closure or not arguments.lock or arguments.destination:
                raise Refusal("arguments: the crate row modes take --closure and --lock and no --destination")
            return _crate_mode(arguments.pins, arguments.closure, arguments.lock, arguments.write_crate_rows)
        if not arguments.destination or arguments.closure or arguments.lock:
            raise Refusal("arguments: a fetch takes --destination and neither --closure nor --lock")
        record = fetch(arguments.pins, arguments.destination, transport=transport, sleep=sleep,
                       progress=lambda line: print(line, file=sys.stderr, flush=True))
    except Refusal as refusal:
        print(f"pinned input fetch refused: {refusal}", file=sys.stderr)
        return 2
    print(json.dumps({
        "destination": str(arguments.destination), "files": len(record["files"]),
        "bytes": sum(entry["bytes"] for entry in record["files"]), "pins_sha256": record["pins_sha256"],
    }, sort_keys=True))
    return 0


if __name__ == "__main__":
    sys.exit(main())
