"""Existing-host intent must never enter Forge's new-VM provisioning path."""
import json
import os
from pathlib import Path
import shlex
import subprocess

import pytest
from pydantic import ValidationError

from launchkit.blueprint import Blueprint, CloudOptions, starter_blueprint
from launchkit.compiler import _readme, generate_files, package_availability
from launchkit.deployment import deployment_handoff
from launchkit.gcp_preflight import GcpPreflightService


def existing():
    value = starter_blueprint().model_dump(mode="json")
    value["target"] = "gcp"
    value["project"]["slug"] = "review-helper"
    value["gcp"].update(project="example-project", provisioning="existing_host", existing_instance="retained-vm",
                        existing_zone_confirmed=True)
    return value


def test_existing_destination_keeps_distinct_instance_and_legacy_serialization():
    legacy = dict(project="example-project", region="europe-west1", zone="europe-west1-b", machine_type="e2-medium", disk_gb=30)
    assert CloudOptions.model_validate(legacy).model_dump(mode="json") == legacy
    assert CloudOptions.model_validate({**legacy, "provisioning": "new_vm"}).model_dump(mode="json") == legacy
    plan = Blueprint.model_validate(existing())
    assert plan.project.slug != plan.gcp.existing_instance
    assert plan.model_dump()["gcp"]["provisioning"] == "existing_host"
    assert Blueprint.model_validate_json(plan.model_dump_json()) == plan
    with pytest.raises(ValidationError, match="requires existing_host"):
        CloudOptions.model_validate({**legacy, "existing_instance": "retained-vm"})
    with pytest.raises(ValidationError, match="requires existing_host"):
        CloudOptions.model_validate({**legacy, "existing_zone_confirmed": True})
    for value in ("true", 1):
        with pytest.raises(ValidationError):
            CloudOptions.model_validate({**existing()["gcp"], "existing_zone_confirmed": value})
    for name in ("--project=other", "vm; touch marker", "vm/path", "UPPER", "trailing-", "a" * 64):
        with pytest.raises(ValidationError):
            Blueprint.model_validate({**existing(), "gcp": {**existing()["gcp"], "existing_instance": name}})


def test_unknown_existing_instance_stays_editable_but_not_packageable():
    value = existing()
    value["gcp"]["existing_instance"] = ""
    spec = Blueprint.model_validate(value)
    assert package_availability(spec)["status"] == "pending-configuration"
    with pytest.raises(ValueError, match="existing VM"):
        generate_files(spec)


def test_existing_vm_does_not_accept_an_unconfirmed_default_zone():
    value = existing()
    value["gcp"].pop("existing_zone_confirmed", None)
    spec = Blueprint.model_validate(value)
    assert spec.gcp.zone == "europe-west1-b"
    assert package_availability(spec)["status"] == "pending-configuration"
    assert "zone" in package_availability(spec)["reason"]
    with pytest.raises(ValueError, match="zone"):
        generate_files(spec)
    reopened = Blueprint.model_validate_json(spec.model_dump_json())
    assert package_availability(reopened)["status"] == "pending-configuration"
    files = generate_files(existing())
    from launchkit.projects import DraftConflict
    with pytest.raises(DraftConflict, match="confirmed zone"):
        deployment_handoff(json.loads(files["deployment.json"]), files, value, "guide", "build")


def test_confirmed_existing_vm_zone_reaches_tunnel_and_handoff():
    value = existing()
    value["gcp"].update(region="us-central1", zone="us-central1-c", existing_zone_confirmed=True)
    spec = Blueprint.model_validate_json(Blueprint.model_validate(value).model_dump_json())
    files = generate_files(spec)
    args = shlex.split(files["deploy/tunnel-gcp.sh"].splitlines()[-1])
    assert args[args.index("--zone") + 1] == "us-central1-c"
    _, _, handoff = deployment_handoff(json.loads(files["deployment.json"]), files,
                                      spec.model_dump(mode="json"), "guide", "build")
    assert "us-central1-c" in json.dumps(handoff)
    assert "us-central1-c" in files["README.md"]
    assert "us-central1-c" in files["deploy/deploy-gcp.sh"]
    assert "europe-west1-b" not in json.dumps(handoff)


