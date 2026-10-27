"""Original product source plus relocated inert byte verification.

The real source/artifact carriers and installed pair/full-prefix readers remain
in use. Selected SDK, native and application bytes are deliberately poisoned;
only interpreter-location discovery is replaced with an inert filesystem seam.
These controls confer no installation, setup, readiness or qualification.
"""
import asyncio
import builtins
from contextlib import contextmanager
from dataclasses import replace
import json
import os
from pathlib import Path
import threading
import sys
from types import SimpleNamespace
import zipfile

import pytest

from launchkit import deployment
from launchkit import deployment_worker
from launchkit import remote_product_destination_owner as owner
from launchkit import runtime_dependencies as runtime
from launchkit import runtime_selection
from launchkit.connect_installer_caller import CustodyError
from launchkit.templates import install_candidate as adapter
from launchkit.templates.storage_setup import project_lock
from test_meta_toolkit_runtime_selection import product, no_product_execution, digest
from test_product_artifact_transfer import (
    custody as artifact_custody, call as artifact_call, frozen_receiver, send_complete,
)
from test_product_destination_owner import (
    destination_source, no_destination_execution, package_bytes, stage,
)


def _installed_bytes(selected):
    """Copy only inert specimens, with actual selected modes and confined links."""
    root = selected.destination
    native = root / ".bin"
    native.mkdir(mode=0o700)
    nodes = selected.custody.product.nodes
    for name, (kind, mode, content) in sorted(nodes.items()):
        path = native / name
        if kind == "directory":
            path.mkdir(parents=True, exist_ok=True)
        elif kind == "link":
            path.symlink_to(content)
            if path.lstat().st_mode & 0o777 != mode:
                os.chmod(path, mode, follow_symlinks=False)
        else:
            path.write_bytes(content)
    for name, (kind, mode, _) in nodes.items():
        if kind != "link":
            (native / name).chmod(mode)
    site = root / ".venv/lib/python-fixture/site-packages"
    site.mkdir(parents=True, mode=0o700)
    (root / ".venv").chmod(0o700)
    python = root / ".venv/bin/python"
    python.parent.mkdir(mode=0o700)
    python.write_bytes(b"INERT interpreter must never execute\n")
    python.chmod(0o700)
    for wheel in (selected.custody.product.target.meta_wheel,
            selected.custody.product.target.toolkit_wheel):
        with zipfile.ZipFile(wheel.path) as archive:
            for name in archive.namelist():
                path = site / name
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_bytes(archive.read(name))
                path.chmod(0o600)
    return python, site


@pytest.fixture
def installed_source(destination_source, monkeypatch):
    selected = destination_source
    stage(selected)
    python, site = _installed_bytes(selected)
    calls = []

    def locations(root, sdk_python=None, site_packages=None):
        assert Path(root) == selected.destination
        assert sdk_python is None or Path(sdk_python) == python
        assert site_packages is None or Path(site_packages) == site
        calls.append((Path(root), sdk_python, site_packages))
        return python, site, selected.destination / ".venv/forge-sdk.receipt.json"

    monkeypatch.setattr(adapter, "_sdk_location", locations)
    return SimpleNamespace(selected=selected, python=python, site=site,
        calls=calls, version=selected.custody.product.target.meta.version)


@pytest.fixture
def complete_installed_source(destination_source, monkeypatch):
    selected = destination_source
    # Complete the existing protected candidate source inventory before its
    # first admission. These source members are poisoned and never imported.
    files = {**selected.files,
        "runtime_candidates.py": b"raise RuntimeError('inert accepted reader must never execute')\n",
        "install_candidate.py": b"raise RuntimeError('inert accepted installer must never execute')\n"}
    selected.package, selected.files = package_bytes(files)
    selected.context.update(package_sha256=digest(selected.package),
        package_size=len(selected.package))
    stage(selected)
    python, site = _installed_bytes(selected)
    calls = []

    def locations(root, sdk_python=None, site_packages=None):
        assert Path(root) == selected.destination
        assert sdk_python is None or Path(sdk_python) == python
        assert site_packages is None or Path(site_packages) == site
        calls.append((Path(root), sdk_python, site_packages))
        return python, site, selected.destination / ".venv/forge-sdk.receipt.json"

    monkeypatch.setattr(adapter, "_sdk_location", locations)
    return SimpleNamespace(selected=selected, python=python, site=site,
        calls=calls, version=selected.custody.product.target.meta.version)


