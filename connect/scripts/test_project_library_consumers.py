"""Checks for the Linux consumer bundle projection. Synthetic inputs only: tiny wheels and catalogs built here.

No installer, provider, adapter, network or child build runs; one case starts the script itself to read its
refusal line.
"""
from __future__ import annotations

import base64
import contextlib
import copy
import gzip
import hashlib
import importlib.util
import io
import json
import os
from pathlib import Path
import subprocess
import sys
import tarfile
import tempfile
import unittest
from unittest.mock import patch
import zipfile

ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT / "scripts/project-library-consumers.py"
REFUSAL = ("consumer projection failed: check the base bundle, the supplied components and the new destinations; "
           "any partial destination must not be used\n")
RETAINED = {"agno": "1" * 64, "activepieces": "2" * 64, "services": "3" * 64}
BASE_DISCOVERY_SHA = "5" * 64
MARKER = "BASE-ONLY-DISCOVERY-TEXT"
ADOPTED = "crates/toolkit-package/catalog/adopted.json"
DISCOVERY = "crates/toolkit-package/catalog/discovery.json"
AGNO = "agno/meerkat_toolkit_agno-0.1.5-py3-none-any.whl"
SERVICES = "services/meerkat_toolkit_services-0.1.23-py3-none-any.whl"
AP = "activepieces/rkat-toolkit-activepieces-0.1.0.tgz"
PATHS = {"agno": AGNO, "activepieces": AP, "services": SERVICES}
CLOSURE_KEY = "docs/verification/run/closure-linux.json"
ENV = {"PATH": "/usr/bin:/bin", "PYTHONDONTWRITEBYTECODE": "1", "PYTHONNOUSERSITE": "1"}
_MODULE = None


def module():
    """The script under test, loaded once from its path."""
    global _MODULE
    if _MODULE is None:
        spec = importlib.util.spec_from_file_location("project_library_consumers", SCRIPT)
        loaded = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(loaded)
        _MODULE = loaded
    return _MODULE


def sha(data):
    return hashlib.sha256(data).hexdigest()


def encoded(value):
    return (json.dumps(value, sort_keys=True, indent=2, ensure_ascii=True) + "\n").encode()


def generated(rows):
    return (json.dumps(rows, ensure_ascii=True, indent=2) + "\n").encode()


def canonical(value):
    return json.dumps(value, ensure_ascii=True, sort_keys=True, separators=(",", ":")).encode()


def entry(name, strategy, text):
    row = {"schema_version": 1, "identity": {"publisher": "meerkat-toolkit", "name": name},
           "display_name": name.title(), "strategies": [{"id": strategy, "instructions": text}]}
    row["identity"]["version"] = "metadata-sha256:" + sha(canonical(row))
    return row


def with_digests(rows, digests):
    """The adopted rows with the retained component digests replaced, versions recomputed."""
    result = []
    for row in copy.deepcopy(rows):
        before = canonical(row)
        for strategy in row["strategies"]:
            for family, retained in RETAINED.items():
                strategy["instructions"] = strategy["instructions"].replace(retained, digests[family])
        if canonical(row) != before:
            del row["identity"]["version"]
            row["identity"]["version"] = "metadata-sha256:" + sha(canonical(row))
        result.append(row)
    return result


def wheel(members):
    stream = io.BytesIO()
    with zipfile.ZipFile(stream, "w") as archive:
        for name, data in members.items():
            archive.writestr(zipfile.ZipInfo(name, (2026, 1, 1, 0, 0, 0)), data)
    return stream.getvalue()


def tarball(members):
    raw = io.BytesIO()
    with tarfile.open(fileobj=raw, mode="w") as archive:
        for name, data in members.items():
            info = tarfile.TarInfo(name)
            info.size = len(data)
            archive.addfile(info, io.BytesIO(data))
    return gzip.compress(raw.getvalue(), mtime=0)


def agno_wheel(label, requires=("dep==1.0",), profiles=("alpha", "beta"), name="meerkat-toolkit-agno",
               version="0.1.5", drop=()):
    info = "meerkat_toolkit_agno-0.1.5.dist-info/"
    metadata = "Metadata-Version: 2.4\nName: %s\nVersion: %s\n" % (name, version)
    metadata += "".join("Requires-Dist: %s\n" % item for item in requires)
    members = {"meerkat_toolkit_agno/__init__.py": b"",
               "meerkat_toolkit_agno/PROFILE-GUIDE.md": ("# Agno guide %s\n" % label).encode(),
               "meerkat_toolkit_agno/CONFIGURATION-GUIDES.json": encoded({"profiles": {p: {} for p in profiles}}),
               info + "METADATA": metadata.encode()}
    for notice in ("LICENSE-MIT", "LICENSE-APACHE", "LICENSE-Agno", "THIRD_PARTY_NOTICES.md"):
        members[info + "licenses/" + notice] = ("%s %s\n" % (notice, label)).encode()
    return wheel({key: value for key, value in members.items() if key not in drop})


def services_wheel(label, requires=("other==2.0", 'extra-only==9.9; extra == "google"')):
    info = "meerkat_toolkit_services-0.1.23.dist-info/"
    metadata = "Metadata-Version: 2.4\nName: meerkat-toolkit-services\nVersion: 0.1.23\n"
    metadata += "".join("Requires-Dist: %s\n" % item for item in requires)
    return wheel({"meerkat_toolkit_services/__init__.py": b"",
                  "meerkat_toolkit_services/GUIDE.md": ("# Services guide %s\n" % label).encode(),
                  info + "METADATA": metadata.encode()})


def ap_archive(label, dependencies=None):
    package = {"name": "@rkat/toolkit-activepieces", "version": "0.1.0",
               "dependencies": {"zod": "1.0.0"} if dependencies is None else dependencies, "engines": {"node": ">=22"}}
    members = {"package/package.json": encoded(package), "package/README.md": ("# Pieces guide %s\n" % label).encode()}
    for notice in ("LICENSE-MIT", "LICENSE-APACHE", "THIRD_PARTY_NOTICES.md"):
        members["package/" + notice] = ("%s %s\n" % (notice, label)).encode()
    return tarball(members)


