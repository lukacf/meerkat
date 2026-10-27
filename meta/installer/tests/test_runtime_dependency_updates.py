"""Qualified dependency updates preserve native authority and current work."""
import asyncio
import copy
import importlib
import io
import json
from pathlib import Path
import sys
from types import ModuleType
import zipfile

import pytest

from launchkit import compiler, deployment, generator
from launchkit import deployment_updates as updates
from launchkit.blueprint import starter_blueprint
from launchkit.deployment import read_receipt
from launchkit.deployment_worker import Supervisor, update_runtime
from launchkit.projects import DraftConflict, fingerprint
from launchkit.templates import storage_setup
from test_deployment import Guide, Implementation
from test_deployment_updates import canonical_update_package, prepared, staged


from historical_runtime_fixture import qualified_runtime_compiler


def dependency_packages(monkeypatch, before=None, after=None):
    if before is None:
        before = starter_blueprint().model_dump(mode="json")
        # This fixture exercises a crossing with no registry change. The
        # separate registry cases explicitly select the formerly custom model.
        for agent in before["agent_classes"]:
            agent.update(provider="openai", model="gpt-5.5")
    after = after or copy.deepcopy(before)
    with monkeypatch.context() as legacy:
        legacy.setattr(compiler, "MOBKIT_VERSION", "0.8.42")
        legacy.setattr(generator, "MOBKIT_VERSION", "0.8.42")
        previous = canonical_update_package(before)
    candidate = canonical_update_package(after)
    # Application package_files projects source modes to executable or plain.
    return ({name: (content, 0o755 if mode & 0o111 else 0o644) for name, (content, mode) in previous.items()},
            {name: (content, 0o755 if mode & 0o111 else 0o644) for name, (content, mode) in candidate.items()})


def change_json(files, name, change):
    value = json.loads(files[name][0])
    change(value)
    files[name] = (json.dumps(value).encode(), files[name][1])


def test_current_compiler_dependency_transition_keeps_exact_agent_authority(monkeypatch):
    previous, candidate = dependency_packages(monkeypatch)
    transition = updates.runtime_dependency_transition(previous, candidate, {})
    assert transition is not None
    assert transition["from_version"] == "0.8.42" and transition["to_version"] == generator.MOBKIT_VERSION
    assert transition["version"] == (1 if generator.MOBKIT_VERSION == "0.8.46" else 4)
    assert len(transition["protected_paths"]) == (26 if generator.MOBKIT_VERSION == "0.8.46" else 75)
    assert transition["core_version"] == "0.8.51" and transition["definition_unchanged"] is True
    assert transition["recovery"] == "forward-only-after-runtime-admission"
    assert previous["config/mob.toml"] == candidate["config/mob.toml"]
    assert previous["blueprint.json"] == candidate["blueprint.json"]
    assert not updates.instruction_configuration_update(previous, candidate)
    assert not updates.compatibility(previous, candidate, {})[1]


@pytest.mark.parametrize("mutation", ["grant", "model", "identity", "unknown", "downgrade",
    "other-pin", "application-pin", "mixed-host", "host-mode", "native-mode", "requirements-mode",
    "unknown-version", "missing-provenance", "foreign-provenance"])
def test_dependency_descriptor_refuses_authority_pin_host_and_mode_changes(monkeypatch, mutation):
    before = starter_blueprint().model_dump(mode="json")
    after = copy.deepcopy(before)
    if mutation == "grant":
        after["agent_classes"][0]["tools"]["shell"] = not after["agent_classes"][0]["tools"]["shell"]
    elif mutation == "model":
        after["agent_classes"][0]["model"] = "gpt-6-luna"
    previous, candidate = dependency_packages(monkeypatch, before, after)
    if mutation == "identity":
        change_json(candidate, "deployment.json", lambda value: value["agents"][0].update(identity="foreign:main"))
    elif mutation == "unknown":
        change_json(candidate, "deployment.json", lambda value: value.update(unknown_authority=True))
    elif mutation == "downgrade":
        previous, candidate = candidate, previous
    elif mutation == "unknown-version":
        change_json(candidate, "deployment.json", lambda value: value.update(mobkitVersion="0.8.49"))
    elif mutation == "other-pin":
        candidate["requirements.txt"] = (candidate["requirements.txt"][0] + b"extra-package==1.0\n", 0o644)
    elif mutation == "application-pin":
        candidate["requirements.application.txt"] = (b"new-application-package==1.0\n", 0o644)
    elif mutation == "mixed-host":
        candidate["app.py"] = (candidate["app.py"][0] + b"# Authored host override\n", 0o644)
    elif mutation.endswith("-mode"):
        name = {"host-mode": "app.py", "native-mode": "config/mob.toml", "requirements-mode": "requirements.txt"}[mutation]
        candidate[name] = (candidate[name][0], 0o755)
    elif mutation == "missing-provenance":
        previous.pop("implementation-report.json")
    elif mutation == "foreign-provenance":
        change_json(candidate, "implementation-report.json", lambda value: value["framework"].update(baseline_digest="f" * 64))
    assert updates.runtime_dependency_transition(previous, candidate, {}) is None
    assert updates.compatibility(previous, candidate, {})[1]


