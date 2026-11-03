"""Explicit trusted Fresh/Restore dispatch for one admitted application source.

These inputs come from the application owner, not a browser, environment flag
or package-generated claim. Admission authenticates bytes and ownership only.
The existing startup owners and Toolkit retain native preparation, activation,
restore and cleanup semantics. This module supplies no producer or replay grant.
"""
from __future__ import annotations

from contextlib import asynccontextmanager
from copy import deepcopy
from dataclasses import dataclass
import asyncio
import hashlib
import importlib
import itertools
import json
import os
from pathlib import Path
import stat
import sys
import uuid

from .connect_accepted_source import retain_accepted_application_source
from .connect_application_callbacks import SelectedApplicationCallbacks, _source_definition
from .connect_application_finalizer import _Current
from .connect_application_startup import (FreshApplicationStartup, RestoredApplicationStartup,
    _ModelConfiguration, _managed_model_classes, _model_configuration_shape, _joined,
    _select_hosted_mcp, _hosted_mcp_runtime_options, _hosted_mcp_startup_options)
from .connect_application_update_startup import _file_identity, _observe
from .connect_installer_caller import (
    CustodyError, ExactFile, ProductGatewayFile, _artifact_pin, _json, _path, _private_directory,
    _private_parent, _read, _same_json_value, _selection,
)
from .connect_policy_input import application_tool_policy_selection
from .deployment import DeploymentService, OWNER_FILE, read_receipt, verify_destination
from .deployment_sources import read_package_source
from . import deployment_updates as updates


FRESH_KEYS = {"kind", "gateway", "preparation_plan", "source_policy", "activation_template",
    "provision_attempt_path", "working_directory", "bundle_path", "activation_path"}
RESTORE_KEYS = {"kind", "gateway", "selection_file", "preparation_plan", "completed_setup"}
ORDINARY_KEYS = {"kind", "gateway", "definition", "compiled_policies", "runtime_candidate"}
STARTUP_OPTIONAL_KEYS = {"model_configuration"}
LIFECYCLE_KEYS = {"interactive_lifecycle_id", "interactive_cleanup_attempt_id", "interactive_cleanup"}
MANAGED_START_WITHOUT_CUSTODY = ("Managed-host classes require their original startup model configuration. "
    "Start Forge with the application model configuration its product provides, or rebuild the application "
    "with models that use your own provider key.")
INSTALLATION_KEYS = set(DeploymentService._configuration_binding({})) - LIFECYCLE_KEYS
FRESH_ENTRY_NAME = "application-bootstrap-entry.json"


def restored_service(receipt):
    """Identify only the explicitly pinned Restore service interpretation."""
    return (receipt.get("execution_mode") == "background"
        and isinstance(receipt.get("service"), dict)
        and "bootstrap_selection" in receipt["service"])


def bootstrap_installation_binding(receipt):
    """Retain the existing source selection across _launch's lifecycle change."""
    result = DeploymentService._configuration_binding(receipt)
    for key in LIFECYCLE_KEYS:
        result.pop(key)
    if isinstance(result["update"], dict):
        result["update"] = updates.update_binding(result["update"])
    if restored_service(receipt):
        # The admission was retained by the stopped interactive owner before
        # its unit existed. The exact service/pin/lifetime is checked separately.
        result.update(execution_mode="interactive", service=None,
            service_supervisor_pid=None, service_instance_id=None,
            service_cleanup_confirmed=None)
    return deepcopy(result)


def same_installation_apart_from_staged_update(retained, current, receipt):
    """A staged or blocked update has changed nothing installed.

    Its pointer alone does not redirect the original build, package, source or
    runtime, so the stopped original can still start. Applying the update
    compares the whole binding, update included.
    """
    if retained == current:
        return True
    if (receipt.get("update") or {}).get("status") not in {"staged", "blocked"}:
        return False
    return {**retained, "update": None} == {**current, "update": None}


def bootstrap_lifecycle_binding(receipt):
    """Freeze this worker's existing lifecycle without source normalization."""
    keys = LIFECYCLE_KEYS | ({"service_supervisor_pid", "service_instance_id",
        "service_cleanup_confirmed"} if restored_service(receipt) else set())
    return json.dumps({key: receipt.get(key) for key in sorted(keys)},
        ensure_ascii=True, allow_nan=False, sort_keys=True, separators=(",", ":"))


def _selected_json(value):
    return json.dumps(value, ensure_ascii=True, allow_nan=False,
        sort_keys=True, separators=(",", ":"))


@dataclass(frozen=True)
class BootstrapObservation:
    directories: tuple
    files: tuple
    current: _Current | None
    destination: Path
    admission: ExactFile | None = None
    selected_json: str | None = None
    lifecycle_json: str | None = None
    gateway: ExactFile | None = None
    model_configuration: _ModelConfiguration | None = None

    def require_current(self):
        """Fence an already authenticated worker result without hashing on loop."""
        if type(self.gateway) is ProductGatewayFile:
            self.gateway.require_current()
        if self.model_configuration is not None:
            self.model_configuration.require_current()
        if (any(_private_directory(path) != identity for path, identity in self.directories)
                or any(_file_identity(path) != identity for path, identity in self.files)):
            raise CustodyError("The original bootstrap source or selected inputs changed.")
        if self.current is not None:
            self.current.require_pointer(self.destination)


def _shape(value):
    if not isinstance(value, dict):
        raise CustodyError("Bootstrap requires explicit original Fresh, Restore or ordinary Meta inputs.")
    keys = ({"fresh": FRESH_KEYS, "restore": RESTORE_KEYS, "ordinary": ORDINARY_KEYS}.get(value.get("kind"))
        if type(value.get("kind")) is str else None)
    if keys is None or not keys <= set(value) or set(value) - keys - STARTUP_OPTIONAL_KEYS:
        raise CustodyError("Bootstrap requires the closed original Fresh or Restore selection, or its closed ordinary Meta selection.")
    if value["kind"] == "ordinary" and (type(value["compiled_policies"]) is not list
            or len(value["compiled_policies"]) > 256):
        raise CustodyError("Ordinary Meta requires its finite original compiled policy selection.")
    if "model_configuration" in value:
        _model_configuration_shape(value["model_configuration"])
    return value["kind"]


def _require_installation(directory, receipt, expected):
    service = restored_service(receipt)
    if service:
        from .deployment_service import verify_service
        verify_service(directory, receipt)
    keys = INSTALLATION_KEYS | ({"runtime_candidate"} if "runtime_candidate" in bootstrap_installation_binding(receipt) else set())
    if (not isinstance(expected, dict) or set(expected) != keys
            or not _same_json_value(bootstrap_installation_binding(receipt), expected)
            or directory.name != receipt.get("deployment_id")
            or directory.parent.name != receipt.get("guide_id")
            or receipt.get("source_kind", "application") != "application"
            or not service and (receipt.get("execution_mode") != "interactive" or receipt.get("service") is not None)):
        raise CustodyError("The original interactive application installation changed.")


def require_service_restore(directory, receipt, selection, selected, *, child=False):
    """Live service execution selects Restore; historical Fresh reads stay valid."""
    if not restored_service(receipt):
        return
    from .deployment_service import verify_service
    service = verify_service(directory, receipt)
    if (not _same_json_value(service["bootstrap_selection"], _artifact_pin(selection).pin())
            or selected["application_startup"]["kind"] != "restore"):
        raise CustodyError("The owned service requires its exact original Restore admission. Fresh is never replayed.")
    if child:
        pid, instance = receipt.get("service_supervisor_pid"), receipt.get("service_instance_id")
        if (type(pid) is not int or pid <= 0 or os.getppid() != pid
                or not isinstance(instance, str) or str(uuid.UUID(instance)) != instance
                or receipt.get("service_cleanup_confirmed") is not False):
            raise CustodyError("The Restore child lost its original service manager lifetime.")


