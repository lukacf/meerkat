#!/usr/bin/env python3
"""Build the three first-party library components from this checkout's sdk sources.

The consumer exporter takes the results as paths and digests. This script stages and builds; it installs
nothing and downloads nothing itself. The Activepieces build runs `npm ci`, which reads the registry for the
development dependencies that the project's lock pins by integrity: that is the build's one network use.

  agno, services   the checkout's staging script, then the staged project's own build backend hook, run with
                   the pinned backend wheel of exactly the version the project names
  activepieces     npm ci, the project's build script and npm pack, in a copy of the project

With --verify-reproducible everything is built a second time and the digests are compared. The result is
stated in the record; a difference is not a failure.
"""
from __future__ import annotations

import argparse
import hashlib
import io
import json
import os
from pathlib import Path, PurePosixPath
import re
import shutil
import signal
import stat
import subprocess
import sys
import tomllib
import zipfile

ROOT = Path(__file__).resolve().parents[1]
PINS_SCHEMA = "connect-library-pins/1"
PINS_TARGET = "macos-arm64"
# The names the consumer exporter and the committed catalog use. Another version is a change there first.
WHEELS = {
    "agno": ("sdk/agno", "meerkat-toolkit-agno", "0.1.5", "meerkat_toolkit_agno-0.1.5-py3-none-any.whl"),
    "services": ("sdk/services", "meerkat-toolkit-services", "0.1.23", "meerkat_toolkit_services-0.1.23-py3-none-any.whl"),
}
AP_PROJECT = "sdk/activepieces"
AP_PACKAGE = ("@rkat/toolkit-activepieces", "0.1.0")
AP_ARCHIVE = "rkat-toolkit-activepieces-0.1.0.tgz"
MAX_FILE = 64 * 1024 * 1024
MAX_UNPACKED = 256 * 1024 * 1024
FAILURE = "component build failed: read the logs under the output directory; no record was written"
# The staged project's backend hook, as an installer calls it without build isolation. The import path holds
# the standard library and the unpacked backend wheel, which carries its own dependencies; no other package.
HOOK = ("import os, sys\n"
        "sys.path.insert(0, sys.argv[1])\n"
        "os.chdir(sys.argv[2])\n"
        "from setuptools import build_meta\n"
        "print(build_meta.build_wheel(sys.argv[3]))\n")
PYTHON_VERSION = "import sys; print('.'.join(str(part) for part in sys.version_info[:3]))"


class BuildError(ValueError):
    pass


