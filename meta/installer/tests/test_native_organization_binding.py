"""Organization scope survives native packaging and installed-source reads."""
from copy import deepcopy
import json

import pytest

from launchkit.artifact_binding import package_matches
from launchkit.deployment import package_files, read_receipt
from launchkit.deployment_sources import compile_native_source, read_package_source, NATIVE_SOURCE
from launchkit.projects import fingerprint
from test_deployment_native_team import accepted_guide, installed, rehash, service


ORGANIZATION = {"id": "example", "version": "1", "guide_context": "Use approved identities"}


@pytest.fixture(scope="module")
def organization_package():
    guide = accepted_guide()
    guide.state["organization"] = deepcopy(ORGANIZATION)
    payload, source, _ = compile_native_source(guide.state, 4)
    return guide.state, payload, source


def test_organization_changes_native_identity_without_changing_compiler_files(organization_package):
    original, payload, source = organization_package
    assert source["organization_hash"] == fingerprint(ORGANIZATION)
    assert package_matches(source, original)
    changed = deepcopy(original)
    changed["organization"]["version"] = "2"
    next_payload, next_source, _ = compile_native_source(changed, 4)
    assert next_source["source_id"] != source["source_id"]
    assert next_source["source_digest"] == source["source_digest"]
    assert next_source["blueprint_hash"] == source["blueprint_hash"]
    assert next_source["brief_hash"] == source["brief_hash"]
    assert not package_matches(source, changed)
    assert not package_matches(next_source, original)
    files, next_files = package_files(payload), package_files(next_payload)
    assert json.loads(files[NATIVE_SOURCE][0])["protocol"] == 2
    assert {key: value for key, value in files.items() if key != NATIVE_SOURCE} == {
        key: value for key, value in next_files.items() if key != NATIVE_SOURCE}
    assert read_package_source(files, source) == source
    with pytest.raises(ValueError, match="^The installed source binding could not be verified\\.$"):
        read_package_source(next_files, source)


def test_missing_and_null_organization_keep_legacy_native_bytes():
    state = accepted_guide().state
    original = compile_native_source(state, 4)
    state["organization"] = None
    assert compile_native_source(state, 4) == original
    payload, source, _ = original
    assert "organization_hash" not in source
    manifest = json.loads(package_files(payload)[NATIVE_SOURCE][0])
    assert "organization_hash" not in manifest and manifest["protocol"] == 1
    assert read_package_source(package_files(payload), source) == source


@pytest.mark.parametrize("invalid", [None, False, 7, "", "a" * 63, "A" * 64, [], {}])
def test_rehashed_native_organization_must_be_a_digest(organization_package, invalid):
    _, payload, _ = organization_package
    files = package_files(payload)
    manifest = json.loads(files[NATIVE_SOURCE][0])
    manifest["organization_hash"] = invalid
    files[NATIVE_SOURCE] = (json.dumps(manifest).encode(), 0o644)
    with pytest.raises(ValueError, match="^The compiled agent-team source manifest could not be verified\\.$"):
        read_package_source(rehash(files))


@pytest.mark.parametrize("change", ["changed", "removed"])
def test_native_organization_cannot_change_under_original_source_id(organization_package, change):
    _, payload, _ = organization_package
    files = package_files(payload)
    manifest = json.loads(files[NATIVE_SOURCE][0])
    if change == "changed":
        manifest["organization_hash"] = "b" * 64
    else:
        del manifest["organization_hash"]
    files[NATIVE_SOURCE] = (json.dumps(manifest).encode(), 0o644)
    with pytest.raises(ValueError, match="^The compiled agent-team source manifest could not be verified\\.$"):
        read_package_source(files)


@pytest.mark.parametrize("protocol,keep_organization", [
    (1, True), (2, False), (3, True), ("2", True), (2.0, True), (True, True),
])
def test_native_protocol_selects_an_exact_versioned_shape(organization_package, protocol, keep_organization):
    _, payload, _ = organization_package
    files = package_files(payload)
    manifest = json.loads(files[NATIVE_SOURCE][0])
    manifest["protocol"] = protocol
    if not keep_organization:
        del manifest["organization_hash"]
    files[NATIVE_SOURCE] = (json.dumps(manifest).encode(), 0o644)
    with pytest.raises(ValueError, match="^The compiled agent-team source manifest could not be verified\\.$"):
        read_package_source(rehash(files))


@pytest.mark.parametrize("change", ["changed", "removed", "null"])
def test_native_receipt_must_retain_original_organization(organization_package, change):
    _, payload, source = organization_package
    receipt = deepcopy(source)
    if change == "removed":
        del receipt["organization_hash"]
    else:
        receipt["organization_hash"] = None if change == "null" else "b" * 64
    with pytest.raises(ValueError, match="^The installed source binding could not be verified\\.$"):
        read_package_source(package_files(payload), receipt)


@pytest.mark.parametrize("change", ["changed", "removed", "chat_only"])
async def test_installed_native_team_currentness_keeps_organization(service, tmp_path, change):
    service.guide.state["organization"] = deepcopy(ORGANIZATION)
    try:
        options = await service.options(service.guide.identity, source_kind="native-team", version=4)
        directory, receipt = await installed(service, tmp_path / "team")
        assert receipt["organization_hash"] == fingerprint(ORGANIZATION)
        assert receipt["source_id"] == options["source_id"]
        package = (directory / "package.zip").read_bytes()
        assert (await service.status(service.guide.identity))["current"] is True
        if change == "removed":
            del service.guide.state["organization"]
        elif change == "changed":
            service.guide.state["organization"]["version"] = "2"
        else:
            service.guide.state["version"] += 1
        observed = await service.status(service.guide.identity)
        assert observed["current"] is (change == "chat_only")
        retained = read_receipt(directory)
        assert retained["organization_hash"] == receipt["organization_hash"]
        assert retained["source_id"] == receipt["source_id"]
        assert retained["guide_version"] == 4
        assert (directory / "package.zip").read_bytes() == package
        assert not service.processes
    finally:
        await service.shutdown()