@contextmanager
def _source(fixture, require_current=lambda: True):
    selected = fixture.selected
    with project_lock(selected.destination) as lease:
        with owner.open_stopped_product_source(selected.context,
                project_lease=lease, require_current=require_current,
                owner_source=selected.source) as source:
            yield source


def _source_bytes(selected):
    return {name: (selected.destination / name).read_bytes()
        for name in selected.files}


def _proof_has_no_readiness(proof):
    assert proof["source"] == "meta-toolkit-product"
    assert not any(proof.get(key) is True for key in (
        "ready", "installed", "qualified", "native_ready", "setup_completed", "admitted"))


def test_default_local_product_verification_still_reads_the_original_target(installed_source):
    f = installed_source
    before = _source_bytes(f.selected)
    proof = runtime.verify_runtime(f.selected.destination, f.version,
        runtime_target=f.selected.custody.plan.origin,
        sdk_python=f.python, site_packages=f.site)
    _proof_has_no_readiness(proof)
    assert proof["assembly_sha256"] == f.selected.custody.plan.origin.assembly.sha256
    assert proof["catalog_sha256"] == f.selected.custody.plan.origin.model_catalog.sha256
    assert set(proof["sdk"]) == {"meta", "toolkit"}
    assert f.calls
    assert _source_bytes(f.selected) == before


def test_local_product_verification_with_a_kept_inventory_skips_only_the_archive(installed_source, monkeypatch):
    from launchkit import runtime_candidates as candidates
    f = installed_source
    target = f.selected.custody.plan.origin
    first = {}
    proof = runtime.verify_runtime(f.selected.destination, f.version, runtime_target=target,
        sdk_python=f.python, site_packages=f.site, observation=first)
    artifacts = first["artifacts"]
    assert first["inventory_source"] == "archive"
    retained = candidates.RetainedPrefixInventory(artifacts.entries, artifacts.prefix, target.assembly.sha256,
        artifacts.archive.sha256, artifacts.archive.size, proof["prefix_inventory_sha256"], "0" * 64,
        "1" * 64)
    inflated = []
    original = candidates.product_prefix_entries
    def entries(content, prefix):
        inflated.append(prefix)
        return original(content, prefix)
    monkeypatch.setattr(candidates, "product_prefix_entries", entries)
    second = {}
    assert runtime.verify_runtime(f.selected.destination, f.version, runtime_target=target,
        sdk_python=f.python, site_packages=f.site, retained=retained, observation=second) == proof
    assert inflated == [] and second["inventory_source"] == "retained"
    readme = f.selected.destination / ".bin" / artifacts.prefix / "README.md"
    content = readme.read_bytes()
    readme.chmod(0o644)
    readme.write_bytes(bytes([content[0] ^ 1]) + content[1:])
    readme.chmod(0o444)
    with pytest.raises(candidates.CandidateArtifactError, match="native prefix file changed"):
        runtime.verify_runtime(f.selected.destination, f.version, runtime_target=target,
            sdk_python=f.python, site_packages=f.site, retained=retained)
    assert inflated == []
    with pytest.raises(runtime.RuntimeDependencyError, match="selected native product only"):
        runtime.verify_runtime(f.selected.destination, "0.8.48", retained=retained)


