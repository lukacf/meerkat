#!/usr/bin/env python3
"""Fetch the pinned public inputs of a Connect library build.

    connect/scripts/repo-cargo tree --locked --manifest-path Cargo.toml -p toolkit-package \\
        --features local-recipes,local-artifacts --target aarch64-apple-darwin -e normal,build \\
        --prefix none --format "{p}" > /private/tmp/cw/tool-closure.txt
    python3 -I connect/scripts/fetch-pinned-inputs.py \\
        --pins connect/release/library-pins-macos-arm64.json \\
        --closure /private/tmp/cw/tool-closure.txt --lock Cargo.lock \\
        --destination /private/tmp/cw/pins

The committed pin file (schema connect-library-pins/1) lists the public index artifacts a library build
reads that no lock in the repository pins: PyPI wheels, npm archives and publisher release downloads,
each with its URL and the digest its own index publishes (sha256; sha512 for npm, whose registry
publishes no sha256). It carries NO crate row: Cargo.lock is the pin for crates. The crate rows are
derived here, at fetch time, from the dependency closure of the six native tools that Cargo prints (the
command above: normal and build edges for the target) and the root Cargo.lock: name, version, the lock's
checksum, and the crates.io download URL. A registry crate without a lock checksum, a crate from a git
source or another registry, and a listing line the lock does not explain are errors.

This step is the only one that downloads these files. It writes, into a NEW destination directory:

    DESTINATION/library-pins-<target>.resolved.json   the RESOLVED pin file: the committed rows plus the
                                                      derived crate rows, same schema, same labels
    DESTINATION/<kind>/<file>                         every row's file, checked against its pinned digest
                                                      before it is moved into place
    DESTINATION/FETCHED.json                          written last:
        {"schema": "connect-pinned-inputs/1", "pins_sha256": "<the committed pin file>",
         "lock_sha256": "<Cargo.lock>", "resolved_pins": {"file", "rows", "sha256"},
         "files": [{"kind", "file", "bytes", "sha256"}]}

where each file's sha256 is computed here from the fetched bytes. A directory without FETCHED.json is a
failed fetch and must not be used. The packagers, the exporter and the producer are given the RESOLVED
file as their --pins and the destination as their --pinned-inputs; they never download and check each
file they read against its row again.

Refused (exit status 2, one line on stderr): a pin file of another shape or with a crate row, a URL that
is not https on a host of its kind (files.pythonhosted.org; registry.npmjs.org; static.crates.io; release
assets of github.com, nodejs.org and bootstrap.pypa.io for release downloads), a redirect out of that
host family (github.com release assets may redirect to GitHub's two asset stores), a file over 2 GiB, any
digest difference, and an existing destination. A server error, a rate limit, a connection error or a
body that ends early is retried a bounded number of times first. Nothing fetched is opened, unpacked or
run here, and this script starts no other program.

    python3 -I connect/scripts/fetch-pinned-inputs.py --pins FILE --closure FILE --lock FILE --resolve OUTPUT

writes only the resolved pin file (no download), for a look at what a fetch would read.
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
# `<name> v<version>`, then ` (proc-macro)`, then ` (<absolute path>)` for a workspace crate, then ` (*)`
# for a repeated line. A git or other registry source prints something else in the brackets and is not
# a line this accepts.
CLOSURE_LINE = re.compile(
    r"(?P<name>[A-Za-z0-9_-]+) v(?P<version>[0-9][0-9A-Za-z.+-]*)"
    r"(?: \(proc-macro\))?(?P<path> \(/[^()]*\))?(?: \(\*\))?")

Pins = collections.namedtuple("Pins", "document rows sha256")


class Refusal(Exception):
    """The step stops; nothing after it may use the destination."""


class Transient(Exception):
    """A failure worth another attempt: the server or the connection, not the pin."""


def resolved_name(target):
    return f"library-pins-{target}.resolved.json"


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


def validate_document(document, resolved=False):
    """The rows of a pin document, or a Refusal that names the first thing wrong with it.

    A committed pin file may carry a `note` for people and never a crate row; a resolved pin file (what
    a fetch writes and every reader is given) has exactly schema, target and pins, crate rows included.
    """
    allowed = {"schema", "target", "pins"} if resolved else {"schema", "target", "pins", "note"}
    if (not isinstance(document, dict) or not {"schema", "target", "pins"} <= set(document)
            or not set(document) <= allowed):
        raise Refusal(f"the pin file keys must be {', '.join(sorted(allowed))}")
    if document["schema"] != SCHEMA:
        raise Refusal(f"the pin file schema must be {SCHEMA}")
    if not isinstance(document["target"], str) or not re.fullmatch(r"[a-z0-9]+(?:-[a-z0-9]+)*", document["target"]):
        raise Refusal("the pin file target must be a target name")
    if "note" in document and (not isinstance(document["note"], str) or not document["note"]):
        raise Refusal("the pin file note must be a non-empty string")
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
        if kind == "crate" and not resolved:
            raise Refusal(f"{where}: a crate row is never committed; crate rows are derived from Cargo.lock "
                          "at fetch time")
        file = row["file"]
        if (not isinstance(file, str) or not FILE_NAME.fullmatch(file) or file == "FETCHED.json"
                or file.endswith(".resolved.json")):
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


def _read(path, what):
    try:
        return Path(path).read_bytes()
    except OSError as error:
        raise Refusal(f"{what} cannot be read ({type(error).__name__})") from None


def load_pins(path, resolved=False):
    data = _read(path, "the pin file")
    try:
        document = json.loads(data.decode("utf-8"), object_pairs_hook=_no_duplicates,
                              parse_constant=_refuse_constant)
    except (UnicodeDecodeError, json.JSONDecodeError):
        raise Refusal("the pin file is not JSON") from None
    return Pins(document, validate_document(document, resolved=resolved), hashlib.sha256(data).hexdigest())


def render(document):
    """A pin file's text: one row per line, keys in ROW_KEYS order."""
    rows = document["pins"]
    lines = ["{", f'  "schema": {json.dumps(document["schema"])},',
             f'  "target": {json.dumps(document["target"])},']
    if "note" in document:
        lines.append(f'  "note": {json.dumps(document["note"], ensure_ascii=True)},')
    lines.append('  "pins": [')
    for index, row in enumerate(rows):
        ordered = {key: row[key] for key in ROW_KEYS}
        lines.append("    " + json.dumps(ordered, ensure_ascii=True) + ("," if index + 1 < len(rows) else ""))
    lines.extend(["  ]", "}"])
    return "\n".join(lines) + "\n"


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
        if entry.get("source") != CRATES_IO:
            raise Refusal(f"{name} {version}: not a crates.io package in the lock")
        checksum = entry.get("checksum")
        if not isinstance(checksum, str) or not SHA256.fullmatch(checksum):
            raise Refusal(f"{name} {version}: a crates.io package without a lock checksum")
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


