"""Connected references with real installed-source custody and inert SDK bytes.

The existing destination Install fixture publishes its actual canonical receipt
and owned source, while its selected SDK/application commands remain inert.
Fresh/Restore specimens below exercise source and history readers only, never
native completion, provider work or a running application.
"""
from contextlib import asynccontextmanager
import json
import os

import pytest

from launchkit import deployment, deployment_application_bootstrap as bootstrap
from launchkit import product_installed_source as installed
from launchkit import remote_product_destination_owner as owner
from launchkit.connect_application_startup import FreshApplicationStartup, RestoredApplicationStartup
from launchkit.connect_installer_caller import CustodyError
from launchkit.projects import DraftConflict
from launchkit.remote_product_artifacts import ProductArtifactTransferError
from launchkit.templates.connectors import ConnectorTools
from launchkit.templates.storage_setup import project_lock
import test_product_destination_install as install_fixture
from test_meta_toolkit_runtime_selection import product, no_product_execution
from test_product_artifact_transfer import custody as artifact_custody
from test_product_destination_owner import destination_source, no_destination_execution
from test_product_installed_bootstrap import installed_bootstrap, _completed_output_specimen


@pytest.fixture
def installation(destination_source, monkeypatch, request):
    """Add the shipping workspace contract before the original first stage."""
    original = install_fixture.setup_spec
    granted = getattr(request, "param", True)

    def spec():
        value = original()
        for row in value["agent_classes"]:
            row["tools"]["workspace"] = False
        value["agent_classes"][0]["tools"]["workspace"] = granted
        return value

    monkeypatch.setattr(install_fixture, "setup_spec", spec)
    destination_source.files = {**destination_source.files,
        "workspace/START-HERE.txt":
            b"Place text references here. Only classes with workspace access may read them.\n"}
    # Reuse the original complete package, actual host and Supervisor fixture.
    # Its own builder reseals this source before the first real source intake.
    return install_fixture.installation.__wrapped__(destination_source, monkeypatch, request)


@asynccontextmanager
async def source_scope(f, selection=None):
    with project_lock(f.selected.destination) as lease:
        async with installed.open_installed_product_source(f.directory,
                project_lease=lease, require_current=lease.require_current,
                owner_source=f.selected.source, application_startup=selection) as source:
            assert source is not None and source._lease is lease
            yield source
        assert source._closed
    assert lease._closed


def settings(f, **changes):
    values, _ = deployment.private_configuration_snapshot(f.selected.destination)
    values.update(changes)
    path = f.selected.destination / ".env"
    path.write_text(deployment._serialized_private_configuration(values))
    path.chmod(0o600)


def reference(f, relative="museum/object.txt"):
    path = f.selected.destination / "workspace" / relative
    path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
    path.write_text("An original reference, not application source.\n")
    path.chmod(0o600)
    return path


def unchanged_package(f, original):
    assert (f.directory / "package.zip").read_bytes() == original
    assert all((f.selected.destination / name).read_bytes() == content
        for name, content in f.selected.files.items())


@pytest.mark.parametrize("configured", [False, True], ids=["omitted", "blank"])
async def test_default_references_remain_data_before_setup_selection(
        installed_bootstrap, monkeypatch, configured):
    f = installed_bootstrap
    original = (f.directory / "package.zip").read_bytes()
    if configured:
        settings(f, FILES_ROOT="")
    else:
        assert "FILES_ROOT" not in deployment.private_configuration_snapshot(f.selected.destination)[0]
    text = reference(f)
    monkeypatch.delenv("FILES_ROOT", raising=False)
    tools = ConnectorTools(f.selected.destination, {})
    assert tools.files_root() == f.selected.destination / "workspace"
    assert tools.files_read({"path": "museum/object.txt"})["content"] == text.read_text()
    async with source_scope(f) as source:
        assert "workspace/museum/object.txt" not in source.files
        assert source.read_source("workspace/START-HERE.txt") == f.selected.files["workspace/START-HERE.txt"]
        source.require_current()
    unchanged_package(f, original)


