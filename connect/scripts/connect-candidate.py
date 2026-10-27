#!/usr/bin/env python3
"""Compose Connect candidates and publish their exact accepted asset bytes.

Candidate construction still refuses unbound inputs before product execution.
Acceptance has no write credentials. Publication only adds the selected bytes
of an accepted installed preview to an existing public release; it never creates
a release/tag, rebuilds a product, or rewrites producer records.
"""
from pathlib import Path, PurePosixPath
import argparse
import email.parser
import gzip
import hashlib
import io
import json
import os
import platform
import re
import shutil
import signal
import stat
import subprocess
import sys
import tarfile
import tempfile
import time
import tomllib
import urllib.parse
import urllib.request
import zipfile

ROOT = Path(__file__).resolve().parents[1]
TARGETS = {"linux-x86_64": ("Linux", "x86_64"), "macos-arm64": ("Darwin", "arm64")}
SCOPE = "installed_gateway_schema4_resource_startup_and_shutdown_preview"
MAX_FILE = 2 * 1024**3 - 1
HEX = re.compile(r"[0-9a-f]{64}\Z")
SHA = re.compile(r"[0-9a-f]{40}\Z")
VERSION = re.compile(r"[0-9]+\.[0-9]+\.[0-9]+\Z")
FORGE_READERS = {"installer/launchkit/runtime_selection.py",
                 "installer/launchkit/runtime_candidates.py",
                 "installer/launchkit/sdk_wheel_integrity.py",
                 "installer/launchkit/templates/install_candidate.py"}


def require(ok, message):
    if not ok:
        raise ValueError(message)


def checked_target(value):
    require(isinstance(value, str) and value in TARGETS, "explicit supported target required")
    return value


def selected_target():
    return checked_target(os.environ.get("CONNECT_TARGET"))


def input_path(target):
    checked_target(target)
    name = "connect-release-inputs.json" if target == "linux-x86_64" else "connect-release-inputs-macos-arm64.json"
    return ROOT / "release" / name


def publication_files(target):
    checked_target(target)
    return (f"connect-{target}-candidate.tar.gz", f"connect-{target}-candidate.json",
            f"connect-{target}-provenance.jsonl")


def require_native_host(target):
    require((platform.system(), platform.machine()) == TARGETS[checked_target(target)],
            "candidate target differs from native build host")


def unique(pairs):
    result = {}
    for key, value in pairs:
        require(key not in result, "duplicate JSON key")
        result[key] = value
    return result


def read_json(path):
    return json.loads(path.read_bytes(), object_pairs_hook=unique)