@pytest.mark.parametrize("receipt", [{"execution_mode": "background"}, {"target": "gcp"},
                                     {"service": {"kind": "launchd"}}])
def test_dependency_descriptor_does_not_admit_other_execution_owners(monkeypatch, receipt):
    previous, candidate = dependency_packages(monkeypatch)
    assert updates.runtime_dependency_transition(previous, candidate, receipt) is None
    assert updates.compatibility(previous, candidate, receipt)[1]


@pytest.fixture
def canonical_fixture_packages(monkeypatch, tmp_path):
    """Reuse the ordinary prepared/staged fixtures with canonical package bytes."""
    # The worker is mocked throughout; no test reserves or starts its real port.
    monkeypatch.setattr(deployment, "require_available_console_port", lambda port: None)
    # prepared() creates an empty workspace for its minimal package. The real
    # compiler already seeds it with START-HERE.txt; retain that owned file.
    mkdir = Path.mkdir
    def seeded_workspace(path, *args, **kwargs):
        if path == tmp_path / "application/workspace" and path.is_dir() and not args and not kwargs:
            return None
        return mkdir(path, *args, **kwargs)
    monkeypatch.setattr(Path, "mkdir", seeded_workspace)
    original_guide = Guide.__init__
    def unchanged_registry(guide):
        original_guide(guide)
        for agent in guide.state["proposal"]["agent_classes"]:
            agent.update(provider="openai", model="gpt-5.5")
    monkeypatch.setattr(Guide, "__init__", unchanged_registry)
    async def package(self, identity, build_id):
        assert build_id == self.identity
        await self.guide.get(identity)
        if not hasattr(self, "_original_package_build"):
            self._original_package_build = build_id
        legacy = build_id == self._original_package_build
        if legacy:
            with monkeypatch.context() as old:
                old.setattr(compiler, "MOBKIT_VERSION", "0.8.42")
                old.setattr(generator, "MOBKIT_VERSION", "0.8.42")
                files = canonical_update_package(self.guide.state["proposal"])
        else:
            files = canonical_update_package(self.guide.state["proposal"])
        if legacy and getattr(self.guide, "legacy_registry_host", False):
            files["app.py"] = (b"# Previously accepted host without current instruction carrier\n", 0o644)
        report = json.loads(files["implementation-report.json"][0])
        report.update(build_id=build_id, guide_version=self.original["version"],
            brief_hash=fingerprint(self.guide.state.get("brief") or {}),
            blueprint_hash=fingerprint(self.guide.state["proposal"]),
            verification={"digest": self.digest}, unresolved=["Actual external data not verified"])
        files["implementation-report.json"] = (json.dumps(report).encode(), 0o644)
        files.update({name: (content.encode(), 0o644) for name, content in self.extra_files.items()})
        stream = io.BytesIO()
        with zipfile.ZipFile(stream, "w") as archive:
            for name, (content, mode) in files.items():
                entry = zipfile.ZipInfo(name)
                entry.external_attr = (0o100000 | mode) << 16
                archive.writestr(entry, content)
        return stream.getvalue(), json.loads(files["deployment.json"][0])["slug"]
    monkeypatch.setattr(Implementation, "package", package)


async def dependency_staged(tmp_path):
    service, directory, destination, previous, private, identity = await staged(tmp_path)
    _, root, update = updates.transaction(directory, identity)
    assert update["runtime_dependency_update"] is not None
    assert not update["blockers"]
    definition = json.loads((destination / "deployment.json").read_text())
    storage_setup.write_private(destination / ".mobkit/launch-storage.json",
        {"format": 1, "project": definition["slug"], "version": "0.8.42", "phase": "ready"})
    (destination / ".bin").mkdir()
    (destination / ".bin/mobkit_gateway").write_bytes(b"retained .42 native executable")
    return service, directory, destination, previous, private, identity


