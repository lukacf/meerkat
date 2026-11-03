"""Backend/installer composition with an explicitly inert artifact fixture.

The runner calls the real installer owner, replacing only its selected binary
and license digests and platform in this fixture. It never executes GitHub or reads a PAT.
These are source regression controls, not actual artifact or GHE acceptance.
"""
import argparse
import os
from pathlib import Path
import unittest
from unittest.mock import patch

import test_official_github as legacy
from test_official_github_targets import elf

backend, owner = legacy.backend, legacy.owner


def repin_inventory(root):
    manifest = backend.decode((root / "CONTENTS.json").read_bytes())
    for name in manifest["files"]:
        data = (root / name).read_bytes()
        manifest["files"][name] = {"sha256": backend.digest(data), "bytes": len(data)}
    (root / "CONTENTS.json").write_bytes(backend.encoded(manifest))
    return backend.digest((root / "CONTENTS.json").read_bytes())


def enterprise_bundle(root):
    raw = elf()
    (root / "github/github-mcp-server").write_bytes(raw)
    delivery = backend.decode((root / "delivery.json").read_bytes())
    delivery["components"][0].update(profile=owner.GHE_PROFILE,
        source_revision=owner.GHE_SOURCE_REVISION, artifact_sha256=backend.digest(raw),
        selected_operations=list(owner.GHE_TOOLS), target="linux-x86_64")
    (root / "delivery.json").write_bytes(backend.encoded(delivery))
    (root / "github/mcp.template.json").write_bytes(backend.encoded({"mcpServers": {
        "github-enterprise-reads": {"command": "ABS_GITHUB_INSTALL/github-mcp-server",
            "cwd": "ABS_GITHUB_INSTALL", "env": {}, "args": ["stdio", "--read-only",
                "--gh-host", "SELECTED_GHE_HTTPS_ORIGIN", "--toolsets=", "--tools", "SELECTED_READ_TOOLS"]}}}))
    manifest = backend.decode((root / "CONTENTS.json").read_bytes())
    for name in list(manifest["files"]):
        if name.startswith("github/notices/") and name != "github/notices/upstream-LICENSE":
            (root / name).unlink()
            del manifest["files"][name]
    (root / "CONTENTS.json").write_bytes(backend.encoded(manifest))
    return repin_inventory(root)


class OwnerRunner(backend.Runner):
    def __init__(self, workspace):
        super().__init__(workspace)
        self.calls = []
        self.drop = False

    def run(self, argv, *, capture=False):
        self.calls.append(list(argv))
        if "--official-bundle" not in argv or Path(argv[3]).read_bytes() != legacy.INSTALLER.read_bytes():
            raise AssertionError("only the exact installer may be invoked")
        fields = {"official_bundle": None, "contents_sha256": None, "component": None,
            "config": None, "config_sha256": None, "server_name": None, "destination": None,
            "official_describe": False, "check_config_only": False, "verify_existing": False}
        cursor = 4
        while cursor < len(argv):
            field = argv[cursor].removeprefix("--").replace("-", "_")
            if field not in fields:
                raise AssertionError("unexpected installer argument")
            if type(fields[field]) is bool:
                fields[field] = True
                cursor += 1
            else:
                fields[field] = argv[cursor + 1]
                cursor += 2
        # This source-local substitution is never applied to a production
        # installer file, bundle, receipt pin or real binary qualification.
        with patch.object(owner, "GHE_ARTIFACT_SHA256", backend.digest(elf())), \
                patch.object(owner, "GHE_LICENSE_SHA256", backend.digest(b"Fixture license")), \
                patch.object(owner.sys, "platform", "linux"), \
                patch("platform.machine", return_value="x86_64"), \
                patch.object(owner.subprocess, "Popen", side_effect=AssertionError("no GitHub process")):
            try:
                result = owner.official_consumer(argparse.Namespace(**fields))
            except (OSError, ValueError, KeyError, TypeError):
                return 2, b""
        if self.drop and fields["destination"] and not fields["verify_existing"]:
            return 2, b""
        return 0, backend.encoded(result)


