"""Official historical requests are candidates under current pinned owners."""
import copy
import json
from pathlib import Path
import unittest
from unittest.mock import patch

import test_official_github as github
import test_official_playwright as playwright

backend = github.backend
CANARY = "PRIVATE_UNAPPLIED_HISTORICAL_VALUE"


class OfficialSuccessorMixin:
    def emitted(self):
        controller = self.controller()
        checked = self.checked(controller)
        job = controller.install(checked)["job"]
        controller.join()
        self.assertEqual(controller.jobs[job["id"]]["status"], "succeeded")
        path = controller.workspace / "requests" / (job["id"] + ".json")
        raw = path.read_bytes()
        record = json.loads(raw)
        self.assertEqual(record["schema_version"], self.schema)
        self.assertEqual(record["official_component"], self.component)
        self.assertEqual(backend.configuration_bytes(record["selection"]),
            (controller.workspace / "configurations" / (record["validation_id"] + ".json")).read_bytes())
        return controller, record, raw

    def historical(self, raw):
        path = self.root / "historical-request.json"
        path.write_bytes(raw)
        return {"prior_request_path": path, "prior_request_sha256": backend.digest(raw)}

    def proposal(self, controller):
        return controller.successor({"prior_request_sha256": controller.prior_request_sha256})

    def replace_current_pins(self):
        guide = self.bundle / self.component / "UPSTREAM-PROFILE.md"
        guide.write_bytes(guide.read_bytes() + b"\nCurrent successor guide.\n")
        contents = self.bundle / "CONTENTS.json"
        manifest = json.loads(contents.read_bytes())
        manifest["files"][self.component + "/UPSTREAM-PROFILE.md"] = {
            "sha256": backend.digest(guide.read_bytes()), "bytes": len(guide.read_bytes())}
        contents.write_bytes(backend.encoded(manifest))
        if self.component == "github":
            self.sha = backend.digest(contents.read_bytes())
        else:
            self.digest = backend.digest(contents.read_bytes())
        installer = self.root / "current-installer.py"
        installer.write_bytes(github.INSTALLER.read_bytes() + b"\n# Current host-selected installer pin.\n")
        changes = {"official_installer": installer,
                   "official_installer_sha256": backend.digest(installer.read_bytes())}
        if self.component == "playwright":
            profile = json.loads(self.profile.read_bytes())
            parent = self.root / "q"
            parent.mkdir()
            profile["process_parent"] = str(parent)
            self.profile.write_bytes(backend.encoded(profile))
        return changes

    def test_emitted_request_reuses_current_pins_and_requires_explicit_check_install(self):
        previous, record, raw = self.emitted()
        self.assertEqual(backend.prior_request(raw), record)
        args = self.historical(raw)
        changes = self.replace_current_pins()
        current = self.controller(**args, **changes)
        contents = current.input_binding(self.family)[0]
        self.assertNotEqual(contents, record["contents_sha256"])
        self.assertNotEqual(current.input_binding(self.family)[1], record["installer_sha256"])
        first = self.proposal(current)
        self.assertEqual(first["status"], "preparation_required")
        self.assertEqual(first["current_contents_sha256"], contents)
        self.assertEqual(current.state()["family_contents_sha256"][self.family], contents)
        self.assertEqual(current.runner.calls, [])
        self.assertEqual(current.validations, {})
        current.prepare({"family": self.family})
        current.join()
        proposed = self.proposal(current)
        self.assertEqual(proposed["status"], "review_required")
        self.assertEqual(proposed["values"], record["selection"]["values"])
        self.assertEqual(proposed["prior_operations"], record["selection"]["operations"])
        self.assertEqual(proposed["unapplied"], [])
        self.assertIs(proposed["help"]["selectable_operations"], False)
        self.assertEqual(current.validations, {})
        self.assertTrue(all("--destination" not in argv for argv in current.runner.calls))
        for argv in current.runner.calls:
            self.assertEqual(argv[argv.index("--contents-sha256") + 1], contents)
            self.assertEqual(argv[3], str(current.official_installer))
        selected = self.request()
        checked = current.check(selected)
        self.assertEqual(checked["contents_sha256"], contents)
        installed = current.install({**selected, "validation_id": checked["validation_id"],
                                     "configuration_sha256": checked["configuration_sha256"]})["job"]
        current.join()
        self.assertEqual(current.jobs[installed["id"]]["status"], "succeeded")
        next_record = json.loads((current.workspace / "requests" / (installed["id"] + ".json")).read_bytes())
        self.assertEqual(next_record["contents_sha256"], contents)
        self.assertEqual(next_record["installer_sha256"], changes["official_installer_sha256"])
        if self.component == "playwright":
            self.assertNotEqual(next_record["playwright_host_profile_sha256"], record["playwright_host_profile_sha256"])
            self.assertEqual(next_record["playwright_host_profile_sha256"], current.playwright_host_profile_sha256)
        self.assertEqual(args["prior_request_path"].read_bytes(), raw)
        self.assertEqual((current.workspace / "prior-request.json").read_bytes(), raw)
        self.assertNotIn(str(previous.workspace), json.dumps(proposed))

    def test_historical_paths_are_never_dereferenced_and_unknown_fields_remain_unapplied(self):
        _, record, _ = self.emitted()
        record["workspace"] = "/UNAVAILABLE_HISTORICAL_WORKSPACE"
        record["runtimes"] = {name: "/UNAVAILABLE_HISTORICAL_" + name for name in record["runtimes"]}
        record["contents_sha256"], record["installer_sha256"] = "1" * 64, "2" * 64
        if self.schema == 3:
            record["playwright_host_profile_sha256"] = "3" * 64
        record["selection"]["operations"].append("retired_operation")
        record["selection"]["values"]["retired_field"] = CANARY
        record["configuration_sha256"] = backend.digest(backend.configuration_bytes(record["selection"]))
        raw = backend.encoded(record)
        original = backend.read_file
        def read(root, *args, **kwargs):
            self.assertNotIn("UNAVAILABLE_HISTORICAL", str(root))
            return original(root, *args, **kwargs)
        with patch.object(backend, "read_file", side_effect=read):
            current = self.controller(**self.historical(raw))
            current.prepare({"family": self.family})
            current.join()
            proposed = self.proposal(current)
        self.assertEqual(proposed["status"], "review_required")
        self.assertEqual({(item["kind"], item["name"]) for item in proposed["unapplied"]},
                         {("operation", "retired_operation"), ("field", "retired_field")})
        self.assertNotIn(CANARY, json.dumps(proposed))
        self.assertNotIn("UNAVAILABLE_HISTORICAL", json.dumps(proposed))
        self.assertEqual(proposed["help"]["operations"], current.family(self.family)["profiles"][0]["operations"])
        self.assertEqual(current.validations, {})
        self.assertTrue(all("UNAVAILABLE_HISTORICAL" not in str(argv) for argv in current.runner.calls))
        self.assertEqual((current.workspace / "prior-request.json").read_bytes(), raw)

    def test_schema_family_component_and_content_tampering_refuse(self):
        _, original, _ = self.emitted()
        edits = [lambda r: r.update(schema_version=True), lambda r: r.update(schema_version=1),
                 lambda r: r.update(schema_version=4), lambda r: r.update(official_component="other"),
                 lambda r: r.pop("official_component"), lambda r: r.update(extra="unknown"),
                 lambda r: r["selection"].update(family="services"),
                 lambda r: r.update(configuration_sha256="f" * 64),
                 lambda r: r["selection"]["values"].update(retired_field="unbound")]
        if self.schema == 3:
            edits.extend([lambda r: r.pop("playwright_host_profile_sha256"),
                          lambda r: r.update(playwright_host_profile_sha256=True),
                          lambda r: r.update(playwright_host_profile_sha256="bad")])
        else:
            edits.append(lambda r: r.update(playwright_host_profile_sha256="3" * 64))
        for index, edit in enumerate(edits):
            record = copy.deepcopy(original)
            edit(record)
            with self.subTest(index=index), self.assertRaisesRegex(backend.SetupError, "invalid_prior_request"):
                backend.prior_request(backend.encoded(record))
        raw = backend.encoded(original).replace(b'"schema_version":', b'"schema_version":1,"schema_version":', 1)
        with self.assertRaisesRegex(backend.SetupError, "invalid_prior_request"):
            backend.prior_request(raw)

    def test_frozen_prior_or_current_pins_changed_after_preparation_refuse(self):
        _, record, raw = self.emitted()
        current = self.controller(**self.historical(raw))
        current.prepare({"family": self.family})
        current.join()
        targets = [current.workspace / "prior-request.json", current.official_installer,
                   current.official_bundle / "CONTENTS.json"]
        if self.schema == 3:
            targets.append(current.playwright_host_profile)
        for target in targets:
            with self.subTest(target=target.name):
                before = target.read_bytes()
                calls = len(current.runner.calls)
                target.write_bytes(b"{}")
                try:
                    with self.assertRaises(backend.SetupError):
                        self.proposal(current)
                    self.assertEqual(len(current.runner.calls), calls)
                finally:
                    target.write_bytes(before)
        self.assertEqual(current.validations, {})
        self.assertTrue(all("--destination" not in argv for argv in current.runner.calls))


class GithubSuccessorTests(OfficialSuccessorMixin, github.Fixture, unittest.TestCase):
    family, component, schema = "official-github", "github", 2


class PlaywrightSuccessorTests(OfficialSuccessorMixin, playwright.Fixture, unittest.TestCase):
    family, component, schema = "official-playwright", "playwright", 3


if __name__ == "__main__":
    unittest.main()