def resolve(pins, crates):
    """The resolved pin document: the committed rows plus the derived crate rows."""
    rows = list(pins.rows) + list(crates)
    rows.sort(key=lambda row: (row["kind"], row["name"], row["version"], row["file"]))
    document = {"schema": pins.document["schema"], "target": pins.document["target"], "pins": rows}
    validate_document(document, resolved=True)
    return document


def _resolve_inputs(pins_path, closure_path, lock_path):
    pins = load_pins(pins_path)
    closure = _read(closure_path, "the closure listing")
    lock = _read(lock_path, "the lock")
    try:
        crates = crate_rows(closure.decode("utf-8"), lock.decode("utf-8"))
    except UnicodeDecodeError:
        raise Refusal("the closure listing or the lock is not UTF-8") from None
    return pins, resolve(pins, crates), hashlib.sha256(lock).hexdigest()


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


def _write(path, text):
    path.write_text(text, encoding="utf-8")
    os.chmod(path, 0o644)


def fetch(pins_path, closure_path, lock_path, destination, transport=None, sleep=time.sleep, progress=None):
    """Resolve the pins, fetch every row into a new destination directory, return the FETCHED.json record."""
    destination = Path(destination)
    if not destination.is_absolute():
        raise Refusal("the destination must be an absolute path")
    pins, document, lock_sha256 = _resolve_inputs(pins_path, closure_path, lock_path)
    try:
        parent = os.lstat(destination.parent)
    except OSError:
        parent = None
    if parent is None or not stat.S_ISDIR(parent.st_mode):
        raise Refusal("the destination's parent must be an existing directory, not a link")
    if os.path.lexists(destination):
        raise Refusal("the destination exists; a fetch writes a new directory")
    transport = https_transport if transport is None else transport
    name = resolved_name(document["target"])
    text = render(document)
    partial = destination / ".partial"
    os.mkdir(destination, 0o755)
    os.chmod(destination, 0o755)
    _write(destination / name, text)
    os.mkdir(partial, 0o755)
    files = []
    for index, row in enumerate(document["pins"]):
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
    record = {
        "schema": FETCHED_SCHEMA, "pins_sha256": pins.sha256, "lock_sha256": lock_sha256,
        "resolved_pins": {"file": name, "rows": len(document["pins"]),
                          "sha256": hashlib.sha256(text.encode("utf-8")).hexdigest()},
        "files": files,
    }
    temporary = destination / ".FETCHED.json"
    _write(temporary, json.dumps(record, indent=2, sort_keys=True) + "\n")
    os.replace(temporary, destination / "FETCHED.json")
    return record