class EnterpriseTests(legacy.Fixture, unittest.TestCase):
    def setUp(self):
        super().setUp()
        self.sha = enterprise_bundle(self.bundle)

    def request(self):
        return {"family": "official-github", "profile": owner.GHE_PROFILE,
            "operations": list(owner.GHE_TOOLS[:2]), "values": {
                "token_env": owner.OFFICIAL_ENV, "github_host": "https://github.enterprise.example"}}

    def controller(self, **updates):
        updates.setdefault("runner_factory", OwnerRunner)
        updates.setdefault("verifier", legacy.INSTALLER.resolve())
        updates.setdefault("verifier_sha256", backend.digest(legacy.INSTALLER.read_bytes()))
        return super().controller(**updates)

    def installed(self, controller, request=None):
        request = self.request() if request is None else request
        checked = controller.check(request)
        selected = dict(request, validation_id=checked["validation_id"],
            configuration_sha256=checked["configuration_sha256"])
        job = controller.install(selected)["job"]
        controller.join()
        return selected, job, controller.install_intents[job["id"]]

    def prepared(self):
        controller = self.controller()
        controller.prepare({"family": "official-github"})
        controller.join()
        self.assertEqual(controller.job["status"], "succeeded")
        return controller

    def test_only_selected_enterprise_profile_is_advertised_and_legacy_stays_fixed(self):
        controller = self.prepared()
        self.assertEqual([row["id"] for row in controller.families["official-github"]["profiles"]], [owner.GHE_PROFILE])
        help_value = controller.help({"family": "official-github", "profile": owner.GHE_PROFILE})
        self.assertTrue(help_value["selectable_operations"])
        self.assertEqual([row["id"] for row in help_value["operations"]], list(owner.GHE_TOOLS))
        with self.assertRaises(backend.SetupError):
            controller.help({"family": "official-github", "profile": "selected-reads"})
        old = self.root / "legacy"
        pin = legacy.bundle(old, owner.OFFICIAL_MAPPED_REVISION)
        # Metadata checks are platform independent; the old macOS payload is
        # never installed through this Linux fixture runner.
        previous = self.controller(official_bundle=old, official_contents_sha256=pin)
        previous.prepare({"family": "official-github"}); previous.join()
        self.assertEqual(previous.job["status"], "succeeded")
        self.assertFalse(previous.help({"family": "official-github", "profile": "selected-reads"})["selectable_operations"])
        with self.assertRaises(backend.SetupError):
            previous.help({"family": "official-github", "profile": owner.GHE_PROFILE})

    def test_actual_install_readback_and_redacted_metadata_bind_selected_host_and_subset(self):
        controller = self.prepared()
        selected, job, intent = self.installed(controller)
        self.assertEqual(controller.job["status"], "succeeded")
        result = controller.reconcile({"job_id": job["id"]}, retained_setup_metadata=True)["observation"]
        self.assertEqual(result["status"], "matched")
        entry = result["result"]["mcp_configuration"]["mcpServers"][intent["configuration"]["server_name"]]
        self.assertEqual(entry, {"command": str(intent["destination"] / "github-mcp-server"),
            "cwd": str(intent["destination"]), "env": {}, "args": ["stdio", "--read-only",
                "--gh-host", self.request()["values"]["github_host"], "--toolsets=", "--tools", ",".join(selected["operations"])]})
        self.assertEqual(result["result"]["configuration_sha256"], selected["configuration_sha256"])
        metadata = result["setup_metadata"]
        self.assertEqual(metadata["selection"]["values"], {"token_env": owner.OFFICIAL_ENV})
        self.assertEqual(metadata["selection"]["operations"], selected["operations"])
        self.assertEqual(metadata["help"]["fields"], [
            {"name": "token_env", "kind": "string", "credential_reference": True},
            {"name": "github_host", "kind": "string", "credential_reference": False}])
        self.assertNotIn(self.request()["values"]["github_host"], backend.encoded(metadata).decode())
        self.assertEqual(backend.retained_request_summary(controller.workspace, {
            "id": job["id"], "sha256": job["recovery_request"]["sha256"]}), {
            "family": "official-github", "profile": owner.GHE_PROFILE, "validation_id": selected["validation_id"]})

    def test_distinct_exact_hosts_and_operation_subsets_never_share_launch(self):
        controller = self.prepared()
        first, _, first_intent = self.installed(controller)
        request = self.request()
        request["values"]["github_host"] = "https://other.enterprise.example"
        request["operations"] = list(owner.GHE_TOOLS[2:4])
        second, _, second_intent = self.installed(controller, request)
        self.assertNotEqual(first["configuration_sha256"], second["configuration_sha256"])
        commands = [controller.expected_command("official-github", row["destination"], row["configuration"])
                    for row in (first_intent, second_intent)]
        self.assertEqual(commands[0][4], first["values"]["github_host"])
        self.assertEqual(commands[1][4], request["values"]["github_host"])
        self.assertEqual(commands[1][-1], ",".join(request["operations"]))
        with self.assertRaises(backend.SetupError):
            controller.receipt("official-github", first_intent["destination"], second_intent["configuration"])

    def test_authenticated_identity_is_explicitly_selected_and_receipt_bound(self):
        controller = self.prepared()
        request = self.request()
        request["operations"] = ["get_me"]
        selected, job, intent = self.installed(controller, request)
        self.assertEqual(controller.job["status"], "succeeded")
        observed = controller.reconcile({"job_id": job["id"]}, retained_setup_metadata=True)["observation"]
        self.assertEqual(observed["status"], "matched")
        self.assertEqual(observed["setup_metadata"]["selection"]["operations"], ["get_me"])
        launch = observed["result"]["mcp_configuration"]["mcpServers"][intent["configuration"]["server_name"]]
        self.assertEqual(launch["args"], ["stdio", "--read-only", "--gh-host",
            request["values"]["github_host"], "--toolsets=", "--tools", "get_me"])
        self.assertEqual(launch["env"], {})
        self.assertNotIn("github_host", observed["setup_metadata"]["selection"]["values"])
        # A collection-only installation never gains identity access by a
        # broader catalog, and the identity-only receipt cannot prove it.
        collection, _, collection_intent = self.installed(controller)
        self.assertNotIn("get_me", collection["operations"])
        self.assertNotEqual(selected["configuration_sha256"], collection["configuration_sha256"])
        with self.assertRaises(backend.SetupError):
            controller.receipt("official-github", intent["destination"], collection_intent["configuration"])

    def test_identity_combines_in_catalog_order_without_unknown_repository_operation(self):
        controller = self.prepared()
        request = self.request()
        request["operations"] = ["issue_read", "get_me"]
        self.assertTrue(controller.check(request)["validation_id"])
        for operations in (["get_me", "issue_read"], ["get_me", "get_me"], ["get_repository"]):
            with self.subTest(operations=operations), self.assertRaises(backend.SetupError):
                controller.check(dict(request, operations=operations))

    def test_enterprise_template_and_checked_launch_explicitly_clear_inherited_toolsets(self):
        # Exact argv binding only: this fixture does not execute the selected
        # GitHub program or independently prove its environment precedence.
        with patch.dict(os.environ, {"GITHUB_TOOLSETS": "all"}, clear=False):
            controller = self.prepared()
            selected, job, intent = self.installed(controller)
            self.assertEqual(controller.job["status"], "succeeded")
            template = backend.decode(controller.official_github_file("github/mcp.template.json"))
            self.assertEqual(template["mcpServers"]["github-enterprise-reads"]["args"], [
                "stdio", "--read-only", "--gh-host", "SELECTED_GHE_HTTPS_ORIGIN",
                "--toolsets=", "--tools", "SELECTED_READ_TOOLS"])
            observed = controller.reconcile({"job_id": job["id"]})["observation"]
            self.assertEqual(observed["status"], "matched")
            launch = observed["result"]["mcp_configuration"]["mcpServers"][intent["configuration"]["server_name"]]
            self.assertEqual(launch["args"], ["stdio", "--read-only", "--gh-host",
                selected["values"]["github_host"], "--toolsets=", "--tools", ",".join(selected["operations"])])
            self.assertEqual(launch["env"], {})
            self.assertEqual(controller.expected_command("official-github", intent["destination"],
                intent["configuration"]), [launch["command"], *launch["args"]])

    def test_config_required_and_digest_change_refuse_launch_projection(self):
        controller = self.prepared()
        _, _, intent = self.installed(controller)
        with self.assertRaises(backend.SetupError):
            controller.expected_command("official-github", intent["destination"])
        path = intent["configuration"]["path"]
        value = backend.decode(path.read_bytes()); value["config"]["github_host"] = "https://other.example"
        path.write_bytes(backend.encoded(value))
        with self.assertRaises(backend.SetupError):
            controller.expected_command("official-github", intent["destination"], intent["configuration"])

    def test_installer_refuses_host_or_operation_expansion_before_install(self):
        controller = self.prepared()
        invalid = []
        for host in ("http://ghe.example", "https://github.com", "https://api.github.com", "https://LOCAL.example", "https://127.0.0.1",
                     "https://localhost", "https://ghe.example/", "https://ghe.example:443", "https://u@ghe.example",
                     "https://ghe.example?x=1", "https://ghe.example#x", "https://[::1]",
                     "https://0x7f.0x0.0x0.0x1", "https://ghe.localhost", "https://ghe.example.",
                     "https://-ghe.example", "https://ghe_.example"):
            request = self.request(); request["values"]["github_host"] = host; invalid.append(request)
        for operations in ([], [owner.GHE_TOOLS[0]] * 2, list(reversed(owner.GHE_TOOLS[:2])), ["create_pull_request"]):
            request = self.request(); request["operations"] = operations; invalid.append(request)
        for request in invalid:
            with self.subTest(request=request), self.assertRaises(backend.SetupError):
                controller.check(request)
        self.assertFalse(any("--destination" in call for call in controller.runner.calls))

    def test_installer_rejects_rebound_enterprise_component_pins_and_unknown_profile(self):
        path = self.bundle / "delivery.json"
        original = path.read_bytes()
        # Repin CONTENTS after each mutation: the owner must reject the selected
        # component contract itself, not just a stale inventory digest.
        for field, value in (("source_revision", owner.OFFICIAL_MAPPED_REVISION),
                             ("artifact_sha256", "f" * 64), ("target", "macos-arm64"),
                             ("profile", "enterprise-unreviewed")):
            declaration = backend.decode(original)
            declaration["components"][0][field] = value
            path.write_bytes(backend.encoded(declaration))
            self.sha = repin_inventory(self.bundle)
            with self.subTest(field=field), \
                    patch.object(owner, "GHE_ARTIFACT_SHA256", backend.digest(elf())), \
                    patch.object(owner, "GHE_LICENSE_SHA256", backend.digest(b"Fixture license")), \
                    patch.object(owner.subprocess, "Popen", side_effect=AssertionError("no child")), \
                    patch.object(owner, "new_destination", side_effect=AssertionError("no install admission")):
                with self.assertRaises(owner.InstallError):
                    owner.official_consumer(self.args(official_describe=True))
        self.assertFalse((self.root / "installation").exists())

    def test_legacy_payload_cannot_be_relabelled_as_enterprise(self):
        old = self.root / "relabelled"
        legacy.bundle(old, owner.OFFICIAL_MAPPED_REVISION)
        path = old / "delivery.json"
        declaration = backend.decode(path.read_bytes())
        declaration["components"][0]["profile"] = owner.GHE_PROFILE
        path.write_bytes(backend.encoded(declaration))
        pin = repin_inventory(old)
        with patch.object(owner.subprocess, "Popen", side_effect=AssertionError("no child")), \
                patch.object(owner, "new_destination", side_effect=AssertionError("no install admission")):
            with self.assertRaises(owner.InstallError):
                owner.official_consumer(self.args(official_describe=True, official_bundle=str(old), contents_sha256=pin))
        self.assertFalse((self.root / "installation").exists())

    def test_installer_configuration_rejects_secret_values_and_alternate_launch_fields(self):
        original = backend.configuration_bytes(self.request())
        mutations = [
            ("config", "token_env", "OTHER_ACCOUNT"),
            ("config", "token_env", "ghp_PRIVATE_VALUE_MUST_NOT_ESCAPE"),
            ("config", "token", "PRIVATE_VALUE_MUST_NOT_ESCAPE"),
            ("config", "env", {owner.OFFICIAL_ENV: "PRIVATE_VALUE_MUST_NOT_ESCAPE"}),
            ("config", "args", ["--gh-host", "https://foreign.example"]),
            (None, "env", {owner.OFFICIAL_ENV: "PRIVATE_VALUE_MUST_NOT_ESCAPE"}),
            (None, "args", ["--tools", "create_pull_request"]),
            (None, "credential", "PRIVATE_VALUE_MUST_NOT_ESCAPE"),
        ]
        for parent, key, value in mutations:
            document = backend.decode(original)
            target = document if parent is None else document[parent]
            target[key] = value
            self.config.write_bytes(backend.encoded(document))
            # Use official_consumer's actual config entry point and fresh config
            # SHA, so parser refusal cannot be attributed to an input pin mismatch.
            with self.subTest(parent=parent, key=key), \
                    patch.object(owner, "GHE_ARTIFACT_SHA256", backend.digest(elf())), \
                    patch.object(owner, "GHE_LICENSE_SHA256", backend.digest(b"Fixture license")), \
                    patch.object(owner.subprocess, "Popen", side_effect=AssertionError("no child")), \
                    patch.object(owner, "new_destination", side_effect=AssertionError("no install admission")):
                with self.assertRaises(owner.InstallError) as refused:
                    owner.official_consumer(self.args(check_config_only=True))
                self.assertNotIn("PRIVATE_VALUE_MUST_NOT_ESCAPE", str(refused.exception))
        self.assertFalse((self.root / "installation").exists())

    def test_lost_install_return_preserves_unknown_and_recovers_exact_intent(self):
        controller = self.prepared(); controller.runner.drop = True
        selected, job, intent = self.installed(controller)
        self.assertEqual(controller.job["status"], "unconfirmed")
        controller.close_admission()
        recovered = self.controller(recover_workspace=controller.workspace, recover_request=job["id"],
            recover_request_sha256=job["recovery_request"]["sha256"])
        self.assertEqual(recovered.runner.calls, [])
        self.assertEqual(recovered.job["status"], "unconfirmed")
        observed = recovered.reconcile({"job_id": job["id"]}, retained_setup_metadata=True)["observation"]
        self.assertEqual(observed["status"], "matched")
        self.assertEqual(recovered.job["status"], "unconfirmed")
        self.assertEqual(observed["result"]["configuration_sha256"], selected["configuration_sha256"])
        self.assertEqual(observed["result"]["mcp_path"], str(intent["destination"] / "ready/mcp.json"))
        self.assertTrue(all(any(flag in call for flag in ("--verify-existing", "--official-describe", "--check-config-only"))
                            for call in recovered.runner.calls))

    def test_self_consistent_receipt_launch_tamper_cannot_override_selected_configuration(self):
        controller = self.prepared()
        _, job, intent = self.installed(controller)
        destination = intent["destination"]
        receipt_path, mcp_path = destination / "ready/installed.json", destination / "ready/mcp.json"
        receipt, mcp = backend.decode(receipt_path.read_bytes()), backend.decode(mcp_path.read_bytes())
        entry = mcp["mcpServers"][intent["configuration"]["server_name"]]
        entry["args"][3] = "https://foreign.example"
        receipt["command"] = [entry["command"], *entry["args"]]
        receipt["mcp_configuration"] = mcp
        receipt_path.write_bytes(backend.encoded(receipt)); mcp_path.write_bytes(backend.encoded(mcp))
        with self.assertRaises(backend.SetupError):
            controller.receipt("official-github", destination, intent["configuration"])
        self.assertEqual(controller.reconcile({"job_id": job["id"]})["observation"]["status"], "unconfirmed")

    def test_catalog_profile_matches_exact_selected_delivery_and_frozen_bytes(self):
        controller = self.prepared()
        component = {"path": "github/github-mcp-server", "sha256": backend.digest(elf()),
            "profiles": [owner.GHE_PROFILE], "contents_sha256": self.sha,
            "installer_sha256": backend.digest(legacy.INSTALLER.read_bytes())}
        controller.official_route_component("official-github", component)
        for profiles in (["selected-reads"], [owner.GHE_PROFILE, "selected-reads"], ["other"]):
            with self.subTest(profiles=profiles), self.assertRaises(backend.SetupError):
                controller.official_route_component("official-github", {**component, "profiles": profiles})
        path = controller.official_bundle / "github/mcp.template.json"
        value = backend.decode(path.read_bytes())
        value["mcpServers"]["github-enterprise-reads"]["args"][3] = "https://foreign.example"
        path.write_bytes(backend.encoded(value))
        with self.assertRaises(backend.SetupError):
            controller.official_route_component("official-github", component)


if __name__ == "__main__":
    unittest.main()