def digest(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def binding(path):
    return {"path": str(path), "size": path.stat().st_size, "sha256": digest(path)}


def write_new(path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("x", encoding="utf-8") as stream:
        json.dump(value, stream, indent=2, sort_keys=True, ensure_ascii=True, allow_nan=False)
        stream.write("\n")
    path.chmod(0o644)


def relative(value):
    require(isinstance(value, str) and value and "\\" not in value, "invalid relative locator")
    path = PurePosixPath(value)
    require(not path.is_absolute() and str(path) == value and ".." not in path.parts,
            "noncanonical relative locator")
    return value


def unbound(value):
    if value is None:
        return True
    if isinstance(value, dict):
        return any(unbound(item) for item in value.values())
    if isinstance(value, list):
        return any(unbound(item) for item in value)
    return False


def public_url(value):
    parsed = urllib.parse.urlsplit(value)
    require(parsed.scheme == "https" and not parsed.username and not parsed.password
            and not parsed.query and not parsed.fragment, "public immutable HTTPS input required")
    github = (parsed.hostname == "github.com" and re.fullmatch(
        r"/(lukacf/(meerkat|meerkat-meta|meerkat-connect)|astral-sh/python-build-standalone)"
        r"/releases/download/[^/]+/[^/]+", parsed.path))
    pypi = parsed.hostname == "files.pythonhosted.org" and parsed.path.startswith("/packages/")
    node = parsed.hostname == "nodejs.org" and re.fullmatch(r"/dist/v[0-9.]+/[^/]+", parsed.path)
    # Existing resource-owner pins, exposed at their official versioned URLs.
    # Keep these bounded to the selected artifacts; byte pins are checked below.
    npm = (parsed.netloc == "registry.npmjs.org" and parsed.path == "/npm/-/npm-12.2.0.tgz")
    pip = (parsed.netloc == "bootstrap.pypa.io" and parsed.path == "/pip/zipapp/pip-26.2.1.pyz")
    require(github or pypi or node or npm or pip, "input URL is outside the selected public release hosts")


def source_state():
    def git(*args):
        return subprocess.check_output(["git", "-C", str(ROOT), *args], text=True).strip()
    require(git("status", "--porcelain", "--untracked-files=normal") == "", "source checkout is dirty")
    revision = git("rev-parse", "HEAD")
    expected = os.environ.get("CONNECT_SOURCE_SHA", "")
    require(SHA.fullmatch(expected) and revision == expected == os.environ.get("GITHUB_SHA")
            == os.environ.get("GITHUB_WORKFLOW_SHA"), "dispatch, workflow and selected source differ")
    return revision, git("rev-parse", "HEAD^{tree}")


def checked_native_releases(value):
    require(isinstance(value, dict) and set(value) == {"core", "meta"},
            "exact Core/Meta selection required")
    for name, repository in (("core", "meerkat"), ("meta", "meerkat-meta")):
        selected = value[name]
        require(isinstance(selected, dict)
                and set(selected) == {"repository", "version", "tag", "source_sha"}
                and selected["repository"] == "lukacf/" + repository
                and selected["version"] == "0.9.4"
                and isinstance(selected["source_sha"], str) and SHA.fullmatch(selected["source_sha"]),
                "exact Core/Meta 0.9.4 commit selection required")
        tag = selected["tag"]
        # A conservative single-component Git tag name. Empty is an explicit
        # commit-only selection, not an unbound value or a mutable-ref fallback.
        require(isinstance(tag, str) and (tag == "" or (
            re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9._-]{0,127}", tag)
            and ".." not in tag and not tag.endswith((".", ".lock")))),
            "native tag must be empty or a safe single-component Git tag")
    return value


def preflight(*, native=False):
    require(os.environ.get("GITHUB_ACTIONS") == "true"
            and os.environ.get("GITHUB_EVENT_NAME") == "workflow_dispatch"
            and os.environ.get("RUNNER_ENVIRONMENT") == "github-hosted",
            "manual clean GitHub-hosted execution required")
    target = selected_target()
    if native:
        require_native_host(target)
    inputs = input_path(target)
    require(os.environ.get("GITHUB_REPOSITORY") == "lukacf/meerkat-connect", "wrong candidate repository")
    revision, tree = source_state()
    expected = os.environ.get("CONNECT_INPUTS_SHA256", "")
    require(HEX.fullmatch(expected) and digest(inputs) == expected, "release input selection differs")
    value = read_json(inputs)
    require(not unbound(value), "unbound release inputs: no build or old-default fallback")
    require(value["schema_version"] == 1 and value["kind"] == "connect-candidate-inputs"
            and value["target"] == target and value["scope"] == SCOPE, "wrong candidate scope")
    require(VERSION.fullmatch(value["connect_version"]), "explicit Connect version required")
    native = tomllib.loads((ROOT / "native-candidate/Cargo.toml").read_text())
    sdk = tomllib.loads((ROOT / "sdk/python/pyproject.toml").read_text())["project"]
    require(native["workspace"]["package"]["version"] == value["connect_version"],
            "Connect version differs from source metadata")
    require(sdk["name"] == value["sdks"]["toolkit"]["distribution"]
            and sdk["version"] == value["sdks"]["toolkit"]["version"], "Connect SDK metadata differs")
    require(value["rust_toolchain"] == "1.97.0", "unreviewed Rust toolchain")
    checked_native_releases(value["native_release"])
    require(value["sdks"]["meta"]["version"] == "0.9.4", "Meta application SDK must match native release")
    require("meerkat-meta==0.9.4" in sdk.get("optional-dependencies", {}).get("native-host", []),
            "selected Connect SDK must declare the matching native-host dependency")
    require(set(value["forge_reader_source_pins"]) == FORGE_READERS
            and all(HEX.fullmatch(pin) for pin in value["forge_reader_source_pins"].values()),
            "exact selected Meta reader source inventory required")
    files = value["files"]
    require(isinstance(files, dict) and 0 < len(files) <= 512, "bounded complete public file selection required")
    for name, row in files.items():
        relative(name)
        require(set(row) == {"url", "size", "sha256"} and type(row["size"]) is int
                and 0 < row["size"] <= MAX_FILE and HEX.fullmatch(row["sha256"]), "invalid public file pin")
        public_url(row["url"])
    references = list(value["resources"]["upstream"])
    references += [value["resources"][key] for key in ("library", "acquisition_node", "npm", "node_prebuilt_policy")]
    references += [value["dependencies"]["requirements"], *value["dependencies"]["wheels"]]
    references += [value["sdk_build"]["requirements"], *value["sdk_build"]["wheels"]]
    references += [value["sdks"]["meta"]["wheel"]]
    if value["resources"].get("node_build_profile"):
        references.append(value["resources"]["node_build_profile"])
    require(set(references) == set(files), "input inventory is incomplete or contains unselected files")
    require(0 < len(value["dependencies"]["wheels"]) <= 256, "offline wheel closure required")
    require(0 < len(value["sdk_build"]["wheels"]) <= 256, "offline SDK build-tool closure required")
    for path in ("native-candidate/Cargo.toml", "native-candidate/Cargo.lock"):
        require(value["source_pins"][path] == digest(ROOT / path), "selected native source pins differ")
    require(value["expected_package_ids"] and value["expected_features"]
            and value["expected_resolve"] and value["native_family_packages"], "exact actual native graph selection required")
    return value, revision, tree


class Recorder:
    def __init__(self, work):
        self.work = work
        self.records = work / "records"
        self.records.mkdir()

    def run(self, name, argv, *, cwd=ROOT, env=None, seconds=600):
        argv = [str(item) for item in argv]
        selected_env = {**os.environ, **(env or {})}
        log = self.records / (name + ".log")
        started = time.time()
        expired = False
        code = None
        with log.open("xb") as stream:
            process = subprocess.Popen(argv, cwd=cwd, env=selected_env,
                                       stdout=stream, stderr=subprocess.STDOUT, start_new_session=True)
            try:
                code = process.wait(timeout=seconds)
            except subprocess.TimeoutExpired:
                expired = True
                os.killpg(process.pid, signal.SIGTERM)
                try:
                    code = process.wait(timeout=15)
                except subprocess.TimeoutExpired:
                    os.killpg(process.pid, signal.SIGKILL)
                    code = process.wait()
        record_env = {key: selected_env[key] for key in (
            "RUSTUP_TOOLCHAIN", "XDG_CACHE_HOME", "RUST_LANE_ID", "CARGO_INCREMENTAL",
            "CARGO_TERM_COLOR", "CARGO_BUILD_JOBS", "MEERKAT_CONNECT_RESOURCES_SHA256", "TMPDIR", "OPENSSL_DIR",
            "HOME", "PATH", "RUST_TEST_THREADS", "RUST_MIN_STACK")
            if key in selected_env}
        record = {"argv": argv, "cwd": str(cwd), "env": record_env, "exit_code": code,
                  "timed_out": expired, "elapsed_seconds": time.time() - started, "log": binding(log)}
        write_new(self.records / (name + ".json"), record)
        require(code == 0 and not expired, "command did not succeed: " + name)
        return record, log


def download(path, row):
    path.parent.mkdir(parents=True, exist_ok=True)
    # GitHub release downloads redirect to its release-asset CDN. The original
    # URL and all resulting bytes remain explicitly selected; no API credentials.
    with urllib.request.urlopen(row["url"], timeout=60) as response, path.open("xb") as stream:
        size = 0
        while block := response.read(1024 * 1024):
            size += len(block)
            require(size <= row["size"], "public input exceeds selected size")
            stream.write(block)
    require(size == row["size"] and digest(path) == row["sha256"], "public input bytes differ")
    path.chmod(0o644)


def inspect_prebuilt(path):
    """Parse bounded retained metadata; producer paths remain unchanged data."""
    if path.name.endswith((".tar.gz", ".tar.zst")):
        # Only gzip libraries carry nested Connect provenance; zstd Python
        # runtime archives are verified by the existing resource packager.
        if path.name.endswith(".tar.zst"):
            return
        with tarfile.open(path, "r:gz") as archive:
            for member in archive:
                if member.isfile() and (Path(member.name).name in {
                        "CONTENTS.json", "source-manifest.json", "build-record.json",
                        "resource-build-record.json", "direct_url.json"}
                        or member.name.endswith(".json")
                        or ("provenance" in PurePosixPath(member.name).parts
                            and Path(member.name).suffix in {
                                ".log", ".txt", ".stdout", ".stderr", ".sha256",
                                ".py", ".sh", ".toml", ".yml", ".yaml", ".md"})):
                    require(member.size <= 32 * 1024**2, "embedded provenance exceeds bound")
                    raw = archive.extractfile(member).read()
                    if member.name.endswith(".json"):
                        json.loads(raw, object_pairs_hook=unique)
                    else:
                        raw.decode("utf-8")
    elif path.suffix == ".json":
        require(path.stat().st_size <= 32 * 1024**2, "metadata exceeds bound")
        read_json(path)
    elif path.suffix == ".whl":
        with zipfile.ZipFile(path) as archive:
            for item in archive.infolist():
                if not item.is_dir() and ".dist-info/" in item.filename:
                    require(item.file_size <= 32 * 1024**2, "wheel provenance exceeds bound")
                    raw = archive.read(item)
                    if item.filename.endswith(".json"):
                        json.loads(raw, object_pairs_hook=unique)
                    else:
                        raw.decode("utf-8")


def wheel_expectation(path, selected, source):
    with zipfile.ZipFile(path) as wheel:
        names = [item.filename for item in wheel.infolist() if not item.is_dir()]
        require(len(names) == len(set(names)) and len(names) <= 4096, "invalid wheel inventory")
        for name in names:
            relative(name)
        roots = {name.split("/", 1)[0] for name in names if ".dist-info/" in name}
        require(len(roots) == 1, "one wheel metadata owner required")
        prefix = next(iter(roots)) + "/"
        metadata = email.parser.BytesParser().parsebytes(wheel.read(prefix + "METADATA"))
        require(metadata["Name"] == selected["distribution"] and metadata["Version"] == selected["version"]
                and VERSION.fullmatch(selected["version"]), "wheel metadata differs from selection")
        payload = sorted(name for name in names if not name.startswith(prefix))
        require(set(payload) == set(selected["source_files"]), "complete wheel-to-source map required")
        for name in payload:
            source_name = relative(selected["source_files"][name])
            require(wheel.read(name) == (source / source_name).read_bytes(), "wheel payload differs from selected source")
        def rows(items, strip=""):
            return [[name.removeprefix(strip), hashlib.sha256(wheel.read(name)).hexdigest(), len(wheel.read(name))]
                    for name in sorted(items)]
        return {"distribution": selected["distribution"], "version": selected["version"],
                "sha256": digest(path), "size": path.stat().st_size,
                "source_payload": rows(payload),
                "metadata": rows([name for name in names if name.startswith(prefix) and name != prefix + "RECORD"], prefix)}


def copy_exact(source, destination):
    destination.parent.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(source, destination)
    destination.chmod(0o644)
    require(digest(destination) == digest(source), "retained file differs")


def archive_tree(directory, destination):
    with destination.open("xb") as stream, gzip.GzipFile(filename="", mode="wb", fileobj=stream, mtime=0) as zipped:
        with tarfile.open(fileobj=zipped, mode="w", format=tarfile.PAX_FORMAT) as archive:
            for path in [directory, *sorted(directory.rglob("*"))]:
                if path.is_symlink():
                    require(not Path(os.readlink(path)).is_absolute()
                            and path.resolve(strict=True).is_relative_to(directory),
                            "candidate symlink escapes its physical closure")
                info = archive.gettarinfo(str(path), arcname=path.relative_to(directory.parent).as_posix())
                info.uid = info.gid = info.mtime = 0
                info.uname = info.gname = ""
                if path.is_file() and not path.is_symlink():
                    with path.open("rb") as payload:
                        archive.addfile(info, payload)
                else:
                    archive.addfile(info)


def fetch_native_source(record, work, domain, selected):
    checkout = work / (domain + "-source")
    repository = "https://github.com/" + selected["repository"] + ".git"
    record.run(domain + "-init", ["git", "init", checkout])
    record.run(domain + "-fetch", ["git", "fetch", "--no-tags", "--depth=1",
        repository, selected["source_sha"]], cwd=checkout)
    _, identity = record.run(domain + "-identity",
        ["git", "rev-parse", "--verify", "FETCH_HEAD^{commit}"], cwd=checkout)
    require(identity.read_text().strip() == selected["source_sha"], "fetched native commit differs")
    if selected["tag"]:
        tag = "refs/tags/" + selected["tag"]
        record.run(domain + "-tag-fetch", ["git", "fetch", "--no-tags", "--depth=1",
            repository, tag + ":" + tag], cwd=checkout)
        _, identity = record.run(domain + "-tag-identity",
            ["git", "rev-parse", "--verify", tag + "^{commit}"], cwd=checkout)
        require(identity.read_text().strip() == selected["source_sha"], "selected native tag points elsewhere")
    record.run(domain + "-checkout", ["git", "checkout", "--detach", selected["source_sha"]], cwd=checkout)
    return checkout


def build():
    value, revision, tree = preflight(native=True)
    target = value["target"]
    inputs_path = input_path(target)
    subjects = publication_files(target)[:2]
    work = Path(os.environ["RUNNER_TEMP"]) / "connect-candidate"
    require(not work.exists() and not (ROOT / "dist").exists(), "fresh output roots required")
    work.mkdir()
    (work / "tmp").mkdir()
    os.environ["TMPDIR"] = str(work / "tmp")
    record = Recorder(work)
    inputs = work / "inputs"
    artifacts = work / "artifacts"
    artifacts.mkdir()
    copy_exact(inputs_path, work / "inputs-selection.json")
    for name, row in value["files"].items():
        download(inputs / name, row)
        inspect_prebuilt(inputs / name)
    # Fetch only the selected commits into fresh checkouts. An optional tag
    # adds a binding check; neither a default branch nor a prior cache is used.
    releases = {}
    for domain in ("core", "meta"):
        releases[domain] = fetch_native_source(record, work, domain, value["native_release"][domain])
    for name, expected in value["forge_reader_source_pins"].items():
        path = releases["meta"] / name
        require(path.is_file() and not path.is_symlink() and digest(path) == expected,
                "selected Meta commit lacks the selected actual Forge reader")
    # --no-index alone does not forbid direct URLs or included requirements.
    # The selected owner's existing reader validates the exact single-hash
    # grammar and each original wheel/RECORD before either pip installation.
    for batch_name in ("sdk_build", "dependencies"):
        batch = value[batch_name]
        batch_selection = work / (batch_name + "-selection.json")
        write_new(batch_selection, {"requirements": binding(inputs / batch["requirements"]),
            "wheels": [binding(inputs / name) for name in batch["wheels"]]})
        record.run(batch_name + "-read", [sys.executable, "-I", "-c",
            "import json,sys; from pathlib import Path; "
            "sys.path.insert(0,sys.argv[1]); "
            "from launchkit.runtime_candidates import ArtifactFile; "
            "from launchkit.templates.install_candidate import _read_selected_dependency_wheels; "
            "selected=json.loads(Path(sys.argv[2]).read_text()); "
            "pin=lambda row: ArtifactFile(Path(row['path']),row['size'],row['sha256']); "
            "pending=_read_selected_dependency_wheels(pin(selected['requirements']),"
            "tuple(pin(row) for row in selected['wheels'])); "
            "print(json.dumps({'status':'exact_batch_verified','wheels':len(pending)}))",
            releases["meta"] / "installer", batch_selection], seconds=300)
    # Build the selected Connect SDK through its existing PEP 517 backend. The
    # scratch copy avoids source-checkout mutations; no prior Connect publication
    # is needed. The frontend/backend dependency wheels are explicit offline inputs.
    sdk_source = work / "sdk-source"
    shutil.copytree(ROOT / "sdk/python", sdk_source, symlinks=True)
    sdk_venv = Path(os.environ["RUNNER_TEMP"]) / "connect-sdk-build-venv"
    require(not sdk_venv.exists(), "fresh SDK build environment required")
    record.run("sdk-build-venv", [sys.executable, "-m", "venv", sdk_venv])
    sdk_python = sdk_venv / "bin/python"
    sdk_tools = work / "sdk-build-wheelhouse"
    sdk_tools.mkdir()
    for name in value["sdk_build"]["wheels"]:
        copy_exact(inputs / name, sdk_tools / Path(name).name)
    record.run("sdk-build-tools", [sdk_python, "-m", "pip", "install", "--no-index", "--no-deps", "--require-hashes",
        "--find-links", sdk_tools, "-r", inputs / value["sdk_build"]["requirements"]])
    record.run("sdk-build-inventory", [sdk_python, "-m", "pip", "list", "--format=json"])
    sdk_output = work / "sdk-built"
    sdk_output.mkdir()
    record.run("sdk-wheel", [sdk_python, "-m", "build", "--wheel", "--no-isolation",
        "--outdir", sdk_output, sdk_source], seconds=600)
    wheels = list(sdk_output.glob("*.whl"))
    require(len(wheels) == 1 and list(sdk_output.iterdir()) == wheels, "one original SDK wheel required")
    sdk_wheels = {"toolkit": wheels[0], "meta": inputs / value["sdks"]["meta"]["wheel"]}
    expectations = {name: wheel_expectation(sdk_wheels[name],
        value["sdks"][name], ROOT if name == "toolkit" else releases["meta"]) for name in ("meta", "toolkit")}
    inspect_prebuilt(sdk_wheels["toolkit"])
    write_new(artifacts / "SDK-BYTE-EXPECTATIONS.json", expectations)
    record.run("rust-version", ["rustc", "+1.97.0", "--version", "--verbose"])
    record.run("cargo-version", ["cargo", "+1.97.0", "--version", "--verbose"])
    record.run("python-version", [sys.executable, "--version"])
    if target == "linux-x86_64":
        record.run("system-packages", ["dpkg-query", "-W"])
    else:
        record.run("system-packages", ["brew", "list", "--versions"])
        record.run("system-sdk", ["xcrun", "--show-sdk-path"])
    record.run("rust-sysroot", ["rustc", "+1.97.0", "--print", "sysroot"])
    record.run("cargo-paths", [ROOT / "scripts/repo-cargo", "--print-env"])
    if target == "macos-arm64":
        record.run("system-prefix", ["brew", "--prefix"])
    record.run("system", ["uname", "-a"])
    resource = value["resources"]
    upstream = work / "upstream"
    starter = work / "starter"
    upstream.mkdir()
    for name in resource["upstream"]:
        copy_exact(inputs / name, upstream / Path(name).name)
    # Project only the actual selected library bytes into the existing starter
    # manifest shape. This is a candidate input selector, not a release receipt.
    library = inputs / resource["library"]
    starter_manifest = work / "starter-library-selection.json"
    write_new(starter_manifest, {"schema_version": 1, "status": "candidate_input_not_published",
        "assets": [{"upload_name": library.name, "bytes": library.stat().st_size, "sha256": digest(library)}]})
    record.run("starter-export", [sys.executable, ROOT / "scripts/export-setup-starter.py",
        "--destination", starter, "--manifest", starter_manifest, "--manifest-sha256", digest(starter_manifest)])
    args = [sys.executable, ROOT / "scripts/package-connect-resources.py",
        "--destination", work / "resources", "--upstream", upstream, "--zstd", shutil.which("zstd"),
        "--target", target, "--starter", starter,
        "--release-manifest-sha256", digest(starter / "release-manifest.json")]
    for key, option in (("library", "library"), ("acquisition_node", "acquisition-node"), ("npm", "npm")):
        row = value["files"][resource[key]]
        locator = option + "-archive" if key in ("acquisition_node", "npm") else option
        pin_option = locator if key == "npm" else option
        args += ["--" + locator, inputs / resource[key], "--" + pin_option + "-sha256", row["sha256"],
                 "--" + pin_option + "-bytes", str(row["size"])]
    for key, option in (("node_prebuilt_policy", "node-prebuilt-policy"), ("node_build_profile", "node-build-profile")):
        if resource.get(key):
            args += ["--" + option, inputs / resource[key], "--" + option + "-sha256",
                     value["files"][resource[key]]["sha256"]]
    record.run("resources", args, seconds=900)
    resource_record = work / "resources/build.json"
    resource_value = read_json(resource_record)
    wrapper = ROOT / "scripts/repo-cargo"
    manifest = ROOT / "native-candidate/Cargo.toml"
    _, metadata_log = record.run("metadata", [wrapper, "+1.97.0", "metadata", "--locked", "--format-version", "1",
        "--manifest-path", manifest, "--features", "console,toolkit-mobkit/native-host"], seconds=900)
    # Cargo may print progress before JSON; only the single actual metadata row counts.
    metadata_rows = [json.loads(line) for line in metadata_log.read_text().splitlines() if line.startswith('{')]
    require(len(metadata_rows) == 1, "one actual Cargo metadata result required")
    metadata = metadata_rows[0]
    connect_packages = [package for package in metadata["packages"] if package["name"] == "meerkat-connect"]
    require(len(connect_packages) == 1 and connect_packages[0]["version"] == value["connect_version"]
            and Path(connect_packages[0]["manifest_path"]) == ROOT / "crates/meerkat-connect/Cargo.toml",
            "actual Connect package does not match selected source/version")
    def normalized(identity):
        # Only the selected workspace path is made portable for comparison;
        # all public registry/Git identities remain exact. Raw metadata is kept.
        return identity.replace("path+file://" + str(ROOT) + "/", "path+workspace:///").replace(
            "path+file://" + str(ROOT) + "#", "path+workspace:///#")
    require(sorted(normalized(package["id"]) for package in metadata["packages"])
            == sorted(value["expected_package_ids"]), "actual package graph differs")
    features = {normalized(node["id"]): sorted(node["features"]) for node in metadata["resolve"]["nodes"]}
    require(features == value["expected_features"], "actual feature graph differs")
    resolve = []
    for node in metadata["resolve"]["nodes"]:
        deps = [{"name": dep["name"], "pkg": normalized(dep["pkg"]),
                 "dep_kinds": sorted([{"kind": kind["kind"] or "normal", "target": kind["target"] or ""}
                                      for kind in dep["dep_kinds"]], key=lambda item: (item["kind"], item["target"]))}
                for dep in node["deps"]]
        resolve.append({"id": normalized(node["id"]), "dependencies": sorted(normalized(item) for item in node["dependencies"]),
                        "deps": sorted(deps, key=lambda item: (item["name"], item["pkg"]))})
    normalized_resolve = {"root": normalized(metadata["resolve"]["root"]) if metadata["resolve"]["root"] else "",
                          "nodes": sorted(resolve, key=lambda item: item["id"])}
    require(normalized_resolve == value["expected_resolve"],
            "actual dependency edges or dependency kinds differ")
    families = []
    for package in metadata["packages"]:
        name = package["name"]
        if name in {"meerkat-connect", "meerkat-toolkit"}:
            require(package["source"] is None and package["id"] in metadata["workspace_members"]
                    and Path(package["manifest_path"]) == ROOT / "crates" / name / "Cargo.toml",
                    "Connect-owned package is not the exact selected workspace member")
        elif name == "meerkat" or name.startswith("meerkat-"):
            families.append(package)
    require({package["name"] for package in families} == set(value["native_family_packages"])
            and all(package["version"] == "0.9.4" for package in families),
            "native family is not the complete selected 0.9.4 graph")
    for package in families:
        source = package["source"]
        require(isinstance(source, str), "path-sourced native family package is not a selected registry/Git source")
        if source == "registry+https://github.com/rust-lang/crates.io-index":
            continue  # Cargo verifies the exact selected lock's registry checksum.
        domain = "meta" if package["name"] == "meerkat-meta" else "core"
        release = value["native_release"][domain]
        require(source.startswith("git+https://github.com/" + release["repository"] + ".git?")
                and source.endswith("#" + release["source_sha"]),
                "native Git source is not the selected commit")
    # Exact IDs distinguish registry versions from Git commit sources; the
    # registry checksums remain pinned by the unchanged selected lock bytes.
    source_files = []
    tracked = subprocess.check_output(["git", "-C", str(ROOT), "ls-files", "-z"]).decode().split('\0')
    for name in filter(None, tracked):
        path = ROOT / name
        require(not path.is_symlink(), "source census requires explicit symlink support before selection")
        source_files.append({"path": name, "sha256": digest(path), "size": path.stat().st_size,
                             "mode": oct(stat.S_IMODE(path.stat().st_mode))})
    source_manifest = artifacts / "source-manifest.json"
    write_new(source_manifest, {"schema_version": 1, "repository": os.environ["GITHUB_REPOSITORY"],
        "revision": revision, "tree": tree, "files": source_files,
        "native_release": value["native_release"], "metadata": binding(metadata_log),
        "input_selection": binding(work / "inputs-selection.json")})
    bound_env = {"MEERKAT_CONNECT_RESOURCES_SHA256": resource_value["resources_sha256"]}
    cargo, cargo_log = record.run("native-build", [wrapper, "+1.97.0", "build", "--locked", "--release",
        "--manifest-path", manifest, "-p", "meerkat-connect", "--features", "console,toolkit-mobkit/native-host",
        "--bin", "toolkit_gateway", "--message-format=json-render-diagnostics"], env=bound_env, seconds=5400)
    emitted = [json.loads(line) for line in cargo_log.read_text().splitlines() if line.startswith('{')]
    binaries = [Path(row["executable"]) for row in emitted if row.get("reason") == "compiler-artifact"
                and row.get("target", {}).get("name") == "toolkit_gateway" and row.get("executable")]
    require(len(binaries) == 1, "one actual emitted gateway executable required")
    binary = work / "binary/toolkit_gateway"
    copy_exact(binaries[0], binary)
    binary.chmod(0o755)
    native_record = work / "build-record.json"
    write_new(native_record, {"schema_version": 1, "target": target, "binary_kind": "toolkit_gateway",
        "resources_sha256": resource_value["resources_sha256"], "binary": binding(binary), "cargo": cargo,
        "source_manifest": binding(source_manifest), "resource_build_record_sha256": digest(resource_record)})
    assembly = [sys.executable, ROOT / "scripts/assemble-connect-product.py", "--binary", binary,
        "--binary-sha256", digest(binary), "--build-record", native_record,
        "--resource-build-record", resource_record, "--resource-build-record-sha256", digest(resource_record),
        "--binary-kind", "toolkit_gateway", "--target", target, "--release-version", value["connect_version"]]
    record.run("stage", [*assembly, "--stage-only", "--destination", work / "stage"], seconds=600)
    # Install the original selected Meta wheel and the just-built Connect wheel.
    # There is no network dependency resolution or second SDK build here.
    venv = Path(os.environ["RUNNER_TEMP"]) / "connect-verifier-venv"
    require(not venv.exists(), "fresh verifier environment required")
    record.run("venv", [sys.executable, "-m", "venv", venv])
    python = venv / "bin/python"
    dependency = value["dependencies"]
    wheelhouse = work / "wheelhouse"
    wheelhouse.mkdir()
    for name in dependency["wheels"]:
        copy_exact(inputs / name, wheelhouse / Path(name).name)
    record.run("dependencies", [python, "-m", "pip", "install", "--no-index", "--no-deps", "--require-hashes",
        "--find-links", wheelhouse, "-r", inputs / dependency["requirements"]])
    record.run("sdks", [python, "-m", "pip", "install", "--no-index", "--no-deps",
        sdk_wheels["meta"], sdk_wheels["toolkit"]])
    _, site_log = record.run("site", [python, "-c", "import sysconfig; print(sysconfig.get_path('purelib'))"])
    site = Path(site_log.read_text().strip())
    staged = work / "stage/staging.json"
    record.run("installed-product", [python, "-I", ROOT / "scripts/verify_native_gateway_subprocess.py",
        "--toolkit-gateway", work / ("stage/meerkat-connect-" + target + "/bin/toolkit_gateway"),
        "--toolkit-python", site, "--meta-python", site, "--case", "installed-product",
        "--installed-staging", staged, "--installed-staging-sha256", digest(staged),
        "--output", work / "verification"], seconds=900)
    verification = work / "verification/manifest.json"
    report = read_json(verification)
    require(report["status"] == "passed", "actual installed verification failed")
    record.run("assemble", [*assembly, "--destination", work / "assembled",
        "--gateway-verification", verification, "--gateway-verification-sha256", digest(verification)], seconds=600)
    final = read_json(work / "assembled/assembly.json")
    require(final["scope"] == SCOPE and final["release_version"] == value["connect_version"],
            "final assembly scope or version differs")
    for name in ("assembly.json", "build-record.json", "resource-build-record.json", "gateway-verification.json", final["archive"], final["archive"] + ".sha256"):
        copy_exact(work / "assembled" / name, artifacts / name)
    for name in ("meta", "toolkit"):
        copy_exact(sdk_wheels[name], artifacts / sdk_wheels[name].name)
    for name in dependency["wheels"]:
        copy_exact(inputs / name, artifacts / "dependencies" / Path(name).name)
    copy_exact(inputs / dependency["requirements"], artifacts / "dependency-requirements.txt")
    catalog = Path(report["cases"][0]["model_catalog"]["path"])
    require(digest(catalog) == report["cases"][0]["model_catalog"]["sha256"], "actual catalog changed")
    copy_exact(catalog, artifacts / "model-catalog.json")
    # Exact public reader shape. The producer's absolute locators stay untouched;
    # Forge owns only their later physical relocation, not record rewriting.
    selection = {"format": "forge-meta-toolkit-runtime-selection-v1", "artifact_root": str(artifacts), "target": target,
        "assembly": binding(artifacts / "assembly.json"), "source_manifest": binding(source_manifest),
        "meta_wheel": binding(artifacts / sdk_wheels["meta"].name),
        "toolkit_wheel": binding(artifacts / sdk_wheels["toolkit"].name),
        "dependency_requirements": binding(artifacts / "dependency-requirements.txt"),
        "dependency_wheels": [binding(artifacts / "dependencies" / Path(name).name) for name in dependency["wheels"]],
        "model_catalog": binding(artifacts / "model-catalog.json"), **expectations}
    write_new(artifacts / "RUNTIME-SELECTION.json", selection)
    # Use the selected release's real reader, not a Connect-side reimplementation
    # of the installer acceptance contract. An absent/incompatible reader refuses.
    record.run("forge-reader", [python, "-I", "-c",
        "import json,sys; from pathlib import Path; "
        "sys.path.insert(0,sys.argv[1]); "
        "from launchkit.runtime_selection import parse_candidate_selection; "
        "from launchkit.runtime_candidates import read_meta_toolkit_artifacts; "
        "target=parse_candidate_selection(json.loads(Path(sys.argv[2]).read_text())); "
        "observed=read_meta_toolkit_artifacts(target); "
        "assert observed.origin=='installed-gateway-assembly'; "
        "print(json.dumps({'origin':observed.origin,'status':'verified','published':False}))",
        releases["meta"] / "installer", artifacts / "RUNTIME-SELECTION.json"], seconds=300)
    require(source_state() == (revision, tree), "selected source changed during production")
    for path in ("native-candidate/Cargo.toml", "native-candidate/Cargo.lock"):
        require(digest(ROOT / path) == value["source_pins"][path], "Cargo changed selected source")
    # Retain raw records and input provenance, but not tool caches, checkouts or
    # the installed venv. The original source/SDK identities remain in records.
    closure = Path(os.environ["RUNNER_TEMP"]) / "connect-export"
    closure.mkdir()
    for name in ("artifacts", "records", "inputs", "upstream", "starter", "resources", "stage", "verification", "binary", "tmp", "sdk-built"):
        shutil.copytree(work / name, closure / name, symlinks=True)
    for name in ("inputs-selection.json", "build-record.json", "starter-library-selection.json",
                 "sdk_build-selection.json", "dependencies-selection.json"):
        copy_exact(work / name, closure / name)
    rows = []
    for path in sorted(closure.rglob("*")):
        if path.is_symlink():
            raw = os.readlink(path).encode()
            rows.append({"path": path.relative_to(closure).as_posix(), "kind": "symlink",
                         "size": len(raw), "sha256": hashlib.sha256(raw).hexdigest()})
        elif path.is_file() and not path.is_symlink():
            if path.suffix == ".json":
                read_json(path)
            elif path.suffix in {".log", ".txt", ".sha256"} or path.name == "gateway.stderr":
                path.read_text(encoding="utf-8", errors="strict")
            rows.append({"path": path.relative_to(closure).as_posix(), "size": path.stat().st_size, "sha256": digest(path)})
    write_new(closure / "CONTENTS.json", {"schema_version": 1, "files": rows})
    dist = ROOT / "dist"
    dist.mkdir()
    archive = dist / subjects[0]
    archive_tree(closure, archive)
    require(0 < archive.stat().st_size <= MAX_FILE, "candidate wrapper exceeds Forge public input/archive limit")
    write_new(dist / subjects[1], {"schema_version": 1, "kind": "connect-installed-candidate",
        "repository": os.environ["GITHUB_REPOSITORY"], "source_sha": revision, "source_tree": tree,
        "workflow_sha": os.environ["GITHUB_WORKFLOW_SHA"], "run_id": os.environ["GITHUB_RUN_ID"],
        "run_attempt": os.environ["GITHUB_RUN_ATTEMPT"], "target": target, "scope": SCOPE,
        "connect_version": value["connect_version"], "native_release": value["native_release"],
        "inputs_sha256": digest(inputs_path), "archive": {"name": archive.name, "size": archive.stat().st_size, "sha256": digest(archive)},
        "runtime_selection_sha256": digest(artifacts / "RUNTIME-SELECTION.json"), "publication": "not_performed"})


CLI_COMPOSITION = "installed_product_native_catalog_preparation_and_help"
CLI_SHUTDOWN = "installed_product_native_shutdown_joins_active_worker"


def cli_producer_inputs(work):
    """Reuse one retained producer, without relocating its execution records."""
    require(work.is_absolute() and work.resolve(strict=True) == work,
            "canonical retained candidate work directory required")
    source_path = work / "artifacts/source-manifest.json"
    source = read_json(source_path)
    selection = work / "inputs-selection.json"
    require(binding(selection) == source["input_selection"], "retained input selection changed")
    value = read_json(selection)
    require(source["revision"] == subprocess.check_output(
        ["git", "-C", str(ROOT), "rev-parse", "HEAD"], text=True).strip(), "CLI producer source revision differs")
    for row in source["files"]:
        path = ROOT / relative(row["path"])
        require(not path.is_symlink() and binding(path)["sha256"] == row["sha256"]
                and path.stat().st_size == row["size"], "CLI producer source bytes differ")
    metadata = source["metadata"]
    require(binding(Path(metadata["path"])) == metadata, "retained native graph evidence changed")
    require(value["native_release"] == source["native_release"], "CLI native selection differs")
    resources = work / "resources/build.json"
    native = read_json(work / "build-record.json")
    require(native["source_manifest"] == binding(source_path)
            and native["resource_build_record_sha256"] == digest(resources)
            and native["resources_sha256"] == read_json(resources)["resources_sha256"],
            "CLI inputs differ from the prepared native candidate")
    return value, source_path, resources


def cli_assembly_args(work, build_record):
    value = read_json(work / "inputs-selection.json")
    resources = work / "resources/build.json"
    binary = work / "cli/binary/meerkat-connect"
    return [sys.executable, ROOT / "scripts/assemble-connect-product.py", "--binary", binary,
        "--binary-sha256", digest(binary), "--build-record", build_record,
        "--resource-build-record", resources, "--resource-build-record-sha256", digest(resources),
        "--binary-kind", "meerkat-connect", "--target", value["target"],
        "--release-version", value["connect_version"]]


def emitted_executable(log, name, *, test):
    rows = [json.loads(line) for line in log.read_text().splitlines() if line.startswith('{')]
    paths = [Path(row["executable"]) for row in rows if row.get("reason") == "compiler-artifact"
             and row.get("target", {}).get("name") == name and row.get("executable")
             and row.get("profile", {}).get("test") is test]
    require(len(paths) == 1, "one exact emitted executable required: " + name)
    return paths[0]


def cli_test_evidence(record, installed, name, directory, prefix):
    directory.mkdir()
    for leaf in ("home", "tmp", "empty-path"):
        (directory / leaf).mkdir()
    execution, log = record.run(name, [installed, "--exact", "--ignored", name, "--nocapture"],
        cwd=directory, env={"HOME": str(directory / "home"), "TMPDIR": str(directory / "tmp"),
            "PATH": str(directory / "empty-path"), "RUST_TEST_THREADS": "1", "RUST_MIN_STACK": "8388608"},
        seconds=900)
    # libtest exits successfully when a filter selects zero tests, too.
    require(re.search(r"test result: ok\. 1 passed; 0 failed; 0 ignored;", log.read_text()),
            "the selected installed test did not actually run once")
    roots = list((directory / "tmp").glob(prefix + "*"))
    require(len(roots) == 1 and (roots[0] / "native-dispatch.jsonl").is_file(),
            "owned installed test evidence is missing")
    evidence = [binding(record.records / (name + ".json")), binding(log)]
    # The existing tests retain their raw trace, descriptor, fixture and process
    # markers at this root. The realm/runtime directories remain in place too.
    evidence += [binding(path) for path in sorted(roots[0].iterdir()) if path.is_file()]
    return execution, evidence


def stage_cli(work):
    selected, revision, _ = preflight(native=True)
    value, source, resources = cli_producer_inputs(work)
    require(value == selected and read_json(source)["revision"] == revision,
            "CLI stage differs from the selected candidate")
    cli = work / "cli"
    cli.mkdir()  # Each producer attempt is retained; no in-place rerun.
    record = Recorder(cli)
    resource = read_json(resources)
    bound_env = {"MEERKAT_CONNECT_RESOURCES_SHA256": resource["resources_sha256"]}
    common = ["--locked", "--release", "--manifest-path", ROOT / "native-candidate/Cargo.toml",
              "-p", "meerkat-connect", "--features", "console,toolkit-mobkit/native-host",
              "--message-format=json-render-diagnostics"]
    # Copy production bytes before test compilation can unify dev features or
    # replace Cargo's ordinary output. The fixture is a separate executable.
    cargo, log = record.run("cli-build", [ROOT / "scripts/repo-cargo", "+1.97.0", "build",
        *common, "--bin", "meerkat-connect"], env=bound_env, seconds=5400)
    binary = cli / "binary/meerkat-connect"
    copy_exact(emitted_executable(log, "meerkat-connect", test=False), binary)
    binary.chmod(0o755)
    _, log = record.run("cli-test-build", [ROOT / "scripts/repo-cargo", "+1.97.0", "test",
        *common, "--test", "packaged_setup", "--no-run"], env=bound_env, seconds=5400)
    tested = cli / "binary/packaged_setup"
    copy_exact(emitted_executable(log, "packaged_setup", test=True), tested)
    tested.chmod(0o755)
    build_path = cli / "build-record.json"
    write_new(build_path, {"schema_version": 1, "target": value["target"], "binary_kind": "meerkat-connect",
        "resources_sha256": resource["resources_sha256"], "binary": binding(binary), "cargo": cargo,
        "source_manifest": binding(source), "resource_build_record_sha256": digest(resources),
        "test_executable": binding(tested)})
    record.run("cli-stage", [*cli_assembly_args(work, build_path), "--stage-only",
        "--destination", cli / "stage"], seconds=600)
    staged = cli / "stage/staging.json"
    candidate = read_json(staged)["candidate"]
    installed = cli / ("stage/meerkat-connect-" + value["target"] + "/bin/packaged_setup")
    copy_exact(tested, installed)
    installed.chmod(0o755)
    execution, composition_evidence = cli_test_evidence(record, installed, CLI_COMPOSITION,
        cli / "composition-run", "connect-product-acceptance-")
    write_new(cli / "installed-composition.json", {"schema_version": 1,
        "kind": "meerkat_connect_installed_composition", "scope": CLI_COMPOSITION,
        "status": "passed", "candidate": candidate, "test_executable": binding(installed),
        "execution": execution, "evidence": composition_evidence})
    execution, evidence = cli_test_evidence(record, installed, CLI_SHUTDOWN,
        cli / "shutdown-run", "connect-active-worker-acceptance-")
    evidence += [binding(path) for path in (staged, build_path, resources, source,
        cli / "records/cli-build.json", cli / "records/cli-test-build.json",
        cli / "installed-composition.json")]
    # This is deliberately not accepted. An independent reviewer assesses the
    # retained observations and emits the existing accepted review separately.
    write_new(cli / "native-shutdown-review.pending.json", {"schema_version": 1,
        "kind": "meerkat_connect_native_shutdown_review", "scope": "active_setup_worker_shutdown",
        "disposition": "pending", "candidate": candidate, "test_executable": binding(installed),
        "execution": execution, "evidence": evidence})
    cli_producer_inputs(work)


def assemble_cli(work, review, review_sha256, output):
    value, _, resources = cli_producer_inputs(work)
    cli = work / "cli"
    staged = read_json(cli / "stage/staging.json")
    build_path = cli / "build-record.json"
    require(digest(build_path) == staged["build_record_sha256"]
            and digest(resources) == staged["resource_build_record_sha256"], "CLI staged inputs changed")
    require(HEX.fullmatch(review_sha256) and digest(review) == review_sha256,
            "exact independent shutdown review required")
    # Invoke the existing validators, not a second definition of acceptance.
    import runpy
    assembler = runpy.run_path(str(ROOT / "scripts/assemble-connect-product.py"))
    helpers = runpy.run_path(str(assembler["PREPARE"]))
    candidate = staged["candidate"]
    binary = cli / "binary/meerkat-connect"
    _, observed = assembler["build_record"](build_path, binary, digest(binary), helpers,
        target=value["target"], resources_sha=candidate["resources_sha256"],
        binary_kind="meerkat-connect", resource_record_sha=digest(resources), stage_only=True)
    require(observed["candidate"] == candidate, "CLI staged candidate changed")
    composition = cli / "installed-composition.json"
    assembler["composition_record"](binding(composition), helpers, candidate)
    assembler["shutdown_record"](binding(review), helpers, candidate)
    pending = read_json(cli / "native-shutdown-review.pending.json")
    accepted = read_json(review)
    require(pending["candidate"] == candidate and all(row in accepted["evidence"] for row in pending["evidence"]),
            "shutdown review does not cover this producer's exact observations")
    require(output.is_absolute() and not output.exists(), "fresh CLI assembly output required")
    output.mkdir()
    record = Recorder(output)
    final_build = read_json(build_path)
    final_build.update(installed_composition=binding(composition), native_shutdown_review=binding(review))
    final_path = output / "qualified-build-record.json"
    write_new(final_path, final_build)
    record.run("cli-assemble", [*cli_assembly_args(work, final_path), "--destination", output / "assembled"], seconds=600)
    final = read_json(output / "assembled/assembly.json")
    require(final["scope"] == "conversation_catalog_local_preparation_and_help_preview"
            and final["release_version"] == value["connect_version"]
            and final["qualification"]["candidate"] == candidate
            and final["qualification"]["cli_tty_qualified"] is False, "CLI assembly scope or candidate differs")


def fetch_built():
    value, _, _ = preflight()
    target = value["target"]
    subjects = publication_files(target)[:2]
    identifier = os.environ.get("CONNECT_ARTIFACT_ID", "")
    expected = os.environ.get("CONNECT_ARTIFACT_DIGEST", "").removeprefix("sha256:")
    require(identifier.isdigit() and HEX.fullmatch(expected), "exact build artifact ID and digest required")
    endpoint = "repos/" + os.environ["GITHUB_REPOSITORY"] + "/actions/artifacts/" + identifier
    metadata = json.loads(subprocess.check_output(["gh", "api", endpoint]))
    expected_name = "connect-" + target + "-build-" + os.environ["GITHUB_RUN_ID"] + "-" + os.environ["GITHUB_RUN_ATTEMPT"]
    require(metadata["name"] == expected_name and str(metadata["workflow_run"]["id"]) == os.environ["GITHUB_RUN_ID"]
            and metadata["workflow_run"]["head_sha"] == os.environ["CONNECT_SOURCE_SHA"]
            and not metadata["expired"] and metadata["digest"].removeprefix("sha256:") == expected,
            "artifact run, attempt, source or digest differs")
    archive = Path(os.environ["RUNNER_TEMP"]) / "connect-built.zip"
    with archive.open("xb") as stream:
        subprocess.run(["gh", "api", endpoint + "/zip"], stdout=stream, check=True)
    require(digest(archive) == expected, "downloaded artifact digest differs")
    dist = ROOT / "dist"
    dist.mkdir()
    with zipfile.ZipFile(archive) as zipped:
        entries = zipped.infolist()
        require(len(entries) == 2 and {entry.filename for entry in entries}
                == set(subjects), "unexpected attestation subject set")
        for entry in entries:
            require(0 < entry.file_size <= MAX_FILE and not stat.S_ISLNK(entry.external_attr >> 16),
                    "invalid attestation subject")
            with zipped.open(entry) as source, (dist / entry.filename).open("xb") as destination:
                shutil.copyfileobj(source, destination)
    manifest = read_json(dist / subjects[1])
    checked_candidate_binding(manifest, dist / subjects[0], {
        "target": target,
        "source_sha": os.environ["CONNECT_SOURCE_SHA"], "workflow_sha": os.environ["GITHUB_WORKFLOW_SHA"],
        "run_id": os.environ["GITHUB_RUN_ID"], "run_attempt": os.environ["GITHUB_RUN_ATTEMPT"],
        "inputs_sha256": os.environ["CONNECT_INPUTS_SHA256"]})


# Publication consumes this producer's installed preview, never a rebuilt or
# relabeled staging product. This local acceptance record is not a new runtime
# selection: the original v1 document and Meta-owned reader remain authoritative.
PUBLICATION_REPO = "lukacf/meerkat-connect"
PUBLICATION_WORKFLOW = ".github/workflows/connect-candidate.yml"
SLSA = "https://slsa.dev/provenance/v1"
MAX_METADATA = 32 * 1024**2


def checked_file(path, maximum=MAX_FILE):
    require(path.is_file() and not path.is_symlink() and 0 < path.stat().st_size <= maximum,
            "missing, linked or oversized publication input")
    return {"name": path.name, "size": path.stat().st_size, "sha256": digest(path)}


def checked_publication_request(value):
    require(set(value) == {"source_sha", "source_tree", "source_ref", "run_id", "run_attempt",
                          "inputs_sha256", "connect_version", "release_id", "tag", "target"},
            "exact publication request fields required")
    checked_target(value["target"])
    require(SHA.fullmatch(value["source_sha"]) and SHA.fullmatch(value["source_tree"])
            and HEX.fullmatch(value["inputs_sha256"]) and VERSION.fullmatch(value["connect_version"]),
            "unbound publication source, version or inputs")
    require(re.fullmatch(r"refs/heads/[A-Za-z0-9][A-Za-z0-9._/-]*", value["source_ref"])
            and ".." not in value["source_ref"] and "//" not in value["source_ref"], "invalid source ref")
    require(all(type(value[key]) is int and value[key] > 0 for key in ("run_id", "run_attempt", "release_id"))
            and re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9._-]{0,127}", value["tag"]),
            "explicit existing release and positive run identity required")
    return value


