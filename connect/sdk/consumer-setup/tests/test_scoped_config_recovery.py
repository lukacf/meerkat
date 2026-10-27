"""Supported string-list settings survive exact, read-only setup recovery."""

import copy
from pathlib import Path
import unittest

from test_reconciliation import ReconciliationMixin, ReconciliationRunner, backend, fixtures
from test_recovery import inventory


class ScopedRunner(ReconciliationRunner):
    """Only configuration metadata is extended; use the existing owner fixture."""

    def run(self, argv, capture=False):
        if any("services" in arg for arg in argv):
            if "profiles" in argv:
                self.calls.append((list(argv), capture))
                return 0, backend.encoded({"profiles": [
                    {"profile": profile, "operations": [{"operation": operation,
                     "tool": profile + "_" + operation, "effect": effect}]}
                    for profile, operation, effect in (("slack", "auth_test", "read"),
                                                       ("gmail", "send_message", "write"))]})
            if "config-help" in argv:
                self.calls.append((list(argv), capture))
                profile = argv[argv.index("--profile") + 1]
                result = fixtures.help_source("services")
                result["profile"] = profile
                operation = "auth_test" if profile == "slack" else "send_message"
                result["operations"] = [{"operation": operation, "tool": profile + "_" + operation,
                                         "effect": "read" if profile == "slack" else "write"}]
                result["config_fields"] = {
                    "token_env": {"kind": "string", "credential_reference": True},
                    "timeout_seconds": {"kind": "integer"},
                    "allowed_channel_ids" if profile == "slack" else "allowed_recipient_addresses":
                        {"kind": "string_list", "required": False}}
                return 0, backend.encoded(result)
        return super().run(argv, capture=capture)


