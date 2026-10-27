"""Pinned multi-request recovery through existing owners, without effect replay."""

import copy
import json
from pathlib import Path
import subprocess
import sys
import unittest
from unittest.mock import patch

import test_acquisition_recovery as acquisition
import test_export_recovery as export
import test_hosted_recipes as hosted
import test_selected_setup as selected
from test_reconciliation import ReconciliationMixin, backend
from test_recovery import inventory
from test_scoped_config_recovery import ScopedRunner


def manifest(path, original, identifiers):
    value = {
        "schema_version": 1,
        "workspace": str(original.workspace),
        "requests": [
            copy.deepcopy(original.jobs[identifier]["recovery_request"])
            for identifier in identifiers
        ],
    }
    path.write_bytes(backend.encoded(value))
    return {
        "recover_manifest": path,
        "recover_manifest_sha256": backend.digest(path.read_bytes()),
    }


class MixedRunner(ScopedRunner):
    """Compose existing inert owner fixtures, keeping one actual Controller."""

    def __init__(self, workspace):
        super().__init__(workspace)
        self.selected = selected.SelectedRunner(workspace)
        self.export = export.ExportCheckRunner(workspace)
        self.selected.calls = self.export.calls = self.calls

    def run(self, argv, capture=False):
        if "--package" in argv or any(
            flag in argv for flag in ("--selected-python", "--selected-node")
        ):
            return self.selected.run(argv, capture=capture)
        if len(argv) > 1 and argv[1] in ("save", "read", "export", "check-export"):
            return self.export.run(argv, capture=capture)
        return super().run(argv, capture=capture)


