#!/usr/bin/env python3
"""Self-test for scripts/release-registry-preflight: no network, nothing published.

The script runs under `python3 -I` against a loopback stub of npm, PyPI's index
and crates.io (MEERKAT_REGISTRY_PREFLIGHT_STUB) and a fake `twine` on PATH, on a
fixture repository, so every registry answer is chosen here. The stub records
every request, the credential it carried and the bytes every write re-sent.

The four safety rules, one group each:
  (a) only an already published version is probed, never the release version,
      and only after its artifact matched the registry's digest;
  (b) the default mode is a plan: no write, no token read;
  (c) crates.io: the fixed family sample at release time, the full probe paced
      and refused inside a release workflow;
  (d) every package gets PASS, FAIL, INCONCLUSIVE or SKIP with the registry's
      exact answer, and no token reaches the log, the report or the summary.
Then every row of each classifier, the credential-less
control, the endpoint guard, the package names derived from this repository,
and the registry-preflight.yml contract.
"""

from __future__ import annotations

import base64
import email.utils
import hashlib
import http.server
import importlib.util
import io
import json
import os
import re
import struct
import subprocess
import sys
import tarfile
import tempfile
import threading
import time
import unittest
import urllib.parse
import zipfile
from importlib.machinery import SourceFileLoader
from pathlib import Path

SCRIPTS = Path(__file__).resolve().parent
REPO_ROOT = SCRIPTS.parent
SCRIPT = SCRIPTS / "release-registry-preflight"
WORKFLOW = REPO_ROOT / ".github" / "workflows" / "registry-preflight.yml"

sys.dont_write_bytecode = True
_LOADER = SourceFileLoader("release_registry_preflight", str(SCRIPT))
_SPEC = importlib.util.spec_from_loader(_LOADER.name, _LOADER)
assert _SPEC is not None
preflight = importlib.util.module_from_spec(_SPEC)
sys.modules[_LOADER.name] = preflight
_LOADER.exec_module(preflight)

# Distinctive values the log, the report and the summary must never contain.
NPM_SECRET = "test-npm-secret-must-not-leak-41"
PYPI_SECRET = "test-pypi-secret-must-not-leak-42"
CRATES_SECRET = "test-crates-secret-must-not-leak-43"
SECRETS = (NPM_SECRET, PYPI_SECRET, CRATES_SECRET)
DERIVED = (base64.b64encode(f"__token__:{PYPI_SECRET}".encode()).decode(),)

RELEASE = "2.0.0"
PUBLISHED = "1.9.0"
CORE_CRATES = ("meerkat-core", "meerkat-store", "rkat")
META_CRATES = ("meta-extension-state", "meerkat-meta")


# ---------------------------------------------------------------------------
# Published artifacts.


def _targz(files: dict[str, bytes]) -> bytes:
    buffer = io.BytesIO()
    with tarfile.open(fileobj=buffer, mode="w:gz") as archive:
        for name, payload in files.items():
            info = tarfile.TarInfo(name)
            info.size = len(payload)
            info.mtime = 0
            archive.addfile(info, io.BytesIO(payload))
    return buffer.getvalue()


def npm_tarball(name: str, version: str, manifest_version: str | None = None) -> bytes:
    manifest = {"name": name, "version": manifest_version or version, "description": "fixture"}
    return _targz({"package/package.json": json.dumps(manifest).encode()})


def wheel(name: str, version: str) -> tuple[str, bytes]:
    dist = name.replace("-", "_")
    buffer = io.BytesIO()
    with zipfile.ZipFile(buffer, "w") as archive:
        archive.writestr(f"{dist}-{version}.dist-info/METADATA",
                         f"Metadata-Version: 2.1\nName: {name}\nVersion: {version}\n")
        archive.writestr(f"{dist}/__init__.py", "")
    return f"{dist}-{version}-py3-none-any.whl", buffer.getvalue()


def crate_file(name: str, version: str) -> bytes:
    manifest = f'[package]\nname = "{name}"\nversion = "{version}"\ndescription = "fixture"\n'
    return _targz({f"{name}-{version}/Cargo.toml": manifest.encode()})


def sri(payload: bytes) -> str:
    return "sha512-" + base64.b64encode(hashlib.sha512(payload).digest()).decode()


# ---------------------------------------------------------------------------
# The registry stub.


