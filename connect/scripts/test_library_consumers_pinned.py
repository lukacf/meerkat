"""Pinned-mode exporter checks: the macOS arm64 bundle from the pin file and components built in the run.

Every fixture is built in a temporary directory. Nothing is downloaded, installed or run, and no retained
record (acceptance, seed manifest, closure, acquisition program, adoption inventory) exists in the fixture.
"""
from __future__ import annotations

import base64
import contextlib
import copy
import hashlib
import importlib.util
import io
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tarfile
import tempfile
import unittest
from unittest.mock import patch
import zipfile

ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT / "scripts/export-library-consumers.py"
SPEC = importlib.util.spec_from_file_location("library_consumers_pinned", SCRIPT)
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)
INSTALLER_SPEC = importlib.util.spec_from_file_location(
    "consumer_install_for_pinned", ROOT / "scripts/install-library-consumer.py")
INSTALLER = importlib.util.module_from_spec(INSTALLER_SPEC)
INSTALLER_SPEC.loader.exec_module(INSTALLER)
ENV = {"PATH": "/usr/bin:/bin", "PYTHONDONTWRITEBYTECODE": "1", "PYTHONNOUSERSITE": "1"}
FAILURE = ("consumer export failed: check accepted inputs and new destination; "
           "any partial destination must not be used\n")
RUN = {"agno": "a" * 64, "activepieces": "b" * 64, "services": "c" * 64}


def sha(data):
    return hashlib.sha256(data).hexdigest()


def integrity(data):
    return "sha512-" + base64.b64encode(hashlib.sha512(data).digest()).decode()


def canonical(value):
    return json.dumps(value, ensure_ascii=True, sort_keys=True, separators=(",", ":")).encode()


def metadata_version(row):
    body = copy.deepcopy(row)
    del body["identity"]["version"]
    return "metadata-sha256:" + sha(canonical(body))


def wheel(name, version, requires=(), members=None, tag="py3-none-any"):
    distribution = name.replace("-", "_")
    output = io.BytesIO()
    with zipfile.ZipFile(output, "w") as archive:
        text = "Metadata-Version: 2.4\nName: %s\nVersion: %s\n" % (name, version)
        text += "".join("Requires-Dist: %s\n" % item for item in requires)
        archive.writestr("%s-%s.dist-info/METADATA" % (distribution, version), text)
        for path, data in (members or {}).items():
            archive.writestr(path, data)
    return "%s-%s-%s.whl" % (distribution, version, tag), output.getvalue()


def tarball(members):
    output = io.BytesIO()
    with tarfile.open(fileobj=output, mode="w:gz") as archive:
        for name, data in members.items():
            row = tarfile.TarInfo("package/" + name)
            row.size = len(data)
            archive.addfile(row, io.BytesIO(data))
    return output.getvalue()


def linux_routes(adopted):
    """The Linux routes the retained exporter compiles, rebuilt from the committed catalog's own text."""
    retained = {"agno": MODULE.AGNO_SHA, "activepieces": MODULE.AP_SHA}
    paths = {"agno": "agno/" + MODULE.AGNO_WHEEL, "activepieces": "activepieces/" + MODULE.AP_ARCHIVE,
             "services": "services/" + MODULE.SERVICES_WHEEL}
    digests = {**retained, "services": MODULE.SERVICES_SHA}
    routes, profiles = [], {family: set() for family in paths}
    for row in adopted:
        primary = row["strategies"][0]
        for family, digest in retained.items():
            if digest in primary["instructions"]:
                profile = re.search(r"^Select profile ([A-Za-z0-9_.-]+?)\.$", primary["instructions"], re.M)[1]
                routes.append({"identity": dict(row["identity"]), "strategy_id": primary["id"],
                               "family": family, "profile": profile})
                profiles[family].add(profile)
        for strategy in row["strategies"][1:]:
            if MODULE.SERVICES_SHA in strategy["instructions"]:
                routes.append({"identity": dict(row["identity"]), "strategy_id": strategy["id"],
                               "family": "services", "profile": row["identity"]["name"]})
                profiles["services"].add(row["identity"]["name"])
    return MODULE.encoded({
        "schema_version": 1, "catalog_sha256": "9" * 64, "runtime_target": MODULE.LINUX_RUNTIME_TARGET,
        "provenance": {"inventory": MODULE.ROUTE_SOURCES["inventory"],
                       "profile_captures": {key: MODULE.ROUTE_SOURCES[key] for key in paths}},
        "components": {family: {"path": paths[family], "sha256": digests[family],
                                "profiles": sorted(profiles[family])} for family in paths},
        "routes": routes})