@pytest.fixture
def registry_fixture_packages(monkeypatch, canonical_fixture_packages):
    original = Guide.__init__
    def with_current_model(guide):
        original(guide)
        for agent in guide.state["proposal"]["agent_classes"]:
            agent.update(provider="openai", model="gpt-6.1-sol")
        guide.legacy_registry_host = True
    monkeypatch.setattr(Guide, "__init__", with_current_model)


def mock_dependency_runtime(monkeypatch, directory, destination, *, failure=None, registry_epoch=None, target_version=None):
    from_version = "0.8.42"
    if target_version is None:
        target_version = read_receipt(directory)["update"]["runtime_dependency_update"]["to_version"]
    events = []
    native_checks = 0
    try:
        module = importlib.import_module("launchkit.runtime_dependencies")
    except ModuleNotFoundError:
        module = ModuleType("launchkit.runtime_dependencies")
        monkeypatch.setitem(sys.modules, module.__name__, module)

    def protected_phase(name, *, leased=True):
        events.append(name)
        receipt = read_receipt(directory)
        assert receipt["update"]["runtime_admission_possible"] is True
        assert receipt["build_id"] == receipt["update"]["to_build_id"]
        if leased:
            with pytest.raises(RuntimeError, match="already"):
                with storage_setup.project_lock(destination):
                    pytest.fail("dependency preparation lost the stopped project lease")
        if name == failure:
            raise RuntimeError(f"{name} failed")

    def retain(root, transaction):
        assert root == destination and transaction.is_dir()
        protected_phase("retain")
        return {"format": "mobkit-launch-runtime-retention-v1", "retained": True,
            "destination": str(root), "backup": str(transaction / "runtime-before"),
            "inventory_sha256": "3" * 64, "previous_native": native_fixture_proof("0.8.42")}

    def verify(root, version):
        nonlocal native_checks
        assert root == destination and version == target_version
        native_checks += 1
        protected_phase("native" if native_checks == 1 else "native-final", leased=native_checks == 1)
        proof = native_fixture_proof(version)
        if native_checks > 1 and failure == "native-changed":
            proof["binaries"]["rpc_gateway"]["binary_sha256"] = "7" * 64
        return proof

    def upgrade(root, from_version, to_version, *, require_owner_current=None):
        assert root == destination and (from_version, to_version) == ("0.8.42", target_version)
        if require_owner_current is not None:
            require_owner_current()
        protected_phase("storage")
        marker = json.loads((root / ".mobkit/launch-storage.json").read_text())
        assert marker["version"] == from_version and marker["phase"] == "ready"
        storage_setup.write_private(root / ".mobkit/launch-storage.json", {**marker, "version": to_version})
        return {"from_version": from_version, "to_version": to_version, "applied": True,
                "read_report": "retained-native-read.json", "apply_report": "retained-native-apply.json"}

    monkeypatch.setattr(module, "retain_runtime", retain, raising=False)
    monkeypatch.setattr(module, "verify_runtime", verify, raising=False)
    monkeypatch.setattr(storage_setup, "upgrade_storage", upgrade)

    class MockRuntime(Supervisor):
        async def native_definition_revision(self, files, *, require_carrier=True, expected_revision=None):
            if registry_epoch is None:
                pytest.fail("unchanged native definition must not be inspected or activated by dependency update")
            definition = json.loads(files["deployment.json"][0])
            current = read_receipt(directory)
            version = definition["mobkitVersion"]
            if version == "0.8.42":
                # Observation precedes every source/runtime replacement. The
                # old host cannot author continuing instructions at this seam.
                assert not require_carrier and expected_revision is None and registry_epoch["revision"] == 1
                assert json.loads((destination / "deployment.json").read_text())["mobkitVersion"] == version
                assert current["package_sha256"] == current["update"]["previous_package_sha256"]
                assert current["update"]["runtime_admission_possible"] is True
                events.append("observe-old")
            else:
                assert version == target_version and not require_carrier and expected_revision == 2
                assert registry_epoch["revision"] == 2
                assert events[-1] == "activation"
                events.append("observe-current")
            revision = 3 if failure == "wrong-epoch" and version == target_version else registry_epoch["revision"]
            return {"mob_id": definition["slug"], "definition_revision": revision,
                "package_sha256": current["package_sha256"], "native_binary_version": version,
                "native_binary_source": "official-release",
                "native_binary_sha256": "4" * 64 if version == "0.8.42" else "6" * 64,
                "host_system_prompt": {"send": version == target_version, "dispatch": version == target_version}}

        async def command(self, args, phase, *other, **kwargs):
            assert "--send" not in args and "--dispatch" not in args
            if "--activate-revision" in args:
                assert registry_epoch is not None
                assert args == [str(destination / ".venv/bin/python"), "app.py", "--activate-revision", "1"]
                assert events[-1] == "native-final"
                assert json.loads((destination / ".mobkit/launch-storage.json").read_text())["version"] == target_version
                # Replaying the original CAS pin converges an already applied
                # successor, including after loss of its activation response.
                assert registry_epoch["revision"] in {1, 2}
                registry_epoch["revision"] = 2
                protected_phase("activation", leased=False)
                if failure == "response-loss":
                    raise RuntimeError("activation response lost")
                return ""
            name = ("install" if args[:2] == ["/bin/bash", "install.sh"] else "sdk" if "-c" in args
                    else "doctor" if args[-1] == "--doctor" else "application-check" if args[-1] == "--application-check" else None)
            assert name is not None
            if name == "install":
                assert args == ["/bin/bash", "install.sh", "--non-interactive"] and kwargs["installer"] is True
                with (destination / ".mobkit/state/native.db").open("ab") as stream:
                    stream.write(b"; current runtime observation")
                with (destination / "data/ledger.db").open("ab") as stream:
                    stream.write(b"; current application record")
                if failure == "cancel":
                    events.append(name)
                    self.stop.set()
                    raise asyncio.CancelledError
            protected_phase(name)
            if name == "application-check" and failure == "override-late":
                with (destination / ".env").open("a") as stream:
                    stream.write('MOBKIT_RPC_GATEWAY_BIN="/foreign/native"\n')
            if name == "sdk":
                return json.dumps({"package": "meerkat-mobkit", "version": "0.8.42" if failure == "wrong-sdk" else target_version})

        async def start(self, definition, *, on_ready):
            events.append("start")
            assert definition["mobkitVersion"] == target_version
            assert events[-4:] == (["native-final", "activation", "observe-current", "start"]
                if registry_epoch is not None else ["doctor", "application-check", "native-final", "start"])
            assert json.loads((destination / ".mobkit/launch-storage.json").read_text())["version"] == target_version
            if failure == "start":
                raise RuntimeError("start failed")
            on_ready()

    return MockRuntime(directory), events