def read_only_environment(work):
    # No inherited GitHub authentication/config or product credentials enter
    # the verifier/reader. Bundle verification needs no artifact-download token.
    home = work / "reader-home"
    home.mkdir()
    return {"PATH": os.environ.get("PATH", ""), "HOME": str(home),
            "GH_CONFIG_DIR": str(home), "LC_ALL": "C.UTF-8", "PYTHONDONTWRITEBYTECODE": "1"}


def command_json(argv, *, env=None, stdin=None):
    return json.loads(subprocess.check_output([str(x) for x in argv], env=env, stdin=stdin, timeout=300),
                      object_pairs_hook=unique)


def verify_publication_subject(path, bundle, request, *, env=None):
    repo = "https://github.com/" + PUBLICATION_REPO
    sha, ref = request["source_sha"], request["source_ref"]
    identity = f"{repo}/{PUBLICATION_WORKFLOW}@{ref}"
    invocation = f"{repo}/actions/runs/{request['run_id']}/attempts/{request['run_attempt']}"
    # This is policy over gh's cryptographically verified result. Never decode
    # the unsigned bundle as evidence or implement signature verification here.
    verified = command_json(["gh", "attestation", "verify", path, "--bundle", bundle, "--hostname", "github.com",
        "--repo", PUBLICATION_REPO, "--source-digest", sha, "--source-ref", ref,
        "--signer-digest", sha, "--cert-identity", identity,
        "--cert-oidc-issuer", "https://token.actions.githubusercontent.com",
        "--deny-self-hosted-runners", "--predicate-type", SLSA, "--format", "json"], env=env)
    require(isinstance(verified, list) and verified, "no verified provenance")
    expected = {"issuer": "https://token.actions.githubusercontent.com",
        "subjectAlternativeName": identity, "buildSignerURI": identity, "buildSignerDigest": sha,
        "sourceRepositoryURI": repo, "sourceRepositoryDigest": sha, "sourceRepositoryRef": ref,
        "buildConfigURI": identity, "buildConfigDigest": sha, "runnerEnvironment": "github-hosted",
        "buildTrigger": "workflow_dispatch", "runInvocationURI": invocation}
    for entry in verified:
        result = entry["verificationResult"]
        certificate = result["signature"]["certificate"]
        require(all(certificate.get(k) == v for k, v in expected.items())
                and result.get("verifiedTimestamps"), "verified certificate identity differs")
        statement = result["statement"]
        require(statement["_type"] == "https://in-toto.io/Statement/v1"
                and statement["predicateType"] == SLSA, "wrong verified statement type")
        subjects = [s for s in statement["subject"] if s.get("name") == path.name]
        require(len(subjects) == 1 and subjects[0]["digest"] == {"sha256": digest(path)},
                "verified subject differs")
        definition = statement["predicate"]["buildDefinition"]
        require(definition["buildType"] == "https://actions.github.io/buildtypes/workflow/v1"
                and definition["externalParameters"]["workflow"] ==
                    {"repository": repo, "path": PUBLICATION_WORKFLOW, "ref": ref}
                and definition["internalParameters"]["github"]["event_name"] == "workflow_dispatch"
                and definition["internalParameters"]["github"]["runner_environment"] == "github-hosted"
                and definition["resolvedDependencies"] ==
                    [{"uri": f"git+{repo}@{ref}", "digest": {"gitCommit": sha}}],
                "verified workflow/source differs")
        details = statement["predicate"]["runDetails"]
        require(details["builder"]["id"] == identity and details["metadata"]["invocationId"] == invocation,
                "verified invocation differs")
    return verified


