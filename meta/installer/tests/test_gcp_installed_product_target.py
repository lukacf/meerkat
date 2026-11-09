"""Preserved GCP source through real byte Install and private target readers.

The existing original source/artifact/Host/Supervisor/receipt readers are real.
Guest platform observations, poisoned SDK/prefix copies and application-check
commands are explicitly inert. No VM, native setup, grants or readiness follows.
"""
import asyncio
from copy import deepcopy
from dataclasses import replace
import json
import platform
import threading

import pytest

from launchkit import deployment, product_runtime_custody as custody, runtime_selection
from launchkit import deployment_updates as updates
from launchkit.connect_installer_caller import CustodyError
from launchkit.destination_application_inputs import DestinationApplicationInputs
from launchkit.forge_application_host import ApplicationPreparationInputs, ForgeApplicationHost
from launchkit.local_application_preparation_inputs import LocalApplicationPreparationOwner
from launchkit.projects import DraftConflict
from launchkit.runtime_candidates import CandidateArtifactError
from launchkit.remote_product_artifacts import ProductArtifactTransferError, product_selection_bytes
from launchkit.templates.storage_setup import project_lock
from test_destination_application_factory import destination
from test_destination_application_inputs import inputs_destination, install as install_destination_inputs
from test_product_destination_install import installation, selected_host, source_bytes
from test_product_destination_owner import (
    destination_source, gcp_destination_source, no_destination_execution,
)
from test_product_artifact_transfer import custody as artifact_custody
from test_meta_toolkit_runtime_selection import product, no_product_execution


@pytest.fixture(autouse=True)
def inert_actual_guest_platform(monkeypatch):
    # The inherited fixture already supplies Linux/x86_64/glibc observations.
    # This is the same explicit physical-guest seam, never a production bypass.
    monkeypatch.setattr(deployment.sys, "platform", "linux")


def package(f):
    return deployment.package_files(f.selected.package)


def canonical_service(f):
    return deployment.DeploymentService(f.selected.custody.home / ".local/state/meta-forge", None, None)


async def checked_configuration(f, service):
    return await custody.await_product_validation(f.source, service._installed_configuration,
        f.selected.context["guide_id"], f.selected.context["deployment_id"],
        source_custody=f.source, with_files=True)


@pytest.mark.parametrize("installation", [None, "gcp"], indirect=True, ids=["ordinary-linux", "original-gcp"])
async def test_original_physical_target_reaches_actual_byte_install_and_installed_reader(
        installation, monkeypatch):
    with selected_host(installation) as f:
        cloud = f.selected.context["schema"] == 2
        before_source = source_bytes(f)
        before_context = deepcopy(f.selected.context)
        before_acquisition = f.acquisition.read(), f.viewer.read()
        owner_task, owner_thread = asyncio.current_task(), threading.get_ident()
        file_reads = []
        original = custody.gcp_product_deployment_target

        def read(*args, **kwargs):
            assert threading.get_ident() != owner_thread
            with pytest.raises(RuntimeError, match="no running event loop"):
                asyncio.get_running_loop()
            assert args[0] is f.source and args[1] == f.source.origin
            result = original(*args, **kwargs)
            file_reads.append(result)
            return result

        monkeypatch.setattr(custody, "gcp_product_deployment_target", read)
        installer = await f.host.install_staged_source(settings=f.settings, require_current=f.current_owner)
        service = canonical_service(f)
        snapshot, files = await checked_configuration(f, service)
        selected = await custody.await_product_validation(f.source, updates._candidate_source_selection,
            files, source_custody=f.source)
        directory, receipt, destination, definition, fields, digest = snapshot
        expected_target = "gcp" if cloud else "linux"
        assert installer is f.host._destination_installation_supervisor and f.supervisors == [installer]
        assert f.host._installation_supervisor is None and f.host._retained_preparation_inputs is None
        assert directory == f.directory and destination == f.selected.destination
        assert definition["target"] == expected_target and json.loads(files["blueprint.json"][0])["target"] == expected_target
        assert receipt["status"] == "stopped" and receipt["phase"] == "configure"
        assert receipt["runtime_candidate"] == f.selected.origin
        assert selected == f.selected.origin
        assert all(receipt[name] == f.selected.context[name] for name in
            ("guide_id", "deployment_id", "build_id", "guide_version", "package_sha256"))
        assert digest == f.selected.context["source_digest"] and fields
        assert receipt["installed_candidate_runtime_proof"]["source"] == "meta-toolkit-product"
        assert receipt.get("runtime_pid") is None and receipt.get("console_url") is None
        assert not receipt.get("process_live") and not receipt.get("bootstrap")
        assert f.owner_calls and all(task is owner_task for task in f.owner_calls)
        assert source_bytes(f) == before_source and f.selected.context == before_context
        assert (f.directory / "package.zip").read_bytes() == f.selected.package
        assert (f.acquisition.read(), f.viewer.read()) == before_acquisition
        assert f.source.origin == f.selected.custody.plan.origin
        assert f.source.relocated != f.source.origin
        if cloud:
            assert file_reads and set(file_reads) == {"linux"}
            disposition = before_context["source_disposition"]
            assert definition["slug"] == disposition["instance"]
            assert all(json.loads(files["blueprint.json"][0])["gcp"][name] == disposition[name]
                for name in ("project", "zone"))
            with pytest.raises(DraftConflict, match="checked installed configuration could not be verified"):
                service._installed_configuration(f.selected.context["guide_id"], f.selected.context["deployment_id"])
        else:
            assert file_reads == []
    assert f.source._closed and f.source._lease._closed