def sha(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def read_regular(path: Path, limit: int = MAX_FILE) -> bytes:
    """One bounded regular file, never through a final symlink."""
    handle = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    with os.fdopen(handle, "rb") as source:
        info = os.fstat(source.fileno())
        if not stat.S_ISREG(info.st_mode) or info.st_size > limit:
            raise BuildError("input must be a bounded regular file")
        return source.read()


def absolute_file(value: str | None) -> Path:
    if not isinstance(value, str) or not value or not Path(value).is_absolute() or not Path(value).is_file():
        raise BuildError("an absolute path of an existing file is required")
    return Path(value)


def normalized(name: str) -> str:
    return re.sub(r"[-_.]+", "-", name).lower()


def pinned_backends(pins: Path, directory: Path, backends: dict[str, str]) -> dict:
    """Select and check the backend wheel of each Python component, by name and exact version."""
    document = json.loads(read_regular(pins))
    if (not isinstance(document, dict) or document.get("schema") != PINS_SCHEMA
            or document.get("target") != PINS_TARGET or not isinstance(document.get("pins"), list)):
        raise BuildError("unsupported pin file")
    rows = [row for row in document["pins"] if isinstance(row, dict) and row.get("kind") == "pypi-wheel"
            and all(isinstance(row.get(key), str) for key in ("name", "version", "file"))]

    def fetched(selected: list[dict]) -> tuple[dict, bytes]:
        if len(selected) != 1:
            raise BuildError("each build wheel needs exactly one pin row")
        row = selected[0]
        if (not re.fullmatch(r"[A-Za-z0-9_][A-Za-z0-9_.-]*-py3-none-any\.whl", row["file"])
                or not isinstance(row.get("sha256"), str) or not re.fullmatch(r"[0-9a-f]{64}", row["sha256"])):
            raise BuildError("a build wheel pin must name a pure wheel and its sha256")
        data = read_regular(directory / "pypi-wheel" / row["file"])
        if sha(data) != row["sha256"]:
            raise BuildError("fetched build wheel differs from its pin")
        return row, data

    return {
        family: fetched([row for row in rows if normalized(row["name"]) == "setuptools" and row["version"] == version])
        for family, version in backends.items()}


def unpack_wheel(data: bytes, destination: Path) -> None:
    """Lay a pure wheel's members out as an import path entry. No installer runs."""
    total = 0
    with zipfile.ZipFile(io.BytesIO(data)) as archive:
        members = archive.infolist()
        for info in members:
            name = PurePosixPath(info.filename)
            kind = (info.external_attr >> 16) & 0o170000
            if (name.is_absolute() or ".." in name.parts or "\\" in info.filename or not name.parts
                    or kind not in (0, 0o100000, 0o040000) or info.flag_bits & 1):
                raise BuildError("pinned wheel holds an unsafe member")
            total += info.file_size
            if total > MAX_UNPACKED:
                raise BuildError("pinned wheel is too large")
        for info in members:
            if info.is_dir():
                continue
            target = destination.joinpath(*PurePosixPath(info.filename).parts)
            target.parent.mkdir(parents=True, exist_ok=True)
            with open(target, "xb") as output:
                output.write(archive.read(info))


def project_backend(root: Path, family: str) -> str:
    """The exact backend version the component's own project file names."""
    directory, distribution, version, _ = WHEELS[family]
    document = tomllib.loads(read_regular(root / directory / "pyproject.toml").decode("utf-8"))
    build, project = document.get("build-system", {}), document.get("project", {})
    requires = build.get("requires")
    match = (re.fullmatch(r"setuptools==([0-9]+(?:\.[0-9]+)*)", requires[0])
             if isinstance(requires, list) and len(requires) == 1 and isinstance(requires[0], str) else None)
    if (match is None or build.get("build-backend") != "setuptools.build_meta"
            or project.get("name") != distribution or project.get("version") != version):
        raise BuildError("component project names another backend, distribution or version")
    return match[1]


class Builder:
    def __init__(self, root: Path, output: Path, python: Path, node: Path, npm_cli: Path, epoch: int):
        self.root, self.output, self.python, self.node, self.npm_cli = root, output, python, node, npm_cli
        self.epoch = epoch
        self.scratch = output / "scratch"
        self.logs = output / "logs"
        # Children get this environment and nothing of the caller's.
        self.environment = {
            "PATH": str(node.parent) + ":/usr/bin:/bin", "HOME": str(self.scratch / "home"),
            "SOURCE_DATE_EPOCH": str(epoch), "TZ": "UTC", "LC_ALL": "C", "LANG": "C",
            "PYTHONDONTWRITEBYTECODE": "1", "PYTHONHASHSEED": "0",
            "npm_config_cache": str(self.scratch / "npm-cache"),
            "npm_config_userconfig": str(self.scratch / "home/npmrc"),
            "npm_config_globalconfig": str(self.scratch / "home/npmrc-global"),
            "npm_config_registry": "https://registry.npmjs.org/",
            "npm_config_update_notifier": "false", "npm_config_fund": "false",
            "npm_config_audit": "false", "npm_config_progress": "false",
        }

    def run(self, label: str, argv: list[str], cwd: Path, timeout: int) -> str:
        """Own and join one child; keep both streams as files under logs/."""
        stdout, stderr = self.logs / (label + ".stdout"), self.logs / (label + ".stderr")
        with open(stdout, "xb") as out, open(stderr, "xb") as err:
            child = subprocess.Popen(argv, cwd=cwd, env=self.environment, stdin=subprocess.DEVNULL,
                                     stdout=out, stderr=err, start_new_session=True)
            try:
                code = child.wait(timeout=timeout)
            except BaseException:
                try:
                    os.killpg(child.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
                child.wait()
                raise
        if code != 0:
            raise BuildError("build step failed: " + label)
        return stdout.read_text(encoding="utf-8", errors="replace").strip()

    def one_file(self, directory: Path, expected: str) -> bytes:
        names = sorted(path.name for path in directory.iterdir())
        if names != [expected]:
            raise BuildError("build produced another file set than the selected component")
        return read_regular(directory / expected)

    def wheel(self, prefix: str, work: Path, family: str, backend: Path) -> bytes:
        directory, _, _, filename = WHEELS[family]
        source, distribution = work / (family + "-source"), work / (family + "-dist")
        distribution.mkdir()
        self.run(prefix + family + "-stage", [str(self.python), "-I", "-S", "-B", str(self.root / directory / "stage.py"),
                                              str(source)], self.root, 600)
        if read_regular(source / "pyproject.toml") != read_regular(self.root / directory / "pyproject.toml"):
            raise BuildError("staged project file differs from the checkout's")
        self.run(prefix + family + "-wheel", [str(self.python), "-I", "-S", "-B", "-c", HOOK, str(backend),
                                              str(source), str(distribution)], source, 1800)
        return self.one_file(distribution, filename)

    def archive(self, prefix: str, work: Path) -> bytes:
        source, distribution = work / "activepieces-source", work / "activepieces-dist"
        distribution.mkdir()
        shutil.copytree(self.root / AP_PROJECT, source, symlinks=True,
                        ignore=lambda directory, names: [name for name in names if Path(directory) == self.root / AP_PROJECT
                                                         and (name in ("node_modules", "dist") or name.endswith(".tgz"))])
        npm = [str(self.node), str(self.npm_cli)]
        self.run(prefix + "activepieces-ci", npm + ["ci", "--ignore-scripts"], source, 3600)
        self.run(prefix + "activepieces-build", npm + ["run", "build"], source, 3600)
        self.run(prefix + "activepieces-pack", npm + ["pack", "--ignore-scripts", "--pack-destination", str(distribution)],
                 source, 1800)
        return self.one_file(distribution, AP_ARCHIVE)

    def build(self, number: int, backends: dict) -> dict[str, bytes]:
        prefix, work = "build-%d-" % number, self.scratch / ("build-%d" % number)
        work.mkdir()
        return {"agno": self.wheel(prefix, work, "agno", backends["agno"]),
                "services": self.wheel(prefix, work, "services", backends["services"]),
                "activepieces": self.archive(prefix, work)}


def build(args: argparse.Namespace) -> dict:
    root = Path(args.root).resolve(strict=True)
    python, node, npm_cli = absolute_file(args.python), absolute_file(args.node), absolute_file(args.npm_cli)
    pins = absolute_file(args.pins)
    if not isinstance(args.pinned_inputs, str) or not Path(args.pinned_inputs).is_absolute():
        raise BuildError("the fetched directory needs an absolute path")
    directory = Path(args.pinned_inputs).resolve(strict=True)
    if not isinstance(args.source_date_epoch, str) or not re.fullmatch(r"[1-9][0-9]{0,11}", args.source_date_epoch):
        raise BuildError("the source date epoch is a positive integer of seconds")
    epoch = int(args.source_date_epoch)
    if (not isinstance(args.output, str) or not Path(args.output).is_absolute()
            or any(part in (".", "..") for part in args.output.split("/"))):
        raise BuildError("the output needs an absolute path without dot components")
    output = Path(args.output).parent.resolve(strict=True) / Path(args.output).name
    if os.path.lexists(output):
        raise BuildError("output already exists")

    # Everything that can be refused without running a child is refused first.
    versions = {family: project_backend(root, family) for family in WHEELS}
    package = json.loads(read_regular(root / AP_PROJECT / "package.json"))
    if not isinstance(package, dict) or (package.get("name"), package.get("version")) != AP_PACKAGE:
        raise BuildError("Activepieces project names another package or version")
    lock = read_regular(root / AP_PROJECT / "package-lock.json")
    backend_wheels = pinned_backends(pins, directory, versions)

    os.umask(0o022)
    os.mkdir(output, 0o755)
    builder = Builder(root, output, python, node, npm_cli, epoch)
    for path in (builder.logs, builder.scratch, builder.scratch / "home", builder.scratch / "npm-cache"):
        path.mkdir()
    backends = {}
    for family, (_, data) in backend_wheels.items():
        backends[family] = builder.scratch / ("backend-" + family)
        unpack_wheel(data, backends[family])
    tools = {
        "python": builder.run("python-version", [str(python), "-I", "-S", "-B", "-c", PYTHON_VERSION], builder.scratch, 60),
        "node": builder.run("node-version", [str(node), "--version"], builder.scratch, 60),
        "npm": builder.run("npm-version", [str(node), str(npm_cli), "--version"], builder.scratch, 60),
    }
    first = builder.build(1, backends)
    reproducible = None
    if args.verify_reproducible:
        second = builder.build(2, backends)
        reproducible = {family: {"equal": second[family] == first[family], "second_sha256": sha(second[family])}
                        for family in first}
    names = {"agno": WHEELS["agno"][3], "services": WHEELS["services"][3], "activepieces": AP_ARCHIVE}
    components = {}
    for family, data in first.items():
        with open(output / names[family], "xb") as target:
            target.write(data)
        components[family] = {"file": names[family], "sha256": sha(data), "bytes": len(data)}
    for family, (row, data) in backend_wheels.items():
        components[family]["backend"] = {"name": "setuptools", "version": versions[family],
                                         "pin": "pypi-wheel/" + row["file"], "sha256": sha(data)}
    components["activepieces"]["lock_sha256"] = sha(lock)
    record = {"schema": "connect-library-components/1", "source_date_epoch": epoch, **tools,
              "components": components, "reproducible": reproducible}
    shutil.rmtree(builder.scratch)
    with open(output / "components.json", "x", encoding="ascii") as target:  # Completion marker, written last.
        target.write(json.dumps(record, sort_keys=True, indent=2, ensure_ascii=True) + "\n")
    return {**record, "output": str(output)}


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--root", default=str(ROOT), help="the connect/ directory of the checkout to build from")
    parser.add_argument("--pins", help="the platform's committed pin file")
    parser.add_argument("--pinned-inputs", help="directory holding <kind>/<file> for every pin, already fetched")
    parser.add_argument("--python", help="absolute path of the CPython (3.11 or later) for staging and the backend")
    parser.add_argument("--node", help="absolute path of the Node executable")
    parser.add_argument("--npm-cli", help="absolute path of npm's bin/npm-cli.js")
    parser.add_argument("--source-date-epoch", help="integer seconds, exported to every child (the commit time)")
    parser.add_argument("--output", help="absolute new directory; its parent must exist")
    parser.add_argument("--verify-reproducible", action="store_true",
                        help="build twice and state per component whether the digests are equal")
    args = parser.parse_args()
    try:
        result = build(args)
    except (OSError, ValueError, KeyError, TypeError, AttributeError, zipfile.BadZipFile,
            subprocess.SubprocessError, shutil.Error):
        print(FAILURE, file=sys.stderr)
        return 2
    print(json.dumps(result, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