@pytest.mark.parametrize("local_disk", [True, False])
def test_local_product_verification_skips_unchanged_files_only_on_a_local_disk(
        installed_source, monkeypatch, local_disk):
    from launchkit import native_prefix_receipt as kept
    from launchkit import runtime_candidates as candidates
    f = installed_source
    target = f.selected.custody.plan.origin
    monkeypatch.setattr(kept, "local_disk_filesystem", lambda path: True)
    first = {}
    proof = runtime.verify_runtime(f.selected.destination, f.version, runtime_target=target,
        sdk_python=f.python, site_packages=f.site, observation=first)
    artifacts = first["artifacts"]
    files = sorted(entry.name for entry in artifacts.entries if entry.kind == "file")
    assert sorted(first["fingerprints"]) == files
    retained = candidates.RetainedPrefixInventory(artifacts.entries, artifacts.prefix, target.assembly.sha256,
        artifacts.archive.sha256, artifacts.archive.size, proof["prefix_inventory_sha256"], "0" * 64,
        "1" * 64, tuple(sorted((name, fingerprint) for name, (fingerprint, _) in first["fingerprints"].items())))
    monkeypatch.setattr(kept, "local_disk_filesystem", lambda path: local_disk)
    prefix = f.selected.destination / ".bin" / artifacts.prefix
    reads = []
    original = adapter.read_owned
    def read_owned(path, *args, **kwargs):
        if prefix in Path(path).parents:
            reads.append(Path(path).relative_to(prefix.parent).as_posix())
        return original(path, *args, **kwargs)
    monkeypatch.setattr(adapter, "read_owned", read_owned)
    second = {}
    assert runtime.verify_runtime(f.selected.destination, f.version, runtime_target=target,
        sdk_python=f.python, site_packages=f.site, retained=retained, observation=second) == proof
    assert second["inventory_source"] == "retained" and second["fingerprints"] == {}
    # The executed gateway is always hashed; the rest only off a local disk.
    assert sorted(reads) == ([artifacts.prefix + "/bin/toolkit_gateway"] if local_disk else files)


def test_relocated_target_alone_cannot_replace_the_accepted_source_identity(installed_source):
    f = installed_source
    with _source(f) as source:
        with pytest.raises(runtime.RuntimeDependencyError, match="admitted source selection"):
            runtime.verify_runtime(f.selected.destination, f.version,
                runtime_target=source.relocated,
                sdk_python=f.python, site_packages=f.site)
        assert source.receipt["status"] == "product-source-staged"
        assert json.loads(source.read_source("runtime-candidate.json")) == f.selected.origin
        assert not f.calls


@pytest.mark.asyncio
@pytest.mark.parametrize("read", ["runtime", "source"])
async def test_original_identity_uses_fresh_local_bytes_and_original_loop_owner(
        complete_installed_source, monkeypatch, read):
    from launchkit.product_runtime_custody import await_product_validation
    f = complete_installed_source
    before = _source_bytes(f.selected)
    loop, task = asyncio.get_running_loop(), asyncio.current_task()
    owner_calls, prefix_calls, sdk_calls = [], [], []

    def current():
        assert asyncio.get_running_loop() is loop
        assert asyncio.current_task() is task
        owner_calls.append(True)

    prefix_reader = adapter.verify_meta_toolkit_prefix
    sdk_reader = adapter.compare_installed_meta_toolkit_wheels

    def prefix(artifacts, **kwargs):
        assert artifacts.selection.artifact_root == f.selected.custody.store / "inputs"
        prefix_calls.append(artifacts.selection)
        return prefix_reader(artifacts, **kwargs)

    def sdk(*facts, **kwargs):
        sdk_calls.append(facts)
        return sdk_reader(*facts, **kwargs)

    monkeypatch.setattr(adapter, "verify_meta_toolkit_prefix", prefix)
    monkeypatch.setattr(adapter, "compare_installed_meta_toolkit_wheels", sdk)
    with _source(f, current) as source:
        # All producer-host paths become unavailable AFTER authentic relocation.
        # The exact transferred records still contain those original path strings.
        original_root = source.origin.artifact_root
        original_root.rename(original_root.with_name(original_root.name + " offline"))
        assert not original_root.exists()
        if read == "runtime":
            result = await await_product_validation(source, runtime.verify_runtime,
                f.selected.destination, f.version, runtime_target=source.origin,
                sdk_python=f.python, site_packages=f.site, source_custody=source)
            _proof_has_no_readiness(result)
            assert prefix_calls and sdk_calls
            assert result["assembly_sha256"] == source.origin.assembly.sha256
            assert result["catalog_sha256"] == source.origin.model_catalog.sha256
        else:
            files = deployment.package_files(f.selected.package)
            result = await await_product_validation(source, deployment.product_runtime_source,
                files, f.selected.origin, source_custody=source)
            assert result.selection == source.relocated
            assert not sdk_calls
        assert owner_calls and source.artifacts.selection == source.relocated
        assert _source_bytes(f.selected) == before
        assert source.receipt["status"] == "product-source-staged"
        assert not hasattr(source, "exact_file")
        assert not any(source.receipt.get(key) is True for key in (
            "ready", "installed", "qualified", "native_ready", "setup_completed", "admitted"))


