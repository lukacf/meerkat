"""The explicit .48 update preserves accepted source and closed history.

All native proofs here are inert fixtures. No released bytes are installed,
and these tests do not activate Forge's still-.47 default.
"""
import copy
import json
from pathlib import Path

import pytest

from launchkit import compiler, deployment_updates as updates, generator
from launchkit.generator import PackageFile
from test_reading_runtime_refresh import background_receipt, previous_framework, refresh_pair
from test_reading_background_runtime_refresh import reading_background, private_bytes
from test_runtime_dependency_updates import native_fixture_proof


def future_target(monkeypatch):
    monkeypatch.setattr(compiler, "MOBKIT_VERSION", "0.8.48")
    monkeypatch.setattr(generator, "MOBKIT_VERSION", "0.8.48")


@pytest.fixture
def future_048(monkeypatch):
    future_target(monkeypatch)


@pytest.fixture
async def background_048(future_048, reading_background):
    yield reading_background


@pytest.mark.parametrize("predecessor", ["0.8.42", "0.8.46", "0.8.47"])
@pytest.mark.parametrize("background", [False, True])
def test_048_refresh_keeps_application_context_and_accepted_design(
        tmp_path, monkeypatch, predecessor, background):
    future_target(monkeypatch)
    previous, candidate, workspace, snapshot = refresh_pair(tmp_path, monkeypatch, predecessor)
    before = copy.deepcopy(previous)
    receipt = background_receipt() if background else {}
    transition = updates.runtime_dependency_transition(previous, candidate, receipt)
    assert transition and transition["from_version"] == predecessor
    assert transition["to_version"] == "0.8.48" and transition["core_version"] == "0.8.51"
    assert transition["definition_unchanged"] is True
    assert transition["native_registry_update"] is False
    assert transition["accepted_baseline_digest"] == workspace.framework_provenance()["baseline_digest"]
    assert transition["compiler_baseline_digest"] == workspace.framework_provenance()["current_compiler_baseline_digest"]
    assert updates.compatibility(previous, candidate, receipt)[1] == []
    for name in ("blueprint.json", "config/mob.toml", "config/console.toml", "config/connections.json",
                 "application/__init__.py", "application/configuration.json", "mcp_connections.py", ".env.example"):
        assert candidate[name] == previous[name]
    old, new = (json.loads(files["deployment.json"][0]) for files in (previous, candidate))
    assert old.pop("mobkitVersion") == predecessor and new.pop("mobkitVersion") == "0.8.48"
    assert old == new
    assert candidate["requirements.txt"] == (
        previous["requirements.txt"][0].replace(
            f"meerkat-mobkit=={predecessor}".encode(), b"meerkat-mobkit==0.8.48"), 0o644)
    assert previous == before and workspace.verification() is None
    assert workspace.framework_provenance()["previous"] == snapshot.origin


def test_048_support_does_not_admit_a_future_package_before_default_activation(tmp_path, monkeypatch):
    assert compiler.MOBKIT_VERSION == generator.MOBKIT_VERSION == "0.8.47"
    with monkeypatch.context() as future:
        future_target(future)
        previous, candidate, _, _ = refresh_pair(tmp_path, future, "0.8.47")
        assert updates.runtime_dependency_transition(previous, candidate, {}) is not None
    assert compiler.MOBKIT_VERSION == generator.MOBKIT_VERSION == "0.8.47"
    assert updates.runtime_dependency_transition(previous, candidate, {}) is None


@pytest.mark.parametrize("mutation", ["instructions", "tool-access", "application", "connector",
    "dependency", "host", "accepted-source"])
def test_048_refresh_refuses_unrelated_changes_to_the_accepted_application(tmp_path, monkeypatch, mutation):
    future_target(monkeypatch)
    previous, candidate, _, _ = refresh_pair(tmp_path, monkeypatch, "0.8.47")
    if mutation in {"instructions", "tool-access"}:
        text, mode = candidate["config/mob.toml"]
        old, new = ((b"Team purpose:", b"Altered purpose:") if mutation == "instructions"
            else (b"memory = false", b"memory = true"))
        assert old in text
        candidate["config/mob.toml"] = (text.replace(old, new, 1), mode)
    elif mutation == "dependency":
        text, mode = candidate["requirements.txt"]
        candidate["requirements.txt"] = (text + b"unaccepted-package==1.0.0\n", mode)
    elif mutation == "accepted-source":
        report = json.loads(candidate["implementation-report.json"][0])
        report["framework"]["previous"]["source_digest"] = "f" * 64
        candidate["implementation-report.json"] = (json.dumps(report).encode(), 0o644)
    else:
        name = {"application": "application/__init__.py", "connector": "mcp_connections.py",
            "host": "app.py"}[mutation]
        text, mode = candidate[name]
        candidate[name] = (text + b"# Unaccepted behavior.\n", mode)
    assert updates.runtime_dependency_transition(previous, candidate, {}) is None


