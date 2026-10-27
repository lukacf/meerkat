"""Recover original local application ownership inside a claimed action.

The launcher-selected registry and signed accepted Build select the source.
Existing private setup files supply no native completion by their presence.
This module neither publishes files nor repeats Install, inventory or Fresh.
"""
from __future__ import annotations

from copy import deepcopy
from dataclasses import dataclass
import hashlib
import io
from pathlib import Path
import os
import stat
import tomllib
import uuid
import zipfile

from .blueprint import Blueprint
from .artifact_binding import proposal_hash
from .connect_installer_caller import (CustodyError, ExactFile, MAX_INPUT_BYTES,
    _json, _path, _private_directory, _private_parent, _read)
from .deployment import read_receipt, private_configuration_snapshot
from .deployment_application_bootstrap import _read_source, bootstrap_installation_binding
from .deployment_supervisor import pending_custody
from .deployment_updates import (_candidate_source_selection, _same_toml,
    checked_package, require_replacement_history_settled)
from .deployment_sources import read_package_source
from .implementation_workspace import ImplementationWorkspace, MAX_PROJECT_FILES
from .remote_product_artifacts import _joined
from .projects import fingerprint
from .runtime_selection import MetaToolkitRuntimeTarget, parse_candidate_selection


def _identity(value):
    try:
        if str(uuid.UUID(value)) != value:
            raise ValueError
    except (ValueError, TypeError, AttributeError) as error:
        raise CustodyError("Recovery needs the original canonical guide, Build and Install identities.") from error
    return value


def _pin(path):
    path = _path(path)
    _private_parent(path)
    content = _read(path, MAX_INPUT_BYTES)
    pin = ExactFile(path, hashlib.sha256(content).hexdigest())
    if pin.read() != content:
        raise CustodyError("The original private recovery input changed during observation.")
    return pin


@dataclass(frozen=True)
class RecoveredLocalApplication:
    guide_id: str
    deployment_id: str
    build_id: str
    guide_version: int
    blueprint: dict
    factory: object
    host: object
    binding: dict