@pytest.mark.asyncio
@pytest.mark.parametrize("untrusted", ["dict", "duck", "bare"])
async def test_untrusted_origin_local_pair_is_not_a_source_owner(installed_source, untrusted):
    from launchkit.product_runtime_custody import await_product_validation
    f = installed_source
    with _source(f) as source:
        values = {"origin": source.origin, "relocated": source.relocated,
            "artifacts": source.artifacts, "receipt": source.receipt}
        alleged = values if untrusted == "dict" else SimpleNamespace(**values)
        if untrusted == "bare":
            alleged = object.__new__(owner.StoppedProductSource)
        with pytest.raises((ValueError, RuntimeError)):
            await await_product_validation(alleged, runtime.verify_runtime,
                f.selected.destination, f.version, runtime_target=source.origin,
                sdk_python=f.python, site_packages=f.site, source_custody=alleged)
        assert not f.calls
        source.require_current()


@pytest.mark.asyncio
async def test_actual_carrier_cannot_substitute_a_different_origin(installed_source):
    from launchkit.product_runtime_custody import await_product_validation
    f = installed_source
    with _source(f) as source:
        another = replace(source.origin, model_catalog=replace(source.origin.model_catalog,
            path=source.origin.model_catalog.path.with_name("another-catalog.json")))
        with pytest.raises((ValueError, RuntimeError)):
            await await_product_validation(source, runtime.verify_runtime,
                f.selected.destination, f.version, runtime_target=another,
                sdk_python=f.python, site_packages=f.site, source_custody=source)
        assert not f.calls
        assert json.loads(source.read_source("runtime-candidate.json")) == f.selected.origin


@pytest.mark.asyncio
async def test_product_source_custody_cannot_enable_an_unselected_ordinary_runtime(installed_source):
    from launchkit.product_runtime_custody import await_product_validation
    f = installed_source
    before = _source_bytes(f.selected)
    with _source(f) as source:
        with pytest.raises(runtime.RuntimeDependencyError, match="original admitted native product"):
            await await_product_validation(source, runtime.verify_runtime,
                f.selected.destination, "0.8.48", source_custody=source)
        assert not f.calls and _source_bytes(f.selected) == before


@pytest.mark.asyncio
@pytest.mark.parametrize("changed", ["app", "guide", "digest", "mode", "inventory"])
async def test_source_a_cannot_validate_other_application_bytes_with_same_runtime(
        complete_installed_source, changed):
    from launchkit.product_runtime_custody import await_product_validation
    f = complete_installed_source
    files = deployment.package_files(f.selected.package)
    if changed == "app":
        files["app.py"] = (b"raise RuntimeError('another application, never execute')\n", 0o644)
    elif changed in {"guide", "digest"}:
        report = json.loads(files["implementation-report.json"][0])
        if changed == "guide":
            report["guide_id"] = "99999999-9999-4999-8999-999999999999"
        else:
            report["verification"]["digest"] = "d" * 64
        files["implementation-report.json"] = (json.dumps(report).encode(), 0o644)
    elif changed == "mode":
        files["config/access.toml"] = (files["config/access.toml"][0], 0o600)
    else:
        files["another-owned-source.txt"] = (b"foreign complete package inventory\n", 0o644)
    # Rebind the ordinary ZIP checksum surface so refusal cannot rely only on a
    # stale checksum file. The exact source carrier must own these app bytes.
    files["SHA256SUMS"] = (b"".join((digest(content) + "  " + name + "\n").encode()
        for name, (content, _) in sorted(files.items()) if name != "SHA256SUMS"), 0o644)
    assert json.loads(files["runtime-candidate.json"][0]) == f.selected.origin
    before = _source_bytes(f.selected)
    with _source(f) as source:
        message = ("exact full accepted application source" if changed == "inventory"
            else "changed accepted source bytes or modes")
        with pytest.raises(CustodyError, match=message):
            await await_product_validation(source, deployment.product_runtime_source,
                files, f.selected.origin, source_custody=source)
        assert not f.calls
        assert _source_bytes(f.selected) == before


