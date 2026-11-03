"""Real local wheel installations at the extension import boundary.

No paired host, network, provider or native agent runtime is involved. Fixture
packages write a marker when imported, so negative cases prove import refusal.
"""

from __future__ import annotations

import base64
import csv
from dataclasses import replace
import hashlib
import importlib
import io
import json
import os
from pathlib import Path
import py_compile
import shutil
import subprocess
import sys
import tempfile
from types import ModuleType
import unittest
from unittest.mock import patch
import zipfile

import meerkat_toolkit.extension_artifacts as extension_owner
from meerkat_toolkit.extension_artifacts import (
    ExtensionArtifactError,
    ExtensionArtifacts,
    SelectedWheel,
    load_extension_entrypoint,
    verify_extension_artifacts,
)
from meerkat_toolkit.host_services import StagedHostServices


ROOTS = ("tkfixture_extension", "tkfixture_one", "tkfixture_two")


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def record(files, name):
    stream = io.StringIO(newline="")
    writer = csv.writer(stream, lineterminator="\n")
    for path, data in sorted(files.items()):
        value = base64.urlsafe_b64encode(hashlib.sha256(data).digest()).rstrip(b"=").decode()
        writer.writerow((path, "sha256=" + value, str(len(data))))
    writer.writerow((name, "", ""))
    return stream.getvalue().encode()


def wheel(directory, distribution, module, source, *, requires=(), additional=None, single_module=False):
    identity = distribution.replace("-", "_")
    info = identity + "-1.0.dist-info"
    metadata = "Metadata-Version: 2.1\nName: " + distribution + "\nVersion: 1.0\n"
    metadata += "Requires-Python: >=3.11\n"
    metadata += "".join("Requires-Dist: " + requirement + "\n" for requirement in requires)
    files = {
        module + (".py" if single_module else "/__init__.py"): source.encode(),
        info + "/METADATA": (metadata + "\n").encode(),
        info + "/WHEEL": b"Wheel-Version: 1.0\nGenerator: toolkit-fixture\nRoot-Is-Purelib: true\nTag: py3-none-any\n",
        **(additional or {}),
    }
    files[info + "/RECORD"] = record(files, info + "/RECORD")
    destination = directory / (identity + "-1.0-py3-none-any.whl")
    with zipfile.ZipFile(destination, "w", zipfile.ZIP_DEFLATED) as archive:
        for name, data in files.items():
            archive.writestr(name, data)
    return destination


def services(*, available=("measurement",), bound=True):
    view = StagedHostServices(declared=("measurement", "optional"), available=available)
    if bound:
        class Measurement:
            def read(self):
                return 7
        view.bind({name: Measurement() for name in available})
    return view