def native_fixture_proof(version):
    """Inert mock receipt shape matching the owned runtime verifier contract."""
    target = "aarch64-apple-darwin"
    release = f"https://github.com/lukacf/meerkat-mobkit/releases/download/v{version}"
    binaries = {name: {"receipt_sha256": "1" * 64, "archive_sha256": "2" * 64,
        "binary_sha256": "4" * 64 if version == "0.8.42" else "6" * 64,
        "archive": f"{prefix}-{version}-{target}.tar.gz", "release_url": release}
        for name, prefix in (("rpc_gateway", "mobkit-rpc-gateway"), ("mobkit_gateway", "mobkit-gateway"))}
    result = {"version": version, "source": "official-release", "platform": target, "binaries": binaries}
    if version in {"0.8.46", "0.8.47"}:
        result["core_version"] = "0.8.51"
    return result


async def test_worker_checks_install_sdk_native_and_storage_before_one_start(tmp_path, monkeypatch, canonical_fixture_packages):
    _, directory, destination, previous, private, identity = await dependency_staged(tmp_path)
    supervisor, events = mock_dependency_runtime(monkeypatch, directory, destination)
    await update_runtime(supervisor, "update", update_id=identity)
    assert events == ["retain", "install", "native", "sdk", "storage", "doctor", "application-check", "native-final", "start"]
    receipt = read_receipt(directory)
    assert receipt["update"]["status"] == "completed" and receipt["update"]["rollback_available"] is False
    assert receipt["package_sha256"] != previous["package_sha256"]
    assert receipt["update"]["runtime_storage_upgrade"]["applied"] is True
    assert receipt["update"].get("native_definition_activation") is None
    assert (destination / ".env").read_bytes() == private
    assert (destination / ".mobkit/state/native.db").read_bytes() == b"native durable sessions; current runtime observation"