class Registry:
    """npm, PyPI's index and crates.io behind one loopback HTTP server."""

    def __init__(self) -> None:
        self.lock = threading.Lock()
        self.log: list[dict] = []
        self.writes: list[dict] = []
        self.violations: list[str] = []
        self.whoami = "ok"
        self.npm: dict[str, dict] = {}
        self.pypi: dict[str, dict] = {}
        self.crates: dict[str, dict] = {}
        self.autocreate: tuple[str, ...] = ()
        server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), _Handler)
        server.registry = self  # type: ignore[attr-defined]
        self.server = server
        self.base = f"http://127.0.0.1:{server.server_address[1]}"
        self.thread = threading.Thread(target=server.serve_forever, daemon=True)
        self.thread.start()

    def close(self) -> None:
        self.server.shutdown()
        self.server.server_close()

    # ---- fixtures

    def add_npm(self, name: str, versions=(PUBLISHED, RELEASE), *, token="duplicate", anon="unauthorized",
                corrupt=(), manifest_version=None) -> None:
        self.npm[name] = {
            "versions": {v: npm_tarball(name, v) for v in versions},
            "served": {v: npm_tarball(name, v, manifest_version) for v in versions},
            "token": token, "anon": anon, "corrupt": set(corrupt),
        }

    def add_pypi(self, name: str, versions=(PUBLISHED, RELEASE), *, corrupt=()) -> None:
        files = dict(wheel(name, v) for v in versions)
        self.pypi[name] = {"files": files, "corrupt": set(corrupt)}

    def add_crate(self, name: str, versions=(PUBLISHED, RELEASE), *, token=("duplicate",), anon="unauthenticated",
                  owners=(("user", 1),), trustpub_only=False, corrupt=(), yanked=()) -> None:
        self.crates[name] = {
            "versions": {v: crate_file(name, v) for v in versions},
            "token": list(token), "anon": anon, "owners": [tuple(owner) for owner in owners],
            "trustpub_only": trustpub_only, "corrupt": set(corrupt), "yanked": set(yanked),
        }

    def add_product(self, product: str, *, versions=(PUBLISHED, RELEASE), crates=None) -> None:
        if product == "core":
            for name in ("@rkat/sdk", "@rkat/web"):
                self.add_npm(name, versions)
            self.add_pypi("meerkat-sdk", versions)
            for crate in crates or CORE_CRATES:
                self.add_crate(crate, versions)
        else:
            self.add_npm("@rkat/meta-sdk", versions)
            self.add_pypi("meerkat-meta", versions)
            for crate in crates or META_CRATES:
                self.add_crate(crate, versions)

    # ---- request handling

    def handle(self, request: http.server.BaseHTTPRequestHandler, method: str) -> None:
        length = int(request.headers.get("Content-Length") or 0)
        body = request.rfile.read(length) if length else b""
        path = urllib.parse.urlsplit(request.path).path
        authorization = request.headers.get("Authorization")
        with self.lock:
            status, headers, payload = self.route(method, path, authorization, body)
            self.log.append({
                "method": method,
                "path": path,
                "auth": self._auth_kind(path, authorization),
                "status": status,
            })
        request.send_response(status)
        for key, value in headers.items():
            request.send_header(key, value)
        request.send_header("Content-Length", str(len(payload)))
        request.end_headers()
        request.wfile.write(payload)

    @staticmethod
    def _auth_kind(path: str, authorization: str | None) -> str:
        if authorization is None:
            return "none"
        if authorization in (f"Bearer {NPM_SECRET}", CRATES_SECRET):
            return "token"
        return "other"

    def route(self, method: str, path: str, authorization: str | None, body: bytes):
        def as_json(status: int, document, headers=None):
            return status, {"Content-Type": "application/json", **(headers or {})}, json.dumps(document).encode()

        if path == "/npm/-/whoami" and method == "GET":
            if self.whoami == "unavailable":
                return 503, {}, b"Service Unavailable"
            if self.whoami == "redirect":
                return 302, {"Location": f"{self.base}/elsewhere"}, b""
            if authorization == f"Bearer {NPM_SECRET}" and self.whoami == "ok":
                return as_json(200, {"username": "fixture-account"})
            return as_json(401, {"error": "Unauthorized"})
        if path.startswith("/npm/") and method in ("GET", "PUT"):
            name = urllib.parse.unquote(path[len("/npm/"):])
            package = self._npm(name)
            if package is None:
                return as_json(404, {"error": "Not found"})
            if method == "GET":
                return as_json(200, {
                    "name": name,
                    "dist-tags": {"latest": max(package["versions"], key=_key)},
                    "versions": {
                        v: {"name": name, "version": v, "dist": {
                            "integrity": sri(data),
                            "shasum": hashlib.sha1(data).hexdigest(),
                            "tarball": f"{self.base}/npm-files/{urllib.parse.quote(name, safe='')}/{v}.tgz",
                        }}
                        for v, data in package["served"].items()
                    },
                })
            return self.npm_put(name, package, authorization, body)
        if path.startswith("/npm-files/") and method == "GET":
            quoted, file = path[len("/npm-files/"):].rsplit("/", 1)
            package = self._npm(urllib.parse.unquote(quoted))
            version = file[: -len(".tgz")]
            if package is None or version not in package["versions"]:
                return 404, {}, b"not found"
            data = package["served"][version]
            return 200, {}, data + b"corrupt" if version in package["corrupt"] else data
        if path.startswith("/pypi/simple/") and method == "GET":
            name = path[len("/pypi/simple/"):].strip("/")
            project = self._pypi(name)
            if project is None:
                return as_json(404, {"message": "Not Found"})
            return as_json(200, {
                "meta": {"api-version": "1.1"},
                "name": name,
                "files": [
                    {"filename": file, "url": f"{self.base}/pypi-files/{file}",
                     "hashes": {"sha256": hashlib.sha256(data).hexdigest()}, "yanked": False}
                    for file, data in project["files"].items()
                ],
            })
        if path.startswith("/pypi-files/") and method == "GET":
            file = path[len("/pypi-files/"):]
            for project in self.pypi.values():
                if file in project["files"]:
                    data = project["files"][file]
                    version = file.split("-")[1]
                    return 200, {}, data + b"corrupt" if version in project["corrupt"] else data
            return 404, {}, b"not found"
        if path == "/crates/api/v1/crates/new" and method == "PUT":
            return self.crates_put(authorization, body)
        match = re.fullmatch(r"/crates/api/v1/crates/([^/]+)(/owners)?", path)
        if match and method == "GET":
            crate = self._crate(match.group(1))
            if crate is None:
                return as_json(404, {"errors": [{"detail": f"crate `{match.group(1)}` does not exist"}]})
            if match.group(2):
                return as_json(200, {"users": [{"id": i, "kind": k, "login": "fixture"} for k, i in crate["owners"]]})
            return as_json(200, {
                "crate": {"name": match.group(1), "trustpub_only": crate["trustpub_only"]},
                "versions": [
                    {"num": v, "yanked": v in crate["yanked"], "checksum": hashlib.sha256(data).hexdigest(),
                     "crate": match.group(1)}
                    for v, data in crate["versions"].items()
                ],
            })
        match = re.fullmatch(r"/static/crates/([^/]+)/([^/]+)\.crate", path)
        if match and method == "GET":
            crate = self._crate(match.group(1))
            version = match.group(2)[len(match.group(1)) + 1:]
            if crate is None or version not in crate["versions"]:
                return 404, {}, b"not found"
            data = crate["versions"][version]
            return 200, {}, data + b"corrupt" if version in crate["corrupt"] else data
        return 404, {}, b"no such stub route"

    def _npm(self, name: str):
        if name not in self.npm and self.autocreate:
            self.add_npm(name, self.autocreate)
        return self.npm.get(name)

    def _pypi(self, normalized: str):
        for name, project in self.pypi.items():
            if re.sub(r"[-_.]+", "-", name).lower() == normalized:
                return project
        if self.autocreate:
            self.add_pypi(normalized, self.autocreate)
            return self.pypi[normalized]
        return None

    def _crate(self, name: str):
        if name not in self.crates and self.autocreate:
            self.add_crate(name, self.autocreate)
        return self.crates.get(name)

    def npm_put(self, name: str, package: dict, authorization: str | None, body: bytes):
        try:
            document = json.loads(body)
            (version,) = document["versions"]
            (attachment,) = document["_attachments"].values()
            sent = base64.b64decode(attachment["data"])
        except (ValueError, KeyError, TypeError):
            self.violations.append(f"npm PUT for {name} is not a publish document")
            return 400, {}, b"bad document"
        if sent != package["served"].get(version):
            self.violations.append(f"npm PUT for {name} {version} does not re-send the published tarball")
        if document.get("dist-tags") != {"registry-preflight": version}:
            self.violations.append(f"npm PUT for {name} moves a dist-tag other than registry-preflight")
        kind = "token" if authorization == f"Bearer {NPM_SECRET}" else ("anon" if authorization is None else "wrong")
        self.writes.append({"registry": "npm", "package": name, "version": version, "auth": kind,
                            "sha256": hashlib.sha256(sent).hexdigest()})
        outcome = package["token"] if kind == "token" else package["anon"]
        return self.npm_answer(outcome, version, authorization)

    @staticmethod
    def npm_answer(outcome: str, version: str, authorization: str | None):
        def as_json(status, document, headers=None):
            return status, {"Content-Type": "application/json", **(headers or {})}, json.dumps(document).encode()

        if outcome == "duplicate":
            return as_json(403, {"error": f"You cannot publish over the previously published versions: {version}."})
        if outcome == "not_found":
            return as_json(404, {"error": "Not found"})
        if outcome == "unauthorized":
            return as_json(401, {"error": "You must be logged in to publish packages."})
        if outcome == "otp":
            return as_json(401, {"error": "This operation requires a one-time password."}, {"WWW-Authenticate": "OTP"})
        if outcome == "forbidden":
            return as_json(403, {"error": "Two-factor authentication or granular access token with bypass 2fa enabled is required to publish packages."})
        if outcome == "echo":
            return as_json(403, {"error": f"refused the credential {authorization}"})
        if outcome == "accept":
            return as_json(201, {"ok": True})
        if outcome == "conflict":
            return as_json(409, {"error": "Document update conflict."})
        return 503, {}, b"Service Unavailable"

    def crates_put(self, authorization: str | None, body: bytes):
        def as_json(status, document, headers=None):
            return status, {"Content-Type": "application/json", **(headers or {})}, json.dumps(document).encode()

        try:
            (json_length,) = struct.unpack("<I", body[:4])
            metadata = json.loads(body[4: 4 + json_length])
            (crate_length,) = struct.unpack("<I", body[4 + json_length: 8 + json_length])
            sent = body[8 + json_length:]
            assert len(sent) == crate_length
            name, version = metadata["name"], metadata["vers"]
        except (ValueError, KeyError, AssertionError, struct.error):
            self.violations.append("crates.io PUT body is not cargo's publish body")
            return as_json(400, {"errors": [{"detail": "invalid upload request"}]})
        crate = self.crates.get(name)
        if crate is None or sent != crate["versions"].get(version):
            self.violations.append(f"crates.io PUT for {name} {version} does not re-send the published .crate")
        kind = "token" if authorization == CRATES_SECRET else ("anon" if authorization is None else "wrong")
        self.writes.append({"registry": "crates", "package": name, "version": version, "auth": kind,
                            "sha256": hashlib.sha256(sent).hexdigest()})
        if kind != "token":
            outcome = crate["anon"] if crate else "unauthenticated"
        else:
            outcome = crate["token"].pop(0) if len(crate["token"]) > 1 else crate["token"][0]
        if outcome == "duplicate":
            return as_json(400, {"errors": [{"detail": f"crate version `{version}` is already uploaded"}]})
        if outcome == "forbidden":
            return as_json(403, {"errors": [{"detail": "this token does not have the required permissions to perform this action"}]})
        if outcome == "unauthenticated":
            return as_json(403, {"errors": [{"detail": "this action requires authentication"}]})
        if outcome == "ratelimit":
            retry = email.utils.formatdate(time.time() + 1, usegmt=True)
            return as_json(429, {"errors": [{"detail": f"You have published too many updates to existing crates in a short period of time. Please try again after {retry} and see https://crates.io/docs/rate-limits for more details."}]},
                           {"Retry-After": retry})
        if outcome == "bad_body":
            return as_json(400, {"errors": [{"detail": "invalid upload request: missing field `deps`"}]})
        if outcome == "echo":
            return as_json(403, {"errors": [{"detail": f"refused the credential {authorization}"}]})
        if outcome == "redirect":
            return 307, {"Location": f"{self.base}/elsewhere"}, b""
        if outcome == "accept":
            return as_json(200, {"warnings": {}})
        return 503, {}, b"Service Unavailable"