def _read_source(directory, receipt):
    """Read all installed source under original before/after file observations."""
    destination = _path(receipt["destination"])
    paths = [directory / "package.zip", destination / OWNER_FILE]
    files = [(path, _file_identity(path)) for path in paths]
    _private_parent(paths[0])
    _, package = updates.checked_package(directory, "package.zip", receipt["package_sha256"])
    source = read_package_source(package, receipt)
    if (source["source_kind"] != "application"
            or source["brief_hash"] != receipt.get("brief_hash")
            or any(receipt.get(key) is not None and source.get(key) != receipt[key]
                for key in ("source_id", "source_digest"))):
        raise CustodyError("The checked package is not this original admitted application source.")
    for name in package:
        path = destination / name
        files.append((path, _file_identity(path)))
    for path, identity in files:
        mode, uid, links = identity[-3:]
        if (not stat.S_ISREG(mode) or uid != os.getuid() or links != 1
                or identity[2] > 64 * 1024 * 1024
                or path in paths and mode & 0o077):
            raise CustodyError("Bootstrap source must retain owned ordinary files and private custody.")
    verify_destination(receipt, package)
    for path, identity in files:
        if _file_identity(path) != identity:
            raise CustodyError("Application source changed during bootstrap observation.")
    return package, files


def _plan_source(plan, package, destination):
    application = plan.get("application") if isinstance(plan, dict) else None
    realm = application.get("realm") if isinstance(application, dict) else None
    if (not isinstance(plan, dict) or plan.get("deployment_root") != str(destination)
            or not isinstance(realm, dict) or realm.get("context_root") != str(destination)
            or realm.get("state_root") != str(destination / ".mobkit/state")):
        raise CustodyError("The bootstrap plan lost its original application storage roots.")
    try:
        definition = _source_definition({"format": "toml", "text": package["config/mob.toml"][0].decode()})
        selected = _source_definition(application["definition"])
        deployment = _json(package["deployment.json"][0])
        expected = {row["identity"]: row["profile"] for row in deployment["agents"]}
        roster = application["roster"]
        actual = {row["identity"]: row["profile"] for row in roster}
        if ((not expected and ("helper" in application or not application.get("member_classes")))
                or len(expected) != len(deployment["agents"])
                or len(actual) != len(roster) or not _same_json_value(expected, actual)
                or not _same_json_value(definition, selected)):
            raise CustodyError("The selected plan differs from the admitted definition or initial roster.")
        from .connect_policy_input import application_tool_policy_path
        from .application_inventory_source import application_inventory_source
        from .connect_installer_caller import _same_member_classes
        inventory_source = application_inventory_source({name: entry[0] for name, entry in package.items()})
        if inventory_source is not None:
            inventory = _json(inventory_source)
            if (not _same_json_value(inventory["roster"], roster)
                    or not _same_member_classes(inventory["member_classes"], application.get("member_classes", []))):
                raise CustodyError("The preparation plan changed its accepted native class source.")
        raw = package[application_tool_policy_path(deployment)][0]
        intent = application_tool_policy_selection(raw, selected_member_identities=tuple(expected))
        members = application["members"]
        if len(members) != len(roster):
            raise CustodyError("The bootstrap plan needs its exact original selected member rows.")
        mapping = {}
        for row, member in zip(roster, members):
            native = member["member"]
            if (native["mob_id"] != definition["id"] or native["role"] != row["profile"]
                    or not isinstance(native["member"], str) or not native["member"]):
                raise CustodyError("The selected member lost its original fixed application binding.")
            mapping[row["identity"]] = native["member"]
        if len(set(mapping.values())) != len(mapping):
            raise CustodyError("The bootstrap plan repeats a selected native member.")
        for row in intent["members"]:
            row["member_identity"] = mapping[row["member_identity"]]
        source = application["tool_policy"]["source"]
        if (not isinstance(source, dict) or set(source) != {"source_id", "source_digest"}
                or not isinstance(source["source_id"], str) or not source["source_id"]
                or source["source_id"].strip() != source["source_id"]
                or source["source_digest"] != "sha256:" + updates.digest(raw)):
            raise CustodyError("The selected native policy input lost its exact source provenance.")
        intent["source"] = source
        if not _same_json_value(intent, application["tool_policy"]):
            raise CustodyError("The selected plan's policy differs from admitted application intent.")
    except (KeyError, TypeError, UnicodeError, ValueError) as error:
        raise CustodyError("The bootstrap plan lacks its exact finite source/member input join.") from error


def _fresh_paths(value, destination, *, entered=False):
    paths = {name: _path(value[name]) for name in
        ("provision_attempt_path", "working_directory", "bundle_path", "activation_path")}
    if (any(destination not in path.parents for path in paths.values())
            or paths["bundle_path"].parent != destination or paths["activation_path"].parent != destination
            or len(set(paths.values())) != len(paths)
            or paths["activation_path"].name == "current"):
        raise CustodyError("Fresh startup requires separate original private project paths.")
    _private_directory(paths["working_directory"])
    for path in paths.values():
        _private_parent(path)
    reserved = [paths["provision_attempt_path"], Path(str(paths["provision_attempt_path"]) + ".outcome.json"),
        paths["working_directory"] / "finalization.json", paths["working_directory"] / "prepare.json",
        paths["working_directory"] / "validate.json", paths["working_directory"] / "publication.json"]
    if paths["provision_attempt_path"] in {
            paths["working_directory"] / name for name in
            ("finalization.json", "prepare.json", "validate.json", "publication.json")}:
        raise CustodyError("Fresh startup must preserve separate existing native attempt paths.")
    if not entered:
        for path in (paths["bundle_path"], paths["activation_path"],
                destination / (paths["activation_path"].name + ".current.next"), *reserved):
            _path(path, fresh=True)
    return paths


def _observe_inputs(directory, receipt, value, *, entered=False, source_custody=None):
    kind = _shape(value)
    destination = _path(receipt["destination"])
    package, files = _read_source(directory, receipt)
    if kind == "ordinary":
        return _observe_ordinary_inputs(directory, receipt, value, destination,
            package, files, source_custody=source_custody)
    names = ("gateway", "preparation_plan", "source_policy", "activation_template") if kind == "fresh" else (
        "gateway", "selection_file", "preparation_plan", "completed_setup")
    pins = {name: _artifact_pin(value[name]) for name in names}
    selected = updates._candidate_source_selection(package,
        **({"source_custody": source_custody} if source_custody is not None else {}))
    if selected is not None or "runtime_candidate" in receipt:
        from .runtime_selection import MetaToolkitRuntimeTarget, parse_candidate_selection
        if not _same_json_value(selected, receipt.get("runtime_candidate")):
            raise CustodyError("The bootstrap product source lost its original admitted runtime selection.")
        target = parse_candidate_selection(selected)
        if type(target) is MetaToolkitRuntimeTarget:
            from .deployment import product_runtime_source
            artifacts = product_runtime_source(package, selected,
                **({"source_custody": source_custody} if source_custody is not None else {}))
            if pins["gateway"].path != destination / ".bin" / artifacts.prefix / "bin/toolkit_gateway":
                raise CustodyError("The bootstrap gateway differs from this installation's selected full prefix.")
            pins["gateway"] = ProductGatewayFile.observe(pins["gateway"], artifacts)
    directories = {path: _private_directory(path) for path in
        (directory, destination, *(pin.path.parent for name, pin in pins.items()
            if name != "gateway" or type(pin) is not ProductGatewayFile))}
    if type(pins["gateway"]) is ProductGatewayFile:
        directories.setdefault(pins["gateway"].enclosing[0], pins["gateway"].enclosing)
    contents = {}
    for name, pin in pins.items():
        contents[name], identity = _observe(pin, executable=name == "gateway")
        files.append((pin.path, identity))
    if len(contents["preparation_plan"]) > 256 * 1024:
        raise CustodyError("The original bootstrap plan exceeds its native carrier bound.")
    plan = _json(contents["preparation_plan"])
    _plan_source(plan, package, destination)
    model_configuration = (None if "model_configuration" not in value else
        _ModelConfiguration.observe(value["model_configuration"]))
    if model_configuration is not None and sys.platform != "linux":
        raise CustodyError("The selected model credential root requires its original Linux host.")
    if _managed_model_classes(_json(package["deployment.json"][0])) and model_configuration is None:
        raise CustodyError(MANAGED_START_WITHOUT_CUSTODY)
    # The existing reader checks selected helper/member/class fields without inventing
    # a native inventory or projected helper profile.
    SelectedApplicationCallbacks(pins["preparation_plan"])
    current = _Current.observe(destination)
    if kind == "fresh":
        paths = _fresh_paths(value, destination, entered=entered)
        if not entered:
            _path(directory / FRESH_ENTRY_NAME, fresh=True)
        directories.setdefault(paths["working_directory"], _private_directory(paths["working_directory"]))
        milliseconds = plan.get("hold_timeout_ms")
        if type(milliseconds) is not int or not 0 < milliseconds < 240_000:
            raise CustodyError("Fresh preparation needs its bounded hold within the existing startup deadline.")
        from .connect_policy_input import application_tool_policy_path
        if contents["source_policy"] != package[application_tool_policy_path(_json(package["deployment.json"][0]))][0]:
            raise CustodyError("Fresh finalization lost the exact admitted source policy bytes.")
    else:
        selected, artifacts = _selection(contents["selection_file"])
        activation, manifest, local_policy = artifacts
        bundle = manifest.path.parent
        if (selected["deployment_root"] != str(destination) or activation.path.parent != destination
                or current.activation != activation or destination not in bundle.parents
                or local_policy.path.parent != bundle or pins["completed_setup"].path.parent != bundle):
            raise CustodyError("Restore lost its original selected current activation and bundle root.")
        directories.setdefault(bundle, _private_directory(bundle))
        values = []
        for pin in artifacts:
            _private_parent(_path(pin.path))
            content, identity = _observe(pin)
            files.append((pin.path, identity))
            values.append(content)
        manifest_value = _json(values[1])
        if (not isinstance(manifest_value, dict) or set(manifest_value) != {"artifacts"}
                or not isinstance(manifest_value["artifacts"], list)):
            raise CustodyError("Restore needs the original manifest's exact artifact membership.")
        membership = {}
        for row in manifest_value["artifacts"]:
            if (not isinstance(row, dict) or set(row) != {"path", "sha256"}
                    or not isinstance(row["path"], str) or row["path"] in membership):
                raise CustodyError("Restore manifest membership is ambiguous.")
            membership[row["path"]] = row["sha256"]
        if (membership.get(pins["completed_setup"].path.name) != pins["completed_setup"].sha256
                or membership.get(local_policy.path.name) != local_policy.sha256):
            raise CustodyError("Restore setup or local policy differs from the exact selected manifest.")
    observation = BootstrapObservation(tuple(directories.items()), tuple(files), current, destination,
        gateway=pins["gateway"], model_configuration=model_configuration)
    observation.require_current()
    return observation