async def test_registry_crossing_observes_old_epoch_then_qualifies_current_pair_before_cas(tmp_path, monkeypatch, registry_fixture_packages):
    _, directory, destination, previous, private, identity = await dependency_staged(tmp_path)
    _, _, update = updates.transaction(directory, identity)
    assert update["native_definition_update"] is True
    # The original .46 host is not today's exact approved instruction carrier.
    # Current .47 coverage retains the original positive carrier assertion.
    assert update["host_instruction_update"] is (generator.MOBKIT_VERSION == "0.8.47")
    assert update["runtime_dependency_update"]["native_registry_update"] is True
    epoch = {"revision": 1}
    supervisor, events = mock_dependency_runtime(monkeypatch, directory, destination, registry_epoch=epoch)
    await update_runtime(supervisor, "update", update_id=identity)
    assert events == ["observe-old", "retain", "install", "native", "sdk", "storage", "doctor",
                      "application-check", "native-final", "activation", "observe-current", "start"]
    receipt = read_receipt(directory)
    assert receipt["update"]["status"] == "completed" and receipt["update"]["rollback_available"] is False
    observation = receipt["update"]["native_definition_observation"]
    assert observation["package_sha256"] == previous["package_sha256"]
    assert observation["definition_revision"] == 1 and observation["native_binary_version"] == "0.8.42"
    activation = receipt["update"]["native_definition_activation"]
    assert activation["definition_revision"] == 2 and activation["expected_revision"] == 1
    assert activation["package_sha256"] == receipt["package_sha256"]
    assert (destination / ".env").read_bytes() == private
    assert (destination / ".mobkit/state/native.db").read_bytes() == b"native durable sessions; current runtime observation"


async def test_registry_cas_response_loss_recovers_original_epoch_without_repeating_storage(tmp_path, monkeypatch, registry_fixture_packages):
    _, directory, destination, _, private, identity = await dependency_staged(tmp_path)
    epoch = {"revision": 1}
    failed, first = mock_dependency_runtime(monkeypatch, directory, destination,
        registry_epoch=epoch, failure="response-loss")
    with pytest.raises(RuntimeError, match="response lost"):
        await update_runtime(failed, "update", update_id=identity)
    assert epoch["revision"] == 2 and first[-1] == "activation" and "start" not in first
    assert read_receipt(directory)["update"]["status"] == "recovery-needed"
    recovering, second = mock_dependency_runtime(monkeypatch, directory, destination, registry_epoch=epoch)
    await update_runtime(recovering, "recover-current", update_id=identity)
    assert second == ["retain", "install", "native", "sdk", "doctor", "application-check", "native-final",
                      "activation", "observe-current", "start"]
    assert first.count("storage") == 1 and "storage" not in second and "observe-old" not in second
    receipt = read_receipt(directory)
    assert receipt["update"]["status"] == "completed" and receipt["update"]["rollback_available"] is False
    assert receipt["update"]["native_activation_expected_revision"] == 1
    assert receipt["update"]["native_definition_activation"]["definition_revision"] == 2
    assert (destination / ".env").read_bytes() == private
    assert (destination / ".mobkit/state/native.db").read_bytes().count(b"current runtime observation") == 2
    assert (destination / "data/ledger.db").read_bytes().count(b"current application record") == 2


@pytest.mark.parametrize("mutation", ["missing", "boolean-epoch", "changed-pin", "foreign-mob",
    "foreign-package", "foreign-version", "invalid-native-hash", "foreign-target-pin", "changed-activation-observation"])