def _key(version: str):
    return tuple(int(part) for part in version.split("."))


class _Handler(http.server.BaseHTTPRequestHandler):
    def log_message(self, *args) -> None:  # noqa: D401
        return

    def do_GET(self) -> None:  # noqa: N802
        self.server.registry.handle(self, "GET")  # type: ignore[attr-defined]

    def do_PUT(self) -> None:  # noqa: N802
        self.server.registry.handle(self, "PUT")  # type: ignore[attr-defined]

    def do_POST(self) -> None:  # noqa: N802
        self.server.registry.handle(self, "POST")  # type: ignore[attr-defined]


# The fake twine: answers per wheel and per credential (the real token,
# recognized by its sha256, or anything else: the control), logs each call.
FAKE_TWINE = """#!{python} -I
import hashlib, json, os, sys
args = sys.argv[1:]
behavior = json.load(open(os.environ["FAKE_TWINE_BEHAVIOR"]))
password = os.environ.get("TWINE_PASSWORD", "")
token = hashlib.sha256(password.encode()).hexdigest() == os.environ["FAKE_TWINE_TOKEN_SHA256"]
credential = "token" if token else "control"
wheel = args[-1]
url = args[args.index("--repository-url") + 1]
name = os.path.basename(wheel)
with open(wheel, "rb") as handle:
    digest = hashlib.sha256(handle.read()).hexdigest()
entry = behavior.get(name, {{}})
outcome = entry.get(credential, "ok" if token else "invalid")
with open(os.environ["FAKE_TWINE_LOG"], "a") as log:
    log.write(json.dumps({{"argv": args, "credential": credential, "wheel": name, "sha256": digest,
                          "username": os.environ.get("TWINE_USERNAME"),
                          "secrets_in_env": sorted(k for k in ("PYPI_API_TOKEN", "NPM_TOKEN", "CARGO_REGISTRY_TOKEN") if k in os.environ)}}) + "\\n")
print("Uploading distributions to " + url)
print("INFO     Uploading " + name)
if outcome == "ok":
    print("INFO     Response from " + url + ":")
    print("         200 OK")
    sys.exit(0)
lines = {{
    "forbidden": ("403 Forbidden", "The given token isn't allowed to upload to project 'x'. See https://pypi.org/help/#invalid-auth for more information."),
    "invalid": ("403 Forbidden", "Invalid or non-existent authentication information. See https://pypi.org/help/#invalid-auth for more information."),
    "exists": ("400 Bad Request", "File already exists ('" + name + "', with blake2_256 hash 'abc'). See https://pypi.org/help/#file-name-reuse for more information."),
    "unavailable": ("503 Service Unavailable", "Service Unavailable"),
    "echo": ("403 Forbidden", "refused the password " + password),
}}
status, reason = lines[outcome]
sys.stderr.write("ERROR    HTTPError: " + status + " from " + url + "\\n         " + reason + "\\n")
sys.exit(1)
"""