class _InstalledLocalAdmission:
    """Actual services plus independently accepted source, not an installed flag.

    Only recover_local_application constructs this after its stopped source
    admission. Later factory reads preserve the stable original installation
    association while normal lifecycle journals and the live guide may advance.
    """

    def __init__(self, registry, directory, receipt, package, source, builds, source_directory):
        self.registry, self.directory = registry, directory
        self.guide_id, self.deployment_id = receipt["guide_id"], receipt["deployment_id"]
        self.build_id, self.guide_version = receipt["build_id"], receipt["guide_version"]
        self.blueprint = Blueprint.model_validate(_json(package["blueprint.json"][0])).model_dump(mode="json")
        self.runtime_target = registry._runtime_target_for_blueprint(self.blueprint)
        self.binding = {**bootstrap_installation_binding(receipt),
            "accepted_source_digest": source["source_digest"]}
        self.installation = bootstrap_installation_binding(receipt)
        self._runtime_proof = deepcopy(receipt["installed_candidate_runtime_proof"])
        self.package, self.source = deepcopy(package), deepcopy(source)
        self.builds = tuple(builds)
        self.source_directory = source_directory
        self.acquisition_selection = _pin(source_directory / "source-selection.json")
        self.viewer_selection = _pin(source_directory / "APPLICATION-SELECTION.json")
        # The canonical installed policy retains the explicit source pin. File
        # presence cannot select a factory, but this already admitted original
        # installation must not reconstruct a different default over that pin.
        self._console_policy_pin = _pin(Path(receipt["destination"]) /
            ".mobkit/forge-local-setup/local-application-policy.json")
        policy = _json(self._console_policy_pin.read())
        if (not isinstance(policy, dict) or "console_setup_source" not in policy
                or policy.get("guide_id") != self.guide_id
                or policy.get("deployment_id") != self.deployment_id):
            raise CustodyError("The original local setup policy lost its Console source selection.")
        self.console_setup_selection = None
        if policy["console_setup_source"] is not None:
            from .connect_installer_caller import _artifact_pin, _same_json_value
            from .local_application_factory import LocalApplicationFactory
            pin = _artifact_pin(policy["console_setup_source"])
            _private_parent(pin.path)
            console = LocalApplicationFactory._console_choice(_json(pin.read()))
            if not _same_json_value(console, policy.get("console_setup")):
                raise CustodyError("The retained Console source pin differs from its original local policy.")
            self.console_setup_selection = pin
        self._original = (registry, directory, self.runtime_target, self.guide_id,
            self.deployment_id, self.build_id, self.guide_version, self.acquisition_selection,
            self.viewer_selection, self.console_setup_selection, self._console_policy_pin)
        self._source_pins = (self.acquisition_selection, self.viewer_selection, self._console_policy_pin,
            *((self.console_setup_selection,) if self.console_setup_selection is not None else ()))
        self._source_bytes = tuple(pin.read() for pin in self._source_pins)
        self._installation_files = tuple(_read_source(directory, receipt)[1])
        self.require_current()

    def require_current(self):
        try:
            self._require_current()
        except (AttributeError, KeyError, IndexError, TypeError) as error:
            raise CustodyError("The original local recovery admission is incomplete or changed.") from error

    def _require_current(self):
        from .local_application_registry import LocalApplicationRegistry
        registry = self.registry
        if (type(registry) is not LocalApplicationRegistry
                or (registry, self.directory, self.runtime_target, self.guide_id,
                    self.deployment_id, self.build_id, self.guide_version, self.acquisition_selection,
                    self.viewer_selection, self.console_setup_selection, self._console_policy_pin) != self._original
                or registry._runtime_target_for_blueprint(self.blueprint) is not self.runtime_target):
            raise CustodyError("Preserve the original trusted local recovery admission.")
        registry._require_registry()
        if registry.deployment._directory(self.guide_id) != self.directory:
            raise CustodyError("The original installed selection changed during recovery.")
        receipt = read_receipt(self.directory)
        if (bootstrap_installation_binding(receipt) != self.installation
                or parse_candidate_selection(receipt.get("runtime_candidate")) != self.runtime_target
                or receipt.get("installed_candidate_runtime_proof") != self._runtime_proof):
            raise CustodyError("The original recovered installation association changed.")
        from .connect_application_update_startup import _file_identity
        if any(_file_identity(path) != identity for path, identity in self._installation_files):
            raise CustodyError("The original recovered package or accepted installed source changed.")
        for build_id, digest, frozen, provenance, observed in self.builds:
            root = registry.implementation._directory(self.guide_id, build_id)
            _require_build_observation(root, observed)
        if not self.builds or self.builds[0][:2] != (self.build_id, self.source["source_digest"]):
            raise CustodyError("Recovery lost the actual accepted original Build association.")
        pins = (self.acquisition_selection, self.viewer_selection, self._console_policy_pin,
            *((self.console_setup_selection,) if self.console_setup_selection is not None else ()))
        if pins != self._source_pins or tuple(pin.read() for pin in pins) != self._source_bytes:
            raise CustodyError("The recovered original protected choices changed.")
        registry._require_registry()


def _build_observation(directory):
    """Retain the admitted byte-reader closure without creating locks on GET."""
    from .connect_application_update_startup import _file_identity
    result = []
    for path in (directory / "input.json", directory / "receipt.json"):
        _pin(path)
        result.append((path, _file_identity(path)))
    for root in (directory / "workspace", directory / "acceptance"):
        _private_directory(root)
        for folder, children, files in os.walk(root, followlinks=False):
            folder = _path(Path(folder))
            if len(result) >= MAX_PROJECT_FILES:
                raise CustodyError("The accepted original Build observation exceeds its source bound.")
            result.append((folder, _file_identity(folder)))
            for name in sorted(children):
                child = _path(folder / name)
                if not stat.S_ISDIR(child.lstat().st_mode):
                    raise CustodyError("The accepted original Build contains a linked directory.")
            for name in sorted(files):
                path = _path(folder / name)
                info = path.lstat()
                if (not stat.S_ISREG(info.st_mode) or info.st_uid != os.getuid()
                        or info.st_nlink != 1 or info.st_mode & 0o022):
                    raise CustodyError("The accepted original Build lost ordinary owned source custody.")
                result.append((path, _file_identity(path)))
    return tuple(sorted(result, key=lambda row: str(row[0])))



def _require_build_observation(directory, observed):
    """Recheck every original file and directory without walking their trees.

    The initial signed admission retains the complete tree. Directory identity
    changes catch added or removed children; file identities include ctime so
    restoring mtime cannot hide a byte mutation. Keep the private Build parent
    check that the original input/receipt reader also performed.
    """
    from .connect_application_update_startup import _file_identity
    try:
        directory = _path(directory)
        _private_directory(directory)
        if not observed or any(directory not in path.parents for path, _ in observed):
            raise CustodyError("The independently accepted original Build/source custody changed.")
        unchanged = all(_file_identity(path) == identity for path, identity in observed)
    except OSError as error:
        raise CustodyError("The independently accepted original Build/source custody changed.") from error
    if not unchanged:
        raise CustodyError("The independently accepted original Build/source custody changed.")


