"""Trusted replacement dispatch under the original deployment admission.

The completed native setup and actual prior desired policy come from the
application owner. This module supplies neither native authority nor a replay
decision. Ordinary packages and service starts cannot select this path.
"""
from __future__ import annotations

from contextlib import asynccontextmanager
from copy import deepcopy
from dataclasses import dataclass
import importlib
import json
import os
from pathlib import Path
import sys
import uuid

from .connect_application_callbacks import _source_definition
from .connect_application_finalizer import _Current
from .connect_application_startup import _joined
from .connect_application_update_startup import (
    ReplacementApplicationStartup, _file_identity, _observe,
)
from .connect_accepted_source import retain_accepted_application_source
from .connect_installer_caller import (
    CustodyError, ExactFile, ProductGatewayFile, _application_update_selection, _artifact_pin, _json,
    _path, _private_directory, _same_json_value,
)
from .connect_policy_input import application_tool_policy_selection
from .deployment import read_receipt, verify_destination
from . import deployment_updates as updates


SELECTION_KEYS = {"kind", "selection_file", "prior_tool_policy", "preparation_plan",
    "gateway", "association_directory", "expected_revision"}
ATTEMPT_NAME = "application-replacement-attempt.json"
ENTRY_NAME = "application-replacement-entry.json"


@dataclass(frozen=True)
class _SelectedObservation:
    directories: tuple
    files: tuple
    current: _Current
    destination: Path

    @property
    def gateway(self):
        return self.files[0][0]

    def require_current(self):
        if type(self.gateway) is ProductGatewayFile:
            self.gateway.require_current()
        # Full hashes run in the joined file worker. Fence its return and later
        # command awaits with metadata, without hashing a gateway on the loop.
        if (any(_private_directory(path) != identity for path, identity in self.directories)
                or any(_file_identity(pin.path) != identity for pin, identity in self.files)):
            raise CustodyError("The original selected replacement inputs changed.")
        self.current.require_pointer(self.destination)