def make_repo(root: Path, *, version: str = RELEASE, core_crates=CORE_CRATES) -> Path:
    (root / "ci").mkdir(parents=True)
    (root / "ci" / "products.json").write_text(json.dumps({"schema_version": 1, "products": [
        {"name": "core", "root": "", "publish_order": "scripts/release-rust-crates.sh"},
        {"name": "meta", "root": "meta", "publish_order": list(META_CRATES)},
        {"name": "connect", "root": "connect", "publish_order": []},
    ]}))
    (root / "scripts").mkdir()
    (root / "scripts" / "release-rust-crates.sh").write_text(
        "#!/usr/bin/env bash\nset -euo pipefail\nprintf '%s\\n' " + " ".join(core_crates) + "\n")
    (root / "Cargo.toml").write_text(f'[workspace.package]\nversion = "{version}"\n')
    for crate in META_CRATES:
        directory = root / "meta" / "crates" / crate
        directory.mkdir(parents=True)
        (directory / "Cargo.toml").write_text(f'[package]\nname = "{crate}"\nversion = "{version}"\n')
    for relative, name in (("sdks/python", "meerkat-sdk"), ("meta/sdk/python", "meerkat-meta")):
        (root / relative).mkdir(parents=True)
        (root / relative / "pyproject.toml").write_text(f'[project]\nname = "{name}"\nversion = "{version}"\n')
    for relative, name in (("sdks/typescript", "@rkat/sdk"), ("sdks/web", "@rkat/web"),
                           ("meta/sdk/typescript", "@rkat/meta-sdk")):
        (root / relative).mkdir(parents=True)
        (root / relative / "package.json").write_text(json.dumps({"name": name, "version": version}))
    return root


class Harness(unittest.TestCase):
    """A fixture repository, the registry stub and the fake twine per test."""

    def setUp(self) -> None:
        self.tmp = Path(tempfile.mkdtemp(prefix="registry-preflight-test-"))
        self.repo = make_repo(self.tmp / "repo")
        self.registry = Registry()
        self.addCleanup(self.registry.close)
        self.bin = self.tmp / "bin"
        self.bin.mkdir()
        twine = self.bin / "twine"
        twine.write_text(FAKE_TWINE.format(python=sys.executable))
        twine.chmod(0o755)
        self.twine_log = self.tmp / "twine.log"
        self.twine_behavior: dict[str, dict] = {}
        self.report = self.tmp / "report.json"
        self.summary = self.tmp / "summary.md"

    def tearDown(self) -> None:
        import shutil

        shutil.rmtree(self.tmp, ignore_errors=True)

    def run_preflight(self, *args: str, tokens: bool = True, env: dict | None = None, repo: Path | None = None):
        behavior = self.tmp / "twine-behavior.json"
        behavior.write_text(json.dumps(self.twine_behavior))
        environment = {
            "PATH": f"{self.bin}{os.pathsep}{os.environ.get('PATH', '/usr/bin:/bin')}",
            "HOME": str(self.tmp),
            "LC_ALL": "C.UTF-8",
            preflight.STUB_ENV: self.registry.base,
            "FAKE_TWINE_BEHAVIOR": str(behavior),
            "FAKE_TWINE_LOG": str(self.twine_log),
            "FAKE_TWINE_TOKEN_SHA256": hashlib.sha256(PYPI_SECRET.encode()).hexdigest(),
            "GITHUB_STEP_SUMMARY": str(self.summary),
        }
        if tokens:
            environment.update(NPM_TOKEN=NPM_SECRET, PYPI_API_TOKEN=PYPI_SECRET, CARGO_REGISTRY_TOKEN=CRATES_SECRET)
        environment.update(env or {})
        if self.report.exists():
            self.report.unlink()
        command = [sys.executable, "-I", str(SCRIPT), "--repo-root", str(repo or self.repo), "--retry-delay", "0",
                   "--report", str(self.report), *args]
        result = subprocess.run(command, env=environment, capture_output=True, text=True, timeout=180, check=False)
        self.output = result.stdout + result.stderr
        self.assertNotLeaked(self.output)
        if self.report.exists():
            self.assertNotLeaked(self.report.read_text())
        if self.summary.exists():
            self.assertNotLeaked(self.summary.read_text())
        self.assertEqual(self.registry.violations, [], self.output)
        return result

    def assertNotLeaked(self, text: str) -> None:  # noqa: N802
        for secret in SECRETS + DERIVED:
            self.assertNotIn(secret, text)

    def rows(self) -> dict[tuple[str, str], dict]:
        report = json.loads(self.report.read_text())
        return {(row["registry"], row["package"]): row for row in report["packages"]}

    def twine_calls(self) -> list[dict]:
        if not self.twine_log.exists():
            return []
        return [json.loads(line) for line in self.twine_log.read_text().splitlines() if line.strip()]

    def token_writes(self, registry: str | None = None) -> list[dict]:
        return [w for w in self.registry.writes if w["auth"] == "token" and (registry is None or w["registry"] == registry)]


# ---------------------------------------------------------------------------
# (a) Only an already published version, never the release version.


class RuleAPublishedVersionOnly(Harness):
    def test_a_probes_only_a_published_version_never_the_release_version(self) -> None:
        self.registry.add_product("core")
        result = self.run_preflight("--product", "core", "--mode", "probe")
        self.assertEqual(result.returncode, 0, self.output)
        writes = self.registry.writes
        self.assertTrue(writes)
        self.assertEqual({w["version"] for w in writes}, {PUBLISHED})
        for write in writes:
            published = (npm_tarball(write["package"], PUBLISHED) if write["registry"] == "npm"
                         else crate_file(write["package"], PUBLISHED))
            self.assertEqual(write["sha256"], hashlib.sha256(published).hexdigest())
        calls = self.twine_calls()
        self.assertEqual([c["wheel"] for c in calls], [wheel("meerkat-sdk", PUBLISHED)[0]] * 2)
        self.assertEqual({c["sha256"] for c in calls}, {hashlib.sha256(wheel("meerkat-sdk", PUBLISHED)[1]).hexdigest()})
        rows = self.rows()
        for key in (("npm", "@rkat/sdk"), ("npm", "@rkat/web"), ("pypi", "meerkat-sdk"),
                    ("crates", "meerkat-core"), ("crates", "rkat")):
            self.assertEqual((rows[key]["verdict"], rows[key]["version"]), ("PASS", PUBLISHED), key)
        self.assertEqual(rows[("crates", "meerkat-store")]["verdict"], "SKIP")
        self.assertIn(RELEASE, json.loads(self.report.read_text())["never_probed_versions"])

    def test_a_a_package_with_nothing_but_the_release_version_fails_and_sends_nothing(self) -> None:
        self.registry.add_product("core")
        result = self.run_preflight("--product", "core", "--mode", "probe", "--release-version", f"v{PUBLISHED}")
        self.assertEqual(result.returncode, 1, self.output)
        self.assertEqual(self.registry.writes, [])
        self.assertEqual(self.twine_calls(), [])
        rows = self.rows()
        for key in (("npm", "@rkat/sdk"), ("pypi", "meerkat-sdk"), ("crates", "meerkat-core"), ("crates", "rkat")):
            self.assertEqual(rows[key]["verdict"], "FAIL", key)
            self.assertIn("no published final version other than the release version", rows[key]["detail"])

    def test_a_an_artifact_that_does_not_match_the_registry_digest_is_never_sent(self) -> None:
        self.registry.add_product("core")
        self.registry.add_npm("@rkat/sdk", corrupt={PUBLISHED})
        self.registry.add_pypi("meerkat-sdk", corrupt={PUBLISHED})
        self.registry.add_crate("rkat", corrupt={PUBLISHED})
        self.registry.add_npm("@rkat/web", manifest_version="9.9.9")
        result = self.run_preflight("--product", "core", "--mode", "probe")
        self.assertEqual(result.returncode, 1, self.output)
        sent = {(w["registry"], w["package"]) for w in self.registry.writes}
        self.assertNotIn(("npm", "@rkat/sdk"), sent)
        self.assertNotIn(("npm", "@rkat/web"), sent)
        self.assertNotIn(("crates", "rkat"), sent)
        self.assertEqual(self.twine_calls(), [])
        rows = self.rows()
        self.assertIn("does not match the registry's sha512 integrity", rows[("npm", "@rkat/sdk")]["detail"])
        self.assertIn("package.json does not name this package and version", rows[("npm", "@rkat/web")]["detail"])
        self.assertIn("does not match PyPI's sha256", rows[("pypi", "meerkat-sdk")]["detail"])
        self.assertIn("does not match the crates.io checksum", rows[("crates", "rkat")]["detail"])
        self.assertEqual(rows[("crates", "meerkat-core")]["verdict"], "PASS")

    def test_a_a_crate_missing_from_crates_io_fails_as_a_first_publication(self) -> None:
        self.registry.add_product("core", crates=("meerkat-core", "rkat"))
        result = self.run_preflight("--product", "core", "--mode", "probe", "--registry", "crates")
        self.assertEqual(result.returncode, 1, self.output)
        row = self.rows()[("crates", "meerkat-store")]
        self.assertEqual((row["verdict"], row["status"]), ("FAIL", 404))
        self.assertIn("first publication needs the publish-new scope", row["detail"])
        self.assertNotIn("meerkat-store", {w["package"] for w in self.registry.writes})

    def test_a_a_yanked_or_pre_release_version_is_never_chosen(self) -> None:
        self.assertEqual(preflight.choose_version(["1.0.0", "1.1.0-rc.1", "0.9.0", "2.0.0"], {"2.0.0"}), "1.0.0")
        self.assertEqual(preflight.choose_version(["0.9.10", "0.9.9"], set()), "0.9.10")
        self.assertIsNone(preflight.choose_version(["2.0.0"], {"2.0.0"}))
        self.registry.add_product("meta")
        self.registry.add_crate("meerkat-meta", versions=("1.8.0", PUBLISHED, RELEASE), yanked={PUBLISHED})
        result = self.run_preflight("--product", "meta", "--mode", "probe", "--registry", "crates")
        self.assertEqual(result.returncode, 0, self.output)
        self.assertEqual(self.rows()[("crates", "meerkat-meta")]["version"], "1.8.0")


