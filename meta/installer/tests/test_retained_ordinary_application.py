"""Source-only regression for a launcher upgrade with an older installed app.

The fixtures use actual guide/package/installation readers and normal restart
admission. Native binaries, signed acceptance and worker execution are inert
specimens; this does not establish an installed restart or hosted acceptance.
"""
from dataclasses import replace
import hashlib
from uuid import uuid4

import pytest

from launchkit import deployment
from launchkit.connect_installer_caller import CustodyError, ExactFile
from launchkit.deployment_application_bootstrap import read_bootstrap_admission
from launchkit.guide import GuideService
from launchkit.guide_store import GuideStore
from launchkit.implementation import ImplementationService
from launchkit.local_application_registry import LocalApplicationRegistry
from launchkit.ordinary_application_source import RetainedOrdinaryApplication
from launchkit.projects import DraftConflict
# build seeds the legacy ordinary specimen: a package from before Connect became the default.
from test_ordinary_application_registry import build, ordinary_registry, ordinary_guide
from test_local_application_registry import (
    registry, product, no_product_execution, no_processes,
    accept_source_fixture, install_fixture, bind_registry,
)
from test_meta_toolkit_runtime_update import catalog_target
from test_development_product_selection import development


@pytest.fixture
def newer_target(tmp_path):
    """A different, complete SDK/native artifact selection, never executed."""
    root = tmp_path / "newer-launcher"
    root.mkdir(mode=0o700)
    generator = product.__wrapped__(root)
    specimen = next(generator)
    try:
        catalog_target(specimen)
        yield development(specimen, meta_version="0.9.1")[0]
    finally:
        next(generator, None)


def upgraded_registry(f, newer_target, retained, *, gateway=None):
    store = GuideStore(f.store.root)
    guide = GuideService(store, architect=None)
    implementation = ImplementationService(f.implementation.root.parent, guide)
    service = deployment.DeploymentService(f.service.root.parent, guide, implementation)
    owner = LocalApplicationRegistry(guide_store=store, implementation=implementation,
        deployment=service, runtime_target=newer_target, ordinary_meta_gateway=gateway,
        retained_ordinary_applications=retained)
    implementation.prepare_workspace = owner.prepare_workspace
    bind_registry(service, owner)
    return implementation, service, owner


async def installed(f, monkeypatch):
    saved = ordinary_guide(f)
    result = await build(f, saved)
    await accept_source_fixture(f, result)
    directory, destination = await install_fixture(f, saved, result, monkeypatch)
    receipt = deployment.read_receipt(directory)
    entry = RetainedOrdinaryApplication(saved["id"], directory.name, result["build_id"],
        receipt["package_sha256"], f.target, f.gateway)
    return saved, result, directory, destination, entry


async def test_launcher_upgrade_restarts_only_the_selected_installed_runtime(
        ordinary_registry, newer_target, monkeypatch):
    f = ordinary_registry
    saved, result, directory, destination, entry = await installed(f, monkeypatch)
    original = f.registry._selected_startup(saved["id"], directory.name)
    package = (directory / "package.zip").read_bytes()
    implementation, service, owner = upgraded_registry(f, newer_target, (entry,))
    assert owner.runtime_target != f.target and owner.ordinary_meta_gateway is None
    before = (directory / "receipt.json").read_bytes()
    status = await owner.setup_status(service, saved["id"], directory.name)
    assert status["reason"] == "owner_reconnect"
    assert not owner._builds and not owner._installations
    assert (directory / "receipt.json").read_bytes() == before

    launched = []
    monkeypatch.setattr(service, "_launch", lambda *args, **kwargs: launched.append((args, kwargs)))
    request = str(uuid4())
    first = await service.action(saved["id"], directory.name, "restart", action_request_id=request)
    assert first["action_request"]["outcome"] == "returned"
    assert owner._selected_startup(saved["id"], directory.name) == original
    assert len(launched) == 1 and launched[0][0][1] == "start"
    admission = read_bootstrap_admission(directory, launched[0][1]["bootstrap_selection"])
    assert admission["application_startup"] == original
    assert admission["application_startup"]["gateway"] == f.gateway.pin()
    second = await service.action(saved["id"], directory.name, "restart", action_request_id=request)
    assert second["action_request"] == first["action_request"] and len(launched) == 1
    assert (directory / "package.zip").read_bytes() == package
    assert not (destination / "current").exists() and not (destination / "shared").exists()
    assert not service.tasks and not service.processes and not implementation.tasks

    # Recovery grants custody of this installation, not a new Build/Install.
    assert not owner._builds
    with pytest.raises(CustodyError, match="no retained local source owner"):
        owner.host_for_build(saved["id"], result["build_id"])
    with pytest.raises((CustodyError, DraftConflict)):
        owner.validate_settings(saved["id"], {}, build_id=result["build_id"])
    assert not owner._builds
    fresh = ordinary_guide(f)
    # The held gateway belongs only to the installed application. Even that
    # guide needs the current ordinary gateway to continue its ordinary
    # source, which Build checks for a continuation. A new plan takes the
    # helperless route, which needs no gateway.
    with pytest.raises(CustodyError, match="without the Meta RPC gateway"):
        owner.legacy_ordinary_continuation(saved["id"], result["build_id"])
    assert owner.initial_build_options(fresh["id"], fresh["version"]) == {"runtime_target": newer_target}
    assert owner._runtime_target_for_blueprint(fresh["proposal"]) is newer_target
    assert owner._ordinary_runtime_selection(saved["id"], saved["proposal"]) == (newer_target, None)