async def test_registry_recovery_refuses_unbound_original_epoch_before_runtime_activity(tmp_path, monkeypatch, registry_fixture_packages, mutation):
    _, directory, destination, _, private, identity = await dependency_staged(tmp_path)
    epoch = {"revision": 1}
    failed, _ = mock_dependency_runtime(monkeypatch, directory, destination,
        registry_epoch=epoch, failure="response-loss")
    with pytest.raises(RuntimeError, match="response lost"):
        await update_runtime(failed, "update", update_id=identity)
    _, root, update = updates.transaction(directory, identity)
    observation = copy.deepcopy(update["native_definition_observation"])
    changed = {"native_definition_observation": observation}
    if mutation == "missing":
        changed["native_definition_observation"] = None
    elif mutation == "boolean-epoch":
        observation["definition_revision"] = True
    elif mutation == "changed-pin":
        changed["native_activation_expected_revision"] = 2
    elif mutation == "foreign-mob":
        observation["mob_id"] = "other-application"
    elif mutation == "foreign-package":
        observation["package_sha256"] = "f" * 64
    elif mutation == "foreign-version":
        observation["native_binary_version"] = "0.8.46"
    elif mutation == "invalid-native-hash":
        observation["native_binary_sha256"] = "unknown"
    elif mutation == "foreign-target-pin":
        changed["native_activation_package_sha256"] = update["previous_package_sha256"]
    else:
        changed["native_activation_observation"] = {**observation, "definition_revision": 2}
    updates.checkpoint(directory, root, update, **changed)
    recovering, events = mock_dependency_runtime(monkeypatch, directory, destination, registry_epoch=epoch)
    with pytest.raises(RuntimeError, match="revision|pin|another application"):
        await update_runtime(recovering, "recover-current", update_id=identity)
    assert not events and epoch["revision"] == 2
    assert read_receipt(directory)["update"]["status"] == "recovery-needed"
    assert (destination / ".env").read_bytes() == private


async def test_registry_activation_refuses_a_later_unrelated_epoch_before_start(tmp_path, monkeypatch, registry_fixture_packages):
    _, directory, destination, _, private, identity = await dependency_staged(tmp_path)
    supervisor, events = mock_dependency_runtime(monkeypatch, directory, destination,
        registry_epoch={"revision": 1}, failure="wrong-epoch")
    with pytest.raises(RuntimeError, match="exact successor revision"):
        await update_runtime(supervisor, "update", update_id=identity)
    assert events[-1] == "observe-current" and "start" not in events
    receipt = read_receipt(directory)
    assert receipt["update"]["status"] == "recovery-needed" and receipt["update"]["rollback_available"] is False
    assert receipt["update"].get("native_definition_activation") is None
    assert (destination / ".env").read_bytes() == private


@pytest.mark.parametrize("failure", ["install", "native", "sdk", "wrong-sdk", "storage", "doctor", "application-check", "native-final", "native-changed", "start", "cancel"])
async def test_dependency_failure_or_owner_cancel_never_falls_back_or_rewinds_current_work(tmp_path, monkeypatch, canonical_fixture_packages, failure):
    _, directory, destination, previous, private, identity = await dependency_staged(tmp_path)
    supervisor, events = mock_dependency_runtime(monkeypatch, directory, destination, failure=failure)
    with pytest.raises(asyncio.CancelledError if failure == "cancel" else RuntimeError):
        await update_runtime(supervisor, "update", update_id=identity)
    receipt = read_receipt(directory)
    assert receipt["update"]["status"] == "recovery-needed" and receipt["update"]["rollback_available"] is False
    assert receipt["update"]["runtime_admission_possible"] is True
    assert receipt["build_id"] == receipt["update"]["to_build_id"] != previous["build_id"]
    assert receipt["package_sha256"] == receipt["update"]["package_sha256"]
    assert (destination / "application/logic.py").read_text() == "VERSION = 'new'\n"
    assert (destination / ".env").read_bytes() == private
    assert (destination / ".mobkit/state/native.db").read_bytes().endswith(b"; current runtime observation")
    assert (destination / "data/ledger.db").read_bytes().endswith(b"; current application record")
    assert events.count("start") == (1 if failure == "start" else 0)
    assert not any("rollback" in name or "activate" in name for name in events)


async def test_forward_recovery_with_ready_new_marker_does_not_repeat_native_apply(tmp_path, monkeypatch, canonical_fixture_packages):
    _, directory, destination, _, private, identity = await dependency_staged(tmp_path)
    failed, first = mock_dependency_runtime(monkeypatch, directory, destination, failure="start")
    with pytest.raises(RuntimeError, match="start failed"):
        await update_runtime(failed, "update", update_id=identity)
    assert first.count("storage") == 1
    assert json.loads((destination / ".mobkit/launch-storage.json").read_text())["version"] == generator.MOBKIT_VERSION
    ready, second = mock_dependency_runtime(monkeypatch, directory, destination)
    await update_runtime(ready, "recover-current", update_id=identity)
    assert second == ["retain", "install", "native", "sdk", "doctor", "application-check", "native-final", "start"]
    receipt = read_receipt(directory)
    assert receipt["update"]["status"] == "completed" and receipt["update"]["rollback_available"] is False
    assert receipt["update"]["runtime_storage_upgrade"]["applied"] is True
    assert (destination / ".env").read_bytes() == private
    assert (destination / ".mobkit/state/native.db").read_bytes().count(b"current runtime observation") == 2
    assert (destination / "data/ledger.db").read_bytes().count(b"current application record") == 2