@pytest.mark.parametrize("mutation", ["protocol", "worker", "missing-service", "remote"])
def test_048_background_refresh_requires_its_existing_owned_service_protocol(tmp_path, monkeypatch, mutation):
    future_target(monkeypatch)
    previous, candidate, _, _ = refresh_pair(tmp_path, monkeypatch, "0.8.47")
    receipt = background_receipt()
    if mutation == "protocol": receipt["service"]["update_startup_protocol"] = True
    elif mutation == "worker": receipt["service"]["update_worker_sha256"] = "f" * 64
    elif mutation == "missing-service": receipt["service"] = None
    else: receipt["target"] = "gcp"
    assert updates.runtime_dependency_transition(previous, candidate, receipt) is None


@pytest.mark.parametrize("predecessor,target", [("0.8.48", "0.8.48"), ("0.8.48", "0.8.47"),
    ("0.8.49", "0.8.48"), ("0.8.47", "0.8.49")])
def test_refresh_refuses_same_backward_or_unknown_release_without_source_changes(monkeypatch, predecessor, target):
    blueprint, snapshot = previous_framework(monkeypatch, "0.8.47")
    accepted = []
    for item in snapshot.files:
        if item.path in {"deployment.json", "requirements.txt"}:
            item = PackageFile(item.path, item.content.replace("0.8.47", predecessor), item.mode)
        accepted.append(item)
    before = [(item.path, item.content, item.mode) for item in accepted]
    monkeypatch.setattr(compiler, "MOBKIT_VERSION", target)
    monkeypatch.setattr(generator, "MOBKIT_VERSION", target)
    with pytest.raises(ValueError, match="qualified current release|supported predecessor"):
        compiler.generate_runtime_refresh(blueprint, accepted)
    assert [(item.path, item.content, item.mode) for item in accepted] == before


def closed_crossing(tmp_path, monkeypatch):
    future_target(monkeypatch)
    previous, candidate, _, _ = refresh_pair(tmp_path, monkeypatch, "0.8.47")
    root = tmp_path / "closed-update"
    root.mkdir()
    old, new = (json.loads(files["implementation-report.json"][0]) for files in (previous, candidate))
    old_bound = {key: "7" * 64 if key == "package_sha256" else old[key] for key in updates.BOUND_KEYS}
    receipt = {key: "8" * 64 if key == "package_sha256" else new[key] for key in updates.BOUND_KEYS}
    receipt["destination"] = str(tmp_path / "inert-destination")
    (root / "previous-receipt.json").write_text(json.dumps(old_bound))
    proof = native_fixture_proof("0.8.48")
    proof["core_version"] = "0.8.51"
    admitted = updates.runtime_dependency_transition(previous, candidate, receipt)
    assert admitted is not None and admitted["version"] == 4
    update = {"status": "completed", "blockers": [], "rollback_available": False,
        "runtime_dependency_update": admitted,
        "runtime_admission_possible": True, "native_definition_update": False,
        "host_instruction_update": False, "from_build_id": old["build_id"], "to_build_id": new["build_id"],
        "previous_source_digest": old["verification"]["digest"], "source_digest": new["verification"]["digest"],
        "previous_package_sha256": old_bound["package_sha256"], "package_sha256": receipt["package_sha256"],
        "guide_version": new["guide_version"], "blueprint_hash": new["blueprint_hash"], "brief_hash": new["brief_hash"],
        "runtime_files_retained": {"format": "mobkit-launch-runtime-retention-v1", "retained": True,
            "destination": receipt["destination"], "backup": str(root / "runtime-before"),
            "inventory_sha256": "9" * 64, "previous_native": native_fixture_proof("0.8.47")},
        "runtime_artifacts": {"native": proof, "sdk": {"package": "meerkat-mobkit", "version": "0.8.48"}},
        "runtime_storage_upgrade": {"from_version": "0.8.47", "to_version": "0.8.48", "applied": False}}
    return previous, candidate, receipt, update, root