def _accepted_build(registry, guide_id, build_id, digest, blueprint, target, *, expected=None):
    """Use the existing private workspace and independent acceptance readers."""
    directory = registry.implementation._directory(guide_id, _identity(build_id))
    _private_directory(directory)
    _private_directory(directory / "workspace")
    _private_directory(directory / "acceptance")
    # Existing readers can create their locks. Require the original retained
    # storage first so recovery never replaces missing acceptance or workspace.
    for name in ("input.json", "receipt.json", "workspace/workspace-state.json",
            "workspace/verification-key", "acceptance/acceptance-state.json", "acceptance/acceptance-key"):
        _pin(directory / name)
    from .connect_application_update_startup import _file_identity
    for name in ("workspace/workspace.lock", "acceptance/acceptance.lock"):
        path = _path(directory / name)
        _private_parent(path)
        identity = _file_identity(path)
        if (not stat.S_ISREG(identity[5]) or identity[6] != os.getuid()
                or identity[7] != 1 or identity[5] & 0o077):
            raise CustodyError("The original workspace or acceptance lock is missing or unsafe.")
    frozen = _json(_pin(directory / "input.json").read())
    receipt = read_receipt(directory)
    if (frozen.get("guide_id") != guide_id or frozen.get("blueprint") != blueprint
            or receipt.get("guide_id") != guide_id or receipt.get("build_id") != build_id
            or frozen.get("runtime_candidate") is None
            or parse_candidate_selection(frozen["runtime_candidate"]) != target):
        raise CustodyError("The original accepted Build differs from the installed plan or trusted product.")
    batch = directory / "workspace/batch-transaction.json"
    if batch.exists() or batch.is_symlink():
        raise CustodyError("Preserve the original unfinished workspace mutation before recovery.")
    snapshot = registry.implementation._accepted_framework(guide_id, build_id, digest,
        frozen, blueprint)
    workspace = ImplementationWorkspace(directory / "workspace", blueprint)
    with workspace._locked():
        state = workspace._state()
        files = workspace._files(state)
        provenance = {key: value for key, value in state["framework"].items() if key != "signature"}
        if (workspace._digest(files) != digest or snapshot.runtime_candidate != frozen["runtime_candidate"]
                or frozen.get("framework") != provenance):
            raise CustodyError("The retained signed Build framework or complete source changed.")
    if expected is not None and (frozen, provenance) != expected:
        raise CustodyError("The original accepted Build input or signed provenance changed.")
    from .acceptance import SemanticAcceptance
    independent = SemanticAcceptance(directory / "acceptance", workspace, frozen).export_files()
    from .application_inventory_source import application_inventory_source
    application_inventory_source({item.path: item.as_bytes() for item in files})
    return frozen, provenance, files, independent, _build_observation(directory)


def bind_accepted_build_inventory(registry, host, guide_id, build_id, *, publish=False):
    """Bind accepted native declarations during deliberate Install or recovery.

    Initial Install alone may copy the accepted bytes to a private native input.
    Recovery only reads that same pin. The original factory and pre-Build
    selections remain unchanged, including for accepted-source continuations.
    """
    if getattr(host, "_accepted_inventory_path", None) is None:
        return
    from .application_inventory_source import application_inventory_source, POLICY_PATH
    from .connect_application_finalizer import _sync_directory
    from .deployment import _publish_new_source
    registry._require_registry()
    host.require_current()
    directory = registry.implementation._directory(guide_id, build_id)
    receipt = read_receipt(directory)
    digest = receipt.get("receipt", {}).get("digest")
    frozen, _, files, _, observation = _accepted_build(
        registry, guide_id, build_id, digest, host._blueprint, host._target)
    selected = {item.path: item.as_bytes() for item in files}
    content = application_inventory_source(selected)
    if content is None:
        raise CustodyError("The original Build requires its accepted native inventory source.")
    path = directory / "accepted-application-inventory.json"
    policy_path = directory / "accepted-application-tool-policy.json"
    for target, data in ((path, content), (policy_path, selected[POLICY_PATH])):
        if publish and not target.exists() and not target.is_symlink():
            if registry._current_plan(guide_id, frozen["guide_version"]) != host._blueprint:
                raise CustodyError("Install requires the original current accepted application plan.")
            def require_current():
                registry._require_registry()
                host.require_current()
                _require_build_observation(directory, observation)
                if registry._current_plan(guide_id, frozen["guide_version"]) != host._blueprint:
                    raise CustodyError("Install requires the original current accepted application plan.")
            _publish_new_source(target, data, 0o600, exact_mode=True, require_current=require_current)
            _sync_directory(directory)
    pin = _pin(path)
    policy_pin = _pin(policy_path)
    _require_build_observation(directory, observation)
    host._bind_accepted_inventory(pin, selected, policy_pin=policy_pin)
    registry._require_registry()