@pytest.mark.parametrize("installation", ["gcp"], indirect=True)
async def test_unbound_gcp_package_retains_public_and_conventional_refusals(installation):
    with selected_host(installation) as f:
        files = package(f)
        original = source_bytes(f)
        # This establishes a complete legitimate typed-source baseline first.
        artifacts = await custody.await_product_validation(f.source, deployment.product_runtime_source,
            files, f.selected.origin, source_custody=f.source)
        assert artifacts is f.source.artifacts
        with pytest.raises(ValueError, match="archived deployment does not select this exact local candidate"):
            deployment.product_runtime_source(files, f.selected.origin)
        with pytest.raises(ValueError, match="does not match this deployment platform"):
            runtime_selection.candidate_selection(f.source.origin, deployment_target="gcp")
        with pytest.raises(ValueError, match="GCP remains a separate deployment adapter"):
            deployment.local_target(json.loads(files["deployment.json"][0]))
        assert source_bytes(f) == original and not f.directory.exists()


@pytest.mark.parametrize("installation", ["gcp"], indirect=True)
@pytest.mark.parametrize("change", ["pair-dict", "other-origin", "caller-source", "source-file", "cloud-context", "store", "owner"])
async def test_gcp_runtime_join_refuses_foreign_or_changed_original_owner(installation, change):
    with selected_host(installation) as f:
        files = package(f)
        artifacts = await custody.await_product_validation(f.source, deployment.product_runtime_source,
            files, f.selected.origin, source_custody=f.source)
        assert artifacts is f.source.artifacts
        original = source_bytes(f), f.acquisition.read(), f.viewer.read()
        supplied = f.source
        selected = f.selected.origin
        error_type = (CustodyError, DraftConflict, CandidateArtifactError)
        if change == "pair-dict":
            supplied = {"origin": f.source.origin, "relocated": f.source.relocated}
            expected = "original initialized stopped source owner"
        elif change == "other-origin":
            target = replace(f.source.origin, target="macos-arm64")
            selected = deepcopy(f.selected.origin)
            selected["target"] = target.target
            assert runtime_selection.parse_candidate_selection(selected) == target
            expected = "exact accepted package"
        elif change == "caller-source":
            files["README.md"] = (b"other application source\n", files["README.md"][1])
            expected = "changed accepted source bytes or modes"
        elif change == "source-file":
            path = f.selected.destination / "application/tool-policy.json"
            path.write_bytes(path.read_bytes() + b" \n")
            original = source_bytes(f), f.acquisition.read(), f.viewer.read()
            # The real worker invalidates this source; its final retained owner
            # fence reports that refusal before returning the worker exception.
            error_type = ProductArtifactTransferError
            expected = "The original stopped source lease is no longer current\\."
        elif change == "cloud-context":
            f.source._context["source_disposition"]["instance"] = "another-original-vm"
            expected = "changed their original cloud source custody"
        elif change == "store":
            f.source._store = f.source._store.parent / "another-source-intake"
            expected = "changed their original cloud source custody"
        else:
            f.current = False
            expected = "Original destination installation owner changed"
        with pytest.raises(error_type, match=expected):
            await custody.await_product_validation(supplied, deployment.product_runtime_source,
                files, selected, source_custody=supplied)
        assert (source_bytes(f), f.acquisition.read(), f.viewer.read()) == original
        assert not f.directory.exists() and not f.supervisors
        assert not (f.selected.destination / ".env").exists()