def test_closed_048_history_remains_readable_after_compiler_advances(tmp_path, monkeypatch):
    previous, candidate, receipt, update, root = closed_crossing(tmp_path, monkeypatch)
    before = copy.deepcopy((previous, candidate, receipt, update))
    retained = (root / "previous-receipt.json").read_bytes()
    expected = updates.runtime_dependency_transition(previous, candidate, receipt)
    assert expected is not None
    def no_new_compilation(*args, **kwargs):
        pytest.fail("Reading closed .48 history must not regenerate or admit a runtime.")
    monkeypatch.setattr(compiler, "generate_package", no_new_compilation)
    monkeypatch.setattr(generator, "MOBKIT_VERSION", "0.8.49")
    actual = updates._completed_runtime_dependency_transition(previous, candidate, receipt, update, root)
    assert actual == expected
    assert (previous, candidate, receipt, update) == before
    assert (root / "previous-receipt.json").read_bytes() == retained


@pytest.mark.parametrize("mutation", ["core", "sdk", "source", "platform", "baseline", "rollback"])
def test_closed_048_history_refuses_missing_or_foreign_qualification(tmp_path, monkeypatch, mutation):
    previous, candidate, receipt, update, root = closed_crossing(tmp_path, monkeypatch)
    if mutation == "core": update["runtime_artifacts"]["native"]["core_version"] = "0.8.52"
    elif mutation == "sdk": update["runtime_artifacts"]["sdk"]["version"] = "0.8.47"
    elif mutation == "source": update["runtime_artifacts"]["native"]["source"] = "accepted-candidate"
    elif mutation == "platform": update["runtime_artifacts"]["native"]["platform"] = "x86_64-unknown-linux-gnu"
    elif mutation == "rollback": update["rollback_available"] = True
    else:
        report = json.loads(candidate["implementation-report.json"][0])
        report["framework"]["baseline_digest"] = "f" * 64
        candidate["implementation-report.json"] = (json.dumps(report).encode(), 0o644)
    assert updates._completed_runtime_dependency_transition(previous, candidate, receipt, update, root) is None


async def test_048_background_worker_preserves_owner_and_history_through_fresh_health(
        background_048, monkeypatch):
    from launchkit import deployment_service as services
    from launchkit.deployment import read_receipt
    from launchkit.deployment_worker import Supervisor
    import test_runtime_dependency_updates as fixtures
    service, directory, destination, previous, identity, manager, frozen = background_048
    before = private_bytes(destination)
    unit = Path(frozen["unit_path"]).read_bytes()
    native = (destination / "config/mob.toml").read_bytes()
    original_proof = fixtures.native_fixture_proof
    def same_core_proof(version):
        proof = original_proof(version)
        if version == "0.8.48": proof["core_version"] = "0.8.51"
        return proof
    monkeypatch.setattr(fixtures, "native_fixture_proof", same_core_proof)
    supervisor, events = fixtures.mock_dependency_runtime(
        monkeypatch, directory, destination, target_version="0.8.48")
    monkeypatch.setattr(Supervisor, "command", type(supervisor).command)
    await services.update_background(directory, manager, "update", update_id=identity)
    receipt = read_receipt(directory)
    assert receipt["update"]["status"] == "completed" and receipt["status"] == "running"
    assert events == ["retain", "install", "native", "sdk", "storage", "doctor", "application-check", "native-final"]
    assert manager.starts == [service.implementation.identity]
    assert receipt["update_service_verification"]["service_instance_id"] != "previous-instance"
    assert receipt["update"]["runtime_dependency_update"]["to_version"] == "0.8.48"
    assert receipt["update"].get("native_definition_activation") is None
    assert Path(frozen["unit_path"]).read_bytes() == unit
    assert services.verify_service(directory, receipt)["host_files"] == frozen["host_files"]
    assert (destination / "config/mob.toml").read_bytes() == native
    assert (destination / ".env").read_bytes() == before[".env"]
    for name in (".mobkit/state/native.db", "data/ledger.db"):
        assert (destination / name).read_bytes().startswith(before[name])
    assert receipt["package_sha256"] != previous["package_sha256"]