@pytest.mark.parametrize("fault", ["absent", "wrong-guide", "wrong-installation", "wrong-build", "wrong-package",
    "wrong-target", "moved-gateway"])
async def test_upgrade_cannot_recover_from_receipts_or_unrelated_launcher_choices(
        ordinary_registry, newer_target, monkeypatch, fault):
    f = ordinary_registry
    saved, result, directory, _, entry = await installed(f, monkeypatch)
    if fault == "wrong-guide":
        entry = replace(entry, guide_id=str(uuid4()))
    elif fault == "wrong-installation":
        entry = replace(entry, deployment_id=str(uuid4()))
    elif fault == "wrong-build":
        entry = replace(entry, build_id=str(uuid4()))
    elif fault == "wrong-package":
        entry = replace(entry, package_sha256="a" * 64)
    elif fault == "wrong-target":
        entry = replace(entry, runtime_target=newer_target)
    elif fault == "moved-gateway":
        path = f.gateway.path.with_name("copied-gateway")
        path.write_bytes(f.gateway.path.read_bytes())
        path.chmod(0o700)
        entry = replace(entry, gateway=ExactFile(path, f.gateway.sha256))
    _, service, owner = upgraded_registry(f, newer_target, () if fault == "absent" else (entry,))
    launched = []
    monkeypatch.setattr(service, "_launch", lambda *args, **kwargs: launched.append((args, kwargs)))
    with pytest.raises((CustodyError, DraftConflict)):
        await service.action(saved["id"], directory.name, "restart", action_request_id=str(uuid4()))
    assert not launched and not owner._builds and not owner._installations


@pytest.mark.parametrize("fault", ["package-bytes", "source-pin", "installed-source"])
async def test_explicit_retention_does_not_bypass_original_installation_checks(
        ordinary_registry, newer_target, monkeypatch, fault):
    f = ordinary_registry
    saved, result, directory, destination, entry = await installed(f, monkeypatch)
    _, service, owner = upgraded_registry(f, newer_target, (entry,))
    if fault == "package-bytes":
        (directory / "package.zip").write_bytes(b"changed accepted package\n")
    elif fault == "source-pin":
        source = f.implementation._directory(saved["id"], result["build_id"]) / "ordinary-application-source"
        (source / "application-runtime-selection.json").write_bytes(b"{}\n")
    else:
        # Ordinary physical-runtime verification belongs to the actual worker.
        # This source-only admission control can prove installed byte refusal.
        (destination / "config/mob.toml").write_bytes(b"changed installed definition\n")
    launched = []
    monkeypatch.setattr(service, "_launch", lambda *args, **kwargs: launched.append((args, kwargs)))
    with pytest.raises((CustodyError, DraftConflict)):
        await service.action(saved["id"], directory.name, "restart", action_request_id=str(uuid4()))
    assert not launched and not owner._builds and not owner._installations


@pytest.mark.parametrize("fault", ["list", "duplicate", "dict", "wrong-id", "wrong-digest", "wrong-target", "wrong-gateway", "changed-gateway"])
async def test_retained_selection_requires_exact_launcher_inputs(ordinary_registry, newer_target, fault):
    f = ordinary_registry
    entry = RetainedOrdinaryApplication(str(uuid4()), str(uuid4()), str(uuid4()), "a" * 64, f.target, f.gateway)
    entries = (entry,)
    if fault == "list":
        entries = [entry]
    elif fault == "duplicate":
        entries = (entry, entry)
    elif fault == "dict":
        entries = ({"guide_id": entry.guide_id},)
    elif fault == "wrong-id":
        entries = (replace(entry, deployment_id="not-an-installation"),)
    elif fault == "wrong-digest":
        entries = (replace(entry, package_sha256="not-a-digest"),)
    elif fault == "wrong-target":
        entries = (replace(entry, runtime_target={}),)
    elif fault == "wrong-gateway":
        entries = (replace(entry, gateway=f.gateway.pin()),)
    else:
        f.gateway.path.write_bytes(b"different binary\n")
    with pytest.raises(CustodyError):
        upgraded_registry(f, newer_target, entries)


@pytest.mark.parametrize("fault", ["tuple", "entry", "gateway-bytes", "gateway-inode"])
async def test_retained_selection_remains_current_after_launcher_selection(
        ordinary_registry, newer_target, fault):
    f = ordinary_registry
    entry = RetainedOrdinaryApplication(str(uuid4()), str(uuid4()), str(uuid4()), "a" * 64, f.target, f.gateway)
    _, _, owner = upgraded_registry(f, newer_target, (entry,))
    if fault == "tuple":
        owner.retained_ordinary_applications = ()
    elif fault == "entry":
        object.__setattr__(entry, "runtime_target", newer_target)
    elif fault == "gateway-bytes":
        f.gateway.path.write_bytes(b"changed held gateway\n")
    else:
        data = f.gateway.path.read_bytes()
        path = f.gateway.path.with_name("replacement")
        path.write_bytes(data)
        path.chmod(0o700)
        assert hashlib.sha256(data).hexdigest() == f.gateway.sha256
        path.replace(f.gateway.path)
    with pytest.raises(CustodyError):
        owner._require_registry()
