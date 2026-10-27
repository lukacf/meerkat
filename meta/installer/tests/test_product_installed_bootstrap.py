"""Installed destination bootstrap custody with actual production source readers.

The canonical receipt comes from the real destination byte-install entry. SDK,
authored application and native calls remain inert. These controls establish
source/lease composition only, never native preparation or useful-work success.
"""
import asyncio
from contextlib import asynccontextmanager, contextmanager
from copy import deepcopy
import errno
import json
import os
from pathlib import Path
import shutil
import sys
from types import ModuleType, SimpleNamespace

import pytest

from launchkit import deployment, deployment_application_bootstrap as bootstrap
from launchkit import deployment_worker as worker, runtime_selection
from launchkit import product_installed_source as installed
from launchkit import remote_interface_bundle as bundles
from launchkit import remote_product_destination_owner as owner
from launchkit.connect_application_startup import RestoredApplicationStartup
from launchkit.connect_installer_caller import CustodyError, ExactFile, ProductGatewayFile
from launchkit.connect_policy_input import application_tool_policy_selection
from launchkit.templates.storage_setup import project_lock
from test_deployment_application_bootstrap import Inputs, inputs, no_runtime_operations, write
from test_meta_toolkit_runtime_selection import product, no_product_execution
from test_product_artifact_transfer import custody as artifact_custody
from test_product_destination_install import installation, selected_host
from test_product_destination_owner import destination_source
from test_application_startup import factory_host


@pytest.fixture
async def installed_bootstrap(installation, monkeypatch):
    f = installation
    # The production opener uses the actual destination account's home.
    monkeypatch.setattr(Path, "home", classmethod(lambda cls: f.selected.custody.home))
    with selected_host(f):
        await f.host.install_staged_source(settings=f.settings, require_current=f.current_owner)
        receipt = deployment.read_receipt(f.directory)
        assert receipt["status"] == "stopped" and receipt["phase"] == "configure"
        receipt = deployment.update_receipt(f.directory,
            interactive_lifecycle_id="00000000-0000-4000-8000-000000000003")
        f.lifecycle_id = receipt["interactive_lifecycle_id"]
        selected = Inputs(f.directory, receipt, f.selected.destination, monkeypatch)
        # Inputs also builds an unrelated Restore specimen. This Fresh-only
        # control does not fabricate completion or leave unselected root files.
        (f.selected.destination / "current").unlink()
        (f.selected.destination / "original-activation.json").unlink()
        shutil.rmtree(f.selected.destination / "original-bundle")
        definition = f.selected.files["config/mob.toml"].decode()
        mob_id = bootstrap._source_definition({"format": "toml", "text": definition})["id"]
        deployed = json.loads(f.selected.files["deployment.json"])
        roster = [{"identity": row["identity"], "profile": row["profile"]}
            for row in deployed["agents"]]
        members = [{"member": {"mob_id": mob_id, "role": row["profile"],
            "member": "inert-input-member-" + str(i)}} for i, row in enumerate(roster)]
        raw_policy = f.selected.files["application/tool-policy.json"]
        policy = application_tool_policy_selection(raw_policy,
            selected_member_identities=tuple(row["identity"] for row in roster))
        identities = {row["identity"]: member["member"]["member"]
            for row, member in zip(roster, members)}
        for row in policy["members"]:
            row["member_identity"] = identities[row["member_identity"]]
        policy["source"] = {"source_id": receipt["build_id"],
            "source_digest": "sha256:" + bootstrap.updates.digest(raw_policy)}
        selected.plan_value["application"].update(definition={"format": "toml", "text": definition},
            roster=roster, members=members, helper=members[0], tool_policy=policy)
        selected.plan = write(selected.plan.path, selected.plan_value)
        selected.policy = write(selected.policy.path, raw_policy)
        selected.gateway = ExactFile(f.selected.destination / ".bin" / f.source.artifacts.prefix
            / "bin/toolkit_gateway", f.source.artifacts.candidate["binary"]["sha256"])
        selected.fresh.update(preparation_plan=selected.plan.pin(), source_policy=selected.policy.pin(),
            gateway=selected.gateway.pin())
        f.pin, observation = bootstrap.retain_bootstrap_admission(f.directory, receipt,
            selected.fresh, source_custody=f.source)
        assert type(observation.gateway) is ProductGatewayFile
        f.bootstrap = selected
        f.parent_source, f.parent_lease = f.source, f.source._lease
    assert f.parent_source._closed
    with pytest.raises(RuntimeError):
        f.parent_lease.require_current()
    f.store = f.selected.custody.home / ".local/state/meta-forge/product-sources" / receipt["deployment_id"]
    f.imports = []
    # Establish a complete real installed baseline before any mutation. A
    # missing account intake must not make a later negative case pass early.
    with project_lock(f.selected.destination) as lease:
        async with installed.open_installed_product_source(f.directory,
                project_lease=lease, require_current=f.current_owner,
                owner_source=f.selected.source, application_startup=f.bootstrap.fresh) as source:
            assert source is not None and source.origin == f.parent_source.origin
            assert source.relocated == f.parent_source.relocated
            assert set(source.files) == set(f.selected.files)
            assert source._store == f.store
            assert lease.root == f.selected.destination and not lease._closed
        assert source._closed
    assert lease._closed
    return f


def _receipt(f, **changes):
    value = deployment.read_receipt(f.directory)
    value.update(changes)
    deployment.write_private(f.directory / "receipt.json", value)


def _forbid_imports(monkeypatch, f):
    def forbidden(name):
        f.imports.append(name)
        pytest.fail("Unowned installed source reached an SDK or authored import: " + name)
    monkeypatch.setattr(bootstrap.importlib, "import_module", forbidden)
    monkeypatch.setattr(sys, "path", list(sys.path))
    monkeypatch.setattr(sys, "argv", list(sys.argv))


def _forbid_producer_reads(monkeypatch, f):
    original = runtime_selection.selected_runtime_artifacts
    reads = []

    def selected(target, *args, **kwargs):
        if target == f.parent_source.origin and kwargs.get("source_custody") is None:
            reads.append(target)
            pytest.fail("Lost destination intake fell back to original producer locators")
        return original(target, *args, **kwargs)

    monkeypatch.setattr(runtime_selection, "selected_runtime_artifacts", selected)
    return reads


