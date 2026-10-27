"""Exact legacy registry admission, native observation and forward recovery."""
import copy
import json
import tomllib
from types import SimpleNamespace

import pytest

from launchkit import deployment_updates as updates
from launchkit.blueprint import starter_blueprint
from launchkit.deployment import read_receipt
from launchkit.deployment_worker import NATIVE_DEFINITION_READ, Supervisor, update_runtime
from launchkit.native_models import legacy_registry_successor, supplemental_model_metadata
from test_runtime_dependency_updates import (canonical_fixture_packages,
    dependency_packages, dependency_staged, mock_dependency_runtime,
    registry_fixture_packages)


@pytest.fixture(autouse=True)
def registry_source_contract(monkeypatch, request):
    contract = getattr(request, "param", "historical-v1-046")
    if contract == "current-v4-047":
        from launchkit import compiler, generator
        assert compiler.MOBKIT_VERSION == generator.MOBKIT_VERSION == "0.8.47"
        yield
        return
    assert contract == "historical-v1-046"
    # Emit the original v1 compiler and templates, not today's wider source
    # inventory relabeled as .46. These packages and native responses are inert.
    from historical_runtime_fixture import qualified_runtime_compiler
    original = qualified_runtime_compiler.__wrapped__(monkeypatch,
        SimpleNamespace(param="historical-v1-046"))
    yield from original


def selected_blueprint():
    value = starter_blueprint().model_dump(mode="json")
    for agent in value["agent_classes"]:
        agent.update(provider="openai", model="gpt-6.1-sol")
    return value


def native(files):
    return tomllib.loads(files["config/mob.toml"][0].decode())


def replace_section(files, heading, transform):
    content, mode = files["config/mob.toml"]
    text = content.decode()
    start = text.index(heading)
    end = text.find("\n[", start + len(heading))
    if end == -1:
        end = len(text)
    files["config/mob.toml"] = (
        (text[:start] + transform(text[start:end]) + text[end:]).encode(), mode)


def old_host_without_current_carrier(files):
    files["app.py"] = (b'"""Earlier accepted protected host without the current instruction carrier."""\n', 0o644)
    baseline = {name: {"sha256": updates.digest(content), "mode": mode}
                for name, (content, mode) in files.items()
                if name != "implementation-report.json"}
    fingerprint = updates.digest(json.dumps(baseline, ensure_ascii=True,
        sort_keys=True, separators=(",", ":")).encode())
    report = json.loads(files["implementation-report.json"][0])
    report["framework"].update(basis="accepted-baseline", reason="unchanged-specification",
        baseline_digest=fingerprint, current_compiler_baseline_digest=fingerprint,
        retained_template_paths=[])
    files["implementation-report.json"] = (json.dumps(report).encode(), 0o644)
    return fingerprint


def test_exact_legacy_registry_removal_keeps_all_selected_profiles_and_authority(monkeypatch):
    previous, candidate = dependency_packages(monkeypatch, selected_blueprint())
    old_native, new_native = native(previous), native(candidate)
    legacy = supplemental_model_metadata(native_version="0.8.42", core_version=None)
    assert old_native["models"] == {"gpt-6.1-sol": legacy[0]["metadata"]}
    assert new_native.get("models", {}) == {}
    assert old_native["profiles"] == new_native["profiles"]
    for profile in new_native["profiles"].values():
        assert (profile["model"], profile["provider"]) == ("gpt-6.1-sol", "openai")
    successor = legacy_registry_successor(old_native)
    assert successor == {**new_native, "models": {}}
    originals = copy.deepcopy((previous, candidate))
    descriptor = updates.runtime_dependency_transition(previous, candidate, {})
    assert descriptor is not None
    assert (descriptor["from_version"], descriptor["to_version"], descriptor["core_version"]) == (
        "0.8.42", "0.8.46", "0.8.51")
    assert descriptor["definition_unchanged"] is False
    assert descriptor["native_registry_update"] is True
    assert previous["blueprint.json"] == candidate["blueprint.json"]
    assert (previous, candidate) == originals