class ReservedImportRootTests(unittest.TestCase):
    """Metadata-only refusal checks; no fixture is installed or imported."""

    def selection(self, directory, module, *, dependency, single_module):
        selected = wheel(directory, "tkfixture-selected", module,
                         "# Inert metadata fixture.\n", single_module=single_module)
        artifact = SelectedWheel(selected, digest(selected))
        if not dependency:
            return ExtensionArtifacts(artifact)
        primary = wheel(directory, "tkfixture-primary", "tkfixture_primary",
                        "# Inert metadata fixture.\n", requires=("tkfixture-selected==1.0",))
        return ExtensionArtifacts(SelectedWheel(primary, digest(primary)), (artifact,))

    def test_platform_names_are_refused_before_import_ownership(self):
        for name in ("meerkat_meta", "meerkat_mobkit"):
            for module in (name, name.upper(), name.title()):
                for dependency in (False, True):
                    for single_module in (False, True):
                        with self.subTest(module=module, dependency=dependency, single_module=single_module):
                            with tempfile.TemporaryDirectory(prefix="toolkit-reserved-root-") as work:
                                directory = Path(work).resolve()
                                selection = self.selection(directory, module, dependency=dependency,
                                                           single_module=single_module)
                                with patch.object(extension_owner, "_import_ownership",
                                                  side_effect=AssertionError("reserved root reached ownership")) as ownership:
                                    with self.assertRaisesRegex(ExtensionArtifactError,
                                                                "^wheel import root is unsupported$"):
                                        verify_extension_artifacts(selection, site_packages=directory)
                                    ownership.assert_not_called()

    def test_host_declared_roots_are_refused_before_import_ownership(self):
        for dependency in (False, True):
            with self.subTest(dependency=dependency):
                with tempfile.TemporaryDirectory(prefix="toolkit-host-root-") as work:
                    directory = Path(work).resolve()
                    base = self.selection(directory, "tkfixture_hostroot", dependency=dependency,
                                          single_module=False)
                    selection = ExtensionArtifacts(base.primary, base.dependencies, ("tkfixture_hostroot",))
                    with patch.object(extension_owner, "_import_ownership",
                                      side_effect=AssertionError("reserved root reached ownership")) as ownership:
                        with self.assertRaisesRegex(ExtensionArtifactError,
                                                    "^wheel import root is unsupported$"):
                            verify_extension_artifacts(selection, site_packages=directory)
                        ownership.assert_not_called()

    def test_reserved_roots_must_be_identifiers(self):
        with tempfile.TemporaryDirectory(prefix="toolkit-host-root-") as work:
            directory = Path(work).resolve()
            base = self.selection(directory, "tkfixture_normal", dependency=False, single_module=False)
            for roots in (["tkfixture_hostroot"], ("not-a-root",), ("",), (1,)):
                with self.subTest(roots=roots):
                    selection = ExtensionArtifacts(base.primary, base.dependencies, roots)
                    with self.assertRaisesRegex(ExtensionArtifactError,
                                                "^reserved import roots must be identifiers$"):
                        verify_extension_artifacts(selection, site_packages=directory)

    def test_normal_name_reaches_import_ownership(self):
        class ReachedOwnership(Exception):
            pass

        for dependency in (False, True):
            for single_module in (False, True):
                with self.subTest(dependency=dependency, single_module=single_module):
                    with tempfile.TemporaryDirectory(prefix="toolkit-normal-root-") as work:
                        directory = Path(work).resolve()
                        selection = self.selection(directory, "tkfixture_normal", dependency=dependency,
                                                   single_module=single_module)
                        with patch.object(extension_owner, "_import_ownership",
                                          side_effect=ReachedOwnership) as ownership:
                            with self.assertRaises(ReachedOwnership):
                                verify_extension_artifacts(selection, site_packages=directory)
                            ownership.assert_called_once()


class ExtensionArtifactTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.base = tempfile.TemporaryDirectory(prefix="toolkit-extension-base-")
        cls.root = Path(cls.base.name).resolve()
        cls.wheels = cls.root / "wheels"
        cls.wheels.mkdir()
        cls.primary = wheel(cls.wheels, "tkfixture-extension", ROOTS[0], '''
import os
from pathlib import Path
Path(os.environ["TK_EXTENSION_SENTINEL"]).write_text("imported")
from tkfixture_one import calculate
class Extension:
    def __init__(self, services):
        self.services = services
    def measure(self):
        return calculate(self.services.get("measurement").read())
''', requires=("tkfixture-one==1.0",))
        cls.one = wheel(cls.wheels, "tkfixture-one", ROOTS[1],
                        "from tkfixture_two import OFFSET\ndef calculate(value): return value * 3 + OFFSET\n",
                        requires=("tkfixture-two==1.0",))
        cls.two = wheel(cls.wheels, "tkfixture-two", ROOTS[2], "OFFSET = 5\n")
        cls.site = cls.root / "site"
        command = [sys.executable, "-m", "pip", "install", "--disable-pip-version-check",
                   "--no-index", "--no-deps", "--target", str(cls.site),
                   *map(str, (cls.primary, cls.one, cls.two))]
        result = subprocess.run(command, text=True, capture_output=True, timeout=60,
                                env={**os.environ, "PIP_CONFIG_FILE": os.devnull})
        if result.returncode:
            raise AssertionError("local fixture installation failed: " + result.stdout + result.stderr)
        cls.install_command = command

    @classmethod
    def tearDownClass(cls):
        cls.base.cleanup()

    def setUp(self):
        self.work = tempfile.TemporaryDirectory(prefix="toolkit-extension-case-")
        self.addCleanup(self.work.cleanup)
        self.root = Path(self.work.name).resolve()
        self.site = self.root / "site"
        self.wheels = self.root / "wheels"
        shutil.copytree(type(self).site, self.site)
        shutil.copytree(type(self).wheels, self.wheels)
        artifacts = [SelectedWheel(self.wheels / original.name, digest(original))
                     for original in (self.primary, self.one, self.two)]
        self.selection = ExtensionArtifacts(artifacts[0], tuple(artifacts[1:]))
        self.marker = self.root / "imported"
        self.addCleanup(self.remove_modules)
        paths = patch.object(sys, "path", [str(self.site), *sys.path])
        paths.start()
        self.addCleanup(paths.stop)
        environment = patch.dict(os.environ, {"TK_EXTENSION_SENTINEL": str(self.marker)})
        environment.start()
        self.addCleanup(environment.stop)
        importlib.invalidate_caches()

    @staticmethod
    def remove_modules():
        for name in tuple(sys.modules):
            if any(name == root or name.startswith(root + ".") for root in ROOTS):
                del sys.modules[name]

    def load(self, **overrides):
        return load_extension_entrypoint(**{
            "selection": self.selection, "site_packages": self.site,
            "entrypoint": ROOTS[0] + ":Extension", "services": services(),
            "required_services": ("measurement",), **overrides})

    def refused(self, **overrides):
        with self.assertRaises(ExtensionArtifactError):
            self.load(**overrides)
        self.assertFalse(self.marker.exists(), "extension code ran before refusal")
        self.assertFalse(any(root in sys.modules for root in ROOTS))

    def test_installed_transitive_dependencies_and_scoped_service_work_without_paired_host(self):
        observed = verify_extension_artifacts(self.selection, site_packages=self.site)
        self.assertEqual(observed.distribution, "tkfixture-extension")
        self.assertEqual(len(observed.dependencies), 2)
        self.assertEqual(observed.primary_dist_info,
                         self.site / "tkfixture_extension-1.0.dist-info")
        self.assertEqual(observed.primary_requirements, ("tkfixture-one==1.0",))
        with zipfile.ZipFile(self.selection.primary.path) as archive:
            expected = tuple((name, hashlib.sha256(archive.read(name)).hexdigest())
                             for name in sorted(archive.namelist())
                             if not name.endswith("/RECORD"))
        self.assertEqual(observed.primary_files, expected)
        self.assertTrue(all(digest(self.site / name) == sha
                            for name, sha in observed.primary_files))
        self.assertFalse(self.marker.exists())
        view = services()
        implementation = self.load(services=view)
        self.assertEqual(implementation(view).measure(), 26)
        self.assertEqual(self.marker.read_text(), "imported")
        self.assertFalse(any(name == "paired_host" or name.startswith("paired_host.") for name in sys.modules))

    def test_fresh_standalone_host_uses_installed_extension(self):
        # Isolated mode prevents inherited PYTHONPATH, user site and cwd imports.
        source = Path(extension_owner.__file__).resolve().parents[1]
        script = '''
import json, os, pathlib, sys
sys.path[:0] = [sys.argv[1], sys.argv[2]]
from meerkat_toolkit.extension_artifacts import ExtensionArtifacts, SelectedWheel, load_extension_entrypoint
from meerkat_toolkit.host_services import StagedHostServices
selected = [SelectedWheel(pathlib.Path(row[0]), row[1]) for row in json.loads(sys.argv[3])]
view = StagedHostServices(declared=["measurement"], available=["measurement"])
class Measurement:
    def read(self): return 7
view.bind({"measurement": Measurement()})
cls = load_extension_entrypoint(ExtensionArtifacts(selected[0], tuple(selected[1:])),
    site_packages=pathlib.Path(sys.argv[2]), entrypoint="tkfixture_extension:Extension",
    services=view, required_services=["measurement"])
print(json.dumps({"measurement": cls(view).measure(), "paired_host_loaded": any(
    name == "paired_host" or name.startswith("paired_host.") for name in sys.modules)}))
'''
        selection = (self.selection.primary, *self.selection.dependencies)
        result = subprocess.run([sys.executable, "-I", "-c", script, str(source), str(self.site),
                                 json.dumps([(str(w.path), w.sha256) for w in selection])],
                                cwd=self.root, text=True, capture_output=True, timeout=20,
                                env={"PATH": os.environ.get("PATH", ""),
                                     "TK_EXTENSION_SENTINEL": str(self.marker)})
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(json.loads(result.stdout), {"measurement": 26, "paired_host_loaded": False})

    def test_changed_selected_wheel_is_refused(self):
        with self.selection.primary.path.open("ab") as stream:
            stream.write(b"changed")
        self.refused()

    def test_changed_installed_payload_is_refused_even_with_forged_record(self):
        target = self.site / ROOTS[0] / "__init__.py"
        target.write_text("raise AssertionError('must not run')\n")
        info = self.site / "tkfixture_extension-1.0.dist-info"
        current = {row[0]: (self.site / row[0]).read_bytes()
                   for row in csv.reader(io.StringIO((info / "RECORD").read_text()))
                   if row[0] != info.name + "/RECORD" and not row[0].endswith(".pyc")}
        (info / "RECORD").write_bytes(record(current, info.name + "/RECORD"))
        self.refused()

    def test_missing_record_entry_is_refused(self):
        path = self.site / "tkfixture_one-1.0.dist-info" / "RECORD"
        path.write_text("\n".join(path.read_text().splitlines()[1:]) + "\n")
        self.refused()

    def test_missing_transitive_dependency_is_refused(self):
        self.refused(selection=replace(self.selection, dependencies=self.selection.dependencies[:1]))

    def test_altered_dependency_is_refused(self):
        (self.site / ROOTS[2] / "__init__.py").write_text("OFFSET = 900\n")
        self.refused()

    def test_shadow_import_root_is_refused_even_after_selected_site(self):
        shadow = self.root / "shadow"
        (shadow / ROOTS[1]).mkdir(parents=True)
        (shadow / ROOTS[1] / "__init__.py").write_text("raise AssertionError('foreign')\n")
        sys.path.append(str(shadow))
        self.refused()

    def test_foreign_loaded_module_is_refused(self):
        sys.modules[ROOTS[2]] = ModuleType(ROOTS[2])
        with self.assertRaises(ExtensionArtifactError):
            self.load()
        self.assertFalse(self.marker.exists())

    def test_other_distribution_cannot_claim_selected_module(self):
        info = self.site / "unrelated-1.0.dist-info"
        info.mkdir()
        (info / "METADATA").write_text("Name: unrelated\nVersion: 1.0\n")
        (info / "RECORD").write_text("tkfixture_one/__init__.py,,\n")
        self.refused()

    def test_extra_code_or_symlink_in_selected_tree_is_refused(self):
        extra = self.site / ROOTS[0] / "extra.py"
        extra.write_text("raise AssertionError('extra')\n")
        self.refused()
        extra.unlink()
        extra.symlink_to(self.site / ROOTS[1] / "__init__.py")
        self.refused()

    def test_owned_timestamp_bytecode_is_removed_before_import(self):
        source = self.site / ROOTS[2] / "__init__.py"
        original, stamp = source.read_bytes(), source.stat().st_mtime
        source.write_bytes(b"OFFSET = 9\n")  # Same length and timestamp as original.
        os.utime(source, (stamp, stamp))
        py_compile.compile(str(source), doraise=True)
        source.write_bytes(original)
        os.utime(source, (stamp, stamp))
        cls = self.load()
        self.assertEqual(cls(services()).measure(), 26)

    def test_default_pip_caches_and_repeated_verification(self):
        recorded = (self.site / "tkfixture_extension-1.0.dist-info" / "RECORD").read_text()
        self.assertIn(".pyc,,", recorded)
        self.assertTrue(list(self.site.rglob("*.pyc")))
        verify_extension_artifacts(self.selection, site_packages=self.site)
        self.assertFalse(list(self.site.rglob("*.pyc")))
        cls = self.load()
        self.assertEqual(cls(services()).measure(), 26)
        self.assertIs(self.load(), cls)
        self.assertEqual(recorded, (self.site / "tkfixture_extension-1.0.dist-info" / "RECORD").read_text())

    def test_foreign_cache_record_is_not_ignored(self):
        path = self.site / "tkfixture_extension-1.0.dist-info" / "RECORD"
        with path.open("a") as stream:
            stream.write("tkfixture_extension/__pycache__/unowned.cpython-311.pyc,,\n")
        self.refused()

    def test_missing_or_unbound_required_service_refuses_before_import(self):
        for view in (services(bound=False), services(available=())):
            with self.subTest(ready=view.ready):
                self.refused(services=view)

    def test_entrypoint_cannot_point_to_dependency(self):
        self.refused(entrypoint="tkfixture_one:calculate")

    def test_selected_module_root_collision_is_refused(self):
        alternative = wheel(self.wheels, "tkfixture-two", ROOTS[1], "OFFSET = 5\n")
        selected = replace(self.selection, dependencies=(self.selection.dependencies[0],
                           SelectedWheel(alternative, digest(alternative))))
        self.refused(selection=selected)

    def test_bad_wheel_record_is_refused_even_when_archive_hash_matches(self):
        path = self.selection.primary.path
        with zipfile.ZipFile(path) as archive:
            files = {name: archive.read(name) for name in archive.namelist()}
        files[ROOTS[0] + "/__init__.py"] += b"\n# changed\n"
        with zipfile.ZipFile(path, "w") as archive:
            for name, data in files.items():
                archive.writestr(name, data)
        self.refused(selection=replace(self.selection, primary=SelectedWheel(path, digest(path))))


if __name__ == "__main__":
    unittest.main()