def _accepted_source(registry, directory, receipt, package, source):
    target = parse_candidate_selection(receipt.get("runtime_candidate"))
    if type(target) is not MetaToolkitRuntimeTarget:
        return None
    blueprint = Blueprint.model_validate(_json(package["blueprint.json"][0])).model_dump(mode="json")
    if target != registry._runtime_target_for_blueprint(blueprint):
        raise CustodyError("Recovery requires the exact product selected by the trusted launcher.")
    # The ordinary reader projects source modes for materialization. Recovery
    # joins the signed producer modes and regular file types to this same
    # already hash-checked canonical archive, without changing that projection.
    payload, observed_package = checked_package(directory, "package.zip", receipt["package_sha256"])
    if observed_package != package:
        raise CustodyError("The original checked package changed during recovery observation.")
    with zipfile.ZipFile(io.BytesIO(payload)) as archive:
        package_metadata = {entry.filename: entry.external_attr >> 16 for entry in archive.infolist()}
    report = _json(package["implementation-report.json"][0])
    if (not _same_toml(_candidate_source_selection(package), receipt["runtime_candidate"])
            or not _same_toml(report.get("framework", {}).get("runtime_candidate"), receipt["runtime_candidate"])
            or source["source_kind"] != "application" or source["source_id"] != receipt["build_id"]
            or receipt.get("source_digest") is not None and source["source_digest"] != receipt["source_digest"]
            or receipt.get("brief_hash") is not None and source["brief_hash"] != receipt["brief_hash"]):
        raise CustodyError("The original package report/runtime/source closure changed.")
    builds, seen = [], set()
    build_id, digest = receipt["build_id"], source["source_digest"]
    while True:
        if build_id in seen or len(seen) >= 128:
            raise CustodyError("The signed original source chain is cyclic or exceeds its bounded history.")
        seen.add(build_id)
        frozen, provenance, files, independent, observed = _accepted_build(registry, receipt["guide_id"], build_id,
            digest, blueprint, target)
        if not builds:
            if (frozen["guide_version"] != receipt["guide_version"]
                    or source["brief_hash"] != fingerprint(frozen["brief"])
                    or source["blueprint_hash"] != proposal_hash(frozen["blueprint"])
                    or report.get("framework") != provenance
                    or any((package.get(item.path, (None, None))[0], package_metadata.get(item.path))
                        != (item.as_bytes(), stat.S_IFREG | item.mode)
                        for item in (*[item for item in files if item.path != "SHA256SUMS"], *independent))
                    or any(package_metadata.get(name) != stat.S_IFREG | 0o644
                        for name in ("SHA256SUMS", "implementation-report.json"))
                    or set(package) != {item.path for item in (*files, *independent)} | {"implementation-report.json"}):
                raise CustodyError("The installed package differs from its independently accepted complete Build.")
        builds.append((build_id, digest, deepcopy(frozen), deepcopy(provenance), observed))
        if provenance.get("basis") == "current-compiler":
            if provenance.get("previous") is not None:
                raise CustodyError("The original initial Build contains a foreign predecessor.")
            original = registry.implementation._directory(receipt["guide_id"], build_id)
            source_directory = original / "local-application-source"
            _private_directory(source_directory)
            return _InstalledLocalAdmission(registry, directory, receipt, package, source, builds, source_directory)
        previous = provenance.get("previous")
        if (provenance.get("basis") != "accepted-baseline" or not isinstance(previous, dict)
                or set(previous) != {"build_id", "source_digest"}):
            raise CustodyError("Recovery needs the original signed same-source Build lineage.")
        # Runtime refresh is a distinct owner; no earlier source choice may
        # silently select another product or accepted framework on restart.
        build_id, digest = _identity(previous["build_id"]), previous["source_digest"]