class Fixture:
    """A checkout root, a pin file with its fetched directory, and three built components."""

    def __init__(self, base):
        self.base = base
        self.root = base / "checkout"
        self.pinned = base / "pins"
        self.built = base / "components"
        self.pins_path = base / "library-pins-macos-arm64.json"
        self.output = base / "consumers"
        self.closure = base / "dependency-closure-macos-arm64.json"
        for directory in (self.root, self.pinned, self.built):
            directory.mkdir()
        self.adopted_raw = (ROOT / MODULE.CATALOG).read_bytes()
        self.adopted = json.loads(self.adopted_raw)
        self.discovery_raw = (ROOT / MODULE.DISCOVERY_CATALOG).read_bytes()
        self.linux_routes = linux_routes(self.adopted)
        agno_profiles = json.loads(self.linux_routes)["components"]["agno"]["profiles"]
        self.rows = []
        # Python closure: `shared` belongs to both families, `native` is a macOS arm64 build.
        self.wheels = {}
        for name, version, tag in (("alpha", "1.0", "py3-none-any"), ("shared-lib", "2.0", "py2.py3-none-any"),
                                   ("native", "3.0", "cp311-cp311-macosx_11_0_arm64"), ("beta", "1.5", "py3-none-any")):
            filename, data = wheel(name, version, tag=tag)
            self.wheels[name] = (filename, data)
            self.add_pin("pypi-wheel", name, version, filename, data,
                         "https://files.pythonhosted.org/packages/fixture/" + filename)
        self.tarballs = {}
        for name, version, file in (("@scope/dep", "1.0.0", "dep-1.0.0.tgz"), ("plain", "2.0.0", "plain-2.0.0.tgz")):
            data = tarball({"package.json": MODULE.encoded({"name": name, "version": version})})
            self.tarballs[name] = (file, data)
            self.add_pin("npm-tarball", name, version, file, data,
                         "https://registry.npmjs.org/%s/-/%s" % (name, file), sha512=True)
        # Rows the exporter must leave alone.
        self.rows.append({"kind": "crate", "name": "serde", "version": "1.0.0", "file": "serde-1.0.0.crate",
                          "url": "https://static.crates.io/crates/serde/serde-1.0.0.crate", "sha256": "0" * 64,
                          "sha512": None, "digest_source": "crates.io index cksum", "use": ["tool build provenance"]})
        self.rows.append({"kind": "pypi-wheel", "name": "alpha", "version": "9.9", "file": "alpha-9.9-py3-none-any.whl",
                          "url": "https://files.pythonhosted.org/packages/fixture/alpha-9.9-py3-none-any.whl",
                          "sha256": "0" * 64, "sha512": None, "digest_source": "PyPI JSON API digests.sha256",
                          "use": ["component build tool"]})
        self.carried = {
            "install-library-consumer.py": b"# fixture installer\n",
            "agno/runtime-constraints.txt": b"alpha==1.0\nshared_lib==2.0\nnative==3.0\n",
            "services/runtime-constraints.txt": b"shared-lib==2.0\nbeta==1.5\n",
            "activepieces/package.json": MODULE.encoded({
                "name": "fixture-consumer", "version": "1.0.0", "private": True, "type": "module",
                "dependencies": {MODULE.AP_COMPONENT: "file:./" + MODULE.AP_ARCHIVE}}),
            "linux-routes.json": self.linux_routes,
            "MACOS-CONSUMERS.md": b"# fixture macOS guide\n",
        }
        dependencies = {"@scope/dep": "1.0.0", "plain": "2.0.0"}
        self.engines = {"node": ">=22.19.0"}
        packages = {"": {"dependencies": {MODULE.AP_COMPONENT: "file:./" + MODULE.AP_ARCHIVE},
                         "name": "fixture-consumer", "version": "1.0.0"},
                    "node_modules/" + MODULE.AP_COMPONENT: {
                        "bin": {"fixture": "dist/mcp-cli.js"}, "dependencies": dependencies, "engines": self.engines,
                        "integrity": integrity(b"the retained archive"), "license": "MIT OR Apache-2.0",
                        "resolved": "file:" + MODULE.AP_ARCHIVE, "version": "0.1.0"}}
        for name, (file, data) in self.tarballs.items():
            packages["node_modules/" + name] = {
                "integrity": integrity(data), "license": "MIT",
                "resolved": "https://registry.npmjs.org/%s/-/%s" % (name, file), "version": dependencies[name]}
        self.carried["activepieces/package-lock.json"] = MODULE.encoded({
            "lockfileVersion": 3, "name": "fixture-consumer", "packages": packages, "requires": True,
            "version": "1.0.0"})
        self.documents = {"agno": {"PROFILE-GUIDE.md": b"agno guide\n", "LICENSE-MIT": b"mit\n",
                                   "LICENSE-APACHE": b"apache\n", "LICENSE-Agno": b"agno licence\n",
                                   "THIRD_PARTY_NOTICES.md": b"agno notices\n"},
                          "services": {"PROFILE-GUIDE.md": b"services guide\n"},
                          "activepieces": {"PROFILE-GUIDE.md": b"activepieces guide\n", "LICENSE-MIT": b"mit\n",
                                           "LICENSE-APACHE": b"apache\n",
                                           "THIRD_PARTY_NOTICES.md": b"activepieces notices\n"}}
        agno_members = {"meerkat_toolkit_agno/PROFILE-GUIDE.md": self.documents["agno"]["PROFILE-GUIDE.md"],
                        "meerkat_toolkit_agno/CONFIGURATION-GUIDES.json": MODULE.encoded(
                            {"schema_version": 1, "profiles": {profile: {} for profile in agno_profiles}})}
        for name in ("LICENSE-MIT", "LICENSE-APACHE", "LICENSE-Agno", "THIRD_PARTY_NOTICES.md"):
            agno_members["meerkat_toolkit_agno-0.1.5.dist-info/licenses/" + name] = self.documents["agno"][name]
        self.agno_members = dict(agno_members)
        self.components = {}
        self.set_component("agno", wheel("meerkat-toolkit-agno", "0.1.5", ("alpha==1.0", "native==3.0"), agno_members)[1])
        self.set_component("services", wheel(
            "meerkat-toolkit-services", "0.1.23", ("shared-lib==2.0", 'extra-only==7.0; extra == "google-oauth"'),
            {"meerkat_toolkit_services/GUIDE.md": self.documents["services"]["PROFILE-GUIDE.md"]})[1])
        self.set_component("activepieces", tarball({
            "package.json": MODULE.encoded({"name": MODULE.AP_COMPONENT, "version": "0.1.0",
                                           "dependencies": dependencies, "engines": self.engines}),
            "README.md": self.documents["activepieces"]["PROFILE-GUIDE.md"],
            "LICENSE-MIT": self.documents["activepieces"]["LICENSE-MIT"],
            "LICENSE-APACHE": self.documents["activepieces"]["LICENSE-APACHE"],
            "THIRD_PARTY_NOTICES.md": self.documents["activepieces"]["THIRD_PARTY_NOTICES.md"]}))
        self.write()

    def add_pin(self, kind, name, version, file, data, url, sha512=False):
        self.rows.append({"kind": kind, "name": name, "version": version, "file": file, "url": url,
                          "sha256": None if sha512 else sha(data),
                          "sha512": hashlib.sha512(data).hexdigest() if sha512 else None,
                          "digest_source": "fixture", "use": ["agno closure"]})
        target = self.pinned / kind / file
        target.parent.mkdir(exist_ok=True)
        target.write_bytes(data)

    def set_component(self, family, data):
        name = {"agno": MODULE.AGNO_WHEEL, "services": MODULE.SERVICES_WHEEL, "activepieces": MODULE.AP_ARCHIVE}[family]
        (self.built / name).write_bytes(data)
        self.components[family] = (self.built / name, sha(data))

    def write(self):
        """(Re)write the checkout files and the pin file from the fixture's current values."""
        for relative, data in ((MODULE.CATALOG, self.adopted_raw), (MODULE.DISCOVERY_CATALOG, self.discovery_raw),
                               *((MODULE.CARRIED + "/" + name, data) for name, data in self.carried.items())):
            target = self.root / relative
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_bytes(data)
        self.pins_path.write_bytes(MODULE.encoded({
            "schema": "connect-library-pins/1", "target": "macos-arm64",
            "pins": sorted(self.rows, key=lambda row: (row["kind"], row["name"], row["version"], row["file"]))}))

    def carried_pins(self):
        return {MODULE.CARRIED + "/" + name: sha(data) for name, data in self.carried.items()
                if name != "MACOS-CONSUMERS.md"}

    def inputs(self, **changes):
        values = {"pins": self.pins_path, "pinned_inputs": self.pinned, "components": dict(self.components),
                  "closure_record": self.closure, "closure_record_key": None, "profile_captures": {}}
        values.update(changes)
        return MODULE.PinnedInputs(**values)

    def argv(self):
        argv = ["--root", str(self.root), "--pins", str(self.pins_path), "--pinned-inputs", str(self.pinned),
                "--closure-record", str(self.closure), "--output", str(self.output)]
        for family, option in (("agno", "agno-wheel"), ("services", "services-wheel"),
                               ("activepieces", "activepieces-archive")):
            argv += ["--" + option, str(self.components[family][0]),
                     "--" + family + "-sha256", self.components[family][1]]
        return argv


class PinnedCase(unittest.TestCase):
    def setUp(self):
        self.scratch = tempfile.TemporaryDirectory(prefix="toolkit-pinned-consumers-")
        self.addCleanup(self.scratch.cleanup)
        self.fixture = Fixture(Path(self.scratch.name).resolve())
        self.bind()

    def bind(self):
        """Point the exporter's carried-file pins at the fixture's files (the real pins are checked below)."""
        patcher = patch.object(MODULE, "CARRIED_PINS", self.fixture.carried_pins())
        patcher.start()
        self.addCleanup(patcher.stop)

    def export(self, **changes):
        return MODULE.export_pinned(self.fixture.root, str(self.fixture.output), self.fixture.inputs(**changes))

    def refuse(self, **changes):
        with self.assertRaises((MODULE.ExportError, OSError, KeyError, ValueError,
                                zipfile.BadZipFile, tarfile.TarError)):
            self.export(**changes)
        self.assertFalse(self.fixture.output.exists())
        self.assertFalse(self.fixture.closure.exists())