@pytest.mark.parametrize("registry_source_contract", ["current-v4-047"], indirect=True)
def test_registry_removal_accepts_provenanced_old_host_without_current_carrier(monkeypatch):
    previous, candidate = dependency_packages(monkeypatch, selected_blueprint())
    fingerprint = old_host_without_current_carrier(previous)
    assert not updates.has_host_instruction_carrier(previous)
    assert updates.has_host_instruction_carrier(candidate)
    report = json.loads(previous["implementation-report.json"][0])
    assert report["framework"]["baseline_digest"] == fingerprint
    descriptor = updates.runtime_dependency_transition(previous, candidate, {})
    assert descriptor is not None and descriptor["native_registry_update"] is True
    assert (descriptor["version"], descriptor["to_version"]) == (4, "0.8.47")
    assert set(descriptor["required_supervisor_sources"]) == set(updates._runtime_supervisor_paths(4))
    assert descriptor["required_supervisor_sources"] == updates._runtime_history_supervisor_sources(4)


@pytest.mark.parametrize("mutation", ["instructions", "model", "grant", "unknown"])
def test_registry_removal_never_masks_another_application_authority_change(monkeypatch, mutation):
    before = selected_blueprint()
    after = copy.deepcopy(before)
    if mutation == "instructions":
        after["agent_classes"][0]["instructions"] += "\nA different operational mandate."
    elif mutation == "model":
        after["agent_classes"][0]["model"] = "gpt-6-luna"
    elif mutation == "grant":
        after["agent_classes"][0]["tools"]["shell"] = True
    previous, candidate = dependency_packages(monkeypatch, before, after)
    if mutation == "unknown":
        deployment = json.loads(candidate["deployment.json"][0])
        deployment["unknown_authority"] = True
        candidate["deployment.json"] = (json.dumps(deployment).encode(), 0o644)
    assert updates.runtime_dependency_transition(previous, candidate, {}) is None


@pytest.mark.parametrize("mutation", ["provider", "timeout", "boolean-type", "float-type", "unknown-field", "foreign-model"])
def test_registry_removal_refuses_foreign_or_changed_old_model_metadata(monkeypatch, mutation):
    previous, candidate = dependency_packages(monkeypatch, selected_blueprint())
    replacements = {
        "provider": ('provider = "openai"', 'provider = "anthropic"'),
        "timeout": ("call_timeout_secs = 600", "call_timeout_secs = 601"),
        "boolean-type": ("vision = true", "vision = 1"),
        "float-type": ("context_window = 1050000", "context_window = 1050000.0"),
    }
    if mutation in replacements:
        before, after = replacements[mutation]
        replace_section(previous, '[models."gpt-6.1-sol"]',
            lambda section: section.replace(before, after))
    elif mutation == "unknown-field":
        replace_section(previous, '[models."gpt-6.1-sol"]',
            lambda section: section + "\nunknown_metadata = true\n")
    else:
        content, mode = previous["config/mob.toml"]
        previous["config/mob.toml"] = (
            content + b'\n[models."foreign-model"]\nprovider = "openai"\n', mode)
    assert updates.runtime_dependency_transition(previous, candidate, {}) is None


def test_registry_removal_cannot_drop_existing_profile_tuning(monkeypatch):
    previous, candidate = dependency_packages(monkeypatch, selected_blueprint())
    profile = next(iter(native(previous)["profiles"]))
    replace_section(previous, f"[profiles.{profile}]", lambda section: section +
        '\nprovider_params = { provider_tag = { provider = "open_ai", reasoning_effort = "high", supports_reasoning_override = true } }\n')
    assert native(previous)["profiles"][profile]["provider_params"]["provider_tag"]["reasoning_effort"] == "high"
    assert "provider_params" not in native(candidate)["profiles"][profile]
    assert updates.runtime_dependency_transition(previous, candidate, {}) is None


@pytest.mark.parametrize("mutation", ["missing", "foreign-blueprint"])
def test_registry_removal_cannot_upgrade_unprovenanced_old_host(monkeypatch, mutation):
    previous, candidate = dependency_packages(monkeypatch, selected_blueprint())
    old_host_without_current_carrier(previous)
    if mutation == "missing":
        del previous["implementation-report.json"]
    else:
        report = json.loads(previous["implementation-report.json"][0])
        report["framework"]["blueprint_digest"] = "f" * 64
        previous["implementation-report.json"] = (json.dumps(report).encode(), 0o644)
    assert updates.runtime_dependency_transition(previous, candidate, {}) is None