# ---------------------------------------------------------------------------
# (b) The default mode is a plan.


class RuleBPlanByDefault(Harness):
    def test_b_the_default_mode_is_a_plan_that_writes_nothing_and_reads_no_token(self) -> None:
        self.registry.add_product("core")
        for tokens in (True, False):
            with self.subTest(tokens=tokens):
                result = self.run_preflight("--product", "core", tokens=tokens)
                self.assertEqual(result.returncode, 0, self.output)
                self.assertEqual(self.registry.writes, [])
                self.assertEqual([entry for entry in self.registry.log if entry["method"] != "GET"], [])
                self.assertEqual([entry for entry in self.registry.log if entry["auth"] != "none"], [])
                self.assertEqual(self.twine_calls(), [])
                rows = self.rows()
                self.assertEqual({row["verdict"] for row in rows.values()}, {"SKIP"})
                self.assertIn("plan: would send PUT", rows[("npm", "@rkat/sdk")]["detail"])
                self.assertIn("plan: would run twine upload", rows[("pypi", "meerkat-sdk")]["detail"])
                self.assertIn("plan: would send PUT", rows[("crates", "rkat")]["detail"])
                self.assertEqual(json.loads(self.report.read_text())["mode"], "plan")
                self.assertIn("plan mode: every read and check, no write, no token", self.output)


# ---------------------------------------------------------------------------
# (c) crates.io: the fixed sample at release time, the full probe paced and early.


class RuleCCratesRateLimit(Harness):
    def test_c_family_mode_probes_only_the_fixed_sample(self) -> None:
        self.registry.add_product("core")
        self.registry.add_product("meta")
        result = self.run_preflight("--product", "core", "--mode", "probe", "--registry", "crates")
        self.assertEqual(result.returncode, 0, self.output)
        self.assertEqual(sorted(w["package"] for w in self.token_writes("crates")), ["meerkat-core", "rkat"])
        self.assertEqual(self.rows()[("crates", "meerkat-store")]["verdict"], "SKIP")
        self.assertEqual(json.loads(self.report.read_text())["crates_sample"], ["meerkat-core", "rkat"])
        self.registry.writes.clear()
        result = self.run_preflight("--product", "meta", "--mode", "probe", "--registry", "crates")
        self.assertEqual(result.returncode, 0, self.output)
        self.assertEqual(sorted(w["package"] for w in self.token_writes("crates")), sorted(META_CRATES))
        self.assertEqual(preflight.PRODUCTS["core"].family_sample, ("meerkat-core", "rkat"))
        self.assertEqual(preflight.PRODUCTS["meta"].family_sample, ("meta-extension-state", "meerkat-meta"))

    def test_c_family_mode_fails_a_crate_the_sample_cannot_vouch_for(self) -> None:
        self.registry.add_product("core", crates=("meerkat-core", "rkat"))
        self.registry.add_crate("meerkat-store", owners=(("user", 2),))
        result = self.run_preflight("--product", "core", "--mode", "probe", "--registry", "crates")
        self.assertEqual(result.returncode, 1, self.output)
        self.assertIn("owners do not include every owner of the probed crates", self.rows()[("crates", "meerkat-store")]["detail"])
        self.registry.add_crate("meerkat-store", trustpub_only=True)
        result = self.run_preflight("--product", "core", "--mode", "probe", "--registry", "crates")
        self.assertEqual(result.returncode, 1, self.output)
        self.assertIn("trusted-publishing-only", self.rows()[("crates", "meerkat-store")]["detail"])
        self.registry.add_crate("meerkat-store", owners=(("user", 1), ("team", 7)))
        result = self.run_preflight("--product", "core", "--mode", "probe", "--registry", "crates")
        self.assertEqual(result.returncode, 0, self.output)

    def test_c_the_full_probe_is_refused_inside_a_release_workflow(self) -> None:
        self.registry.add_product("core")
        for workflow in ("release.yml", "meta-release.yml"):
            with self.subTest(workflow=workflow):
                ref = f"example/meerkat/.github/workflows/{workflow}@refs/heads/main"
                result = self.run_preflight("--product", "core", "--mode", "probe", "--crates", "all",
                                            env={"GITHUB_WORKFLOW_REF": ref})
                self.assertEqual(result.returncode, 2, self.output)
                self.assertIn("never inside a release workflow", self.output)
                self.assertEqual(self.registry.log, [])
        ref = "example/meerkat/.github/workflows/registry-preflight.yml@refs/heads/main"
        result = self.run_preflight("--product", "core", "--mode", "probe", "--crates", "all", "--registry", "crates",
                                    env={"GITHUB_WORKFLOW_REF": ref})
        self.assertEqual(result.returncode, 0, self.output)
        self.assertEqual(sorted(w["package"] for w in self.token_writes("crates")), sorted(CORE_CRATES))

    def test_c_the_full_probe_is_paced_to_the_publish_rate_limit(self) -> None:
        delays = preflight.probe_delays(49, 20, 61)
        self.assertEqual(delays[:20], [0.0] * 20)
        self.assertEqual(delays[20:], [61.0] * 29)
        self.assertEqual(preflight.probe_delays(2, 2, 61), [0.0, 0.0])
        for flags in (("--crates-burst", "31"), ("--crates-burst", "0"), ("--crates-interval", "59")):
            with self.subTest(flags=flags):
                result = self.run_preflight("--product", "core", "--crates", "all", *flags)
                self.assertEqual(result.returncode, 2, self.output)
                self.assertEqual(self.registry.log, [])

    def test_c_a_rate_limited_probe_waits_for_the_advertised_time_and_retries_once(self) -> None:
        self.registry.add_product("meta")
        self.registry.add_crate("meerkat-meta", token=("ratelimit", "duplicate"))
        result = self.run_preflight("--product", "meta", "--mode", "probe", "--registry", "crates")
        self.assertEqual(result.returncode, 0, self.output)
        self.assertEqual(len([w for w in self.token_writes() if w["package"] == "meerkat-meta"]), 2)
        self.assertIn("one retry in", self.output)
        self.registry.writes.clear()
        self.registry.add_crate("meerkat-meta", token=("ratelimit",))
        result = self.run_preflight("--product", "meta", "--mode", "probe", "--registry", "crates")
        self.assertEqual(result.returncode, 1, self.output)
        self.assertEqual(len([w for w in self.token_writes() if w["package"] == "meerkat-meta"]), 2)
        row = self.rows()[("crates", "meerkat-meta")]
        self.assertEqual((row["verdict"], row["status"]), ("INCONCLUSIVE", 429))
        self.assertIn("too many updates to existing crates", row["answer"])