def _observe_ordinary_inputs(directory, receipt, value, destination, package, files, *, source_custody=None):
    """Join the owner's separate ordinary gateway to its accepted Meta source.

    This is byte custody for an explicitly selected ordinary consumer. The
    trusted owner must have accepted that gateway's producer and SDK pair;
    neither this envelope nor its hash qualifies another native surface.
    No Toolkit plan, helper, compiled policy or native identity is constructed.
    """
    from .ordinary_meta_startup import read_definition
    from .runtime_selection import MetaToolkitRuntimeTarget, parse_candidate_selection
    from .deployment import product_runtime_source
    selected = updates._candidate_source_selection(package,
        **({"source_custody": source_custody} if source_custody is not None else {}))
    if (selected is None or not _same_json_value(selected, receipt.get("runtime_candidate"))
            or not _same_json_value(selected, value["runtime_candidate"])
            or type(parse_candidate_selection(selected)) is not MetaToolkitRuntimeTarget):
        raise CustodyError("Ordinary Meta lost its original accepted product SDK selection.")
    artifacts = product_runtime_source(package, selected,
        **({"source_custody": source_custody} if source_custody is not None else {}))
    blueprint = _json(package["blueprint.json"][0])
    deployment = _json(package["deployment.json"][0])
    if (blueprint.get("acquisition") is not None
            or any(row.get("tools", {}).get("rust_bundles") for row in deployment["classes"])
            or not deployment["agents"]):
        raise CustodyError("The ordinary Meta branch cannot replace a Toolkit application setup owner.")
    gateway, definition = _artifact_pin(value["gateway"]), _artifact_pin(value["definition"])
    if "application/runtime-selection.json" in package:
        retained = _json(package["application/runtime-selection.json"][0])
        if retained != {"format": "forge-ordinary-meta-source-selection-v1", "gateway": gateway.pin()}:
            raise CustodyError("The ordinary gateway differs from its original signed application source.")
    if gateway.path == destination / ".bin" / artifacts.prefix / "bin/toolkit_gateway":
        raise CustodyError("Ordinary Meta needs its separately selected ordinary RPC gateway.")
    if definition.path != destination / "config/mob.toml":
        raise CustodyError("Ordinary Meta needs this accepted application's exact definition.")
    policies = tuple(_artifact_pin(pin) for pin in value["compiled_policies"])
    if len({pin.path for pin in policies}) != len(policies):
        raise CustodyError("Ordinary Meta cannot repeat a selected compiled policy file.")
    directories = {path: _private_directory(path) for path in
        (directory, destination, gateway.path.parent, *(pin.path.parent for pin in policies))}
    _, identity = _observe(gateway, executable=True)
    files.append((gateway.path, identity))
    before = _file_identity(definition.path)
    if read_definition(definition) != package["config/mob.toml"][0] or _file_identity(definition.path) != before:
        raise CustodyError("Ordinary Meta definition differs from its admitted source.")
    files.append((definition.path, before))
    for pin in policies:
        content, identity = _observe(pin)
        content.decode("utf-8")
        files.append((pin.path, identity))
    model = None if "model_configuration" not in value else _ModelConfiguration.observe(value["model_configuration"])
    if model is not None and sys.platform != "linux":
        raise CustodyError("The selected model credential root requires its original Linux host.")
    if _managed_model_classes(deployment) and model is None:
        raise CustodyError(MANAGED_START_WITHOUT_CUSTODY)
    observation = BootstrapObservation(tuple(directories.items()), tuple(files), None, destination,
        gateway=gateway, model_configuration=model)
    observation.require_current()
    return observation


def retain_bootstrap_admission(directory, receipt, application_startup, *, source_custody=None):
    """Run in the owner's joined file lane before queueing one private start."""
    if restored_service(receipt):
        raise CustodyError("The frozen Restore service cannot retain a new Fresh or Restore admission.")
    directory, selected = _path(directory), deepcopy(application_startup)
    installation = bootstrap_installation_binding(deepcopy(receipt))
    _require_installation(directory, receipt, installation)
    if receipt.get("status") != "stopped":
        raise CustodyError("Explicit application bootstrap requires its stopped original installation.")
    original_receipt = _file_identity(directory / "receipt.json")
    current = read_receipt(directory)
    _require_installation(directory, current, installation)
    if current.get("status") != "stopped":
        raise CustodyError("The original installation is no longer stopped.")
    observation = _observe_inputs(directory, current, selected,
        **({"source_custody": source_custody} if source_custody is not None else {}))
    content = json.dumps({"kind": "application-bootstrap", "installation": installation,
        "application_startup": selected}, ensure_ascii=True, allow_nan=False,
        sort_keys=True, separators=(",", ":")).encode()
    path = directory / ("bootstrap-admission-" + str(uuid.uuid4()) + ".json")
    updates.write_archive(path, content)
    pin = {"path": str(path), "sha256": updates.digest(content)}
    observation = BootstrapObservation(observation.directories,
        (*observation.files, (directory / "receipt.json", original_receipt), (path, _file_identity(path))),
        observation.current, observation.destination, gateway=observation.gateway,
        model_configuration=observation.model_configuration)
    read_bootstrap_admission(directory, pin)
    observation.require_current()
    return pin, observation


def read_bootstrap_admission(directory, selection):
    """Read the exact original envelope, never a newly found latest admission."""
    directory = _path(directory)
    value = _admission_value(directory, selection)
    _require_installation(directory, read_receipt(directory), value["installation"])
    return deepcopy(value)


def read_original_fresh_admission(directory, selection, *, in_flight=None):
    """Read an entered Fresh's original admission across this installation's own lineage.

    This is the one reader for the original Fresh admission that a retained
    entry names. Per-start Restore admissions and update admissions keep the
    exact binding of read_bootstrap_admission.
    """
    directory = _path(directory)
    value = _admission_value(directory, selection)
    require_installation_lineage(directory, read_receipt(directory), value["installation"],
        in_flight=in_flight)
    return deepcopy(value)


def _admission_value(directory, selection):
    _private_directory(directory)
    pin = _artifact_pin(selection)
    if pin.path.parent != directory or not pin.path.name.startswith("bootstrap-admission-"):
        raise CustodyError("The bootstrap admission belongs to another installation owner.")
    _private_parent(_path(pin.path))
    value = _json(pin.read())
    if (not isinstance(value, dict) or set(value) != {"kind", "installation", "application_startup"}
            or value["kind"] != "application-bootstrap"):
        raise CustodyError("The original bootstrap admission has changed fields.")
    _shape(value["application_startup"])
    return value