async def test_forward_recovery_rejects_boolean_ready_marker_format_before_start(tmp_path, monkeypatch, canonical_fixture_packages):
    _, directory, destination, _, private, identity = await dependency_staged(tmp_path)
    failed, first = mock_dependency_runtime(monkeypatch, directory, destination, failure="start")
    with pytest.raises(RuntimeError, match="start failed"):
        await update_runtime(failed, "update", update_id=identity)
    assert first.count("storage") == 1
    marker_path = destination / ".mobkit/launch-storage.json"
    marker = json.loads(marker_path.read_text())
    assert marker["version"] == generator.MOBKIT_VERSION and type(marker["format"]) is int
    storage_setup.write_private(marker_path, {**marker, "format": True})
    invalid_marker = marker_path.read_bytes()
    recovering, events = mock_dependency_runtime(monkeypatch, directory, destination)
    with pytest.raises(RuntimeError, match="storage marker"):
        await update_runtime(recovering, "recover-current", update_id=identity)
    assert events == ["retain", "install", "native", "sdk"]
    receipt = read_receipt(directory)
    assert receipt["update"]["status"] == "recovery-needed" and receipt["update"]["rollback_available"] is False
    assert receipt["build_id"] == receipt["update"]["to_build_id"]
    assert receipt["update"]["runtime_storage_upgrade"]["applied"] is True
    assert marker_path.read_bytes() == invalid_marker
    assert (destination / ".env").read_bytes() == private
    assert (destination / ".mobkit/state/native.db").read_bytes().count(b"current runtime observation") == 2
    assert (destination / "data/ledger.db").read_bytes().count(b"current application record") == 2


async def test_prepared_old_package_has_a_supported_transition_without_installation(tmp_path, canonical_fixture_packages):
    service, directory, destination, previous, private = await prepared(tmp_path)
    _, old = updates.checked_package(directory, "package.zip", previous["package_sha256"])
    payload, _ = await service.implementation.package(service.guide.identity, service.implementation.identity)
    candidate = updates.package_files(payload)
    assert updates.runtime_dependency_transition(old, candidate, previous) is not None
    assert (destination / ".env").read_bytes() == private
    assert json.loads((destination / "deployment.json").read_text())["mobkitVersion"] == "0.8.42"


async def test_completed_dependency_transition_refuses_explicit_old_runtime_rollback(tmp_path, monkeypatch, canonical_fixture_packages):
    _, directory, destination, _, _, identity = await dependency_staged(tmp_path)
    ready, _ = mock_dependency_runtime(monkeypatch, directory, destination)
    await update_runtime(ready, "update", update_id=identity)
    accepted = read_receipt(directory)["package_sha256"]
    forbidden, events = mock_dependency_runtime(monkeypatch, directory, destination)
    with pytest.raises(DraftConflict, match="downgrade"):
        await update_runtime(forbidden, "rollback", update_id=identity)
    assert not events and read_receipt(directory)["package_sha256"] == accepted
    assert json.loads((destination / "deployment.json").read_text())["mobkitVersion"] == generator.MOBKIT_VERSION


@pytest.mark.parametrize("late", [False, True])
async def test_owned_settings_native_override_cannot_enter_qualified_dependency_runtime(tmp_path, monkeypatch, canonical_fixture_packages, late):
    _, directory, destination, _, _, identity = await dependency_staged(tmp_path)
    if not late:
        with (destination / ".env").open("a") as stream:
            stream.write('MOBKIT_RPC_GATEWAY_BIN="/foreign/native"\n')
    supervisor, events = mock_dependency_runtime(monkeypatch, directory, destination,
                                                failure="override-late" if late else None)
    with pytest.raises(RuntimeError, match="native gateway"):
        await update_runtime(supervisor, "update", update_id=identity)
    assert "start" not in events
    if not late:
        assert not events
    else:
        assert events[-1] == "application-check"
    assert 'MOBKIT_RPC_GATEWAY_BIN="/foreign/native"' in (destination / ".env").read_text()
    assert read_receipt(directory)["update"]["status"] == "recovery-needed"