@pytest.mark.asyncio
async def test_another_destination_is_not_authorized_by_equal_product_identity(
        installed_source, tmp_path):
    from launchkit.product_runtime_custody import await_product_validation
    f = installed_source
    foreign = tmp_path.resolve() / "unowned-by-this-carrier"
    foreign.mkdir(mode=0o700)
    before = _source_bytes(f.selected)
    with _source(f) as source:
        with pytest.raises((ValueError, RuntimeError)):
            await await_product_validation(source, runtime.verify_runtime,
                foreign, f.version, runtime_target=source.origin,
                sdk_python=f.python, site_packages=f.site, source_custody=source)
        assert not f.calls and not any(foreign.iterdir())
        assert _source_bytes(f.selected) == before


@pytest.mark.asyncio
@pytest.mark.parametrize("alleged", ["dict", "origin-artifacts"])
async def test_existing_source_owner_does_not_authenticate_a_foreign_artifact_proof(
        complete_installed_source, alleged):
    from launchkit.product_runtime_custody import await_product_validation
    f = complete_installed_source
    with _source(f) as source:
        artifacts = ({"selection": source.relocated, "archive": source.artifacts.archive}
            if alleged == "dict" else runtime_selection.selected_runtime_artifacts(source.origin))
        with pytest.raises(CustodyError, match="cannot substitute another artifact observation"):
            await await_product_validation(source, deployment.product_runtime_source,
                deployment.package_files(f.selected.package), f.selected.origin,
                verified_artifacts=artifacts, source_custody=source)
        assert not f.calls
        source.require_current()


@pytest.mark.asyncio
@pytest.mark.parametrize("changed", ["source", "artifact", "lease", "host-path", "owner-binding", "package"])
async def test_runtime_join_refuses_changed_actual_source_custody_before_installed_reads(
        installed_source, changed):
    from launchkit.product_runtime_custody import await_product_validation
    f = installed_source
    with pytest.raises((ValueError, OSError, RuntimeError)):
        with _source(f) as source:
            if changed == "source":
                path = f.selected.destination / "app.py"
                path.write_bytes(path.read_bytes() + b"# changed after source admission\n")
            elif changed == "artifact":
                path = source.relocated.assembly.path
                path.write_bytes(path.read_bytes() + b"changed after local admission")
            elif changed == "lease":
                (f.selected.destination / ".mobkit/launch.lock").chmod(0o644)
            elif changed == "host-path":
                old = f.selected.destination
                retained = old.with_name(old.name + " original bytes")
                old.rename(retained)
                old.symlink_to(retained, target_is_directory=True)
            else:
                store = f.selected.custody.home / ".local/state/meta-forge/product-sources" / \
                    f.selected.context["deployment_id"]
                path = store / ("owner.json" if changed == "owner-binding" else "package.zip")
                if changed == "owner-binding":
                    value = json.loads(path.read_bytes())
                    value["deployment_id"] = "99999999-9999-4999-8999-999999999999"
                    path.write_bytes(json.dumps(value).encode())
                else:
                    path.write_bytes(path.read_bytes() + b"another accepted package claim")
            await await_product_validation(source, runtime.verify_runtime,
                f.selected.destination, f.version, runtime_target=source.origin,
                sdk_python=f.python, site_packages=f.site, source_custody=source)
            pytest.fail("Changed original source custody produced a runtime proof.")
    assert not f.calls


@pytest.mark.asyncio
@pytest.mark.parametrize("changed", ["prefix", "sdk"])
async def test_late_selected_installed_byte_change_cannot_return_a_completed_proof(
        installed_source, monkeypatch, changed):
    from launchkit.product_runtime_custody import await_product_validation
    f = installed_source
    compare = adapter.compare_installed_meta_toolkit_wheels
    calls = []

    def completed_pair(*args, **kwargs):
        result = compare(*args, **kwargs)
        calls.append(result)
        # Both selected SDKs and the full prefix were genuinely checked already.
        # Changing either now must invalidate this same physical proof.
        path = (f.site / "meerkat_meta/agent_builder.py" if changed == "sdk" else
            f.selected.destination / ".bin" /
            next(name for name in f.selected.custody.product.nodes if name.endswith("/bin/toolkit_gateway")))
        path.write_bytes(path.read_bytes() + b"changed after the byte comparison\n")
        return result

    monkeypatch.setattr(adapter, "compare_installed_meta_toolkit_wheels", completed_pair)
    before = _source_bytes(f.selected)
    with _source(f) as source:
        with pytest.raises((ValueError, OSError, RuntimeError)):
            await await_product_validation(source, runtime.verify_runtime,
                f.selected.destination, f.version, runtime_target=source.origin,
                sdk_python=f.python, site_packages=f.site, source_custody=source)
        assert len(calls) == 1
        assert _source_bytes(f.selected) == before
        assert source.receipt["status"] == "product-source-staged"


