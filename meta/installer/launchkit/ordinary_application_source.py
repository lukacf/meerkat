"""Original local Build/Install composition for a helperless Meta application.

The launcher separately selects an ordinary gateway and its genuine Meta SDK
product. Source publication adds protected Console metadata, not Toolkit
preparation, native grants or runtime acceptance. No gateway executes here.
"""
from __future__ import annotations

from copy import deepcopy
from dataclasses import dataclass
import hashlib
import io
import json
from pathlib import Path
import stat
import zipfile

from .blueprint import Blueprint
from .connect_application_finalizer import _write_bytes, _sync_directory
from .connect_application_startup import _ModelConfiguration
from .connect_application_update_startup import _file_identity
from .connect_installer_caller import CustodyError, ExactFile, _path, _private_directory, _private_parent
from .deployment_application_bootstrap import (bootstrap_installation_binding, require_bootstrap_history_settled,
    require_owner_installation, same_installation_apart_from_staged_update)
from .deployment_updates import checked_package
from .forge_application_host import application_setup_binding
from .projects import DraftConflict, fingerprint
from .runtime_selection import MetaToolkitRuntimeTarget, parse_candidate_selection


@dataclass(frozen=True)
class RetainedOrdinaryApplication:
    """An explicit launcher choice for one already installed ordinary app.

    The launcher accepts this native/SDK pair independently of saved receipts.
    Recovery still authenticates the original accepted source and installation.
    This selection never supplies a runtime to a new Build or Install.
    """

    guide_id: str
    deployment_id: str
    build_id: str
    package_sha256: str
    runtime_target: MetaToolkitRuntimeTarget
    gateway: ExactFile


def ordinary_application(spec):
    """Select the existing fixed local consumer, without inventing a helper."""
    spec = spec if isinstance(spec, Blueprint) else Blueprint.model_validate(spec)
    return (spec.target in {"mac", "linux"} and spec.acquisition is None
        and not any(row.tools.rust_bundles for row in spec.agent_classes))


def ordinary_installation(receipt, files):
    """Whether an installed package is a helperless application on the selected Meta product.

    Its Start needs its owner's ordinary startup inputs, so no update worker
    starts it: the owner's next Start runs the installed source.
    """
    selection = receipt.get("runtime_candidate")
    if (receipt.get("source_kind", "application") != "application" or selection is None
            or "application/runtime-selection.json" not in files or "blueprint.json" not in files):
        return False
    try:
        from .application_inventory_source import selects_application_inventory
        if selects_application_inventory({name: row[0] for name, row in files.items()}):
            return False
        return (type(parse_candidate_selection(selection)) is MetaToolkitRuntimeTarget
            and ordinary_application(json.loads(files["blueprint.json"][0])))
    except (TypeError, ValueError):
        return False


def _console_source(guide_id):
    subject = "forge-" + guide_id
    configuration = {"fields": [
        {"key": "CONSOLE_USER_ID", "label": "Console user", "secret": False, "required": True,
            "description": "Use " + subject + ", the user selected for this application. Enter it in protected Install settings."},
        {"key": "CONSOLE_LOGIN_PASSWORD", "label": "Console password", "secret": True, "required": True,
            "description": "Choose a password for this application in protected Install settings. Keep it out of the conversation.",
            "min_length": 12, "max_length": 4096}]}
    encode = lambda value: (json.dumps(value, ensure_ascii=True, separators=(",", ":")) + "\n").encode()
    return {"application/configuration.json": encode(configuration),
        "application/console.json": encode({"authentication": "local-password"}),
        "config/access.toml": ("enabled = true\nadmins = [" + json.dumps(subject) + "]\nrules = []\n\n[groups]\n").encode()}


def _source_files(guide_id, gateway):
    # The signed workspace protects this original source selection. Recovery
    # compares it with the launcher's held gateway; it never selects a binary
    # from this file or treats the pin as native producer qualification.
    return {**_console_source(guide_id), "application/runtime-selection.json":
        (json.dumps({"format": "forge-ordinary-meta-source-selection-v1", "gateway": gateway.pin()},
            ensure_ascii=True, sort_keys=True, separators=(",", ":")) + "\n").encode()}