@pytest.mark.parametrize("installation", ["gcp"], indirect=True)
async def test_closed_original_gcp_source_cannot_reopen_physical_target(installation):
    with selected_host(installation) as f:
        files = package(f)
        artifacts = await custody.await_product_validation(f.source, deployment.product_runtime_source,
            files, f.selected.origin, source_custody=f.source)
        assert artifacts is f.source.artifacts
    assert f.source._closed and f.source._lease._closed
    with pytest.raises((CustodyError, ValueError), match="lease is no longer current"):
        await custody.await_product_validation(f.source, deployment.product_runtime_source,
            files, f.selected.origin, source_custody=f.source)
    assert not f.directory.exists()


@pytest.mark.parametrize("installation", ["gcp"], indirect=True)
@pytest.mark.parametrize("host", ["os", "architecture", "libc"])
async def test_gcp_target_still_requires_its_exact_supported_physical_guest(installation, monkeypatch, host):
    with selected_host(installation) as f:
        files = package(f)
        artifacts = await custody.await_product_validation(f.source, deployment.product_runtime_source,
            files, f.selected.origin, source_custody=f.source)
        assert artifacts is f.source.artifacts
        original = source_bytes(f)
        if host == "os":
            monkeypatch.setattr(platform, "system", lambda: "Darwin")
        elif host == "architecture":
            monkeypatch.setattr(platform, "machine", lambda: "aarch64")
        else:
            monkeypatch.setattr(platform, "libc_ver", lambda: ("musl", "1.2.5"))
        error_type = RuntimeError if host == "libc" else CustodyError
        with pytest.raises(error_type, match="supported Linux x86_64 guest|glibc 2.31"):
            await custody.await_product_validation(f.source, deployment.product_runtime_source,
                files, f.selected.origin, source_custody=f.source)
        assert source_bytes(f) == original and not f.directory.exists()


@pytest.mark.parametrize("installation", ["gcp"], indirect=True)
@pytest.mark.parametrize("change", ["staged-scope", "other-origin"])
async def test_installed_gcp_configuration_cannot_borrow_a_staging_or_foreign_receipt(installation, change):
    with selected_host(installation) as f:
        await f.host.install_staged_source(settings=f.settings, require_current=f.current_owner)
        service = canonical_service(f)
        snapshot, files = await checked_configuration(f, service)
        assert snapshot[1]["status"] == "stopped" and snapshot[3]["target"] == "gcp"
        receipt = deployment.read_receipt(f.directory)
        if change == "staged-scope":
            receipt["scope"] = f.source.receipt["scope"]
        else:
            target = replace(f.source.origin, target="macos-arm64")
            receipt["runtime_candidate"] = deepcopy(f.selected.origin)
            receipt["runtime_candidate"]["target"] = target.target
            assert runtime_selection.parse_candidate_selection(receipt["runtime_candidate"]) == target
        deployment.write_private(f.directory / "receipt.json", receipt)
        original = source_bytes(f), (f.directory / "receipt.json").read_bytes(), (f.selected.destination / ".env").read_bytes()
        with pytest.raises(DraftConflict, match="^The checked installed configuration could not be verified on this computer\\.$"):
            await checked_configuration(f, service)
        assert (source_bytes(f), (f.directory / "receipt.json").read_bytes(),
            (f.selected.destination / ".env").read_bytes()) == original
        assert f.supervisors == [f.host._destination_installation_supervisor]
        assert f.host._installation_supervisor is None and not service.tasks and not service.processes