def _child_host(f, tmp_path, monkeypatch, run):
    """Use maintained app.main and its real lease; replace external run only."""
    app = factory_host(tmp_path / "maintained-entrypoint", monkeypatch)
    app.__file__ = str(f.selected.destination / "app.py")
    monkeypatch.setattr(app, "ROOT", f.selected.destination)
    monkeypatch.setattr(app, "load_spec", lambda: json.loads(f.selected.files["deployment.json"]))
    monkeypatch.setattr(app, "load_environment", lambda *args, **kwargs: None)
    monkeypatch.setattr(app, "run", run)
    f.opened, f.factories = [], []
    original_open = installed.open_installed_product_source
    original_factory = bootstrap.BootstrapFactory.__init__

    @asynccontextmanager
    async def observe_open(*args, **kwargs):
        async with original_open(*args, **kwargs) as source:
            if source is not None:
                f.opened.append((source, kwargs["project_lease"]))
            yield source

    def observe_factory(factory, *args, **kwargs):
        original_factory(factory, *args, **kwargs)
        f.factories.append(factory)

    monkeypatch.setattr(installed, "open_installed_product_source", observe_open)
    monkeypatch.setattr(bootstrap.BootstrapFactory, "__init__", observe_factory)

    def load(name):
        f.imports.append(name)
        if name == "app":
            assert f.opened and all(source._closed for source, _ in f.opened)
            assert all(lease._closed for _, lease in f.opened)
            return app
        if name == "remote_operations":
            return sys.modules[name]
        assert name in {"meerkat_meta.builder", "meerkat_toolkit.host_control",
            "meerkat_toolkit.application_runtime"}
        assert str(f.selected.destination) not in sys.path
        return ModuleType(name)

    monkeypatch.setattr(bootstrap.importlib, "import_module", load)
    monkeypatch.setattr(sys, "path", list(sys.path))
    monkeypatch.setattr(sys, "argv", list(sys.argv))
    return app


def _invoke_child(f):
    # Execute the real fixed child declaration with inert import boundaries.
    # No subprocess runs. Its independent trusted digest compares received
    # source-owner.py, which is never imported or self-selected.
    arguments = bootstrap.bootstrap_child_arguments(f.directory, f.pin,
        lifecycle_id=f.lifecycle_id, owner_source=f.selected.source)
    assert arguments[:2] == ("-I", "-c")
    original_argv, original_bytecode = sys.argv, sys.dont_write_bytecode
    original_path = list(sys.path)
    try:
        sys.argv = ["fixed-trusted-bootstrap", *arguments[3:]]
        exec(arguments[2], {})
    finally:
        sys.argv, sys.dont_write_bytecode = original_argv, original_bytecode
        # Separate real -I children do not inherit authored import paths.
        sys.path[:] = original_path


def _live_factory(f, startup):
    factory = f.factories[-1]
    source = factory._product_source_custody
    assert source is not None and source is not f.parent_source
    assert source.origin == f.parent_source.origin
    assert source._lease is startup.lease and factory.source._lease is startup.lease
    source.require_owner_current()
    startup.lease.require_current()
    assert set(source.files) == set(f.selected.files)
    assert all(source.read_source(name) == content for name, content in f.selected.files.items())
    assert all(factory.source.exact_file(name).read() == f.selected.files[name] for name in
        ("config/mob.toml", "application/tool-policy.json", "application/configuration.json",
            "application/console.json", "config/access.toml"))
    policy = factory.source.exact_file("application/tool-policy.json")
    assert policy.path != f.selected.destination / "application/tool-policy.json"
    assert not isinstance(source.source_file("application/tool-policy.json"), ExactFile)
    assert type(startup.gateway) is ProductGatewayFile
    with pytest.raises(RuntimeError, match="already running"):
        with project_lock(f.selected.destination):
            pytest.fail("The actual child source lease was released early")
    return factory, source


def test_actual_fixed_child_enters_maintained_main_with_its_exact_source_lease(
        installed_bootstrap, tmp_path, monkeypatch):
    f = installed_bootstrap
    entered = []

    async def run(spec, args, *, application_startup):
        factory, source = _live_factory(f, application_startup)
        entered.append((factory, source, application_startup.lease))
        assert source.receipt["scope"] == "staged-product-application-source"
        assert deployment.read_receipt(f.directory)["status"] == "stopped"
        assert (f.directory / bootstrap.FRESH_ENTRY_NAME).exists()
        assert application_startup.runtime is None
        await asyncio.sleep(0)
        source.require_owner_current()

    _child_host(f, tmp_path, monkeypatch, run)
    def current_bundle():
        pytest.fail("An explicit original bundle was replaced by the current host helper")
    monkeypatch.setattr(bundles, "product_destination_owner_bytes", current_bundle)
    _invoke_child(f)
    assert len(entered) == 1
    factory, source, lease = entered[0]
    assert factory.source._closed and source._closed and lease._closed
    assert f.parent_source._closed and f.parent_lease._closed
    assert f.imports[:4] == ["meerkat_meta.builder", "meerkat_toolkit.host_control",
        "meerkat_toolkit.application_runtime", "app"]
    assert (f.selected.destination / "app.py").read_bytes() == f.selected.files["app.py"]


async def test_original_private_parent_closes_source_and_project_scope_before_launch(
        installed_bootstrap, monkeypatch):
    f = installed_bootstrap
    # The complete destination fixture selects Linux through the real product
    # reader. The parent's normal local target predicate sees that same host.
    monkeypatch.setattr(sys, "platform", "linux")
    # The initial byte installer has no previous runtime lifetime. Other child
    # controls select an explicit fixture lifetime to exercise its exact fence.
    deployment.update_receipt(f.directory, interactive_lifecycle_id=None,
        interactive_cleanup=None, interactive_cleanup_attempt_id=None)
    service = deployment.DeploymentService(f.selected.custody.home / ".local/state/meta-forge",
        None, None)
    opened, launched = [], []
    original_open = installed.open_installed_product_source

    @asynccontextmanager
    async def observe_open(*args, **kwargs):
        async with original_open(*args, **kwargs) as source:
            assert source is not None
            opened.append((source, kwargs["project_lease"]))
            yield source

    def launch(directory, mode, **options):
        assert service.lock.locked()
        assert directory == f.directory and mode == "start"
        assert len(opened) == 1 and opened[0][0]._closed and opened[0][1]._closed
        assert options["bootstrap_owner_source"] == f.selected.source
        selected = bootstrap.read_bootstrap_admission(directory, options["bootstrap_selection"])
        assert selected["application_startup"] == f.bootstrap.fresh
        assert deployment.read_receipt(directory)["status"] == "queued"
        # This uses the actual nonreentrant ProjectLockLease. A still-held
        # parent or staging lease would refuse this child handoff boundary.
        with project_lock(f.selected.destination) as child_lease:
            child_lease.require_current()
        launched.append(options)

    async def status(guide_id, **options):
        assert guide_id == f.selected.context["guide_id"]
        return deployment.read_receipt(f.directory)

    from launchkit import runtime_candidates as candidates
    inflated = []
    original_entries = candidates.product_prefix_entries
    def entries(content, prefix):
        inflated.append(prefix)
        return original_entries(content, prefix)
    monkeypatch.setattr(candidates, "product_prefix_entries", entries)
    monkeypatch.setattr(installed, "open_installed_product_source", observe_open)
    monkeypatch.setattr(service, "_launch", launch)
    monkeypatch.setattr(service, "status", status)
    result = await service.start_application(f.selected.context["guide_id"],
        f.selected.context["deployment_id"], application_startup=f.bootstrap.fresh,
        owner_source=f.selected.source)
    assert result["status"] == "queued" and len(launched) == 1
    assert service.tasks == {} and service.processes == {}
    assert not (f.directory / bootstrap.FRESH_ENTRY_NAME).exists()
    # The parent's source opening uses the kept inventory, not the archive.
    assert inflated == []


