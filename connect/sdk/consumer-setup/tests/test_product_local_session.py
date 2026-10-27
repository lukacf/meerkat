"""Product readback uses actual retained Controller requests and launcher custody.

The controlled package runner does not establish native MCP admission or grants.
"""
from __future__ import annotations

import json
import shutil
from pathlib import Path
import sys
import threading
from types import ModuleType, SimpleNamespace
import unittest
from unittest.mock import patch

import test_local_session as local_session_tests
import test_selected_setup as selected_setup
import test_export_recovery as remote_exports
from test_local_session import (ROOT, backend, fixtures, load, local,
                                reconciliation)


product = load("product_local_readback_test", ROOT / "product/prepare.py")
host = load("product_local_host_test", ROOT / "prepare_host.py")


class ProductLocalSessionTests(fixtures.FixtureMixin, unittest.TestCase):
    create = local_session_tests.LocalSessionTests.create

    def setUp(self):
        super().setUp()
        self.realm_root = self.root / "realm-state"
        self.prepared = self.realm_root / ".meerkat-connect/realm/setup-selected/prepared"
        (self.prepared / "config").mkdir(parents=True)
        self.release = self.prepared / "distribution/fixture"
        (self.release / "setup").mkdir(parents=True)
        self.helper_path = self.release / "setup/local_session.py"
        self.helper_raw = (ROOT / "local_session.py").read_bytes()
        self.helper_path.write_bytes(self.helper_raw)
        self.retained = {"setup/local_session.py": self.helper_raw}
        for name in ("backend.py", "mcp_adapter.py"):
            raw = (ROOT / name).read_bytes()
            (self.release / "setup" / name).write_bytes(raw)
            self.retained["setup/" + name] = raw
        self.configuration = self.prepared / "config/mcp.json"
        self.configuration.write_bytes(local.encoded({"mcpServers": {"toolkit-setup": {
            "command": "/usr/bin/env", "args": ["-i", "PATH=/usr/bin:/bin",
                "PYTHONDONTWRITEBYTECODE=1", str(Path(sys.executable).resolve()), "-I", "-B",
                str(self.release / "setup/mcp_adapter.py"), "--workspace", str(self.prepared / "session")]}}}))
        self.configuration_sha = local.digest(self.configuration.read_bytes())
        self.helper, self.binding, self.descriptor = self.project()
        self.state_root = self.prepared / "local-session"

    def project(self):
        return product.local_projection(host, self.prepared, self.release,
                                        self.retained, self.configuration_sha)

    def install(self, selected):
        owner = self.create(*selected, runner=reconciliation.ReconciliationRunner)
        owner.prepare({"family": "agno"}); owner.join()
        job = owner.install(self.checked(owner))["job"]; owner.join()
        return owner, {"job_id": job["id"],
                       "recovery_request_sha256": job["recovery_request"]["sha256"]}

    def observe(self, owner, selected, *, workspace=None, binding=None):
        return product.local_workspace_evidence(self.helper, backend, self.prepared,
            workspace or owner.workspace, binding or self.binding, selected)

    def test_readback_uses_checkpoint_while_live_lease_is_held_then_restart_is_read_only(self):
        with local.session_owner(self.state_root, self.binding) as retained:
            owner, selected = self.install(retained)
            files = {p: p.read_bytes() for p in self.state_root.rglob("*") if p.is_file()}
            evidence = self.observe(owner, selected)
            self.assertEqual(evidence[0], (self.state_root / "session.json").read_bytes())
            self.assertEqual(evidence[1], (self.state_root / "recovery.json").read_bytes())
            self.assertEqual({p: p.read_bytes() for p in files}, files)
            with self.assertRaisesRegex(local.LocalSetupError, "local_setup_already_open"):
                with local.session_owner(self.state_root, self.binding):
                    self.fail("parallel launcher entered the selected attempt")
            owner.close_admission()
        with local.session_owner(self.state_root, self.binding) as retained:
            recovered = self.create(*retained, runner=reconciliation.ReconciliationRunner)
            self.assertTrue(recovered.recovery_only)
            self.assertEqual(recovered.runner.calls, [])
            self.error("recovery_read_only", lambda: recovered.prepare({"family": "agno"}))
            self.assertEqual(recovered.reconcile({"job_id": selected["job_id"]})["observation"]["status"], "matched")
            self.assertTrue(all("--verify-existing" in args for args, _ in recovered.runner.calls))
            self.assertEqual(self.observe(owner, selected), evidence)

    def test_old_product_readback_uses_the_exact_retained_round_after_second_acquisition(self):
        with local.session_owner(self.state_root, self.binding) as retained:
            original, first = self.install(retained)
            previous = self.observe(original, first)[1]
            original.close_admission(); original.join()
        descriptor = self.project()[2]
        with local_session_tests.LiveRoundOffer(self, self.state_root, self.binding) as offer:
            with local.session_owner(self.state_root, self.binding) as retained:
                current, second = self.install(retained)
                offer.finish()
                self.assertNotEqual(original.workspace, current.workspace)
                self.assertEqual(self.observe(original, first)[1], previous)
                self.assertNotEqual(self.observe(current, second)[1], previous)
                self.assertEqual(self.project()[2], descriptor)
                wrong = {**first, "recovery_request_sha256": second["recovery_request_sha256"]}
                with self.assertRaisesRegex(product.ResourceError, "local_request_not_selected"):
                    self.observe(original, wrong)
                current.close_admission(); current.join()
        with local.session_owner(self.state_root, self.binding) as retained:
            recovered = self.create(*retained)
            self.assertTrue(recovered.recovery_only)
            self.assertEqual(self.observe(original, first)[1], previous)

    def test_foreign_workspace_and_unselected_request_refuse_without_creating_work(self):
        with local.session_owner(self.state_root, self.binding) as retained:
            owner, selected = self.install(retained)
            before = set(self.state_root.iterdir())
            foreign = self.root / "other-attempt" / owner.workspace.name
            with self.assertRaisesRegex(product.ResourceError, "local_attempt_mismatch"):
                self.observe(owner, selected, workspace=foreign)
            with self.assertRaisesRegex(product.ResourceError, "local_request_not_selected"):
                self.observe(owner, {**selected, "job_id": "f" * 32})
            with self.assertRaisesRegex(product.ResourceError, "local_request_not_selected"):
                self.observe(owner, {**selected, "recovery_request_sha256": "f" * 64})
            self.assertEqual(set(self.state_root.iterdir()), before)
            self.assertFalse(foreign.exists())

    def test_changed_prepared_configuration_or_helper_refuses_descriptor_reuse(self):
        for candidate in (self.configuration, self.helper_path,
                          self.helper_path.with_name("backend.py"), self.helper_path.with_name("mcp_adapter.py")):
            with self.subTest(candidate=candidate.name):
                original = candidate.read_bytes()
                candidate.write_bytes(original + b"\n")
                with self.assertRaisesRegex(ValueError, "local_setup_selection_changed"):
                    self.project()
                candidate.write_bytes(original)
        self.assertFalse(self.state_root.exists())
        self.assertEqual(self.project()[2], self.descriptor)

    def test_changed_session_binding_refuses_existing_checkpoint(self):
        with local.session_owner(self.state_root, self.binding) as retained:
            owner, selected = self.install(retained)
            saved = (self.state_root / "session.json").read_bytes()
            for field in self.binding:
                with self.subTest(field=field), self.assertRaisesRegex(ValueError, "session_changed"):
                    self.observe(owner, selected, binding={**self.binding, field: "f" * 64})
            self.assertEqual((self.state_root / "session.json").read_bytes(), saved)

    def test_missing_checkpoint_never_infers_no_effect_or_selects_new_workspace(self):
        with local.session_owner(self.state_root, self.binding) as retained:
            owner, selected = self.install(retained)
            (self.state_root / "recovery.json").unlink()
            before = set(self.state_root.iterdir())
            with self.assertRaisesRegex(ValueError, "previous_session_needs_review"):
                self.observe(owner, selected)
            self.assertEqual(set(self.state_root.iterdir()), before)
            self.assertTrue((owner.workspace / "requests" / (selected["job_id"] + ".json")).is_file())

    def test_partial_checkpoint_refuses_even_when_requested_job_is_in_old_selection(self):
        with local.session_owner(self.state_root, self.binding) as retained:
            owner, selected = self.install(retained)
            checkpoint = self.state_root / "recovery.json"
            old = checkpoint.read_bytes()
            later = owner.install(self.checked(owner))["job"]; owner.join()
            self.assertNotEqual(later["id"], selected["job_id"])
            checkpoint.write_bytes(old)
            before = set(self.state_root.iterdir())
            with self.assertRaisesRegex(ValueError, "recovery_selection_incomplete"):
                self.observe(owner, selected)
            self.assertEqual(set(self.state_root.iterdir()), before)
            self.assertEqual(checkpoint.read_bytes(), old)

    def test_changed_canonical_request_refuses_readback(self):
        with local.session_owner(self.state_root, self.binding) as retained:
            owner, selected = self.install(retained)
            request = owner.workspace / "requests" / (selected["job_id"] + ".json")
            request.write_bytes(request.read_bytes() + b" ")
            self.error("recovery_request_changed", lambda: self.observe(owner, selected))

    def test_full_product_readback_checks_realm_and_changes_during_actual_owner_read(self):
        self.full_product_readback("services")

    def test_public_library_readback_uses_the_same_canonical_verifier(self):
        self.full_product_readback("agno")

    def test_salesforce_account_readback_uses_the_current_selected_profile(self):
        self.full_product_readback("agno", account_profile="salesforce")

    def test_selected_python_public_readback_uses_the_prepared_owner_and_request(self):
        self.full_product_readback("selected-python")

    def test_selected_node_public_readback_uses_the_prepared_owner_and_request(self):
        self.full_product_readback("selected-node")

    def test_software_python_public_readback_uses_canonical_installed_declaration(self):
        self.full_product_readback("selected-python", local_software=True)

    def test_software_node_public_readback_uses_canonical_installed_declaration(self):
        self.full_product_readback("selected-node", local_software=True)

    def test_remote_public_readback_recovers_the_exact_export_without_install_or_replay(self):
        self.full_product_readback("remote-recipe")

    def test_credential_library_readback_retains_checked_slots_without_metadata_reexecution(self):
        self.full_product_readback("services", credential_service=True)

    def test_credential_selected_python_readback_requires_complete_declared_slots(self):
        self.full_product_readback("selected-python", credential_service=True)

    def test_credential_software_node_readback_requires_complete_declared_slots(self):
        self.full_product_readback("selected-node", local_software=True, credential_service=True)

    def test_remote_declaration_readback_uses_existing_product_owner_without_host_snapshots(self):
        self.full_product_readback("remote-recipe", remote_declaration=True)

    def test_retained_checkpoint_bounds_children_before_canonical_closure_or_foreign_scan(self):
        with local.session_owner(self.state_root, self.binding) as retained:
            owner, _ = self.install(retained)
            record = local.decode((self.state_root / "session.json").read_bytes())
            context = SimpleNamespace(prepared=self.prepared, record=record, helper=self.helper, backend=backend)
            with patch.object(product.os, "scandir", side_effect=AssertionError("foreign directory enumerated")):
                with self.assertRaisesRegex(product.ResourceError, "workspace_not_selected"):
                    product.retained_checkpoint(context, "work-" + "f" * 32)
            extras = [owner.workspace / "requests" / ("extra-" + str(number)) for number in range(256)]
            try:
                for path in extras:
                    path.write_bytes(b"")
                with patch.object(self.helper, "read_recovery_checkpoint", side_effect=AssertionError("unbounded closure entered")):
                    with self.assertRaisesRegex(product.ResourceError, "retained_request_limit"):
                        product.retained_checkpoint(context, owner.workspace.name)
            finally:
                for path in extras:
                    if path.exists():
                        path.unlink()

    def test_fresh_realm_discovery_reads_library_metadata_without_reinstall(self):
        self.full_product_readback("services", retained_return=True)

    def test_retained_discovery_pages_and_changed_selection_do_not_hide_another_candidate(self):
        self.full_product_readback("services", retained_return=True, retained_pages=True)

    def test_fresh_realm_discovery_reads_selected_python_secret_declaration(self):
        self.full_product_readback("selected-python", retained_return=True)

    def test_fresh_realm_discovery_reads_selected_node_software_secret_declaration(self):
        self.full_product_readback("selected-node", retained_return=True, local_software=True)

    def test_retained_selected_python_readback_keeps_the_acquisition_runtime_pair(self):
        self.full_product_readback("selected-python", retained_return=True, split_selected_runtime=True)

    def test_retained_selected_node_readback_keeps_the_acquisition_runtime_pair(self):
        self.full_product_readback("selected-node", retained_return=True, local_software=True,
                                   split_selected_runtime=True)

    def test_credential_selected_python_readback_keeps_the_acquisition_runtime_pair(self):
        self.full_product_readback("selected-python", credential_service=True, split_selected_runtime=True)

    def test_credential_selected_node_readback_keeps_the_acquisition_runtime_pair(self):
        self.full_product_readback("selected-node", local_software=True, credential_service=True,
                                   split_selected_runtime=True)

    def full_product_readback(self, family, account_profile=None, *, remote_declaration=False, local_software=False,
                              retained_return=False, retained_pages=False, credential_service=False,
                              split_selected_runtime=False):
        # Resource/archive verification is controlled here; the public readback
        # function, receipt/attempt joins, Controller and request decoder are real.
        product_root = self.root / "product"
        product_root.mkdir()
        (product_root / "starter.py").write_bytes(b"")
        archive = product_root / "fixture.tar.gz"
        archive.write_bytes(b"controlled selected archive")
        manifest = {"target": product.HOST_TARGETS[(product.platform.system(), product.platform.machine())],
                    "python": str(Path(sys.executable).resolve()), "node": str(Path(sys.executable).resolve()),
                    "npm_cli": str(Path(sys.executable).resolve()), "archive": archive.name, "starter": "starter.py"}
        consumers = self.release / "consumers"
        shutil.copytree(self.bundle, consumers)
        self.assertEqual({str(p.relative_to(consumers)): p.read_bytes()
                          for p in consumers.rglob("*") if p.is_file()},
                         {str(p.relative_to(self.bundle)): p.read_bytes()
                          for p in self.bundle.rglob("*") if p.is_file()})
        snapshot = {"hashes": {"consumers/CONTENTS.json": self.sha,
                               "consumers/install-library-consumer.py": self.manifest["files"]["install-library-consumer.py"]["sha256"]}}
        self.retained["setup/backend.py"] = (ROOT / "backend.py").read_bytes()
        selected_package = family.startswith("selected-")
        remote = family == "remote-recipe"
        selected_options = {}
        files = {"mcp.json": self.configuration_sha}
        if selected_package:
            self.staging = self.prepared / "staging"
            self.staging.mkdir()
            tools = {}
            for name in ("installer", "python", "node"):
                path = self.release / (name + ".tool")
                path.write_bytes(("inert selected " + name).encode())
                path.chmod(0o700)
                tools[name] = {"path": str(path), "sha256": local.digest(path.read_bytes())}
            profile_path = self.prepared / "config/selected-install.json"
            profile_raw = local.encoded({"schema_version": 1, "scope": backend.SELECTED_SCOPE,
                "staging_root": str(self.staging), "installer": tools["installer"],
                "validators": {name: tools[name] for name in ("python", "node")}})
            profile_path.write_bytes(profile_raw)
            profile_sha = local.digest(profile_raw)
            selected_options = {"selected_install_profile": profile_path,
                                "selected_install_profile_sha256": profile_sha}
            config = json.loads(self.configuration.read_bytes())
            config["mcpServers"]["toolkit-setup"]["args"] += ["--selected-install-profile", str(profile_path),
                "--selected-install-profile-sha256", profile_sha]
            if split_selected_runtime:
                # Model the supplied product's separate library and acquisition
                # runtimes. Only the controlled package runner executes here.
                manifest["npm_acquisition"] = {}
                for name in ("node", "npm_cli", "npm_archive"):
                    path = product_root / ("acquisition-" + name)
                    path.write_bytes(("inert acquisition " + name).encode())
                    path.chmod(0o700)
                    manifest["npm_acquisition"][name] = path.name
                for option, resource in (("selected_node", "node"), ("selected_npm_cli", "npm_cli")):
                    selected_options[option] = str(product_root / manifest["npm_acquisition"][resource])
                    config["mcpServers"]["toolkit-setup"]["args"] += [
                        "--" + option.replace("_", "-"), selected_options[option]]
            self.configuration.write_bytes(local.encoded(config))
            self.configuration_sha = local.digest(self.configuration.read_bytes())
            self.helper, self.binding, self.descriptor = self.project()
            files = {"mcp.json": self.configuration_sha, "selected-install.json": profile_sha}
        if remote:
            tool = self.release / "recipe.tool"
            tool.write_bytes(b"inert recipe owner transport; not executed")
            tool.chmod(0o700)
            database = self.realm_root / ".meerkat-connect/realm/recipes.sqlite"
            database.write_bytes(b"inert database transport; Rust owns real store verification")
            profile_path = self.prepared / "config/local-recipes.json"
            profile_raw = local.encoded({"schema_version":1,"scope":backend.LOCAL_RECIPES_RETAIN_SCOPE,
                "database":str(database),"realm":"realm","owner":"meerkat-connect",
                "tool":{"path":str(tool),"sha256":local.digest(tool.read_bytes())}})
            profile_path.write_bytes(profile_raw)
            profile_sha = local.digest(profile_raw)
            selected_options = {"local_recipes_profile":profile_path,
                                "local_recipes_profile_sha256":profile_sha,"allow_remote_recipes":True}
            config = json.loads(self.configuration.read_bytes())
            config["mcpServers"]["toolkit-setup"]["args"] += ["--local-recipes-profile",str(profile_path),
                "--local-recipes-profile-sha256",profile_sha,"--allow-remote-recipes"]
            self.configuration.write_bytes(local.encoded(config))
            self.configuration_sha = local.digest(self.configuration.read_bytes())
            self.helper,self.binding,self.descriptor = self.project()
            files = {"mcp.json":self.configuration_sha,"local-recipes.json":profile_sha}
        receipt_raw = local.encoded({"target": manifest["target"], "workspace": str(self.prepared / "session"),
            "release": {"archive_sha256": product.sha256(archive)}, "files": files})
        (self.prepared / "config/prepared.json").write_bytes(receipt_raw)
        descriptor_path = self.prepared / "product-mcp.json"
        descriptor_raw = local.encoded(self.descriptor)
        descriptor_path.write_bytes(descriptor_raw)
        (self.prepared / "product-prepared.json").write_bytes(local.encoded({
            "resources_sha256": "a" * 64, "mcp_sha256": self.configuration_sha,
            "preparation": {"bootstrap": {"host": {"receipt_sha256": local.digest(receipt_raw)}}},
            "local_setup": {"descriptor_sha256": local.digest(descriptor_raw),
                            "helper_sha256": self.binding["helper_sha256"]}}))
        with local.session_owner(self.state_root, self.binding) as retained:
            runner = (remote_exports.ExportCheckRunner if remote else
                      selected_setup.SelectedRunner if selected_package else reconciliation.ReconciliationRunner)
            if account_profile is not None:
                self.assertEqual((family, account_profile), ("agno", "salesforce"))

                class SalesforceRunner(reconciliation.ReconciliationRunner):
                    # A package-owner fixture for the real Controller join,
                    # not Salesforce credentials, wheel verification or calls.
                    def run(inner, argv, capture=False):
                        if "profiles" in argv or "config-help" in argv:
                            inner.calls.append((list(argv), capture))
                            row = {"profile": "salesforce", "operations": ["get_record"]}
                            if "profiles" in argv:
                                return 0, backend.encoded({"profiles": [row]})
                            row = {**fixtures.help_source("agno"), **row}
                            row["config_fields"] = [
                                {"name": "api_key_env", "kind": "string", "required": True, "credential_reference": True},
                                {"name": "timeout", "kind": "integer", "required": True},
                                {"name": "instance_url", "kind": "string", "required": True}]
                            return 0, backend.encoded(row)
                        return super().run(argv, capture=capture)

                runner = SalesforceRunner
            if remote:
                owner = local.controller_factory(backend,*retained)(
                    self.bundle,self.sha,self.root / "unused",(),
                    str(Path(sys.executable).resolve()),str(Path(sys.executable).resolve()),str(Path(sys.executable).resolve()),
                    runner_factory=runner,**selected_options)
                self.controllers.append(owner)
                owner.remember_remote(remote_exports.hosted.REQUEST)
                request,declaration = remote_exports.declared_request(remote_declaration=remote_declaration)
                owner.runner.declaration = declaration
                job = owner.export_recipe(request)["job"]; owner.join()
                validation = None
            elif selected_package:
                owner = local.controller_factory(backend, *retained)(
                    self.bundle, self.sha, self.root / "unused", (),
                    str(Path(sys.executable).resolve()), str(Path(sys.executable).resolve()), str(Path(sys.executable).resolve()),
                    runner_factory=runner, **selected_options)
                self.controllers.append(owner)
                request = selected_setup.SelectedFixture.stage(self, family.removeprefix("selected-"),
                    mutate=lambda plan: plan.update(declared_service={"authentication": {"kind": "none"},
                        "operation_tools": {"declared/read": "actual_read"}, **({
                            "authentication": {"kind": "binding", "owner": "process", "requirement": "api-key",
                                               "credential_audience": "service"},
                            "environment_secret_slots": ["API_KEY", "TOKEN"]} if retained_return or credential_service else {})}))
                if local_software:
                    request = selected_setup.software_form(self.staging, request)
                validation = owner.selected_check(request)
                job = owner.selected_install({key: validation[key] for key in ("validation_id", "effect_sha256")})["job"]
                owner.join()
            else:
                owner = self.create(*retained, runner=runner)
                owner.prepare({"family": family}); owner.join()
                request = ({"family": "services", "profile": "slack", "operations": ["auth_test"],
                            "values": {"token_env": "TEST_TOKEN", "timeout_seconds": 10}}
                           if family == "services" else self.request())
                if account_profile is not None:
                    request = {"family": "agno", "profile": account_profile, "operations": ["get_record"],
                        "values": {"api_key_env": "UNRESOLVED_FIXTURE_REFERENCE", "timeout": 3,
                                   "instance_url": "https://selected.my.salesforce.com"}}
                validation = owner.check(request)
                checked = {**request, "validation_id": validation["validation_id"],
                           "configuration_sha256": validation["configuration_sha256"]}
                job = owner.install(checked)["job"]; owner.join()
            self.assertEqual(owner.jobs[job["id"]]["status"], "succeeded")
            if retained_pages:
                for number in range(2):
                    additional = {**request, "values": {**request["values"], "token_env": "OTHER_TOKEN_" + str(number)}}
                    check = owner.check(additional)
                    owner.install({**additional, "validation_id": check["validation_id"],
                                   "configuration_sha256": check["configuration_sha256"]})
                    owner.join()
            if remote:
                exported = owner.jobs[job["id"]]["result"]
                selected = {"job_id":job["id"],"request_sha256":job["request_sha256"],
                    "recovery_request_sha256":job["recovery_request"]["sha256"],"export":exported}
            else:
                mcp = owner.workspace / "installations" / validation["validation_id"] / "ready/mcp.json"
                selected = {"family": family, "mcp_path": str(mcp), "receipt_path": str(mcp.with_name("installed.json")),
                            "validation_id": validation["validation_id"], "job_id": job["id"],
                            "recovery_request_sha256": job["recovery_request"]["sha256"]}
            if account_profile is not None:
                selected["profile"] = account_profile
            public = {}
            if (family != "services" or credential_service) and account_profile is None:
                basis = {"kind": "package_published_metadata"}
                if selected_package:
                    basis = {"kind": "selected_software_declarations" if local_software else "selected_manifest_declarations",
                        "authentication": {"kind": "none"}, "package_sha256": request["package_sha256"],
                        **{key: validation[key] for key in ("selection_sha256", "effect_sha256")},
                        **({"document_kind": "local_software_declaration", "declaration_digest": validation["declaration_digest"]}
                           if local_software else {"manifest_digest": validation["manifest_digest"]}),
                        "source": {"plugin_source": None, "acquired_source": None}}
                if remote:
                    basis = {"kind":"remote_recipe_declarations","recipe":exported["recipe"],
                        "selection_digest":exported["selection_digest"],
                        "declared_remote_service":exported["declared_remote_service"]}
                public = {"public_service": {"operation_basis": basis,
                    "operations": ["actual.lookup"] if remote else ["actual_read"]}}
                if credential_service:
                    if selected_package:
                        basis["authentication"] = {"kind": "binding", "owner": "process", "requirement": "api-key",
                                                   "credential_audience": "service"}
                        slots = [{"source": "selected_environment", "declaration": name} for name in ("API_KEY", "TOKEN")]
                    else:
                        # This request represents the protected review's checked
                        # names-only projection of the exact installed config.
                        slots = [{"source": "library_field", "declaration": {
                            "field_path": ["config", "token_env"], "environment_variable": "TEST_TOKEN"}}]
                    public = {"credential_service": {"operation_basis": basis,
                        "operations": ["actual_read"] if selected_package else ["slack_auth_test"], "credential_slots": slots}}
            source_key = "remote_export" if remote else "software"
            request_path = self.root / "readback.json"
            request_path.write_bytes(local.encoded({source_key: selected,
                "realm_state_root": str(self.realm_root), "realm": "realm", **public}))
            entered = []
            mutation = None

            class ObservedRunner(runner):
                def run(inner, argv, capture=False):
                    result = super().run(argv, capture=capture)
                    if retained_return and family == "services":
                        if "config-help" in argv:
                            value = backend.decode(result[1])
                            for row in value["operations"]:
                                row["tool"] = "slack_" + row["operation"]
                            result = (result[0], backend.encoded(value))
                        elif "check-config" in argv:
                            value = backend.decode(result[1])
                            configuration = backend.decode(Path(argv[argv.index("--config") + 1]).read_bytes())
                            value["credential_environment_references"] = [
                                {"field_path": ["config", "token_env"], "environment_variable": configuration["config"]["token_env"]}]
                            result = (result[0], backend.encoded(value))
                    if "--verify-existing" in argv or remote and argv[1] == "check-export":
                        entered.append(list(argv))
                        if mutation is not None:
                            mutation()
                    return result

            def create_readback(**kwargs):
                value = backend.Controller(**kwargs, runner_factory=ObservedRunner)
                if remote:
                    value.runner.declaration = json.loads(json.dumps(declaration))
                self.controllers.append(value)
                return value

            original_loader = host.owner_module

            def load_owner(name, path, raw):
                if name == "product_readback_backend":
                    return SimpleNamespace(Controller=create_readback,
                        read_recovery_selection=backend.read_recovery_selection,
                        retained_request_summary=backend.retained_request_summary,
                        selected_proposal=backend.selected_proposal,
                        selected_identity=backend.selected_identity,
                        LOCAL_RECIPES_RETAIN_SCOPE=backend.LOCAL_RECIPES_RETAIN_SCOPE,
                        REMOTE_EXPORT_READBACK=backend.REMOTE_EXPORT_READBACK)
                return original_loader(name, path, raw)

            def starter_module(name):
                module = ModuleType(name)
                module.host = host
                return module

            with patch.object(product, "verify_resources", return_value=(manifest, "a" * 64)), \
                 patch.object(product, "ModuleType", side_effect=starter_module), \
                 patch.object(host, "release_snapshot", return_value=(snapshot, self.retained)), \
                 patch.object(host, "recheck_release"), patch.object(host, "owner_module", side_effect=load_owner), \
                 patch.object(product, "RETAINED_PAGE", 1 if retained_pages else product.RETAINED_PAGE):
                if retained_return:
                    # The new product interface starts from realm inventory.
                    # No prior mcp path, job ID or digest is supplied to it.
                    owner.close_admission()
                    saved = {path: path.read_bytes() for path in (
                        owner.workspace / "requests" / (job["id"] + ".json"),
                        mcp, mcp.with_name("installed.json"), self.state_root / "recovery.json")}
                    original_calls = list(owner.runner.calls)
                    self.assertEqual(sum("--destination" in argv and "--verify-existing" not in argv
                        and (("--selected-" + family.removeprefix("selected-")) in argv if selected_package else "--config" in argv)
                        for argv, _ in original_calls), 3 if retained_pages else 1)
                    realm_parent = self.prepared.parent.parent
                    for directory in (realm_parent.parent, realm_parent, self.prepared.parent):
                        directory.chmod(0o700)
                    partial = realm_parent / "setup-a-partial"
                    (partial / "prepared").mkdir(parents=True)
                    partial.chmod(0o700)
                    (realm_parent / "setup-b-readback").mkdir(mode=0o700)
                    (realm_parent / "setup-c-symlink").symlink_to(self.prepared.parent, target_is_directory=True)
                    query = {"realm_state_root": str(self.realm_root), "realm": "realm",
                             "retained_installations": {"action": "list", "cursor": None}}
                    candidates, pages, omitted = [], 0, False
                    while True:
                        request_path.write_bytes(local.encoded(query))
                        page = product.installation_readback(product_root, self.root / ("unused-list-" + str(pages)),
                                                            request_path, threading.Event())
                        pages += 1
                        self.assertLess(pages, 8)
                        self.assertLessEqual(len(page["candidates"]), product.RETAINED_PAGE)
                        candidates.extend(page["candidates"])
                        omitted |= page["unconfirmed"]
                        if page["next_cursor"] is None:
                            break
                        query["retained_installations"]["cursor"] = page["next_cursor"]
                    self.assertTrue(omitted)
                    self.assertEqual(len(candidates), 3 if retained_pages else 1)
                    self.assertEqual(len({item["locator"]["job_id"] for item in candidates}), len(candidates))
                    candidate = next(item for item in candidates if item["locator"]["job_id"] == job["id"])
                    self.assertEqual(candidate["family"], family)
                    self.assertEqual(candidate["profile"], family.removeprefix("selected-") if selected_package else "slack")
                    self.assertEqual(entered, [], "listing never invokes the verifier or installed metadata adapter")
                    self.assertEqual(owner.runner.calls, original_calls)
                    locator = candidate["locator"]
                    self.assertEqual(locator["job_id"], job["id"])
                    self.assertNotIn(str(self.root), json.dumps(candidates))
                    chosen = {"realm_state_root": str(self.realm_root), "realm": "realm",
                              "retained_installations": {"action": "select", "locator": locator}}
                    request_path.write_bytes(local.encoded(chosen))
                    value = product.installation_readback(product_root, self.root / "retained-read", request_path, threading.Event())
                    self.assertEqual(value["kind"], "retained_installation_readback")
                    self.assertEqual(value["locator"], locator)
                    self.assertEqual(value["installation"]["job_id"], job["id"])
                    self.assertEqual(value["installation"]["recovery_request_sha256"], locator["request_sha256"])
                    self.assertEqual(value["installation"]["scope"], backend.READBACK_SCOPE)
                    recovered = self.controllers[-1]
                    self.assertTrue(recovered.recovery_only)
                    self.assertTrue(recovered.closing)
                    if split_selected_runtime:
                        self.assertEqual(recovered.selected_binding, owner.selected_binding)
                        self.assertNotEqual(recovered.selected_node, recovered.node)
                        self.assertNotEqual(recovered.selected_npm_cli, recovered.npm_cli)
                    self.assertEqual(recovered.jobs[job["id"]]["status"], "unconfirmed")
                    self.assertEqual(recovered.jobs[job["id"]]["error"], "historical_outcome_unknown")
                    self.assertEqual(sum("--verify-existing" in argv for argv, _ in recovered.runner.calls), 2)
                    self.assertTrue(all("--verify-existing" in argv or "config-help" in argv or "check-config" in argv
                                        for argv, _ in recovered.runner.calls))
                    if selected_package:
                        declaration = value["metadata"]["declaration"]
                        self.assertEqual(declaration["declared_service"]["environment_secret_slots"], ["API_KEY", "TOKEN"])
                        self.assertEqual(declaration["declared_service"]["authentication"]["kind"], "binding")
                        document = "declaration.json" if local_software else "activation.json"
                        self.assertEqual(set(declaration["files"]), {"package.json", document})
                        self.assertEqual(declaration["files"]["package.json"], request["package_sha256"])
                        self.assertEqual(len(recovered.runner.calls), 2, "selected metadata never starts selected software")
                    else:
                        metadata = value["metadata"]
                        self.assertEqual(metadata["selection"], {"family": "services", "profile": "slack",
                            "operations": ["auth_test"], "values": {"token_env": "TEST_TOKEN"}})
                        self.assertEqual(metadata["configuration_check"], {"credential_environment_references": [
                            {"field_path": ["config", "token_env"], "environment_variable": "TEST_TOKEN"}]})
                        self.assertTrue(all(row["tool"].startswith("slack_") for row in metadata["help"]["operations"]))
                        self.assertNotIn("timeout_seconds", metadata["selection"]["values"])
                        self.assertNotIn("CONFIGURATION_VALUE_MUST_NOT_ESCAPE", json.dumps(value))
                    self.assertEqual({path: path.read_bytes() for path in saved}, saved)
                    self.assertEqual(owner.runner.calls, original_calls)
                    if split_selected_runtime:
                        # A different supplied pair cannot replace the recorded
                        # selected host, even if resource verification succeeds.
                        original_pair = dict(manifest["npm_acquisition"])
                        original_reads = list(entered)
                        for resource, other in (("node", "npm_cli"), ("npm_cli", "node")):
                            with self.subTest(changed_selected_runtime=resource):
                                manifest["npm_acquisition"][resource] = original_pair[other]
                                destination = self.root / ("changed-selected-" + resource)
                                try:
                                    with self.assertRaisesRegex(backend.SetupError, "selected_proposal_changed"):
                                        product.installation_readback(product_root, destination,
                                                                      request_path, threading.Event())
                                finally:
                                    manifest["npm_acquisition"] = dict(original_pair)
                                self.assertFalse(destination.exists())
                                self.assertEqual(entered, original_reads)
                    # Refusal is specific to this selection, without a retained
                    # failure bit that poisons a later unchanged selection.
                    if retained_pages:
                        # One selected artifact can change without invalidating
                        # another unchanged checkpoint-selected request.
                        receipt_file = mcp.with_name("installed.json")
                        request_path.write_bytes(local.encoded(chosen))
                        receipt_file.write_bytes(b"{}")
                        with self.assertRaises(product.ResourceError):
                            product.installation_readback(product_root, self.root / "bad-artifact", request_path, threading.Event())
                        other = next(item for item in candidates if item["locator"]["job_id"] != job["id"])
                        other_query = {**chosen, "retained_installations": {"action": "select", "locator": other["locator"]}}
                        request_path.write_bytes(local.encoded(other_query))
                        other_value = product.installation_readback(product_root, self.root / "other-retained", request_path, threading.Event())
                        self.assertEqual(other_value["installation"]["job_id"], other["locator"]["job_id"])
                        receipt_file.write_bytes(saved[receipt_file])
                    changed = dict(chosen, realm="other")
                    request_path.write_bytes(local.encoded(changed))
                    with self.assertRaises(product.ResourceError):
                        product.installation_readback(product_root, self.root / "foreign-retained", request_path, threading.Event())
                    request_path.write_bytes(local.encoded(chosen))
                    request_file = owner.workspace / "requests" / (job["id"] + ".json")
                    request_file.write_bytes(saved[request_file] + b" ")
                    with self.assertRaises(Exception):
                        product.installation_readback(product_root, self.root / "changed-retained", request_path, threading.Event())
                    request_file.write_bytes(saved[request_file])
                    current = product.installation_readback(product_root, self.root / "retained-read-again", request_path, threading.Event())
                    self.assertEqual(current, value)
                    self.assertEqual({path: path.read_bytes() for path in saved}, saved)
                    return
                value = product.installation_readback(product_root, self.root / "readback-good", request_path, threading.Event())
                if remote:
                    self.assertEqual(value,exported)
                    self.assertEqual(value.get("selection_kind"), "remote_declaration" if remote_declaration else None)
                    self.assertNotIn("configuration", backend.decode(request[
                        "declaration_json" if remote_declaration else "activation_json"].encode()))
                    self.assertNotIn("software",json.loads(request_path.read_bytes()))
                    self.assertNotIn("receipt_path",selected)
                    self.assertEqual([argv[1] for argv in entered],["check-export"])
                    recovered = self.controllers[-1]
                    self.assertTrue(recovered.recovery_only)
                    self.assertEqual([argv[1] for argv,_ in recovered.runner.calls],["check-export"])
                    self.assertTrue(recovered.closing)
                elif selected_package:
                    self.assertEqual(value["selection_kind"], family.removeprefix("selected-"))
                    self.assertEqual(value["package_sha256"], request["package_sha256"])
                    if split_selected_runtime:
                        recovered = self.controllers[-1]
                        self.assertEqual(recovered.selected_binding, owner.selected_binding)
                        self.assertNotEqual(recovered.selected_node, recovered.node)
                        self.assertNotEqual(recovered.selected_npm_cli, recovered.npm_cli)
                    self.assertEqual(value["declared_service"]["authentication"],
                                     basis["authentication"] if credential_service else {"kind": "none"})
                    if local_software:
                        self.assertEqual(value["document_kind"], "local_software_declaration")
                        self.assertEqual(value["declaration_digest"], validation["declaration_digest"])
                        self.assertNotIn("manifest_digest", value)
                else:
                    self.assertEqual(value["family"], family)
                self.assertEqual(len(entered), 1)
                if credential_service:
                    saved_request = request_path.read_bytes()
                    saved_files = {path: path.read_bytes() for path in (
                        owner.workspace / "requests" / (job["id"] + ".json"), mcp, mcp.with_name("installed.json"))}
                    recovered = self.controllers[-1]
                    self.assertTrue(recovered.recovery_only)
                    self.assertTrue(recovered.closing)
                    self.assertEqual(len(recovered.runner.calls), 1)
                    self.assertIn("--verify-existing", recovered.runner.calls[0][0])
                    malformed = []
                    for field, replacement in (("credential_slots", []), ("credential_slots", slots + slots),
                            ("credential_slots", [{"source": "other", "declaration": "API_KEY"}]),
                            ("operations", ["actual_read", "actual_read"]), ("operations", ["bad operation"]),
                            ("operation_basis", {"kind": "unselected"})):
                        item = json.loads(saved_request)
                        item["credential_service"][field] = replacement
                        malformed.append(item)
                    item = json.loads(saved_request)
                    item["credential_service"]["credential_slots"][0]["secret_value"] = "MUST_NOT_BE_ACCEPTED"
                    malformed.append(item)
                    item = json.loads(saved_request)
                    item["public_service"] = {"operation_basis": {"kind": "package_published_metadata"}, "operations": ["actual_read"]}
                    malformed.append(item)
                    item = json.loads(saved_request)
                    slot = item["credential_service"]["credential_slots"][0]
                    if selected_package:
                        slot["declaration"] = "not an environment name"
                    else:
                        slot["declaration"]["field_path"] = ["config", None]
                    malformed.append(item)
                    for number, item in enumerate(malformed):
                        request_path.write_bytes(local.encoded(item))
                        with self.assertRaises(product.ResourceError):
                            product.installation_readback(product_root, self.root / ("credential-invalid-" + str(number)),
                                                          request_path, threading.Event())
                    self.assertEqual(len(entered), 1, "malformed credential evidence never enters the verifier")
                    if selected_package:
                        for number, (field, replacement) in enumerate((
                                ("credential_slots", slots[:1]),
                                ("credential_slots", slots + [{"source": "selected_environment", "declaration": "OTHER"}]),
                                ("operations", ["unselected_operation"]),
                                ("operation_basis", dict(basis, authentication=dict(basis["authentication"], owner="other"))))):
                            item = json.loads(saved_request)
                            item["credential_service"][field] = replacement
                            request_path.write_bytes(local.encoded(item))
                            with self.assertRaisesRegex(product.ResourceError, "selected_declaration_changed"):
                                product.installation_readback(product_root, self.root / ("credential-changed-" + str(number)),
                                                              request_path, threading.Event())
                        self.assertEqual(len(entered), 5, "complete declaration comparisons follow actual canonical verification")
                    self.assertEqual({path: path.read_bytes() for path in saved_files}, saved_files)
                    self.assertTrue(all(len(controller.runner.calls) == 1 and "--verify-existing" in controller.runner.calls[0][0]
                                        for controller in self.controllers if controller is not owner))
                    request_path.write_bytes(saved_request)
                    entered.clear()
                    # Continue the existing realm/current-request/refusal checks.
                    product.installation_readback(product_root, self.root / "credential-current-again", request_path, threading.Event())
                    self.assertEqual(len(entered), 1)
                if account_profile is not None:
                    self.assertEqual(value["profile"], "salesforce")
                    self.assertNotIn("public_service", json.loads(request_path.read_bytes()))
                    saved_request = request_path.read_bytes()
                    for number, profile in enumerate((None, "tavily", "hackernews", ["salesforce"])):
                        changed = json.loads(saved_request)
                        if profile is None:
                            del changed["software"]["profile"]
                        else:
                            changed["software"]["profile"] = profile
                        request_path.write_bytes(local.encoded(changed))
                        refused_destination = self.root / ("readback-unsupported-profile-" + str(number))
                        with self.assertRaisesRegex(product.ResourceError, "unsupported_readback_owner"):
                            product.installation_readback(product_root, refused_destination, request_path, threading.Event())
                        self.assertFalse(refused_destination.exists())
                    request_path.write_bytes(saved_request)
                    self.assertEqual(len(entered), 1, "unsupported profiles refuse before a verifier is entered")
                for number, changed in enumerate((descriptor_path, self.state_root / "session.json")):
                    saved = changed.read_bytes()
                    mutation = lambda: changed.write_bytes(saved + b"\n")
                    with self.assertRaisesRegex(product.ResourceError, "local_attempt_changed"):
                        product.installation_readback(product_root, self.root / ("readback-changed-" + str(number)),
                                                      request_path, threading.Event())
                    changed.write_bytes(saved)
                self.assertEqual(len(entered), 3, "each refusal follows the actual canonical readback")
                mutation = None
                if selected_package or remote:
                    saved = profile_path.read_bytes()
                    profile_path.write_bytes(saved + b"\n")
                    with self.assertRaisesRegex(product.ResourceError, "preparation_receipt_changed"):
                        product.installation_readback(product_root, self.root / "readback-changed-profile", request_path, threading.Event())
                    profile_path.write_bytes(saved)
                    self.assertEqual(len(entered), 3, "changed owner profile refuses before any verifier")
                    changed_public = json.loads(json.dumps(public))
                    if remote:
                        changed_public["public_service"]["operation_basis"]["selection_digest"] = "f" * 64
                    else:
                        changed_public["credential_service" if credential_service else "public_service"]["operation_basis"]["package_sha256"] = "f" * 64
                    request_path.write_bytes(local.encoded({source_key: selected,
                        "realm_state_root": str(self.realm_root), "realm": "realm", **changed_public}))
                    with self.assertRaisesRegex(product.ResourceError, "remote_export_changed" if remote else "selected_declaration_changed"):
                        product.installation_readback(product_root, self.root / "readback-changed-declaration", request_path, threading.Event())
                    self.assertEqual(len(entered), 4, "declaration comparison retains actual canonical verification")
                request_path.write_bytes(local.encoded({source_key: selected,
                    "realm_state_root": str(self.realm_root), "realm": "other", **public}))
                with self.assertRaisesRegex(product.ResourceError, "readback_realm_mismatch"):
                    product.installation_readback(product_root, self.root / "readback-wrong-realm", request_path, threading.Event())
                self.assertEqual(len(entered), 4 if selected_package or remote else 3)
                if remote:
                    request_path.write_bytes(local.encoded({source_key:selected,
                        "realm_state_root":str(self.realm_root),"realm":"realm",**public}))
                    stopped = threading.Event(); stopped.set()
                    with self.assertRaisesRegex(product.ResourceError,"readback_cancelled"):
                        product.installation_readback(product_root,self.root / "readback-cancelled",request_path,stopped)
                    self.assertEqual(len(entered),4)
                    self.assertEqual(sum(argv[1] == "export" for argv,_ in owner.runner.calls),1,
                        "only the originally admitted export ran; all later reads use check-export")


if __name__ == "__main__":
    unittest.main()