async def _started(event):
    assert await asyncio.wait_for(asyncio.to_thread(event.wait, 5), timeout=6), \
        "The actual byte worker did not reach the retained comparison boundary."


@pytest.mark.asyncio
@pytest.mark.parametrize("changed", ["source", "lease", "owner"])
async def test_original_owner_and_file_fences_rejoin_after_suspended_byte_work(
        installed_source, monkeypatch, changed):
    from launchkit.product_runtime_custody import await_product_validation
    f = installed_source
    started, release, finished = threading.Event(), threading.Event(), threading.Event()
    compare = adapter.compare_installed_meta_toolkit_wheels
    current, owner_calls, held = {"value": True}, [], {}
    loop = asyncio.get_running_loop()

    def suspended_pair(*args, **kwargs):
        started.set()
        try:
            assert release.wait(6), "The fixture did not release its original byte worker."
            return compare(*args, **kwargs)
        finally:
            finished.set()

    monkeypatch.setattr(adapter, "compare_installed_meta_toolkit_wheels", suspended_pair)

    async def original_operation():
        task = asyncio.current_task()
        def owner_current():
            assert asyncio.get_running_loop() is loop and asyncio.current_task() is task
            owner_calls.append(True)
            return current["value"]
        with _source(f, owner_current) as source:
            held["source"] = source
            return await await_product_validation(source, runtime.verify_runtime,
                f.selected.destination, f.version, runtime_target=source.origin,
                sdk_python=f.python, site_packages=f.site, source_custody=source)

    pending = asyncio.create_task(original_operation())
    try:
        await _started(started)
        if changed == "source":
            path = f.selected.destination / "application/tool-policy.json"
            path.write_bytes(path.read_bytes() + b"changed while selected byte work waited")
        elif changed == "lease":
            (f.selected.destination / ".mobkit/launch.lock").chmod(0o644)
        else:
            current["value"] = False
        release.set()
        with pytest.raises((ValueError, OSError, RuntimeError)):
            await asyncio.wait_for(asyncio.shield(pending), timeout=7)
        assert owner_calls and finished.is_set() and pending.done()
        with pytest.raises((ValueError, RuntimeError)):
            held["source"].require_owner_current()
    finally:
        release.set()
        if not pending.done():
            pending.cancel()
            await asyncio.gather(pending, return_exceptions=True)


@pytest.mark.asyncio
async def test_cancelled_verification_drains_the_same_byte_worker_before_releasing_source(
        installed_source, monkeypatch):
    from launchkit.product_runtime_custody import await_product_validation
    f = installed_source
    started, release, finished = threading.Event(), threading.Event(), threading.Event()
    compare = adapter.compare_installed_meta_toolkit_wheels
    owner_calls, held = [], {}
    loop = asyncio.get_running_loop()
    before = _source_bytes(f.selected)

    def suspended_pair(*args, **kwargs):
        started.set()
        try:
            assert release.wait(6), "The fixture did not release its original byte worker."
            return compare(*args, **kwargs)
        finally:
            finished.set()

    monkeypatch.setattr(adapter, "compare_installed_meta_toolkit_wheels", suspended_pair)

    async def original_operation():
        task = asyncio.current_task()
        def current():
            assert asyncio.get_running_loop() is loop and asyncio.current_task() is task
            owner_calls.append(True)
        with _source(f, current) as source:
            held["source"] = source
            return await await_product_validation(source, runtime.verify_runtime,
                f.selected.destination, f.version, runtime_target=source.origin,
                sdk_python=f.python, site_packages=f.site, source_custody=source)

    pending = asyncio.create_task(original_operation())
    try:
        await _started(started)
        pending.cancel()
        await asyncio.sleep(0)
        assert not pending.done() and not finished.is_set()
        # The original lease is still held until the same worker settles.
        with pytest.raises(RuntimeError):
            with project_lock(f.selected.destination):
                pytest.fail("Cancellation released source custody before its worker drained.")
        release.set()
        with pytest.raises(asyncio.CancelledError):
            await asyncio.wait_for(asyncio.shield(pending), timeout=7)
        assert finished.is_set() and pending.cancelled() and owner_calls
        assert _source_bytes(f.selected) == before
        with project_lock(f.selected.destination):
            pass
        with pytest.raises((ValueError, RuntimeError)):
            held["source"].require_owner_current()
    finally:
        release.set()
        if not pending.done():
            pending.cancel()
            await asyncio.gather(pending, return_exceptions=True)


