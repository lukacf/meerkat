"""Adversarial checks for the build-time adoption-to-discovery boundary."""

import copy
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location("adopted_catalog", ROOT / "scripts/render-adopted-catalog.py")
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


class ProjectionTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.inventory = json.loads((ROOT / "docs/adoption/oss-inventory.json").read_bytes())

    def test_only_adopted_identities_and_selected_operations_are_exposed(self):
        output = MODULE.project(self.inventory)
        expected = {row["id"]: row for row in self.inventory["integrations"] if row["status"] == "adopted"}
        self.assertEqual(set(expected), {item["identity"]["name"] for item in output})
        for item in output:
            row = expected[item["identity"]["name"]]
            operations = row["implementation"]["selected_operations"] + [
                op for route in row.get("alternative_implementations", []) for op in route["selected_operations"]]
            self.assertEqual(operations, [op["id"] for op in item["known_exports"]])
            self.assertTrue(all(op["role"] == "service_connector" for op in item["known_exports"]))
            self.assertEqual(item["assessments"][0]["status"], "compatible")
            self.assertIn("does not install", item["assessments"][0]["scope"])
            self.assertEqual(item["strategies"][0]["unresolved_details"][:len(row["unknowns"])], row["unknowns"])

    def test_input_order_cannot_change_catalog_or_identity(self):
        shuffled = copy.deepcopy(self.inventory)
        shuffled["integrations"].reverse()
        shuffled["evidence"].reverse()
        self.assertEqual(MODULE.project(shuffled), MODULE.project(self.inventory))

    def test_changed_guidance_changes_immutable_metadata_identity(self):
        changed = copy.deepcopy(self.inventory)
        target = next(row for row in changed["integrations"] if row["status"] == "adopted")
        target["prerequisites"].append("Additional explicit host requirement.")
        before = {item["identity"]["name"]: item for item in MODULE.project(self.inventory)}
        after = {item["identity"]["name"]: item for item in MODULE.project(changed)}
        self.assertNotEqual(before.pop(target["id"])["identity"]["version"], after.pop(target["id"])["identity"]["version"])
        self.assertEqual(before, after)

    def test_candidate_and_revoked_support_are_not_promoted(self):
        changed = copy.deepcopy(self.inventory)
        target = next(row for row in changed["integrations"] if row["status"] == "adopted")
        target["status"] = "candidate"
        changed["counts"]["adopted"] -= 1
        changed["counts"]["candidate"] += 1
        self.assertNotIn(target["id"], {item["identity"]["name"] for item in MODULE.project(changed)})

    def test_mutated_evidence_cannot_retain_compatible_claim(self):
        changed = copy.deepcopy(self.inventory)
        target = next(row for row in changed["integrations"] if row["status"] == "adopted")
        record = next(item for item in changed["evidence"] if item["id"] in target["evidence_refs"] and item.get("kind") == "support-disposition")
        record["sha256"] = "0" * 64
        with self.assertRaisesRegex(ValueError, "evidence digest mismatch"):
            MODULE.project(changed)

    def test_changed_artifact_cannot_borrow_unchanged_acceptance(self):
        changed = copy.deepcopy(self.inventory)
        target = next(row for row in changed["integrations"] if row["id"] == "github")
        target["implementation"]["artifact_sha256"] = "0" * 64
        with self.assertRaisesRegex(ValueError, "artifact differs from the verified adoption receipt"):
            MODULE.project(changed)

    def test_unindexed_configuration_is_not_freshly_blessed(self):
        changed = copy.deepcopy(self.inventory)
        row = next(item for item in changed["integrations"] if item["id"] == "hacker-news")
        path = row["implementation"]["configuration_document"]
        ids = {item["id"] for item in changed["evidence"] if item.get("path") == path}
        row["evidence_refs"] = [reference for reference in row["evidence_refs"] if reference not in ids]
        with self.assertRaisesRegex(ValueError, "every emitted evidence path"):
            MODULE.project(changed)

    def test_changed_configuration_fails_against_its_reviewed_digest(self):
        row = next(item for item in self.inventory["integrations"] if item["id"] == "hacker-news")
        target = row["implementation"]["configuration_document"]
        original = MODULE.evidence_file

        def changed_read(root, path):
            data = original(root, path)
            return data + b" altered configuration" if path == target else data

        with patch.object(MODULE, "evidence_file", side_effect=changed_read):
            with self.assertRaisesRegex(ValueError, "evidence digest mismatch"):
                MODULE.project(self.inventory)

    def test_missing_evidence_and_duplicate_identity_fail(self):
        for mutation in ("evidence", "integrations"):
            changed = copy.deepcopy(self.inventory)
            if mutation == "evidence":
                changed[mutation] = []
            else:
                changed[mutation].append(copy.deepcopy(changed[mutation][0]))
            with self.assertRaises((KeyError, ValueError)):
                MODULE.project(changed)

    def test_path_traversal_and_outside_symlinks_fail(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "docs").mkdir()
            (root / "outside").symlink_to(ROOT / "docs")
            (root / "docs/link").symlink_to(ROOT / "docs")
            for path in ("../outside", "/etc/hosts", "docs/../outside", "docs/link/requirements.json"):
                with self.assertRaises((OSError, ValueError)):
                    MODULE.evidence_file(root, path)

    def test_frontend_role_cannot_borrow_service_adoption(self):
        changed = copy.deepcopy(self.inventory)
        next(row for row in changed["integrations"] if row["status"] == "adopted")["role"] = "agent_frontend"
        with self.assertRaisesRegex(ValueError, "frontend compatibility"):
            MODULE.project(changed)

    def test_alternative_routes_keep_primary_adoption_and_scope_their_exports(self):
        output = {item["identity"]["name"]: item for item in MODULE.project(self.inventory)}
        self.assertEqual(101, len(output))
        self.assertEqual({"gmail", "slack", "google-calendar", "google-drive"}, {name for name, item in output.items() if len(item["strategies"]) > 1})
        for name, count in (("gmail", 9), ("slack", 8), ("google-calendar", 5), ("google-drive", 4)):
            row = next(row for row in self.inventory["integrations"] if row["id"] == name)
            item = output[name]
            route = row["alternative_implementations"][0]
            self.assertEqual("activepieces-action-adapter", row["adoption_route"])
            self.assertEqual(row["sources"][0]["repository"], item["provenance"]["source"])
            self.assertEqual(["selected-local-mcp", "first-party-services-mcp"], [strategy["id"] for strategy in item["strategies"]])
            self.assertEqual(count + 1, len(item["known_exports"]))
            self.assertIn("primary strategy selected-local-mcp only", item["provenance"]["notices"][-1])
            self.assertIn("Strategy selected-local-mcp only; effect read", item["known_exports"][0]["description"])
            for export in item["known_exports"][1:]:
                self.assertIn("Strategy first-party-services-mcp only; effect " + route["operation_effects"][export["id"]], export["description"])
            self.assertEqual(route["provenance"]["source_revision"], item["assessments"][1]["source_revision"])
            self.assertEqual(MODULE.assessment_timestamp(route["assessed_at"]), item["assessments"][1]["assessed_at_unix_ms"])
            self.assertEqual(MODULE.assessment_timestamp(self.inventory["generated_at"]), item["assessments"][0]["assessed_at_unix_ms"])
            for index, strategy in enumerate(item["strategies"]):
                self.assertIn("Strategy " + strategy["id"] + " only.", item["assessments"][index]["scope"])
            self.assertNotEqual(item["assessments"][0]["evidence"], item["assessments"][1]["evidence"])

    def alternative(self, inventory):
        return next(row for row in inventory["integrations"] if row["id"] == "gmail")["alternative_implementations"][0]

    def test_duplicate_strategy_or_export_ownership_is_rejected(self):
        for mutation in ("strategy", "export", "operation"):
            with self.subTest(mutation=mutation):
                changed = copy.deepcopy(self.inventory)
                route = self.alternative(changed)
                if mutation == "strategy":
                    route["strategy_id"] = "selected-local-mcp"
                    expected = "duplicate strategy identity"
                elif mutation == "export":
                    route["selected_operations"].append("gmail_get_thread")
                    route["operation_effects"]["gmail_get_thread"] = "read"
                    expected = "ambiguous export ownership"
                else:
                    route["selected_operations"].append(route["selected_operations"][0])
                    expected = "unique selected operation"
                with self.assertRaisesRegex(ValueError, expected):
                    MODULE.project(changed)

    def test_alternative_frontend_role_is_rejected(self):
        changed = copy.deepcopy(self.inventory)
        self.alternative(changed)["role"] = "agent_frontend"
        with self.assertRaisesRegex(ValueError, "frontend compatibility"):
            MODULE.project(changed)

    def test_alternative_receipt_shape_and_assessment_time_are_explicit(self):
        for field, value, expected in (("receipt_kind", "unreviewed-provider", "unsupported alternative receipt"),
                                       ("assessed_at", "2026-10-03T03:36:25", "assessment time")):
            with self.subTest(field=field):
                changed = copy.deepcopy(self.inventory)
                self.alternative(changed)[field] = value
                with self.assertRaisesRegex(ValueError, expected):
                    MODULE.project(changed)

    def test_component_receipt_cannot_borrow_other_or_failed_installed_evidence(self):
        for mutation in ("binding", "missing_check", "failed_check", "profile_output", "component_wheel", "downgrade",
                         "component_failed", "review_wheel", "review_failed", "profile_wheel"):
            with self.subTest(mutation=mutation):
                changed = copy.deepcopy(self.inventory)
                route = self.alternative(changed)
                receipt_path = route["acceptance"]
                receipt = json.loads(MODULE.evidence_file(ROOT, receipt_path))
                replacements = {}
                if mutation == "binding":
                    receipt["evidence"]["profile_command"]["sha256"] = "0" * 64
                elif mutation == "missing_check":
                    receipt["evidence"]["installed_checks"].pop()
                elif mutation == "downgrade":
                    route["receipt_kind"] = "configured-services-consumer-v1"
                else:
                    if mutation == "failed_check":
                        binding = receipt["evidence"]["installed_checks"][0]
                    elif mutation.startswith("review_"):
                        binding = receipt["evidence"]["package_review"]
                    else:
                        binding = receipt["evidence"]["profile_command" if mutation in ("profile_output", "profile_wheel") else "component_acceptance"]
                    path = binding["path"]
                    data = json.loads(MODULE.evidence_file(ROOT, path))
                    key = {"failed_check": "exit_code", "profile_output": "stdout_sha256", "component_wheel": "wheel_sha256",
                           "component_failed": "status", "review_wheel": "final_wheel_sha256",
                           "review_failed": "status", "profile_wheel": "wheel_sha256"}[mutation]
                    data[key] = 1 if mutation == "failed_check" else "0" * 64
                    replacements[path] = json.dumps(data).encode()
                    binding["sha256"] = MODULE.digest(replacements[path])
                replacements[receipt_path] = json.dumps(receipt).encode()
                for item in changed["evidence"]:
                    if item.get("path") in replacements:
                        item["sha256"] = MODULE.digest(replacements[item["path"]])
                read = MODULE.evidence_file

                def replaced(root, path):
                    return replacements[path] if path in replacements else read(root, path)

                with patch.object(MODULE, "evidence_file", side_effect=replaced):
                    with self.assertRaises(ValueError):
                        MODULE.project(changed)

    def test_previous_v2_receipt_remains_readable_without_new_claims(self):
        path = "docs/verification/20261004-setup-distribution/services/promotion-evidence.json"
        receipt = json.loads(MODULE.evidence_file(ROOT, path))
        fields = {"component_acceptance": "installation_acceptance", "profile_command": "inspection_commands",
                  "profile_output": "profile_descriptor", "package_review": "review", "guide": "configuration_document"}
        alternative = {fields[key]: value["path"] for key, value in receipt["evidence"].items() if key in fields}
        alternative.update(receipt_kind="configured-services-consumer-v2", artifact_sha256=receipt["wheel_sha256"],
                           artifact_path="dist/services-consumer-20261003-structured-send/meerkat_toolkit_services-0.1.3-py3-none-any.whl",
                           provenance={"source_document": receipt["evidence"]["source_freeze"]["path"]})
        rows = [row for key, row in receipt["evidence"].items() if key != "installed_checks"] + receipt["evidence"]["installed_checks"]
        indexed = {row["path"]: MODULE.digest(MODULE.evidence_file(ROOT, row["path"])) for row in rows}
        MODULE.validate_services_component_receipt(receipt, alternative, indexed, ROOT)

    def test_v3_source_map_cannot_be_rebound_without_changing_accepted_wheel(self):
        changed = copy.deepcopy(self.inventory)
        route = self.alternative(changed)
        source_path = route["provenance"]["source_document"]
        source = json.loads(MODULE.evidence_file(ROOT, source_path))
        source["source_mapping"]["src/meerkat_toolkit_services/connectors/gmail.py"]["sha256"] = "0" * 64
        source_bytes = json.dumps(source).encode()
        source_digest = MODULE.digest(source_bytes)
        receipt_path = route["acceptance"]
        receipt = json.loads(MODULE.evidence_file(ROOT, receipt_path))
        receipt["source_freeze_sha256"] = source_digest
        receipt["evidence"]["source_freeze"]["sha256"] = source_digest
        route["provenance"]["source_revision"] = "sha256:" + source_digest
        replacements = {source_path: source_bytes, receipt_path: json.dumps(receipt).encode()}
        for row in changed["evidence"]:
            if row.get("path") in replacements:
                row["sha256"] = MODULE.digest(replacements[row["path"]])
        read = MODULE.evidence_file
        with patch.object(MODULE, "evidence_file", side_effect=lambda root, path: replacements.get(path) or read(root, path)):
            with self.assertRaisesRegex(ValueError, "source map differs from accepted wheel"):
                MODULE.project(changed)

    def test_alternative_cannot_invent_an_operation_or_relabel_a_write(self):
        for mutation in ("operation", "effect", "missing_effect", "profile"):
            with self.subTest(mutation=mutation):
                changed = copy.deepcopy(self.inventory)
                route = self.alternative(changed)
                if mutation == "operation":
                    route["selected_operations"].append("gmail_delete_message")
                    route["operation_effects"]["gmail_delete_message"] = "write"
                    expected = "export/effect differs"
                elif mutation == "effect":
                    route["operation_effects"]["gmail_send_raw"] = "read"
                    expected = "export/effect differs"
                elif mutation == "missing_effect":
                    del route["operation_effects"]["gmail_send_raw"]
                    expected = "own read/write effect"
                else:
                    route["profile"] = "slack"
                    expected = "this service identity"
                with self.assertRaisesRegex(ValueError, expected):
                    MODULE.project(changed)

    def test_alternative_artifact_requires_its_own_index_and_acceptance(self):
        for mutation in ("unindexed", "changed_bytes", "substituted_receipt"):
            with self.subTest(mutation=mutation):
                changed = copy.deepcopy(self.inventory)
                route = self.alternative(changed)
                original = MODULE.repository_file

                def changed_read(root, path, prefix):
                    data = original(root, path, prefix)
                    return data + b"changed artifact" if path == route["artifact_path"] else data

                artifact_evidence = next(item for item in changed["evidence"]
                    if item["id"] in route["evidence_refs"]
                    and item.get("artifact_path") == route["artifact_path"])
                if mutation == "unindexed":
                    route["evidence_refs"].remove(artifact_evidence["id"])
                    with self.assertRaisesRegex(ValueError, "exact indexed digest"):
                        MODULE.project(changed)
                elif mutation == "changed_bytes":
                    with patch.object(MODULE, "repository_file", side_effect=changed_read):
                        with self.assertRaisesRegex(ValueError, "artifact digest mismatch"):
                            MODULE.project(changed)
                else:
                    route["artifact_sha256"] = MODULE.digest(changed_read(ROOT, route["artifact_path"], "dist/"))
                    artifact_evidence["sha256"] = route["artifact_sha256"]
                    with patch.object(MODULE, "repository_file", side_effect=changed_read):
                        with self.assertRaisesRegex(ValueError, "artifact differs from its accepted consumer receipt"):
                            MODULE.project(changed)

    def test_alternative_document_bindings_cannot_borrow_compatibility(self):
        for mutation in ("unindexed", "changed_guide", "wrong_guide", "source", "descriptor"):
            with self.subTest(mutation=mutation):
                changed = copy.deepcopy(self.inventory)
                route = self.alternative(changed)
                if mutation == "unindexed":
                    reference = next(item["id"] for item in changed["evidence"]
                        if item["id"] in route["evidence_refs"]
                        and item.get("path") == route["configuration_document"])
                    route["evidence_refs"].remove(reference)
                    expected = "every alternative evidence path"
                elif mutation == "wrong_guide":
                    route["configuration_document"] = route["support"]
                    expected = "guide differs from its accepted source freeze"
                elif mutation == "source":
                    route["provenance"]["source_revision"] = "sha256:" + "0" * 64
                    expected = "source differs from its accepted consumer receipt"
                elif mutation == "descriptor":
                    route["profile_descriptor"] = route["support"]
                    expected = "profile descriptor differs from the accepted command output"
                else:
                    original = MODULE.evidence_file

                    def changed_read(root, path):
                        data = original(root, path)
                        return data + b"changed guide" if path == route["configuration_document"] else data

                    with patch.object(MODULE, "evidence_file", side_effect=changed_read):
                        with self.assertRaisesRegex(ValueError, "evidence digest mismatch"):
                            MODULE.project(changed)
                    continue
                with self.assertRaisesRegex(ValueError, expected):
                    MODULE.project(changed)


if __name__ == "__main__":
    unittest.main()