async def test_initialized_destination_host_forwards_original_owner_selection_only(
        installation):
    """Argument propagation only, not a runnable destination host handoff.

    The live staged host owns an external real lease. Its private supplier must
    still gain a genuine installed scope handoff before manager Start can run.
    """
    with selected_host(installation) as f:
        pin = f.pins["application/tool-policy.json"].pin()
        value = {"kind": "restore", "gateway": pin, "preparation_plan": pin,
            "selection_file": pin, "completed_setup": pin}
        selected = write(f.selected.destination / ".mobkit/forwarding-selection.json", value)
        calls = []
        f.host._startup_selection = lambda guide_id, deployment_id: selected

        async def start(guide_id, deployment_id, **options):
            f.source.require_owner_current()
            assert f.host._product_source_custody is f.source and not f.source._lease._closed
            calls.append((guide_id, deployment_id, options))
            return {"scope": "argument-propagation-only"}

        result = await f.host.start_application(SimpleNamespace(start_application=start),
            f.selected.context["guide_id"], f.selected.context["deployment_id"])
        assert result == {"scope": "argument-propagation-only"}
        assert calls == [(f.selected.context["guide_id"], f.selected.context["deployment_id"],
            {"application_startup": value, "owner_source": f.selected.source})]
        assert f.host._destination_installation_supervisor is None
        assert not f.directory.exists()


@pytest.mark.parametrize("different_definition", [False, True])
async def test_actual_supervisor_verifies_relocated_source_before_fixed_child_spawn(
        installed_bootstrap, monkeypatch, different_definition):
    f = installed_bootstrap
    opened, verified, spawned = [], [], []
    original_open = installed.open_installed_product_source
    original_verify = worker.Supervisor._verify_candidate_runtime

    @asynccontextmanager
    async def observe_open(*args, **kwargs):
        async with original_open(*args, **kwargs) as source:
            assert source is not None
            opened.append((source, kwargs["project_lease"]))
            yield source

    def verify(supervisor, files, *, source_custody=None, **options):
        assert source_custody is opened[-1][0] and not source_custody._closed
        source_custody.require_filesystem_current()
        proof = original_verify(supervisor, files, source_custody=source_custody, **options)
        assert proof["source"] == "meta-toolkit-product"
        verified.append(source_custody)
        return proof

    class SpawnBoundary(RuntimeError):
        pass

    async def spawn(supervisor, *args, **options):
        assert all(source._closed and lease._closed for source, lease in opened)
        assert len(verified) == 1
        assert args[:3] == (str(f.selected.destination / ".venv/bin/python"), "-I", "-c")
        assert "owner_source_sha256=" in args[3]
        assert bootstrap.updates.digest(f.selected.source) in args[3]
        assert f.selected.source.decode() not in args[3]
        with project_lock(f.selected.destination) as child_lease:
            child_lease.require_current()
        spawned.append((args, options))
        raise SpawnBoundary("Inert application spawn boundary reached")

    monkeypatch.setattr(installed, "open_installed_product_source", observe_open)
    monkeypatch.setattr(worker.Supervisor, "_verify_candidate_runtime", verify)
    monkeypatch.setattr(worker.Supervisor, "spawn_child", spawn)
    monkeypatch.setattr(worker, "require_available_console_port", lambda port: {"available": True})
    supervisor = worker.Supervisor(f.directory)
    supervisor._bootstrap_owner_source = f.selected.source
    definition = json.loads(f.selected.files["deployment.json"])
    if different_definition:
        definition["slug"] += "-changed"
        with pytest.raises(deployment.DraftConflict, match="definition differs"):
            await supervisor.start(definition, bootstrap_selection=f.pin)
        assert verified == spawned == []
    else:
        with pytest.raises(SpawnBoundary, match="spawn boundary"):
            await supervisor.start(definition, bootstrap_selection=f.pin)
        assert len(verified) == len(spawned) == 1
    assert len(opened) == 1 and opened[0][0]._closed and opened[0][1]._closed
    assert supervisor.child is None
    assert not (f.directory / bootstrap.FRESH_ENTRY_NAME).exists()


KEPT_FIELD = "native_prefix_receipt_sha256"


def _kept_files(f):
    return sorted(path.name for path in f.directory.iterdir() if "native-prefix-receipt" in path.name)


def _forget_kept_inventory(f):
    receipt = deployment.read_receipt(f.directory)
    named = receipt.pop(KEPT_FIELD)
    deployment.write_private(f.directory / "receipt.json", receipt)
    (f.directory / f"native-prefix-receipt-{named}.json").unlink()
    return named


async def _start_until_spawn(f, monkeypatch):
    """Run one actual product Start up to its inert spawn boundary."""
    from launchkit import runtime_candidates as candidates
    inflated, sources = [], []
    original_entries = candidates.product_prefix_entries
    def entries(content, prefix):
        inflated.append(prefix)
        return original_entries(content, prefix)
    original_verify = worker.Supervisor._verify_candidate_runtime
    def verify(supervisor, files, **options):
        proof = original_verify(supervisor, files, **options)
        sources.append((options.get("observation") or {}).get("inventory_source"))
        return proof
    class SpawnBoundary(RuntimeError):
        pass
    async def spawn(supervisor, *args, **options):
        raise SpawnBoundary("Inert application spawn boundary reached")
    with monkeypatch.context() as held:
        held.setattr(candidates, "product_prefix_entries", entries)
        held.setattr(worker.Supervisor, "_verify_candidate_runtime", verify)
        held.setattr(worker.Supervisor, "spawn_child", spawn)
        held.setattr(worker, "require_available_console_port", lambda port: {"available": True})
        supervisor = worker.Supervisor(f.directory)
        supervisor._bootstrap_owner_source = f.selected.source
        try:
            await supervisor.start(json.loads(f.selected.files["deployment.json"]), bootstrap_selection=f.pin)
        except SpawnBoundary:
            outcome = None
        except Exception as error:
            outcome = error
        else:
            pytest.fail("The inert product Start passed its spawn boundary.")
    return inflated, sources, outcome


async def test_product_start_checks_the_prefix_against_the_kept_inventory_without_the_archive(
        installed_bootstrap, monkeypatch):
    f = installed_bootstrap
    inflated, sources, outcome = await _start_until_spawn(f, monkeypatch)
    assert outcome is None and inflated == [] and sources == ["retained"]
    named = deployment.read_receipt(f.directory)[KEPT_FIELD]
    assert _kept_files(f) == [f"native-prefix-receipt-{named}.json"]


async def test_legacy_product_start_runs_the_full_check_once_and_then_keeps_the_inventory(
        installed_bootstrap, monkeypatch):
    f = installed_bootstrap
    _forget_kept_inventory(f)
    inflated, sources, outcome = await _start_until_spawn(f, monkeypatch)
    assert outcome is None and inflated == [f.source.artifacts.prefix] and sources == ["archive"]
    # The lazy mint keeps the same verified entries the install kept.
    kept = deployment.read_receipt(f.directory)[KEPT_FIELD]
    assert _kept_files(f) == [f"native-prefix-receipt-{kept}.json"]
    assert (json.loads((f.directory / f"native-prefix-receipt-{kept}.json").read_bytes())["inventory_sha256"]
        == deployment.read_receipt(f.directory)["installed_candidate_runtime_proof"]["prefix_inventory_sha256"])
    inflated, sources, outcome = await _start_until_spawn(f, monkeypatch)
    assert outcome is None and inflated == [] and sources == ["retained"]


