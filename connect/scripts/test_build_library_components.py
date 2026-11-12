"""Checks of the first-party component build: orchestration, pins, records and the reproducibility measure.

The two wheels are staged by the checkout's real staging scripts. The pinned build backend and the Node
runtime are test doubles written into a temporary directory, so nothing is downloaded and no real package
manager runs; what a real backend and npm produce is measured by the release run, not here.
"""
from __future__ import annotations

import hashlib
import io
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tarfile
import tempfile
import time
import unittest
import zipfile

ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT / "scripts/build-library-components.py"
FAILURE = "component build failed: read the logs under the output directory; no record was written\n"
AGNO = "meerkat_toolkit_agno-0.1.5-py3-none-any.whl"
SERVICES = "meerkat_toolkit_services-0.1.23-py3-none-any.whl"
ARCHIVE = "rkat-toolkit-activepieces-0.1.0.tgz"
EPOCH = 1794000000

# A stand-in for the pinned backend: it packs the staged tree the way a wheel is laid out and records which
# backend version built it and whether the helper wheels were importable beside it.
BACKEND = '''
import os, pathlib, re, time, zipfile
import packaging, wheel
VERSION = %(version)r
UNSTABLE = %(unstable)r

def build_wheel(wheel_directory, config_settings=None, metadata_directory=None):
    if "G5_TEST_SENTINEL" in os.environ:
        raise SystemExit("the caller's environment reached the backend")
    text = pathlib.Path("pyproject.toml").read_text()
    name = re.search(r'^name = "([^"]+)"', text, re.M)[1].replace("-", "_")
    version = re.search(r'^version = "([^"]+)"', text, re.M)[1]
    stamp = time.gmtime(int(os.environ["SOURCE_DATE_EPOCH"]))[:6]
    filename = "%%s-%%s-py3-none-any.whl" %% (name, version)
    with zipfile.ZipFile(os.path.join(wheel_directory, filename), "w") as archive:
        for path in sorted(pathlib.Path("src").rglob("*")):
            if path.is_file():
                archive.writestr(zipfile.ZipInfo(path.relative_to("src").as_posix(), stamp), path.read_bytes())
        note = "Backend: %%s\\nHelpers: %%s %%s\\n" %% (VERSION, packaging.MARK, wheel.MARK)
        if UNSTABLE:
            note += "Built-At: %%d\\n" %% time.time_ns()
        archive.writestr(zipfile.ZipInfo("%%s-%%s.dist-info/METADATA" %% (name, version), stamp),
                         "Metadata-Version: 2.4\\nName: %%s\\nVersion: %%s\\n%%s" %% (name, version, note))
    return filename
'''

# A stand-in for `node npm-cli.js ...`: the three npm commands of the build and the two version questions.
NODE = '''#!%(python)s
import io, json, os, pathlib, sys, tarfile, time
if "G5_TEST_SENTINEL" in os.environ:
    raise SystemExit("the caller's environment reached npm")
argv = sys.argv[1:]
if argv == ["--version"]:
    print("v24.19.0")
    raise SystemExit(0)
cli, command = argv[0], argv[1:]
if not pathlib.Path(cli).name == "npm-cli.js":
    raise SystemExit("unexpected program")
root = pathlib.Path.cwd()
if command == ["--version"]:
    print("11.0.0")
elif command == ["ci", "--ignore-scripts"]:
    if not (root / "package-lock.json").is_file() or (root / "node_modules").exists() or (root / "dist").exists():
        raise SystemExit("ci needs the lock and a clean copy")
    if not os.environ.get("npm_config_cache", "").startswith(os.environ.get("HOME", "missing").rsplit("/", 1)[0]):
        raise SystemExit("npm cache is outside the scratch directory")
    (root / "node_modules").mkdir()
    (root / "node_modules/.installed").write_text("ok")
elif command == ["run", "build"]:
    if %(fail_build)r:
        print("simulated compiler error", file=sys.stderr)
        raise SystemExit(1)
    if not (root / "node_modules/.installed").is_file():
        raise SystemExit("build before ci")
    (root / "dist").mkdir()
    (root / "dist/index.js").write_text("// built from " + (root / "src/index.ts").read_text())
elif command[:2] == ["pack", "--ignore-scripts"] and command[2] == "--pack-destination" and len(command) == 4:
    package = json.loads((root / "package.json").read_text())
    name = package["name"].lstrip("@").replace("/", "-") + "-" + package["version"] + ".tgz"
    members = {"package.json": (root / "package.json").read_bytes(), "README.md": b"guide\\n"}
    for path in sorted((root / "dist").rglob("*")):
        if path.is_file():
            members["dist/" + path.relative_to(root / "dist").as_posix()] = path.read_bytes()
    if %(unstable)r:
        members["dist/built-at.txt"] = str(time.time_ns()).encode()
    raw = io.BytesIO()
    with tarfile.open(fileobj=raw, mode="w") as archive:
        for member, data in members.items():
            row = tarfile.TarInfo("package/" + member)
            row.size = len(data)
            archive.addfile(row, io.BytesIO(data))
    import gzip
    with open(os.path.join(command[3], name), "wb") as output:
        with gzip.GzipFile(fileobj=output, mode="wb", mtime=0) as stream:
            stream.write(raw.getvalue())
    print(name)
else:
    raise SystemExit("unexpected npm command: " + " ".join(command))
'''