# ---------------------------------------------------------------------------
# (d) A verdict and the registry's exact answer per package; no token anywhere.


class RuleDVerdictsAndAnswers(Harness):
    def test_d_every_package_gets_a_verdict_and_the_registry_answer(self) -> None:
        self.registry.add_product("core")
        self.registry.add_npm("@rkat/web", token="not_found")
        self.registry.add_crate("rkat", token=("forbidden",))
        self.twine_behavior = {wheel("meerkat-sdk", PUBLISHED)[0]: {"token": "forbidden"}}
        result = self.run_preflight("--product", "core", "--mode", "probe")
        self.assertEqual(result.returncode, 1, self.output)
        rows = self.rows()
        expected = {
            ("npm", "@rkat/sdk"): ("PASS", 403, f"You cannot publish over the previously published versions: {PUBLISHED}."),
            ("npm", "@rkat/web"): ("FAIL", 404, "Not found"),
            ("pypi", "meerkat-sdk"): ("FAIL", 403, "isn't allowed to upload to project"),
            ("crates", "meerkat-core"): ("PASS", 400, f"crate version `{PUBLISHED}` is already uploaded"),
            ("crates", "rkat"): ("FAIL", 403, "this token does not have the required permissions to perform this action"),
            ("crates", "meerkat-store"): ("SKIP", None, ""),
        }
        self.assertEqual(set(rows), set(expected))
        for key, (verdict, status, answer) in expected.items():
            with self.subTest(package=key):
                self.assertEqual((rows[key]["verdict"], rows[key]["status"]), (verdict, status))
                self.assertIn(answer, rows[key]["answer"])
                self.assertTrue(rows[key]["detail"])
        self.assertIn(f'PASS         npm    @rkat/sdk {PUBLISHED}: HTTP 403 "You cannot publish over', self.output)
        self.assertIn(f'FAIL         npm    @rkat/web {PUBLISHED}: HTTP 404 "Not found"', self.output)
        self.assertRegex(self.output, r"registry pre-flight core: 2 PASS, 3 FAIL, 0 INCONCLUSIVE, 1 SKIP: NOT OK")
        report = json.loads(self.report.read_text())
        self.assertFalse(report["ok"])
        self.assertEqual(report["counts"], {"PASS": 2, "FAIL": 3, "INCONCLUSIVE": 0, "SKIP": 1})
        summary = self.summary.read_text()
        self.assertIn("| FAIL | npm | @rkat/web |", summary)
        self.assertIn("| PASS | crates | meerkat-core |", summary)

    def test_d_no_token_or_token_derived_value_is_printed_or_written(self) -> None:
        self.registry.add_product("core")
        self.registry.add_npm("@rkat/sdk", token="echo")
        self.registry.add_crate("rkat", token=("echo",))
        self.twine_behavior = {wheel("meerkat-sdk", PUBLISHED)[0]: {"token": "echo"}}
        result = self.run_preflight("--product", "core", "--mode", "probe")
        self.assertEqual(result.returncode, 1, self.output)
        rows = self.rows()
        self.assertIn("refused the credential Bearer ***", rows[("npm", "@rkat/sdk")]["answer"])
        self.assertIn("refused the credential ***", rows[("crates", "rkat")]["answer"])
        self.assertIn("refused the password ***", rows[("pypi", "meerkat-sdk")]["answer"])
        for call in self.twine_calls():
            self.assertEqual(call["username"], "__token__")
            self.assertEqual(call["secrets_in_env"], [])
            self.assertNotLeaked(json.dumps(call["argv"]))
        # A token goes only with a write or whoami, never with an anonymous read.
        for entry in self.registry.log:
            if entry["auth"] != "none":
                self.assertTrue(entry["method"] == "PUT" or entry["path"] == "/npm/-/whoami", entry)


# ---------------------------------------------------------------------------
# Every row of each classifier.


def response(status, body=b"", headers=None):
    if isinstance(body, (dict, list)):
        body = json.dumps(body).encode()
    return preflight.Response(status, {k.lower(): v for k, v in (headers or {}).items()}, body)