def integrity(data):
    return "sha512-" + base64.b64encode(hashlib.sha512(data).digest()).decode()


def offline(package, component, dependency, dependency_data):
    lines = {dependency.split("==")[0]: dependency + " --hash=sha256:" + sha(dependency_data),
             package.split("==")[0]: package + " --hash=sha256:" + sha(component)}
    return ("\n".join(line for _, line in sorted(lines.items())) + "\n").encode()


class Fixture:
    """A small checkout root, base bundle, closure record and a new set of components."""

    def __init__(self, directory):
        self.directory = Path(directory).resolve()
        self.root = self.directory / "root"
        self.base = self.directory / "base"
        self.closure_in = self.directory / "closure-in.json"
        self.closure_out = self.directory / "records" / "closure-out.json"
        self.output = self.directory / "projected"
        (self.directory / "records").mkdir()
        self.adopted = [entry("agno-one", "selected-local-mcp", "Install the wheel with sha256 %s." % RETAINED["agno"]),
                        entry("pieces-one", "selected-local-mcp", "Use archive %s here." % RETAINED["activepieces"]),
                        entry("mail", "services-alternative", "Service wheel %s." % RETAINED["services"]),
                        entry("plain", "manual", "No component digest is named.")]
        self.discovery = [entry("found-one", "manual", "A checkout discovery record.")]
        self.base_discovery = [entry("found-one", "manual", MARKER + " first"), entry("found-two", "manual", MARKER + " second")]
        self.components = {"agno": agno_wheel("base"), "services": services_wheel("base"), "activepieces": ap_archive("base")}
        self.new = {"agno": agno_wheel("new"), "services": services_wheel("new"), "activepieces": ap_archive("new")}
        self.files, self.sources = {}, {}
        self.build()

    def digests(self, components=None):
        return {family: sha(data) for family, data in (components or self.components).items()}

    def build(self):
        files, sources, runtime = {}, {}, module().EXPORTER.LINUX_RUNTIME_TARGET
        digests = self.digests()

        def put(path, data, source):
            files[path], sources[path] = data, source

        retained_path = "docs/verification/run/"
        put(AGNO, self.components["agno"], {"path": retained_path + "agno.whl", "sha256": digests["agno"]})
        put(SERVICES, self.components["services"], {"path": retained_path + "services.whl", "sha256": digests["services"]})
        put(AP, self.components["activepieces"], {"path": retained_path + "pieces.tgz", "sha256": digests["activepieces"]})
        dependencies = {"agno/dep-1.0-py3-none-any.whl": b"dep wheel", "services/other-2.0-py3-none-any.whl": b"other wheel",
                        "activepieces/zod-1.0.0.tgz": b"zod archive"}
        for path, data in dependencies.items():
            put(path, data, {"path": "dist/objects/" + sha(data) + "/" + path.split("/")[1], "sha256": sha(data)})
        put("agno/runtime-constraints.txt", b"dep==1.0\n", {"path": retained_path + "agno-constraints.txt", "sha256": sha(b"dep==1.0\n")})
        put("services/runtime-constraints.txt", b"other==2.0\n",
            {"path": retained_path + "services-constraints.txt", "sha256": sha(b"other==2.0\n")})
        self.closure = encoded({
            "schema_version": 1, "scope": "dependency_bytes_for_existing_consumer_selection",
            "python_platform": {"implementation": "cpython", "version": "3.11", "system": "Linux", "machine": "x86_64",
                                "acquired_with": "3.11.16"},
            "predecessor_closure": {"path": "dist/objects/closure.json", "sha256": "6" * 64},
            "accepted_inputs": {}, "artifacts": [{"name": "dep", "sha256": sha(b"dep wheel")}]})
        package = encoded({"dependencies": {"@rkat/toolkit-activepieces": "file:rkat-toolkit-activepieces-0.1.0.tgz"}})
        lock = encoded({"lockfileVersion": 3, "packages": {
            "": {"dependencies": {"@rkat/toolkit-activepieces": "file:rkat-toolkit-activepieces-0.1.0.tgz"}},
            "node_modules/@rkat/toolkit-activepieces": {
                "version": "0.1.0", "resolved": "file:rkat-toolkit-activepieces-0.1.0.tgz",
                "integrity": integrity(self.components["activepieces"]), "dependencies": {"zod": "1.0.0"},
                "engines": {"node": ">=22"}},
            "node_modules/zod": {"version": "1.0.0", "resolved": "https://registry.npmjs.org/zod/-/zod-1.0.0.tgz",
                                 "integrity": integrity(b"zod archive")}}})
        put("activepieces/package.json", package, {"path": retained_path + "package.json", "sha256": sha(package)})
        put("activepieces/package-lock.json", lock, {"path": retained_path + "package-lock.json", "sha256": sha(lock)})
        bound = ("agno/runtime-constraints.txt", "services/runtime-constraints.txt", "activepieces/package.json",
                 "activepieces/package-lock.json", AGNO, SERVICES, AP)
        record = json.loads(self.closure)
        record["accepted_inputs"] = {path: sha(files[path]) for path in bound}
        self.closure = encoded(record)
        closure_sha = sha(self.closure)
        put("agno/install-offline.txt", offline("meerkat-toolkit-agno==0.1.5", self.components["agno"], "dep==1.0", b"dep wheel"),
            {"kind": "hash_locked_requirements_from_selected_closure", "closure_sha256": closure_sha})
        put("services/install-offline.txt",
            offline("meerkat-toolkit-services==0.1.23", self.components["services"], "other==2.0", b"other wheel"),
            {"kind": "hash_locked_requirements_from_selected_closure", "closure_sha256": closure_sha})
        for family in ("agno", "services"):
            names = sorted(path.split("/")[1] for path in files if path.startswith(family + "/") and path.endswith(".whl"))
            put(family + "/install-requirements.txt", ("\n".join("./" + name for name in names) + "\n").encode(),
                {"kind": "wheel_list_from_selected_linux_closure", "closure_sha256": closure_sha})
        put("activepieces/offline-cache.json", encoded({"schema_version": 1, "tarballs": [{"name": "zod"}]}),
            {"kind": "local_tarballs_for_ordinary_npm_cache_add", "closure_sha256": closure_sha})
        with zipfile.ZipFile(io.BytesIO(self.components["agno"])) as archive:
            members = [("meerkat_toolkit_agno/PROFILE-GUIDE.md", "PROFILE-GUIDE.md")] + [
                ("meerkat_toolkit_agno-0.1.5.dist-info/licenses/" + name, name)
                for name in ("LICENSE-MIT", "LICENSE-APACHE", "LICENSE-Agno", "THIRD_PARTY_NOTICES.md")]
            for member, output in members:
                put("agno/" + output, archive.read(member),
                    {"artifact": AGNO, "artifact_sha256": digests["agno"], "member": member})
        with zipfile.ZipFile(io.BytesIO(self.components["services"])) as archive:
            guide = archive.read("meerkat_toolkit_services/GUIDE.md")
            put("services/PROFILE-GUIDE.md", guide, {"path": retained_path + "services-README.md", "sha256": sha(guide)})
        with tarfile.open(fileobj=io.BytesIO(self.components["activepieces"]), mode="r:gz") as archive:
            for source, output in (("README.md", "PROFILE-GUIDE.md"), ("LICENSE-MIT", "LICENSE-MIT"),
                                   ("LICENSE-APACHE", "LICENSE-APACHE"), ("THIRD_PARTY_NOTICES.md", "THIRD_PARTY_NOTICES.md")):
                put("activepieces/" + output, archive.extractfile("package/" + source).read(),
                    {"artifact": AP, "artifact_sha256": digests["activepieces"], "member": "package/" + source})
        for path in ("agno/hackernews.json", "services/slack.json", "activepieces/tally.json"):
            put(path, encoded({"profile": path}), {"kind": "nonsecret_configuration_example"})
        put("install-library-consumer.py", b"# installer\n", {"path": "dist/installer.py", "sha256": sha(b"# installer\n")})
        readme = ("# Guide\n\nThe catalog retains 4 adopted integration records and adds\n"
                  "2 separate discovery records, keyed by identity.\nThe discovery records remain manual guidance.\n").encode()
        put("README.md", readme, {"path": "docs/adoption/LINUX-CONSUMERS.md", "base": "exporter_checkout",
                                  "sha256": sha(readme), "kind": "consumer_instructions"})
        self.adopted_raw, self.discovery_raw = generated(self.adopted), generated(self.discovery)
        base_rows = with_digests(self.adopted, digests) + self.base_discovery
        catalog = encoded(base_rows)
        put("catalog.json", catalog, {"kind": "composed_catalog_metadata", "sources": [
            {"path": ADOPTED, "sha256": sha(self.adopted_raw), "scope": "adopted", "entries": len(self.adopted)},
            {"path": DISCOVERY, "sha256": BASE_DISCOVERY_SHA, "scope": "discovery", "entries": len(self.base_discovery)}]})
        profiles = {"agno": ["alpha", "beta"], "activepieces": ["piece.one"], "services": ["gmail"]}
        routes = {"schema_version": 1, "catalog_sha256": sha(catalog), "runtime_target": runtime,
                  "provenance": {"inventory": {"path": "docs/adoption/inventory.json", "sha256": "7" * 64}},
                  "components": {family: {"path": PATHS[family], "sha256": digests[family], "profiles": profiles[family]}
                                 for family in PATHS},
                  "routes": [{"identity": base_rows[index]["identity"], "strategy_id": base_rows[index]["strategies"][0]["id"],
                              "family": family, "profile": profiles[family][0]}
                             for index, family in enumerate(("agno", "activepieces", "services"))]}
        put("routes.json", encoded(routes), {"kind": "explicit_maintainer_catalog_routes", "sources": {}})
        self.files, self.sources = files, sources
        self.accepted = {CLOSURE_KEY: closure_sha, "docs/verification/run/acceptance.json": "4" * 64}
        self.runtime = runtime

    def manifests(self):
        contents = {"schema_version": 2, "scope": "consumer_export_only_no_installation_or_activation",
                    "runtime_target": self.runtime, "accepted_inputs": self.accepted,
                    "files": {path: {"sha256": sha(data), "source": self.sources[path]}
                              for path, data in sorted(self.files.items())}}
        raw = encoded(contents)
        sums = "".join(sha(data) + "  " + path + "\n"
                       for path, data in sorted({**self.files, "CONTENTS.json": raw}.items())).encode()
        return raw, sums

    def write(self):
        contents, sums = self.manifests()
        for path, data in {**self.files, "CONTENTS.json": contents, "SHA256SUMS": sums}.items():
            target = self.base / path
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_bytes(data)
        self.closure_in.write_bytes(self.closure)
        root_catalog = self.root / "crates/toolkit-package/catalog"
        root_catalog.mkdir(parents=True)
        (root_catalog / "adopted.json").write_bytes(self.adopted_raw)
        (root_catalog / "discovery.json").write_bytes(self.discovery_raw)
        self.supplied = self.directory / "components"
        self.supplied.mkdir()
        for family, data in self.new.items():
            (self.supplied / PATHS[family].split("/")[1]).write_bytes(data)
        self.contents_sha = sha(contents)
        return self

    def arguments(self, activepieces=False, **changes):
        values = {"--base": str(self.base), "--base-contents-sha256": self.contents_sha,
                  "--agno-wheel": str(self.supplied / AGNO.split("/")[1]), "--agno-sha256": sha(self.new["agno"]),
                  "--services-wheel": str(self.supplied / SERVICES.split("/")[1]),
                  "--services-sha256": sha(self.new["services"]),
                  "--closure-record-in": str(self.closure_in), "--closure-record-out": str(self.closure_out),
                  "--output": str(self.output), "--root": str(self.root)}
        if activepieces:
            values["--activepieces-archive"] = str(self.supplied / AP.split("/")[1])
            values["--activepieces-sha256"] = sha(self.new["activepieces"])
        values.update(changes)
        return [item for pair in values.items() if pair[1] is not None for item in pair]