class PinnedExportTests(PinnedCase):
    def test_bundle_holds_the_run_components_the_pinned_closure_and_the_carried_files(self):
        summary = self.export()
        output = self.fixture.output
        contents_raw = (output / "CONTENTS.json").read_bytes()
        contents = json.loads(contents_raw)
        self.assertEqual(summary["mode"], "pinned")
        self.assertEqual(summary["scope"], "consumer_export_only_no_installation_or_activation")
        self.assertEqual(summary["contents_sha256"], sha(contents_raw))
        self.assertEqual(summary["components"], {family: digest for family, (_, digest) in self.fixture.components.items()})
        # The macOS contract of both installers: schema 1 and no runtime target.
        self.assertEqual(contents["schema_version"], 1)
        self.assertNotIn("runtime_target", contents)
        self.assertEqual(set(contents), {"schema_version", "scope", "accepted_inputs", "files"})
        members = {str(path.relative_to(output)) for path in output.rglob("*") if path.is_file()}
        self.assertEqual(members, set(contents["files"]) | {"CONTENTS.json", "SHA256SUMS"})
        self.assertEqual(summary["files"], len(members))
        wheels = {name: self.fixture.wheels[name][0] for name in self.fixture.wheels}
        self.assertEqual(members, {
            "README.md", "install-library-consumer.py", "catalog.json", "routes.json", "CONTENTS.json", "SHA256SUMS",
            "agno/" + MODULE.AGNO_WHEEL, "agno/" + wheels["alpha"], "agno/" + wheels["shared-lib"],
            "agno/" + wheels["native"], "agno/install-offline.txt", "agno/install-requirements.txt",
            "agno/runtime-constraints.txt", "agno/PROFILE-GUIDE.md", "agno/LICENSE-MIT", "agno/LICENSE-APACHE",
            "agno/LICENSE-Agno", "agno/THIRD_PARTY_NOTICES.md", "agno/hackernews.json",
            "services/" + MODULE.SERVICES_WHEEL, "services/" + wheels["shared-lib"], "services/" + wheels["beta"],
            "services/install-offline.txt", "services/install-requirements.txt", "services/runtime-constraints.txt",
            "services/PROFILE-GUIDE.md", "services/slack.json", "services/gmail.json",
            "activepieces/" + MODULE.AP_ARCHIVE, "activepieces/scope-dep-1.0.0.tgz", "activepieces/plain-2.0.0.tgz",
            "activepieces/package.json", "activepieces/package-lock.json", "activepieces/offline-cache.json",
            "activepieces/PROFILE-GUIDE.md", "activepieces/LICENSE-MIT", "activepieces/LICENSE-APACHE",
            "activepieces/THIRD_PARTY_NOTICES.md", "activepieces/tally.json"})
        for path, record in contents["files"].items():
            self.assertEqual(record["sha256"], sha((output / path).read_bytes()), path)
        checksums = dict(line.split("  ", 1)[::-1] for line in (output / "SHA256SUMS").read_text().splitlines())
        self.assertEqual(set(checksums), members - {"SHA256SUMS"})
        for path, expected in checksums.items():
            self.assertEqual(expected, sha((output / path).read_bytes()), path)
        self.assertEqual(summary["checksums_sha256"], sha((output / "SHA256SUMS").read_bytes()))
        for path in output.rglob("*"):
            self.assertFalse(path.is_symlink())
            self.assertEqual(path.stat().st_mode & 0o777, 0o700 if path.is_dir() else 0o600, path)
        # Components and dependencies are the given bytes.
        for family, (path, digest) in self.fixture.components.items():
            self.assertEqual((output / family / path.name).read_bytes(), path.read_bytes())
            self.assertEqual(contents["files"][family + "/" + path.name]["source"],
                             {"kind": "component_built_in_this_run", "sha256": digest})
        native = contents["files"]["agno/" + wheels["native"]]["source"]
        self.assertEqual(native, {"kind": "pinned_public_index_artifact", "pin": "pypi-wheel/" + wheels["native"],
                                  "url": "https://files.pythonhosted.org/packages/fixture/" + wheels["native"],
                                  "sha256": sha(self.fixture.wheels["native"][1])})
        self.assertEqual((output / "services" / wheels["shared-lib"]).read_bytes(), self.fixture.wheels["shared-lib"][1])
        self.assertEqual((output / "activepieces/scope-dep-1.0.0.tgz").read_bytes(), self.fixture.tarballs["@scope/dep"][1])
        # Carried files are copied; the guide comes from the checkout without a pin.
        for name in ("install-library-consumer.py", "agno/runtime-constraints.txt",
                     "services/runtime-constraints.txt", "activepieces/package.json"):
            self.assertEqual((output / name).read_bytes(), self.fixture.carried[name], name)
            self.assertEqual(contents["files"][name]["source"],
                             {"path": MODULE.CARRIED + "/" + name, "sha256": sha(self.fixture.carried[name])})
        self.assertEqual((output / "README.md").read_bytes(), self.fixture.carried["MACOS-CONSUMERS.md"])
        self.assertEqual(contents["files"]["README.md"]["source"], {
            "path": MODULE.MACOS_GUIDE, "base": "exporter_checkout", "kind": "consumer_instructions",
            "sha256": sha(self.fixture.carried["MACOS-CONSUMERS.md"])})
        # Documents are members of the run's components.
        for family, documents in self.fixture.documents.items():
            for name, data in documents.items():
                self.assertEqual((output / family / name).read_bytes(), data, family + "/" + name)
        self.assertEqual(json.loads((output / "agno/hackernews.json").read_text()),
                         {"profile": "hackernews", "operations": ["get_top_hackernews_stories"], "config": {"timeout": 10}})
        self.assertEqual(json.loads((output / "activepieces/tally.json").read_text()),
                         {"profile": "tally.get-form", "token_env": "MY_TALLY_TOKEN"})

    def test_no_retained_record_is_read_or_named(self):
        # The fixture checkout has no docs/ and no dist/ directory at all (M1, M2).
        self.assertFalse((self.fixture.root / "docs").exists())
        self.assertFalse((self.fixture.root / "dist").exists())
        self.export()
        contents = json.loads((self.fixture.output / "CONTENTS.json").read_bytes())
        closure_sha = sha(self.fixture.closure.read_bytes())
        self.assertEqual(contents["accepted_inputs"], {
            self.fixture.pins_path.name: sha(self.fixture.pins_path.read_bytes()),
            MODULE.CATALOG: MODULE.CATALOG_SHA,
            MODULE.DISCOVERY_CATALOG: MODULE.PINNED_DISCOVERY_CATALOG_SHA,
            **self.fixture.carried_pins(),
            self.fixture.closure.name: closure_sha})
        named = json.dumps(contents["accepted_inputs"]) + json.dumps(
            {path: row["source"] for path, row in contents["files"].items() if path != "routes.json"})
        for word in ("docs/verification", "dist/", "acceptance", "seed-manifest", "acquire.py", "oss-inventory"):
            self.assertNotIn(word, named)
        for name in MODULE.PINS:
            self.assertNotIn(name, contents["accepted_inputs"])

    def test_pin_file_inside_the_checkout_is_named_by_its_relative_path(self):
        inside = self.fixture.root / "release/library-pins-macos-arm64.json"
        inside.parent.mkdir()
        inside.write_bytes(self.fixture.pins_path.read_bytes())
        self.export(pins=inside)
        contents = json.loads((self.fixture.output / "CONTENTS.json").read_bytes())
        self.assertEqual(contents["accepted_inputs"]["release/library-pins-macos-arm64.json"], sha(inside.read_bytes()))

    def test_requirement_lists_and_npm_inputs_satisfy_the_installer_closure_rules(self):
        summary = self.export()
        output = self.fixture.output
        wheels = {name: self.fixture.wheels[name] for name in self.fixture.wheels}
        agno = (output / "agno/install-offline.txt").read_text().splitlines()
        self.assertEqual(agno, sorted([
            "alpha==1.0 --hash=sha256:" + sha(wheels["alpha"][1]),
            "meerkat-toolkit-agno==0.1.5 --hash=sha256:" + self.fixture.components["agno"][1],
            "native==3.0 --hash=sha256:" + sha(wheels["native"][1]),
            "shared-lib==2.0 --hash=sha256:" + sha(wheels["shared-lib"][1])]))
        self.assertEqual((output / "agno/install-requirements.txt").read_text(), "".join(
            "./" + name + "\n" for name in sorted([MODULE.AGNO_WHEEL, wheels["alpha"][0], wheels["native"][0], wheels["shared-lib"][0]])))
        self.assertEqual((output / "services/install-requirements.txt").read_text(), "".join(
            "./" + name + "\n" for name in sorted([MODULE.SERVICES_WHEEL, wheels["beta"][0], wheels["shared-lib"][0]])))
        contents = json.loads((output / "CONTENTS.json").read_bytes())
        closure_sha = sha(self.fixture.closure.read_bytes())
        for family in ("agno", "services"):
            self.assertEqual(contents["files"][family + "/install-requirements.txt"]["source"],
                             {"kind": "wheel_list_from_selected_macos_closure", "closure_sha256": closure_sha})
            self.assertEqual(contents["files"][family + "/install-offline.txt"]["source"],
                             {"kind": "hash_locked_requirements_from_selected_closure", "closure_sha256": closure_sha})
        # The checkout's installer reads the bundle the way a consumer's installation does (nothing is installed).
        for family, expected in (("agno", {"alpha": "1.0", "native": "3.0", "shared-lib": "2.0", "meerkat-toolkit-agno": "0.1.5"}),
                                 ("services", {"beta": "1.5", "shared-lib": "2.0", "meerkat-toolkit-services": "0.1.23"})):
            files, target = INSTALLER.inputs(output, summary["contents_sha256"], family)
            self.assertIsNone(target)
            self.assertEqual(INSTALLER.python_closure(files, family), expected)
        files, target = INSTALLER.inputs(output, summary["contents_sha256"], "activepieces")
        self.assertIsNone(target)
        rows = INSTALLER.npm_closure(files)
        self.assertEqual([row["name"] for row in rows], ["@scope/dep", "plain"])
        cache = json.loads((output / "activepieces/offline-cache.json").read_bytes())
        self.assertEqual(cache, {"schema_version": 1, "tarballs": [
            {"name": name, "version": version, "path": path, "sha256": sha(self.fixture.tarballs[name][1]),
             "integrity": integrity(self.fixture.tarballs[name][1])}
            for name, version, path in (("@scope/dep", "1.0.0", "scope-dep-1.0.0.tgz"), ("plain", "2.0.0", "plain-2.0.0.tgz"))]})

    def test_lock_changes_only_the_component_integrity(self):
        self.export()
        carried = json.loads(self.fixture.carried["activepieces/package-lock.json"])
        derived_raw = (self.fixture.output / "activepieces/package-lock.json").read_bytes()
        derived = json.loads(derived_raw)
        archive = self.fixture.components["activepieces"][0].read_bytes()
        key = "node_modules/" + MODULE.AP_COMPONENT
        self.assertEqual(derived["packages"][key]["integrity"], integrity(archive))
        self.assertNotEqual(carried["packages"][key]["integrity"], integrity(archive))
        derived["packages"][key]["integrity"] = carried["packages"][key]["integrity"]
        self.assertEqual(derived, carried)
        self.assertEqual(derived_raw, MODULE.encoded(json.loads(derived_raw)))
        contents = json.loads((self.fixture.output / "CONTENTS.json").read_bytes())
        self.assertEqual(contents["files"]["activepieces/package-lock.json"]["source"], {
            "kind": "consumer_lock_with_this_run_component_integrity",
            "path": MODULE.CARRIED + "/activepieces/package-lock.json",
            "carried_sha256": sha(self.fixture.carried["activepieces/package-lock.json"]),
            "component_sha256": self.fixture.components["activepieces"][1]})

    def test_closure_record_is_generated_from_the_pins_and_bound_by_the_bundle(self):
        summary = self.export(closure_record_key="closure/macos.json")
        raw = self.fixture.closure.read_bytes()
        record = json.loads(raw)
        output = self.fixture.output
        self.assertEqual(summary["closure_record"], {"path": str(self.fixture.closure), "key": "closure/macos.json",
                                                     "sha256": sha(raw)})
        contents = json.loads((output / "CONTENTS.json").read_bytes())
        self.assertEqual(contents["accepted_inputs"]["closure/macos.json"], sha(raw))
        # The fields the library packager's closure check reads.
        self.assertEqual(record["schema_version"], 1)
        self.assertEqual(record["scope"], "dependency_bytes_for_existing_consumer_selection")
        self.assertEqual(record["python_platform"], {"implementation": "cpython", "version": "3.11",
                                                     "system": "Darwin", "machine": "arm64"})
        required = {"agno/runtime-constraints.txt", "services/runtime-constraints.txt", "activepieces/package.json",
                    "activepieces/package-lock.json", "agno/" + MODULE.AGNO_WHEEL,
                    "services/" + MODULE.SERVICES_WHEEL, "activepieces/" + MODULE.AP_ARCHIVE}
        self.assertEqual(record["accepted_inputs"], {name: sha((output / name).read_bytes()) for name in required})
        self.assertEqual(record["pins"], {"name": self.fixture.pins_path.name, "sha256": sha(self.fixture.pins_path.read_bytes())})
        rows = {(row["ecosystem"], row["name"]): row for row in record["artifacts"]}
        self.assertEqual(sorted(rows), [("npm", "@scope/dep"), ("npm", "plain"), ("python", "alpha"),
                                        ("python", "beta"), ("python", "native"), ("python", "shared-lib")])
        self.assertEqual(record["artifacts"], [rows[key] for key in sorted(rows)])
        shared = self.fixture.wheels["shared-lib"]
        self.assertEqual(rows[("python", "shared-lib")], {
            "ecosystem": "python", "name": "shared-lib", "version": "2.0", "filename": shared[0],
            "sha256": sha(shared[1]), "bytes": len(shared[1]), "families": ["agno", "services"],
            "path": "pypi-wheel/" + shared[0],
            "source": {"kind": "pinned_public_index_artifact",
                       "url": "https://files.pythonhosted.org/packages/fixture/" + shared[0]}})
        dep = self.fixture.tarballs["@scope/dep"]
        self.assertEqual(rows[("npm", "@scope/dep")], {
            "ecosystem": "npm", "name": "@scope/dep", "version": "1.0.0", "filename": "scope-dep-1.0.0.tgz",
            "sha256": sha(dep[1]), "bytes": len(dep[1]), "families": ["activepieces"],
            "integrity": integrity(dep[1]), "path": "npm-tarball/dep-1.0.0.tgz",
            "source": {"kind": "pinned_public_index_artifact", "url": "https://registry.npmjs.org/@scope/dep/-/dep-1.0.0.tgz"}})
        self.assertEqual(self.fixture.closure.stat().st_mode & 0o777, 0o600)

    def test_catalog_is_the_committed_one_with_the_run_digests_and_the_repository_discovery_rows(self):
        summary = self.export()
        output = self.fixture.output
        catalog_raw = (output / "catalog.json").read_bytes()
        catalog = json.loads(catalog_raw)
        adopted, discovery = self.fixture.adopted, json.loads(self.fixture.discovery_raw)
        self.assertEqual(len(adopted), 101)
        self.assertEqual(len(catalog), len(adopted) + len(discovery))
        self.assertEqual(catalog[len(adopted):], discovery)
        self.assertEqual(summary["catalog_sha256"], sha(catalog_raw))
        run = {family: digest for family, (_, digest) in self.fixture.components.items()}
        retained = {"agno": MODULE.AGNO_SHA, "activepieces": MODULE.AP_SHA, "services": MODULE.SERVICES_SHA}
        text = catalog_raw.decode()
        for family in retained:
            self.assertNotIn(retained[family], text)
        changed = 0
        for before, after in zip(adopted, catalog):
            # Every entry's version is the digest of its own metadata, as the catalog's generator computes it.
            self.assertEqual(after["identity"]["version"], metadata_version(after))
            expected = json.loads(json.dumps(before).replace(retained["agno"], run["agno"]).replace(
                retained["activepieces"], run["activepieces"]).replace(retained["services"], run["services"]))
            expected["identity"]["version"] = after["identity"]["version"]
            self.assertEqual(after, expected)
            changed += before != after
        self.assertEqual(changed, 99)
        self.assertEqual(summary["catalog_entries_changed"], 99)
        for name in ("github", "playwright"):
            self.assertEqual(next(row for row in catalog if row["identity"]["name"] == name),
                             next(row for row in adopted if row["identity"]["name"] == name))
        contents = json.loads((output / "CONTENTS.json").read_bytes())
        self.assertEqual(contents["files"]["catalog.json"]["source"], {"kind": "composed_catalog_metadata", "sources": [
            {"path": MODULE.CATALOG, "sha256": MODULE.CATALOG_SHA, "scope": "adopted", "entries": 101,
             "derivation": {"kind": "component_digests_of_this_run", "entries_changed": 99}},
            {"path": MODULE.DISCOVERY_CATALOG, "sha256": MODULE.PINNED_DISCOVERY_CATALOG_SHA,
             "scope": "discovery", "entries": len(discovery)}]})

    def test_routes_differ_from_the_linux_file_only_in_target_and_digest_values(self):
        summary = self.export()
        output = self.fixture.output
        linux = json.loads(self.fixture.linux_routes)
        routes_raw = (output / "routes.json").read_bytes()
        mac = json.loads(routes_raw)
        catalog_raw = (output / "catalog.json").read_bytes()
        entries = {row["identity"]["name"]: row for row in json.loads(catalog_raw)[:101]}
        run = {family: digest for family, (_, digest) in self.fixture.components.items()}
        self.assertEqual(summary["routes_sha256"], sha(routes_raw))
        self.assertEqual(routes_raw, MODULE.encoded(mac))
        self.assertEqual(set(mac), set(linux))
        # 1. The target field.
        self.assertEqual(linux["runtime_target"], MODULE.LINUX_RUNTIME_TARGET)
        self.assertIsNone(mac["runtime_target"])
        # 2. The component digests.
        for family, component in linux["components"].items():
            self.assertEqual(mac["components"][family], {**component, "sha256": run[family]})
        # 3. Digests over text that names a component digest: the catalog digest and the entry versions.
        self.assertEqual(mac["catalog_sha256"], sha(catalog_raw))
        self.assertEqual(len(mac["routes"]), len(linux["routes"]))
        self.assertEqual(len(mac["routes"]), 103)
        for before, after in zip(linux["routes"], mac["routes"]):
            entry = entries[before["identity"]["name"]]
            self.assertEqual(after["identity"]["version"], entry["identity"]["version"])
            self.assertEqual(after["identity"]["version"], metadata_version(entry))
            self.assertNotEqual(after["identity"]["version"], before["identity"]["version"])
        # Nothing else: with those values put back the two documents are equal, value by value.
        restored = copy.deepcopy(mac)
        restored["runtime_target"] = linux["runtime_target"]
        restored["catalog_sha256"] = linux["catalog_sha256"]
        for family in restored["components"]:
            restored["components"][family]["sha256"] = linux["components"][family]["sha256"]
        for before, after in zip(linux["routes"], restored["routes"]):
            after["identity"]["version"] = before["identity"]["version"]
        self.assertEqual(restored, linux)
        self.assertEqual(MODULE.encoded(restored), self.fixture.linux_routes)
        contents = json.loads((output / "CONTENTS.json").read_bytes())
        self.assertEqual(contents["files"]["routes.json"]["source"], {
            "kind": "derived_from_public_linux_routes", "path": MODULE.LINUX_ROUTES,
            "sha256": sha(self.fixture.linux_routes),
            "substituted": ["runtime_target", "components.sha256", "routes.identity.version", "catalog_sha256"]})
        # The setup reader's binding: every route names an entry and a strategy of this bundle's catalog.
        identities = {tuple(row["identity"][key] for key in ("publisher", "name", "version")): row
                      for row in json.loads(catalog_raw)}
        for route in mac["routes"]:
            entry = identities[tuple(route["identity"][key] for key in ("publisher", "name", "version"))]
            self.assertEqual(sum(strategy["id"] == route["strategy_id"] for strategy in entry["strategies"]), 1)
            self.assertIn(route["profile"], mac["components"][route["family"]]["profiles"])

    def test_cli_prints_the_summary_and_writes_the_same_bundle(self):
        stdout = io.StringIO()
        with patch.object(sys, "argv", [str(SCRIPT), *self.fixture.argv()]), contextlib.redirect_stdout(stdout):
            self.assertEqual(MODULE.main(), 0)
        summary = json.loads(stdout.getvalue())
        self.assertEqual(summary["output"], str(self.fixture.output))
        self.assertEqual(summary["mode"], "pinned")
        self.assertEqual(summary["contents_sha256"], sha((self.fixture.output / "CONTENTS.json").read_bytes()))
        self.assertEqual(summary["closure_record"]["key"], self.fixture.closure.name)

    def test_export_opens_no_socket_and_starts_no_process(self):
        with patch("socket.socket", side_effect=AssertionError("no network")), \
                patch("subprocess.Popen", side_effect=AssertionError("no child process")), \
                patch("os.system", side_effect=AssertionError("no child process")):
            self.export()
        self.assertTrue((self.fixture.output / "SHA256SUMS").is_file())

    def test_profile_captures_must_equal_the_routes_lists(self):
        routes = json.loads(self.fixture.linux_routes)
        captures = {}
        for family in ("agno", "services", "activepieces"):
            path = self.fixture.base / (family + "-profiles.json")
            path.write_bytes(MODULE.encoded({"profiles": [{"profile": name, "operations": []}
                                                           for name in routes["components"][family]["profiles"]]}))
            captures[family] = path
        self.export(profile_captures=captures)
        self.assertTrue((self.fixture.output / "SHA256SUMS").is_file())