class ScopedRecoveryTests(ReconciliationMixin, unittest.TestCase):
    def controller(self, **changes):
        changes.setdefault("runner_factory", ScopedRunner)
        return super().controller(**changes)

    def scoped_install(self, profile="slack", values=None):
        original = self.prepared("services")
        request = {"family": "services", "profile": profile,
                   "operations": ["auth_test" if profile == "slack" else "send_message"],
                   "values": {"token_env": "FIXTURE_TOKEN", "timeout_seconds": 10, **(values or {})}}
        checked = original.check(request)
        install = {**request, "validation_id": checked["validation_id"],
                   "configuration_sha256": checked["configuration_sha256"]}
        job = original.install(install)["job"]
        original.join()
        self.assertEqual(original.jobs[job["id"]]["status"], "succeeded")
        return original, install, job["id"]

    def recover(self, original, identifier, **changes):
        args = {"recover_workspace": original.workspace, "recover_request": identifier,
                "recover_request_sha256": original.jobs[identifier]["recovery_request"]["sha256"]}
        args.update(changes)
        return self.controller(**args)

    def test_omitted_empty_and_populated_lists_recover_without_repeating_installation(self):
        for profile, key, items in (("slack", "allowed_channel_ids", ["C1", "C2"]),
                                    ("gmail", "allowed_recipient_addresses", ["Reader@example.invalid"])):
            hashes = set()
            for values in ({}, {key: []}, {key: items}):
                with self.subTest(profile=profile, values=values):
                    original, install, identifier = self.scoped_install(profile, values)
                    hashes.add(install["configuration_sha256"])
                    before = inventory(original.workspace)
                    recovered = self.recover(original, identifier)
                    self.assertEqual(recovered.runner.calls, [])
                    self.assertEqual(recovered.state()["mode"], "recovery")
                    self.assertEqual(recovered.job["status"], "unconfirmed")
                    self.assertEqual(recovered.job["error"], "historical_outcome_unknown")
                    result = recovered.reconcile({"job_id": identifier})
                    self.assertEqual(result["observation"]["status"], "matched")
                    self.assertEqual(recovered.job["status"], "unconfirmed")
                    self.assertEqual(before, inventory(original.workspace))
                    self.assertEqual(len(recovered.runner.calls), 1)
                    self.assertIn("--verify-existing", recovered.runner.calls[0][0])
            self.assertEqual(len(hashes), 3)

    def test_coherent_malformed_lists_refuse_before_workspace_or_runner(self):
        original, _, identifier = self.scoped_install(values={"allowed_channel_ids": []})
        path = original.workspace / "requests" / (identifier + ".json")
        raw = path.read_bytes(); record = backend.decode(raw)
        config = original.workspace / "configurations" / (record["validation_id"] + ".json")
        config_raw = config.read_bytes()
        invalid = [[["C1"]], [1], [True], [None], [{}], ["C1"] * 257, ["C" * 2049],
                   ["C1\n"], ["C\x00"], ["C\x7f"]]
        try:
            for index, value in enumerate(invalid):
                changed = copy.deepcopy(record)
                changed["selection"]["values"]["allowed_channel_ids"] = value
                changed_config = backend.configuration_bytes(changed["selection"])
                changed["configuration_sha256"] = backend.digest(changed_config)
                data = backend.encoded(changed)
                path.write_bytes(data); config.write_bytes(changed_config)
                target = self.root / ("invalid-list-" + str(index))
                def forbidden_runner(_):
                    self.fail("Recovery constructed a runner for an invalid list")
                with self.subTest(value=repr(value)[:60]), self.assertRaisesRegex(backend.SetupError, "recovery_selection_mismatch"):
                    self.recover(original, identifier, workspace=target,
                                 recover_request_sha256=backend.digest(data), runner_factory=forbidden_runner)
                self.assertFalse(target.exists())
        finally:
            path.write_bytes(raw); config.write_bytes(config_raw)

    def test_list_support_does_not_weaken_original_bindings(self):
        original, _, identifier = self.scoped_install("gmail", {"allowed_recipient_addresses": []})
        path = original.workspace / "requests" / (identifier + ".json")
        raw = path.read_bytes(); record = backend.decode(raw)
        changes = [lambda r: r.update(contents_sha256="0" * 64),
                   lambda r: r.update(installer_sha256="0" * 64),
                   lambda r: r["runtimes"].update(python="/different/python"),
                   lambda r: r["selection"]["values"].update(allowed_recipient_addresses=["other@example.invalid"])]
        try:
            for index, change in enumerate(changes):
                changed = copy.deepcopy(record); change(changed)
                data = backend.encoded(changed); path.write_bytes(data)
                target = self.root / ("binding-" + str(index))
                with self.subTest(index=index), self.assertRaises(backend.SetupError):
                    self.recover(original, identifier, workspace=target, recover_request_sha256=backend.digest(data))
                self.assertFalse(target.exists())
            path.write_bytes(raw)
            with self.assertRaisesRegex(backend.SetupError, "recovery_digest_mismatch"):
                self.recover(original, identifier, recover_request_sha256="0" * 64)
        finally:
            path.write_bytes(raw)

    def test_mutation_after_recovery_refuses_before_inspection(self):
        for which in ("request", "configuration"):
            with self.subTest(which=which):
                original, install, identifier = self.scoped_install(values={"allowed_channel_ids": ["C1"]})
                recovered = self.recover(original, identifier)
                path = (original.workspace / "requests" / (identifier + ".json") if which == "request" else
                        original.workspace / "configurations" / (install["validation_id"] + ".json"))
                path.write_bytes(path.read_bytes() + b" ")
                self.assertEqual(recovered.reconcile({"job_id": identifier})["observation"]["status"], "unconfirmed")
                self.assertEqual(recovered.runner.calls, [])

    def test_recovered_list_configuration_never_authorizes_setup_or_replay(self):
        original, install, identifier = self.scoped_install("gmail", {"allowed_recipient_addresses": []})
        recovered = self.recover(original, identifier)
        for path, arguments in (("/api/prepare", {"family": "services"}), ("/api/install", install),
                                ("/api/check", {k: install[k] for k in ("family", "profile", "operations", "values")}),
                                ("/api/retry_prepare", {"job_id": identifier}), ("/api/stop", {"job_id": identifier})):
            with self.subTest(path=path), self.assertRaisesRegex(backend.SetupError, "recovery_read_only"):
                recovered.dispatch(path, arguments)
        self.assertEqual(recovered.runner.calls, [])
        self.assertFalse(recovered.validations)


if __name__ == "__main__":
    unittest.main()
