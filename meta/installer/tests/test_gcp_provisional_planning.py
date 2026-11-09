"""Cloud planning can precede a project choice, but package export cannot."""
from copy import deepcopy
import io
import json
import os
import shlex
import subprocess
import zipfile

import pytest
from pydantic import ValidationError

from launchkit.architect import ArchitectService
from launchkit.blueprint import Blueprint, starter_blueprint
from launchkit import compiler


def cloud_plan():
    value = starter_blueprint().model_dump(mode="json")
    value["target"] = "gcp"
    return value


@pytest.mark.parametrize("target", ["mac", "linux"])
@pytest.mark.parametrize("provisioning", ["new_vm", "existing_host"])
@pytest.mark.parametrize("script_name", ["deploy-gcp.sh", "tunnel-gcp.sh"])
def test_local_package_cannot_execute_cloud_operations(
        tmp_path, target, provisioning, script_name):
    plan = starter_blueprint().model_dump(mode="json")
    plan["target"] = target
    # Stale cloud settings cannot select another destination for a local plan.
    plan["gcp"].update(project="forge-cloud-123", provisioning=provisioning)
    if provisioning == "existing_host":
        plan["gcp"].update(existing_instance="retained-host", existing_zone_confirmed=True)
    original = deepcopy(plan)
    files = {item.path: item for item in compiler.generate_package(plan)}
    path = "deploy/" + script_name
    script = files[path]
    assert script.mode == 0o755
    with zipfile.ZipFile(io.BytesIO(compiler.build_archive(plan))) as archive:
        assert archive.read(plan["project"]["slug"] + "/" + path) == script.as_bytes()
    assert plan == original

    directory = tmp_path / "package"
    directory.mkdir()
    executable = directory / script_name
    executable.write_bytes(script.as_bytes())
    retained = directory / "retained-state"
    retained.write_text("original state")
    commands = tmp_path / "bin"
    commands.mkdir()
    cloud_calls = tmp_path / "cloud-calls"
    fake = commands / "gcloud"
    fake.write_text('#!/bin/sh\nprintf called >> "$CLOUD_CALLS"\nexit 87\n')
    fake.chmod(0o755)
    env = {**os.environ, "PATH": str(commands) + ":/usr/bin:/bin",
        "CLOUD_CALLS": str(cloud_calls)}
    before = {str(p.relative_to(directory)): p.read_bytes()
        for p in directory.rglob("*") if p.is_file()}
    for arguments in ([], ["--dry-run"], ["--setup"], ["--resume"]):
        result = subprocess.run(["/bin/bash", str(executable), *arguments],
            cwd=directory, env=env, capture_output=True, text=True, timeout=10)
        assert result.returncode == 1
        assert "explicitly selected GCP package" in result.stderr
        assert result.stdout == ""
        assert not cloud_calls.exists()
        assert {str(p.relative_to(directory)): p.read_bytes()
            for p in directory.rglob("*") if p.is_file()} == before


@pytest.mark.asyncio
@pytest.mark.parametrize("explicit_project", [False, True])
async def test_architect_can_validate_and_propose_cloud_before_project_choice(
        tmp_path, monkeypatch, explicit_project):
    service = ArchitectService(tmp_path / "architect")
    current = starter_blueprint().model_dump(mode="json")
    service.set_context(current)
    plan = cloud_plan()
    if not explicit_project:
        plan["gcp"].pop("project")

    def cannot_compile(*args, **kwargs):
        raise AssertionError("A provisional cloud proposal must not compile files.")

    monkeypatch.setattr(compiler, "generate_package", cannot_compile)
    validated = await service.validate({"blueprint": plan})
    proposed = await service.propose({"blueprint": plan, "summary": "A cloud team with its project still open."})
    for result in (validated, proposed):
        assert result["valid"] is True
        assert result["summary"]["files"] == 0
        assert result["checks"] == [{"label": "Google Cloud project",
            "status": "pending-configuration", "detail": compiler.GCP_PROJECT_PENDING}]
    assert service.context == current
    assert service.proposal["blueprint"]["gcp"]["project"] == ""
    assert service.runtime is None
    assert json.loads((service.root / "proposal.json").read_text())["blueprint"] == service.proposal["blueprint"]