async def test_product_start_with_kept_fingerprints_reads_no_unchanged_native_file(
        installed_bootstrap, monkeypatch):
    from launchkit import native_prefix_receipt as kept
    from launchkit.templates import install_candidate as adapter
    f = installed_bootstrap
    _forget_kept_inventory(f)
    # The fixture's install has just written every file, so all of them are
    # racy; a later legacy Start keeps their fingerprints.
    monkeypatch.setattr(kept, "RACY_NS", 0)
    monkeypatch.setattr(kept, "local_disk_filesystem", lambda path: True)
    inflated, sources, outcome = await _start_until_spawn(f, monkeypatch)
    assert outcome is None and sources == ["archive"]
    named = deployment.read_receipt(f.directory)[KEPT_FIELD]
    files = sorted(entry.name for entry in f.source.artifacts.entries if entry.kind == "file")
    document = json.loads((f.directory / f"native-prefix-receipt-{named}.json").read_bytes())
    assert sorted(document["fingerprints"]) == files
    prefix = f.selected.destination / ".bin" / f.source.artifacts.prefix
    reads = []
    original = adapter.read_owned
    def read_owned(path, *args, **kwargs):
        if prefix in Path(path).parents:
            reads.append(Path(path).relative_to(prefix).as_posix())
        return original(path, *args, **kwargs)
    monkeypatch.setattr(adapter, "read_owned", read_owned)
    inflated, sources, outcome = await _start_until_spawn(f, monkeypatch)
    # Only the executed gateway is read again, once per prefix walk.
    assert outcome is None and inflated == [] and sources == ["retained"]
    assert reads and set(reads) == {"bin/toolkit_gateway"}


async def test_failed_legacy_product_start_keeps_no_inventory(installed_bootstrap, monkeypatch):
    f = installed_bootstrap
    _forget_kept_inventory(f)
    readme = f.selected.destination / ".bin" / f.source.artifacts.prefix / "README.md"
    original = readme.read_bytes()
    readme.chmod(0o644)
    readme.write_bytes(bytes([original[0] ^ 1]) + original[1:])
    readme.chmod(0o444)
    for _ in range(2):
        inflated, sources, outcome = await _start_until_spawn(f, monkeypatch)
        assert outcome is not None and "native prefix file changed" in str(outcome)
        assert inflated == [f.source.artifacts.prefix] and sources == []
        assert KEPT_FIELD not in deployment.read_receipt(f.directory) and _kept_files(f) == []


async def test_changed_kept_inventory_refuses_the_product_start(installed_bootstrap, monkeypatch):
    f = installed_bootstrap
    named = deployment.read_receipt(f.directory)[KEPT_FIELD]
    path = f.directory / f"native-prefix-receipt-{named}.json"
    content = path.read_bytes()
    path.write_bytes(content.rstrip(b"\n") + b" \n")
    inflated, sources, outcome = await _start_until_spawn(f, monkeypatch)
    assert outcome is not None and "differs from the copy receipt.json names" in str(outcome)
    assert str(path) in str(outcome) and "Remove that file" in str(outcome)
    assert inflated == [] and sources == []
    path.unlink()
    inflated, sources, outcome = await _start_until_spawn(f, monkeypatch)
    assert outcome is None and inflated == [f.source.artifacts.prefix] and sources == ["archive"]


def _keep_with_fingerprints(f, monkeypatch):
    """Keep the inventory with fingerprints, as a later full Start does."""
    from launchkit import native_prefix_receipt as kept
    from launchkit.templates import install_candidate as adapter
    monkeypatch.setattr(kept, "RACY_NS", 0)
    monkeypatch.setattr(kept, "local_disk_filesystem", lambda path: True)
    receipt = deployment.read_receipt(f.directory)
    proof = receipt["installed_candidate_runtime_proof"]
    record = {}
    adapter.verify_meta_toolkit_prefix(f.source.artifacts, deployment_root=f.selected.destination, record=record)
    named = kept.mint(f.directory, f.source.artifacts, proof=proof,
        runtime_candidate=receipt["runtime_candidate"], fingerprints=record)
    assert named and kept.record(f.directory, named, inventory_sha256=proof["prefix_inventory_sha256"],
        runtime_candidate=receipt["runtime_candidate"])
    return named


def _child_counts(monkeypatch, f):
    from launchkit import runtime_candidates as candidates
    from launchkit.templates import install_candidate as adapter
    # "reads" counts unchanged native files read again; the executed gateway
    # itself is always hashed and is left out of that count.
    counts = {"inflated": 0, "reads": 0}
    prefix = f.selected.destination / ".bin" / f.source.artifacts.prefix
    gateway = prefix / "bin/toolkit_gateway"
    original_entries, original_read = candidates.product_prefix_entries, adapter.read_owned
    def entries(content, name):
        counts["inflated"] += 1
        return original_entries(content, name)
    def read_owned(path, *args, **kwargs):
        if prefix in Path(path).parents and Path(path) != gateway:
            counts["reads"] += 1
        return original_read(path, *args, **kwargs)
    monkeypatch.setattr(candidates, "product_prefix_entries", entries)
    monkeypatch.setattr(adapter, "read_owned", read_owned)
    return counts


@pytest.mark.parametrize("bundle", ["current", "historical"])
async def test_console_start_reads_no_archive_and_no_unchanged_native_file_in_any_process(
        installed_bootstrap, tmp_path, monkeypatch, bundle):
    f = installed_bootstrap
    if bundle == "historical":
        # A product installed by an earlier Forge: its intake holds owner
        # bytes A, while this Forge's own bundle is B. Every lane passes A.
        monkeypatch.setattr(bundles, "product_destination_owner_bytes",
            lambda: f.selected.source + b"\n# Different current trusted owner bundle B.\n")
    monkeypatch.setattr(sys, "platform", "linux")
    deployment.update_receipt(f.directory, interactive_lifecycle_id=None,
        interactive_cleanup=None, interactive_cleanup_attempt_id=None)
    _keep_with_fingerprints(f, monkeypatch)
    counts = _child_counts(monkeypatch, f)
    service = deployment.DeploymentService(f.selected.custody.home / ".local/state/meta-forge",
        None, None)
    launched = []
    def launch(directory, mode, **options):
        assert directory == f.directory and mode == "start"
        launched.append(options)
    async def status(guide_id, **options):
        return deployment.read_receipt(f.directory)
    monkeypatch.setattr(service, "_launch", launch)
    monkeypatch.setattr(service, "status", status)
    await service.start_application(f.selected.context["guide_id"],
        f.selected.context["deployment_id"], application_startup=f.bootstrap.fresh,
        owner_source=f.selected.source)
    # Server process: its source opening joins the kept inventory.
    assert len(launched) == 1 and counts == {"inflated": 0, "reads": 0}
    f.pin = launched[0]["bootstrap_selection"]
    inflated, sources, outcome = await _start_until_spawn(f, monkeypatch)
    # Worker: no archive pass, and every prefix walk skips the unchanged files.
    assert outcome is None and sources == ["retained"] and counts == {"inflated": 0, "reads": 0}
    f.lifecycle_id = deployment.read_receipt(f.directory).get("interactive_lifecycle_id")
    entered = []
    async def run(spec, args, *, application_startup):
        entered.append(application_startup)
    _child_host(f, tmp_path, monkeypatch, run)
    await asyncio.to_thread(_invoke_child, f)
    # Application child: the same, in both of its source scopes.
    assert len(entered) == 1 and counts == {"inflated": 0, "reads": 0}