class ProjectionTests(unittest.TestCase):
    def setUp(self):
        self.module = module()
        scratch = tempfile.TemporaryDirectory(prefix="consumer-projection-tests-")
        self.addCleanup(scratch.cleanup)
        self.fixture = Fixture(scratch.name)

    def run_main(self, arguments, fixture=None):
        fixture = fixture or self.fixture
        exporter = self.module.EXPORTER
        out, err = io.StringIO(), io.StringIO()
        with patch.multiple(exporter, AGNO_SHA=RETAINED["agno"], AP_SHA=RETAINED["activepieces"],
                            SERVICES_SHA=RETAINED["services"], CATALOG_SHA=sha(fixture.adopted_raw)), \
                patch.object(self.module, "DISCOVERY_CATALOG_SHA", sha(fixture.discovery_raw)), \
                contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            code = self.module.main(arguments)
        return code, out.getvalue(), err.getvalue()

    def project(self, **options):
        fixture = self.fixture.write()
        code, out, err = self.run_main(fixture.arguments(**options))
        self.assertEqual((code, err), (0, ""), out)
        self.assertEqual(out.count("\n"), 1)
        return json.loads(out)

    def refused(self, arguments=None, **options):
        fixture = self.fixture
        if not fixture.base.exists():
            fixture.write()
        code, out, err = self.run_main(arguments if arguments is not None else fixture.arguments(**options))
        self.assertEqual((code, out, err), (2, "", REFUSAL))
        self.assertFalse(os.path.lexists(fixture.output))
        self.assertFalse(os.path.lexists(fixture.closure_out))

    def read(self, path):
        return (self.fixture.output / path).read_bytes()

    def expected_digests(self, activepieces=False):
        digests = self.fixture.digests(self.fixture.new)
        if not activepieces:
            digests["activepieces"] = sha(self.fixture.components["activepieces"])
        return digests

    # a. catalog

    def test_catalog_is_the_checkout_adopted_rows_with_this_run_digests_then_checkout_discovery(self):
        result = self.project()
        digests = self.expected_digests()
        expected = with_digests(self.fixture.adopted, digests) + self.fixture.discovery
        self.assertEqual(self.read("catalog.json"), encoded(expected))
        self.assertEqual((result["catalog_rows"], result["discovery_rows"], result["catalog_entries_changed"]), (5, 1, 3))
        self.assertEqual(result["catalog_sha256"], sha(self.read("catalog.json")))
        self.assertEqual(result["components"], digests)
        rows = json.loads(self.read("catalog.json"))
        self.assertEqual(rows[1], with_digests(self.fixture.adopted, self.fixture.digests())[1])  # the kept archive's row
        self.assertEqual(rows[3], self.fixture.adopted[3])
        for row in rows[:4]:
            body = {**row, "identity": {k: v for k, v in row["identity"].items() if k != "version"}}
            self.assertEqual(row["identity"]["version"], "metadata-sha256:" + sha(canonical(body)))

    def test_no_row_of_the_base_catalog_reaches_the_output(self):
        self.project()
        names = [row["identity"]["name"] for row in json.loads(self.read("catalog.json"))]
        self.assertEqual(names, ["agno-one", "pieces-one", "mail", "plain", "found-one"])
        for path in sorted(p for p in self.fixture.output.rglob("*") if p.is_file()):
            self.assertNotIn(MARKER.encode(), path.read_bytes(), path)
        self.assertNotIn(MARKER.encode(), self.fixture.closure_out.read_bytes())

    def test_retained_digests_supplied_again_give_the_committed_adopted_rows(self):
        fixture = self.fixture
        fixture.components = {"agno": agno_wheel("kept"), "services": services_wheel("kept"), "activepieces": ap_archive("kept")}
        fixture.new = dict(fixture.components)
        retained = fixture.digests()
        with patch.dict(RETAINED, retained):
            fixture.adopted = [entry("agno-one", "selected-local-mcp", "Wheel %s." % retained["agno"]),
                               entry("pieces-one", "selected-local-mcp", "Archive %s." % retained["activepieces"]),
                               entry("mail", "services-alternative", "Wheel %s." % retained["services"])]
            fixture.build()
            fixture.write()
            code, out, err = self.run_main(fixture.arguments())
            self.assertEqual((code, err), (0, ""))
            self.assertEqual(json.loads(self.read("catalog.json"))[:3], fixture.adopted)
            self.assertEqual(json.loads(out)["catalog_entries_changed"], 0)
            self.assertEqual(self.read(AGNO), fixture.components["agno"])

    def test_base_adopted_rows_must_be_the_checkout_rows_for_the_base_digests(self):
        fixture = self.fixture
        rows = json.loads(fixture.files["catalog.json"])
        rows[3]["display_name"] = "Changed in the base"
        fixture.files["catalog.json"] = encoded(rows)
        routes = json.loads(fixture.files["routes.json"])
        routes["catalog_sha256"] = sha(fixture.files["catalog.json"])
        fixture.files["routes.json"] = encoded(routes)
        self.refused()

    def test_base_with_fewer_rows_than_the_adopted_catalog_is_refused(self):
        fixture = self.fixture
        fixture.files["catalog.json"] = encoded(json.loads(fixture.files["catalog.json"])[:3])
        routes = json.loads(fixture.files["routes.json"])
        routes["catalog_sha256"] = sha(fixture.files["catalog.json"])
        fixture.files["routes.json"] = encoded(routes)
        self.refused()

    def test_checkout_catalogs_must_be_the_pinned_bytes(self):
        fixture = self.fixture.write()
        (fixture.root / DISCOVERY).write_bytes(generated(fixture.discovery + [entry("late", "manual", "Added later.")]))
        self.refused()

    def test_component_digest_outside_strategy_instructions_is_refused(self):
        fixture = self.fixture
        row = entry("odd", "manual", "Nothing here.")
        del row["identity"]["version"]
        row["display_name"] = RETAINED["agno"]
        row["identity"]["version"] = "metadata-sha256:" + sha(canonical(row))
        fixture.adopted.append(row)
        fixture.build()
        self.refused()

    # b. components and what the exporter derives from them

    def test_supplied_wheels_replace_the_base_wheels_and_their_documents(self):
        result = self.project()
        fixture = self.fixture
        self.assertEqual(self.read(AGNO), fixture.new["agno"])
        self.assertEqual(self.read(SERVICES), fixture.new["services"])
        self.assertEqual(self.read(AP), fixture.components["activepieces"])
        self.assertEqual(self.read("agno/PROFILE-GUIDE.md"), b"# Agno guide new\n")
        self.assertEqual(self.read("agno/THIRD_PARTY_NOTICES.md"), b"THIRD_PARTY_NOTICES.md new\n")
        self.assertEqual(self.read("agno/LICENSE-Agno"), b"LICENSE-Agno new\n")
        self.assertEqual(self.read("services/PROFILE-GUIDE.md"), b"# Services guide new\n")
        self.assertEqual(self.read("activepieces/PROFILE-GUIDE.md"), b"# Pieces guide base\n")
        self.assertEqual(self.read("agno/install-offline.txt"),
                         offline("meerkat-toolkit-agno==0.1.5", fixture.new["agno"], "dep==1.0", b"dep wheel"))
        self.assertEqual(self.read("services/install-offline.txt"),
                         offline("meerkat-toolkit-services==0.1.23", fixture.new["services"], "other==2.0", b"other wheel"))
        files = json.loads(self.read("CONTENTS.json"))["files"]
        self.assertEqual(files[AGNO]["source"], {"kind": "component_built_in_this_run", "sha256": sha(fixture.new["agno"])})
        self.assertEqual(files["agno/THIRD_PARTY_NOTICES.md"]["source"], {
            "artifact": AGNO, "artifact_sha256": sha(fixture.new["agno"]),
            "member": "meerkat_toolkit_agno-0.1.5.dist-info/licenses/THIRD_PARTY_NOTICES.md"})
        self.assertEqual(files["services/PROFILE-GUIDE.md"]["source"], {
            "artifact": SERVICES, "artifact_sha256": sha(fixture.new["services"]), "member": "meerkat_toolkit_services/GUIDE.md"})
        self.assertEqual(files[AP]["source"], fixture.sources[AP])
        self.assertIn("agno/PROFILE-GUIDE.md", result["changed"])
        self.assertNotIn(AP, result["changed"])
        self.assertNotIn("activepieces/package-lock.json", result["changed"])

    def test_supplied_archive_replaces_the_base_archive_documents_and_lock_integrity(self):
        result = self.project(activepieces=True)
        fixture = self.fixture
        self.assertEqual(self.read(AP), fixture.new["activepieces"])
        self.assertEqual(self.read("activepieces/PROFILE-GUIDE.md"), b"# Pieces guide new\n")
        self.assertEqual(self.read("activepieces/LICENSE-MIT"), b"LICENSE-MIT new\n")
        lock, base_lock = json.loads(self.read("activepieces/package-lock.json")), json.loads(fixture.files["activepieces/package-lock.json"])
        self.assertEqual(lock["packages"]["node_modules/@rkat/toolkit-activepieces"]["integrity"], integrity(fixture.new["activepieces"]))
        base_lock["packages"]["node_modules/@rkat/toolkit-activepieces"]["integrity"] = integrity(fixture.new["activepieces"])
        self.assertEqual(lock, base_lock)
        self.assertEqual(result["components"], self.expected_digests(activepieces=True))
        self.assertEqual(result["catalog_entries_changed"], 3)
        closure = json.loads(fixture.closure_out.read_bytes())
        self.assertEqual(closure["accepted_inputs"]["activepieces/package-lock.json"], sha(self.read("activepieces/package-lock.json")))
        self.assertEqual(closure["accepted_inputs"][AP], sha(fixture.new["activepieces"]))
        self.assertEqual(json.loads(self.read("routes.json"))["components"]["activepieces"]["sha256"], sha(fixture.new["activepieces"]))

    def test_component_with_another_digest_name_or_version_is_refused(self):
        fixture = self.fixture.write()
        self.refused(fixture.arguments(**{"--agno-sha256": "0" * 64}))
        self.refused(fixture.arguments(**{"--services-sha256": sha(fixture.new["agno"])}))
        moved = fixture.directory / "agno.whl"
        moved.write_bytes(fixture.new["agno"])
        self.refused(fixture.arguments(**{"--agno-wheel": str(moved)}))
        self.refused(fixture.arguments(**{"--agno-wheel": "components/" + AGNO.split("/")[1]}))
        for changed in (agno_wheel("new", version="0.1.6"), agno_wheel("new", name="another-distribution")):
            (fixture.supplied / AGNO.split("/")[1]).write_bytes(changed)
            self.refused(fixture.arguments(**{"--agno-sha256": sha(changed)}))

    def test_component_requirements_must_be_the_bundle_constraints(self):
        fixture = self.fixture.write()
        for requires in (("dep==1.1",), ("dep==1.0", "more==3.0"), ("dep>=1.0",)):
            changed = agno_wheel("new", requires=requires)
            (fixture.supplied / AGNO.split("/")[1]).write_bytes(changed)
            self.refused(fixture.arguments(**{"--agno-sha256": sha(changed)}))
        changed = services_wheel("new", requires=("other==2.0", "extra-only==9.9"))
        (fixture.supplied / SERVICES.split("/")[1]).write_bytes(changed)
        self.refused(fixture.arguments(**{"--services-sha256": sha(changed)}))

    def test_component_must_hold_the_documents_and_profiles_of_the_bundle(self):
        fixture = self.fixture.write()
        info = "meerkat_toolkit_agno-0.1.5.dist-info/"
        for changed in (agno_wheel("new", drop=(info + "licenses/LICENSE-Agno",)),
                        agno_wheel("new", drop=("meerkat_toolkit_agno/PROFILE-GUIDE.md",)),
                        agno_wheel("new", drop=("meerkat_toolkit_agno/CONFIGURATION-GUIDES.json",)),
                        agno_wheel("new", profiles=("alpha",)), agno_wheel("new", profiles=("alpha", "beta", "gamma"))):
            (fixture.supplied / AGNO.split("/")[1]).write_bytes(changed)
            self.refused(fixture.arguments(**{"--agno-sha256": sha(changed)}))

    def test_supplied_archive_must_be_the_locked_package(self):
        fixture = self.fixture.write()
        changed = ap_archive("new", dependencies={"zod": "2.0.0"})
        (fixture.supplied / AP.split("/")[1]).write_bytes(changed)
        self.refused(fixture.arguments(activepieces=True, **{"--activepieces-sha256": sha(changed)}))
        self.refused(fixture.arguments(**{"--activepieces-archive": str(fixture.supplied / AP.split("/")[1])}))

    def test_base_documents_must_come_from_the_base_components(self):
        self.fixture.files["services/PROFILE-GUIDE.md"] = b"# Another guide\n"
        self.refused()

    def test_base_requirement_list_must_be_the_exporter_form(self):
        fixture = self.fixture
        fixture.files["agno/install-offline.txt"] += b"dep==1.0 --hash=sha256:" + sha(b"dep wheel").encode() + b"\n"
        self.refused()

    # c. routes

    def test_routes_change_only_component_digests_entry_versions_and_catalog_digest(self):
        result = self.project()
        base, routes = json.loads(self.fixture.files["routes.json"]), json.loads(self.read("routes.json"))
        self.assertEqual(self.read("routes.json"), encoded(routes))
        self.assertEqual(result["routes_sha256"], sha(self.read("routes.json")))
        rows = json.loads(self.read("catalog.json"))
        expected = copy.deepcopy(base)
        expected["catalog_sha256"] = sha(self.read("catalog.json"))
        expected["components"]["agno"]["sha256"] = sha(self.fixture.new["agno"])
        expected["components"]["services"]["sha256"] = sha(self.fixture.new["services"])
        expected["routes"][0]["identity"]["version"] = rows[0]["identity"]["version"]
        expected["routes"][2]["identity"]["version"] = rows[2]["identity"]["version"]
        self.assertEqual(routes, expected)
        self.assertEqual(routes["runtime_target"], self.module.EXPORTER.LINUX_RUNTIME_TARGET)
        self.assertNotEqual(routes["routes"][0]["identity"]["version"], base["routes"][0]["identity"]["version"])
        self.assertEqual(routes["routes"][1], base["routes"][1])

    def test_routes_that_do_not_bind_the_base_are_refused(self):
        for change in ("component", "version", "catalog", "target"):
            with self.subTest(change=change):
                scratch = tempfile.TemporaryDirectory(prefix="consumer-projection-tests-")
                self.addCleanup(scratch.cleanup)
                self.fixture = fixture = Fixture(scratch.name)
                routes = json.loads(fixture.files["routes.json"])
                if change == "component":
                    routes["components"]["agno"]["sha256"] = "8" * 64
                elif change == "version":
                    routes["routes"][0]["identity"]["version"] = "metadata-sha256:" + "8" * 64
                elif change == "catalog":
                    routes["catalog_sha256"] = "8" * 64
                else:
                    routes["runtime_target"] = None
                fixture.files["routes.json"] = encoded(routes)
                self.refused()

    # d. the consumer guide

    def test_guide_states_the_composed_number_of_discovery_records_and_changes_nothing_else(self):
        result = self.project()
        base = self.fixture.files["README.md"].decode()
        self.assertEqual(self.read("README.md").decode(), base.replace("adds\n2 separate", "adds\n1 separate"))
        self.assertIn("README.md", result["changed"])
        source = json.loads(self.read("CONTENTS.json"))["files"]["README.md"]["source"]
        self.assertEqual(source["base"], self.fixture.sources["README.md"])
        self.assertEqual(source["discovery_records"], 1)

    def test_guide_is_kept_byte_exact_when_the_number_already_fits(self):
        self.fixture.discovery.append(entry("found-three", "manual", "A second checkout discovery record."))
        self.fixture.build()
        result = self.project()
        self.assertEqual(self.read("README.md"), self.fixture.files["README.md"])
        self.assertNotIn("README.md", result["changed"])
        self.assertEqual(json.loads(self.read("CONTENTS.json"))["files"]["README.md"]["source"], self.fixture.sources["README.md"])

    def test_guide_without_one_exact_statement_of_the_base_number_is_refused(self):
        base = self.fixture.files["README.md"].decode()
        for text in (base.replace("2 separate discovery records", "two separate discovery records"),
                     base + "It adds 2 separate discovery records.\n",
                     base.replace("2 separate discovery records", "3 separate discovery records")):
            with self.subTest(text=text[-60:]):
                scratch = tempfile.TemporaryDirectory(prefix="consumer-projection-tests-")
                self.addCleanup(scratch.cleanup)
                self.fixture = Fixture(scratch.name)
                self.fixture.files["README.md"] = text.encode()
                self.refused()

    # e. CONTENTS.json and SHA256SUMS

    def test_manifests_use_the_exporter_encoding_and_bind_every_file(self):
        result = self.project()
        fixture = self.fixture
        raw = self.read("CONTENTS.json")
        contents = json.loads(raw)
        self.assertEqual(raw, encoded(contents))
        self.assertEqual((contents["schema_version"], contents["scope"], contents["runtime_target"]),
                         (2, "consumer_export_only_no_installation_or_activation", fixture.runtime))
        written = {str(path.relative_to(fixture.output)): path.read_bytes()
                   for path in fixture.output.rglob("*") if path.is_file()}
        self.assertEqual(set(written), set(fixture.files) | {"CONTENTS.json", "SHA256SUMS"})
        self.assertEqual({path: row["sha256"] for path, row in contents["files"].items()},
                         {path: sha(data) for path, data in written.items() if path not in ("CONTENTS.json", "SHA256SUMS")})
        self.assertEqual(written["SHA256SUMS"], "".join(
            sha(data) + "  " + path + "\n" for path, data in sorted(written.items()) if path != "SHA256SUMS").encode())
        self.assertEqual((result["contents_sha256"], result["checksums_sha256"], result["files"]),
                         (sha(raw), sha(written["SHA256SUMS"]), len(written)))
        self.assertEqual(result["output"], str(fixture.output))
        for path in fixture.output.rglob("*"):
            self.assertFalse(path.is_symlink())
            self.assertEqual(path.stat().st_mode & 0o777, 0o700 if path.is_dir() else 0o600, path)

    def test_accepted_inputs_name_the_checkout_catalogs_and_the_new_closure_record(self):
        result = self.project()
        fixture = self.fixture
        contents = json.loads(self.read("CONTENTS.json"))
        closure_sha = sha(fixture.closure_out.read_bytes())
        self.assertEqual(contents["accepted_inputs"], {
            "docs/verification/run/acceptance.json": "4" * 64, ADOPTED: sha(fixture.adopted_raw),
            DISCOVERY: sha(fixture.discovery_raw), "closure-out.json": closure_sha})
        self.assertEqual(result["closure_record"], {"path": str(fixture.closure_out), "key": "closure-out.json", "sha256": closure_sha})
        text = self.read("CONTENTS.json").decode()
        for stale in (BASE_DISCOVERY_SHA, sha(fixture.closure), CLOSURE_KEY, sha(fixture.components["agno"]),
                      sha(fixture.components["services"]), sha(fixture.files["catalog.json"])):
            self.assertNotIn(stale, text)
        self.assertEqual(contents["files"]["catalog.json"]["source"], {"kind": "composed_catalog_metadata", "sources": [
            {"path": ADOPTED, "sha256": sha(fixture.adopted_raw), "scope": "adopted", "entries": 4,
             "derivation": {"kind": "component_digests_of_this_run", "entries_changed": 3}},
            {"path": DISCOVERY, "sha256": sha(fixture.discovery_raw), "scope": "discovery", "entries": 1}]})
        self.assertEqual(contents["files"]["routes.json"]["source"]["substituted"],
                         ["components.sha256", "routes.identity.version", "catalog_sha256"])

    def test_closure_record_key_can_be_the_path_the_packager_looks_up(self):
        key = "docs/verification/projected/closure-linux.json"
        result = self.project(**{"--closure-record-key": key})
        contents = json.loads(self.read("CONTENTS.json"))
        self.assertEqual(contents["accepted_inputs"][key], result["closure_record"]["sha256"])
        self.assertEqual(result["closure_record"]["key"], key)
        self.fixture.output = self.fixture.directory / "second"
        self.fixture.closure_out = self.fixture.directory / "records" / "second.json"
        for bad in (ADOPTED, "docs/verification/run/acceptance.json", "../closure.json", ""):
            self.refused(self.fixture.arguments(**{"--closure-record-key": bad}))

    def test_unchanged_files_keep_their_base_source(self):
        result = self.project()
        fixture = self.fixture
        contents = json.loads(self.read("CONTENTS.json"))
        closure_sha = result["closure_record"]["sha256"]
        for path, source in fixture.sources.items():
            if path in result["changed"]:
                continue
            expected = dict(source)
            if "closure_sha256" in expected:
                expected["closure_sha256"] = closure_sha  # the one record the bundle accepts
            self.assertEqual(contents["files"][path]["source"], expected, path)
            self.assertEqual(self.read(path), fixture.files[path], path)
        self.assertEqual(contents["files"]["agno/install-offline.txt"]["source"],
                         {"kind": "hash_locked_requirements_from_selected_closure", "closure_sha256": closure_sha})

    # f. the dependency closure record

    def test_closure_record_is_the_input_record_rebound_to_the_new_components(self):
        self.project()
        fixture = self.fixture
        raw = fixture.closure_out.read_bytes()
        record, base = json.loads(raw), json.loads(fixture.closure)
        self.assertEqual(raw, encoded(record))
        self.assertEqual(record["accepted_inputs"], {path: sha(self.read(path)) for path in base["accepted_inputs"]})
        self.assertNotEqual(record["accepted_inputs"][AGNO], base["accepted_inputs"][AGNO])
        self.assertEqual({k: v for k, v in record.items() if k != "accepted_inputs"},
                         {k: v for k, v in base.items() if k != "accepted_inputs"})
        self.assertEqual(fixture.closure_out.stat().st_mode & 0o777, 0o600)
        self.assertEqual(fixture.closure_in.read_bytes(), fixture.closure)

    def test_closure_record_that_the_base_does_not_accept_is_refused(self):
        fixture = self.fixture.write()
        record = json.loads(fixture.closure)
        record["artifacts"] = []
        fixture.closure_in.write_bytes(encoded(record))
        self.refused()
        record = json.loads(fixture.closure)
        record["accepted_inputs"][AGNO] = "9" * 64
        fixture.closure = encoded(record)
        fixture.accepted[CLOSURE_KEY] = sha(fixture.closure)
        contents, sums = fixture.manifests()
        (fixture.base / "CONTENTS.json").write_bytes(contents)
        (fixture.base / "SHA256SUMS").write_bytes(sums)
        fixture.closure_in.write_bytes(fixture.closure)
        fixture.contents_sha = sha(contents)
        self.refused()

    # g. the base is verified before use; nothing is written on a refusal

    def test_base_that_differs_from_its_own_manifests_is_refused(self):
        fixture = self.fixture.write()
        self.refused(fixture.arguments(**{"--base-contents-sha256": "0" * 64}))
        for change in ("payload", "extra", "missing", "link", "sums"):
            with self.subTest(change=change):
                scratch = tempfile.TemporaryDirectory(prefix="consumer-projection-tests-")
                self.addCleanup(scratch.cleanup)
                self.fixture = fixture = Fixture(scratch.name).write()
                if change == "payload":
                    (fixture.base / "agno/hackernews.json").write_bytes(b"{}\n")
                elif change == "extra":
                    (fixture.base / "agno/unlisted.txt").write_bytes(b"x\n")
                elif change == "missing":
                    (fixture.base / "services/slack.json").unlink()
                elif change == "link":
                    (fixture.base / "services/slack.json").unlink()
                    (fixture.base / "services/slack.json").symlink_to(fixture.base / "agno/hackernews.json")
                else:
                    sums = (fixture.base / "SHA256SUMS").read_bytes().replace(sha(fixture.files["README.md"]).encode(), b"0" * 64)
                    (fixture.base / "SHA256SUMS").write_bytes(sums)
                self.refused()

    def test_existing_or_nested_destinations_are_refused_and_the_base_is_left_alone(self):
        fixture = self.fixture.write()
        before = {path: path.read_bytes() for path in fixture.base.rglob("*") if path.is_file()}
        fixture.output.mkdir()
        code, out, err = self.run_main(fixture.arguments())
        self.assertEqual((code, out, err), (2, "", REFUSAL))
        self.assertEqual(list(fixture.output.iterdir()), [])
        fixture.output.rmdir()
        fixture.closure_out.write_bytes(b"{}\n")
        code, out, err = self.run_main(fixture.arguments())
        self.assertEqual((code, out, err), (2, "", REFUSAL))
        self.assertEqual(fixture.closure_out.read_bytes(), b"{}\n")
        self.assertFalse(os.path.lexists(fixture.output))
        fixture.closure_out.unlink()
        for arguments in (fixture.arguments(**{"--output": str(fixture.base / "inside")}),
                          fixture.arguments(**{"--closure-record-out": str(fixture.output / "closure.json")}),
                          fixture.arguments(**{"--closure-record-out": str(fixture.base / "closure.json")}),
                          fixture.arguments(**{"--output": str(fixture.root / "crates/toolkit-package/catalog/out")}),
                          fixture.arguments(**{"--output": "relative/output"})):
            self.refused(arguments)
        self.assertEqual({path: path.read_bytes() for path in fixture.base.rglob("*") if path.is_file()}, before)

    def test_checksums_are_written_last_and_a_failed_write_leaves_no_complete_bundle(self):
        fixture = self.fixture.write()
        exporter, order = self.module.EXPORTER, []
        original = exporter.write_file

        def recording(directory, name, data):
            order.append(name)
            return original(directory, name, data)

        with patch.object(exporter, "write_file", recording):
            code, out, err = self.run_main(fixture.arguments())
        self.assertEqual((code, err), (0, ""))
        self.assertEqual(order[-1], "SHA256SUMS")
        self.assertEqual(order.count("SHA256SUMS"), 1)
        self.assertEqual(order[0], "closure-out.json")

    def fresh_fixture(self):
        scratch = tempfile.TemporaryDirectory(prefix="consumer-projection-tests-")
        self.addCleanup(scratch.cleanup)
        self.fixture = Fixture(scratch.name)
        return self.fixture

    def test_a_failed_bundle_write_leaves_no_closure_record_and_no_complete_bundle(self):
        exporter = self.module.EXPORTER
        original = exporter.write_file
        for failing in ("catalog.json", "SHA256SUMS"):
            with self.subTest(failing=failing):
                fixture = self.fresh_fixture().write()

                def failing_write(directory, name, data, failing=failing):
                    if name == failing:
                        raise OSError("synthetic write failure")
                    return original(directory, name, data)

                with patch.object(exporter, "write_file", failing_write):
                    code, out, err = self.run_main(fixture.arguments())
                self.assertEqual((code, out, err), (2, "", REFUSAL))
                self.assertFalse(os.path.lexists(fixture.closure_out))
                self.assertFalse((fixture.output / "SHA256SUMS").exists())

    def test_a_cut_or_damaged_component_archive_is_the_one_fixed_refusal(self):
        whole = self.fixture.new["activepieces"]
        damaged = bytearray(whole)
        damaged[len(damaged) // 2] ^= 0xFF
        for label, data in (("cut", whole[:-12]), ("cut-short", whole[:20]), ("damaged", bytes(damaged)),
                            ("data-after-the-stream", whole + b"more")):
            with self.subTest(archive=label):
                fixture = self.fresh_fixture().write()
                (fixture.supplied / AP.split("/")[1]).write_bytes(data)
                self.refused(fixture.arguments(activepieces=True, **{"--activepieces-sha256": sha(data)}))
        # the base's own archive, bound by the base's checksums, is read with the same care
        fixture = self.fresh_fixture()
        fixture.files[AP] = fixture.files[AP][:-12]
        self.refused()

    def test_a_replaced_digest_that_stays_in_the_projected_bundle_is_refused(self):
        fixture = self.fixture
        # the base records an unchanged file beside the digest of a component this run replaces
        fixture.sources["agno/hackernews.json"] = {"kind": "nonsecret_configuration_example",
                                                   "built_beside": sha(fixture.components["agno"])}
        self.refused()
        # the same record with the digest of a component that stays projects
        fixture = self.fresh_fixture()
        fixture.sources["agno/hackernews.json"] = {"kind": "nonsecret_configuration_example",
                                                   "built_beside": sha(fixture.components["activepieces"])}
        self.project()

    def test_command_line_refusal_is_one_fixed_line(self):
        fixture = self.fixture.write()
        result = subprocess.run([sys.executable, "-I", "-B", str(SCRIPT), *fixture.arguments()],
                                env=ENV, capture_output=True, text=True, check=False)
        self.assertEqual((result.returncode, result.stdout, result.stderr), (2, "", REFUSAL))  # the real pins differ
        self.assertFalse(os.path.lexists(fixture.output))
        result = subprocess.run([sys.executable, "-I", "-B", str(SCRIPT), "--help"],
                                env=ENV, capture_output=True, text=True, check=False)
        self.assertEqual(result.returncode, 0)
        for option in ("--base", "--base-contents-sha256", "--agno-wheel", "--services-wheel", "--activepieces-archive",
                       "--closure-record-in", "--closure-record-out", "--closure-record-key", "--output", "--root"):
            self.assertIn(option, result.stdout)


if __name__ == "__main__":
    unittest.main()