class ClassifierRows(unittest.TestCase):
    def test_npm_classifier_rows(self) -> None:
        rows = [
            (response(403, {"error": "You cannot publish over the previously published versions: 1.0.0."}), "PASS"),
            (response(404, {"error": "Not found"}), "FAIL"),
            (response(401, {"error": "Unauthorized"}), "FAIL"),
            (response(401, {"error": "otp required"}, {"WWW-Authenticate": "OTP"}), "FAIL"),
            (response(403, {"error": "Two-factor authentication is required to publish packages."}), "FAIL"),
            (response(201, {"ok": True}), "FAIL"),
            (response(409, {"error": "conflict"}), "INCONCLUSIVE"),
            (response(429, b"slow down"), "INCONCLUSIVE"),
            (response(503, b"unavailable"), "INCONCLUSIVE"),
            (preflight.Response(None, {}, b"", error="URLError: refused"), "INCONCLUSIVE"),
        ]
        for answer, verdict in rows:
            with self.subTest(status=answer.status, body=answer.body[:40]):
                self.assertEqual(preflight.classify_npm(answer)[0], verdict)
        self.assertIn("2FA", preflight.classify_npm(rows[3][0])[1])
        self.assertIn("INCIDENT", preflight.classify_npm(rows[5][0])[1])
        self.assertIn("0.9.9 refusal", preflight.classify_npm(rows[1][0])[1])

    def test_crates_classifier_rows(self) -> None:
        def errors(detail):
            return {"errors": [{"detail": detail}]}

        rows = [
            (response(400, errors("crate version `1.0.0` is already uploaded")), "PASS"),
            (response(400, errors("crate version `1.0.1` is already uploaded")), "INCONCLUSIVE"),
            (response(403, errors("this token does not have the required permissions to perform this action")), "FAIL"),
            (response(403, errors("this crate exists but you don't seem to be an owner.")), "FAIL"),
            (response(403, errors("New versions of this crate can only be published using Trusted Publishing")), "FAIL"),
            (response(401, errors("authentication failed")), "FAIL"),
            (response(429, errors("You have published too many updates")), "INCONCLUSIVE"),
            (response(400, errors("invalid upload request: missing field `deps`")), "INCONCLUSIVE"),
            (response(200, {"warnings": {}}), "FAIL"),
            (response(307, b"", {"Location": "https://elsewhere.example/"}), "INCONCLUSIVE"),
            (response(500, b"boom"), "INCONCLUSIVE"),
            (preflight.Response(None, {}, b"", error="TimeoutError"), "INCONCLUSIVE"),
        ]
        for answer, verdict in rows:
            with self.subTest(status=answer.status, body=answer.body[:60]):
                self.assertEqual(preflight.classify_crates(answer, "1.0.0")[0], verdict)
        self.assertIn("fix the probe body", preflight.classify_crates(rows[7][0], "1.0.0")[1])
        self.assertIn("INCIDENT", preflight.classify_crates(rows[8][0], "1.0.0")[1])

    def test_twine_classifier_rows(self) -> None:
        url = "https://upload.pypi.org/legacy/"
        rows = [
            (preflight.TwineResult(0, f"INFO     Response from {url}:\n         200 OK\n"), "PASS", 200),
            (preflight.TwineResult(1, f"ERROR    HTTPError: 403 Forbidden from {url}\n         The given token isn't allowed to upload to project 'x'.\n"), "FAIL", 403),
            (preflight.TwineResult(1, f"ERROR    HTTPError: 401 Unauthorized from {url}\n         Unauthorized\n"), "FAIL", 401),
            (preflight.TwineResult(1, f"ERROR    HTTPError: 400 Bad Request from {url}\n         File already exists ('x.whl').\n"), "INCONCLUSIVE", 400),
            (preflight.TwineResult(1, f"ERROR    HTTPError: 503 Service Unavailable from {url}\n         down\n"), "INCONCLUSIVE", 503),
            (preflight.TwineResult(1, "ERROR    InvalidDistribution: Metadata is missing required fields\n"), "INCONCLUSIVE", None),
            (preflight.TwineResult(None, "", error="TimeoutExpired"), "INCONCLUSIVE", None),
        ]
        for result, verdict, status in rows:
            with self.subTest(output=result.output[:60]):
                self.assertEqual(preflight.classify_twine(result)[0], verdict)
                self.assertEqual(preflight.twine_status(result), status)
        self.assertIn("isn't allowed", preflight.twine_answer(rows[1][0]))

    def test_retry_after_is_read_from_a_date_or_seconds_and_capped(self) -> None:
        self.assertEqual(preflight.retry_after_seconds(response(429, b"", {"Retry-After": "7"}), 61), 7)
        self.assertEqual(preflight.retry_after_seconds(response(429, b"", {"Retry-After": "99999"}), 61),
                         preflight.MAX_RETRY_AFTER)
        future = email.utils.formatdate(time.time() + 30, usegmt=True)
        self.assertTrue(25 <= preflight.retry_after_seconds(response(429, b"", {"Retry-After": future}), 61) <= 32)
        self.assertEqual(preflight.retry_after_seconds(response(429), 61), 61)


class NpmWhoamiAndControl(Harness):
    def test_npm_whoami_refusal_fails_every_npm_package_without_a_probe(self) -> None:
        self.registry.add_product("core")
        self.registry.whoami = "refused"
        result = self.run_preflight("--product", "core", "--mode", "probe", "--registry", "npm")
        self.assertEqual(result.returncode, 1, self.output)
        self.assertEqual(self.registry.writes, [])
        for name in ("@rkat/sdk", "@rkat/web"):
            row = self.rows()[("npm", name)]
            self.assertEqual((row["verdict"], row["status"]), ("FAIL", 401))
            self.assertIn("whoami refused the token", row["detail"])
        self.assertNotIn("fixture-account", self.output)

    def test_an_answer_given_without_credentials_proves_nothing(self) -> None:
        self.registry.add_product("meta")
        self.registry.add_npm("@rkat/meta-sdk", anon="duplicate")
        self.registry.add_crate("meerkat-meta", anon="duplicate")
        self.twine_behavior = {wheel("meerkat-meta", PUBLISHED)[0]: {"control": "ok"}}
        result = self.run_preflight("--product", "meta", "--mode", "probe")
        self.assertEqual(result.returncode, 1, self.output)
        rows = self.rows()
        for key in (("npm", "@rkat/meta-sdk"), ("crates", "meerkat-meta"), ("pypi", "meerkat-meta")):
            with self.subTest(package=key):
                self.assertEqual(rows[key]["verdict"], "INCONCLUSIVE")
                self.assertIn("proves nothing", rows[key]["detail"])
        sent = {(w["registry"], w["package"]) for w in self.token_writes()}
        self.assertNotIn(("npm", "@rkat/meta-sdk"), sent)
        self.assertNotIn(("crates", "meerkat-meta"), sent)
        self.assertEqual([c["credential"] for c in self.twine_calls()], ["control"])
        self.assertEqual(rows[("crates", "meta-extension-state")]["verdict"], "PASS")

    def test_the_npm_otp_and_2fa_refusals_fail(self) -> None:
        self.registry.add_product("core")
        self.registry.add_npm("@rkat/sdk", token="otp")
        self.registry.add_npm("@rkat/web", token="forbidden")
        result = self.run_preflight("--product", "core", "--mode", "probe", "--registry", "npm")
        self.assertEqual(result.returncode, 1, self.output)
        self.assertIn("one-time password", self.rows()[("npm", "@rkat/sdk")]["detail"])
        self.assertIn("2FA", self.rows()[("npm", "@rkat/web")]["detail"])


