"""Release-composed routes use separate official pins and existing setup owners."""
import copy
import importlib.util
from pathlib import Path
import unittest

from test_backend import FakeRunner, FixtureMixin, backend
from test_catalog_routes import route_fixture
from test_official_github import INSTALLER, bundle as official_bundle


spec = importlib.util.spec_from_file_location(
    "official_route_packager", Path(__file__).resolve().parents[3] / "scripts/package-library-release.py")
packager = importlib.util.module_from_spec(spec)
spec.loader.exec_module(packager)


BINDINGS = [
    {"identity": {"publisher": "meerkat-toolkit", "name": "github", "version": "metadata-sha256:" + "a" * 64},
     "strategy_id": "selected-local-mcp", "family": "official-github", "profile": "selected-reads", "path": "github/github-mcp-server"},
    {"identity": {"publisher": "meerkat-toolkit", "name": "playwright", "version": "metadata-sha256:" + "b" * 64},
     "strategy_id": "selected-local-mcp", "family": "official-playwright", "profile": "isolated-chrome", "path": "playwright/package-lock.json"},
]
HOSTED = {"publisher": "meerkat-toolkit", "name": "github", "version": "discovery-2026-09-30.2"}


class OfficialRunner(FakeRunner):
    def run(self, argv, capture=False):
        if "--official-bundle" in argv:
            self.calls.append((list(argv), capture))
            # These route tests may inspect metadata only, never invoke a server.
            if "--official-describe" not in argv:
                raise AssertionError(argv)
            return backend.Runner(self.workspace).run(argv, capture=capture)
        return super().run(argv, capture=capture)