class PinnedRefusalTests(PinnedCase):
    def test_component_digest_name_or_content_mismatch_refuses(self):
        path, digest = self.fixture.components["agno"]
        with self.subTest("digest"):
            self.refuse(components={**self.fixture.components, "agno": (path, "0" * 64)})
        with self.subTest("name"):
            other = path.with_name("meerkat_toolkit_agno-0.1.6-py3-none-any.whl")
            other.write_bytes(path.read_bytes())
            self.refuse(components={**self.fixture.components, "agno": (other, digest)})
        with self.subTest("relative path"):
            self.refuse(components={**self.fixture.components, "agno": (Path(path.name), digest)})
        with self.subTest("symlink"):
            link = self.fixture.base / "linked" / path.name
            link.parent.mkdir()
            link.symlink_to(path)
            self.refuse(components={**self.fixture.components, "agno": (link, digest)})
        with self.subTest("missing family"):
            self.refuse(components={key: value for key, value in self.fixture.components.items() if key != "services"})

    def test_component_requirement_outside_the_pinned_constraints_refuses(self):
        members = self.fixture.agno_members
        for label, requires in (("unknown", ("gamma==1.0",)), ("other version", ("alpha==1.1",)),
                                ("range", ("alpha>=1.0",)), ("direct url", ("alpha @ https://example.invalid/a.whl",))):
            with self.subTest(label):
                self.fixture.set_component("agno", wheel("meerkat-toolkit-agno", "0.1.5", requires, members)[1])
                self.refuse()
        # The same wheel with a requirement the constraints do hold is accepted.
        self.fixture.set_component("agno", wheel("meerkat-toolkit-agno", "0.1.5", ("Alpha==1.0",), members)[1])
        self.export()

    def test_component_metadata_documents_and_profile_list_are_checked(self):
        routes = json.loads(self.fixture.linux_routes)
        guides = MODULE.encoded({"schema_version": 1, "profiles": {
            profile: {} for profile in routes["components"]["agno"]["profiles"]}})
        licences = {"meerkat_toolkit_agno-0.1.5.dist-info/licenses/" + name: b"text\n"
                    for name in ("LICENSE-MIT", "LICENSE-APACHE", "LICENSE-Agno", "THIRD_PARTY_NOTICES.md")}
        complete = {"meerkat_toolkit_agno/PROFILE-GUIDE.md": b"guide\n",
                    "meerkat_toolkit_agno/CONFIGURATION-GUIDES.json": guides, **licences}
        cases = {
            "wrong version": wheel("meerkat-toolkit-agno", "0.1.6", (), complete)[1],
            "wrong name": wheel("meerkat-toolkit-other", "0.1.5", (), complete)[1],
            "missing guide": wheel("meerkat-toolkit-agno", "0.1.5", (), {
                key: value for key, value in complete.items() if not key.endswith("PROFILE-GUIDE.md")})[1],
            "missing licence": wheel("meerkat-toolkit-agno", "0.1.5", (), {
                key: value for key, value in complete.items() if not key.endswith("LICENSE-Agno")})[1],
            "profile added": wheel("meerkat-toolkit-agno", "0.1.5", (), {**complete,
                "meerkat_toolkit_agno/CONFIGURATION-GUIDES.json": MODULE.encoded({"profiles": {
                    **json.loads(guides)["profiles"], "new-profile": {}}})})[1],
            "not a wheel": b"not a zip archive",
        }
        for label, data in cases.items():
            with self.subTest(label):
                self.fixture.set_component("agno", data)
                self.refuse()
        self.fixture.set_component("agno", wheel("meerkat-toolkit-agno", "0.1.5", (), complete)[1])
        with self.subTest("services guide"):
            self.fixture.set_component("services", wheel("meerkat-toolkit-services", "0.1.23", (), {})[1])
            self.refuse()

    def test_activepieces_archive_must_agree_with_the_carried_lock(self):
        documents = {"README.md": b"guide\n", "LICENSE-MIT": b"mit\n", "LICENSE-APACHE": b"apache\n",
                     "THIRD_PARTY_NOTICES.md": b"notices\n"}
        base = {"name": MODULE.AP_COMPONENT, "version": "0.1.0",
                "dependencies": {"@scope/dep": "1.0.0", "plain": "2.0.0"}, "engines": self.fixture.engines}
        cases = {"dependency added": {**base, "dependencies": {**base["dependencies"], "extra": "1.0.0"}},
                 "dependency version": {**base, "dependencies": {"@scope/dep": "1.0.1", "plain": "2.0.0"}},
                 "engines": {**base, "engines": {"node": ">=24"}},
                 "version": {**base, "version": "0.1.1"}, "name": {**base, "name": "@other/package"}}
        for label, package in cases.items():
            with self.subTest(label):
                self.fixture.set_component("activepieces", tarball({"package.json": MODULE.encoded(package), **documents}))
                self.refuse()
        with self.subTest("missing notices"):
            self.fixture.set_component("activepieces", tarball({
                "package.json": MODULE.encoded(base), "README.md": b"guide\n", "LICENSE-MIT": b"mit\n",
                "LICENSE-APACHE": b"apache\n"}))
            self.refuse()

    def test_pin_rows_and_fetched_files_are_checked(self):
        alpha = next(row for row in self.fixture.rows if row["name"] == "alpha" and row["version"] == "1.0")
        dep = next(row for row in self.fixture.rows if row["name"] == "@scope/dep")
        native = next(row for row in self.fixture.rows if row["name"] == "native")

        def restore(row, saved):
            row.clear()
            row.update(saved)
            self.fixture.write()

        with self.subTest("missing row"):
            self.fixture.rows.remove(alpha)
            self.fixture.write()
            self.refuse()
            self.fixture.rows.append(alpha)
            self.fixture.write()
        with self.subTest("two rows for one selection"):
            twin = {**alpha, "file": "alpha-1.0-py2.py3-none-any.whl"}
            self.fixture.rows.append(twin)
            self.fixture.write()
            self.refuse()
            self.fixture.rows.remove(twin)
            self.fixture.write()
        for label, change in (("digest", {"sha256": "0" * 64}), ("no digest", {"sha256": None}),
                              ("unsafe file name", {"file": "../alpha-1.0-py3-none-any.whl"})):
            with self.subTest(label):
                saved = dict(alpha)
                alpha.update(change)
                self.fixture.write()
                self.refuse()
                restore(alpha, saved)
        with self.subTest("fetched file changed"):
            target = self.fixture.pinned / "pypi-wheel" / alpha["file"]
            saved = target.read_bytes()
            target.write_bytes(saved + b"changed")
            self.refuse()
            target.write_bytes(saved)
        with self.subTest("fetched file is a symlink"):
            target = self.fixture.pinned / "pypi-wheel" / alpha["file"]
            saved = self.fixture.base / "saved.whl"
            target.rename(saved)
            target.symlink_to(saved)
            self.refuse()
            target.unlink()
            saved.rename(target)
        with self.subTest("fetched file missing"):
            target = self.fixture.pinned / "npm-tarball" / dep["file"]
            saved = target.read_bytes()
            target.unlink()
            self.refuse()
            target.write_bytes(saved)
        with self.subTest("npm digest is not the lock's"):
            other = tarball({"package.json": b"{}"})
            target = self.fixture.pinned / "npm-tarball" / dep["file"]
            saved_bytes, saved_row = target.read_bytes(), dict(dep)
            target.write_bytes(other)
            dep["sha512"] = hashlib.sha512(other).hexdigest()
            self.fixture.write()
            self.refuse()
            target.write_bytes(saved_bytes)
            restore(dep, saved_row)
        with self.subTest("npm url is not the lock's"):
            saved = dict(dep)
            dep["url"] = "https://registry.npmjs.org/@scope/dep/-/dep-1.0.1.tgz"
            self.fixture.write()
            self.refuse()
            restore(dep, saved)
        with self.subTest("wheel holds another distribution"):
            target = self.fixture.pinned / "pypi-wheel" / alpha["file"]
            saved_bytes, saved_row = target.read_bytes(), dict(alpha)
            other = wheel("omega", "1.0")[1]
            target.write_bytes(other)
            alpha["sha256"] = sha(other)
            self.fixture.write()
            self.refuse()
            target.write_bytes(saved_bytes)
            restore(alpha, saved_row)
        with self.subTest("wheel for another platform"):
            saved_row = dict(native)
            linux_name = "native-3.0-cp311-cp311-manylinux_2_28_x86_64.whl"
            (self.fixture.pinned / "pypi-wheel" / linux_name).write_bytes(self.fixture.wheels["native"][1])
            native["file"] = linux_name
            self.fixture.write()
            self.refuse()
            restore(native, saved_row)
        # The fixture is whole again.
        self.export()

    def test_pin_file_shape_is_checked(self):
        document = json.loads(self.fixture.pins_path.read_bytes())
        for label, change in (("schema", {"schema": "connect-library-pins/2"}), ("target", {"target": "linux-x86_64"}),
                              ("pins", {"pins": {}}), ("row", {"pins": [*document["pins"], "not a row"]})):
            with self.subTest(label):
                self.fixture.pins_path.write_bytes(MODULE.encoded({**document, **change}))
                self.refuse()
        self.fixture.pins_path.write_bytes(b"not json")
        self.refuse()

    def test_changed_carried_file_or_catalog_refuses(self):
        names = [MODULE.CARRIED + "/" + name for name in self.fixture.carried if name != "MACOS-CONSUMERS.md"]
        for relative in [*names, MODULE.CATALOG, MODULE.DISCOVERY_CATALOG]:
            with self.subTest(relative=relative):
                target = self.fixture.root / relative
                saved = target.read_bytes()
                target.write_bytes(saved + b"\n")
                self.refuse()
                target.write_bytes(saved)
        with self.subTest("guide missing"):
            (self.fixture.root / MODULE.MACOS_GUIDE).unlink()
            self.refuse()

    def test_constraints_must_be_exact_local_pins(self):
        valid = self.fixture.carried["agno/runtime-constraints.txt"]
        for label, line in (("url", b"gamma @ https://example.invalid/gamma.whl\n"), ("range", b"gamma>=1.0\n"),
                            ("duplicate", b"Alpha==1.0\n")):
            with self.subTest(label):
                self.fixture.carried["agno/runtime-constraints.txt"] = valid + line
                self.fixture.write()
                with patch.object(MODULE, "CARRIED_PINS", self.fixture.carried_pins()):
                    self.refuse()

    def test_linux_routes_must_be_the_retained_linux_shape(self):
        original = json.loads(self.fixture.linux_routes)

        def attempt(label, mutate):
            with self.subTest(label):
                routes = copy.deepcopy(original)
                mutate(routes)
                self.fixture.carried["linux-routes.json"] = MODULE.encoded(routes)
                self.fixture.write()
                with patch.object(MODULE, "CARRIED_PINS", self.fixture.carried_pins()):
                    self.refuse()

        attempt("another component digest", lambda r: r["components"]["agno"].update(sha256="d" * 64))
        attempt("another component path", lambda r: r["components"]["agno"].update(path="agno/other.whl"))
        attempt("official family", lambda r: r["components"].update({"official-github": {"path": "x", "sha256": "0" * 64, "profiles": ["p"]}}))
        attempt("schema", lambda r: r.update(schema_version=2))
        attempt("already macOS", lambda r: r.update(runtime_target=None))
        attempt("unknown key", lambda r: r.update(extra=True))
        attempt("unknown catalog entry", lambda r: r["routes"][0]["identity"].update(name="not-in-the-catalog"))
        attempt("another catalog revision", lambda r: r["routes"][0]["identity"].update(version="metadata-sha256:" + "0" * 64))
        attempt("unknown strategy", lambda r: r["routes"][0].update(strategy_id="not-a-strategy"))
        attempt("unknown profile", lambda r: r["routes"][0].update(profile="not-a-profile"))
        attempt("unknown family", lambda r: r["routes"][0].update(family="other"))
        attempt("duplicate route", lambda r: r["routes"].append(copy.deepcopy(r["routes"][0])))
        attempt("unsorted profiles", lambda r: r["components"]["agno"]["profiles"].reverse())

    def test_profile_capture_that_differs_from_the_routes_refuses(self):
        routes = json.loads(self.fixture.linux_routes)
        path = self.fixture.base / "services-profiles.json"
        names = routes["components"]["services"]["profiles"]
        for label, profiles in (("missing", names[:-1]), ("added", [*names, "another"]), ("duplicate", [*names, names[0]])):
            with self.subTest(label):
                path.write_bytes(MODULE.encoded({"profiles": [{"profile": name} for name in profiles]}))
                self.refuse(profile_captures={"services": path})
        with self.subTest("unknown family"):
            self.refuse(profile_captures={"official-github": path})

    def test_output_and_closure_record_locations_are_checked(self):
        fixture = self.fixture
        for label, output in (("relative", "consumers"), ("dot", str(fixture.base) + "/./consumers"),
                              ("missing parent", str(fixture.base / "missing/consumers")),
                              ("inside the carried files", str(fixture.root / MODULE.CARRIED / "export")),
                              ("inside the catalogs", str(fixture.root / Path(MODULE.CATALOG).parent / "export")),
                              ("inside the fetched inputs", str(fixture.pinned / "export"))):
            with self.subTest(label):
                with self.assertRaises((MODULE.ExportError, OSError)):
                    MODULE.export_pinned(fixture.root, output, fixture.inputs())
                self.assertFalse(fixture.closure.exists())
        with self.subTest("existing output"):
            fixture.output.mkdir()
            (fixture.output / "owned.txt").write_bytes(b"keep me")
            with self.assertRaises((MODULE.ExportError, OSError)):
                self.export()
            self.assertEqual([path.name for path in fixture.output.iterdir()], ["owned.txt"])
            self.assertFalse(fixture.closure.exists())
            (fixture.output / "owned.txt").unlink()
            fixture.output.rmdir()
        for label, closure in (("inside the output", fixture.output / "closure.json"),
                               ("relative", Path("closure.json")),
                               ("missing parent", fixture.base / "missing/closure.json"),
                               ("inside the fetched inputs", fixture.pinned / "closure.json")):
            with self.subTest(label):
                with self.assertRaises((MODULE.ExportError, OSError)):
                    self.export(closure_record=closure)
                self.assertFalse(fixture.output.exists())
        with self.subTest("existing closure record"):
            fixture.closure.write_bytes(b"keep me")
            with self.assertRaises((MODULE.ExportError, OSError)):
                self.export()
            self.assertEqual(fixture.closure.read_bytes(), b"keep me")
            self.assertFalse(fixture.output.exists())

    def test_interrupted_write_leaves_no_completion_marker(self):
        original = MODULE.write_file
        calls = 0

        def interrupted(directory, name, data):
            nonlocal calls
            calls += 1
            if calls == 5:
                raise OSError("simulated disk failure")
            return original(directory, name, data)

        with patch.object(MODULE, "write_file", side_effect=interrupted):
            with self.assertRaises(OSError):
                self.export()
        self.assertTrue(self.fixture.output.is_dir())
        self.assertFalse((self.fixture.output / "SHA256SUMS").exists())

    def test_cli_refusals_print_the_fixed_line_and_write_nothing(self):
        fixture = self.fixture

        def run(argv):
            return subprocess.run([sys.executable, "-I", "-B", str(SCRIPT), *argv], env=ENV,
                                  capture_output=True, text=True, timeout=60)

        complete = fixture.argv()
        cases = {"linux target": [*complete, "--target", "linux-x86_64-cpython311"],
                 # The subprocess has the real carried pins, which the fixture's files do not have.
                 "carried files differ from the real pins": complete}
        for option in ("--pinned-inputs", "--agno-wheel", "--agno-sha256", "--services-wheel", "--services-sha256",
                       "--activepieces-archive", "--activepieces-sha256", "--closure-record"):
            index = complete.index(option)
            cases["without " + option] = complete[:index] + complete[index + 2:]
        index = complete.index("--pins")
        cases["pinned options without --pins"] = complete[:index] + complete[index + 2:]
        cases["bad capture"] = [*complete, "--profile-capture", "services"]
        for label, argv in cases.items():
            with self.subTest(label):
                result = run(argv)
                self.assertEqual(result.returncode, 2, result.stdout + result.stderr)
                self.assertEqual(result.stdout, "")
                self.assertEqual(result.stderr, FAILURE)
                self.assertFalse(fixture.output.exists())
                self.assertFalse(fixture.closure.exists())

    def test_without_pins_the_retained_path_is_still_the_one_selected(self):
        # A checkout without the retained records: the unchanged path refuses exactly as before.
        result = subprocess.run([sys.executable, "-I", "-B", str(SCRIPT), "--root", str(self.fixture.root),
                                 "--output", str(self.fixture.output)], env=ENV, capture_output=True, text=True, timeout=60)
        self.assertEqual((result.returncode, result.stdout, result.stderr), (2, "", FAILURE))
        self.assertFalse(self.fixture.output.exists())
        with self.assertRaises(FileNotFoundError):
            MODULE.collect(self.fixture.root)