def test_missing_cloud_project_is_valid_plan_with_zero_exported_files(monkeypatch):
    plan = cloud_plan()
    original = deepcopy(plan)
    selected = Blueprint.model_validate(plan)

    def cannot_compile(*args, **kwargs):
        raise AssertionError("Pending package preview must not compile files.")

    monkeypatch.setattr(compiler, "generate_package", cannot_compile)
    preview = compiler.package_preview(selected)
    assert preview["valid"] is True and preview["issues"] == []
    assert preview["files"] == [] and preview["file_count"] == 0
    assert preview["summary"]["files"] == 0
    assert len(preview["resolved"]["identities"]) == len(selected.identities())
    assert preview["package_availability"] == {"available": False,
        "status": "pending-configuration", "reason": compiler.GCP_PROJECT_PENDING}
    assert preview["checks"] == [{"label": "Google Cloud project",
        "status": "pending-configuration", "detail": compiler.GCP_PROJECT_PENDING}]
    assert selected.model_dump(mode="json") == original == plan


@pytest.mark.parametrize("export_name", ["generate_package", "generate_files", "build_archive"])
def test_missing_cloud_project_refuses_export_before_source_or_archive_write(monkeypatch, export_name):
    plan = cloud_plan()

    def no_template_read(*args, **kwargs):
        raise AssertionError("A project is required before reading export templates.")

    def no_archive_write(*args, **kwargs):
        raise AssertionError("A project is required before writing archive members.")

    monkeypatch.setattr(compiler.Path, "read_text", no_template_read)
    monkeypatch.setattr(compiler.Path, "read_bytes", no_template_read)
    monkeypatch.setattr(zipfile.ZipFile, "writestr", no_archive_write)
    with pytest.raises(ValueError, match="Tell the guide your Google Cloud project ID"):
        getattr(compiler, export_name)(plan)


def test_later_exact_project_choice_restores_export_and_pins_destination():
    before = cloud_plan()
    before["gcp"].update(region="us-central1", zone="us-central1-f",
        machine_type="e2-standard-4", disk_gb=120)
    before["advanced"]["port"] = 4310
    after = deepcopy(before)
    after["gcp"]["project"] = "forge-cloud-123"
    assert compiler.package_availability(before)["available"] is False
    selected = Blueprint.model_validate(after)
    assert compiler.package_availability(selected) == {"available": True, "status": "ready", "reason": None}
    files = {item.path: item for item in compiler.generate_package(selected)}
    script = files["deploy/deploy-gcp.sh"]
    assert script.mode == 0o755
    assert {name: shlex.split(script.content.split(name + "=", 1)[1].splitlines()[0])[0]
        for name in ("PROJECT", "ZONE", "INSTANCE", "MACHINE", "PORT")} == {
        "PROJECT": "forge-cloud-123", "ZONE": "us-central1-f", "INSTANCE": selected.project.slug,
        "MACHINE": "e2-standard-4", "PORT": "4310"}
    assert "--boot-disk-size 120GB" in script.content
    assert "__PROJECT__" not in script.content
    assert shlex.split(files["deploy/tunnel-gcp.sh"].content.splitlines()[-1]) == [
        "exec", "gcloud", "compute", "ssh", selected.project.slug,
        "--project", "forge-cloud-123", "--zone", "us-central1-f", "--tunnel-through-iap",
        "--", "-N", "-L", "4310:127.0.0.1:4310"]
    assert json.loads(files["blueprint.json"].content) == after
    assert before["gcp"]["project"] == "" and selected.model_dump(mode="json") == after
    with zipfile.ZipFile(io.BytesIO(compiler.build_archive(selected))) as archive:
        assert archive.read(selected.project.slug + "/deploy/deploy-gcp.sh").decode() == script.content


@pytest.mark.parametrize("project", [" ", "forge", "Forge-cloud-123", "1forge-cloud", "-forge-cloud",
    "forge-cloud-", "forge_cloud", "forge/cloud", "forge.cloud", "\t\n", "a" * 31, None, 123])
def test_only_empty_project_is_deferred_and_malformed_values_still_reject(project):
    plan = cloud_plan()
    plan["gcp"]["project"] = project
    original = deepcopy(plan)
    with pytest.raises(ValidationError):
        Blueprint.model_validate(plan)
    assert plan == original


def test_existing_valid_project_whitespace_normalization_is_unchanged():
    plan = cloud_plan()
    plan["gcp"]["project"] = " forge-cloud-123 \n"
    original = deepcopy(plan)
    selected = Blueprint.model_validate(plan)
    assert selected.gcp.project == "forge-cloud-123"
    assert compiler.package_availability(selected)["available"] is True
    assert plan == original


