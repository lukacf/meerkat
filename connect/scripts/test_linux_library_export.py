"""Linux export selection and exact component/dependency bytes; no packages execute."""
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location("linux_export", ROOT / "scripts/export-library-consumers.py")
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)
TARGET = "linux-x86_64-cpython311"


class LinuxExportTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.files, cls.origins = MODULE.collect(ROOT, TARGET)

    def test_current_components_match_mac_while_linux_dependencies_stay_pinned(self):
        darwin, _ = MODULE.collect(ROOT)
        for path, expected in (("agno/" + MODULE.AGNO_WHEEL, MODULE.LINUX_AGNO_SHA),
                               ("services/" + MODULE.LINUX_SERVICES_WHEEL, MODULE.LINUX_SERVICES_SHA),
                               ("activepieces/" + MODULE.AP_ARCHIVE, MODULE.LINUX_AP_SHA)):
            self.assertEqual(MODULE.sha(self.files[path]), expected)
            self.assertEqual(self.files[path], darwin[path])
        self.assertEqual(self.files["catalog.json"], darwin["catalog.json"])
        self.assertEqual(json.loads(self.files["catalog.json"])[:101],
                         json.loads((ROOT / MODULE.LINUX_CATALOG).read_bytes()))
        closure = json.loads((ROOT / MODULE.LINUX_DEPENDENCY_CLOSURE).read_bytes())
        previous = ROOT / "dist/consumer-dependencies-20261003-credential-env-linux-x86_64"
        old = json.loads((previous / "closure.json").read_bytes())
        self.assertEqual(closure["artifacts"], old["artifacts"])
        self.assertEqual(len(closure["artifacts"]), 115)
        for row in closure["artifacts"]:
            self.assertEqual((ROOT / MODULE.LINUX_DEPENDENCIES / row["path"]).read_bytes(),
                             (previous / row["path"]).read_bytes())
        self.assertNotEqual(self.files["services/install-offline.txt"], darwin["services/install-offline.txt"])

    def test_successor_closures_rebind_selected_inputs_and_keep_object_roots(self):
        for target in MODULE.TARGETS:
            with self.subTest(target=target):
                objects, path, pins, _ = MODULE.dependency_selection(target)
                closure = json.loads((ROOT / path).read_bytes())
                predecessor = json.loads((ROOT / objects / "closure.json").read_bytes())
                self.assertNotEqual(path, objects + "/closure.json")
                self.assertEqual(closure["artifacts"], predecessor["artifacts"])
                self.assertEqual(closure["python_platform"], predecessor["python_platform"])
                old_inputs = dict(predecessor["accepted_inputs"])
                old_inputs.pop("services/meerkat_toolkit_services-0.1.2-py3-none-any.whl")
                old_inputs.pop("agno/meerkat_toolkit_agno-0.1.0-py3-none-any.whl")
                selected = MODULE.service_selection(target)
                old_inputs["services/" + selected["wheel"]] = selected["sha256"]
                old_inputs["agno/" + MODULE.AGNO_WHEEL] = selected["agno_sha256"]
                old_inputs["activepieces/" + MODULE.AP_ARCHIVE] = selected["ap_sha256"]
                old_inputs["activepieces/package-lock.json"] = MODULE.sha(
                    (ROOT / selected["ap_seed"] / "package-lock.json").read_bytes())
                self.assertEqual(closure["accepted_inputs"], old_inputs)
                self.assertEqual(pins[path], MODULE.sha((ROOT / path).read_bytes()))

    def test_rebound_dependency_record_or_component_is_refused(self):
        read = MODULE.read_file
        for target in MODULE.TARGETS:
            _, path, _, _ = MODULE.dependency_selection(target)
            for mutation in ("platform", "artifact", "component", "predecessor"):
                with self.subTest(target=target, mutation=mutation):
                    closure = json.loads(read(ROOT, path))
                    if mutation == "platform":
                        closure["python_platform"]["machine"] = "wrong-target"
                    elif mutation == "artifact":
                        closure["artifacts"][0]["source"] = {"kind": "unreviewed"}
                    elif mutation == "component":
                        closure["accepted_inputs"]["services/" + MODULE.SERVICES_WHEEL] = "0" * 64
                    else:
                        closure["predecessor_closure"]["sha256"] = "0" * 64

                    def replaced(root, relative, expected=None):
                        original = read(root, relative, expected)
                        return MODULE.encoded(closure) if relative == path else original

                    with patch.object(MODULE, "read_file", side_effect=replaced):
                        with self.assertRaises(MODULE.ExportError):
                            MODULE.collect(ROOT, target)

    def test_retained_services_acceptance_cannot_borrow_old_component_success(self):
        read = MODULE.read_file
        for target in MODULE.TARGETS:
            selected = MODULE.service_selection(target)
            anchor = json.loads(read(ROOT, selected["acceptance"]))
            retained = anchor["retained_acceptance"]
            for mutation in ("old_component", "unsuccessful"):
                with self.subTest(target=target, mutation=mutation):
                    value = json.loads(read(ROOT, retained["path"]))
                    if mutation == "old_component":
                        value["wheel_sha256"] = "c49fcf9362b1cb37c0bfc6f982c77dff7a7f0a8b5024fcaee44ab20c0e259108"
                    else:
                        value["status"] = "unconfirmed"

                    def replaced(root, relative, expected=None):
                        original = read(root, relative, expected)
                        return MODULE.encoded(value) if relative == retained["path"] else original

                    with tempfile.TemporaryDirectory(prefix="toolkit-service-acceptance-") as temp:
                        output = Path(temp).resolve() / "bundle"
                        with patch.object(MODULE, "read_file", side_effect=replaced):
                            with self.assertRaisesRegex(MODULE.ExportError, "retained services acceptance"):
                                MODULE.export(ROOT, str(output), target)
                        self.assertFalse(output.exists())

    def test_agno_acceptance_and_artifact_follow_explicit_build_target(self):
        darwin = MODULE.service_selection(MODULE.TARGETS[0])
        linux = MODULE.service_selection(TARGET)
        for selected in (darwin, linux):
            acceptance = json.loads((ROOT / selected["agno_acceptance"]).read_bytes())
            self.assertEqual(acceptance["wheel_sha256"], selected["agno_sha256"])
            self.assertEqual(MODULE.sha((ROOT / selected["agno_artifact"]).read_bytes()),
                             selected["agno_sha256"])
            self.assertIn(selected["agno_acceptance"], selected["pins"])
        self.assertIn(MODULE.AGNO_ACCEPTANCE, linux["pins"])
        self.assertEqual(linux["agno_sha256"], MODULE.AGNO_SHA)
        with tempfile.TemporaryDirectory(prefix="toolkit-agno-target-") as temp:
            destination = Path(temp).resolve() / "bundle"
            with patch.object(MODULE, "AGNO_SHA", "0" * 64), self.assertRaises(MODULE.ExportError):
                MODULE.export(ROOT, str(destination), MODULE.TARGETS[0])
            self.assertFalse(destination.exists())
        with self.assertRaises(MODULE.ExportError):
            MODULE.output_location(str(ROOT / Path(MODULE.AGNO_ACCEPTANCE).parent / "new-export"), ROOT)

    def test_wheel_inputs_contain_exactly_linux_closure_and_component(self):
        closure = json.loads((ROOT / MODULE.LINUX_DEPENDENCY_CLOSURE).read_bytes())
        for family, component, count in (("agno", MODULE.AGNO_WHEEL, 112),
                                         ("services", MODULE.LINUX_SERVICES_WHEEL, 32)):
            expected = {row["filename"] for row in closure["artifacts"]
                        if row["ecosystem"] == "python" and family in row["families"]} | {component}
            actual = {p.split("/", 1)[1] for p in self.files if p.startswith(family + "/") and p.endswith(".whl")}
            self.assertEqual(actual, expected)
            self.assertEqual(len(actual), count)
            self.assertFalse(any("macosx" in p for p in actual))
            self.assertEqual(self.files[family + "/install-requirements.txt"].decode().splitlines(),
                             ["./" + p for p in sorted(expected)])
            self.assertEqual(len(self.files[family + "/install-offline.txt"].splitlines()), count)

    def test_export_binds_exact_runtime_target_and_every_file(self):
        with tempfile.TemporaryDirectory(prefix="toolkit-linux-export-") as temp:
            destination = Path(temp).resolve() / "bundle"
            result = MODULE.export(ROOT, str(destination), TARGET)
            contents = json.loads((destination / "CONTENTS.json").read_bytes())
            self.assertEqual(contents["schema_version"], 2)
            self.assertEqual(contents["runtime_target"], MODULE.LINUX_RUNTIME_TARGET)
            self.assertEqual(contents["accepted_inputs"][MODULE.LINUX_DEPENDENCY_CLOSURE], MODULE.LINUX_CLOSURE_SHA)
            self.assertEqual(result["contents_sha256"], MODULE.sha((destination / "CONTENTS.json").read_bytes()))
            for path, row in contents["files"].items():
                self.assertEqual(MODULE.sha((destination / path).read_bytes()), row["sha256"])
            self.assertEqual(contents["files"]["README.md"]["source"]["path"], "docs/adoption/LINUX-CONSUMERS.md")

    def test_unknown_or_rebound_target_cannot_create_output(self):
        with tempfile.TemporaryDirectory(prefix="toolkit-linux-target-") as temp:
            destination = Path(temp).resolve() / "bundle"
            with self.assertRaises(MODULE.ExportError):
                MODULE.export(ROOT, str(destination), "choose-for-me")
            target = json.loads(json.dumps(MODULE.LINUX_RUNTIME_TARGET))
            target["python"]["version"] = "3.11.17"
            with patch.object(MODULE, "LINUX_RUNTIME_TARGET", target), self.assertRaises(MODULE.ExportError):
                MODULE.export(ROOT, str(destination), TARGET)
            self.assertFalse(destination.exists())

    def test_linux_inputs_and_evidence_trees_are_protected(self):
        for parent in (ROOT / MODULE.LINUX_DEPENDENCIES, ROOT / "docs/verification/20261003-linux-consumers"):
            with self.subTest(parent=parent), self.assertRaises(MODULE.ExportError):
                MODULE.output_location(str(parent / "new-export"), ROOT)

    def test_target_specific_catalog_operations_and_artifacts_remain_bound(self):
        darwin, _ = MODULE.collect(ROOT)
        for files, wheel, version, contains_permalink in (
            (darwin, MODULE.SERVICES_WHEEL, MODULE.service_selection(MODULE.TARGETS[0])["version"], True),
            (self.files, MODULE.LINUX_SERVICES_WHEEL, MODULE.service_selection(TARGET)["version"], True),
        ):
            catalog = json.loads(files["catalog.json"])
            self.assertEqual(len(catalog), 111)
            slack = next(row for row in catalog if row["identity"]["name"] == "slack")
            exports = {row["id"] for row in slack["known_exports"]}
            self.assertEqual("slack_get_permalink" in exports, contains_permalink)
            self.assertIn(wheel, slack["strategies"][1]["instructions"])
            selected = json.loads(files["services/slack.json"])["operations"]
            self.assertEqual("get_permalink" in selected, contains_permalink)
            gmail = next(row for row in catalog if row["identity"]["name"] == "gmail")
            send = next(row for row in gmail["known_exports"] if row["id"] == "gmail_send_message")
            self.assertIn("effect write", send["description"])
            self.assertIn("send_message", json.loads(files["services/gmail.json"])["operations"])
            self.assertIn("gmail_get_profile", {row["id"] for row in gmail["known_exports"]})
            self.assertIn("allowed_recipient_addresses", gmail["strategies"][1]["instructions"])
            self.assertIn("allowed_channel_ids", slack["strategies"][1]["instructions"])
            self.assertNotIn("services/install-google-oauth-offline.txt", files)
            self.assertFalse(any(path.startswith("services/google_auth-") for path in files))
            requirements = files["services/install-offline.txt"].decode()
            self.assertIn("meerkat-toolkit-services==" + version + " --hash=sha256:", requirements)
        frozen = ROOT / "dist/local-consumers-20261003-credential-env-linux-final"
        self.assertTrue((frozen / "services/meerkat_toolkit_services-0.1.0-py3-none-any.whl").is_file())
        self.assertNotIn("get_permalink", json.loads((frozen / "services/slack.json").read_bytes())["operations"])
        self.assertEqual(MODULE.sha((frozen / "CONTENTS.json").read_bytes()),
                         "c5294465563510c6e6b1c27f2c1fe7a44bbba6eadc3da4bdfcbac2bddfcd1e13")
        self.assertEqual(MODULE.sha(darwin["install-library-consumer.py"]), MODULE.INSTALLER_SHA)
        self.assertEqual(MODULE.sha(self.files["install-library-consumer.py"]), MODULE.LINUX_INSTALLER_SHA)

    def test_linux_service_graph_rejects_invalid_seed_and_retains_platform_hashes(self):
        read = MODULE.read_file
        path = MODULE.SERVICES_SEED + "/install-offline.txt"
        accepted = read(ROOT, path)
        lines = accepted.decode("ascii").splitlines()
        name, rest = lines[0].split("==", 1)
        variants = [
            "\n".join(lines[1:]) + "\n",
            accepted.decode() + lines[0] + "\n",
            accepted.decode() + name.upper().replace("-", "_") + "==" + rest + "\n",
            accepted.decode().replace(lines[0], name + "==999 --hash=sha256:" + "0" * 64),
            accepted.decode().replace(lines[0], lines[0].split(" --hash")[0]),
            accepted.decode() + "--extra-index-url https://invalid.example\n",
        ]
        def changed(data):
            def replacement(root, relative, expected=None):
                original = read(root, relative, expected)
                return data if relative == path else original
            return replacement
        for value in variants:
            with self.subTest(value=value[:80]), patch.object(MODULE, "read_file", side_effect=changed(value.encode())), self.assertRaises(MODULE.ExportError):
                MODULE.collect(ROOT, TARGET)
        reordered = "\n".join(name.upper().replace("-", "_") + "==" + rest
                              for name, rest in (line.split("==", 1) for line in reversed(lines))) + "\n"
        with patch.object(MODULE, "read_file", side_effect=changed(reordered.encode())):
            files, _ = MODULE.collect(ROOT, TARGET)
        self.assertEqual(files["services/install-offline.txt"], self.files["services/install-offline.txt"])


if __name__ == "__main__":
    unittest.main()