def checked_candidate_binding(manifest, payload, expected):
    require(all(manifest[key] == expected[key] for key in
                ("source_sha", "workflow_sha", "run_id", "run_attempt", "inputs_sha256", "target")),
            "candidate binding differs")
    require(manifest["archive"] == {"name": payload.name, "size": payload.stat().st_size,
                                    "sha256": digest(payload)}, "candidate closure differs")


def checked_publication_candidate(directory, request):
    files = publication_files(request["target"])
    subjects = files[:2]
    rows = [checked_file(directory / name, MAX_FILE if name == subjects[0] else MAX_METADATA)
            for name in files]
    manifest = read_json(directory / subjects[1])
    checked_candidate_binding(manifest, directory / subjects[0], {
        "target": request["target"], "source_sha": request["source_sha"], "workflow_sha": request["source_sha"],
        "run_id": str(request["run_id"]), "run_attempt": str(request["run_attempt"]),
        "inputs_sha256": request["inputs_sha256"]})
    require(manifest["schema_version"] == 1 and manifest["kind"] == "connect-installed-candidate"
            and manifest["repository"] == PUBLICATION_REPO and manifest["source_tree"] == request["source_tree"]
            and manifest["target"] == request["target"] and manifest["scope"] == SCOPE
            and manifest["connect_version"] == request["connect_version"]
            and manifest["publication"] == "not_performed", "candidate scope or source differs")
    checked_native_releases(manifest["native_release"])
    return manifest, rows