async def test_default_reference_survives_original_fresh_lineage_and_restore_reopening(
        installed_bootstrap):
    f = installed_bootstrap
    settings(f, FILES_ROOT="")
    text = reference(f)
    package = (f.directory / "package.zip").read_bytes()
    receipt = (f.directory / "receipt.json").read_bytes()
    factory = bootstrap.InstalledBootstrapFactory(f.directory, f.pin,
        lifecycle_id=f.lifecycle_id, owner_source=f.selected.source)
    with project_lock(f.selected.destination) as lease:
        async with factory(lease) as startup:
            assert isinstance(startup, FreshApplicationStartup)
            await startup.prepare(f.bootstrap.app())
            assert startup.runtime is None
            _completed_output_specimen(f)
            factory.source.require_current()
    entry = (f.directory / bootstrap.FRESH_ENTRY_NAME).read_bytes()
    async with source_scope(f, f.bootstrap.restore) as source:
        pin, _ = bootstrap.retain_bootstrap_admission(f.directory,
            deployment.read_receipt(f.directory), f.bootstrap.restore, source_custody=source)
        source.require_current()
    restored = bootstrap.InstalledBootstrapFactory(f.directory, pin,
        lifecycle_id=f.lifecycle_id, owner_source=f.selected.source)
    with project_lock(f.selected.destination) as lease:
        async with restored(lease) as startup:
            assert isinstance(startup, RestoredApplicationStartup)
            await startup.prepare(f.bootstrap.app())
            assert startup.runtime is None
            assert text.read_text() == "An original reference, not application source.\n"
            restored.source.require_current()
    assert (f.directory / bootstrap.FRESH_ENTRY_NAME).read_bytes() == entry
    assert (f.directory / "receipt.json").read_bytes() == receipt
    unchanged_package(f, package)


@pytest.mark.parametrize("change", ["bytes", "mode"])
async def test_packaged_workspace_member_keeps_exact_original_source_protection(
        installed_bootstrap, change):
    f = installed_bootstrap
    package = (f.directory / "package.zip").read_bytes()
    reference(f)
    path = f.selected.destination / "workspace/START-HERE.txt"
    if change == "bytes":
        path.write_bytes(b"Changed packaged source.\n")
    else:
        path.chmod(0o600)
    error = DraftConflict if change == "bytes" else ProductArtifactTransferError
    detail = "Installed source differs from the checked package" if change == "bytes" else "changed ownership, size or access"
    with pytest.raises(error, match=detail):
        async with source_scope(f, f.bootstrap.fresh):
            pytest.fail("Connected data discarded an original packaged member's protection")
    assert (f.directory / "package.zip").read_bytes() == package


@pytest.mark.parametrize("relative", ["application/new.py", "assets/new.txt", "new-root.txt"])
async def test_connected_input_role_never_admits_new_application_source(
        installed_bootstrap, relative):
    f = installed_bootstrap
    reference(f)
    package = (f.directory / "package.zip").read_bytes()
    path = f.selected.destination / relative
    path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
    path.write_bytes(b"Unselected application source, not a connected reference.\n")
    path.chmod(0o600)
    with pytest.raises(CustodyError, match="missing or unselected application files"):
        async with source_scope(f, f.bootstrap.fresh):
            pytest.fail("Workspace input authority extended to application source")
    unchanged_package(f, package)


async def test_connected_reference_link_is_not_an_owned_ordinary_input(installed_bootstrap):
    f = installed_bootstrap
    target = f.selected.destination / "workspace/reference-link.txt"
    target.symlink_to(f.selected.destination / "app.py")
    with pytest.raises(CustodyError, match="linked or unsafe application file"):
        async with source_scope(f, f.bootstrap.fresh):
            pytest.fail("A linked input entered installed source custody")


@pytest.mark.parametrize("installation", [False], indirect=True)
async def test_workspace_scaffold_without_actual_grant_stays_source(installed_bootstrap):
    f = installed_bootstrap
    definition = json.loads(f.selected.files["deployment.json"])
    assert not any(row["tools"]["workspace"] for row in definition["agents"])
    assert not any(row["key"] == "FILES_ROOT" for row in definition["environment"])
    reference(f)
    with pytest.raises(CustodyError, match="missing or unselected application files"):
        async with source_scope(f, f.bootstrap.fresh):
            pytest.fail("A scaffold alone supplied a workspace grant")


async def test_switching_to_external_connected_folder_preserves_original_workspace_data(
        installed_bootstrap, tmp_path, monkeypatch):
    f = installed_bootstrap
    original = (f.directory / "package.zip").read_bytes()
    retained = reference(f)
    async with source_scope(f, f.bootstrap.fresh) as source:
        source.require_current()
    external = tmp_path / "operator references"
    external.mkdir(mode=0o700)
    text = external / "museum.txt"
    text.write_text("Existing external connected references.\n")
    settings(f, FILES_ROOT=str(external))
    monkeypatch.setenv("FILES_ROOT", str(external))
    tools = ConnectorTools(f.selected.destination, {})
    assert tools.files_read({"path": "museum.txt"})["content"] == text.read_text()
    async with source_scope(f, f.bootstrap.fresh) as source:
        assert source._installed_view.reference_root == f.selected.destination / "workspace"
        # The connected folder is outside the accepted application closure.
        (external / "later.txt").write_text("A later external reference.\n")
        source.require_current()
    assert text.read_text() == "Existing external connected references.\n"
    assert retained.read_text() == "An original reference, not application source.\n"
    unchanged_package(f, original)