async def native_observer(tmp_path, *, current, revision):
    """Read through the production parser with owned fixture source and fake I/O."""
    _, directory, _, previous, _, identity = await dependency_staged(tmp_path)
    files = (updates.activate(directory, identity) if current else
        updates.checked_package(directory, "package.zip", previous["package_sha256"])[1])
    definition = json.loads(files["deployment.json"][0])
    commands = []

    class Observation(Supervisor):
        def native_binary_provenance(self, checked_files):
            assert checked_files == files
            return {"native_binary_version": definition["mobkitVersion"],
                "native_binary_sha256": "6" * 64 if current else "4" * 64}

        async def command(self, args, phase, *other, **kwargs):
            assert args[-1] == NATIVE_DEFINITION_READ and kwargs["capture_output"] is True
            commands.append(phase)
            return "\n".join(json.dumps({"mob_id": definition["slug"], **record}) for record in [
                {"source": "native structural event ledger", "definition_revision": revision},
                {"source": "native runtime capabilities",
                 "host_system_prompt": {"send": False, "dispatch": False}}])

    return Observation(directory), files, commands


@pytest.mark.parametrize("revision", [1, 2, 3])
async def test_registry_successor_false_carrier_needs_exact_observed_revision(
        tmp_path, registry_fixture_packages, revision):
    observer, files, commands = await native_observer(tmp_path, current=True, revision=revision)
    if revision != 2:
        with pytest.raises(RuntimeError, match="exact expected revision"):
            await observer.native_definition_revision(files, require_carrier=False, expected_revision=2)
    else:
        result = await observer.native_definition_revision(files, require_carrier=False, expected_revision=2)
        assert result["definition_revision"] == 2 and result["native_binary_version"] == "0.8.46"
        assert result["host_system_prompt"] == {"send": False, "dispatch": False}
        assert result["package_sha256"] == read_receipt(observer.directory)["package_sha256"]
    assert commands == ["Native definition revision"]


@pytest.mark.parametrize("revision", [1, 2])
async def test_legacy_false_carrier_without_successor_pin_remains_initial_epoch_only(
        tmp_path, registry_fixture_packages, revision):
    observer, files, commands = await native_observer(tmp_path, current=False, revision=revision)
    if revision != 1:
        with pytest.raises(RuntimeError, match="cannot author current role instructions"):
            await observer.native_definition_revision(files, require_carrier=False)
    else:
        result = await observer.native_definition_revision(files, require_carrier=False)
        assert result["definition_revision"] == 1 and result["native_binary_version"] == "0.8.42"
    assert commands == ["Native definition revision"]


@pytest.mark.parametrize("expected_revision", [None, 2])
async def test_instruction_observation_stays_strict_even_with_exact_successor_pin(
        tmp_path, registry_fixture_packages, expected_revision):
    observer, files, _ = await native_observer(tmp_path, current=True, revision=2)
    with pytest.raises(RuntimeError, match="cannot author current role instructions"):
        await observer.native_definition_revision(files, expected_revision=expected_revision)


@pytest.mark.parametrize("expected_revision", [True, 2.0, "2", 0])
async def test_registry_observation_refuses_untyped_or_nonpositive_pin_before_native_read(
        tmp_path, registry_fixture_packages, expected_revision):
    observer, files, commands = await native_observer(tmp_path, current=True, revision=2)
    with pytest.raises(RuntimeError, match="exact positive revision"):
        await observer.native_definition_revision(files, require_carrier=False,
            expected_revision=expected_revision)
    assert not commands


def false_carrier_worker(monkeypatch, directory, destination, epoch, *, failure=None):
    """Reuse qualified update fixtures while exercising the actual native reader."""
    supervisor, events = mock_dependency_runtime(monkeypatch, directory, destination,
        registry_epoch=epoch, failure="response-loss" if failure == "response-loss" else None)
    original_command = supervisor.command
    commands, observations = [], []

    def provenance(files):
        version = json.loads(files["deployment.json"][0])["mobkitVersion"]
        return {"native_binary_version": version, "native_binary_source": "official-release",
            "native_binary_sha256": "4" * 64 if version == "0.8.42" else "6" * 64}

    async def command(args, phase, *other, **kwargs):
        assert "--send" not in args and "--dispatch" not in args
        commands.append(tuple(args))
        if phase != "Native definition revision":
            return await original_command(args, phase, *other, **kwargs)
        assert args[-1] == NATIVE_DEFINITION_READ and kwargs["capture_output"] is True
        definition = json.loads((destination / "deployment.json").read_text())
        events.append("observe-old" if definition["mobkitVersion"] == "0.8.42" else "observe-current")
        if failure == "observation-loss" and definition["mobkitVersion"] == "0.8.46":
            raise RuntimeError("native observation interrupted after CAS")
        return "\n".join(json.dumps({"mob_id": definition["slug"], **record}) for record in [
            {"source": "native structural event ledger", "definition_revision": epoch["revision"]},
            {"source": "native runtime capabilities",
             "host_system_prompt": {"send": False, "dispatch": False}}])

    async def observe(files, *, require_carrier=True, expected_revision=None):
        version = json.loads(files["deployment.json"][0])["mobkitVersion"]
        observations.append((version, require_carrier, expected_revision))
        return await Supervisor.native_definition_revision(supervisor, files,
            require_carrier=require_carrier, expected_revision=expected_revision)

    monkeypatch.setattr(supervisor, "native_binary_provenance", provenance)
    monkeypatch.setattr(supervisor, "command", command)
    monkeypatch.setattr(supervisor, "native_definition_revision", observe)
    return supervisor, events, commands, observations