def unpack_publication_candidate(archive_path, destination):
    """Data-only bounded extraction; retained links are checked but not created."""
    import posixpath
    destination.mkdir()
    inventory, names, leaves, parents, expanded = [], set(), set(), set(), 0
    with tarfile.open(archive_path, "r:gz") as archive:
        for member in archive:
            name = relative(member.name)
            parts = PurePosixPath(name).parts
            require(parts[0] == "connect-export" and all(p not in (".", "..") for p in parts)
                    and len(name) <= 4096 and len(parts) <= 64 and name not in names and len(names) < 32768, "invalid candidate archive inventory")
            require(not any(str(parent) in leaves for parent in PurePosixPath(name).parents),
                    "candidate member traverses an archive leaf")
            names.add(name)
            parents.update(str(parent) for parent in PurePosixPath(name).parents)
            if member.isdir():
                continue
            require(len(parts) > 1 and (member.isfile() or member.issym()), "unsupported archive member")
            require(name not in parents, "candidate leaf shadows descendants")
            leaves.add(name)
            rel = PurePosixPath(*parts[1:]).as_posix()
            if member.issym():
                link = member.linkname
                resolved = posixpath.normpath(str(PurePosixPath(name).parent / link))
                require(link and not link.startswith("/") and "\\" not in link
                        and resolved.startswith("connect-export/"), "escaping candidate link")
                raw = link.encode()
                inventory.append({"path": rel, "kind": "symlink", "size": len(raw),
                                  "sha256": hashlib.sha256(raw).hexdigest()})
                continue
            require(0 <= member.size <= MAX_FILE, "candidate member exceeds bound")
            expanded += member.size
            require(expanded <= 16 * 1024**3, "candidate expanded closure exceeds bound")
            path = destination / rel
            path.parent.mkdir(parents=True, exist_ok=True)
            with archive.extractfile(member) as source, path.open("xb") as output:
                shutil.copyfileobj(source, output)
            require(path.stat().st_size == member.size, "truncated candidate member")
            if path.suffix == ".json":
                require(member.size <= MAX_METADATA, "candidate metadata exceeds bound")
                read_json(path)
            elif path.suffix in {".log", ".txt", ".sha256"} or path.name == "gateway.stderr":
                require(member.size <= MAX_METADATA, "candidate text exceeds bound")
                path.read_text()
            if rel != "CONTENTS.json":
                inventory.append({"path": rel, "size": member.size, "sha256": digest(path)})
    contents = read_json(destination / "CONTENTS.json")
    require(contents == {"schema_version": 1, "files": sorted(inventory, key=lambda row: row["path"])},
            "original candidate inventory differs")
    return {row["path"]: row for row in inventory}