def test_product_child_start_uses_the_kept_inventory_and_reads_no_unchanged_native_file(
        installed_bootstrap, tmp_path, monkeypatch):
    f = installed_bootstrap
    _keep_with_fingerprints(f, monkeypatch)
    counts = _child_counts(monkeypatch, f)
    entered = []
    async def run(spec, args, *, application_startup):
        entered.append(application_startup)
    _child_host(f, tmp_path, monkeypatch, run)
    _invoke_child(f)
    assert len(entered) == 1 and counts == {"inflated": 0, "reads": 0}


def test_product_child_start_refuses_a_changed_native_file_despite_kept_fingerprints(
        installed_bootstrap, tmp_path, monkeypatch):
    f = installed_bootstrap
    _keep_with_fingerprints(f, monkeypatch)
    readme = f.selected.destination / ".bin" / f.source.artifacts.prefix / "README.md"
    original = readme.read_bytes()
    readme.chmod(0o644)
    readme.write_bytes(bytes([original[0] ^ 1]) + original[1:])
    readme.chmod(0o444)
    async def run(spec, args, *, application_startup):
        pytest.fail("The child started over a changed native prefix.")
    _child_host(f, tmp_path, monkeypatch, run)
    with pytest.raises(Exception, match="native prefix file changed"):
        _invoke_child(f)


def test_product_child_start_refuses_a_changed_kept_inventory(installed_bootstrap, tmp_path, monkeypatch):
    f = installed_bootstrap
    named = deployment.read_receipt(f.directory)[KEPT_FIELD]
    path = f.directory / f"native-prefix-receipt-{named}.json"
    path.write_bytes(path.read_bytes().rstrip(b"\n") + b" \n")
    async def run(spec, args, *, application_startup):
        pytest.fail("The child started with a changed kept inventory.")
    _child_host(f, tmp_path, monkeypatch, run)
    with pytest.raises(Exception, match="differs from the copy receipt.json names"):
        _invoke_child(f)


def _descriptor_target(descriptor):
    return os.readlink(f"/proc/self/fd/{descriptor}")


async def test_failed_lazy_inventory_write_keeps_the_start_and_the_next_start_is_full(
        installed_bootstrap, monkeypatch):
    f = installed_bootstrap
    _forget_kept_inventory(f)
    failing = [True]
    original = os.fsync
    def fsync(descriptor):
        if failing[0] and ".native-prefix-receipt-" in _descriptor_target(descriptor):
            raise OSError(errno.ENOSPC, "No space left on device")
        return original(descriptor)
    monkeypatch.setattr(os, "fsync", fsync)
    inflated, sources, outcome = await _start_until_spawn(f, monkeypatch)
    assert outcome is None and sources == ["archive"]
    assert KEPT_FIELD not in deployment.read_receipt(f.directory) and _kept_files(f) == []
    failing[0] = False
    inflated, sources, outcome = await _start_until_spawn(f, monkeypatch)
    assert outcome is None and inflated == [f.source.artifacts.prefix] and sources == ["archive"]
    assert KEPT_FIELD in deployment.read_receipt(f.directory)


async def test_lazy_inventory_is_durable_before_the_receipt_names_it(installed_bootstrap, monkeypatch):
    from launchkit.templates import storage_setup
    f = installed_bootstrap
    _forget_kept_inventory(f)
    events = []
    original_fsync, original_replace, original_write = os.fsync, os.replace, storage_setup.write_private
    def fsync(descriptor):
        target = _descriptor_target(descriptor)
        if "native-prefix-receipt-" in target:
            events.append("file fsync")
        elif target == str(f.directory):
            events.append("folder fsync")
        return original_fsync(descriptor)
    def replace(source, destination):
        if "native-prefix-receipt-" in str(source):
            events.append("rename")
        return original_replace(source, destination)
    def write_private(path, value):
        if Path(path).name == "receipt.json" and KEPT_FIELD in value:
            events.append("receipt names it")
        return original_write(path, value)
    monkeypatch.setattr(os, "fsync", fsync)
    monkeypatch.setattr(os, "replace", replace)
    monkeypatch.setattr(storage_setup, "write_private", write_private)
    inflated, sources, outcome = await _start_until_spawn(f, monkeypatch)
    assert outcome is None and sources == ["archive"]
    assert events[:4] == ["file fsync", "rename", "folder fsync", "receipt names it"]


def test_lazy_mint_records_the_verified_selection_not_a_later_receipt(installed_bootstrap):
    from launchkit import native_prefix_receipt as kept
    f = installed_bootstrap
    _forget_kept_inventory(f)
    verified = deployment.read_receipt(f.directory)
    observation = {"artifacts": f.source.artifacts, "inventory_source": "archive", "fingerprints": {}}
    # The live receipt changes after the verification ran on `verified`.
    deployment.update_receipt(f.directory, runtime_candidate={**verified["runtime_candidate"], "replaced": True})
    worker.Supervisor(f.directory)._settle_native_inventory(None, observation,
        verified["installed_candidate_runtime_proof"], verified)
    assert KEPT_FIELD not in deployment.read_receipt(f.directory)
    [name] = _kept_files(f)
    document = json.loads((f.directory / name).read_bytes())
    assert document["runtime_candidate_sha256"] == kept.selection_sha256(verified["runtime_candidate"])


def test_product_child_loads_the_kept_inventory_once_for_both_source_scopes(
        installed_bootstrap, tmp_path, monkeypatch):
    from launchkit import native_prefix_receipt as kept
    f = installed_bootstrap
    loads, handed = [], []
    original_load = kept.load
    def load(directory, receipt):
        value = original_load(directory, receipt)
        loads.append(value)
        return value
    monkeypatch.setattr(kept, "load", load)
    async def run(spec, args, *, application_startup):
        return None
    _child_host(f, tmp_path, monkeypatch, run)
    observed_open = installed.open_installed_product_source
    @asynccontextmanager
    async def observe(*args, **kwargs):
        handed.append(kwargs.get("retained"))
        async with observed_open(*args, **kwargs) as source:
            yield source
    monkeypatch.setattr(installed, "open_installed_product_source", observe)
    _invoke_child(f)
    assert len(loads) == 1 and loads[0] is not None
    assert len(handed) == 2 and all(value is loads[0] for value in handed)