class OrdinaryApplicationHost:
    """Keep the source and the original explicit ordinary gateway choice.

    The enclosing registry owns service, Build and installation associations.
    This object does not grant native callback access or select a replacement
    runtime from package claims. The native gateway/SDK producer acceptance is
    a prerequisite of the launcher's explicit gateway selection.
    """

    def __init__(self, *, guide_id, runtime_target, gateway, blueprint, definition,
            protected, require_current, model_configuration=None):
        if (type(runtime_target) is not MetaToolkitRuntimeTarget or type(gateway) is not ExactFile
                or not callable(require_current) or not ordinary_application(blueprint)):
            raise CustodyError("Ordinary application source needs its original local Meta owner and gateway.")
        self.guide_id, self._target, self.gateway = guide_id, runtime_target, gateway
        self._blueprint, self._definition = deepcopy(blueprint), definition
        self._protected = tuple(protected)
        self._pins = tuple(pin for _, pin in self._protected)
        expected_source = _source_files(guide_id, gateway)
        if {name for name, _ in self._protected} != set(expected_source):
            raise CustodyError("Ordinary application source needs its complete protected Console selection.")
        if dict(self._protected)["application/runtime-selection.json"].read() != expected_source["application/runtime-selection.json"]:
            raise CustodyError("Preserve the original signed ordinary gateway source selection.")
        self._require_owner = require_current
        self._original = (guide_id, runtime_target, gateway, definition, self._protected, require_current)
        self._original_blueprint = deepcopy(self._blueprint)
        self._model = None if model_configuration is None else _ModelConfiguration.observe(deepcopy(model_configuration))
        self._original_model = self._model
        self._source_files = tuple((pin.path, _file_identity(pin.path)) for pin in self._pins)
        self._source_directories = tuple((path, _private_directory(path)) for path in {pin.path.parent for pin in self._pins})
        self._startups = {}
        self.require_current()

    def require_current(self):
        if ((self.guide_id, self._target, self.gateway, self._definition, self._protected, self._require_owner) != self._original
                or self._blueprint != self._original_blueprint or self._model is not self._original_model):
            raise CustodyError("Preserve the original ordinary application source and gateway selection.")
        self._require_owner()
        for path, identity in self._source_directories:
            if _private_directory(path) != identity:
                raise CustodyError("The original ordinary application source directory changed.")
        for path, identity in self._source_files:
            if _file_identity(path) != identity:
                raise CustodyError("The original protected Console source changed.")
        for pin in self._pins:
            pin.read()
        if self._model is not None:
            self._model.require_environment()
            self._model.require_filesystem_current()
        self._require_owner()

    def applies(self, guide_id):
        return guide_id == self.guide_id

    def console_operator_declaration(self):
        self.require_current()
        return {"id": "forge-" + self.guide_id}

    def validate_settings(self, guide_id, settings):
        if not self.applies(guide_id):
            raise CustodyError("The ordinary Console selection belongs to another guide.")
        self.require_current()
        if settings.get("CONSOLE_USER_ID") not in (None, "", "forge-" + guide_id):
            raise DraftConflict("Use the Console user selected for this application's access policy.")

    def validate_installed_settings(self, guide_id, receipt, destination, files, settings):
        from .implementation_workspace import _read_protected_application_file
        self.validate_settings(guide_id, settings)
        for name, pin in self._protected:
            raw = pin.read()
            installed = _read_protected_application_file(_path(destination / name), name)
            if (files.get(name) != (raw, 0o644) or installed.content.encode() != raw
                    or installed.mode & ~0o644 or not installed.mode & 0o400):
                raise CustodyError("The installed Console source differs from its original selected metadata.")
        self.require_current()

    def prepare_workspace(self, guide_id, build_id, workspace):
        self.require_current()
        if (not self.applies(guide_id) or workspace.blueprint.model_dump(mode="json") != self._blueprint
                or workspace.read_file("config/mob.toml") != self._definition
                or parse_candidate_selection(workspace.framework_provenance().get("runtime_candidate")) != self._target):
            raise CustodyError("The reserved ordinary workspace changed its original source or Meta SDK.")
        workspace.adopt_application_inputs({name: pin.path for name, pin in self._protected}, require_current=self.require_current)
        for wheel in (self._target.meta_wheel, self._target.toolkit_wheel, *self._target.dependency_wheels):
            workspace.adopt_wheel("application/vendor/" + wheel.path.name, wheel.path, wheel.sha256)
        self.require_current()

    def retain_installation_inputs(self, deployment, guide_id, deployment_id):
        self.require_current()
        directory, receipt, current = deployment._stopped_application_owner(guide_id, deployment_id)
        _, files = checked_package(directory, "package.zip", receipt["package_sha256"])
        if (parse_candidate_selection(receipt.get("runtime_candidate")) != self._target
                or files["config/mob.toml"][0] != self._definition.encode()
                or any(files.get(name, (None, None))[0] != pin.read() for name, pin in self._protected)):
            raise CustodyError("The ordinary installation differs from its original accepted application source.")
        require_bootstrap_history_settled(directory)
        inputs = {"kind": "ordinary", "gateway": self.gateway.pin(),
            "definition": ExactFile(Path(receipt["destination"]) / "config/mob.toml", hashlib.sha256(self._definition.encode()).hexdigest()).pin(),
            "compiled_policies": [], "runtime_candidate": deepcopy(receipt["runtime_candidate"])}
        if self._model is not None:
            inputs["model_configuration"] = self._model.selection()
        binding = bootstrap_installation_binding(receipt)
        previous = self._startups.get(deployment_id)
        rebound = "The original ordinary installation inputs cannot be rebound."
        if previous is not None:
            if previous[1] != inputs:
                raise CustodyError(rebound)
            # The original anchor stays; the installation's own completed
            # updates, and a restored previous source, keep its inputs.
            if not same_installation_apart_from_staged_update(previous[0], binding, receipt):
                require_owner_installation(directory, receipt, previous[0], rebound, setup_entry=False)
            binding = previous[0]
        current()
        self.require_current()
        self._startups[deployment_id] = (deepcopy(binding), deepcopy(inputs))

    def _selected_startup(self, guide_id, deployment_id):
        self.require_current()
        if not self.applies(guide_id) or deployment_id not in self._startups:
            raise CustodyError("The original ordinary installation has no retained startup selection.")
        return deepcopy(self._startups[deployment_id][1])

    async def prepare_application(self, deployment, guide_id, deployment_id):
        self.retain_installation_inputs(deployment, guide_id, deployment_id)
        return self._selected_startup(guide_id, deployment_id)

    async def continue_application_startup(self, deployment, guide_id, deployment_id):
        return await self.prepare_application(deployment, guide_id, deployment_id)

    async def setup_status(self, deployment, guide_id, deployment_id):
        settings = await deployment.settings_options(guide_id, deployment_id)
        binding = application_setup_binding(settings, guide_id, deployment_id)
        self.require_current()
        missing = [row["label"] for row in settings["fields"]
            if row.get("required") and not row.get("configured") and not row.get("automatic")]
        if missing:
            return {**binding, "available": False, "reason": "private_settings_missing", "missing_fields": missing,
                "detail": "Save the required private settings before starting the application."}
        require_bootstrap_history_settled(deployment._directory(guide_id, deployment_id))
        return {**binding, "available": True, "reason": None, "mode": "ordinary", "kind": "ordinary",
            "label": "Start installed application",
            "detail": "Start the accepted application with its selected ordinary Meta runtime. Startup availability is not native readiness."}

    async def start_application(self, deployment, guide_id, deployment_id):
        return await deployment.start_application(guide_id, deployment_id,
            application_startup=self._selected_startup(guide_id, deployment_id))


