"""Application source projection retains the package's organization binding."""
from copy import deepcopy
import io
import json
from types import SimpleNamespace
from unittest.mock import AsyncMock, Mock
import zipfile

import pytest

from launchkit import deployment
from launchkit.artifact_binding import inputs, package_matches
from launchkit.deployment_sources import read_package_source
from launchkit.projects import DraftConflict, fingerprint
from test_deployment import Guide, Implementation, Service, submit


ORGANIZATION = {"id": "example", "version": "1", "guide_context": "Use the approved identity contract"}


@pytest.mark.parametrize("report_kind,state_kind,expected", [
    ("bound", "bound", True),
    ("bound", "different", False),
    ("bound", "absent", False),
    ("different", "bound", False),
    ("absent", "bound", False),
    ("absent", "absent", True),
    ("null", "bound", False),
    ("null", "absent", False),
    ("malformed", "bound", False),
    ("malformed", "absent", False),
])
def test_application_source_preserves_organization_scope(report_kind, state_kind, expected):
    state = Guide().state
    if state_kind != "absent":
        state["organization"] = {**ORGANIZATION, "version": "2" if state_kind == "different" else "1"}
    report = {"build_id": "build", "guide_version": state["version"],
              **{key: value for key, value in inputs(state).items() if key != "organization_hash"},
              "verification": {"digest": "a" * 64}}
    if report_kind != "absent":
        report["organization_hash"] = {
            "bound": fingerprint(ORGANIZATION), "different": "b" * 64,
            "null": None, "malformed": "invalid",
        }[report_kind]
    files = {"implementation-report.json": (json.dumps(report).encode(), 0o644)}
    before = deepcopy((files, report, state))
    source = read_package_source(files)
    # Exercise the admission consumer, including refusal after removing the
    # organization from a previously bound guide. Absence is not a null field.
    assert package_matches(report, state) is expected
    assert package_matches(source, state) is expected
    assert ("organization_hash" in source) == ("organization_hash" in report)
    if "organization_hash" in report:
        assert source["organization_hash"] == report["organization_hash"]
    assert (files, report, state) == before


@pytest.mark.parametrize("change", ["unchanged", "changed", "removed", "changed_during_options"])
async def test_deployment_options_keeps_frozen_package_organization_scope(tmp_path, monkeypatch, change):
    guide = Guide()
    guide.state["organization"] = deepcopy(ORGANIZATION)
    implementation = Implementation(guide)
    implementation.digest = "a" * 64
    payload, slug = await implementation.package(guide.identity, implementation.identity)
    output = io.BytesIO()
    with zipfile.ZipFile(io.BytesIO(payload)) as original, zipfile.ZipFile(output, "w") as archive:
        for info in original.infolist():
            content = original.read(info)
            if info.filename == "implementation-report.json":
                report = json.loads(content)
                report["organization_hash"] = fingerprint(ORGANIZATION)
                content = json.dumps(report).encode()
            archive.writestr(info, content)
    frozen_payload = output.getvalue()
    # Keep the package fixed while the guide changes. The implementation
    # fixture still claims current acceptance, so deployment must check scope.
    implementation.package = AsyncMock(return_value=(frozen_payload, slug))
    service = Service(tmp_path / "host", guide, implementation)

    async def capability():
        if change == "changed_during_options":
            guide.state["organization"]["version"] = "2"
        return {"available": False}

    capability_read = AsyncMock(side_effect=capability)
    service.service_manager = SimpleNamespace(capability=capability_read)
    port_read = Mock(return_value={"available": True})
    monkeypatch.setattr(deployment, "observe_local_console_port", port_read)
    if change == "changed":
        guide.state["organization"]["version"] = "2"
    elif change == "removed":
        del guide.state["organization"]
    if change == "unchanged":
        options = await service.options(guide.identity, implementation.identity)
        assert options["build_id"] == implementation.identity
        assert options["source_digest"] == implementation.digest
        assert options["current"] is True
        capability_read.assert_awaited_once()
        port_read.assert_called_once()
    else:
        reason = ("^The setup changed while preparing private setup\\. Reload before installing\\.$"
                  if change == "changed_during_options" else
                  "^The package belongs to a different proposal or application brief\\. Reload before installing\\.$")
        with pytest.raises(DraftConflict, match=reason):
            await service.options(guide.identity, implementation.identity)
        if change == "changed_during_options":
            capability_read.assert_awaited_once()
        else:
            capability_read.assert_not_awaited()
        port_read.assert_not_called()
    assert implementation.package.return_value == (frozen_payload, slug)
    assert not service.tasks


@pytest.mark.parametrize("change", ["changed", "removed", "chat_only"])
async def test_installed_application_retains_organization_currentness(tmp_path, monkeypatch, change):
    guide = Guide()
    guide.state["organization"] = deepcopy(ORGANIZATION)
    implementation = Implementation(guide)
    implementation.digest = "a" * 64
    service = Service(tmp_path / "host", guide, implementation)
    service.service_manager = SimpleNamespace(capability=AsyncMock(return_value={"available": False}))
    monkeypatch.setattr(deployment, "require_available_console_port", lambda *args, **kwargs: None)
    monkeypatch.setattr(deployment, "observe_local_console_port", lambda *args, **kwargs: {"available": True})
    try:
        receipt = await submit(service, tmp_path / "application")
        directory = service._directory(guide.identity)
        deployment.materialize(directory)
        await service.action(guide.identity, receipt["deployment_id"], "stop")
        package = (directory / "package.zip").read_bytes()
        assert receipt["organization_hash"] == fingerprint(ORGANIZATION)
        assert (await service.status(guide.identity))["current"] is True
        if change == "removed":
            del guide.state["organization"]
        elif change == "changed":
            guide.state["organization"]["version"] = "2"
        else:
            guide.state["version"] += 1
        assert (await service.status(guide.identity))["current"] is (change == "chat_only")
        retained = deployment.read_receipt(directory)
        assert retained["organization_hash"] == receipt["organization_hash"]
        assert retained["guide_version"] == 4
        assert (directory / "package.zip").read_bytes() == package
        assert not service.processes
    finally:
        await service.shutdown()


@pytest.mark.parametrize("change", ["changed", "null", "missing_source_field"])
def test_application_receipt_cannot_claim_another_organization(change):
    state = Guide().state
    report = {"build_id": "build", "guide_version": state["version"], **inputs(state),
              "organization_hash": fingerprint(ORGANIZATION), "verification": {"digest": "a" * 64}}
    receipt = {key: report[key] for key in ("build_id", "guide_version", "blueprint_hash", "organization_hash")}
    if change == "missing_source_field":
        del report["organization_hash"]
        receipt["organization_hash"] = None
    else:
        receipt["organization_hash"] = None if change == "null" else "b" * 64
    files = {"implementation-report.json": (json.dumps(report).encode(), 0o644)}
    with pytest.raises(ValueError, match="^The installed source binding could not be verified\\.$"):
        read_package_source(files, receipt)


def test_legacy_application_receipt_is_read_without_backfilling_organization():
    state = Guide().state
    report = {"build_id": "build", "guide_version": state["version"], **inputs(state),
              "organization_hash": fingerprint(ORGANIZATION), "verification": {"digest": "a" * 64}}
    receipt = {key: report[key] for key in ("build_id", "guide_version", "blueprint_hash")}
    before = deepcopy(receipt)
    source = read_package_source({"implementation-report.json": (json.dumps(report).encode(), 0o644)}, receipt)
    assert source["organization_hash"] == report["organization_hash"]
    assert receipt == before and "organization_hash" not in receipt