class DerivationTests(unittest.TestCase):
    """The two derivations on the committed catalog, without a bundle."""

    @classmethod
    def setUpClass(cls):
        cls.adopted_raw = (ROOT / MODULE.CATALOG).read_bytes()
        cls.adopted = json.loads(cls.adopted_raw)
        cls.linux = linux_routes(cls.adopted)
        cls.retained = {"agno": MODULE.AGNO_SHA, "activepieces": MODULE.AP_SHA, "services": MODULE.SERVICES_SHA}

    def test_committed_catalog_is_the_pinned_one_and_names_the_retained_digests(self):
        self.assertEqual(sha(self.adopted_raw), MODULE.CATALOG_SHA)
        counts = {family: sum(digest in json.dumps(row) for row in self.adopted)
                  for family, digest in self.retained.items()}
        self.assertEqual(counts, {"agno": 36, "activepieces": 63, "services": 4})
        for row in self.adopted:
            self.assertEqual(row["identity"]["version"], metadata_version(row))

    def test_retained_digests_give_back_the_committed_catalog_and_the_linux_routes(self):
        derived, versions = MODULE.derive_catalog(self.adopted_raw, dict(self.retained))
        self.assertEqual(derived, self.adopted_raw)
        self.assertEqual(versions, {})
        routes = json.loads(MODULE.derive_routes(self.linux, derived, dict(self.retained), versions))
        linux = json.loads(self.linux)
        self.assertIsNone(routes["runtime_target"])
        self.assertEqual(routes["catalog_sha256"], MODULE.CATALOG_SHA)
        routes["runtime_target"], routes["catalog_sha256"] = linux["runtime_target"], linux["catalog_sha256"]
        self.assertEqual(routes, linux)

    def test_one_rebuilt_component_moves_only_the_entries_that_name_it(self):
        digests = {**self.retained, "services": RUN["services"]}
        derived, versions = MODULE.derive_catalog(self.adopted_raw, digests)
        rows = json.loads(derived)
        changed = [after["identity"]["name"] for before, after in zip(self.adopted, rows) if before != after]
        self.assertEqual(sorted(changed), ["gmail", "google-calendar", "google-drive", "slack"])
        self.assertEqual(sorted(versions), sorted(changed))
        for before, after in zip(self.adopted, rows):
            self.assertEqual(after["identity"]["version"], metadata_version(after))
            if before["identity"]["name"] in versions:
                self.assertEqual(versions[before["identity"]["name"]],
                                 (before["identity"]["version"], after["identity"]["version"]))
        routes = json.loads(MODULE.derive_routes(self.linux, derived, digests, versions))
        linux = json.loads(self.linux)
        moved = [(before["identity"]["name"], before["strategy_id"]) for before, after in zip(linux["routes"], routes["routes"])
                 if before != after]
        # Each of the four services entries has its primary route and its services alternative.
        self.assertEqual(len(moved), 8)
        self.assertEqual({name for name, _ in moved}, set(changed))
        self.assertEqual(routes["components"]["agno"], linux["components"]["agno"])
        self.assertEqual(routes["components"]["services"]["sha256"], RUN["services"])

    def test_catalog_that_is_not_the_generator_output_refuses(self):
        rows = copy.deepcopy(self.adopted)
        rows[0]["description"] += " changed"
        with self.assertRaises(MODULE.ExportError):
            MODULE.derive_catalog((json.dumps(rows, ensure_ascii=True, indent=2) + "\n").encode(), RUN)
        with self.assertRaises(MODULE.ExportError):
            MODULE.derive_catalog(json.dumps(self.adopted).encode(), RUN)
        rows = copy.deepcopy(self.adopted)
        rows[0]["description"] += " " + MODULE.AP_SHA
        rows[0]["identity"]["version"] = metadata_version(rows[0])
        with self.assertRaises(MODULE.ExportError):
            MODULE.derive_catalog((json.dumps(rows, ensure_ascii=True, indent=2) + "\n").encode(), RUN)

    def test_wheel_tags_for_cpython_311_on_macos_arm64(self):
        accepted = ("agno-3.0.11-py3-none-any.whl", "six-1.17.0-py2.py3-none-any.whl",
                    "aiohttp-3.14.3-cp311-cp311-macosx_11_0_arm64.whl",
                    "cryptography-50.0.2-cp311-abi3-macosx_11_0_arm64.whl",
                    "wcwidth-0.9.1-cp310-abi3-macosx_11_0_arm64.whl",
                    "lxml-6.1.3-cp311-cp311-macosx_10_9_universal2.whl",
                    "regex-2026.9.29-cp311-cp311-macosx_10_9_x86_64.macosx_11_0_arm64.whl",
                    "tool-1.0-1-py3-none-any.whl")
        refused = ("aiohttp-3.14.3-cp311-cp311-manylinux2014_x86_64.manylinux_2_28_x86_64.whl",
                   "aiohttp-3.14.3-cp311-cp311-macosx_10_9_x86_64.whl",
                   "aiohttp-3.14.3-cp312-cp312-macosx_11_0_arm64.whl",
                   "aiohttp-3.14.3-cp311-cp311-macosx_14_0_arm64.whl",
                   "newer-1.0-cp312-abi3-macosx_11_0_arm64.whl",
                   "aiohttp-3.14.3-cp311-cp311-win_arm64.whl", "agno-3.0.11.tar.gz", "agno-3.0.11-py3-none-any.zip",
                   "only-py2-1.0-py2-none-any.whl", "short-1.0-any.whl")
        for name in accepted:
            self.assertTrue(MODULE.wheel_supported_on_macos_arm64(name), name)
        for name in refused:
            self.assertFalse(MODULE.wheel_supported_on_macos_arm64(name), name)