LINEAGE_IDENTITY_KEYS = (*updates.UPDATE_INSTALLATION_KEYS, "source_id", "source_digest", "service",
    "service_supervisor_pid", "service_instance_id", "service_cleanup_confirmed", "runtime_candidate")
# Activation publishes these receipt keys from the candidate's own report.
LINEAGE_SOURCE_KEYS = ("build_id", "package_sha256", "guide_version", "blueprint_hash", "brief_hash")
MAX_LINEAGE_UPDATES = 2048
MAX_LINEAGE_ADMISSIONS = 64
# A worker is changing installed source in these statuses. Only that worker,
# or the action that admits it, may read its own transaction as lineage.
ACTIVE_UPDATE = frozenset({"snapshotting", "applying", "verifying", "restoring",
    "recovering-candidate", "recovering-current"})
UPDATE_IN_FLIGHT = ACTIVE_UPDATE | {"candidate-restored-stopped", "recovery-needed"}
LINEAGE_CHANGED = ("This installation's source is not reached from its original setup through its own "
    "completed updates. Keep its files and update history; install the current build into a new folder.")
UNFINISHED_UPDATE = ("This application's installed source belongs to an update that has not finished. "
    "Recover the current application or restore its previous source, then start it.")


def _lineage_journals(directory, in_flight):
    """Index this installation's update journals that can be source edges.

    The listing and each journal read are bounded. Packages and admissions are
    checked only for the edges that a walk takes.
    """
    root = directory / "updates"
    if not root.exists() and not root.is_symlink():
        return {}
    if root.is_symlink() or not root.is_dir():
        raise CustodyError(LINEAGE_CHANGED)
    listed = list(itertools.islice(root.iterdir(), MAX_LINEAGE_UPDATES + 1))
    if len(listed) > MAX_LINEAGE_UPDATES:
        raise CustodyError(LINEAGE_CHANGED)
    edges = {}
    for child in sorted(listed):
        if child.is_symlink():
            raise CustodyError(LINEAGE_CHANGED)
        if not child.is_dir():
            continue
        try:
            journal = _json(_read(child / "update.json", 4 * 1024 * 1024))
        except (OSError, CustodyError):
            # An unreadable journal is no edge; a walk that needs it refuses.
            continue
        if not isinstance(journal, dict) or journal.get("update_id") != child.name:
            raise CustodyError(LINEAGE_CHANGED)
        status = journal.get("status")
        if status == "completed" or (in_flight is not None and status in UPDATE_IN_FLIGHT
                and _same_json_value(updates.update_binding(journal), in_flight)):
            edges.setdefault((journal.get("from_build_id"), journal.get("previous_package_sha256")),
                []).append((child, journal))
    return edges


# Edges already verified in this process, keyed by their journal directory. A
# hit needs the same file identities, which include ctime, and the same walked
# journal content; anything else is verified again in full.
_verified_edges = {}
MAX_VERIFIED_EDGES = 4096


def _edge_identities(child):
    paths = (child / "update.json", child / "candidate.zip", child / "previous.zip",
        *itertools.islice(sorted(child.glob("admission-*.json")), MAX_LINEAGE_ADMISSIONS))
    return tuple((str(path), _file_identity(path)) for path in paths)


def _require_lineage_edge(directory, child, journal):
    """One walked edge: its exact packages, and this installation's admission of it."""
    try:
        observed = _edge_identities(child)
    except OSError:
        observed = None
    # The walk trusts the journal it read, which can differ from the bytes
    # behind the identities observed here.
    verified = (observed, hashlib.sha256(_selected_json(journal).encode()).hexdigest())
    key = (str(directory), str(child))
    if observed is not None and _verified_edges.get(key) == verified:
        return
    _verify_lineage_edge(directory, child, journal)
    if observed is not None and _edge_identities(child) == observed:
        if len(_verified_edges) >= MAX_VERIFIED_EDGES:
            _verified_edges.clear()
        _verified_edges[key] = verified


def _verify_lineage_edge(directory, child, journal):
    from .artifact_binding import report_brief_hash
    try:
        if updates.update_directory(directory, journal["update_id"]) != child:
            raise CustodyError(LINEAGE_CHANGED)
        _, candidate = updates.checked_package(child, "candidate.zip", journal["package_sha256"])
        _, previous = updates.checked_package(child, "previous.zip", journal["previous_package_sha256"])
        report = _json(candidate["implementation-report.json"][0])
        prior = _json(previous["implementation-report.json"][0])
        if (report["build_id"] != journal["to_build_id"] or prior["build_id"] != journal["from_build_id"]
                or report["guide_version"] != journal["guide_version"]
                or report["blueprint_hash"] != journal["blueprint_hash"]
                or report_brief_hash(report) != journal["brief_hash"]
                or report["verification"]["digest"] != journal["source_digest"]):
            raise CustodyError(LINEAGE_CHANGED)
        binding = updates.update_binding(journal)
        # Apply retained this admission for this installation's own receipt.
        for path in itertools.islice(sorted(child.glob("admission-*.json")), MAX_LINEAGE_ADMISSIONS):
            try:
                selected = updates.read_update_admission(directory, "update", {"path": str(path.absolute()),
                    "sha256": updates.digest(_read(path, 1024 * 1024))})
            except (updates.DraftConflict, CustodyError, OSError):
                continue
            if _same_json_value(selected["binding"], binding):
                return
    except (KeyError, TypeError, ValueError, OSError, updates.DraftConflict) as error:
        raise CustodyError(LINEAGE_CHANGED) from error
    raise CustodyError(LINEAGE_CHANGED)


def _lineage_refusal(receipt, target):
    pointer = receipt.get("update") or {}
    status = pointer.get("status")
    unfinished = status != "completed" and (status in ACTIVE_UPDATE
        or (pointer.get("to_build_id"), pointer.get("package_sha256")) == target)
    return CustodyError(UNFINISHED_UPDATE if unfinished else LINEAGE_CHANGED)


def require_installation_lineage(directory, receipt, original, *, in_flight=None):
    """The original installation, or its own source reached through completed updates.

    Identity keys stay equal, and the update pointer is not lineage. The source
    keys may differ only through a bounded walk of this installation's admitted,
    completed transactions. in_flight is the binding of the one transaction an
    update worker, or the action admitting it, is settling. Native validity of a
    later Restore stays with the existing plan, policy, gateway and activation
    checks.
    """
    directory = _path(directory)
    service = restored_service(receipt)
    if service:
        from .deployment_service import verify_service
        verify_service(directory, receipt)
    current = bootstrap_installation_binding(receipt)
    if (not isinstance(original, dict) or set(original) != set(current)
            or (set(current) - {"runtime_candidate"}) != INSTALLATION_KEYS
            or directory.name != receipt.get("deployment_id")
            or directory.parent.name != receipt.get("guide_id")
            or receipt.get("source_kind", "application") != "application"
            or not service and (receipt.get("execution_mode") != "interactive" or receipt.get("service") is not None)
            or any(not _same_json_value(current.get(key), original.get(key)) for key in LINEAGE_IDENTITY_KEYS)):
        raise CustodyError("The original interactive application installation changed.")
    state = (original["build_id"], original["package_sha256"])
    target = (current["build_id"], current["package_sha256"])
    values = {key: original[key] for key in LINEAGE_SOURCE_KEYS}
    pointer = receipt.get("update")
    edges = _lineage_journals(directory, in_flight) if state != target or pointer is not None else {}
    walked = []
    while state != target:
        following = edges.get(state, [])
        if not following:
            raise _lineage_refusal(receipt, target)
        if len(following) != 1 or len(walked) >= MAX_LINEAGE_UPDATES:
            raise CustodyError(LINEAGE_CHANGED)
        child, journal = following[0]
        if any(row["update_id"] == journal["update_id"] for row in walked):
            raise CustodyError(LINEAGE_CHANGED)
        _require_lineage_edge(directory, child, journal)
        walked.append(journal)
        state = (journal["to_build_id"], journal["package_sha256"])
        values.update({key: journal[key] for key in ("guide_version", "blueprint_hash", "brief_hash")},
            build_id=journal["to_build_id"], package_sha256=journal["package_sha256"])
    if any(not _same_json_value(current[key], values[key]) for key in LINEAGE_SOURCE_KEYS):
        raise CustodyError(LINEAGE_CHANGED)
    if pointer is None:
        if walked:
            raise CustodyError(LINEAGE_CHANGED)
        return
    if walked and walked[-1]["update_id"] == pointer.get("update_id"):
        return
    # Not an edge: a staged, blocked or settled transaction is transparent only
    # from the current source. A changing one is visible only to its own worker.
    own = in_flight is not None and _same_json_value(updates.update_binding(pointer), in_flight)
    if (pointer.get("status") == "completed" or pointer.get("status") in ACTIVE_UPDATE and not own
            or (pointer.get("from_build_id"), pointer.get("previous_package_sha256")) != target):
        raise _lineage_refusal(receipt, target)


