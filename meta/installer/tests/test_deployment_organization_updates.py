"""Organization source bindings follow real update and rollback publication."""
from copy import deepcopy
import uuid

import pytest

from launchkit import deployment, deployment_updates as updates
from launchkit.deployment_sources import read_package_source
from launchkit.deployment_worker import Supervisor, update_runtime
from launchkit.projects import fingerprint
from test_deployment import Guide, Implementation, Service, submit


TRANSITIONS = [("A", "B"), ("A", None), (None, "B")]


def organization(scope):
    return {"id": "example", "version": scope, "guide_context": "Approved context"}


async def staged_organization_update(tmp_path, before, after):
    guide = Guide()
    if before is not None:
        guide.state["organization"] = organization(before)
    implementation = Implementation(guide)
    implementation.digest = "a" * 64
    implementation.extra_files = {"application/logic.py": "VERSION = 'old'\n"}
    service = Service(tmp_path / "host", guide, implementation)
    destination = tmp_path / "application"
    await submit(service, destination)
    directory = service._directory(guide.identity)
    deployment.materialize(directory)
    await service.shutdown()
    deployment.update_receipt(directory, status="stopped")
    (destination / "data").mkdir()
    (destination / "data/ledger.db").write_bytes(b"latest domain records")
    (destination / ".mobkit/state").mkdir()
    (destination / ".mobkit/state/native.db").write_bytes(b"latest native records")
    private = (destination / ".env").read_bytes()
    previous = assert_source_scope(directory, before)
    if after is None:
        guide.state.pop("organization", None)
    else:
        guide.state["organization"] = organization(after)
    # The fixture's retained build inputs change only for the new build.
    implementation.original = deepcopy(guide.state)
    implementation.identity = str(uuid.uuid4())
    implementation.digest = "b" * 64
    implementation.extra_files = {"application/logic.py": "VERSION = 'new'\n"}
    receipt = await service.stage_update(guide.identity, previous["deployment_id"],
                                        guide.state["version"], implementation.identity)
    assert not receipt["update"]["blockers"]
    return directory, destination, previous, private, receipt["update"]["update_id"]


def assert_source_scope(directory, scope):
    receipt = deployment.read_receipt(directory)
    files = deployment.package_files((directory / "package.zip").read_bytes())
    # Use the reader consumed by package download, registry and recovery.
    source = read_package_source(files, receipt)
    assert ("organization_hash" in receipt) is (scope is not None)
    assert ("organization_hash" in source) is (scope is not None)
    if scope is not None:
        assert receipt["organization_hash"] == source["organization_hash"] == fingerprint(organization(scope))
    assert source["build_id"] == receipt["build_id"]
    return receipt


def assert_private_state(destination, private):
    assert (destination / ".env").read_bytes() == private
    assert (destination / "data/ledger.db").read_bytes() == b"latest domain records"
    assert (destination / ".mobkit/state/native.db").read_bytes() == b"latest native records"


class Ready(Supervisor):
    async def command(self, *args, **kwargs):
        pass

    async def start(self, definition, *, on_ready):
        on_ready()


@pytest.mark.parametrize("before,after", [*TRANSITIONS, (None, None), ("A", "A")])
async def test_update_and_manual_rollback_publish_the_selected_organization(tmp_path, before, after):
    directory, destination, previous, private, identity = await staged_organization_update(tmp_path, before, after)
    await update_runtime(Ready(directory), "update", update_id=identity)
    current = assert_source_scope(directory, after)
    assert current["update"]["status"] == "completed"
    assert current["build_id"] != previous["build_id"]
    await update_runtime(Ready(directory), "rollback", update_id=identity)
    restored = assert_source_scope(directory, before)
    assert restored["update"]["status"] == "rolled-back"
    assert restored["build_id"] == previous["build_id"]
    assert_private_state(destination, private)


@pytest.mark.parametrize("before,after", TRANSITIONS)
async def test_failed_update_restores_previous_organization(tmp_path, before, after):
    directory, destination, previous, private, identity = await staged_organization_update(tmp_path, before, after)
    class FailingCandidate(Ready):
        async def start(self, definition, *, on_ready):
            if deployment.read_receipt(directory)["build_id"] != previous["build_id"]:
                assert_source_scope(directory, after)
                raise RuntimeError("candidate fixture startup failed")
            assert_source_scope(directory, before)
            on_ready()
    await update_runtime(FailingCandidate(directory), "update", update_id=identity)
    restored = assert_source_scope(directory, before)
    assert restored["update"]["status"] == "rolled-back"
    assert "candidate fixture startup failed" in restored["update"]["activation_error"]
    assert_private_state(destination, private)


@pytest.mark.parametrize("before,after", TRANSITIONS)
async def test_failed_rollback_republishes_candidate_organization(tmp_path, before, after):
    directory, destination, previous, private, identity = await staged_organization_update(tmp_path, before, after)
    await update_runtime(Ready(directory), "update", update_id=identity)
    class FailingPrevious(Ready):
        async def start(self, definition, *, on_ready):
            if deployment.read_receipt(directory)["build_id"] == previous["build_id"]:
                assert_source_scope(directory, before)
                raise RuntimeError("previous fixture startup failed")
            assert_source_scope(directory, after)
            on_ready()
    await update_runtime(FailingPrevious(directory), "rollback", update_id=identity)
    current = assert_source_scope(directory, after)
    assert current["update"]["status"] == "completed"
    assert current["build_id"] != previous["build_id"]
    assert "previous fixture startup failed" in current["update"]["activation_error"]
    assert_private_state(destination, private)


@pytest.mark.parametrize("before,after", TRANSITIONS)
async def test_current_recovery_republishes_candidate_organization(tmp_path, before, after):
    directory, destination, previous, private, identity = await staged_organization_update(tmp_path, before, after)
    updates.activate(directory, identity)
    _, root, update = updates.transaction(directory, identity)
    updates.checkpoint(directory, root, update, runtime_admission_possible=True)
    updates.restore(directory, identity, restore_data=False, rollback_kind="failed-update")
    assert_source_scope(directory, before)
    _, root, update = updates.transaction(directory, identity)
    updates.checkpoint(directory, root, update, status="recovery-needed", rollback_available=True,
                       activation_error="previous source cannot read current state")
    deployment.update_receipt(directory, status="failed", phase="recovery", health="unknown", process_live=False)
    await update_runtime(Ready(directory), "recover-current", update_id=identity)
    current = assert_source_scope(directory, after)
    assert current["update"]["status"] == "completed"
    assert current["build_id"] != previous["build_id"]
    assert_private_state(destination, private)


async def test_legacy_update_stale_organization_is_refused_without_receipt_repair(tmp_path):
    directory, _, previous, _, identity = await staged_organization_update(tmp_path, "A", "B")
    await update_runtime(Ready(directory), "update", update_id=identity)
    assert_source_scope(directory, "B")
    # Model the old publisher retaining the predecessor's field. This does not
    # claim execution of an older Forge binary or a downgrade qualification.
    deployment.update_receipt(directory, organization_hash=previous["organization_hash"])
    receipt_path = directory / "receipt.json"
    before = receipt_path.read_bytes()
    files = deployment.package_files((directory / "package.zip").read_bytes())
    with pytest.raises(ValueError, match=r"^The installed source binding could not be verified\.$"):
        read_package_source(files, deployment.read_receipt(directory))
    assert receipt_path.read_bytes() == before