def _child_refusal(f, tmp_path, monkeypatch):
    read_fd, write_fd = os.pipe()
    monkeypatch.setenv("MOBKIT_LAUNCH_READY_FD", str(write_fd))
    async def run(spec, args, *, application_startup):
        pytest.fail("The child started over a refused source.")
    _child_host(f, tmp_path, monkeypatch, run)
    try:
        with pytest.raises(Exception) as raised:
            _invoke_child(f)
    finally:
        os.close(write_fd)
    with os.fdopen(read_fd, "rb") as channel:
        line = channel.readline()
    return worker.startup_refusal(line), raised.value


def test_product_child_reports_a_changed_kept_inventory_on_its_readiness_channel(
        installed_bootstrap, tmp_path, monkeypatch):
    f = installed_bootstrap
    named = deployment.read_receipt(f.directory)[KEPT_FIELD]
    path = f.directory / f"native-prefix-receipt-{named}.json"
    path.write_bytes(path.read_bytes().rstrip(b"\n") + b" \n")
    refusal, error = _child_refusal(f, tmp_path, monkeypatch)
    assert refusal == "The application refused to start: " + str(error)
    assert str(path) in refusal and "Remove that file" in refusal


async def test_a_native_file_changed_after_the_worker_check_is_refused_in_the_child(
        installed_bootstrap, tmp_path, monkeypatch):
    f = installed_bootstrap
    _keep_with_fingerprints(f, monkeypatch)
    inflated, sources, outcome = await _start_until_spawn(f, monkeypatch)
    assert outcome is None and sources == ["retained"]
    readme = f.selected.destination / ".bin" / f.source.artifacts.prefix / "README.md"
    original = readme.read_bytes()
    readme.chmod(0o644)
    readme.write_bytes(bytes([original[0] ^ 1]) + original[1:])
    readme.chmod(0o444)
    refusal, error = await asyncio.to_thread(_child_refusal, f, tmp_path, monkeypatch)
    assert refusal == "The application refused to start: " + str(error)
    assert "native prefix file changed" in refusal


@pytest.mark.parametrize("change", ["receipt", "selection", "lifecycle", "source"])
def test_exact_child_refuses_drift_between_preimport_and_application_scope(
        installed_bootstrap, tmp_path, monkeypatch, change):
    f = installed_bootstrap
    entered = []

    async def run(*args, **kwargs):
        entered.append(True)
        pytest.fail("Changed original source or lifecycle reached application run")

    app = _child_host(f, tmp_path, monkeypatch, run)
    original = app.main

    def main(**kwargs):
        if change == "receipt":
            _receipt(f, build_id="00000000-0000-4000-8000-000000000001")
        elif change == "selection":
            pin = Path(f.pin["path"])
            value = json.loads(pin.read_bytes())
            value["application_startup"]["preparation_plan"]["sha256"] = "f" * 64
            write(pin, value)
        elif change == "lifecycle":
            _receipt(f, interactive_lifecycle_id="a-different-original-child")
        else:
            write(f.selected.destination / "application/tool-policy.json", b"{}\n", 0o644)
        original(**kwargs)

    monkeypatch.setattr(app, "main", main)
    with pytest.raises((CustodyError, ValueError, OSError, RuntimeError)):
        _invoke_child(f)
    assert entered == []
    assert not (f.directory / bootstrap.FRESH_ENTRY_NAME).exists()


def test_actual_main_repeated_cancellation_keeps_same_shutdown_and_application_lease(
        installed_bootstrap, tmp_path, monkeypatch):
    f = installed_bootstrap
    original_run = asyncio.run
    callers, entered, shutdown_calls, observed = [], [], [], []

    def child_loop(coroutine):
        async def caller():
            callers.append(asyncio.current_task())
            return await coroutine
        return original_run(caller())

    class Runtime:
        async def shutdown(self):
            shutdown_calls.append((self, asyncio.current_task()))
            cleanup_entered.set()
            await cleanup_release.wait()

    runtime = Runtime()

    async def run(spec, args, *, application_startup):
        nonlocal cleanup_entered, cleanup_release
        cleanup_entered, cleanup_release = asyncio.Event(), asyncio.Event()
        factory, source = _live_factory(f, application_startup)
        entered.append((factory, source, application_startup.lease))
        application_startup.runtime = runtime
        caller = callers[-1]

        async def interrupt():
            caller.cancel("original caller cancellation")
            try:
                await asyncio.wait_for(cleanup_entered.wait(), 30)
                assert not factory.source._closed and not source._closed
                application_startup.lease.require_current()
                with pytest.raises(RuntimeError, match="already running"):
                    with project_lock(f.selected.destination):
                        pytest.fail("Original application lease left during pending shutdown")
                original_shutdown = factory._shutdown_task
                assert original_shutdown is not None and not original_shutdown.done()
                caller.cancel("repeated caller cancellation")
                await asyncio.sleep(0)
                assert factory._shutdown_task is original_shutdown
                assert not original_shutdown.done() and not source._closed
                application_startup.lease.require_current()
                observed.append((original_shutdown, source, application_startup.lease))
            finally:
                cleanup_release.set()

        interruption = asyncio.create_task(interrupt())
        try:
            await asyncio.Event().wait()
        finally:
            source.require_owner_current()
            try:
                await application_startup.shutdown(runtime)
            finally:
                await interruption

    cleanup_entered = cleanup_release = None
    _child_host(f, tmp_path, monkeypatch, run)
    monkeypatch.setattr(asyncio, "run", child_loop)
    with pytest.raises(asyncio.CancelledError) as result:
        _invoke_child(f)
    assert str(result.value) == "original caller cancellation"
    assert len(entered) == len(observed) == len(shutdown_calls) == 1
    factory, source, lease = entered[0]
    assert observed[0] == (factory._shutdown_task, source, lease)
    assert shutdown_calls[0][0] is runtime and factory._shutdown_task.done()
    assert factory.source._closed and source._closed and lease._closed
    assert f.parent_source._closed and f.parent_lease._closed


def _completed_output_specimen(f):
    """Selected-file specimens test custody, not native completion authority."""
    selected = f.bootstrap
    bundle = Path(selected.fresh["bundle_path"])
    bundle.mkdir(mode=0o700)
    activation = write(Path(selected.fresh["activation_path"]), {"inert": "selected activation"})
    policy = write(bundle / "policy.json", {"inert": "selected policy"})
    setup = write(bundle / "setup.json", {"inert": "selected setup bytes"})
    manifest = write(bundle / "manifest.json", {"artifacts": [
        {"path": policy.path.name, "sha256": policy.sha256},
        {"path": setup.path.name, "sha256": setup.sha256}]})
    pointer = f.selected.destination / "current"
    if pointer.is_symlink():
        pointer.unlink()
    pointer.symlink_to(str(activation.path))
    selection = write(selected.selection.path, {"deployment_root": str(f.selected.destination),
        "activation": activation.pin(), "policy_manifest": manifest.pin(), "local_policy": policy.pin()})
    selected.restore.update(gateway=selected.gateway.pin(), preparation_plan=selected.plan.pin(),
        selection_file=selection.pin(), completed_setup=setup.pin())
    return setup