def selected_inputs(value, *, directory, binding, installation, retain_observation=False):
    """Authenticate byte/root/source joins, leaving native semantics to Toolkit.

    This is an explicit input to the trusted owner, never a browser or .env
    selector. The original admission freezes every pin across child startup.
    Permission changes retain the existing refusal until their compiler and
    accepted-policy publication are joined to the production owner.
    """
    if (not isinstance(value, dict) or set(value) != SELECTION_KEYS
            or value["kind"] != "replacement"
            or (value["expected_revision"] is not None
                and (type(value["expected_revision"]) is not int
                     or value["expected_revision"] < 0))):
        raise CustodyError("Replacement startup requires the original explicit native revision decision and exact pins.")
    destination = _path(installation["destination"])
    receipt, root, update = updates.transaction(directory, binding["update_id"],
        expected_binding=binding, expected_installation=installation)
    updates.require_application_policy_continuity(directory, update)
    _, candidate = updates.checked_package(root, "candidate.zip", binding["package_sha256"])
    pins = {name: _artifact_pin(value[name]) for name in
            ("selection_file", "prior_tool_policy", "preparation_plan", "gateway")}
    associations = _path(value["association_directory"])
    if destination not in associations.parents:
        raise CustodyError("Replacement observations must remain inside their original project.")
    selection = updates._candidate_source_selection(candidate)
    if selection is not None:
        from .runtime_selection import MetaToolkitRuntimeTarget, parse_candidate_selection, selected_runtime_artifacts
        target = parse_candidate_selection(selection)
        if type(target) is MetaToolkitRuntimeTarget:
            descriptor = update.get("runtime_dependency_update") or {}
            if (descriptor.get("kind") != "selected-meta-toolkit-runtime-dependency-update"
                    or not _same_json_value(descriptor.get("runtime_candidate"), selection)):
                raise CustodyError("The product gateway lacks its original admitted runtime selection.")
            pins["gateway"] = ProductGatewayFile.observe(pins["gateway"], selected_runtime_artifacts(target))
    directories = {path: _private_directory(path) for path in
        (destination, associations, *(pin.path.parent for name, pin in pins.items()
            if name != "gateway" or type(pin) is not ProductGatewayFile))}
    if type(pins["gateway"]) is ProductGatewayFile:
        directories.setdefault(pins["gateway"].enclosing[0], pins["gateway"].enclosing)
    _, gateway_identity = _observe(pins["gateway"], executable=True)
    selection_bytes, selection_identity = _observe(pins["selection_file"])
    selected, artifacts = _application_update_selection(selection_bytes)
    files = [(pins["gateway"], gateway_identity), (pins["selection_file"], selection_identity)]
    if selected["update_id"] != binding["update_id"]:
        raise CustodyError("The native replacement belongs to a different admitted update.")
    if selected["predecessor"]["deployment_root"] != str(destination):
        raise CustodyError("The native predecessor belongs to another installed project.")
    activation, manifest, local_policy, compilation, candidate_policy = artifacts
    current = _Current.observe(destination)
    if (activation.path.parent != destination or destination not in manifest.path.parent.parents
            or local_policy.path.parent != manifest.path.parent
            or current.activation != activation):
        raise CustodyError("The replacement lost its original current native bootstrap selection.")
    contents = {}
    for pin in (*artifacts, pins["prior_tool_policy"], pins["preparation_plan"]):
        directories.setdefault(pin.path.parent, _private_directory(pin.path.parent))
        contents[pin], identity = _observe(pin)
        files.append((pin, identity))
    plan_bytes = contents[pins["preparation_plan"]]
    if len(plan_bytes) > 256 * 1024:
        raise CustodyError("The original callback plan exceeds its native carrier bound.")
    plan, application = _json(plan_bytes), _json(contents[compilation])
    realm = application.get("realm") if isinstance(application, dict) else None
    if (not isinstance(plan, dict) or plan.get("deployment_root") != str(destination)
            or not _same_json_value(plan.get("application"), application)
            or not isinstance(realm, dict) or realm.get("context_root") != str(destination)
            or realm.get("state_root") != str(destination / ".mobkit/state")):
        raise CustodyError("The callback plan differs from its exact selected application and storage roots.")
    try:
        package_definition = _source_definition({"format": "toml",
            "text": candidate["config/mob.toml"][0].decode("utf-8")})
        selected_definition = _source_definition(application["definition"])
    except (KeyError, UnicodeError, TypeError) as error:
        raise CustodyError("The candidate package lacks its selected finite native definition.") from error
    if not _same_json_value(package_definition, selected_definition):
        raise CustodyError("The native replacement definition differs from its admitted candidate package.")
    try:
        deployment = _json(candidate["deployment.json"][0])
        expected_roster = [{"identity": row["identity"], "profile": row["profile"]}
            for row in deployment["agents"]]
    except (KeyError, TypeError) as error:
        raise CustodyError("The candidate package lacks its original fixed application roster.") from error
    roster = application.get("roster")
    if (not isinstance(roster, list) or len(roster) != len(expected_roster)
            or any(not isinstance(row, dict) or not isinstance(row.get("identity"), str)
                   or not isinstance(row.get("profile"), str) for row in roster)
            or len({row["identity"] for row in roster}) != len(roster)
            or {row["identity"]: row["profile"] for row in roster}
               != {row["identity"]: row["profile"] for row in expected_roster}):
        raise CustodyError("The native replacement roster differs from its admitted fixed application identities.")
    if "application/tool-policy.json" not in candidate:
        raise CustodyError("Selected replacement requires the admitted application's explicit source tool policy.")
    # Preserve source intent through the explicitly selected native member
    # rows. This is the existing compiler-input join, not native compilation or
    # proof of authority. Toolkit still validates the canonical policy against
    # this schema 4 input and its original owner.
    raw_policy = candidate["application/tool-policy.json"][0]
    intent = application_tool_policy_selection(raw_policy,
        selected_member_identities=tuple(row["identity"] for row in expected_roster))
    try:
        members = application["members"]
        if not isinstance(members, list) or len(members) != len(expected_roster):
            raise CustodyError("The selected native policy needs the exact fixed member rows.")
        mapping = {}
        for row, member in zip(roster, members):
            native = member["member"]
            if (native["mob_id"] != package_definition["id"]
                    or native["role"] != row["profile"]
                    or not isinstance(native["member"], str) or not native["member"]):
                raise CustodyError("The selected native policy member lost its original application role.")
            mapping[row["identity"]] = native["member"]
        if len(set(mapping.values())) != len(mapping):
            raise CustodyError("The selected native policy repeats a member binding.")
        for row in intent["members"]:
            row["member_identity"] = mapping[row["member_identity"]]
        actual_policy_input = application["tool_policy"]
        source = actual_policy_input["source"]
        if (not isinstance(source, dict) or set(source) != {"source_id", "source_digest"}
                or not isinstance(source["source_id"], str) or not source["source_id"]
                or source["source_id"].strip() != source["source_id"]
                or source["source_digest"] != "sha256:" + updates.digest(raw_policy)):
            raise CustodyError("The selected native policy input lost its admitted application source provenance.")
        intent["source"] = source
    except (KeyError, TypeError) as error:
        raise CustodyError("The selected native policy lacks its explicit source/member input join.") from error
    if not _same_json_value(intent, actual_policy_input):
        raise CustodyError("The selected native policy input differs from its admitted application tool intent.")
    # Every artifact remains literal input. A native validator checks the
    # compiled policy and original -> actual prior -> candidate sequence.
    observed = _SelectedObservation(tuple(directories.items()), tuple(files), current, destination)
    observed.require_current()
    return (deepcopy(value), observed) if retain_observation else deepcopy(value)