async def test_registry_only_worker_observes_false_carrier_successor_after_one_original_cas(
        tmp_path, monkeypatch, registry_fixture_packages):
    _, directory, destination, previous, private, identity = await dependency_staged(tmp_path)
    epoch = {"revision": 1}
    supervisor, events, commands, observations = false_carrier_worker(
        monkeypatch, directory, destination, epoch)
    await update_runtime(supervisor, "update", update_id=identity)
    assert observations == [("0.8.42", False, None), ("0.8.46", False, 2)]
    assert events[-4:] == ["native-final", "activation", "observe-current", "start"]
    activations = [args for args in commands if "--activate-revision" in args]
    assert activations == [(str(destination / ".venv/bin/python"), "app.py", "--activate-revision", "1")]
    receipt = read_receipt(directory)
    activation = receipt["update"]["native_definition_activation"]
    assert receipt["update"]["status"] == "completed" and epoch["revision"] == 2
    assert activation["definition_revision"] == 2 and activation["expected_revision"] == 1
    assert activation["host_system_prompt"] == {"send": False, "dispatch": False}
    assert activation["package_sha256"] == receipt["package_sha256"] != previous["package_sha256"]
    assert (destination / ".env").read_bytes() == private


@pytest.mark.parametrize("failure", ["response-loss", "observation-loss"])
async def test_false_carrier_registry_recovery_converges_original_pin_without_repeating_storage(
        tmp_path, monkeypatch, registry_fixture_packages, failure):
    _, directory, destination, _, private, identity = await dependency_staged(tmp_path)
    epoch = {"revision": 1}
    failed, first, first_commands, _ = false_carrier_worker(
        monkeypatch, directory, destination, epoch, failure=failure)
    with pytest.raises(RuntimeError, match="response lost|interrupted after CAS"):
        await update_runtime(failed, "update", update_id=identity)
    retained = read_receipt(directory)
    assert epoch["revision"] == 2 and "start" not in first
    assert retained["update"]["status"] == "recovery-needed"
    assert retained["update"]["native_activation_expected_revision"] == 1
    original_observation = retained["update"]["native_definition_observation"]
    assert original_observation["definition_revision"] == 1
    assert original_observation["host_system_prompt"] == {"send": False, "dispatch": False}
    assert json.loads((destination / ".mobkit/launch-storage.json").read_text())["version"] == "0.8.46"

    recovering, second, second_commands, observations = false_carrier_worker(
        monkeypatch, directory, destination, epoch)
    await update_runtime(recovering, "recover-current", update_id=identity)
    assert observations == [("0.8.46", False, 2)]
    assert "observe-old" not in second and "storage" not in second
    assert first.count("storage") == 1 and second[-4:] == ["native-final", "activation", "observe-current", "start"]
    for calls in (first_commands, second_commands):
        assert [args for args in calls if "--activate-revision" in args] == [
            (str(destination / ".venv/bin/python"), "app.py", "--activate-revision", "1")]
    receipt = read_receipt(directory)
    assert receipt["update"]["status"] == "completed" and receipt["update"]["rollback_available"] is False
    assert receipt["update"]["native_activation_expected_revision"] == 1
    assert receipt["update"]["native_definition_observation"] == original_observation
    assert receipt["update"]["native_definition_activation"]["definition_revision"] == 2
    assert (destination / ".env").read_bytes() == private