def sha(data):
    return hashlib.sha256(data).hexdigest()


def pure_wheel(name, version, files):
    path_name = "%s-%s-py3-none-any.whl" % (name, version)
    output = io.BytesIO()
    with zipfile.ZipFile(output, "w") as archive:
        for member, text in files.items():
            archive.writestr(member, text)
        archive.writestr("%s-%s.dist-info/METADATA" % (name, version), "Name: %s\nVersion: %s\n" % (name, version))
    return path_name, output.getvalue()


class ComponentBuildTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.scratch = tempfile.TemporaryDirectory(prefix="toolkit-component-build-")
        cls.addClassCleanup(cls.scratch.cleanup)
        cls.base = Path(cls.scratch.name).resolve()
        # A checkout root with the real staging inputs and a small Activepieces project.
        cls.root = cls.base / "checkout"
        for name in ("agno", "services", "python"):
            shutil.copytree(ROOT / "sdk" / name, cls.root / "sdk" / name,
                            ignore=shutil.ignore_patterns("__pycache__", "*.pyc", ".venv*", "node_modules"))
        project = cls.root / "sdk/activepieces"
        (project / "src").mkdir(parents=True)
        (project / "src/index.ts").write_text("export const value = 1;\n")
        (project / "package.json").write_text(json.dumps({
            "name": "@rkat/toolkit-activepieces", "version": "0.1.0", "scripts": {"build": "node scripts/build.mjs"}}))
        (project / "package-lock.json").write_text(json.dumps({"lockfileVersion": 3, "packages": {}}))
        # What a developer's checkout may hold and the build must not take.
        (project / "node_modules").mkdir()
        (project / "node_modules/poison.js").write_text("stale")
        (project / "dist").mkdir()
        (project / "dist/stale.js").write_text("stale")

    def setUp(self):
        self.work = Path(tempfile.mkdtemp(prefix="case-", dir=self.base))
        self.pinned = self.work / "pins"
        self.pins_path = self.work / "library-pins-macos-arm64.json"
        self.output = self.work / "components"
        self.rows = []
        self.backend("84.0.0")
        self.backend("82.0.0")
        self.backend("84.2.1", use=("component build tool",))
        for name in ("packaging", "wheel"):
            # As in the committed pin file, one row can serve two readers: `use` is a sorted list of labels.
            self.pin(*pure_wheel(name, "1.0", {name + "/__init__.py": "MARK = %r\n" % (name + "-helper")}),
                     name=name, version="1.0",
                     use=("agno closure", "component build tool") if name == "packaging" else ("component build tool",))
        self.write_pins()
        self.node = self.fake_node()

    def backend(self, version, unstable=False, use=("component build backend",)):
        self.rows = [row for row in self.rows if not (row["name"] == "setuptools" and row["version"] == version)]
        filename, data = pure_wheel("setuptools", version, {
            "setuptools/__init__.py": "", "setuptools/build_meta.py": BACKEND % {"version": version, "unstable": unstable}})
        self.pin(filename, data, name="setuptools", version=version, use=use)

    def pin(self, filename, data, name, version, use):
        target = self.pinned / "pypi-wheel" / filename
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes(data)
        self.rows.append({"kind": "pypi-wheel", "name": name, "version": version, "file": filename,
                          "url": "https://files.pythonhosted.org/packages/fixture/" + filename,
                          "sha256": sha(data), "sha512": None, "digest_source": "fixture", "use": sorted(use)})

    def write_pins(self):
        self.pins_path.write_text(json.dumps({"schema": "connect-library-pins/1", "target": "macos-arm64",
                                              "pins": self.rows}))

    def fake_node(self, fail_build=False, unstable=False):
        directory = Path(tempfile.mkdtemp(prefix="node-", dir=self.work))
        node = directory / "node"
        node.write_text(NODE % {"python": sys.executable, "fail_build": fail_build, "unstable": unstable})
        node.chmod(0o755)
        (directory / "npm-cli.js").write_text("// stand-in\n")
        return node

    def argv(self, *extra, **changes):
        values = {"root": self.root, "pins": self.pins_path, "pinned-inputs": self.pinned, "python": sys.executable,
                  "node": self.node, "npm-cli": self.node.with_name("npm-cli.js"),
                  "source-date-epoch": EPOCH, "output": self.output}
        values.update(changes)
        argv = [sys.executable, "-I", "-B", str(SCRIPT)]
        for key, value in values.items():
            if value is not None:
                argv += ["--" + key, str(value)]
        return argv + list(extra)

    def run_build(self, *extra, **changes):
        # A value in the caller's environment that no child may see.
        env = {"PATH": "/usr/bin:/bin", "PYTHONDONTWRITEBYTECODE": "1", "G5_TEST_SENTINEL": "present"}
        return subprocess.run(self.argv(*extra, **changes), env=env, capture_output=True, text=True, timeout=300)

    def built(self, *extra, **changes):
        result = self.run_build(*extra, **changes)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        return json.loads(result.stdout)

    def refused(self, *extra, **changes):
        result = self.run_build(*extra, **changes)
        self.assertEqual((result.returncode, result.stdout, result.stderr), (2, "", FAILURE))
        self.assertFalse((self.output / "components.json").exists())
        return result

    def test_builds_the_three_components_and_records_their_digests(self):
        summary = self.built()
        record = json.loads((self.output / "components.json").read_bytes())
        self.assertEqual(summary, {**record, "output": str(self.output)})
        self.assertEqual(record["schema"], "connect-library-components/1")
        self.assertEqual(record["source_date_epoch"], EPOCH)
        self.assertEqual(record["python"], ".".join(str(part) for part in sys.version_info[:3]))
        self.assertEqual((record["node"], record["npm"]), ("v24.19.0", "11.0.0"))
        self.assertIsNone(record["reproducible"])
        self.assertEqual(set(record["components"]), {"agno", "services", "activepieces"})
        for family, filename in (("agno", AGNO), ("services", SERVICES), ("activepieces", ARCHIVE)):
            data = (self.output / filename).read_bytes()
            row = record["components"][family]
            self.assertEqual((row["file"], row["sha256"], row["bytes"]), (filename, sha(data), len(data)))
        self.assertEqual(sorted(path.name for path in self.output.iterdir()),
                         sorted([AGNO, SERVICES, ARCHIVE, "components.json", "logs"]))
        self.assertTrue(any((self.output / "logs").iterdir()))
        # Each wheel was built by the backend version its own project names, with the helper wheels beside it.
        for family, filename, version in (("agno", AGNO, "84.0.0"), ("services", SERVICES, "82.0.0")):
            backend = record["components"][family]["backend"]
            row = next(row for row in self.rows if row["name"] == "setuptools" and row["version"] == version)
            self.assertEqual(backend, {"name": "setuptools", "version": version,
                                       "pin": "pypi-wheel/" + row["file"], "sha256": row["sha256"]})
            with zipfile.ZipFile(self.output / filename) as archive:
                metadata = archive.read(next(name for name in archive.namelist() if name.endswith("METADATA"))).decode()
                self.assertIn("Backend: " + version + "\n", metadata)
                self.assertIn("Helpers: packaging-helper wheel-helper\n", metadata)
                self.assertEqual({info.date_time for info in archive.infolist()}, {tuple(time.gmtime(EPOCH)[:6])})
        # The staged trees are the checkout's own staging scripts' output.
        with zipfile.ZipFile(self.output / AGNO) as archive:
            self.assertIn("meerkat_toolkit_agno/PROFILE-GUIDE.md", archive.namelist())
            self.assertIn("meerkat_toolkit_agno/PACKAGE-SOURCES.json", archive.namelist())
            self.assertEqual(archive.read("meerkat_toolkit_agno/agno_mcp.py"),
                             (self.root / "sdk/python/src/meerkat_toolkit/adapters/agno_mcp.py").read_bytes())
        with zipfile.ZipFile(self.output / SERVICES) as archive:
            self.assertIn("meerkat_toolkit_services/GUIDE.md", archive.namelist())
        # The archive is built from a copy without the checkout's own node_modules and dist.
        with tarfile.open(self.output / ARCHIVE) as archive:
            self.assertEqual(sorted(archive.getnames()), ["package/README.md", "package/dist/index.js", "package/package.json"])
        self.assertEqual(record["components"]["activepieces"]["lock_sha256"],
                         sha((self.root / "sdk/activepieces/package-lock.json").read_bytes()))
        project = self.root / "sdk/activepieces"
        self.assertEqual(sorted(path.name for path in (project / "node_modules").iterdir()), ["poison.js"])
        self.assertEqual(sorted(path.name for path in (project / "dist").iterdir()), ["stale.js"])
        self.assertEqual(sorted(path.name for path in project.iterdir()),
                         ["dist", "node_modules", "package-lock.json", "package.json", "src"])
        self.assertEqual(self.output.stat().st_mode & 0o777, 0o755)
        self.assertEqual((self.output / AGNO).stat().st_mode & 0o777, 0o644)

    def test_two_builds_of_one_source_are_compared_and_the_result_is_stated(self):
        record = self.built("--verify-reproducible")
        self.assertEqual(record["reproducible"], {
            family: {"equal": True, "second_sha256": record["components"][family]["sha256"]}
            for family in ("agno", "services", "activepieces")})
        logs = sorted(path.name for path in (self.output / "logs").iterdir())
        self.assertTrue(any(name.startswith("build-1-") for name in logs))
        self.assertTrue(any(name.startswith("build-2-") for name in logs))

    def test_a_build_that_does_not_reproduce_is_reported_not_hidden(self):
        self.backend("82.0.0", unstable=True)
        self.write_pins()
        self.node = self.fake_node(unstable=True)
        record = self.built("--verify-reproducible")
        reproducible = record["reproducible"]
        self.assertEqual(reproducible["agno"], {"equal": True, "second_sha256": record["components"]["agno"]["sha256"]})
        for family in ("services", "activepieces"):
            self.assertFalse(reproducible[family]["equal"])
            self.assertRegex(reproducible[family]["second_sha256"], r"^[0-9a-f]{64}$")
            self.assertNotEqual(reproducible[family]["second_sha256"], record["components"][family]["sha256"])
        # The delivered files are the first build's.
        self.assertEqual(sha((self.output / SERVICES).read_bytes()), record["components"]["services"]["sha256"])

    def test_the_same_epoch_gives_the_same_bytes_in_another_run(self):
        first = self.built()
        second = self.built(output=self.work / "again")
        self.assertEqual(first["components"], second["components"])
        third = self.built(**{"output": self.work / "later", "source-date-epoch": EPOCH + 86400})
        self.assertNotEqual(first["components"]["agno"]["sha256"], third["components"]["agno"]["sha256"])

    def test_backend_must_be_the_exact_pinned_version_the_project_names(self):
        saved = list(self.rows)
        with self.subTest("row missing"):
            self.rows = [row for row in saved if not (row["name"] == "setuptools" and row["version"] == "82.0.0")]
            self.write_pins()
            self.refused()
            self.assertFalse(self.output.exists())
        with self.subTest("two rows"):
            twin = next(row for row in saved if row["name"] == "setuptools" and row["version"] == "84.0.0")
            self.rows = [*saved, {**twin, "file": "setuptools-84.0.0-py2.py3-none-any.whl"}]
            self.write_pins()
            self.refused()
        with self.subTest("digest"):
            self.rows = [dict(row, sha256="0" * 64) if row["name"] == "setuptools" and row["version"] == "84.0.0" else row
                         for row in saved]
            self.write_pins()
            self.refused()
            self.assertFalse(self.output.exists())
        with self.subTest("helper missing"):
            self.rows = [row for row in saved if row["name"] != "packaging"]
            self.write_pins()
            self.refused()
        with self.subTest("helper under another use label"):
            self.rows = [dict(row, use=["agno closure"]) if row["name"] == "wheel" else row for row in saved]
            self.write_pins()
            self.refused()
        with self.subTest("label as a string, not the pin file's list"):
            self.rows = [dict(row, use="component build tool") if row["name"] == "wheel" else row for row in saved]
            self.write_pins()
            self.refused()
        with self.subTest("pin file schema"):
            self.pins_path.write_text(json.dumps({"schema": "connect-library-pins/2", "target": "macos-arm64", "pins": saved}))
            self.refused()

    def test_project_that_names_another_backend_version_or_component_version_refuses(self):
        cases = {"sdk/agno/pyproject.toml": [
            ('requires = ["setuptools==84.0.0"]', 'requires = ["setuptools>=84"]'),
            ('requires = ["setuptools==84.0.0"]', 'requires = ["setuptools==84.0.0", "wheel"]'),
            ('build-backend = "setuptools.build_meta"', 'build-backend = "other.backend"'),
            ('version = "0.1.5"', 'version = "0.1.6"')],
            "sdk/activepieces/package.json": [('"version": "0.1.0"', '"version": "0.2.0"'),
                                               ('"@rkat/toolkit-activepieces"', '"@rkat/other"')]}
        for relative, replacements in cases.items():
            target = self.root / relative
            saved = target.read_text()
            for old, new in replacements:
                with self.subTest(relative=relative, new=new):
                    self.assertIn(old, saved)
                    try:
                        target.write_text(saved.replace(old, new))
                        self.refused()
                        self.assertFalse(self.output.exists())
                    finally:
                        target.write_text(saved)

    def test_failing_child_keeps_its_log_and_writes_no_record(self):
        self.node = self.fake_node(fail_build=True)
        self.refused()
        logs = {path.name: path.read_text() for path in (self.output / "logs").iterdir()}
        self.assertIn("simulated compiler error", logs["build-1-activepieces-build.stderr"])
        self.assertFalse((self.output / ARCHIVE).exists())
        self.assertFalse((self.output / AGNO).exists())

    def test_paths_and_destination_are_checked_before_anything_runs(self):
        for label, changes in (("relative python", {"python": "python3"}), ("relative node", {"node": "node"}),
                               ("missing npm", {"npm-cli": self.work / "absent/npm-cli.js"}),
                               ("relative output", {"output": "components"}),
                               ("missing parent", {"output": self.work / "absent/components"}),
                               ("epoch", {"source-date-epoch": "yesterday"}), ("negative epoch", {"source-date-epoch": -5}),
                               ("no pins", {"pins": None}), ("no epoch", {"source-date-epoch": None})):
            with self.subTest(label):
                self.refused(**changes)
                self.assertFalse(self.output.exists())
        self.output.mkdir()
        (self.output / "owned.txt").write_text("keep me")
        self.refused()
        self.assertEqual([path.name for path in self.output.iterdir()], ["owned.txt"])

    def test_pinned_wheel_with_an_unsafe_member_is_not_unpacked(self):
        raw = io.BytesIO()
        with zipfile.ZipFile(raw, "w") as archive:
            archive.writestr("setuptools/__init__.py", "")
            archive.writestr("../escaped.py", "raise SystemExit('unpacked outside')")
        row = next(row for row in self.rows if row["name"] == "setuptools" and row["version"] == "84.0.0")
        (self.pinned / "pypi-wheel" / row["file"]).write_bytes(raw.getvalue())
        row["sha256"] = sha(raw.getvalue())
        self.write_pins()
        self.refused()
        self.assertEqual([path.name for path in self.work.rglob("escaped.py")], [])


if __name__ == "__main__":
    unittest.main()