def require_owner_installation(directory, receipt, retained, changed, *, setup_entry=True):
    """An owner's retained installation binding still selects this installation.

    The retained binding is the anchor an owner holds in memory, and nothing
    re-anchors it. It holds exactly, apart from a staged or blocked update
    pointer, or, after setup, through this installation's own completed source
    updates. An owner without a setup (setup_entry=False) follows its own
    completed updates the same way. Any other difference refuses with the
    owner's own text. Returns the current binding.
    """
    current = bootstrap_installation_binding(receipt)
    if not isinstance(retained, dict) or set(retained) != set(current):
        raise CustodyError(changed)
    if same_installation_apart_from_staged_update(retained, current, receipt):
        return current
    entry = _path(directory) / FRESH_ENTRY_NAME
    if (setup_entry and not entry.exists() and not entry.is_symlink() or receipt.get("update") is None
            or any(not _same_json_value(current.get(key), retained.get(key)) for key in LINEAGE_IDENTITY_KEYS)):
        raise CustodyError(changed)
    try:
        require_installation_lineage(directory, receipt, retained)
    except CustodyError as error:
        # Without a setup, a source not reached through the installation's own
        # updates keeps the owner's own refusal; an unfinished one says so.
        if not setup_entry and str(error) == LINEAGE_CHANGED:
            raise CustodyError(changed) from None
        raise
    return current


def observe_bootstrap_admission(directory, selection, *, source_custody=None):
    """Authenticate one original pin and its inputs in the joined file lane."""
    directory, selection = _path(directory), deepcopy(selection)
    admission = _artifact_pin(selection)
    identity = _file_identity(admission.path)
    receipt = read_receipt(directory)
    lifecycle = bootstrap_lifecycle_binding(receipt)
    selected = read_bootstrap_admission(directory, selection)
    require_service_restore(directory, receipt, selection, selected)
    selected_json = _selected_json(selected)
    observation = _observe_inputs(directory, receipt, selected["application_startup"],
        **({"source_custody": source_custody} if source_custody is not None else {}))
    require_bootstrap_history_settled(directory, selection)
    entry = directory / FRESH_ENTRY_NAME
    retained_entry = ((entry, _file_identity(entry)),) if entry.exists() or entry.is_symlink() else ()
    observation = BootstrapObservation(observation.directories,
        (*observation.files, (admission.path, identity), *retained_entry),
        observation.current, observation.destination, admission, selected_json, lifecycle, observation.gateway,
        observation.model_configuration)
    require_bootstrap_admission_current(directory, selection, selected, observation)
    return selected, observation


def require_bootstrap_admission_current(directory, selection, selected, observation):
    """Fence original authenticated bytes before entry without archive hashing."""
    directory = _path(directory)
    pin = _artifact_pin(selection)
    if (not isinstance(selected, dict) or set(selected) != {"kind", "installation", "application_startup"}
            or selected["kind"] != "application-bootstrap"
            or pin.path.parent != directory or not pin.path.name.startswith("bootstrap-admission-")
            or observation.admission is None or pin.pin() != observation.admission.pin()
            or observation.selected_json != _selected_json(selected)
            or not any(path == pin.path for path, _ in observation.files)):
        raise CustodyError("The original bootstrap admission or observation changed.")
    _shape(selected["application_startup"])
    receipt = read_receipt(directory)
    _require_installation(directory, receipt, selected["installation"])
    require_service_restore(directory, receipt, selection, selected)
    if observation.lifecycle_json != bootstrap_lifecycle_binding(receipt):
        raise CustodyError("The original bootstrap worker lifecycle changed.")
    observation.require_current()
    require_bootstrap_history_settled(directory, selection)


FRESH_RETAINED = ("This application retains a Fresh bootstrap attempt. Preserve its original inputs "
    "and reconcile the native outcome before an ordinary start or source update.")


def require_bootstrap_history_settled(directory, bootstrap_selection=None):
    """Only authentic Restore inputs may follow an entered uncertain Fresh."""
    directory = _path(directory)
    entry = directory / FRESH_ENTRY_NAME
    if not entry.exists() and not entry.is_symlink():
        return
    if bootstrap_selection is None:
        raise CustodyError(FRESH_RETAINED)
    selected = read_bootstrap_admission(directory, bootstrap_selection)
    require_restore_history_current(directory, selected["application_startup"])


SETUP_RESTORE_CHANGED = ("The application's completed setup changed after this update was staged. Keep "
    "its files and update history, and stage the accepted update again.")
UPDATE_STAGED_BEFORE_SETUP = ("This update was staged before the application's first setup. Its installed "
    "application is unchanged. Stage the accepted update again, then apply it.")


def require_update_bootstrap_history(directory, restore_selection, *, in_flight=None):
    """Admit a source update after setup only through its owner's published Restore.

    restore_selection is the exact pin of the completed Restore that the
    trusted owner published on a normal Stop, recorded when the update was
    staged. It is never request input. An entered Fresh without one refuses.
    Returns the Restore value, or None for an installation that never entered
    Fresh.
    """
    directory = _path(directory)
    entry = directory / FRESH_ENTRY_NAME
    if not entry.exists() and not entry.is_symlink():
        if restore_selection is not None:
            raise CustodyError(SETUP_RESTORE_CHANGED)
        return None
    if restore_selection is None:
        raise CustodyError(FRESH_RETAINED)
    try:
        pin = _artifact_pin(restore_selection)
        _private_parent(_path(pin.path))
        value = _json(pin.read())
    except (CustodyError, OSError, TypeError, ValueError) as error:
        raise CustodyError(SETUP_RESTORE_CHANGED) from error
    if _shape(value) != "restore":
        raise CustodyError(FRESH_RETAINED)
    require_restore_history_current(directory, value, in_flight=in_flight)
    return value


def setup_keeps_candidate(directory, application_startup, candidate):
    """Whether a completed Restore's native inputs also hold for a candidate package.

    These are the plan, roster, policy, runtime selection and model checks the
    next Restore start applies to installed source, read from the candidate's
    bytes. False means the update needs the application's replacement owner.
    """
    receipt = read_receipt(directory)
    try:
        plan = _json(_artifact_pin(application_startup["preparation_plan"]).read())
        _plan_source(plan, candidate, _path(receipt["destination"]))
        if not _same_json_value(updates._candidate_source_selection(candidate), receipt.get("runtime_candidate")):
            return False
        if (_managed_model_classes(_json(candidate["deployment.json"][0]))
                and "model_configuration" not in application_startup):
            return False
    except (CustodyError, KeyError, TypeError, ValueError, UnicodeError, updates.DraftConflict):
        return False
    return True