def read_publication_product(root, meta, manifest, request, work, env):
    inputs = read_json(root / "inputs-selection.json")
    require(digest(root / "inputs-selection.json") == request["inputs_sha256"]
            and inputs["native_release"] == manifest["native_release"]
            and inputs["connect_version"] == request["connect_version"]
            and inputs["target"] == request["target"], "original input selection differs")
    def git(*args):
        return subprocess.check_output(["git", "-C", str(meta), *args], env=env, timeout=30, text=True).strip()
    selected = checked_native_releases(manifest["native_release"])["meta"]
    require(git("rev-parse", "--verify", "HEAD^{commit}") == selected["source_sha"]
            and (not selected["tag"] or git("rev-parse", "--verify",
                "refs/tags/" + selected["tag"] + "^{commit}") == selected["source_sha"])
            and git("status", "--porcelain", "--untracked-files=all", "--ignored", "--", "installer") == "",
            "reader is not the exact clean selected Meta checkout")
    require(not any(meta.joinpath("installer").rglob("*.pyc")), "reader checkout contains cached Python code")
    pins = inputs["forge_reader_source_pins"]
    require(set(pins) == FORGE_READERS, "reader source inventory differs")
    for name, hashed in pins.items():
        require(HEX.fullmatch(hashed) and checked_file(meta / name)["sha256"] == hashed,
                "actual Meta reader source differs")
    document = root / "artifacts/RUNTIME-SELECTION.json"
    require(digest(document) == manifest["runtime_selection_sha256"], "original selection differs")
    selection = read_json(document)
    require(selection["target"] == request["target"], "runtime selection target differs")
    # This is the existing Forge fetch-layout map. The owner alone performs
    # physical relocation on its in-memory target; no serialized bytes change.
    artifact_root = PurePosixPath(selection["artifact_root"])
    def local(item):
        return "artifacts/" + relative(PurePosixPath(item["path"]).relative_to(artifact_root).as_posix())
    paths = {name: local(selection[name]) for name in
             ("assembly", "source_manifest", "meta_wheel", "toolkit_wheel", "dependency_requirements", "model_catalog")}
    paths.update(artifact_root="artifacts", dependency_wheels=[local(x) for x in selection["dependency_wheels"]])
    layout = work / "layout.json"
    write_new(layout, paths)
    output = command_json([sys.executable, "-I", "-B", "-c",
        "import json,sys; from pathlib import Path; sys.path.insert(0,sys.argv[1]); "
        "from launchkit.runtime_selection import parse_candidate_selection,selected_runtime_artifacts; "
        "from launchkit.packaged_runtime import relocate_selection; "
        "source=json.loads(Path(sys.argv[2]).read_bytes()); parse_candidate_selection(source); "
        "root=Path(sys.argv[4]); target=relocate_selection(source,json.loads(Path(sys.argv[3]).read_bytes()),root); "
        "observed=selected_runtime_artifacts(target); "
        "assert observed.origin=='installed-gateway-assembly'; "
        "print(json.dumps({'origin':observed.origin,'evidence':[{'path':str(f.path.relative_to(root)),"
        "'size':f.size,'sha256':f.sha256} for f in observed.evidence]}))",
        meta / "installer", document, layout, root], env=env)
    require(output["origin"] == "installed-gateway-assembly" and output["evidence"], "Meta did not accept installed preview")
    assembly = read_json(root / paths["assembly"])
    require(assembly["scope"] == SCOPE and assembly["release_version"] == request["connect_version"],
            "assembly is not the selected installed preview")
    evidence = list(output["evidence"])
    for extra in ("artifacts/RUNTIME-SELECTION.json", "artifacts/" + assembly["archive"] + ".sha256"):
        row = checked_file(root / extra)
        evidence.append({"path": extra, "size": row["size"], "sha256": row["sha256"]})
    return selection, paths, evidence