class RecoverySelectionTests(ReconciliationMixin, unittest.TestCase):
    def controller(self, **changes):
        return super().controller(**{"runner_factory": MixedRunner, **changes})

    def pair(self, **changes):
        original = self.controller(**changes)
        original.prepare({"family": "services"})
        original.join()
        ids = []
        for profile, field, ceiling in (
            ("slack", "allowed_channel_ids", ["C1"]),
            ("gmail", "allowed_recipient_addresses", ["reader@example.invalid"]),
        ):
            request = {
                "family": "services",
                "profile": profile,
                "operations": ["auth_test" if profile == "slack" else "send_message"],
                "values": {
                    "token_env": "FIXTURE_TOKEN",
                    "timeout_seconds": 10,
                    field: ceiling,
                },
            }
            checked = original.check(request)
            job = original.install(
                {
                    **request,
                    **{
                        k: checked[k] for k in ("validation_id", "configuration_sha256")
                    },
                }
            )["job"]
            original.join()
            self.assertEqual(original.jobs[job["id"]]["status"], "succeeded")
            ids.append(job["id"])
        return original, ids

    def recover(self, original, ids, **changes):
        args = manifest(
            self.root / ("manifest-" + str(self.attempts) + ".json"), original, ids
        )
        return self.controller(**{**args, **changes})

    def test_two_results_keep_order_ceilings_and_independent_observations(self):
        original, ids = self.pair()
        original.close_admission()
        original.join()
        before = inventory(original.workspace)
        recovered = self.recover(original, ids)
        self.assertEqual(recovered.runner.calls, [])
        self.assertEqual(list(recovered.jobs), ids)
        self.assertEqual(recovered.state()["job"]["id"], ids[-1])
        self.assertEqual(
            recovered.state()["job_history"],
            {"available": True, "count": 2, "recovery_selection": True},
        )
        summaries = recovered.job_summaries({"before": None})["jobs"]
        self.assertEqual([row["id"] for row in summaries], list(reversed(ids)))
        for row in summaries:
            self.assertTrue(row["recovered"])
            self.assertEqual(
                row["recovery_request"], original.jobs[row["id"]]["recovery_request"]
            )
        for identifier in ids:
            job = recovered.job_detail({"job_id": identifier})["job"]
            self.assertEqual(job["error"], "historical_outcome_unknown")
            self.assertNotIn("result", job)
            result = recovered.reconcile({"job_id": identifier})["observation"]
            self.assertEqual(result["status"], "matched")
            self.assertEqual(
                result["result"]["mcp_configuration"],
                original.jobs[identifier]["result"]["mcp_configuration"],
            )
            self.assertEqual(recovered.jobs[identifier], job)
            config = recovered.install_intents[identifier]["configuration"]
            self.assertEqual(
                backend.decode(config["path"].read_bytes())["config"],
                backend.decode(
                    (
                        original.workspace / "requests" / (identifier + ".json")
                    ).read_bytes()
                )["selection"]["values"],
            )
        self.assertEqual(before, inventory(original.workspace))
        self.assertTrue(
            all("--verify-existing" in argv for argv, _ in recovered.runner.calls)
        )

    def test_changed_member_or_anchor_affects_only_its_exact_job(self):
        for target in ("receipt", "anchor"):
            with self.subTest(target=target):
                original, ids = self.pair()
                recovered = self.recover(original, ids)
                path = (
                    Path(original.jobs[ids[0]]["result"]["receipt_path"])
                    if target == "receipt"
                    else original.workspace / "requests" / (ids[0] + ".json")
                )
                path.write_bytes(b"{}")
                bad = recovered.reconcile({"job_id": ids[0]})["observation"]
                good = recovered.reconcile({"job_id": ids[1]})["observation"]
                self.assertEqual(
                    (bad["status"], good["status"]), ("unconfirmed", "matched")
                )
                self.assertNotIn("result", bad)
                self.assertEqual(
                    recovered.job_detail({"job_id": ids[0]})["reconciliation"][
                        "observation"
                    ],
                    bad,
                )
                self.assertEqual(
                    recovered.state()["reconciliation"]["observation"], good
                )

    def test_invalid_manifest_and_missing_or_incompatible_request_create_no_workspace(
        self,
    ):
        original, ids = self.pair()
        path = self.root / "bad.json"
        good = {
            "schema_version": 1,
            "workspace": str(original.workspace),
            "requests": [original.jobs[i]["recovery_request"] for i in ids],
        }
        variants = [
            None,
            {},
            {**good, "schema_version": True},
            {**good, "requests": []},
            {**good, "requests": good["requests"] * 129},
            {**good, "extra": "private"},
            {**good, "requests": [good["requests"][0]] * 2},
            {
                **good,
                "requests": [
                    good["requests"][0],
                    {**good["requests"][0], "sha256": "a" * 64},
                ],
            },
            {**good, "requests": [{"id": "../escape", "sha256": "b" * 64}]},
            {**good, "requests": [{**good["requests"][0], "status": "succeeded"}]},
            {**good, "requests": [{"id": "0" * 32, "sha256": "b" * 64}]},
            {**good, "workspace": str(original.workspace) + "/."},
        ]
        for index, value in enumerate(variants):
            path.write_bytes(backend.encoded(value))
            target = self.root / ("bad-work-" + str(index))
            with (
                self.subTest(value=value),
                self.assertRaises((backend.SetupError, OSError)),
            ):
                self.controller(
                    workspace=target,
                    recover_manifest=path,
                    recover_manifest_sha256=backend.digest(path.read_bytes()),
                )
            self.assertFalse(target.exists())
        for changes in (
            {"recover_manifest_sha256": "0" * 64},
            {"recover_manifest_sha256": None},
            {"recover_request": ids[0]},
            {"prior_request_path": path},
            {"families": ("agno",)},
            {"verifier": None, "verifier_sha256": None},
        ):
            path.write_bytes(backend.encoded(good))
            target = self.root / ("bad-flags-" + str(self.attempts))
            args = {
                "workspace": target,
                "recover_manifest": path,
                "recover_manifest_sha256": backend.digest(path.read_bytes()),
                **changes,
            }
            with self.subTest(changes=changes), self.assertRaises(backend.SetupError):
                self.controller(**args)
            self.assertFalse(target.exists())

    def test_limits_precede_request_reads_and_manifest_is_frozen_exactly(self):
        original, ids = self.pair()
        path = self.root / "selection.json"
        args = manifest(path, original, ids)
        data = path.read_bytes()
        recovered = self.controller(**args)
        path.write_bytes(b"{}")
        self.assertEqual(
            (recovered.workspace / "recovery-manifest.json").read_bytes(), data
        )
        for value in (
            {"schema_version": 1, "workspace": str(original.workspace), "requests": []},
            {
                "schema_version": 1,
                "workspace": str(original.workspace),
                "requests": [
                    {"id": f"{i:032x}", "sha256": "a" * 64} for i in range(257)
                ],
            },
        ):
            path.write_bytes(backend.encoded(value))
            real_read = backend.read_file

            def guard(root, name, maximum=backend.MAX_JSON):
                self.assertFalse(name.startswith("requests/"))
                return real_read(root, name, maximum)

            with (
                patch.object(backend, "read_file", guard),
                self.assertRaises(backend.SetupError),
            ):
                self.controller(
                    recover_manifest=path,
                    recover_manifest_sha256=backend.digest(path.read_bytes()),
                )
        path.write_bytes(b" " * (backend.MAX_BODY + 1))
        with self.assertRaises(backend.SetupError):
            self.controller(
                recover_manifest=path,
                recover_manifest_sha256=backend.digest(path.read_bytes()),
            )

    def test_no_mutations_and_legacy_shape_stays_unchanged(self):
        original, ids = self.pair()
        recovered = self.recover(original, ids)
        for path, args in (
            ("/api/prepare", {"family": "services"}),
            ("/api/stop", {"job_id": ids[0]}),
            ("/api/retry_prepare", {"job_id": ids[0]}),
            ("/api/check", self.request()),
        ):
            with (
                self.subTest(path=path),
                self.assertRaisesRegex(backend.SetupError, "recovery_read_only"),
            ):
                recovered.dispatch(path, args)
        legacy = self.controller(
            recover_workspace=original.workspace,
            recover_request=ids[0],
            recover_request_sha256=original.jobs[ids[0]]["recovery_request"]["sha256"],
        )
        self.assertNotIn("recovery_selection", legacy.state()["job_history"])
        self.assertFalse(legacy.recovery_selection)
        self.assertEqual(recovered.runner.calls, [])

    def test_mixed_library_selected_and_export_use_retained_kind(self):
        selected_case = type(
            "SelectedCase", (selected.SelectedFixture, unittest.TestCase), {}
        )()
        selected_case.setUp()
        self.addCleanup(selected_case.tearDown)
        export_case = type(
            "ExportCase", (hosted.HostedFixture, unittest.TestCase), {}
        )()
        export_case.setUp()
        self.addCleanup(export_case.tearDown)
        settings = {
            "selected_install_profile": selected_case.profile_path,
            "selected_install_profile_sha256": selected_case.profile_sha,
            "local_recipes_profile": export_case.profile_path,
            "local_recipes_profile_sha256": export_case.profile_sha,
        }
        original, ids = self.pair(**settings, allow_remote_recipes=True)
        checked = original.selected_check(selected_case.stage())
        job = original.selected_install(selected_case.selection(checked))["job"]
        original.join()
        ids.append(job["id"])
        self.assertEqual(original.jobs[ids[-1]]["status"], "succeeded")
        original.remember_remote(hosted.REQUEST)
        job = original.export_recipe(hosted.EXPORT)["job"]
        original.join()
        ids.append(job["id"])
        self.assertEqual(original.jobs[ids[-1]]["status"], "succeeded")
        recovered = self.recover(original, ids, **settings)
        self.assertEqual(
            [job["kind"] for job in recovered.jobs.values()],
            ["install", "install", "install", "export"],
        )
        self.assertEqual(recovered.runner.calls, [])
        for identifier in ids:
            self.assertEqual(
                recovered.reconcile({"job_id": identifier})["observation"]["status"],
                "matched",
            )
        self.assertEqual(
            sum("--verify-existing" in argv for argv, _ in recovered.runner.calls), 3
        )
        self.assertEqual(
            sum(argv[1] == "check-export" for argv, _ in recovered.runner.calls), 1
        )

    def test_mixed_library_and_acquisition_need_no_external_acquisition_tools(self):
        case = acquisition.AcquisitionRecoveryTests()
        self.addCleanup(case.doCleanups)
        fixture, source, _ = case.package_controller("python")
        profile = fixture.root / "selected.json"
        original, ids = self.pair(
            selected_install_profile=profile,
            selected_install_profile_sha256=backend.digest(profile.read_bytes()),
            python_acquisition_profile=fixture.profile_path,
            python_acquisition_profile_sha256=backend.digest(
                fixture.profile_path.read_bytes()
            ),
        )
        owner = original.python_acquisition
        run = owner.run

        def acquire(request, directory, run_callable, check_stop):
            fixture.job = directory
            with patch.object(
                owner,
                "fetch_retained",
                lambda row, checkpoint, *_: fixture.fetch(row, checkpoint),
            ):
                return run(request, directory, fixture.runner, check_stop)

        owner.run = acquire
        job = original.acquire_python(fixture.request)["job"]
        original.join()
        self.assertEqual(original.jobs[job["id"]]["status"], "succeeded")
        ids.append(job["id"])
        fixture.profile_path.unlink()
        fixture.pip.unlink()
        recovered = self.recover(original, ids)
        self.assertEqual(recovered.runner.calls, [])
        for identifier in ids:
            observed = recovered.reconcile({"job_id": identifier})["observation"]
            self.assertEqual(observed["status"], "matched")
        self.assertEqual(len(recovered.runner.calls), 2)
        self.assertEqual(
            recovered.acquisition_intents[ids[-1]]["root"], original.workspace
        )

    def test_anchor_change_during_import_prevents_publication(self):
        original, ids = self.pair()
        args = manifest(self.root / "race.json", original, ids)
        loader = backend.load_recovery
        target = self.root / "race-work"

        def changed(*args, **kwargs):
            result = loader(*args, **kwargs)
            path = original.workspace / "requests" / (ids[0] + ".json")
            path.write_bytes(path.read_bytes() + b" ")
            return result

        with (
            patch.object(backend, "load_recovery", changed),
            self.assertRaises(backend.SetupError),
        ):
            self.controller(workspace=target, **args)
        self.assertFalse(target.exists())

    def test_fresh_process_recovers_two_without_installing_or_losing_first(self):
        original, ids = self.pair()
        original.close_admission()
        original.join()
        args = manifest(self.root / "fresh.json", original, ids)
        values = {
            "bundle": str(self.bundle),
            "contents_sha256": self.sha,
            "workspace": str(self.root / "fresh-worker"),
            "families": list(backend.FAMILIES),
            "python": sys.executable,
            "node": sys.executable,
            "npm_cli": sys.executable,
            "verifier": str(self.bundle / "install-library-consumer.py"),
            "verifier_sha256": self.manifest["files"]["install-library-consumer.py"][
                "sha256"
            ],
            **{k: str(v) for k, v in args.items()},
        }
        config = self.root / "child.json"
        config.write_text(json.dumps(values))
        script = """import json,pathlib,sys
sys.path.insert(0,sys.argv[1]); import test_recovery_selection as t
a=json.loads(pathlib.Path(sys.argv[2]).read_text())
for name in ('bundle','workspace','verifier','recover_manifest'):a[name]=pathlib.Path(a[name])
c=t.backend.Controller(**a,runner_factory=t.MixedRunner)
assert c.runner.calls==[]
results=[c.reconcile({'job_id':i})['observation'] for i in c.jobs]
assert all('--verify-existing' in a for a,_ in c.runner.calls)
print(json.dumps({'jobs':c.job_summaries({'before':None}),'results':results}))
"""
        before = inventory(original.workspace)
        run = subprocess.run(
            [
                sys.executable,
                "-I",
                "-B",
                "-c",
                script,
                str(Path(__file__).parent),
                str(config),
            ],
            env={"PATH": "/usr/bin:/bin", "PYTHONDONTWRITEBYTECODE": "1"},
            capture_output=True,
            timeout=15,
        )
        self.assertEqual(run.returncode, 0, run.stderr)
        observed = json.loads(run.stdout)
        self.assertEqual(
            [item["status"] for item in observed["results"]], ["matched", "matched"]
        )
        self.assertEqual([item["id"] for item in observed["jobs"]["jobs"]], ids[::-1])
        self.assertEqual(before, inventory(original.workspace))