def require_restore_history_current(directory, application_startup, *, in_flight=None):
    """Read original Fresh lineage for an already selected Restore, without publishing."""
    directory = _path(directory)
    entry = directory / FRESH_ENTRY_NAME
    if not entry.exists() and not entry.is_symlink():
        return
    if _shape(application_startup) != "restore":
        raise CustodyError("An entered Fresh bootstrap requires its actual completed Restore selection.")
    identity = _file_identity(entry)
    mode, uid, links = identity[-3:]
    if (not stat.S_ISREG(mode) or uid != os.getuid() or links != 1
            or mode & 0o077 or identity[2] > 16 * 1024):
        raise CustodyError("The retained Fresh bootstrap entry lost private custody.")
    value = _json(entry.read_bytes())
    if (not isinstance(value, dict) or set(value) != {"kind", "admission", "provision_attempt_path"}
            or value["kind"] != "application-bootstrap-entry"
            or _artifact_pin(value["admission"]).path.parent != directory
            or _path(read_receipt(directory)["destination"]) not in _path(value["provision_attempt_path"]).parents
            or _file_identity(entry) != identity):
        raise CustodyError("The retained original Fresh bootstrap entry changed.")
    original = read_original_fresh_admission(directory, value["admission"], in_flight=in_flight)
    if (original["application_startup"]["kind"] != "fresh"
            or original["application_startup"]["provision_attempt_path"] != value["provision_attempt_path"]):
        raise CustodyError("The retained entry lost its exact original Fresh selection.")
    restore_pin = _artifact_pin(application_startup["selection_file"])
    restore_bytes = _read(restore_pin.path, 64 * 1024)
    if hashlib.sha256(restore_bytes).hexdigest() != restore_pin.sha256:
        raise CustodyError("The retained Restore selection bytes changed.")
    restored, artifacts = _selection(restore_bytes)
    activation, manifest, _ = artifacts
    fresh = original["application_startup"]
    if (("model_configuration" in application_startup) != ("model_configuration" in fresh)
            or "model_configuration" in fresh and not _same_json_value(
                application_startup["model_configuration"], fresh["model_configuration"])):
        raise CustodyError("Restore changed its original Fresh model configuration custody.")
    if (activation.path != _path(fresh["activation_path"])
            or manifest.path.parent != _path(fresh["bundle_path"])
            or restored["deployment_root"] != str(_path(read_receipt(directory)["destination"]))):
        raise CustodyError("Restore does not select the retained original Fresh output paths. Reconcile the original native attempt before another startup.")


def bootstrap_output_paths(directory, application_startup):
    """Finite locations from actual private inputs, without native admission."""
    directory = _path(directory)
    root = _path(read_receipt(directory)["destination"])
    kind = _shape(application_startup)
    if kind == "ordinary":
        require_restore_history_current(directory, application_startup)
        return root, (), None
    original = application_startup
    if kind == "restore":
        require_restore_history_current(directory, application_startup)
        selected, artifacts = _selection(_artifact_pin(application_startup["selection_file"]).read())
        activation, manifest, _ = artifacts
        bundle, activation = manifest.path.parent, activation.path
        if selected["deployment_root"] != str(root) or activation.parent != root or root not in bundle.parents:
            raise CustodyError("Installed source lost its original Restore output locations.")
        entry = directory / FRESH_ENTRY_NAME
        if entry.exists() or entry.is_symlink():
            value = _json(_read(entry, 16 * 1024))
            original = read_original_fresh_admission(directory, value["admission"])["application_startup"]
        else:
            return root, (bundle, activation), activation
    paths = _fresh_paths(original, root, entered=True)
    if kind == "fresh":
        activation = paths["activation_path"]
    outputs = (*paths.values(), Path(str(paths["provision_attempt_path"]) + ".outcome.json"))
    return root, outputs, activation


def bootstrap_child_arguments(directory, selection, *, lifecycle_id=None, owner_source=None):
    """Fixed isolated Python entry, before any authored application import."""
    _artifact_pin(selection)
    trusted_root = str(Path(__file__).resolve().parent.parent)
    script = ("import sys\n" "sys.dont_write_bytecode = True\n"
        f"sys.path.insert(0, {trusted_root!r})\n"
        "from launchkit.deployment_application_bootstrap import run_child\n")
    if owner_source is None:
        script += "run_child(*sys.argv[1:])\n"
    else:
        from .product_installed_source import _trusted_owner_source
        selected_owner = _trusted_owner_source(owner_source)
        # This literal comes from the original trusted launcher selection, not
        # an intake path. The historical Python is compared, never executed.
        script += ("run_child(*sys.argv[1:], "
            f"owner_source_sha256={hashlib.sha256(selected_owner).hexdigest()!r})\n")
    arguments = ("-I", "-c", script, str(_path(directory)), selection["path"], selection["sha256"])
    return (*arguments, lifecycle_id) if lifecycle_id is not None else arguments