def accept_publication(request_path, candidate, meta, output):
    checked_file(request_path, MAX_METADATA)
    request = checked_publication_request(read_json(request_path))
    files = publication_files(request["target"])
    subjects = files[:2]
    require(not output.exists(), "fresh acceptance directory required")
    output.mkdir()
    env = read_only_environment(output)
    # Retain each input once before verifier/reader awaits. Recheck these exact
    # copies before committing acceptance; no write credential enters children.
    assets = output / "assets"
    assets.mkdir()
    for name in files:
        checked_file(candidate / name, MAX_FILE if name == subjects[0] else MAX_METADATA)
        copy_exact(candidate / name, assets / name)
    manifest, rows = checked_publication_candidate(assets, request)
    for name in subjects:
        verified = verify_publication_subject(assets / name, assets / files[2], request, env=env)
        write_new(output / (name + ".verification.json"), verified)
    root = output / "unpacked"
    inventory = unpack_publication_candidate(assets / subjects[0], root)
    selection, paths, evidence = read_publication_product(root, meta, manifest, request, output, env)
    require(0 < len(evidence) <= 512 and len({r["path"] for r in evidence}) == len(evidence),
            "bounded distinct owner evidence required")
    for row in evidence:
        relative(row["path"])
        require(row == inventory.get(row["path"]), "owner evidence differs from original closure")
    require(checked_publication_candidate(assets, request)[1] == rows, "accepted assets changed")
    accepted = {"schema_version": 1, "kind": "connect-publication-acceptance", "request": request,
        "scope": SCOPE, "assets": rows, "selection": selection, "paths": paths, "evidence": evidence,
        "publication": "not_performed"}
    write_new(output / "acceptance.json", accepted)
    return accepted


def publication_api(endpoint, *, upload=None):
    argv = ["gh", "api", endpoint, "--hostname", "github.com"]
    if upload is not None:
        argv += ["--method", "POST", "--header", "Content-Type: application/octet-stream", "--input", "-"]
    return command_json(argv, stdin=upload)