def create_ordinary_application_source(*, guide_id, runtime_target, gateway, blueprint,
        definition, output_directory, require_current, model_configuration=None):
    """Publish Console intent only in the real owner's fresh reserved Build."""
    require_current()
    if not ordinary_application(blueprint):
        raise CustodyError("Select the helperless local application consumer for this source.")
    output = _path(Path(output_directory), fresh=True)
    _private_parent(output)
    output.mkdir(mode=0o700)
    protected = []
    for name, content in _source_files(guide_id, gateway).items():
        require_current()
        path = output / name.replace("/", "-")
        _write_bytes(path, content)
        protected.append((name, ExactFile(path, hashlib.sha256(content).hexdigest())))
    _sync_directory(output)
    require_current()
    return OrdinaryApplicationHost(guide_id=guide_id, runtime_target=runtime_target,
        gateway=gateway, blueprint=blueprint, definition=definition, protected=tuple(protected),
        require_current=require_current, model_configuration=model_configuration)


def recover_ordinary_application_source(registry, guide_id, build_id, *, installed=None):
    """Read the original signed accepted source on an explicit owner action.

    A saved file cannot choose the gateway or SDK. The actual launcher keeps
    those choices; the existing acceptance reader joins the whole workspace,
    source digest and bounded predecessor lineage before source retention.
    Recovery creates no Console configuration, helper state or native runtime.
    """
    from .local_application_recovery import _accepted_build, _pin, _require_build_observation
    from .deployment_sources import read_package_source
    from .connect_installer_caller import _json
    registry._require_registry()
    if installed is None and registry.ordinary_meta_gateway is None:
        return None
    directory = registry.implementation._directory(guide_id, build_id)
    frozen = _json(_pin(directory / "input.json").read())
    receipt = _json(_pin(directory / "receipt.json").read())
    blueprint = Blueprint.model_validate(frozen["blueprint"]).model_dump(mode="json")
    target = parse_candidate_selection(frozen.get("runtime_candidate"))
    if not ordinary_application(blueprint):
        return None
    selected_target, gateway = registry._ordinary_runtime_selection(guide_id, blueprint,
        installation=None if installed is None else installed[0])
    if gateway is None:
        return None
    if target != selected_target:
        raise CustodyError("The recovered ordinary source differs from its original launcher SDK product.")
    if installed is None:
        if (registry.implementation._directory(guide_id) != directory
                or registry._current_plan(guide_id, frozen["guide_version"]) != blueprint):
            raise CustodyError("Install needs the original current accepted ordinary Build and guide.")
        digest = receipt.get("receipt", {}).get("digest")
    else:
        installation, package = installed
        source = read_package_source(package, installation)
        if (installation["guide_id"] != guide_id or installation["build_id"] != build_id
                or installation["guide_version"] != frozen["guide_version"]
                or parse_candidate_selection(installation["runtime_candidate"]) != target
                or Blueprint.model_validate(_json(package["blueprint.json"][0])).model_dump(mode="json") != blueprint):
            raise CustodyError("Recovery needs this original installed source and accepted Build.")
        digest = source["source_digest"]
    observed, seen, selected_files = [], set(), None
    current_id = build_id
    while True:
        if current_id in seen or len(seen) >= 128:
            raise CustodyError("The ordinary accepted source lineage is cyclic or unbounded.")
        seen.add(current_id)
        item, provenance, files, independent, observation = _accepted_build(registry,
            guide_id, current_id, digest, blueprint, target)
        root = registry.implementation._directory(guide_id, current_id)
        observed.append((root, observation))
        if selected_files is None:
            selected_files = {file.path: file.as_bytes() for file in files}
            from .application_inventory_source import selects_application_inventory
            if selects_application_inventory(selected_files):
                return None
            if item != frozen:
                raise CustodyError("The original ordinary Build input changed during recovery.")
            if installed is not None:
                from .artifact_binding import proposal_hash
                installation_directory = registry.deployment._directory(guide_id, installation["deployment_id"])
                payload, current_package = checked_package(installation_directory, "package.zip", installation["package_sha256"])
                with zipfile.ZipFile(io.BytesIO(payload)) as archive:
                    metadata = {entry.filename: entry.external_attr >> 16 for entry in archive.infolist()}
                report = _json(package["implementation-report.json"][0])
                if (current_package != package or source["source_id"] != build_id
                        or source["brief_hash"] != fingerprint(frozen["brief"])
                        or source["blueprint_hash"] != proposal_hash(frozen["blueprint"])
                        or report.get("framework") != provenance
                        or any((package.get(entry.path, (None, None))[0], metadata.get(entry.path))
                            != (entry.as_bytes(), stat.S_IFREG | entry.mode)
                            for entry in (*[entry for entry in files if entry.path != "SHA256SUMS"], *independent))
                        or any(metadata.get(name) != stat.S_IFREG | 0o644
                            for name in ("SHA256SUMS", "implementation-report.json"))
                        or set(package) != {entry.path for entry in (*files, *independent)} | {"implementation-report.json"}):
                    raise CustodyError("The installed ordinary package differs from its independently accepted complete Build.")
        if provenance.get("basis") == "current-compiler" and provenance.get("previous") is None:
            source_directory = root / "ordinary-application-source"
            _private_directory(source_directory)
            break
        previous = provenance.get("previous")
        if (provenance.get("basis") != "accepted-baseline" or type(previous) is not dict
                or set(previous) != {"build_id", "source_digest"}):
            raise CustodyError("Recover the original signed ordinary Build lineage.")
        current_id, digest = previous["build_id"], previous["source_digest"]
    protected = tuple((name, _pin(source_directory / name.replace("/", "-")))
        for name in _source_files(guide_id, gateway))
    if any(selected_files.get(name) != pin.read() for name, pin in protected):
        raise CustodyError("The original protected ordinary Console source differs from its accepted Build.")
    if installed is not None and any(package.get(name, (None, None))[0] != value
            for name, value in selected_files.items() if name in {name for name, _ in protected} | {"config/mob.toml"}):
        raise CustodyError("The installed ordinary source differs from its original signed framework.")
    host = OrdinaryApplicationHost(guide_id=guide_id, runtime_target=target,
        gateway=gateway, blueprint=blueprint,
        definition=selected_files["config/mob.toml"].decode(), protected=protected,
        require_current=registry._require_registry,
        model_configuration=registry._selected_model_configuration())
    for root, observation in observed:
        _require_build_observation(root, observation)
    registry._require_registry()
    return frozen["guide_version"], blueprint, None, host
