"""Exact owner-result contract, with fixture installation and no native process.

Run with --closure pointing to the reviewed OWNER-CLOSURE.json. The selected
verifier reads the real pinned wheel; the selected backend owns projection.
The existing fixture replaces installation and verifier process execution.
This does not prove installed payloads, product launch, auth or readiness.
"""
from __future__ import annotations

import argparse
import copy
import hashlib
import importlib.util
import json
from pathlib import Path
import sys
import unittest


def load(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def contract_suite(closure_path):
    closure_path = closure_path.resolve(strict=True)
    closure = json.loads(closure_path.read_bytes())
    paths = {}
    for name, record in closure["inputs"].items():
        path = closure_path.parent / record["path"]
        raw = path.read_bytes()
        if len(raw) != record["bytes"] or hashlib.sha256(raw).hexdigest() != record["sha256"]:
            raise ValueError("owner contract input changed: " + name)
        paths[name] = path
    # The retained fixture loads backend.py relative to itself. Confirm that
    # exact topology before import so a different checkout cannot be selected.
    if paths["backend"] != paths["backend_fixture"].parents[1] / "backend.py":
        raise ValueError("backend fixture does not select the pinned owner")
    if paths["backend_fixture"] != paths["reconciliation_fixture"].with_name("test_backend.py"):
        raise ValueError("reconciliation fixture does not select the pinned fixture")
    verifier = load("contract_verifier", paths["verifier"])
    reconciliation = load("contract_reconciliation", paths["reconciliation_fixture"])
    backend = reconciliation.backend
    if Path(backend.__file__) != paths["backend"]:
        raise ValueError("fixture loaded a different backend")
    wheel_name = paths["wheel"].name

    class PackageOwnerResultContract(reconciliation.ReconciliationMixin, unittest.TestCase):
        def installed_services(self, wheel):
            controller = self.prepared("services")
            request = {"family": "services", "profile": "slack", "operations": ["auth_test"],
                       "values": {"token_env": "SLACK_TOKEN"}}
            checked = controller.check(request)
            request.update(validation_id=checked["validation_id"],
                           configuration_sha256=checked["configuration_sha256"])
            job = controller.install(request)["job"]["id"]
            controller.join()
            receipt_path = controller.install_intents[job]["destination"] / "ready/installed.json"
            receipt = backend.decode(receipt_path.read_bytes())
            # Only installation is a fixture. The member/declaration observation
            # below is produced from actual selected wheel bytes by its owner.
            receipt["inputs"] = {wheel_name: hashlib.sha256(wheel).hexdigest()}
            receipt_path.write_bytes(backend.encoded(receipt))
            return controller, job, receipt_path

        def test_actual_publisher_observation_reaches_backend_result_unchanged(self):
            wheel = paths["wheel"].read_bytes()
            native = verifier.native_connector_observation({wheel_name: wheel}, "services")
            self.assertEqual(native, closure["expected_observation"])
            controller, job, receipt_path = self.installed_services(wheel)
            before = receipt_path.read_bytes()
            calls = len(controller.runner.calls)
            controller.runner.transform = lambda proof: dict(proof, native_connector=copy.deepcopy(native))
            observation = controller.reconcile({"job_id": job})["observation"]
            self.assertEqual(observation["status"], "matched")
            self.assertEqual(observation["result"]["native_connector"], native)
            self.assertEqual(observation["result"]["receipt_sha256"], hashlib.sha256(before).hexdigest())
            self.assertEqual(receipt_path.read_bytes(), before)
            self.assertNotIn("native_connector", backend.decode(before))
            self.assertEqual(len(controller.runner.calls), calls + 1)
            self.assertIn("--verify-existing", controller.runner.calls[-1][0])

        def test_actual_old_wheel_does_not_supply_native_entry(self):
            wheel = paths["unsupported_wheel"].read_bytes()
            self.assertIsNone(verifier.native_connector_observation({wheel_name: wheel}, "services"))
            controller, job, receipt_path = self.installed_services(wheel)
            before = receipt_path.read_bytes()
            observation = controller.reconcile({"job_id": job})["observation"]
            self.assertEqual(observation["status"], "matched")
            self.assertNotIn("native_connector", observation["result"])
            self.assertEqual(receipt_path.read_bytes(), before)

    return unittest.defaultTestLoader.loadTestsFromTestCase(PackageOwnerResultContract)


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--closure", required=True, type=Path)
    args = parser.parse_args()
    result = unittest.TextTestRunner(verbosity=2).run(contract_suite(args.closure))
    sys.exit(0 if result.wasSuccessful() else 1)