def release_state(request):
    endpoint = "repos/" + PUBLICATION_REPO
    repository = publication_api(endpoint)
    require(repository.get("full_name") == PUBLICATION_REPO and repository.get("private") is False
            and repository.get("visibility") == "public", "selected repository is not public")
    release = publication_api(endpoint + "/releases/" + str(request["release_id"]))
    require(release["id"] == request["release_id"] and release["tag_name"] == request["tag"]
            and type(release["draft"]) is bool, "selected existing release differs")
    ref = publication_api(endpoint + "/git/ref/tags/" + request["tag"])["object"]
    for _ in range(4):
        if ref["type"] == "commit":
            break
        require(ref["type"] == "tag" and SHA.fullmatch(ref["sha"]), "invalid release tag target")
        ref = publication_api(endpoint + "/git/tags/" + ref["sha"])["object"]
    require(ref == {"type": "commit", "sha": request["source_sha"], "url": ref.get("url")},
            "release tag does not select accepted source")
    assets = []
    for page in range(1, 7):
        batch = publication_api(endpoint + f"/releases/{request['release_id']}/assets?per_page=100&page={page}")
        require(isinstance(batch, list) and len(batch) <= 100, "invalid release asset page")
        assets.extend(batch)
        require(len(assets) <= 512, "release asset inventory exceeds bound")
        if len(batch) < 100:
            break
    require(len({x["name"] for x in assets}) == len(assets), "ambiguous release asset names")
    # Download counters and other presentation fields change during our own
    # public reads. Keep only the actual asset identity/state in the fence.
    selected = {}
    for row in assets:
        require(type(row["id"]) is int and row["id"] > 0 and type(row["size"]) is int,
                "invalid release asset identity")
        selected[row["name"]] = {key: row[key] for key in
            ("id", "name", "size", "state", "browser_download_url")}
        selected[row["name"]]["digest"] = row.get("digest")
    return release, selected


def readback_asset(asset, expected, request, output):
    url = f"https://github.com/{PUBLICATION_REPO}/releases/download/{request['tag']}/{expected['name']}"
    require(asset["name"] == expected["name"] and asset["size"] == expected["size"]
            and asset["state"] == "uploaded" and asset["browser_download_url"] == url,
            "public asset identity differs")
    # Public GET never receives the write token. download checks both size and hash.
    download(output, {"url": url, "size": expected["size"], "sha256": expected["sha256"]})
    return url


def copy_publication_input(path, output, maximum):
    # Open the selected input once. Replacement of its pathname cannot change
    # this read; modification during the copy must still match the accepted hash.
    with os.fdopen(os.open(path, os.O_RDONLY | os.O_NOFOLLOW), "rb") as source:
        info = os.fstat(source.fileno())
        require(stat.S_ISREG(info.st_mode) and 0 < info.st_size <= maximum,
                "bounded regular publication input required")
        size, hashed = 0, hashlib.sha256()
        while chunk := source.read(1024 * 1024):
            size += len(chunk)
            require(size <= maximum, "publication input grew beyond bound")
            output.write(chunk)
            hashed.update(chunk)
        require(size == info.st_size, "publication input size changed")
    return {"name": path.name, "size": size, "sha256": hashed.hexdigest()}


def publish_accepted(directory, expected_sha256, output):
    # This command deliberately has no extraction or Forge-reader call. The
    # independently selected acceptance hash is the handoff into write custody.
    acceptance_path = directory / "acceptance.json"
    with io.BytesIO() as retained:
        row = copy_publication_input(acceptance_path, retained, MAX_METADATA)
        require(HEX.fullmatch(expected_sha256) and row["sha256"] == expected_sha256,
                "selected acceptance changed")
        retained.seek(0)
        accepted = json.loads(retained.read(), object_pairs_hook=unique)
    require(set(accepted) == {"schema_version", "kind", "request", "scope", "assets", "selection", "paths", "evidence", "publication"}
            and accepted["schema_version"] == 1 and accepted["kind"] == "connect-publication-acceptance"
            and accepted["scope"] == SCOPE and accepted["publication"] == "not_performed", "invalid acceptance")
    request = checked_publication_request(accepted["request"])
    files = publication_files(request["target"])
    subjects = files[:2]
    require(isinstance(accepted["assets"], list) and len(accepted["assets"]) == len(files)
            and [row["name"] for row in accepted["assets"]] == list(files),
            "accepted asset allowlist differs")
    require(not output.exists(), "fresh publication observation directory required")
    output.mkdir(mode=0o700)
    assets = output / "assets"
    assets.mkdir(mode=0o700)
    report = {"status": "refused_or_partial", "write_attempted": [], "verified_assets": [], "scope": SCOPE}
    verified_rows = {}
    try:
        # Take write-phase custody before any verifier/API call. Original
        # handoff paths are never reopened after these checked private copies.
        for expected in accepted["assets"]:
            name = expected["name"]
            with (assets / name).open("xb") as retained:
                row = copy_publication_input(directory / "assets" / name, retained,
                    MAX_FILE if name == subjects[0] else MAX_METADATA)
            require(row == expected, "accepted allowlist bytes differ")
            (assets / name).chmod(0o400)
        require(checked_publication_candidate(assets, request)[1] == accepted["assets"],
                "accepted allowlist bytes differ")
        verifier_env = read_only_environment(output)
        for name in subjects:
            verify_publication_subject(assets / name, assets / files[2], request, env=verifier_env)
        release, original = release_state(request)
        # Drafts remain draft under their release owner's control. This command
        # neither creates nor publishes a release/tag, nor invents public URLs.
        require(not release["draft"], "release owner has not published this selected release")
        # Detect an existing conflict anywhere in the selected set before the
        # first write. Otherwise a later conflicting manifest or bundle would
        # leave newly uploaded assets on a release we already cannot complete.
        for expected in accepted["assets"]:
            name = expected["name"]
            if name in original:
                readback_asset(original[name], expected, request, output / name)
        for expected in accepted["assets"]:
            name = expected["name"]
            current_release, current = release_state(request)
            require(not current_release["draft"] and all(current.get(n) == row for n, row in original.items()),
                    "existing release assets changed")
            if name in current:
                # Equal replay is safe only after reading the existing public bytes.
                pass
            else:
                # The anonymous file stays open through gh's stdin read. No
                # pathname is reopened by the write-capable child process.
                with tempfile.TemporaryFile("w+b", dir=output) as upload:
                    row = copy_publication_input(assets / name, upload,
                        MAX_FILE if name == subjects[0] else MAX_METADATA)
                    require(row == expected, "allowlist changed before upload")
                    upload.seek(0)
                    report["write_attempted"].append(name)
                    publication_api(f"https://uploads.github.com/repos/{PUBLICATION_REPO}/releases/{request['release_id']}/assets?name={name}",
                                    upload=upload)
                current_release, current = release_state(request)
                require(not current_release["draft"], "release became private during upload")
            require(name in current, "uploaded asset was not observed")
            if name not in original:
                readback_asset(current[name], expected, request, output / name)
            report["verified_assets"].append(name)
            verified_rows[name] = current[name]
        final_release, final = release_state(request)
        require(not final_release["draft"] and all(final.get(n) == row for n, row in original.items())
                and all(final.get(n) == row for n, row in verified_rows.items())
                and checked_publication_candidate(assets, request)[1] == accepted["assets"],
                "publication changed during readback")
        archive = next(row for row in accepted["assets"] if row["name"] == subjects[0])
        url = f"https://github.com/{PUBLICATION_REPO}/releases/download/{request['tag']}/{archive['name']}"
        files = {row["path"]: {"url": url, "archive_size": archive["size"], "archive_sha256": archive["sha256"],
            "member": "connect-export/" + row["path"], "size": row["size"], "sha256": row["sha256"]}
            for row in accepted["evidence"]}
        report.update(status="verified_public_inventory", acceptance_sha256=expected_sha256,
                      release_id=request["release_id"], tag=request["tag"], files=files,
                      selection=accepted["selection"], paths=accepted["paths"])
        return report
    finally:
        write_new(output / "publication.json", report)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=("preflight", "build", "stage-cli", "assemble-cli", "fetch-built", "accept-publication", "publish-accepted"))
    for name in ("request", "candidate", "meta-source", "accepted", "output", "candidate-work", "shutdown-review"):
        parser.add_argument("--" + name, type=Path)
    parser.add_argument("--acceptance-sha256")
    parser.add_argument("--shutdown-review-sha256")
    args = parser.parse_args()
    command = args.command
    if command == "preflight":
        preflight(native=True)
    elif command == "build":
        build()
    elif command == "stage-cli":
        require(args.candidate_work, "retained candidate work directory required")
        stage_cli(args.candidate_work)
    elif command == "assemble-cli":
        require(all((args.candidate_work, args.shutdown_review, args.shutdown_review_sha256, args.output)),
                "retained producer, exact independent review and output required")
        assemble_cli(args.candidate_work, args.shutdown_review, args.shutdown_review_sha256, args.output)
    elif command == "fetch-built":
        fetch_built()
    elif command == "accept-publication":
        require(all((args.request, args.candidate, args.meta_source, args.output)), "acceptance paths required")
        accept_publication(args.request, args.candidate, args.meta_source, args.output)
    else:
        require(all((args.accepted, args.acceptance_sha256, args.output)), "selected acceptance and output required")
        publish_accepted(args.accepted, args.acceptance_sha256, args.output)


if __name__ == "__main__":
    main()
