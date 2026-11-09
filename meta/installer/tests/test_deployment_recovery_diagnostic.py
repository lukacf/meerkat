"""The current fallback failure reaches the existing recovery UI projection."""
import json
from pathlib import Path

import pytest

from launchkit import deployment_updates as updates
from launchkit import deployment_worker as worker
from launchkit.deployment import read_receipt
from test_deployment_updates import staged


async def test_double_start_failure_retains_distinct_redacted_current_recovery_reason_and_latest_state(tmp_path):
    # This also proves that the isolated control never imported the shared host.
    assert Path(worker.__file__).resolve() == Path(__file__).resolve().parents[1] / "launchkit/deployment_worker.py"
    _, directory, destination, previous, private, identity = await staged(tmp_path)
    candidate_build = read_receipt(directory)["update"]["to_build_id"]
    _, update_root, update = updates.transaction(directory, identity)
    updates.checkpoint(directory, update_root, update, recovery_error="obsolete earlier recovery failure")
    candidate_error = "Candidate application A failed before Console readiness"
    private_value = worker.read_settings(destination)["OPENAI_API_KEY"]
    fallback_error = f"Restored application B rejected {private_value}; " + "details " * 260
    latest_domain = b"new application receipt recorded before failed readiness"
    latest_native = b"new native occurrence recorded before failed readiness"
    checked_builds, started_builds = [], []

    class FailedCandidateAndRecovery(worker.Supervisor):
        async def command(self, argv, *args, **kwargs):
            # No subprocess, application check or native bootstrap is executed.
            assert argv[-1] in {"--doctor", "--application-check"}
            checked_builds.append(read_receipt(directory)["build_id"])

        async def native_definition_revision(self, *args, **kwargs):
            raise AssertionError("An ordinary source update must not enter native observation here.")

        async def start(self, definition, *, on_ready):
            started_builds.append(read_receipt(directory)["build_id"])
            assert read_receipt(directory)["update"]["runtime_admission_possible"] is True
            if len(started_builds) == 1:
                assert started_builds[-1] == candidate_build
                (destination / "data/ledger.db").write_bytes(latest_domain)
                (destination / ".mobkit/state/native.db").write_bytes(latest_native)
                (destination / "workspace/attempt.txt").write_text("current failed-attempt record")
                raise RuntimeError(candidate_error)
            assert started_builds[-1] == previous["build_id"]
            assert (destination / "data/ledger.db").read_bytes() == latest_domain
            assert (destination / ".mobkit/state/native.db").read_bytes() == latest_native
            raise RuntimeError(fallback_error)

    supervisor = FailedCandidateAndRecovery(directory)
    with pytest.raises(RuntimeError, match="source update needs recovery") as failure:
        await worker.update_runtime(supervisor, "update", update_id=identity)

    receipt = read_receipt(directory)
    authoritative = json.loads((update_root / "update.json").read_text())
    assert started_builds == [candidate_build, previous["build_id"]]
    assert checked_builds == [candidate_build, candidate_build, previous["build_id"], previous["build_id"]]
    assert receipt["package_sha256"] == previous["package_sha256"]
    assert receipt["build_id"] == previous["build_id"]
    assert receipt["update"] == authoritative
    assert authoritative["status"] == "recovery-needed"
    assert authoritative["rollback_available"] is True
    assert authoritative["runtime_admission_possible"] is True
    assert (destination / "application/logic.py").read_text() == "VERSION = 'old'\n"
    assert not (destination / "application/new.py").exists()
    assert (destination / ".env").read_bytes() == private
    assert (destination / "data/ledger.db").read_bytes() == latest_domain
    assert (destination / ".mobkit/state/native.db").read_bytes() == latest_native
    assert (destination / "workspace/attempt.txt").read_text() == "current failed-attempt record"
    assert authoritative["activation_error"] == candidate_error
    assert failure.value.__cause__.args == (fallback_error,)
    expected = supervisor.redact(fallback_error)[:1500]
    assert authoritative.get("recovery_error") == expected
    assert expected.startswith("Restored application B rejected [private]; ")
    assert len(expected) == 1500
    assert private_value not in json.dumps(receipt)
    assert "obsolete earlier recovery failure" not in json.dumps(receipt)