@pytest.mark.parametrize("args", [[], ["--dry-run"], ["--resume"], ["--setup"], ["--status"], ["--stop"], ["--restart"], ["--remove-service"], ["--dry-run", "--resume"]])
def test_exported_existing_host_script_cannot_change_cloud_or_host(tmp_path, args):
    files = generate_files(existing())
    path = tmp_path / "deploy-gcp.sh"
    path.write_text(files["deploy/deploy-gcp.sh"])
    # Any accidental gcloud invocation fails visibly; no real cloud access.
    binary = tmp_path / "gcloud"
    binary.write_text('#!/bin/sh\nprintf "called" > "$TRIPWIRE"\nexit 99\n')
    binary.chmod(0o755)
    marker = tmp_path / "unexpected-cloud-call"
    result = subprocess.run(["/bin/bash", str(path), *args], cwd=tmp_path,
        env={**os.environ, "PATH": str(tmp_path) + ":/usr/bin:/bin", "TRIPWIRE": str(marker)},
        text=True, capture_output=True, timeout=10)
    assert result.returncode == (0 if args == ["--dry-run"] else 1)
    assert "example-project / europe-west1-b / retained-vm" in result.stdout
    assert not marker.exists()
    assert sorted(p.name for p in tmp_path.iterdir()) == ["deploy-gcp.sh", "gcloud"]
    assert "instances create" not in path.read_text()
    assert "firewall-rules" not in path.read_text()
    assert "sudo" not in path.read_text()


def test_existing_host_tunnel_uses_instance_and_handoff_has_no_create_or_resume(monkeypatch):
    proposal = existing()
    files = generate_files(proposal)
    args = shlex.split(files["deploy/tunnel-gcp.sh"].splitlines()[-1])
    assert args[:5] == ["exec", "gcloud", "compute", "ssh", "retained-vm"]
    assert "review-helper" not in args
    monkeypatch.setattr("launchkit.deployment.sys.platform", "darwin")
    _, _, handoff = deployment_handoff(json.loads(files["deployment.json"]), files, proposal, "guide", "build")
    commands = [cmd for item in handoff["steps"] + handoff["operations"] for cmd in item.get("commands", [])]
    assert "bash install.sh" in commands
    assert "bash deploy/install-service.sh" in commands
    assert "bash deploy/tunnel-gcp.sh" in commands
    assert not any("deploy-gcp.sh" in cmd or "gcloud" in cmd for cmd in commands)
    content = json.dumps(handoff)
    assert "retained-vm" in content
    assert "Review, then create" not in content
    assert "--resume" not in content
    readme = files["README.md"]
    assert "existing VM example-project / europe-west1-b / retained-vm" in readme
    assert "creating billed resources" not in readme
    assert "instances delete" not in readme
    assert "compute ssh review-helper" not in readme
    for candidate, product in [(True, False), (True, True)]:
        selected = _readme(Blueprint.model_validate(proposal), candidate=candidate, product=product)
        assert "existing VM example-project" in selected
        assert "creating billed resources" not in selected
        assert "saved" in selected


async def test_cloud_preflight_refuses_existing_host_before_package_or_cloud_access():
    from test_gcp_preflight import Local
    local = Local()
    local.state["proposal"]["gcp"].update(provisioning="existing_host", existing_instance="retained-vm")
    async def no_package(*args, **kwargs):
        pytest.fail("existing-host intent must refuse new-VM preparation before reading the package")
    local.package = no_package
    service = GcpPreflightService(local)
    with pytest.raises(ValueError, match="existing host"):
        await service.options(local.state["id"], local.build_id)