class OfficialRouteTests(FixtureMixin, unittest.TestCase):
    def setUp(self):
        super().setUp()
        route_fixture(self)
        self.official = self.root / "official-source"
        official_bundle(self.official)
        declaration = backend.decode((self.official / "delivery.json").read_bytes())
        declaration["components"].append({"catalog_id": "playwright", "package_lock": "playwright/package-lock.json"})
        (self.official / "delivery.json").write_bytes(backend.encoded(declaration))
        (self.official / "playwright").mkdir()
        (self.official / "playwright/package-lock.json").write_bytes(b'{"fixture":"inert"}\n')
        official_files = {str(path.relative_to(self.official)): path.read_bytes()
                          for path in self.official.rglob("*") if path.is_file() and path.name != "CONTENTS.json"}
        (self.official / "CONTENTS.json").write_bytes(backend.encoded({"format": 1, "files": {
            name: {"sha256": backend.digest(raw), "bytes": len(raw)} for name, raw in official_files.items()}}))
        self.official_sha = backend.digest((self.official / "CONTENTS.json").read_bytes())
        catalog = backend.decode((self.bundle / "catalog.json").read_bytes())
        catalog += [{**copy.deepcopy(catalog[0]), "identity": binding["identity"], "strategies": [{"id": binding["strategy_id"], "kind": "local_mcp"}]}
                    for binding in BINDINGS]
        catalog += [{**copy.deepcopy(catalog[0]), "identity": HOSTED, "strategies": [{"id": "hosted", "kind": "remote_mcp"}]}]
        catalog_raw = backend.encoded(catalog)
        (self.bundle / "catalog.json").write_bytes(catalog_raw)
        self.manifest["files"]["catalog.json"] = {"sha256": backend.digest(catalog_raw)}
        routes = backend.decode((self.bundle / "routes.json").read_bytes())
        routes["catalog_sha256"] = backend.digest(catalog_raw)
        (self.bundle / "routes.json").write_bytes(backend.encoded(routes))
        self.manifest["files"]["routes.json"] = {"sha256": backend.digest(backend.encoded(routes))}
        self.write_manifest()
        self.files = {"consumers/" + str(path.relative_to(self.bundle)): (path.read_bytes(), 0o644)
                      for path in self.bundle.rglob("*") if path.is_file()}
        self.files.update({"official/" + str(path.relative_to(self.official)): (path.read_bytes(), 0o644)
                           for path in self.official.rglob("*") if path.is_file()})
        self.files["setup/install-library-consumer.py"] = (INSTALLER.read_bytes(), 0o644)

    def compose(self):
        packager.compose_official_routes(self.files, copy.deepcopy(BINDINGS))
        for name, (raw, _) in self.files.items():
            if name.startswith("consumers/"):
                (self.bundle / name.removeprefix("consumers/")).write_bytes(raw)
        self.manifest = backend.decode((self.bundle / "CONTENTS.json").read_bytes())
        self.sha = backend.digest((self.bundle / "CONTENTS.json").read_bytes())

    def selected(self, **changes):
        args = {"families": (*backend.FAMILIES, backend.OFFICIAL_FAMILY), "runner_factory": OfficialRunner,
                "official_bundle": self.official, "official_contents_sha256": self.official_sha,
                "official_installer": INSTALLER.resolve(), "official_installer_sha256": backend.digest(INSTALLER.read_bytes())}
        args.update(changes)
        return self.controller(**args)

    def route_request(self, controller, index=0):
        return {**BINDINGS[index]["identity"], "strategy_id": BINDINGS[index]["strategy_id"],
                "routes_sha256": controller.routes_sha256}

    def change_index(self, edit):
        value = backend.decode((self.bundle / "routes.json").read_bytes())
        edit(value)
        raw = backend.encoded(value)
        (self.bundle / "routes.json").write_bytes(raw)
        self.manifest["files"]["routes.json"]["sha256"] = backend.digest(raw)
        self.write_manifest()

    def test_composition_changes_only_three_metadata_files_and_preserves_prior_routes(self):
        before = copy.deepcopy(self.files)
        old_index = backend.decode(before["consumers/routes.json"][0])
        self.compose()
        changed = {name for name in self.files if self.files[name] != before.get(name)}
        self.assertEqual(changed, {"consumers/routes.json", "consumers/CONTENTS.json", "consumers/SHA256SUMS"})
        index = backend.decode(self.files["consumers/routes.json"][0])
        self.assertEqual(index["schema_version"], 2)
        self.assertEqual(index["routes"][:-2], old_index["routes"])
        self.assertEqual(index["components"]["official-github"]["contents_sha256"], self.official_sha)
        sums = dict(line.split("  ", 1)[::-1] for line in self.files["consumers/SHA256SUMS"][0].decode().splitlines())
        for name, sha in sums.items():
            self.assertEqual(backend.digest(self.files["consumers/" + name][0]), sha)

    def test_disabled_routes_visible_and_hosted_entry_stays_manual(self):
        self.compose()
        c = self.controller()
        for index, binding in enumerate(BINDINGS):
            option, = c.catalog_entry(binding["identity"])["setup_options"]
            self.assertEqual(option["status"], "disabled")
            self.assertEqual(option["family"], binding["family"])
            with self.assertRaisesRegex(backend.SetupError, "unknown_family"):
                c.catalog_route(self.route_request(c, index))
        self.assertEqual(c.catalog_entry(HOSTED)["setup_options"], [{"strategy_id": "hosted", "status": "manual", "reason": "no_bound_setup_route"}])
        self.assertEqual(c.runner.calls, [])

    def test_enabled_github_reuses_actual_metadata_prepare_help_and_keeps_playwright_disabled(self):
        self.compose()
        c = self.selected()
        self.assertEqual(c.catalog_route(self.route_request(c))["status"], "preparation_required")
        self.assertEqual(c.runner.calls, [])
        c.prepare({"family": "official-github"})
        c.join()
        result = c.catalog_route(self.route_request(c))
        self.assertEqual(result["status"], "prepared")
        self.assertEqual(result["help"], c.help({"family": "official-github", "profile": "selected-reads"}))
        self.assertFalse(result["help"]["selectable_operations"])
        self.assertEqual(c.catalog_entry(BINDINGS[1]["identity"])["setup_options"][0]["status"], "disabled")
        self.assertTrue(all("--official-describe" in call[0] for call in c.runner.calls))
        self.assertEqual(c.validations, {})

    def test_frozen_manifest_installer_artifact_and_nonprimary_payload_tampering_refuse_without_spawn(self):
        self.compose()
        for path in ("official/CONTENTS.json", "official-installer.py", "official/github/github-mcp-server", "official/github/mcp.template.json"):
            with self.subTest(path=path):
                c = self.selected()
                (c.workspace / path).write_bytes(b"changed")
                with self.assertRaisesRegex(backend.SetupError, "artifact_mismatch"):
                    c.catalog_route(self.route_request(c))
                self.assertEqual(c.runner.calls, [])

    def test_wrong_selection_pins_refuse_but_disabled_pin_does_not_require_payload(self):
        self.compose()
        for key in ("contents_sha256", "installer_sha256", "sha256"):
            with self.subTest(key=key):
                original = (self.bundle / "routes.json").read_bytes()
                self.change_index(lambda index: index["components"]["official-github"].update({key: "f" * 64}))
                c = self.selected()
                with self.assertRaisesRegex(backend.SetupError, "artifact_mismatch"):
                    c.catalog_route(self.route_request(c))
                self.assertEqual(c.runner.calls, [])
                disabled = self.controller()
                self.assertEqual(disabled.catalog_entry(BINDINGS[0]["identity"])["setup_options"][0]["status"], "disabled")
                (self.bundle / "routes.json").write_bytes(original)
                self.manifest["files"]["routes.json"]["sha256"] = backend.digest(original)
                self.write_manifest()

    def test_binding_swaps_invalid_profiles_paths_and_hosted_remapping_refuse(self):
        self.compose()
        def hosted(index):
            index["routes"][-2].update(identity=HOSTED, strategy_id="hosted")
        def swap(index):
            index["routes"][-2].update(family="official-playwright", profile="isolated-chrome")
        edits = [hosted, swap,
                 lambda index: index["components"]["official-github"].update(path="../github-mcp-server"),
                 lambda index: index["components"]["official-github"].update(profiles=["other"]),
                 lambda index: index.update(schema_version=1),
                 lambda index: index.update(schema_version=True)]
        original = (self.bundle / "routes.json").read_bytes()
        for edit in edits:
            with self.subTest(edit=edits.index(edit)):
                self.change_index(edit)
                c = self.controller()
                with self.assertRaises(backend.SetupError):
                    c.catalog_entry(BINDINGS[0]["identity"])
                self.assertEqual(c.runner.calls, [])
                (self.bundle / "routes.json").write_bytes(original)

    def test_packager_refusals_do_not_mutate_inputs(self):
        cases = [lambda bindings, files: bindings[0].update(identity=HOSTED, strategy_id="hosted"),
                 lambda bindings, files: bindings[0]["identity"].update(version="metadata-sha256:" + "e" * 64),
                 lambda bindings, files: bindings[0].update(path="playwright/package-lock.json"),
                 lambda bindings, files: bindings.append(copy.deepcopy(bindings[0])),
                 lambda bindings, files: files.update({"official/github/github-mcp-server": (b"tamper", 0o755)}),
                 lambda bindings, files: files.pop("setup/install-library-consumer.py")]
        for mutate in cases:
            with self.subTest(case=cases.index(mutate)):
                bindings, files = copy.deepcopy(BINDINGS), copy.deepcopy(self.files)
                mutate(bindings, files)
                before = copy.deepcopy(files)
                with self.assertRaises(packager.PackageError):
                    packager.compose_official_routes(files, bindings)
                self.assertEqual(files, before)

    def test_recovery_and_closed_admission_never_gain_route_access(self):
        self.compose()
        c = self.selected()
        c.recovery_only = True
        with self.assertRaisesRegex(backend.SetupError, "recovery_read_only"):
            c.catalog_route(self.route_request(c))
        c.recovery_only = False
        c.close_admission()
        with self.assertRaisesRegex(backend.SetupError, "closing"):
            c.catalog_route(self.route_request(c))
        self.assertEqual(c.runner.calls, [])


if __name__ == "__main__":
    unittest.main()