def recover_local_build(registry, guide_id, build_id):
    """Restore an accepted, uninstalled Build only for deliberate Install.

    Signed workspace and independent acceptance bind the original private
    source. The launcher still selects target and model custody. This reader
    creates no source, installation or native runtime and does not read secrets.
    """
    from .local_application_registry import LocalApplicationRegistry
    from .local_application_factory import LocalApplicationFactory
    if type(registry) is not LocalApplicationRegistry or registry.manual_host is not None:
        raise CustodyError("Build recovery requires its original automatic local registry.")
    guide_id, build_id = _identity(guide_id), _identity(build_id)
    registry._require_registry()
    directory = registry.implementation._directory(guide_id, build_id)
    if registry.implementation._directory(guide_id) != directory:
        raise CustodyError("Install needs the current original accepted Build.")
    frozen = _json(_pin(directory / "input.json").read())
    receipt = _json(_pin(directory / "receipt.json").read())
    selection = frozen.get("runtime_candidate")
    target = parse_candidate_selection(selection) if selection is not None else None
    if type(target) is not MetaToolkitRuntimeTarget:
        return None
    blueprint = Blueprint.model_validate(frozen["blueprint"]).model_dump(mode="json")
    version = frozen["guide_version"]
    if (target != registry._runtime_target_for_blueprint(blueprint)
            or registry._current_plan(guide_id, version) != blueprint):
        raise CustodyError("The accepted Build differs from the original launcher or current guide.")
    digest = receipt.get("receipt", {}).get("digest")
    observed, seen = [], set()
    selected_files = None
    current_id = build_id
    while True:
        if current_id in seen or len(seen) >= 128:
            raise CustodyError("The accepted Build source lineage is cyclic or unbounded.")
        seen.add(current_id)
        item, provenance, files, independent, observation = _accepted_build(
            registry, guide_id, current_id, digest, blueprint, target)
        observed.append((registry.implementation._directory(guide_id, current_id), observation))
        if selected_files is None:
            selected_files = {file.path: file.as_bytes() for file in files}
            if item != frozen:
                raise CustodyError("The original selected Build input changed during recovery.")
        if provenance.get("basis") == "current-compiler" and provenance.get("previous") is None:
            source_directory = registry.implementation._directory(guide_id, current_id) / "local-application-source"
            _private_directory(source_directory)
            break
        previous = provenance.get("previous")
        if (provenance.get("basis") != "accepted-baseline" or not isinstance(previous, dict)
                or set(previous) != {"build_id", "source_digest"}):
            raise CustodyError("Install needs its original signed same-source Build lineage.")
        current_id, digest = _identity(previous["build_id"]), previous["source_digest"]
    factory = LocalApplicationFactory(guide_store=registry.guide_store, guide_id=guide_id,
        runtime_target=registry._runtime_target_for_blueprint(blueprint),
        acquisition_selection=_pin(source_directory / "source-selection.json"),
        viewer_selection=_pin(source_directory / "APPLICATION-SELECTION.json"),
        model_configuration=registry._selected_model_configuration(),
        replacement_selection=registry.replacement_selection)
    host = factory()
    if (host._accepted_inventory_path is not None
            and (directory / "accepted-application-inventory.json").exists()
            and (directory / "accepted-application-tool-policy.json").exists()):
        bind_accepted_build_inventory(registry, host, guide_id, build_id)
    access = tomllib.loads(selected_files.get("config/access.toml", b"").decode("utf-8"))
    if (host._blueprint != blueprint
            or access.get("enabled") is not True or access.get("admins") != [host._console_subject]
            or selected_files.get("config/mob.toml") != host._definition.encode("utf-8")
            or any(selected_files.get(name) != pin.read() for name, pin in host._protected)):
        raise CustodyError("The original protected source differs from the independently accepted Build.")
    for root, observation in observed:
        _require_build_observation(root, observation)
    if (registry.implementation._directory(guide_id) != directory
            or registry._current_plan(guide_id, version) != blueprint):
        raise CustodyError("The current accepted Build or guide changed during Install recovery.")
    host.require_current()
    registry._require_registry()
    return version, blueprint, factory, host