def write_resolved(pins_path, closure_path, lock_path, output):
    """Write only the resolved pin file (no download); never replaces a file."""
    output = Path(output)
    _, document, _ = _resolve_inputs(pins_path, closure_path, lock_path)
    if os.path.lexists(output):
        raise Refusal("the resolved pin file exists; it is never replaced")
    if not output.parent.is_dir():
        raise Refusal("the resolved pin file's parent must be an existing directory")
    text = render(document)
    _write(output, text)
    return {"resolved_pins": str(output), "rows": len(document["pins"]),
            "resolved_pins_sha256": hashlib.sha256(text.encode("utf-8")).hexdigest()}


class _Parser(argparse.ArgumentParser):
    def error(self, message):
        raise Refusal(f"arguments: {message}")


def main(argv=None, transport=None, sleep=time.sleep):
    parser = _Parser(description="Fetch the pinned public inputs of a Connect library build.")
    parser.add_argument("--pins", required=True, help="the committed pin file (connect-library-pins/1)")
    parser.add_argument("--closure", required=True,
                        help="the tool closure listing Cargo printed (the command is in the module text)")
    parser.add_argument("--lock", required=True, help="the root Cargo.lock: the pin for crates")
    parser.add_argument("--destination", help="a new absolute directory for the fetched files")
    parser.add_argument("--resolve", metavar="OUTPUT",
                        help="write only the resolved pin file to OUTPUT; download nothing")
    try:
        arguments = parser.parse_args(argv)
        if bool(arguments.destination) == bool(arguments.resolve):
            raise Refusal("arguments: give exactly one of --destination and --resolve")
        if arguments.resolve:
            summary = write_resolved(arguments.pins, arguments.closure, arguments.lock, arguments.resolve)
        else:
            record = fetch(arguments.pins, arguments.closure, arguments.lock, arguments.destination,
                           transport=transport, sleep=sleep,
                           progress=lambda line: print(line, file=sys.stderr, flush=True))
            summary = {
                "destination": str(arguments.destination), "files": len(record["files"]),
                "bytes": sum(entry["bytes"] for entry in record["files"]),
                "pins_sha256": record["pins_sha256"],
                "resolved_pins": str(Path(arguments.destination) / record["resolved_pins"]["file"]),
                "resolved_pins_sha256": record["resolved_pins"]["sha256"],
            }
    except Refusal as refusal:
        print(f"pinned input fetch refused: {refusal}", file=sys.stderr)
        return 2
    print(json.dumps(summary, sort_keys=True))
    return 0


if __name__ == "__main__":
    sys.exit(main())