@pytest.mark.parametrize("installation", ["gcp"], indirect=True)
async def test_original_gcp_factory_install_retains_exact_native_inputs_on_original_task(
        inputs_destination, monkeypatch, installation):
    """Actual default supplier and synchronous retainer; no inventory or Prepare."""
    f = inputs_destination
    # The input fixture selects the original access source before requesting
    # destination/installation; this explicit argument exposes the GCP param.
    assert installation is f
    original_source = source_bytes(f)
    original_context = deepcopy(f.selected.context)
    original_package = f.selected.package
    original_pins = f.acquisition.read(), f.viewer.read(), tuple(pin.read() for pin in f.pins.values())
    task, loop, thread = asyncio.current_task(), asyncio.get_running_loop(), threading.get_ident()
    retained = []
    original_retain = LocalApplicationPreparationOwner.retain

    def retain(owner, guide_id, deployment_id, supervisor):
        assert asyncio.current_task() is task and asyncio.get_running_loop() is loop
        assert threading.get_ident() == thread and f.service.lock.locked()
        source = owner.host._product_source_custody
        assert source is f.factory._source and not source._closed
        assert source.origin == f.selected.custody.plan.origin and source._context == original_context
        source._lease.require_current()
        projections = tuple((pin, pin.read()) for pin in owner.factory._pins)
        chosen = original_retain(owner, guide_id, deployment_id, supervisor)
        assert type(chosen) is ApplicationPreparationInputs and chosen.supervisor is supervisor
        assert tuple((pin, pin.read()) for pin in owner.factory._pins) == projections
        retained.append((owner, guide_id, deployment_id, supervisor, projections))
        return chosen

    monkeypatch.setattr(LocalApplicationPreparationOwner, "retain", retain)
    owner, installer = await install_destination_inputs(f)
    assert type(owner) is DestinationApplicationInputs and type(owner.host) is ForgeApplicationHost
    assert retained == [(owner.preparation_owner, f.selected.context["guide_id"],
        f.selected.context["deployment_id"], f.held.supervisor,
        tuple((pin, pin.read()) for pin in owner._pins))]
    assert f.supervisors == [installer, f.held.supervisor]
    assert owner.host._destination_installation_supervisor is installer
    assert owner.host._installation_supervisor is f.held.supervisor
    assert owner.host._retained_preparation_inputs is f.held
    assert owner.host._blueprint["target"] == "gcp"
    assert owner.host._blueprint["gcp"]["project"] == original_context["source_disposition"]["project"]
    assert owner.host._blueprint["gcp"]["zone"] == original_context["source_disposition"]["zone"]
    receipt = deployment.read_receipt(f.directory)
    assert receipt["status"] == "stopped" and receipt["phase"] == "configure"
    assert receipt["runtime_candidate"] == f.selected.origin
    assert all(receipt[name] == original_context[name] for name in
        ("guide_id", "deployment_id", "build_id", "guide_version", "package_sha256"))
    assert json.loads(package(f)["deployment.json"][0])["slug"] == original_context["source_disposition"]["instance"]
    assert source_bytes(f) == original_source and f.selected.context == original_context
    assert f.selected.package == original_package == (f.directory / "package.zip").read_bytes()
    assert (f.acquisition.read(), f.viewer.read(), tuple(pin.read() for pin in f.pins.values())) == original_pins
    assert tuple(pin.read() for pin in owner._pins) == owner._bytes
    assert f.factory._source is None and f.held.supervisor._product_source_custody._closed
    choices = json.loads(owner.preparation_owner._choices_pin.read())
    assert choices["namespace"] == original_context["deployment_id"] and choices["generation"] == 1
    assert choices["console_operator"]["id"] == "fixture-operator"
    assert json.loads(f.held.activation_template.read())["activation_id"] == original_context["deployment_id"]
    assert not f.inventory_calls and not f.held.inventory_attempt_path.exists()
    assert not f.held.preparation_plan_path.exists() and not f.held.startup_selection_path.exists()
    assert not receipt.get("bootstrap") and receipt.get("runtime_pid") is None
    assert not f.service.tasks and not f.service.processes
    with project_lock(f.selected.destination):
        pass