class AcquisitionSelectionTests(unittest.TestCase):
    def test_two_acquisitions_keep_anchors_and_need_no_original_tools(self):
        case = acquisition.AcquisitionRecoveryTests()
        self.addCleanup(case.doCleanups)
        fixture, original, owner = case.package_controller("python")
        identifiers = []
        for identifier in ("1" * 32, "2" * 32):
            original.acquire_python({**fixture.request, "request_id": identifier})
            original.join()
            self.assertEqual(original.job["status"], "succeeded", original.job)
            identifiers.append(original.job["id"])
        args = manifest(fixture.root / "recover.json", original, identifiers)
        owner.profile_path.unlink()
        fixture.pip.unlink()
        recovered = acquisition.backend.Controller(
            None,
            None,
            fixture.root / "multi-recovery",
            (),
            sys.executable,
            None,
            None,
            **args,
        )
        self.addCleanup(recovered.join)
        with patch.object(
            subprocess, "Popen", side_effect=AssertionError("unexpected process")
        ):
            for identifier in identifiers:
                observed = recovered.reconcile({"job_id": identifier})["observation"]
                self.assertEqual(observed["status"], "matched", observed)
                self.assertEqual(
                    observed["result"], original.jobs[identifier]["result"]
                )
        self.assertTrue(recovered.state()["python_acquisition"]["readback_only"])

    def test_incompatible_same_kind_and_staging_overlap_refuse_before_mkdir(self):
        case = acquisition.AcquisitionRecoveryTests()
        self.addCleanup(case.doCleanups)
        fixture, original, _ = case.package_controller("python")
        ids = []
        for identifier in ("1" * 32, "2" * 32):
            original.acquire_python({**fixture.request, "request_id": identifier})
            original.join()
            ids.append(original.job["id"])
        target = fixture.staging / "forbidden-recovery"
        args = manifest(fixture.root / "recover.json", original, ids)
        with self.assertRaises(acquisition.backend.SetupError):
            acquisition.backend.Controller(
                None, None, target, (), sys.executable, None, None, **args
            )
        self.assertFalse(target.exists())
        path = original.workspace / "requests" / (ids[1] + ".json")
        value = acquisition.backend.decode(path.read_bytes())
        value["staging_root"] += "-other"
        path.write_bytes(acquisition.backend.encoded(value))
        original.jobs[ids[1]]["recovery_request"]["sha256"] = (
            acquisition.backend.digest(path.read_bytes())
        )
        args = manifest(fixture.root / "recover.json", original, ids)
        target = fixture.root / "incompatible"
        with self.assertRaisesRegex(
            acquisition.backend.SetupError, "recovery_profile_changed"
        ):
            acquisition.backend.Controller(
                None, None, target, (), sys.executable, None, None, **args
            )
        self.assertFalse(target.exists())


if __name__ == "__main__":
    unittest.main()