class EndpointGuard(Harness):
    def test_tokens_go_only_to_the_registries_or_a_loopback_stub(self) -> None:
        for stub in ("http://registry.example:8080", "ftp://127.0.0.1:21"):
            with self.subTest(stub=stub):
                result = self.run_preflight("--product", "core", "--mode", "probe", env={preflight.STUB_ENV: stub})
                self.assertEqual(result.returncode, 2, self.output)
                self.assertIn("loopback", self.output)
        self.assertEqual(self.registry.log, [])

    def test_a_request_carrying_a_token_never_follows_a_redirect(self) -> None:
        self.registry.add_product("meta")
        self.registry.whoami = "redirect"
        result = self.run_preflight("--product", "meta", "--mode", "probe", "--registry", "npm")
        self.assertEqual(result.returncode, 1, self.output)
        self.assertNotIn("/elsewhere", {entry["path"] for entry in self.registry.log})
        row = self.rows()[("npm", "@rkat/meta-sdk")]
        self.assertEqual((row["verdict"], row["status"]), ("INCONCLUSIVE", 302))
        self.assertEqual(self.registry.writes, [])
        self.registry.whoami = "ok"
        self.registry.add_crate("meerkat-meta", token=("redirect",))
        result = self.run_preflight("--product", "meta", "--mode", "probe", "--registry", "crates")
        self.assertEqual(result.returncode, 1, self.output)
        self.assertNotIn("/elsewhere", {entry["path"] for entry in self.registry.log})
        self.assertEqual(self.rows()[("crates", "meerkat-meta")]["status"], 307)

    def test_a_missing_secret_fails_its_registry_and_sends_nothing(self) -> None:
        self.registry.add_product("core")
        result = self.run_preflight("--product", "core", "--mode", "probe", tokens=False)
        self.assertEqual(result.returncode, 1, self.output)
        self.assertEqual(self.registry.log, [])
        for row in self.rows().values():
            self.assertEqual(row["verdict"], "FAIL")
            self.assertIn("secret is empty or unset", row["detail"])


# ---------------------------------------------------------------------------
# The names come from this repository; the workflow contract.


class Repository(Harness):
    def test_package_names_come_from_the_repository(self) -> None:
        self.registry.autocreate = ("0.0.1",)
        listed = subprocess.run(["bash", str(REPO_ROOT / "scripts" / "release-rust-crates.sh")], capture_output=True,
                                text=True, check=True).stdout.split()
        products = json.loads((REPO_ROOT / "ci" / "products.json").read_text())["products"]
        meta_order = next(p for p in products if p["name"] == "meta")["publish_order"]
        expected = {
            "core": {"crates": listed, "pypi": ["meerkat-sdk"], "npm": ["@rkat/sdk", "@rkat/web"]},
            "meta": {"crates": meta_order, "pypi": ["meerkat-meta"], "npm": ["@rkat/meta-sdk"]},
        }
        for product, names in expected.items():
            with self.subTest(product=product):
                result = self.run_preflight("--product", product, repo=REPO_ROOT, tokens=False)
                self.assertEqual(result.returncode, 0, self.output)
                packages = json.loads(self.report.read_text())["packages"]
                for registry in ("crates", "pypi", "npm"):
                    self.assertEqual([row["package"] for row in packages if row["registry"] == registry], names[registry])
                for crate in preflight.PRODUCTS[product].family_sample:
                    self.assertIn(crate, names["crates"])

    def test_a_stale_family_sample_is_refused_before_any_request(self) -> None:
        stale = make_repo(self.tmp / "stale", core_crates=("meerkat-core", "meerkat-store"))
        result = self.run_preflight("--product", "core", repo=stale)
        self.assertEqual(result.returncode, 2, self.output)
        self.assertIn("family sample names ['rkat']", self.output)
        self.assertEqual(self.registry.log, [])


def _job_block(text: str, job: str) -> str:
    match = re.search(rf"(?ms)^  {re.escape(job)}:\n(.*?)(?=^  [A-Za-z0-9_-]+:\n|\Z)", text)
    if match is None:
        raise AssertionError(f"workflow has no job {job}")
    return match.group(0)


class WorkflowContract(unittest.TestCase):
    def test_registry_preflight_workflow_is_dispatch_only_main_only_and_read_only(self) -> None:
        text = WORKFLOW.read_text()
        triggers = re.search(r"(?ms)^on:\n(.*?)(?=^\S)", text).group(1)
        self.assertEqual(re.findall(r"(?m)^  ([a-z_]+):", triggers), ["workflow_dispatch"])
        top = re.search(r"(?ms)^permissions:\n(.*?)(?=^\S)", text).group(1)
        self.assertEqual(top.split(), ["contents:", "read"])
        self.assertNotRegex(text, r"(?m)^[ \t]+permissions:")
        self.assertNotIn("secrets: inherit", text)
        self.assertNotRegex(text, r"(?m):\s*write\s*$")
        inputs = re.search(r"(?ms)^    inputs:\n(.*?)(?=^permissions:)", text).group(1)
        mode = re.search(r"(?ms)^      mode:\n(.*?)(?=^      \S)", inputs).group(1)
        self.assertRegex(mode, r"default: plan\b")
        self.assertRegex(mode, r"options: \[plan, probe\]")
        crates = re.search(r"(?ms)^      crates:\n(.*?)(?=^      \S|\Z)", inputs).group(1)
        self.assertRegex(crates, r"default: family\b")
        self.assertRegex(crates, r"options: \[family, all\]")
        job = _job_block(text, "preflight")
        self.assertIn("if: github.ref != 'refs/heads/main'", job)
        self.assertRegex(job, r"exit 1")
        self.assertIn("max-parallel: 1", job)
        self.assertIn("persist-credentials: false", job)
        self.assertIn("'twine==7.0.0'", job)
        for name in ("NPM_TOKEN", "PYPI_API_TOKEN", "CARGO_REGISTRY_TOKEN"):
            uses = re.findall(rf"secrets\.{name}\b[^\n]*", text)
            self.assertEqual(uses, [f"secrets.{name} || '' }}}}"], name)
            self.assertIn(f"{name}: ${{{{ inputs.mode == 'probe' && secrets.{name} || '' }}}}", job)
        self.assertEqual(len(re.findall(r"secrets\.", text)), 3)
        steps = re.split(r"(?m)^      - ", job)
        secret_steps = [step for step in steps if "secrets." in step]
        self.assertEqual(len(secret_steps), 1)
        self.assertIn("python3 -I scripts/release-registry-preflight", secret_steps[0])
        self.assertIn('--mode "${MODE}"', secret_steps[0])
        self.assertIn('--crates "${CRATES}"', secret_steps[0])
        self.assertIn("actions/upload-artifact@", job)
        for line in re.findall(r"(?m)^\s*(?:-\s*)?uses:\s*(\S+.*)$", text):
            self.assertRegex(line, r"^[^@\s]+@[0-9a-f]{40} # \S+$")


if __name__ == "__main__":
    unittest.main()