def test_normalized_cloud_target_cannot_turn_whitespace_project_into_provisional_empty():
    plan = cloud_plan()
    plan.update(target=" gcp ")
    plan["gcp"]["project"] = " \t\n"
    original = deepcopy(plan)
    with pytest.raises(ValidationError, match="Enter a valid Google Cloud project ID"):
        Blueprint.model_validate(plan)
    assert plan == original


def test_guide_recomputes_availability_after_exact_cloud_project_revision():
    from launchkit.guide import guide_package_availability
    from launchkit.projects import fingerprint

    plan = cloud_plan()
    state = {"id": "fixture-cloud-guide", "version": 4, "proposal": plan,
        "package_availability": {"available": True, "status": "ready", "reason": None}}
    pending = guide_package_availability(state)
    assert pending == {"guide_id": state["id"], "guide_version": 4, "source": "proposal",
        "blueprint_hash": fingerprint(plan), "available": False, "status": "pending-configuration",
        "reason": compiler.GCP_PROJECT_PENDING}
    revised = deepcopy(plan)
    revised["gcp"]["project"] = "forge-cloud-123"
    state.update(version=5, proposal=revised, package_availability=pending)
    available = guide_package_availability(state)
    assert available == {"guide_id": state["id"], "guide_version": 5, "source": "proposal",
        "blueprint_hash": fingerprint(revised), "available": True, "status": "ready", "reason": None}
    assert available["blueprint_hash"] != pending["blueprint_hash"]
    assert plan["gcp"]["project"] == ""


@pytest.mark.parametrize("changes,reason", [
    ({"zone": "us-central1-f"}, "Choose a zone inside your selected region"),
    ({"machine_type": "unavailable-vm"}, "machine_type"),
    ({"disk_gb": 29}, "disk_gb"),
    ({"disk_gb": True}, "disk_gb"),
])
def test_missing_project_does_not_relax_other_cloud_constraints(changes, reason):
    plan = cloud_plan()
    plan["gcp"].update(changes)
    with pytest.raises(ValidationError, match=reason):
        Blueprint.model_validate(plan)


def test_local_elephant_still_needs_capacity_before_project_choice():
    plan = cloud_plan()
    plan["knowledge"]["mode"] = "local"
    with pytest.raises(ValidationError, match="Building the local knowledge service on GCP"):
        Blueprint.model_validate(plan)
    plan["gcp"].update(machine_type="e2-standard-8", disk_gb=100)
    assert compiler.package_availability(Blueprint.model_validate(plan))["status"] == "pending-configuration"


@pytest.mark.parametrize("target", ["mac", "linux", "gcp"])
@pytest.mark.parametrize("selection", ["acquisition", "rust_bundles"])
def test_existing_integration_holds_keep_precedence_and_exact_preview_labels(monkeypatch, target, selection):
    plan = starter_blueprint().model_dump(mode="json")
    plan["target"] = target
    if selection == "acquisition":
        plan["acquisition"] = {"helper_identity": plan["topology"]["entrypoint"],
            "helper_profile": "exclusive_application_role", "helper_tools": "preparation"}
        reason = compiler.CONNECTION_SETUP_PENDING
    else:
        plan["agent_classes"][0]["tools"]["rust_bundles"] = ["fixture_selected_native_tools"]
        reason = compiler.NATIVE_BUNDLE_PENDING

    def cannot_compile(*args, **kwargs):
        raise AssertionError("An integration hold must not compile package files.")

    monkeypatch.setattr(compiler, "generate_package", cannot_compile)
    preview = compiler.package_preview(plan)
    assert preview["package_availability"] == {"available": False,
        "status": "pending-integration", "reason": reason}
    assert preview["checks"] == [{"label": "Connection helper", "status": "pending-integration", "detail": reason}]
    assert preview["files"] == [] and preview["file_count"] == 0


@pytest.mark.parametrize("target", ["mac", "linux"])
def test_local_plans_still_allow_unused_empty_cloud_project(target):
    plan = starter_blueprint().model_dump(mode="json")
    plan["target"] = target
    assert plan["gcp"]["project"] == ""
    assert compiler.package_availability(plan) == {"available": True, "status": "ready", "reason": None}