async def test_held_reference_role_rechecks_original_private_settings(installed_bootstrap):
    f = installed_bootstrap
    reference(f)
    with pytest.raises(ProductArtifactTransferError,
            match=r"^The original stopped source lease is no longer current\.$"):
        async with source_scope(f, f.bootstrap.fresh) as source:
            lease = source._lease
            original = source._installed_view._configuration
            settings(f, FILES_ROOT=str(f.selected.destination / "assets"))
            changed = (f.selected.destination / ".env").read_bytes()
            with pytest.raises(CustodyError, match="connected inputs changed their original private configuration"):
                source.require_current()
            assert source._invalidated
            assert source._installed_view._configuration == original
    assert source._closed and lease._closed
    assert (f.selected.destination / ".env").read_bytes() == changed
    with project_lock(f.selected.destination) as renewed:
        assert renewed is not lease
        renewed.require_current()


async def test_staged_source_never_inherits_installed_connected_input_role(installed_bootstrap):
    f = installed_bootstrap
    reference(f)
    with project_lock(f.selected.destination) as lease:
        with pytest.raises(ProductArtifactTransferError, match="missing or unselected application files"):
            with owner.open_stopped_product_source(f.selected.context, project_lease=lease,
                    require_current=lease.require_current, owner_source=f.selected.source):
                pytest.fail("A staged source admitted installed reference data")


def native_context_specimen(root):
    """Only inert native-output locations; no native state or authority."""
    for name in ("shared", "shared/data", "shared/control", "shared/data/toolkit-native",
                 "shared/data/toolkit-native/inert-realm"):
        (root / name).mkdir(mode=0o700, exist_ok=True)
    files = [root / "shared/data/toolkit-native/inert-realm/originals.sqlite3",
             root / "shared/control/deny.lock", root / "shared/control/deny.json"]
    for path in files:
        path.write_bytes(b"inert bytes, never interpreted")
        path.chmod(0o644)
    return files


async def test_native_context_outputs_are_allowed_before_finalizer_inputs(installed_bootstrap):
    f = installed_bootstrap
    package = (f.directory / "package.zip").read_bytes()
    outputs = native_context_specimen(f.selected.destination)
    async with source_scope(f) as source:
        assert source._installed_view.inputs is None
        source.require_filesystem_current()
        assert all(path.read_bytes() == b"inert bytes, never interpreted" for path in outputs)
    unchanged_package(f, package)


@pytest.mark.parametrize("change", ["native-link", "file-link", "hardlink", "writable", "realm-mode", "parent-mode", "unknown-data"])
async def test_native_output_allowance_does_not_admit_unsafe_or_other_shared_files(installed_bootstrap, change):
    f = installed_bootstrap
    root = f.selected.destination
    outputs = native_context_specimen(root)
    path = outputs[0]
    if change == "native-link":
        path.unlink()
        (path.parent).rmdir()
        (root / "shared/data/toolkit-native").rmdir()
        (root / "shared/data/toolkit-native").symlink_to(root / "application", target_is_directory=True)
    elif change == "file-link":
        path.unlink()
        path.symlink_to(root / "app.py")
    elif change == "hardlink":
        os.link(path, path.parent / "second-name")
    elif change == "writable":
        path.chmod(0o666)
    elif change == "realm-mode":
        path.parent.chmod(0o755)
    elif change == "parent-mode":
        (root / "shared/control").chmod(0o755)
    else:
        (root / "shared/data/unselected.txt").write_bytes(b"unselected shared source")
    with pytest.raises((CustodyError, ProductArtifactTransferError)):
        async with source_scope(f):
            pytest.fail("Unsafe native or unrelated shared file was accepted")


@pytest.mark.parametrize("name", ["shared/data/toolkit-native/owner.json", "shared/control/deny.lock", "shared/control/deny.json", "shared/data"])
def test_native_output_allowance_refuses_overlap_with_accepted_application_source(installed_bootstrap, name):
    f = installed_bootstrap
    package = {**deployment.package_files((f.directory / "package.zip").read_bytes()),
        name: (b"inert authored source", 0o644)}
    with pytest.raises(CustodyError, match="overlap"):
        installed._InstalledSourceView(f.directory, None, package, root=f.selected.destination)


@pytest.mark.parametrize("name", ["shared/data/toolkit-native-notes.json", "shared/control/deny.json-notes"])
def test_native_output_allowance_keeps_distinct_component_siblings(installed_bootstrap, name):
    f = installed_bootstrap
    package = {**deployment.package_files((f.directory / "package.zip").read_bytes()),
        name: (b"inert authored sibling source", 0o644)}
    view = installed._InstalledSourceView(f.directory, None, package, root=f.selected.destination)
    assert view.native_context == (f.selected.destination / "shared/data/toolkit-native",
        f.selected.destination / "shared/control/deny.lock", f.selected.destination / "shared/control/deny.json")