class CarriedFilesTests(unittest.TestCase):
    """The files the checkout carries for the bundle, as committed."""

    def test_carried_files_have_the_pinned_bytes_and_a_public_origin(self):
        carried = ROOT / MODULE.CARRIED
        sources = json.loads((carried / "SOURCES.json").read_bytes())
        self.assertEqual(sources["schema"], "connect-consumer-bundle-sources/1")
        self.assertRegex(sources["origin"]["asset_url"], r"^https://github\.com/lukacf/meerkat/releases/download/connect-v[0-9.]+/")
        self.assertRegex(sources["origin"]["asset_sha256"], r"^[0-9a-f]{64}$")
        rows = {MODULE.CARRIED + "/" + row["path"]: row for row in sources["files"]}
        self.assertEqual(set(rows), set(MODULE.CARRIED_PINS))
        for relative, pin in MODULE.CARRIED_PINS.items():
            data = (ROOT / relative).read_bytes()
            self.assertEqual(sha(data), pin, relative)
            self.assertEqual(rows[relative]["sha256"], pin, relative)
            self.assertEqual(rows[relative]["bytes"], len(data), relative)
        self.assertEqual(MODULE.CARRIED_PINS[MODULE.CARRIED + "/install-library-consumer.py"], MODULE.INSTALLER_SHA)
        present = {str(path.relative_to(ROOT)) for path in carried.rglob("*") if path.is_file()}
        self.assertEqual(present, set(MODULE.CARRIED_PINS) | {MODULE.MACOS_GUIDE, MODULE.CARRIED + "/SOURCES.json"})

    def test_carried_closure_inputs_have_the_counts_the_guide_states(self):
        carried = ROOT / MODULE.CARRIED
        agno = MODULE.parse_constraints((carried / "agno/runtime-constraints.txt").read_bytes())
        services = MODULE.parse_constraints((carried / "services/runtime-constraints.txt").read_bytes())
        self.assertEqual((len(agno), len(services)), (111, 31))
        self.assertEqual(len({*agno.items(), *services.items()}), 111)
        lock = json.loads((carried / "activepieces/package-lock.json").read_bytes())["packages"]
        self.assertEqual(len([key for key in lock if key and key != "node_modules/" + MODULE.AP_COMPONENT]), 4)
        guide = (carried / "MACOS-CONSUMERS.md").read_text(encoding="ascii")
        for phrase in ("all 115 dependency archives (111 Python wheels and 4 npm archives)", "36 Agno and 63 Activepieces",
                       "101 adopted integration", "the 9 discovery records", "CPython 3.11 on macOS arm64",
                       "/usr/bin/shasum -a 256 -c SHA256SUMS", "--pins release/library-pins-macos-arm64.json"):
            self.assertIn(phrase, guide)
        for word in ("glibc", "sha256sum", "docs/verification", "linux-x86_64-cpython311"):
            self.assertNotIn(word, guide)
        self.assertEqual(len(json.loads((ROOT / MODULE.DISCOVERY_CATALOG).read_bytes())), 9)
        self.assertEqual(sha((ROOT / MODULE.DISCOVERY_CATALOG).read_bytes()), MODULE.PINNED_DISCOVERY_CATALOG_SHA)

    def test_macos_routes_derived_from_the_committed_linux_file_differ_only_in_target_and_digest_values(self):
        linux_raw = (ROOT / MODULE.LINUX_ROUTES).read_bytes()
        adopted_raw = (ROOT / MODULE.CATALOG).read_bytes()
        linux = json.loads(linux_raw)
        self.assertEqual(linux_raw, MODULE.encoded(linux))
        # The committed Linux file holds exactly the associations the committed catalog's own text states.
        rebuilt = json.loads(linux_routes(json.loads(adopted_raw)))
        key = lambda route: (route["identity"]["name"], route["strategy_id"])
        self.assertEqual(sorted(linux["routes"], key=key), sorted(rebuilt["routes"], key=key))
        self.assertEqual(linux["components"], rebuilt["components"])
        self.assertEqual({name: linux[name] for name in ("schema_version", "runtime_target", "provenance")},
                         {name: rebuilt[name] for name in ("schema_version", "runtime_target", "provenance")})
        derived, versions = MODULE.derive_catalog(adopted_raw, RUN)
        self.assertEqual(len(versions), 99)
        mac = json.loads(MODULE.derive_routes(linux_raw, derived, RUN, versions))
        entries = {row["identity"]["name"]: row for row in json.loads(derived)}
        self.assertEqual(len(mac["routes"]), 103)
        self.assertIsNone(mac["runtime_target"])
        self.assertEqual(mac["catalog_sha256"], sha(derived))
        restored = copy.deepcopy(mac)
        restored["runtime_target"], restored["catalog_sha256"] = linux["runtime_target"], linux["catalog_sha256"]
        for family, component in restored["components"].items():
            self.assertEqual(component["sha256"], RUN[family])
            component["sha256"] = linux["components"][family]["sha256"]
        for before, after in zip(linux["routes"], restored["routes"]):
            self.assertEqual(after["identity"]["version"], metadata_version(entries[before["identity"]["name"]]))
            self.assertNotEqual(after["identity"]["version"], before["identity"]["version"])
            after["identity"]["version"] = before["identity"]["version"]
        self.assertEqual(restored, linux)
        self.assertEqual(MODULE.encoded(restored), linux_raw)


if __name__ == "__main__":
    unittest.main()