def test_original_fresh_output_view_can_reopen_related_restore_without_source_rewrite(
        installed_bootstrap, tmp_path, monkeypatch):
    f = installed_bootstrap
    sources, entries = [], []
    setup = None

    async def fresh_run(spec, args, *, application_startup):
        nonlocal setup
        factory, source = _live_factory(f, application_startup)
        sources.append(source)
        entries.append((f.directory / bootstrap.FRESH_ENTRY_NAME).read_bytes())
        setup = _completed_output_specimen(f)
        source.require_owner_current()
        await factory.source.require_current_async()
        assert application_startup.runtime is None

    _child_host(f, tmp_path / "fresh", monkeypatch, fresh_run)
    _invoke_child(f)
    assert sources[0]._closed
    assert (f.directory / bootstrap.FRESH_ENTRY_NAME).read_bytes() == entries[0]

    async def select_restore():
        with project_lock(f.selected.destination) as lease:
            async with installed.open_installed_product_source(f.directory, project_lease=lease,
                    require_current=lease.require_current, owner_source=f.selected.source,
                    application_startup=f.bootstrap.restore) as source:
                source.require_owner_current()
                pin, _ = bootstrap.retain_bootstrap_admission(f.directory,
                    deployment.read_receipt(f.directory), f.bootstrap.restore, source_custody=source)
                return pin

    f.pin = asyncio.run(select_restore())

    async def restore_run(spec, args, *, application_startup):
        factory, source = _live_factory(f, application_startup)
        sources.append(source)
        assert isinstance(application_startup, RestoredApplicationStartup)
        await application_startup.prepare(f.bootstrap.app())
        assert application_startup.setup == setup
        assert application_startup._setup_bytes == setup.read()
        assert application_startup.runtime is None
        assert (f.directory / bootstrap.FRESH_ENTRY_NAME).read_bytes() == entries[0]

    _child_host(f, tmp_path / "restore", monkeypatch, restore_run)
    _invoke_child(f)
    assert len(sources) == 2 and all(source._closed for source in sources)
    assert (f.directory / bootstrap.FRESH_ENTRY_NAME).read_bytes() == entries[0]
    assert all((f.selected.destination / name).read_bytes() == content
        for name, content in f.selected.files.items())


@pytest.mark.parametrize("change", ["unselected-file", "unselected-link", "bundle-link",
    "public-activation", "foreign-current"])
def test_actual_selected_output_view_still_refuses_foreign_or_unsafe_publication(
        installed_bootstrap, tmp_path, monkeypatch, change):
    f = installed_bootstrap
    entered = []

    async def run(spec, args, *, application_startup):
        factory, source = _live_factory(f, application_startup)
        entered.append((factory, source))
        _completed_output_specimen(f)
        root = f.selected.destination
        if change == "unselected-file":
            write(root / "unselected-output.json", b"unselected output\n")
        elif change == "unselected-link":
            (root / "unselected-output.json").symlink_to(Path(f.bootstrap.fresh["activation_path"]))
        elif change == "bundle-link":
            (Path(f.bootstrap.fresh["bundle_path"]) / "linked-policy.json").symlink_to("policy.json")
        elif change == "public-activation":
            Path(f.bootstrap.fresh["activation_path"]).chmod(0o644)
        else:
            (root / "current").unlink()
            (root / "current").symlink_to(str(root / "unselected-activation.json"))
        source.require_owner_current()
        pytest.fail("A selected output view accepted unrelated or unsafe lifecycle publication")

    _child_host(f, tmp_path, monkeypatch, run)
    with pytest.raises((CustodyError, ValueError, OSError, RuntimeError, BaseExceptionGroup)):
        _invoke_child(f)
    assert len(entered) == 1
    assert "app" in f.imports
    assert all((f.selected.destination / name).read_bytes() == content
        for name, content in f.selected.files.items())


@pytest.mark.parametrize(("report", "cancel"), [("private-close", False),
    ("unknown", True), ("exited", True)])
def test_actual_main_keeps_lease_until_same_sdk_shutdown_continuation_joins(
        installed_bootstrap, tmp_path, monkeypatch, report, cancel):
    """Use Meta's real repeated shutdown contract with an inert SDK boundary.

    An exited public report cannot substitute for Toolkit private close. The
    original error remains an error after the same runtime's lifetime settles.
    """
    f = installed_bootstrap
    original_run = asyncio.run
    refusal = RuntimeError("Inert original composed SDK shutdown failed")
    callers, entered, calls, observations = [], [], [], []

    def child_loop(coroutine):
        async def caller():
            callers.append(asyncio.current_task())
            return await coroutine
        return original_run(caller())

    class Runtime:
        def __init__(self):
            self._report = SimpleNamespace(state="unknown" if report == "unknown" else "exited",
                exit_code=None if report == "unknown" else (1 if report == "exited" else 0),
                signal=None, escalation="none",
                native_shutdown="unanswered" if report == "unknown" else
                    ("refused" if report == "exited" else "acknowledged"))

        @property
        def last_gateway_termination(self):
            return self._report

        async def connect(self):
            pytest.fail("Shutdown continuation must not reconnect or replace the runtime")

        async def shutdown(self):
            calls.append((self, asyncio.current_task()))
            if len(calls) == 1:
                raise refusal
            assert len(calls) == 2
            continuation_entered.set()
            await continuation_release.wait()
            if report == "unknown":
                self._report = SimpleNamespace(state="exited", exit_code=0,
                    signal=None, escalation="none", native_shutdown="acknowledged")
            # This normal return represents the same original SDK composed
            # lifetime join, never native preparation or successful useful work.

    runtime = Runtime()

    async def run(spec, args, *, application_startup):
        nonlocal continuation_entered, continuation_release
        continuation_entered, continuation_release = asyncio.Event(), asyncio.Event()
        factory, source = _live_factory(f, application_startup)
        entered.append((factory, source, application_startup))
        application_startup.runtime = runtime
        caller = callers[-1]

        async def observe():
            try:
                await asyncio.wait_for(continuation_entered.wait(), 30)
                assert calls[0][0] is calls[1][0] is runtime
                assert calls[0][1] is not calls[1][1]
                assert application_startup._shutdown_error is refusal
                assert not factory.source._closed and not source._closed
                application_startup.lease.require_current()
                with pytest.raises(RuntimeError, match="already running"):
                    with project_lock(f.selected.destination):
                        pytest.fail("Terminal original cleanup released the actual app lease")
                wrapper = factory._shutdown_task
                attempt = application_startup._shutdown_attempt
                assert not wrapper.done() and not attempt.done()
                if cancel:
                    caller.cancel("original terminal-cleanup caller cancellation")
                    await asyncio.sleep(0)
                    caller.cancel("repeated terminal-cleanup caller cancellation")
                    await asyncio.sleep(0)
                assert factory._shutdown_task is wrapper
                assert application_startup._shutdown_attempt is attempt
                assert not attempt.done() and not source._closed
                application_startup.lease.require_current()
                observations.append((wrapper, attempt, source, application_startup.lease))
            finally:
                continuation_release.set()

        observation = asyncio.create_task(observe())
        try:
            await application_startup.shutdown(runtime)
        finally:
            await observation

    continuation_entered = continuation_release = None
    _child_host(f, tmp_path, monkeypatch, run)
    monkeypatch.setattr(asyncio, "run", child_loop)
    with pytest.raises(BaseException) as result:
        _invoke_child(f)

    def leaves(error):
        if isinstance(error, BaseExceptionGroup):
            return [item for member in error.exceptions for item in leaves(member)]
        return [error]

    failures = leaves(result.value)
    assert any(error is refusal for error in failures)
    if cancel:
        assert any(isinstance(error, asyncio.CancelledError) and str(error) ==
            "original terminal-cleanup caller cancellation" for error in failures)
    else:
        assert result.value is refusal
    assert len(calls) == 2 and len(entered) == len(observations) == 1
    factory, source, startup = entered[0]
    assert factory._shutdown_runtime is startup._shutdown_runtime is runtime
    assert factory._shutdown_task.exception() is refusal
    assert startup._shutdown_task.exception() is refusal
    assert startup._shutdown_attempt.done() and startup._shutdown_attempt.exception() is None
    assert startup._shutdown_error is refusal
    assert observations[0] == (factory._shutdown_task, startup._shutdown_attempt, source, startup.lease)
    assert factory.source._closed and source._closed and startup.lease._closed
    assert f.parent_source._closed and f.parent_lease._closed


