"""Host request persistence and export observations with an inert owner fixture."""
import copy
from pathlib import Path
import threading
import unittest
from unittest.mock import patch

import test_hosted_recipes as hosted

backend = hosted.backend


# Inert owner-response data. The canonical CLI tests cover complete manifest
# and recipe validation; these controls cover the real Controller custody path.
DECLARATION = {"manifest_digest": "b" * 64, "component": "remote", "export": "read-service",
               "mcp_name": "fixture", "authentication": {"kind": "none"},
               "operation_tools": {"read-item": "actual.lookup"}}


def declared_request(authentication=None, *, remote_declaration=False):
    service = copy.deepcopy(DECLARATION)
    if authentication is not None:
        service["authentication"] = copy.deepcopy(authentication)
    activation = {"components": [{"id": service["component"]}],
        "exports": [{"id": service["export"], "authentication": service["authentication"]}],
        "portable_mcp_entries": [{"name": service["mcp_name"], "component": service["component"],
                                  "operation_tools": service["operation_tools"]}]}
    if remote_declaration:
        activation["selection_kind"] = "remote_declaration"
        service["selection_kind"] = "remote_declaration"
        service["declaration_digest"] = service.pop("manifest_digest")
        return {**hosted.fixtures.REQUEST, "declaration_json": backend.encoded(activation).decode()}, service
    return {**hosted.EXPORT, "activation_json": backend.encoded(activation).decode()}, service


class ExportCheckRunner(hosted.HostedRunner):
    def owner_output(self, raw):
        if not hasattr(self, "declaration"):
            return raw
        value = backend.decode(raw)
        value["declared_remote_service"] = copy.deepcopy(self.declaration)
        if hasattr(self, "output_kind"):
            if self.output_kind is None:
                value.pop("selection_kind", None)
            else:
                value["selection_kind"] = self.output_kind
        return backend.encoded(value)

    def run(self, argv, *, capture=False):
        if argv[1] != "check-export":
            code, raw = super().run(argv, capture=capture)
            return code, self.owner_output(raw) if argv[1] == "export" and code == 0 and raw else raw
        self.calls.append((list(argv), capture))
        destination = Path(argv[argv.index("--destination") + 1])
        files = {}
        for name, field in (("plugin.json", "plugin_json"), ("mcp.json", "mcp_json")):
            data = hosted.REQUEST[field].encode()
            expected = {"sha256": backend.digest(data), "bytes": len(data)}
            actual = (destination / name).read_bytes() if (destination / name).is_file() else None
            observed = None if actual is None else {"sha256": backend.digest(actual), "bytes": len(actual)}
            state = "missing" if actual is None else "matches" if expected == observed else "mismatch"
            files[name] = {"state": state, "expected": expected, "observed": observed}
        state = "matches" if all(v["state"] == "matches" for v in files.values()) else "mismatch" if any(v["state"] == "mismatch" for v in files.values()) else "missing"
        summary = hosted.fixtures.record()
        summary["origin"]["decision"] = copy.deepcopy(hosted.REQUEST["decision"])
        value = hosted.fixtures.envelope("inspected", recipe=summary,
            selection_digest="b" * 64, destination=str(destination), export_check={"state": state,
                "files": files, "inventory": {"state": "matches" if destination.exists() else "missing",
                "complete": True, "unexpected_entries": 0}})
        if "--declaration" in argv:
            value["selection_kind"] = "remote_declaration"
        return 0, self.owner_output(backend.encoded(value))