async def recover_local_application(registry, guide_id, deployment_id):
    """Recover one original owner only inside an already claimed action lock.

    A nonproduct source returns None after canonical package observation. All
    product substitutions, partial private preparation and uncertain cleanup
    refuse. Returned objects remain unpublished until the registry's exact
    final canonical association check. No files or native inputs are created.
    """
    from .local_application_registry import LocalApplicationRegistry
    from .local_application_factory import LocalApplicationFactory
    from .deployment_worker import Supervisor
    from .templates.storage_setup import project_lock
    if (type(registry) is not LocalApplicationRegistry or not registry.deployment.lock.locked()
            or registry.manual_host is not None):
        raise CustodyError("Recovery requires the automatic trusted registry and original claimed action.")
    guide_id, deployment_id = _identity(guide_id), _identity(deployment_id)
    registry._require_registry()
    if registry._storage is None:
        from .local_application_source import _store_identity
        registry._storage = _store_identity(registry.guide_store)
    directory, receipt, stopped = registry.deployment._stopped_application_owner(guide_id, deployment_id)
    original = deepcopy(bootstrap_installation_binding(receipt))
    def current():
        registry._require_registry()
        stopped()
        if bootstrap_installation_binding(read_receipt(directory)) != original:
            raise CustodyError("The original canonical installation changed during owner recovery.")
    current()
    if deployment_id in registry.deployment._application_preparations:
        raise CustodyError("Settle the original preparation cleanup before recovery.")
    destination = _path(receipt["destination"])
    with project_lock(destination) as lease:
        def owned():
            current()
            lease.require_current()
        owned()
        def inspect_source():
            lease.require_current()
            if pending_custody(directory, receipt):
                raise CustodyError("The original installation support custody is unresolved.")
            require_replacement_history_settled(directory)
            package, observations = _read_source(directory, receipt)
            source = read_package_source(package, receipt)
            admission = _accepted_source(registry, directory, receipt, package, source)
            lease.require_current()
            return admission, observations
        admission, observations = await _joined(inspect_source, _owner_current=owned)
        owned()
        if admission is None:
            return None
        factory = LocalApplicationFactory(guide_store=registry.guide_store, guide_id=guide_id,
            runtime_target=admission.runtime_target, acquisition_selection=admission.acquisition_selection,
            viewer_selection=admission.viewer_selection, _installed_admission=admission,
            console_setup_selection=admission.console_setup_selection,
            model_configuration=registry._selected_model_configuration(),
            replacement_selection=registry.replacement_selection)
        host = factory()
        bind_accepted_build_inventory(registry, host, guide_id, admission.build_id)
        from .forge_application_host import owner_choices_match
        if not owner_choices_match(host, admission.package):
            raise CustodyError("Recovered protected choices differ from the original accepted installation.")
        values, private = private_configuration_snapshot(destination)
        host.validate_installed_settings(guide_id, receipt, destination, admission.package, values)
        from .runtime_dependencies import verify_runtime
        proof = await _joined(verify_runtime, destination, admission.runtime_target.meta.version,
            runtime_target=admission.runtime_target, _owner_current=owned)
        if proof != receipt.get("installed_candidate_runtime_proof"):
            raise CustodyError("The recovered installed SDK and full native prefix differ from the original proof.")
        owner = factory.preparation_owner
        # Validate every original private source before the one constructor
        # can create its ordinary runtime home. Missing/partial choices refuse.
        retained = owner._recovery_inputs(admission, receipt)
        _private_directory(destination / ".mobkit/home")
        owned()
        supervisor = Supervisor(directory)
        inputs = owner._recover(admission, supervisor, retained)
        host._installation_supervisor = supervisor
        host._retained_preparation_inputs = inputs
        host._retained_fresh_paths = deepcopy(inputs.fresh_paths)
        owned()
        owner._require(guide_id, deployment_id)
        inventory = await _joined(owner._recover_startup, lease, _owner_current=owned)
        owned()
        if inventory is not None:
            # Declarations and the normal translator retain the original
            # action task; only their exact file comparison runs off-loop.
            expected = host.preparation_plan(inventory, **owner.declarations(inventory))
            owned()
            await _joined(owner._require_recovered_plan, inventory, expected, lease, _owner_current=owned)
        owned()
        admission.require_current()
        from .connect_application_update_startup import _file_identity
        if (any(_file_identity(path) != identity for path, identity in observations)
                or private_configuration_snapshot(destination)[1] != private):
            raise CustodyError("The original installed source or private settings changed during recovery.")
        return RecoveredLocalApplication(guide_id, deployment_id, admission.build_id,
            admission.guide_version, deepcopy(admission.blueprint), factory, host, deepcopy(admission.binding))