@pytest.mark.parametrize("change", ["missing-marker", "missing-context", "both-records-missing", "foreign-guide",
    "foreign-deployment", "foreign-package", "foreign-source", "foreign-runtime",
    "package-bytes", "owner-bundle", "trusted-bundle", "linked-context", "extra-file", "extra-link"])
def test_fixed_child_refuses_foreign_installed_source_before_any_import(
        installed_bootstrap, monkeypatch, change):
    f = installed_bootstrap
    marker = f.selected.destination / owner.SOURCE_DIRECTORY / "owner.json"
    if change == "missing-marker":
        marker.unlink()
    elif change == "missing-context":
        (f.store / "owner.json").unlink()
    elif change == "both-records-missing":
        marker.unlink()
        shutil.rmtree(f.store)
    elif change.startswith("foreign-") and change != "foreign-runtime":
        value = json.loads(marker.read_bytes())
        key = {"foreign-guide": "guide_id", "foreign-deployment": "deployment_id",
            "foreign-package": "package_sha256", "foreign-source": "source_digest"}[change]
        value["context"][key] = ("00000000-0000-4000-8000-000000000001"
            if key.endswith("_id") else "f" * 64)
        write(marker, owner._json(value))
    elif change == "foreign-runtime":
        candidate = deepcopy(deployment.read_receipt(f.directory)["runtime_candidate"])
        candidate["assembly"]["sha256"] = "f" * 64
        _receipt(f, runtime_candidate=candidate)
    elif change == "package-bytes":
        write(f.directory / "package.zip", b"changed original package")
    elif change == "owner-bundle":
        write(f.store / "source-owner.py", b"raise RuntimeError('received bytes are not a trusted owner')\n")
    elif change == "trusted-bundle":
        f.selected.source += b"\n# Different trusted bundle selection\n"
    elif change == "linked-context":
        context = f.store / "owner.json"
        retained = context.with_name("retained-owner.json")
        context.rename(retained)
        context.symlink_to(retained.name)
    elif change == "extra-file":
        write(f.selected.destination / "unselected-application.py", b"never execute\n", 0o644)
    else:
        (f.selected.destination / "unselected-application.py").symlink_to("app.py")
    _forbid_imports(monkeypatch, f)
    producer_reads = _forbid_producer_reads(monkeypatch, f)
    with pytest.raises((CustodyError, ValueError, OSError, RuntimeError)):
        _invoke_child(f)
    assert f.imports == []
    assert producer_reads == []
    assert not (f.directory / bootstrap.FRESH_ENTRY_NAME).exists()


def test_trusted_selected_owner_refuses_both_records_removed_at_file_scope_entry(
        installed_bootstrap, monkeypatch):
    f = installed_bootstrap
    _forbid_imports(monkeypatch, f)
    producer_reads = _forbid_producer_reads(monkeypatch, f)
    original_open = installed._open_files
    entered = []

    @contextmanager
    def remove_after_owner_selection(directory, lease, callback, owner_source, application_startup, *rest):
        # This is the actual worker's __enter__ seam, after on-loop selection
        # authenticated the independently pinned original owner bytes.
        assert owner_source == f.selected.source
        assert lease.root == f.selected.destination and not lease._closed
        (f.selected.destination / owner.SOURCE_DIRECTORY / "owner.json").unlink()
        shutil.rmtree(f.store)
        entered.append(lease)
        with original_open(directory, lease, callback, owner_source, application_startup, *rest) as source:
            yield source

    monkeypatch.setattr(installed, "_open_files", remove_after_owner_selection)
    with pytest.raises(CustodyError, match="intake disappeared"):
        _invoke_child(f)
    assert len(entered) == 1 and entered[0]._closed
    assert f.imports == producer_reads == []
    assert not (f.directory / bootstrap.FRESH_ENTRY_NAME).exists()


def test_ordinary_child_without_relocated_source_keeps_existing_factory(inputs, monkeypatch):
    pin, _ = inputs.admit("restore")
    imported = []
    def load(name):
        imported.append(name)
        if name != "app":
            return ModuleType(name)
        def main(*, application_startup_factory):
            assert type(application_startup_factory) is bootstrap.BootstrapFactory
            assert application_startup_factory._product_source_custody is None
        return SimpleNamespace(__file__=str(inputs.destination / "app.py"), main=main)
    monkeypatch.setattr(bootstrap.importlib, "import_module", load)
    monkeypatch.setattr(sys, "path", list(sys.path))
    monkeypatch.setattr(sys, "argv", list(sys.argv))
    bootstrap.run_child(str(inputs.directory), pin["path"], pin["sha256"])
    assert imported == ["meerkat_meta.builder", "meerkat_toolkit.host_control",
        "meerkat_toolkit.application_runtime", "app"]
    assert not (inputs.directory / bootstrap.FRESH_ENTRY_NAME).exists()


def test_child_prints_one_timestamped_line_per_startup_phase(inputs, monkeypatch, capsys):
    # The worker stamps every captured child line, so these lines split a slow
    # Start into its phases without a profiler.
    import re
    pin, _ = inputs.admit("restore")
    def load(name):
        if name != "app":
            return ModuleType(name)
        return SimpleNamespace(__file__=str(inputs.destination / "app.py"),
            main=lambda *, application_startup_factory: None)
    monkeypatch.setattr(bootstrap.importlib, "import_module", load)
    monkeypatch.setattr(sys, "path", list(sys.path))
    monkeypatch.setattr(sys, "argv", list(sys.argv))
    bootstrap.run_child(str(inputs.directory), pin["path"], pin["sha256"])
    lines = [line for line in capsys.readouterr().err.splitlines() if line.startswith("forge-child-phase: ")]
    assert [line.split()[1] for line in lines] == [
        "entered", "custody-observed", "sdk-imported", "app-imported", "app-main"]
    assert all(re.fullmatch(r"forge-child-phase: [a-z-]+ \+[0-9]+\.[0-9]{3}s", line) for line in lines)