class ExportRecoveryTests(hosted.HostedFixture, unittest.TestCase):
    def controller(self, **changes):
        return super().controller(**{"runner_factory": ExportCheckRunner, **changes})

    def exported(self):
        owner = self.retained()
        job = owner.export_recipe(hosted.EXPORT)["job"]
        owner.join()
        return owner, owner.job_detail({"job_id": job["id"]})["job"]

    def recover(self, owner, job):
        return self.controller(allow_remote_recipes=False, recover_workspace=owner.workspace,
            recover_request=job["recovery_request"]["id"],
            recover_request_sha256=job["recovery_request"]["sha256"])

    def declared_export(self, authentication=None, omitted=False, *, remote_declaration=False):
        owner = self.retained()
        request, declaration = declared_request(authentication, remote_declaration=remote_declaration)
        if not omitted:
            owner.runner.declaration = declaration
        job = owner.export_recipe(request)["job"]
        owner.join()
        return owner, owner.job_detail({"job_id": job["id"]})["job"], request, declaration

    def test_exact_declaration_survives_export_and_fresh_recovery_without_export_replay(self):
        for remote_declaration in (False, True):
            with self.subTest(remote_declaration=remote_declaration):
                owner, job, request, declaration = self.declared_export(remote_declaration=remote_declaration)
                self.assertEqual(job["status"], "succeeded")
                self.assertEqual(job["result"]["declared_remote_service"], declaration)
                self.assertNotIn("installed", job["result"])
                self.assertEqual(job["result"]["scope"], "portable_mcp_configuration_only")
                calls = list(owner.runner.calls)
                self.assertEqual(owner.export_recipe(request)["job"], job)
                self.assertEqual(owner.runner.calls, calls)
                recovered = self.recover(owner, job)
                recovered.runner.declaration = copy.deepcopy(declaration)
                result = recovered.reconcile({"job_id": job["id"]})["observation"]
                self.assertEqual(result["status"], "matched")
                self.assertEqual(result["result"]["declared_remote_service"], declaration)
                self.assertEqual(result["result"]["configuration"], job["result"])
                self.assertEqual([args[1] for args, _ in recovered.runner.calls], ["check-export"])
                self.assertEqual(recovered.job["status"], "unconfirmed")
                result["result"]["configuration"]["declared_remote_service"]["operation_tools"]["read-item"] = "mutated"
                self.assertEqual(recovered.observations[job["id"]]["result"]["configuration"]["declared_remote_service"], declaration)
                self.assertEqual(owner.jobs[job["id"]]["result"]["declared_remote_service"], declaration)

                expected_flag = "--declaration" if remote_declaration else "--selection"
                document_key = "declaration_json" if remote_declaration else "activation_json"
                for args, _ in [*owner.runner.calls, *recovered.runner.calls]:
                    if args[1] in ("export", "check-export"):
                        self.assertIn(expected_flag, args)
                        document = Path(args[args.index(expected_flag) + 1])
                        self.assertEqual(document.name, "declaration.json" if remote_declaration else "activation.json")
                        self.assertEqual(document.read_bytes(), request[document_key].encode())
                self.assertEqual(job["result"].get("selection_kind"), "remote_declaration" if remote_declaration else None)
                self.assert_refused(lambda: recovered.export_recipe(request), "recovery_read_only")

    def test_explicit_forms_refuse_both_fields_and_kind_or_digest_substitution(self):
        request, _ = declared_request(remote_declaration=True)
        owner = self.retained()
        calls = list(owner.runner.calls)
        self.assert_refused(lambda: owner.export_recipe({**request, "activation_json": "{}"}), "invalid_request")
        self.assertEqual(owner.runner.calls, calls)
        self.assertEqual(owner.jobs, {})
        for form in (False, True):
            for mode in ("missing-kind", "wrong-kind", "inner-kind", "swapped-digest", "both-digests"):
                with self.subTest(form=form, mode=mode):
                    owner, job, request, declaration = self.declared_export(remote_declaration=form)
                    changed = copy.deepcopy(declaration)
                    recovered = self.recover(owner, job)
                    if mode == "missing-kind":
                        recovered.runner.output_kind = None if form else "remote_declaration"
                    elif mode == "wrong-kind":
                        recovered.runner.output_kind = "activation"
                    elif mode == "inner-kind":
                        changed["selection_kind"] = "activation"
                    elif mode == "swapped-digest":
                        original, other = (("declaration_digest", "manifest_digest") if form else
                                           ("manifest_digest", "declaration_digest"))
                        changed[other] = changed.pop(original)
                    else:
                        changed["manifest_digest" if form else "declaration_digest"] = "b" * 64
                    recovered.runner.declaration = changed
                    observed = recovered.reconcile({"job_id": job["id"]})["observation"]
                    self.assertEqual(observed["status"], "unconfirmed")
                    self.assertNotIn("result", observed)
                    self.assertEqual(owner.jobs[job["id"]], job)
                    self.assertEqual([args[1] for args, _ in recovered.runner.calls], ["check-export"])

    def test_binding_declaration_is_retained_without_becoming_no_sign_in_evidence(self):
        authentication = {"kind": "binding", "owner": "existing-auth", "requirement": "account",
                          "credential_audience": "fixture-provider"}
        owner, job, _, declaration = self.declared_export(authentication)
        self.assertEqual(job["status"], "succeeded")
        result = owner.reconcile({"job_id": job["id"]})["observation"]
        self.assertEqual(result["result"]["configuration"]["declared_remote_service"], declaration)
        self.assertEqual(declaration["authentication"], authentication)

    def test_legacy_owner_omission_is_only_configuration_evidence(self):
        owner, job, _, declaration = self.declared_export(omitted=True)
        self.assertEqual(job["status"], "succeeded")
        self.assertNotIn("declared_remote_service", job["result"])
        recovered = self.recover(owner, job)
        result = recovered.reconcile({"job_id": job["id"]})["observation"]
        self.assertEqual(result["status"], "matched")
        self.assertNotIn("declared_remote_service", result["result"])
        self.assertNotIn("declared_remote_service", result["result"]["configuration"])
        self.assertEqual([args[1] for args, _ in recovered.runner.calls], ["check-export"])
        # Disappearance is never filled from the earlier result. A consuming
        # reviewed selection must compare its exact retained declaration.
        owner, job, _, declaration = self.declared_export()
        recovered = self.recover(owner, job)
        result = recovered.reconcile({"job_id": job["id"]})["observation"]
        self.assertNotIn("declared_remote_service", result["result"]["configuration"])
        self.assertEqual(owner.jobs[job["id"]]["result"]["declared_remote_service"], declaration)

    def test_malformed_or_substituted_owner_declarations_refuse_export_and_recovery(self):
        for mode in ("null", "extra", "authentication", "manifest", "component", "export", "server",
                     "missing-map", "empty-map", "wrong-operation", "changed-raw", "duplicate-raw", "invalid-raw"):
            with self.subTest(mode=mode):
                owner, job, request, declaration = self.declared_export()
                changed = copy.deepcopy(declaration)
                if mode == "null": changed = None
                elif mode == "extra": changed["ready"] = True
                elif mode == "authentication": changed["authentication"] = {"kind": "binding",
                    "owner": "changed", "requirement": "account", "credential_audience": "fixture"}
                elif mode == "manifest": changed["manifest_digest"] = "c" * 64
                elif mode in ("component", "export"): changed[mode] = "changed"
                elif mode == "server": changed["mcp_name"] = "changed"
                elif mode == "missing-map": del changed["operation_tools"]
                elif mode == "empty-map": changed["operation_tools"] = {}
                elif mode == "wrong-operation": changed["operation_tools"] = {"other": "actual.lookup"}
                elif mode == "changed-raw": changed["operation_tools"]["read-item"] = "substituted"
                elif mode == "duplicate-raw": changed["operation_tools"]["other"] = "actual.lookup"
                elif mode == "invalid-raw": changed["operation_tools"]["read-item"] = "not a raw name"
                recovered = self.recover(owner, job)
                recovered.runner.declaration = changed
                result = recovered.reconcile({"job_id": job["id"]})["observation"]
                self.assertEqual(result["status"], "unconfirmed")
                self.assertNotIn("result", result)
                self.assertEqual([args[1] for args, _ in recovered.runner.calls], ["check-export"])
                self.assertEqual(owner.jobs[job["id"]], job)
                failed = self.retained()
                failed.runner.declaration = changed
                rejected = failed.export_recipe(request)["job"]
                failed.join()
                rejected = failed.job_detail({"job_id": rejected["id"]})["job"]
                self.assertEqual(rejected["status"], "unconfirmed")
                self.assertNotIn("result", rejected)
                calls = list(failed.runner.calls)
                self.assertEqual(failed.export_recipe(request)["job"], rejected)
                self.assertEqual(failed.runner.calls, calls)

    def test_restart_reads_same_files_and_keeps_historical_outcome_unknown(self):
        owner, job = self.exported()
        files = {p: p.read_bytes() for p in owner.workspace.rglob("*") if p.is_file()}
        recovered = self.recover(owner, job)
        self.assertEqual(recovered.state()["mode"], "recovery")
        self.assertEqual(recovered.job["status"], "unconfirmed")
        self.assert_refused(lambda: recovered.export_recipe(hosted.EXPORT), "recovery_read_only")
        check = recovered.reconcile({"job_id": job["id"]})["observation"]
        self.assertEqual(check["status"], "matched")
        self.assertEqual(check["result"]["configuration"], job["result"])
        self.assertEqual(recovered.job["status"], "unconfirmed")
        self.assertEqual([args[1] for args, _ in recovered.runner.calls], ["check-export"])
        self.assertTrue(all(p.read_bytes() == data for p, data in files.items()))

    def test_changed_file_never_returns_stale_configuration(self):
        owner, job = self.exported()
        before = copy.deepcopy(job)
        Path(job["result"]["mcp_path"]).write_text("changed")
        check = owner.reconcile({"job_id": job["id"]})["observation"]
        self.assertEqual(check["status"], "unconfirmed")
        self.assertEqual(check["result"]["export_check"]["state"], "mismatch")
        self.assertNotIn("configuration", check["result"])
        self.assertEqual(owner.jobs[job["id"]], before)

    def test_thread_start_failure_can_be_inspected_without_export(self):
        owner = self.retained()
        with patch.object(threading.Thread, "start", side_effect=RuntimeError("inert refusal")):
            job = owner.export_recipe(hosted.EXPORT)["job"]
        self.assertEqual(job["status"], "unconfirmed")
        recovered = self.recover(owner, job)
        check = recovered.reconcile({"job_id": job["id"]})["observation"]
        self.assertEqual(check["result"]["export_check"]["state"], "missing")
        self.assertEqual([args[1] for args, _ in owner.runner.calls], ["save", "read"])
        self.assertEqual(owner.export_recipe(hosted.EXPORT)["job"], job)

    def test_request_sync_failure_dispatches_no_export_and_releases_operation(self):
        owner = self.retained()
        with patch.object(backend, "sync_file", side_effect=OSError("inert sync refusal")):
            with self.assertRaises(OSError):
                owner.export_recipe(hosted.EXPORT)
        self.assertEqual(owner.recipe_exports, {})
        self.assertEqual(owner.jobs, {})
        self.assertFalse(owner.operation_lock.locked())
        self.assertEqual([args[1] for args, _ in owner.runner.calls], ["save", "read"])

    def test_request_anchor_failure_blocks_export_and_remember_but_retains_files(self):
        for stage in ("sync", "readback"):
            with self.subTest(stage=stage):
                owner = self.retained()
                error = OSError("private request failure")
                real_sync, real_load = backend.sync_file, backend.load_export_recovery

                def sync(root, name):
                    if root == owner.workspace / "requests" and stage == "sync":
                        raise error
                    return real_sync(root, name)

                def load(*args, **kwargs):
                    if stage == "readback":
                        raise error
                    return real_load(*args, **kwargs)

                calls = list(owner.runner.calls)
                with patch.object(backend, "sync_file", sync), patch.object(backend, "load_export_recovery", load):
                    with self.assertRaises(OSError) as caught:
                        owner.export_recipe(hosted.EXPORT)
                self.assertIs(caught.exception, error)
                retained = {p: p.read_bytes() for p in owner.workspace.rglob("*") if p.is_file()}
                self.assertEqual(len(list((owner.workspace / "requests").iterdir())), 1)
                for call in (lambda: owner.export_recipe(hosted.EXPORT),
                             lambda: owner.remember_remote(hosted.REQUEST)):
                    self.assert_refused(call, "recovery_request_unavailable")
                self.assertEqual(owner.runner.calls, calls)
                self.assertEqual({p: p.read_bytes() for p in retained}, retained)
                self.assertFalse(owner.recipe_exports)
                self.assertFalse(owner.jobs)
                self.assertFalse(owner.operation_lock.locked())
                self.assertFalse(owner.closing)

    def test_request_and_profile_tampering_refuse_before_owner_entry(self):
        owner, job = self.exported()
        for target in (owner.workspace / "requests" / (job["id"] + ".json"),
                       owner.workspace / "local-recipes-profile.json"):
            with self.subTest(target=target.name):
                original = target.read_bytes()
                before = len(owner.runner.calls)
                target.write_text("{}")
                check = owner.reconcile({"job_id": job["id"]})["observation"]
                self.assertEqual(check["status"], "unconfirmed")
                self.assertEqual(len(owner.runner.calls), before)
                target.write_bytes(original)


if __name__ == "__main__":
    unittest.main()