@pytest.mark.asyncio
async def test_cancelled_product_mutation_stops_before_its_same_prefix_worker_drains(
        destination_source):
    f = SimpleNamespace(selected=destination_source)
    stage(f.selected)
    before = _source_bytes(f.selected)
    loop = asyncio.get_running_loop()
    supervisor = SimpleNamespace(stop=asyncio.Event())
    started, release, finished, refused = (threading.Event() for _ in range(4))
    owner_calls, file_calls = [], []

    async def original_operation():
        task = asyncio.current_task()
        def original_owner():
            assert asyncio.get_running_loop() is loop and asyncio.current_task() is task
            owner_calls.append(True)
        with _source(f, original_owner) as source:
            def file_current():
                file_calls.append(True)
                if len(file_calls) == 2:
                    # Hold the real prefix installer before its first entry
                    # write, after its real archive byte/inventory comparison.
                    started.set()
                    assert release.wait(6), "The fixture did not release its original prefix worker."
                if supervisor.stop.is_set():
                    refused.set()
                    raise ValueError("The original product mutation owner stopped.")
                source.require_filesystem_current()
            def prefix_worker():
                try:
                    return adapter.install_meta_toolkit_prefix(source.artifacts,
                        deployment_root=f.selected.destination, require_current=file_current)
                finally:
                    finished.set()
            return await deployment_worker._joined_product_mutation(supervisor,
                prefix_worker, _owner_current=source.require_owner_current)

    pending = asyncio.create_task(original_operation())
    try:
        await _started(started)
        pending.cancel()
        await asyncio.wait_for(supervisor.stop.wait(), timeout=30)
        assert not pending.done() and not finished.is_set()
        with pytest.raises(RuntimeError):
            with project_lock(f.selected.destination):
                pytest.fail("The original prefix worker lost its held source lease.")
        release.set()
        with pytest.raises(asyncio.CancelledError):
            await asyncio.wait_for(asyncio.shield(pending), timeout=7)
        assert refused.is_set() and finished.is_set() and pending.cancelled()
        assert len(file_calls) == 2 and owner_calls
        assert (f.selected.destination / ".bin").is_dir()
        assert not any((f.selected.destination / ".bin").iterdir())
        assert _source_bytes(f.selected) == before
        with project_lock(f.selected.destination):
            pass
    finally:
        release.set()
        if not pending.done():
            pending.cancel()
            await asyncio.gather(pending, return_exceptions=True)


def test_actual_frozen_receiver_default_path_needs_no_optional_source_owner_helper(
        artifact_custody, monkeypatch):
    selected = artifact_custody
    original = {pin.path: pin.path.read_bytes() for pin in selected.plan.files}
    importer = builtins.__import__

    def no_optional_helper(name, *args, **kwargs):
        assert "product_runtime_custody" not in name, \
            "Default frozen intake imported an unavailable optional host-owner helper."
        return importer(name, *args, **kwargs)

    with frozen_receiver(selected.source) as frozen:
        # This is the existing trusted explicit bundle, whose package path is
        # empty. It cannot reach production or received Python to fill a gap.
        sys.modules.pop("launchkit.product_runtime_custody", None)
        monkeypatch.setattr(builtins, "__import__", no_optional_helper)
        assert sys.modules["launchkit"].__path__ == []
        send_complete(selected, module=frozen)
        result = artifact_call(selected, "finish", module=frozen)
        relocated = json.loads((selected.store / "selection.json").read_bytes())
        assert result["status"] == "runtime-artifacts-transferred"
        assert "launchkit.product_runtime_custody" not in sys.modules
        assert relocated["format"] == runtime_selection.PRODUCT_SELECTION_FORMAT
        assert not Path(selected.host["destination"]).exists()
    assert all(path.read_bytes() == content for path, content in original.items())
    assert not any(result.get(key) is True for key in (
        "ready", "installed", "qualified", "native_ready", "setup_completed", "admitted"))