class BootstrapFactory:
    """One child lease retains actual accepted source through runtime cleanup.

    Optional relocated custody is an in-memory trusted owner under its original
    held lease. The ordinary child entry does not reconstruct that custody from
    a serialized startup envelope or create a replacement source lease.
    """

    def __init__(self, directory, selection, *, lifecycle_id=None, source_custody=None,
            hosted_mcp_redirect_base=None, hosted_mcp_targets=()):
        _select_hosted_mcp(self, hosted_mcp_redirect_base, hosted_mcp_targets)
        self.directory, self.selection = _path(directory), deepcopy(selection)
        self._product_source_custody = source_custody
        if source_custody is not None:
            from .product_runtime_custody import product_source_custody, require_product_installation
            product_source_custody(source_custody).require_owner_current()
            receipt = read_receipt(self.directory)
            require_product_installation(source_custody, receipt)
        self._lifecycle_id = lifecycle_id
        self._original_lifecycle_id = lifecycle_id
        if lifecycle_id is not None and read_receipt(self.directory).get("interactive_lifecycle_id") != lifecycle_id:
            raise CustodyError("The original child bootstrap lifecycle changed before selection.")
        admission = _artifact_pin(self.selection)
        self._admission_identity = _file_identity(admission.path)
        self.selected = read_bootstrap_admission(self.directory, self.selection)
        receipt = read_receipt(self.directory)
        require_service_restore(self.directory, receipt, self.selection, self.selected, child=True)
        self._service_lifecycle = bootstrap_lifecycle_binding(receipt) if restored_service(receipt) else None
        require_bootstrap_history_settled(self.directory, self.selection)
        if _file_identity(admission.path) != self._admission_identity:
            raise CustodyError("The bootstrap admission changed during child selection.")
        self.inputs = deepcopy(self.selected["application_startup"])
        if hosted_mcp_redirect_base is not None and self.inputs["kind"] not in ("fresh", "restore"):
            raise CustodyError("The hosted MCP selection requires a Toolkit application startup.")
        self._original_directory = self.directory
        self._original_selection = deepcopy(self.selection)
        self._original_selected = deepcopy(self.selected)
        self.source = self.startup = self._observed = None
        self._entered = False
        self._shutdown_task = self._shutdown_runtime = None
        self._cleanup_joined = False

    def _retain_shutdown(self, startup):
        original = startup.shutdown

        async def shutdown(runtime):
            if runtime is not startup.runtime or runtime is None:
                raise CustodyError("Bootstrap cleanup must retain its exact original runtime.")
            if self._shutdown_task is None:
                self._shutdown_runtime = runtime
                self._shutdown_task = asyncio.create_task(original(runtime))
            elif runtime is not self._shutdown_runtime:
                raise CustodyError("Bootstrap cannot replace its original cleanup owner.")
            cancelled = None
            while True:
                try:
                    result = await asyncio.shield(self._shutdown_task)
                    break
                except asyncio.CancelledError as error:
                    if self._shutdown_task.cancelled():
                        raise
                    cancelled = cancelled or error
                except BaseException as error:
                    if cancelled is not None:
                        raise BaseExceptionGroup("Bootstrap cleanup and its waiter failed.",
                            [cancelled, error]) from None
                    raise
            if cancelled is not None:
                raise cancelled
            return result

        # This retains the existing owner's one shutdown invocation. Generated
        # run and the factory exit join it, including after a failed cleanup;
        # neither creates a replacement runtime or repeats the native call.
        startup.shutdown = shutdown

    def require_selection_current(self):
        """Original selection fence, independent of an optional source callback."""
        _hosted_mcp_runtime_options(self)
        if (self.directory != self._original_directory
                or self._lifecycle_id != self._original_lifecycle_id
                or not _same_json_value(self.selection, self._original_selection)
                or not _same_json_value(self.selected, self._original_selected)
                or not _same_json_value(self.inputs, self._original_selected["application_startup"])):
            raise CustodyError("The original child bootstrap selection changed.")
        if _file_identity(Path(self._original_selection["path"])) != self._admission_identity:
            raise CustodyError("The original child bootstrap envelope changed.")
        receipt = read_receipt(self.directory)
        _require_installation(self.directory, receipt, self.selected["installation"])
        require_service_restore(self.directory, receipt, self.selection, self.selected, child=True)
        if self._service_lifecycle is not None and bootstrap_lifecycle_binding(receipt) != self._service_lifecycle:
            raise CustodyError("The original Restore service lifetime changed.")
        if self._lifecycle_id is not None and receipt.get("interactive_lifecycle_id") != self._lifecycle_id:
            raise CustodyError("The original child bootstrap lifecycle changed.")
        if self._observed is not None:
            self._observed.require_current()

    def require_current(self):
        # File readers never invoke the original caller's loop/task callback.
        if self._product_source_custody is not None:
            self._product_source_custody.require_filesystem_current()
        self.require_selection_current()
        if self._product_source_custody is not None:
            from .product_runtime_custody import require_product_installation
            require_product_installation(self._product_source_custody, read_receipt(self.directory))

    def require_owner_current(self):
        _hosted_mcp_runtime_options(self)
        if self._product_source_custody is not None:
            self._product_source_custody.require_owner_current()
        self.require_current()
        if self._product_source_custody is not None:
            self._product_source_custody.require_owner_current()

    @asynccontextmanager
    async def __call__(self, project_lease):
        if self._entered:
            raise CustodyError("The original bootstrap factory was already entered.")
        self._entered = True
        self.require_owner_current()
        require_bootstrap_history_settled(self.directory, self.selection)
        if project_lease.root != Path(self.selected["installation"]["destination"]):
            raise CustodyError("The bootstrap child acquired another project's lease.")
        project_lease.require_current()
        source_custody = self._product_source_custody
        if source_custody is not None and project_lease is not source_custody._lease:
            raise CustodyError("Relocated startup must retain its original actual project lease.")
        from .product_runtime_custody import await_product_validation
        receipt = read_receipt(self.directory)
        self._observed = (await _joined(_observe_inputs, self.directory, receipt, self.inputs)
            if source_custody is None else await await_product_validation(source_custody,
                _observe_inputs, self.directory, receipt, self.inputs, source_custody=source_custody))
        self.require_owner_current()
        # Keep the original envelope as well as every source/input file. Receipt
        # progress remains outside this child observation; AcceptedSource retains
        # its actual post-launch lifecycle binding under this child lease.
        admission = _artifact_pin(self.selection)
        _, admission_identity = (await _joined(_observe, admission) if source_custody is None else
            await await_product_validation(source_custody, _observe, admission))
        self._observed = BootstrapObservation(self._observed.directories,
            (*self._observed.files, (admission.path, admission_identity)),
            self._observed.current, self._observed.destination, gateway=self._observed.gateway,
            model_configuration=self._observed.model_configuration)
        self.require_owner_current()
        self.source = await retain_accepted_application_source(self.directory, receipt,
            project_lease=project_lease, require_owner_current=self.require_owner_current,
            custody_directory=self.directory / ("application-source-" + str(uuid.uuid4())))
        try:
            self.require_owner_current()
            require_bootstrap_history_settled(self.directory, self.selection)
            inputs = self.inputs
            if inputs["kind"] == "fresh":
                def claim_entry():
                    self.source.require_filesystem_current()
                    paths = _fresh_paths(inputs, project_lease.root)
                    # Fixed installation entry refuses reconstruction with new
                    # paths after uncertain Fresh. It is not a native receipt,
                    # and cannot be reset by retaining another envelope.
                    entry = _path(self.directory / FRESH_ENTRY_NAME, fresh=True)
                    updates.write_archive(entry, json.dumps({"kind": "application-bootstrap-entry",
                        "admission": self.selection, "provision_attempt_path": str(paths["provision_attempt_path"])},
                        ensure_ascii=True, sort_keys=True, separators=(",", ":")).encode())
                    parent = os.open(entry.parent, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
                    try:
                        os.fsync(parent)
                    finally:
                        os.close(parent)
                    return entry, _file_identity(entry)
                entry, identity = (await _joined(claim_entry) if source_custody is None else
                    await await_product_validation(source_custody, claim_entry))
                self._observed = BootstrapObservation(self._observed.directories,
                    (*self._observed.files, (entry, identity)), self._observed.current, self._observed.destination,
                    gateway=self._observed.gateway, model_configuration=self._observed.model_configuration)
                self.require_owner_current()
                remote = importlib.import_module("remote_operations")
                if Path(remote.__file__).absolute() != project_lease.root / "remote_operations.py":
                    raise CustodyError("Fresh startup lost its maintained application command executor.")
                self.startup = FreshApplicationStartup(project_lease=project_lease, accepted_source=self.source,
                    gateway=self._observed.gateway,
                    **{name: _artifact_pin(inputs[name]) for name in
                        ("preparation_plan", "source_policy", "activation_template")},
                    command=remote.command, command_timeout=60, startup_timeout=240,
                    **{name: _path(inputs[name]) for name in
                        ("provision_attempt_path", "working_directory", "bundle_path", "activation_path")},
                    model_configuration=inputs.get("model_configuration"),
                    **_hosted_mcp_startup_options(self))
                # Fresh publication legitimately replaces current. Its existing
                # finalizer retains the original pointer and owns that change;
                # source custody must not veto the owner's successful publish.
                self._observed = BootstrapObservation(self._observed.directories,
                    self._observed.files, None, self._observed.destination, gateway=self._observed.gateway,
                    model_configuration=self._observed.model_configuration)
            elif inputs["kind"] == "restore":
                self.startup = RestoredApplicationStartup(project_lease=project_lease, accepted_source=self.source,
                    gateway=self._observed.gateway,
                    **{name: _artifact_pin(inputs[name]) for name in
                        ("selection_file", "preparation_plan", "completed_setup")}, startup_timeout=240,
                    model_configuration=inputs.get("model_configuration"),
                    **_hosted_mcp_startup_options(self))
            else:
                from .ordinary_meta_startup import OrdinaryMetaStartup
                self.startup = OrdinaryMetaStartup(project_lease=project_lease, accepted_source=self.source,
                    gateway=self._observed.gateway, definition=_artifact_pin(inputs["definition"]),
                    compiled_policies=tuple(_artifact_pin(pin) for pin in inputs["compiled_policies"]),
                    startup_timeout=240, model_configuration=inputs.get("model_configuration"))
            self._retain_shutdown(self.startup)
            yield self.startup
        finally:
            original_failure = sys.exception()
            if self.startup is not None and self.startup.runtime is not None:
                try:
                    await self.startup.shutdown(self.startup.runtime)
                except BaseException as cleanup_error:
                    from .connect_application_startup import shutdown_joined
                    # The original cleanup error remains visible even when its
                    # supported same-runtime continuation actually joined.
                    if shutdown_joined(self.startup):
                        self.source.close()
                        self._cleanup_joined = True
                    if original_failure is not None and original_failure is not cleanup_error:
                        raise BaseExceptionGroup("Bootstrap owner and original cleanup failed.",
                            [original_failure, cleanup_error]) from None
                    raise
            self.source.close()
            self._cleanup_joined = True


@asynccontextmanager
async def installed_bootstrap_source(directory, selection, *, require_current=None,
        owner_source=None, owner_source_sha256=None, retained=None):
    """Hold one temporary source/project scope before its original handoff."""
    from .deployment import await_validation
    from .templates.storage_setup import project_lock
    from .product_installed_source import open_installed_product_source
    from .remote_product_artifacts import _current
    _current(require_current)
    selected = await await_validation(read_bootstrap_admission, directory, selection)
    _current(require_current)
    receipt = read_receipt(directory)
    original_lifecycle = bootstrap_lifecycle_binding(receipt)
    admission = _artifact_pin(selection)
    identity = _file_identity(admission.path)
    def current():
        _current(require_current)
        if (bootstrap_lifecycle_binding(read_receipt(directory)) != original_lifecycle
                or _file_identity(admission.path) != identity):
            raise CustodyError("The original bootstrap source owner changed before child handoff.")
        _require_installation(_path(directory), read_receipt(directory), selected["installation"])
        require_service_restore(directory, read_receipt(directory), selection, selected)
    current()
    destination = _path(receipt["destination"])
    with project_lock(destination) as lease:
        async with open_installed_product_source(directory, project_lease=lease,
                require_current=current, owner_source=owner_source,
                owner_source_sha256=owner_source_sha256,
                application_startup=selected["application_startup"], retained=retained) as source:
            yield source
            current()
    current()


async def observe_installed_bootstrap(directory, selection, *, require_current=None,
        owner_source=None, owner_source_sha256=None):
    """Reobserve source under a temporary lease, released before child spawn."""
    from .product_runtime_custody import await_product_validation
    async with installed_bootstrap_source(directory, selection, require_current=require_current,
            owner_source=owner_source, owner_source_sha256=owner_source_sha256) as source:
        result = await await_product_validation(source, observe_bootstrap_admission,
            directory, selection, **({"source_custody": source} if source is not None else {}))
        require_bootstrap_admission_current(directory, selection, *result)
        expected_owner = source._owner_source if source is not None else None
    require_bootstrap_admission_current(directory, selection, *result)
    return *result, expected_owner


class InstalledBootstrapFactory:
    """Reopen immutable installed source under the app's actual child lease."""

    def __init__(self, directory, selection, *, lifecycle_id=None, owner_source=None,
            owner_source_sha256=None, hosted_mcp_redirect_base=None, hosted_mcp_targets=()):
        _select_hosted_mcp(self, hosted_mcp_redirect_base, hosted_mcp_targets)
        self.selection_factory = BootstrapFactory(directory, selection, lifecycle_id=lifecycle_id,
            **_hosted_mcp_startup_options(self))
        self.directory, self.selection = self.selection_factory.directory, self.selection_factory.selection
        self.selected, self.inputs = self.selection_factory.selected, self.selection_factory.inputs
        self._lifecycle = bootstrap_lifecycle_binding(read_receipt(self.directory))
        self._owner_source, self._owner_source_sha256 = owner_source, owner_source_sha256
        self.factory = self.source = self.source_scope = None
        # The inventory before_import loaded, before any authored import; this
        # scope reuses that object and never reads receipt.json for it again.
        self.retained = None
        self._entered = False

    def require_selection_current(self):
        # This is independent of source.require_owner_current, avoiding a
        # factory/source callback cycle after the actual app lease opens.
        self.selection_factory.require_selection_current()
        if bootstrap_lifecycle_binding(read_receipt(self.directory)) != self._lifecycle:
            raise CustodyError("The original installed child lifecycle changed across import or lease handoff.")

    def require_current(self):
        self.require_selection_current()
        if self.factory is not None:
            self.factory.require_current()

    @asynccontextmanager
    async def __call__(self, project_lease):
        if self._entered:
            raise CustodyError("The original installed bootstrap wrapper was already entered.")
        self._entered = True
        self.require_selection_current()
        from .product_installed_source import open_installed_product_source
        self.source_scope = open_installed_product_source(self.directory,
            project_lease=project_lease, require_current=self.require_selection_current,
            owner_source=self._owner_source, owner_source_sha256=self._owner_source_sha256,
            application_startup=self.inputs, retained=self.retained)
        self.source = await self.source_scope.__aenter__()
        try:
            self.require_selection_current()
            self.factory = BootstrapFactory(self.directory, self.selection,
                lifecycle_id=self.selection_factory._lifecycle_id, source_custody=self.source,
                **_hosted_mcp_startup_options(self.selection_factory))
            # Use the same actual private input path projection as pre-admission.
            bootstrap_output_paths(self.directory, self.factory.inputs)
            async with self.factory(project_lease) as startup:
                yield startup
        finally:
            startup = self.factory.startup if self.factory is not None else None
            if startup is None or startup.runtime is None or self.factory._cleanup_joined:
                await self.source_scope.__aexit__(None, None, None)
                self.source_scope = None
            # Unsupported terminal cleanup retains the exact source scope. It
            # cannot itself keep an outer app project lease alive; only the
            # original app/native lifetime owner may settle that boundary.


def startup_refusals():
    """Source custody refusals a child reports to its parent's Start failure."""
    from .native_prefix_receipt import RetainedInventoryError
    from .runtime_candidates import CandidateArtifactError
    return (RetainedInventoryError, CandidateArtifactError, CustodyError)


def report_startup_refusal(error):
    """Write a custody refusal to the readiness channel before the child exits.

    The parent turns it into the reported Start failure, so the file and the
    remedy reach the user instead of only the runtime log. After readiness the
    channel is gone and nothing is written.
    """
    supplied = os.environ.get("MOBKIT_LAUNCH_READY_FD")
    if supplied is None:
        return
    try:
        os.write(int(supplied), (json.dumps({"refused": str(error)}) + "\n").encode())
    except (OSError, ValueError):
        pass


def kept_native_inventory(directory):
    """The verified native inventory receipt.json names for a product Start.

    The child joins the same receipt-bound inventory as its parent's Start:
    forged bytes refuse, and a stale, missing or legacy one keeps the full
    archive read.
    """
    from .native_prefix_receipt import load
    return load(directory, read_receipt(directory))


def run_child(directory, selection_path, selection_sha256, lifecycle_id=None, *,
        owner_source=None, owner_source_sha256=None, hosted_mcp_redirect_base=None, hosted_mcp_targets=()):
    import time
    started = time.monotonic()

    def phase(name):
        # One timestamped line per startup phase in the worker's captured
        # output, so a slow Start can be split by phase. Nothing reads it.
        print(f"forge-child-phase: {name} +{time.monotonic() - started:.3f}s", file=sys.stderr, flush=True)

    phase("entered")
    wrapper = InstalledBootstrapFactory(Path(directory),
        {"path": selection_path, "sha256": selection_sha256}, lifecycle_id=lifecycle_id,
        owner_source=owner_source, owner_source_sha256=owner_source_sha256,
        hosted_mcp_redirect_base=hosted_mcp_redirect_base, hosted_mcp_targets=hosted_mcp_targets)
    factory = wrapper.selection_factory
    destination = Path(factory.selected["installation"]["destination"])
    # Every source/artifact read precedes installed SDK and authored imports.
    # This temporary scope is fully released before app.main acquires its lease.
    async def before_import():
        from .templates.storage_setup import project_lock
        from .product_installed_source import open_installed_product_source
        from .product_runtime_custody import await_product_validation
        wrapper.retained = kept_native_inventory(factory.directory)
        with project_lock(destination) as lease:
            async with open_installed_product_source(factory.directory,
                    project_lease=lease, require_current=wrapper.require_selection_current,
                    owner_source=owner_source, owner_source_sha256=owner_source_sha256,
                    application_startup=factory.inputs, retained=wrapper.retained) as source:
                receipt = read_receipt(factory.directory)
                observation = await await_product_validation(source, _observe_inputs,
                    factory.directory, receipt, factory.inputs,
                    **({"source_custody": source} if source is not None else {}))
                wrapper.require_selection_current()
                observation.require_current()
                return observation, source is not None
    try:
        observation, relocated = asyncio.run(before_import())
    except startup_refusals() as error:
        report_startup_refusal(error)
        raise
    phase("custody-observed")
    wrapper.require_selection_current()
    model_configuration = observation.model_configuration
    if model_configuration is not None:
        model_configuration.require_current()
        # The cleaned worker environment does not inherit this value. Only the
        # original authenticated startup selection supplies native store custody.
        os.environ["XDG_CONFIG_HOME"] = str(model_configuration.credential_config_root)
        model_configuration.require_environment()
    # Import genuine installed SDKs while isolated, before authored code can
    # shadow their package roots or later relative imports.
    imports = (("meerkat_meta.builder", "meerkat_meta.runtime") if factory.inputs["kind"] == "ordinary" else
        ("meerkat_meta.builder", "meerkat_toolkit.host_control", "meerkat_toolkit.application_runtime"))
    if factory.hosted_mcp_redirect_base is not None:
        imports += ("meerkat_toolkit.mcp_auth", "meerkat_toolkit.mcp_auth_native", "meerkat_toolkit.mcp_auth_host")
    for name in imports:
        importlib.import_module(name)
    phase("sdk-imported")
    observation.require_current()
    wrapper.require_selection_current()
    if model_configuration is not None:
        model_configuration.require_environment()
    sys.path.insert(0, str(destination))
    app = importlib.import_module("app")
    if Path(app.__file__).absolute() != destination / "app.py":
        raise CustodyError("The trusted bootstrap selected another application module.")
    phase("app-imported")
    observation.require_current()
    wrapper.require_selection_current()
    if model_configuration is not None:
        model_configuration.require_environment()
    sys.argv = [str(destination / "app.py")]
    phase("app-main")
    try:
        app.main(application_startup_factory=wrapper if relocated else factory)
    except startup_refusals() as error:
        # Only before readiness: report_host_readiness pops the channel.
        report_startup_refusal(error)
        raise