def retain_attempt(directory, admission, selected):
    """Persist one effect-possible attempt before dispatch, without resetting it.

    A new admission envelope cannot erase a prior uncertain attempt for this
    update. Explicit native reconciliation, outside this dispatcher, is needed
    before a successor can be selected.
    """
    root = updates.update_directory(directory, selected["binding"]["update_id"])
    path = root / ATTEMPT_NAME
    content = json.dumps({"schema": 1, "admission": admission,
        "binding": selected["binding"], "installation": selected["installation"],
        "application_startup": selected["application_startup"],
        "state": "dispatch-selected", "policy_publication": "unknown"},
        ensure_ascii=True, sort_keys=True, separators=(",", ":")).encode()
    try:
        _write_effect_record(path, content)
    except FileExistsError:
        raise CustodyError("This replacement already has a retained attempt. Reconcile its exact native outcome before another startup.") from None
    # The exclusive file and its original bytes remain evidence, even if the
    # child never enters. Do not rewrite it from Ready or service health.
    return ExactFile(path, updates.digest(content))


def _write_effect_record(path, content):
    """Persist both exclusive file bytes and their original directory entry."""
    updates.write_archive(path, content)
    parent = os.open(path.parent, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    try:
        os.fsync(parent)
    finally:
        os.close(parent)


def child_arguments(directory, mode, admission, attempt):
    """Import the frozen trusted owner before adding the authored project path."""
    trusted_root = str(Path(__file__).resolve().parent.parent)
    script = ("import sys\n"
        "sys.dont_write_bytecode = True\n"
        f"sys.path.insert(0, {trusted_root!r})\n"
        "from launchkit.deployment_application_startup import run_child\n"
        "run_child(*sys.argv[1:])\n")
    return ("-I", "-c", script, str(Path(directory).absolute()), mode,
        admission["path"], admission["sha256"], str(attempt.path), attempt.sha256)


class ReplacementFactory:
    """Retain the original child lease/source until its exact runtime settles."""

    def __init__(self, directory, mode, admission, attempt):
        self.directory, self.mode = _path(directory), mode
        self.admission, self.attempt = deepcopy(admission), attempt
        self.selected = updates.read_update_admission(self.directory, mode, self.admission)
        self.inputs = self.selected.get("application_startup")
        if self.inputs is None:
            raise CustodyError("The original update admission did not select a replacement startup.")
        self._attempt_bytes = attempt.read()
        expected = {"schema": 1, "admission": self.admission,
            "binding": self.selected["binding"], "installation": self.selected["installation"],
            "application_startup": self.inputs, "state": "dispatch-selected",
            "policy_publication": "unknown"}
        if (attempt.path != updates.update_directory(self.directory,
                self.selected["binding"]["update_id"]) / ATTEMPT_NAME
                or not _same_json_value(_json(self._attempt_bytes), expected)):
            raise CustodyError("The child lost its original retained replacement attempt.")
        self.source = self.startup = None
        self._entered = False
        self._observed = None

    def require_current(self):
        current = updates.read_update_admission(self.directory, self.mode, self.admission)
        if not _same_json_value(current, self.selected) or self.attempt.read() != self._attempt_bytes:
            raise CustodyError("The original replacement admission or attempt changed.")
        updates.transaction(self.directory, self.selected["binding"]["update_id"],
            expected_binding=self.selected["binding"],
            expected_installation=self.selected["installation"])
        if self._observed is not None:
            self._observed.require_current()

    @asynccontextmanager
    async def __call__(self, project_lease):
        if self._entered:
            raise CustodyError("The original replacement child factory was already entered.")
        self._entered = True
        self.require_current()
        if project_lease.root != Path(self.selected["installation"]["destination"]):
            raise CustodyError("The replacement child acquired another project's lease.")
        _, self._observed = await _joined(selected_inputs, self.inputs, directory=self.directory,
            binding=self.selected["binding"], installation=self.selected["installation"],
            retain_observation=True)
        self.require_current()
        receipt = read_receipt(self.directory)
        if (receipt["package_sha256"] != self.selected["binding"]["package_sha256"]
                or receipt["build_id"] != self.selected["binding"]["to_build_id"]):
            raise CustodyError("The selected child has not installed its original admitted candidate source.")
        self.source = await retain_accepted_application_source(self.directory, receipt,
            project_lease=project_lease, require_owner_current=self.require_current,
            custody_directory=self.directory / ("application-source-" + str(uuid.uuid4())))
        try:
            self.require_current()
            # Cross-process entry is exclusive under this actual child lease.
            # Retaining a new envelope or reconstructing a factory cannot
            # bypass Toolkit's otherwise process-local one-use lifetime.
            entry = updates.update_directory(self.directory,
                self.selected["binding"]["update_id"]) / ENTRY_NAME
            try:
                _write_effect_record(entry, json.dumps({"schema": 1,
                    "attempt": self.attempt.pin(), "admission": self.admission},
                    ensure_ascii=True, sort_keys=True, separators=(",", ":")).encode())
            except FileExistsError:
                raise CustodyError("This replacement already entered its original child. Reconcile it before another native startup.") from None
            self.require_current()
            inputs = self.inputs
            self.startup = ReplacementApplicationStartup(project_lease=project_lease,
                accepted_source=self.source, gateway=self._observed.gateway,
                selection_file=_artifact_pin(inputs["selection_file"]),
                prior_tool_policy=_artifact_pin(inputs["prior_tool_policy"]),
                preparation_plan=_artifact_pin(inputs["preparation_plan"]),
                association_directory=_path(inputs["association_directory"]),
                expected_revision=inputs["expected_revision"], startup_timeout=240)
            yield self.startup
        finally:
            # The generated host also owns shutdown. A retained runtime here
            # means that owner did not settle it; keep the lease and source
            # until this exact cleanup call completes or reports its failure.
            original_failure = sys.exception()
            if self.startup is not None and self.startup.runtime is not None:
                try:
                    await self.startup.shutdown(self.startup.runtime)
                except BaseException as cleanup_error:
                    # Preserve descriptors and the failed original cleanup
                    # task. A fresh cleanup call is not evidence of settlement.
                    if original_failure is not None and original_failure is not cleanup_error:
                        raise BaseExceptionGroup("Replacement owner and original cleanup failed.",
                            [original_failure, cleanup_error]) from None
                    raise
            self.source.close()


def run_child(directory, mode, admission_path, admission_sha256, attempt_path, attempt_sha256):
    """Fixed trusted entrypoint for one original selected replacement."""
    factory = ReplacementFactory(Path(directory), mode,
        {"path": admission_path, "sha256": admission_sha256},
        ExactFile(Path(attempt_path), attempt_sha256))
    destination = Path(factory.selected["installation"]["destination"])
    receipt = read_receipt(factory.directory)
    if (receipt["package_sha256"] != factory.selected["binding"]["package_sha256"]
            or receipt["build_id"] != factory.selected["binding"]["to_build_id"]):
        raise CustodyError("The original replacement candidate changed before application import.")
    _, files = updates.checked_package(factory.directory, "package.zip", receipt["package_sha256"])
    verify_destination(receipt, files)
    factory.require_current()
    # In isolated Python these resolve through the installed SDK before any
    # authored directory can shadow the package. Their retained package paths
    # also govern later relative imports made during prepare/connect.
    for name in ("meerkat_meta.builder", "meerkat_toolkit.host_control",
            "meerkat_toolkit.application_update"):
        importlib.import_module(name)
    # launchkit was imported from the trusted closure above, before application
    # code can import its own similarly named package.
    sys.path.insert(0, str(destination))
    import app
    if Path(app.__file__).absolute() != destination / "app.py":
        raise CustodyError("The trusted replacement entrypoint selected another application module.")
    sys.argv = [str(destination / "app.py")]
    app.main(application_startup_factory=factory)
