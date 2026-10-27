"""Trusted source composition for one existing Forge application owner.

The supplying host selects these pins and the already qualified runtime target.
Nothing here discovers an operator, authenticates a Console user, constructs a
native authority, approves a preparation plan or owns a second runtime. Initial
builds use the existing workspace hook; startup uses the existing stopped owner.
"""
from __future__ import annotations

from collections.abc import Callable
from contextlib import nullcontext
from copy import deepcopy
from dataclasses import dataclass, field
import inspect
import json
import math
import os
from pathlib import Path
import sys
from typing import TYPE_CHECKING

from .blueprint import Blueprint
from .connect_application_callbacks import _source_definition
from .connect_installer_caller import (CustodyError, ExactFile, NativeApplicationInventory, ProductGatewayFile,
    _inventory_input, _json, _path, _private_parent, _same_json_value, _same_member_classes)
from .connect_policy_input import application_tool_policy_selection, _compiler_input
from .projects import DraftConflict
from .runtime_selection import MetaToolkitRuntimeTarget, parse_candidate_selection
from .templates_v2.console_auth import ISSUER

if TYPE_CHECKING:
    from .deployment_worker import Supervisor


_ABSENT = object()


@dataclass(frozen=True)
class ApplicationPreparationInputs:
    """Retained private inputs selected only by their original trusted host.

    The supplying factory retains the inactive Supervisor and existing files.
    Its provider only reads that selection, including during GET: it must not
    construct another Supervisor, create files, acquire leases or enter native
    inventory. The existing locked action and bridge own those effects.
    """

    supervisor: Supervisor
    declarations: Callable[[NativeApplicationInventory], dict]
    inventory_attempt_path: Path
    preparation_plan_path: Path
    startup_selection_path: Path
    activation_template: ExactFile
    fresh_paths: dict
    command_timeout: float
    model_configuration: dict | None = None
    _model_configuration_custody: object = field(init=False, repr=False, compare=False, default=None)

    def __post_init__(self):
        if self.model_configuration is not None:
            from .connect_application_startup import _ModelConfiguration
            selected = _ModelConfiguration.observe(self.model_configuration)
            if not _same_json_value(self.model_configuration, selected.selection()):
                raise CustodyError("Preparation needs its exact original model configuration selection.")
            object.__setattr__(self, "_model_configuration_custody", selected)

    def _require_model_configuration(self):
        selected = self._model_configuration_custody
        original = None if selected is None else selected.selection()
        if not _same_json_value(self.model_configuration, original):
            raise CustodyError("The original supplier changed its retained model configuration.")
        if selected is not None:
            selected.require_filesystem_current()
        return original


def _pin(entry):
    if (not isinstance(entry, dict) or not isinstance(entry.get("path"), str)
            or not isinstance(entry.get("sha256"), str)):
        raise CustodyError("The trusted source selection lacks an exact original file pin.")
    pin = ExactFile(Path(entry["path"]), entry["sha256"])
    content = pin.read()
    if "bytes" in entry and (type(entry["bytes"]) is not int or entry["bytes"] != len(content)):
        raise CustodyError("The original source size differs from its selected bytes.")
    return pin, content


def application_setup_binding(settings, guide_id, deployment_id):
    """Public installed-source readback from existing private-settings checks."""
    if not isinstance(settings, dict) or settings.get("deployment_id") != deployment_id:
        raise CustodyError("Application setup readback belongs to another installation.")
    return {"scope": "selected-stopped-application-inputs",
        "guide_id": guide_id, "deployment_id": deployment_id,
        **{name: settings.get(name) for name in ("version", "guide_version", "build_id",
            "source_digest", "package_sha256")}}


def owner_choices_match(host, files):
    """Whether a package keeps the protected choices of this source owner.

    Its Blueprint, native definition, protected application files and runtime
    selection must equal the owner's. Stage, the installation lookup and
    recovery use this one reader, so Stage refuses what the owner would.
    """
    from .deployment_updates import _candidate_source_selection
    try:
        blueprint = json.loads(files["blueprint.json"][0])
        definition = files["config/mob.toml"][0]
        selection = _candidate_source_selection(files)
        target = parse_candidate_selection(selection) if selection is not None else None
    except (KeyError, TypeError, ValueError, UnicodeError):
        return False
    from .application_inventory_source import application_inventory_source
    try:
        inventory = application_inventory_source({name: entry[0] for name, entry in files.items()})
    except (CustodyError, KeyError, TypeError, ValueError):
        return False
    inventory_pin = getattr(host, "_inventory_pin", None)
    return ((inventory is None or inventory_pin is not None and inventory == inventory_pin.read())
        and (getattr(host, "_accepted_inventory_path", None) is None
            or inventory is not None and host._accepted_inventory_pin is not None)
        and blueprint == host._blueprint and definition == host._definition.encode("utf-8")
        and target == host._target
        and all(files.get(name, (None, None))[0] == pin.read() for name, pin in host._protected))


class ForgeApplicationHost:
    """Host-injected selection, never initialized from an HTTP request.

    require_current belongs to the original trusted selection transaction.
    startup_selection, when supplied, returns its retained private Fresh/Restore
    envelope for the exact installation. It cannot substitute source declarations
    for authentication or native completion. The ordinary startup admission
    performs its existing package, destination, history and authority joins.
    replacement_selection belongs to the installed replacement owner, whose
    currentness is separate from initial acquisition. It only selects a retained
    private envelope; the existing update admission checks its source/native joins.
    """

    def __init__(self, *, guide_id: str, runtime_target: MetaToolkitRuntimeTarget,
            acquisition_selection: ExactFile, viewer_selection: ExactFile,
            require_current: Callable[[], None], startup_selection=None,
            preparation_inputs: Callable[[str, str], ApplicationPreparationInputs] | None = None,
            retain_preparation_inputs: Callable[[str, str, Supervisor], ApplicationPreparationInputs] | None = None,
            replacement_selection: Callable[[str, str, str], ExactFile] | None = None,
            source_custody=None, startup_continuation=None):
        if (not isinstance(guide_id, str) or not guide_id or guide_id.strip() != guide_id
                or type(runtime_target) is not MetaToolkitRuntimeTarget
                or not isinstance(acquisition_selection, ExactFile)
                or not isinstance(viewer_selection, ExactFile)
                or not callable(require_current)
                or startup_selection is not None and not callable(startup_selection)
                or preparation_inputs is not None and startup_selection is None
                or retain_preparation_inputs is not None and preparation_inputs is None):
            raise CustodyError("Application composition needs its existing trusted host selection.")
        if preparation_inputs is not None:
            call = getattr(preparation_inputs, "__call__", None)
            if (not callable(preparation_inputs) or any(inspect.iscoroutinefunction(value)
                    or inspect.isasyncgenfunction(value) for value in (preparation_inputs, call))):
                raise CustodyError("Preparation inputs need a synchronous original host supplier.")
            try:
                inspect.signature(preparation_inputs).bind(guide_id, "installation")
            except (TypeError, ValueError) as error:
                raise CustodyError("Preparation inputs must select the exact guide and installation.") from error
        if retain_preparation_inputs is not None:
            call = getattr(retain_preparation_inputs, "__call__", None)
            if (not callable(retain_preparation_inputs) or any(inspect.iscoroutinefunction(value)
                    or inspect.isasyncgenfunction(value) for value in (retain_preparation_inputs, call))):
                raise CustodyError("Install completion needs a synchronous trusted input retainer.")
            try:
                inspect.signature(retain_preparation_inputs).bind(guide_id, "installation", object())
            except (TypeError, ValueError) as error:
                raise CustodyError("The input retainer must accept the original installation Supervisor.") from error
        if replacement_selection is not None:
            call = getattr(replacement_selection, "__call__", None)
            if (not callable(replacement_selection) or any(inspect.iscoroutinefunction(value)
                    or inspect.isasyncgenfunction(value) for value in (replacement_selection, call))):
                raise CustodyError("Replacement inputs need a synchronous read-only installed owner supplier.")
            try:
                inspect.signature(replacement_selection).bind(guide_id, "installation", "update")
            except (TypeError, ValueError) as error:
                raise CustodyError("Replacement inputs must select the exact guide, installation and update.") from error
        if startup_continuation is not None:
            if (startup_selection is None or not callable(startup_continuation)
                    or not inspect.iscoroutinefunction(startup_continuation)):
                raise CustodyError("Startup continuation needs the original async installation owner.")
            try:
                inspect.signature(startup_continuation).bind(object(), guide_id, "installation")
            except (TypeError, ValueError) as error:
                raise CustodyError("Startup continuation must retain its exact deployment owner and installation.") from error
        self.guide_id = guide_id
        self._target = runtime_target
        self._product_source_custody = source_custody
        self._destination_source_identity = None
        if source_custody is None:
            self._require_owner_current = require_current
        else:
            from .product_runtime_custody import product_source_custody, require_product_source
            product_source_custody(source_custody).require_owner_current()
            require_product_source(source_custody, runtime_target)
            if source_custody.receipt["guide_id"] != guide_id:
                raise CustodyError("The original staged source belongs to another selected guide.")
            self._destination_source_identity = (deepcopy(source_custody._context),
                source_custody._owner_source)
            def owner_current():
                current_source = product_source_custody(self._product_source_custody)
                if (not _same_json_value(current_source._context, self._destination_source_identity[0])
                        or current_source._owner_source != self._destination_source_identity[1]):
                    raise CustodyError("The original destination host source selection changed.")
                current_source.require_owner_current()
                require_current()
                require_product_source(current_source, self._target)
                current_source.require_owner_current()
            self._require_owner_current = owner_current
        self._startup_selection = startup_selection
        self._preparation_inputs = preparation_inputs
        self._retain_preparation_inputs = retain_preparation_inputs
        self._replacement_selection = replacement_selection
        self._original_replacement_selection = replacement_selection
        self._startup_continuation = startup_continuation
        self._retained_preparation_inputs = None
        self._retained_fresh_paths = None
        self._installation_supervisor = None
        self._destination_installation_supervisor = None
        self._destination_installation_entered = False
        require_current()
        acquisition = _json(acquisition_selection.read())
        viewer = _json(viewer_selection.read())
        if (not isinstance(acquisition, dict) or not isinstance(viewer, dict)
                or viewer.get("base_source_selection") != acquisition_selection.pin()
                or not isinstance(viewer.get("source_inputs"), dict)
                or set(viewer["source_inputs"]) != {"application/configuration.json",
                    "application/console.json", "config/access.toml"}):
            raise CustodyError("The viewer source must retain this exact acquisition selection.")
        operator = viewer.get("operator")
        if (not isinstance(operator, dict) or not isinstance(operator.get("subject"), str)
                or not operator["subject"] or operator["subject"].strip() != operator["subject"]
                or operator.get("private_configuration_key") != "CONSOLE_USER_ID"):
            raise CustodyError("The original viewer source lacks its explicit local Console operator selection.")
        self._console_subject = operator["subject"]
        try:
            pins_and_bytes = {name: _pin(acquisition[name]) for name in
                ("blueprint", "definition", "original_inventory", "source_policy")}
            protected = {name: _pin(entry)[0] for name, entry in viewer["source_inputs"].items()}
        except (KeyError, TypeError) as error:
            raise CustodyError("The original application source selection is incomplete.") from error
        self._policy_source_path = acquisition.get("source_policy_path", "application/tool-policy.json")
        from .application_inventory_source import POLICY_PATH
        if self._policy_source_path not in {"application/tool-policy.json", POLICY_PATH}:
            raise CustodyError("Preserve the selected application policy source path.")
        protected[self._policy_source_path] = pins_and_bytes["source_policy"][0]
        self._blueprint = Blueprint.model_validate(_json(pins_and_bytes["blueprint"][1])).model_dump(mode="json")
        original = _inventory_input(pins_and_bytes["original_inventory"][1])
        self._policy_bytes = pins_and_bytes["source_policy"][1]
        self._policy = application_tool_policy_selection(self._policy_bytes,
            selected_member_identities=tuple(row["identity"] for row in original["roster"]))
        helper_selection = self._blueprint.get("acquisition")
        if ((helper_selection is not None) != ("helper_identity" in original)
                or self._blueprint["project"]["slug"] != original["mob_id"]
                or (helper_selection is not None and (
                    helper_selection["helper_identity"] != original["helper_identity"]
                    or helper_selection["helper_profile"] != "dedicated"
                    or helper_selection["helper_tools"] != "checked_installation"))):
            raise CustodyError("The selected blueprint and original helper roster differ.")
        from .compiler import application_inventory_input
        # Retain selected native class declarations without deriving their
        # grants or callback catalog from Blueprint role text. An absent list
        # stays absent in the original source; explicit empty is also valid
        # for a fixed helperless inventory.
        class_arguments = {}
        if "member_classes" in original or helper_selection is None:
            class_arguments["member_classes"] = original.get("member_classes", [])
        regenerated = _inventory_input(application_inventory_input(self._blueprint, **class_arguments))
        if "member_classes" not in original:
            regenerated.pop("member_classes", None)
        if not _same_json_value(original, regenerated):
            raise CustodyError("The selected blueprint changed the original complete fixed roster.")
        self._inventory = original
        self._inventory_pin = pins_and_bytes["original_inventory"][0]
        self._accepted_inventory_path = acquisition.get("accepted_inventory_path")
        self._accepted_inventory_pin = None
        self._accepted_policy_pin = None
        if self._accepted_inventory_path is not None:
            from .application_inventory_source import INVENTORY_PATH
            if self._accepted_inventory_path != INVENTORY_PATH:
                raise CustodyError("Preserve the original accepted-inventory source selection.")
        self._definition = pins_and_bytes["definition"][1].decode("utf-8")
        self._protected = tuple(sorted(protected.items()))
        self._pins = tuple(dict.fromkeys((acquisition_selection, viewer_selection,
            *(pin for pin, _ in pins_and_bytes.values()), *protected.values())))
        self.require_current()

    def _bind_accepted_inventory(self, pin, files, *, policy_pin):
        """Retain exact accepted bytes on this same original source owner.

        The registry's deliberate Install or recovery action supplies the
        independently accepted snapshot and its private pin. No lookup creates
        a file, and the original pre-Build source selections remain pinned.
        """
        if self._accepted_inventory_path is None:
            return
        from .application_inventory_source import application_inventory_source, POLICY_PATH
        self.require_current()
        content = application_inventory_source(files)
        if type(pin) is not ExactFile or content is None or pin.read() != content:
            raise CustodyError("Bind native classes only from this exact accepted application source.")
        if type(policy_pin) is not ExactFile or policy_pin.read() != files[POLICY_PATH]:
            raise CustodyError("Bind seed grants only from this exact accepted application policy.")
        if self._accepted_inventory_pin is not None:
            if pin != self._accepted_inventory_pin or policy_pin != self._accepted_policy_pin:
                raise CustodyError("Preserve the original accepted application inventory pin.")
            return
        if self._retained_preparation_inputs is not None or self._installation_supervisor is not None:
            raise CustodyError("Native inventory must be selected before installation preparation.")
        original = _inventory_input(content)
        base = {key: value for key, value in self._inventory.items() if key != "member_classes"}
        if not _same_json_value(base, {key: value for key, value in original.items() if key != "member_classes"}):
            raise CustodyError("The accepted inventory changed the original Build roster.")
        self._accepted_inventory_pin = self._inventory_pin = pin
        self._inventory = original
        self._accepted_policy_pin = policy_pin
        self._policy_source_path = POLICY_PATH
        self._policy_bytes = policy_pin.read()
        self._policy = application_tool_policy_selection(self._policy_bytes,
            selected_member_identities=tuple(row["identity"] for row in original["roster"]))
        self._protected = tuple(sorted((*self._protected, (POLICY_PATH, policy_pin))))
        self._pins = (*self._pins, pin, policy_pin)
        self.require_current()

    def preparation_plan(self, inventory, *, deployment_root, owner, startup, authorities,
            realm_storage, output, hold_timeout_ms, source_id, helper=_ABSENT, acquisition=_ABSENT):
        """Join genuine inventory to explicit current public declarations.

        The host supplies the existing public owner fields, including qualified
        principal values and its retained issuance IDs. Helper and acquisition
        declarations are supplied together only for a selected helper. Native
        class declarations stay bound to the original inventory source.
        This method does not invent their encoding or turn declarations into
        authentication. The original installation transaction must retain and
        select the returned draft before the existing Fresh startup consumes it.
        """
        if type(inventory) is not NativeApplicationInventory:
            raise CustodyError("Preparation needs the original native inventory observation.")
        self.require_current()
        if type(inventory.gateway) is not ProductGatewayFile:
            raise CustodyError("Preparation inventory differs from the selected native product owner.")
        if self._product_source_custody is None:
            if inventory.gateway.artifacts.selection != self._target:
                raise CustodyError("Preparation inventory differs from the selected native product owner.")
        else:
            from .runtime_selection import selected_runtime_artifacts
            selected_runtime_artifacts(self._target, verified_artifacts=inventory.gateway.artifacts,
                source_custody=self._product_source_custody, destination=deployment_root)
        if not isinstance(source_id, str) or not source_id or source_id.strip() != source_id:
            raise CustodyError("Preparation policy provenance needs its exact selected source ID.")
        original, actual = inventory.authenticate()
        if (inventory.compiler_input != self._inventory_pin
                or not _same_json_value(original, self._inventory)
                or not isinstance(owner, dict) or not isinstance(owner.get("native_host"), dict)):
            raise CustodyError("Preparation changed the original inventory input or owner selection.")
        native = owner["native_host"]
        human = self.console_operator_declaration()
        routes = native.get("routes")
        if not isinstance(routes, list):
            raise CustodyError("Preparation needs its selected original Console routes.")
        console_routes = [route for route in routes
            if isinstance(route, dict) and isinstance(route.get("origin"), dict)
            and route["origin"].get("kind") == "local_console"]
        if (not _same_json_value(native.get("remote_operator"), human)
                or (actual["members"] and not console_routes)
                or any(not _same_json_value(route["origin"].get("requester"), human)
                    or not _same_json_value(route.get("represented_subject"), human)
                    for route in console_routes)):
            raise CustodyError("Preparation changed the original logical Console operator or represented self.")
        selected_members = native.get("members")
        if (native.get("realm") != actual["realm_id"]
                or not isinstance(selected_members, list)
                or len(selected_members) != len(actual["members"])
                or any(not isinstance(member, dict) or not _same_json_value(member.get("member"), row["member"])
                    for member, row in zip(selected_members, actual["members"]))):
            raise CustodyError("The host member declarations differ from the actual same-order native inventory.")
        has_helper = "helper_identity" in original
        if not isinstance(realm_storage, dict) or "realm_id" in realm_storage:
            raise CustodyError("Preparation changed its selected storage declaration.")
        if has_helper:
            helper_binding = next(row["member"] for row in actual["members"]
                if row["identity"] == original["helper_identity"])
            if (not isinstance(helper, dict) or not _same_json_value(helper.get("member"), helper_binding)
                    or acquisition is _ABSENT):
                raise CustodyError("Preparation changed its exact helper or acquisition selection.")
        elif helper is not _ABSENT or acquisition is not _ABSENT:
            raise CustodyError("Helperless preparation must omit helper and acquisition declarations.")
        policy = deepcopy(self._policy)
        mapping = {row["identity"]: row["member"]["member"] for row in actual["members"]}
        for row in policy["members"]:
            row["member_identity"] = mapping[row["member_identity"]]
        policy = _json(_compiler_input(policy, self._policy_bytes, source_id))
        application = {"schema": 4,
                "realm": {"realm_id": actual["realm_id"], **deepcopy(realm_storage)},
                "definition": {"format": "toml", "text": self._definition},
                "roster": deepcopy(original["roster"]), "members": deepcopy(selected_members)}
        if has_helper:
            application.update(helper=deepcopy(helper), helper_tools="checked_installation",
                helper_profile="dedicated")
        application["tool_policy"] = policy
        if "member_classes" in original:
            application["member_classes"] = deepcopy(original["member_classes"])
        classes = original.get("member_classes", [])
        if "member_classes" in native and not _same_member_classes(native["member_classes"], classes):
            raise CustodyError("The host owner changed the selected native member classes.")
        selected_owner = deepcopy(owner)
        if classes:
            selected_owner["native_host"]["member_classes"] = deepcopy(classes)
        plan = {"schema": 1, "deployment_root": str(deployment_root), "owner": selected_owner,
            "startup": deepcopy(startup), "authorities": deepcopy(authorities), "application": application}
        if has_helper:
            plan["acquisition"] = deepcopy(acquisition)
        plan.update(output=str(output), hold_timeout_ms=hold_timeout_ms)
        from .connect_application_plan import MAX_PLAN_BYTES, _assemble
        assembled = _assemble(plan)
        result = (json.dumps(assembled, ensure_ascii=True, allow_nan=False, separators=(",", ":")) + "\n").encode()
        if len(result) > MAX_PLAN_BYTES:
            raise CustodyError("The original preparation draft exceeds its native byte limit.")
        self.require_current()
        inventory.authenticate()
        return result

    def applies(self, guide_id):
        return guide_id == self.guide_id

    def console_operator_declaration(self):
        """Selected logical owner only, without authentication or native grants.

        The current Meta public contract derives the eventual verified Human
        from the exact Console issuer and subject using the oidc-issuer: domain.
        Initialize selects this logical root before that later authentication.
        """
        return {"kind": "human", "id": self._console_subject,
            "qualification": {"kind": "qualified", "trust_domain_id": "oidc-issuer:" + ISSUER}}

    def validate_settings(self, guide_id, settings):
        """Preserve the selected viewer subject without defaults or secret reads."""
        if not self.applies(guide_id):
            return
        self.require_current()
        self._validate_console_subject(settings)

    def _validate_console_subject(self, settings):
        supplied = settings.get("CONSOLE_USER_ID") if isinstance(settings, dict) else None
        if supplied not in (None, "", self._console_subject):
            raise DraftConflict("Use the Console user selected for this application's access policy.")

    def validate_installed_settings(self, guide_id, receipt, destination, files, settings):
        """Join the locked installed source to retained viewer bytes, not a new proposal.

        DeploymentService supplies its already authenticated package and owned
        destination under the original stopped/source checks and project lease.
        Initial Install and native-team validation retain their original guard.
        """
        if not self.applies(guide_id):
            return
        if receipt.get("source_kind", "application") != "application":
            self.validate_settings(guide_id, settings)
            return
        from .implementation_workspace import _read_protected_application_file
        for pin in self._pins:
            pin.read()
        protected = dict(self._protected)
        for name in ("application/configuration.json", "application/console.json", "config/access.toml"):
            original = protected[name].read()
            # Host-adopted public text uses the existing package mode projection.
            if files.get(name) != (original, 0o644):
                raise CustodyError("The installed viewer source differs from the original selected application.")
            path = _path(destination / name)
            installed = _read_protected_application_file(path, name)
            # Ordinary source publication honors the owner's private umask.
            if (path.lstat().st_uid != os.getuid() or installed.mode & ~0o644
                    or not installed.mode & 0o400
                    or installed.content.encode("utf-8") != original):
                raise CustodyError("The installed viewer bytes or permissions changed before private settings validation.")
        self._validate_console_subject(settings)
        for pin in self._pins:
            pin.read()

    def require_current(self):
        self._require_owner_current()
        for pin in self._pins:
            pin.read()
        self._require_owner_current()

    def _bind_destination_source(self, source, *, factory):
        """Renew only this original destination owner's settled operation lease.

        Private inputs and the retained inactive preparation Supervisor remain
        the same objects. This cannot select another package, destination,
        runtime, source owner or an unresolved child lifetime.
        """
        from .destination_application_factory import DestinationApplicationFactory
        from .product_runtime_custody import product_source_custody, require_product_source
        if (type(factory) is not DestinationApplicationFactory or factory.host is not self
                or self._destination_source_identity is None):
            raise CustodyError("Destination source renewal needs its original composed factory.")
        source = product_source_custody(source)
        source.require_owner_current()
        if (not _same_json_value(source._context, self._destination_source_identity[0])
                or source._owner_source != self._destination_source_identity[1]):
            raise CustodyError("Destination source renewal changed the original selected installation.")
        require_product_source(source, self._target)
        previous = self._product_source_custody
        if previous is source:
            self.require_current()
            return
        if previous is None or not previous._closed:
            raise CustodyError("The original destination source operation is still entered.")
        installer = self._destination_installation_supervisor
        if installer is not None:
            installer._destination_install_cleanup.require_settled()
        supervisor = self._installation_supervisor
        if supervisor is not None:
            if (supervisor._product_source_custody is not previous
                    or supervisor.child is not None or supervisor._child_admission is not None
                    or supervisor._selected_sdk_install_task is not None or supervisor.stop.is_set()):
                raise CustodyError("The original destination preparation lifetime remains unresolved.")
        for pin in self._pins:
            pin.read()
        if supervisor is not None:
            supervisor._product_source_custody = source
        self._product_source_custody = source
        self.require_current()

    def initial_build_options(self, guide_id):
        """Supply only the host target; the build owner admits source and bytes."""
        if not self.applies(guide_id):
            return {}
        self.require_current()
        return {"runtime_target": self._target}

    async def install_staged_source(self, *, settings, require_current):
        """Install this actual destination source under its existing held owner.

        The caller must retain the source scope and project lease until this
        original installer and any uncertain cleanup have settled. This entry
        neither selects native preparation inputs nor constructs a remote host.
        """
        from .destination_product_install import install_staged_product_source
        return await install_staged_product_source(self, settings=settings,
            require_current=require_current)

    async def cleanup_staged_installation(self, *, supervisor):
        """Continue the same retained installer cleanup without retrying Install."""
        from .destination_product_install import _DestinationInstallCleanup
        cleanup = getattr(supervisor, "_destination_install_cleanup", None)
        if (supervisor is not self._destination_installation_supervisor
                or type(cleanup) is not _DestinationInstallCleanup
                or cleanup.supervisor is not supervisor
                or cleanup.source is not self._product_source_custody):
            raise CustodyError("Cleanup needs this host's exact retained destination installer.")
        return await cleanup.continue_cleanup()

    def prepare_workspace(self, guide_id, build_id, workspace):
        """Adopt original protected source through its signed existing baseline."""
        if not self.applies(guide_id):
            return
        self.require_current()
        if (not isinstance(build_id, str) or not build_id
                or workspace.blueprint.model_dump(mode="json") != self._blueprint
                or parse_candidate_selection(workspace.framework_provenance().get("runtime_candidate")) != self._target):
            raise CustodyError("The initial workspace differs from the host-selected application or product.")
        original = _source_definition({"format": "toml", "text": self._definition})
        seeded = _source_definition({"format": "toml", "text": workspace.read_file("config/mob.toml")})
        if original != seeded:
            raise CustodyError("The product compiler changed the selected original application definition.")
        target = self._target
        if self._product_source_custody is not None:
            from .runtime_selection import selected_runtime_artifacts
            target = selected_runtime_artifacts(self._target,
                source_custody=self._product_source_custody).selection
        wheels = (target.meta_wheel, target.toolkit_wheel, *target.dependency_wheels)
        names = ["application/vendor/" + wheel.path.name for wheel in wheels]
        if len(set(names)) != len(names):
            raise CustodyError("The host-selected SDK/dependency wheels alias a package path.")
        workspace.adopt_application_inputs(
            {name: pin.path for name, pin in self._protected}, require_current=self.require_current)
        for name, wheel in zip(names, wheels):
            self.require_current()
            workspace.adopt_wheel(name, wheel.path, wheel.sha256)
        self.require_current()

    def _selected_startup_observation(self, guide_id, deployment_id, *, selected=None):
        if not self.applies(guide_id):
            raise CustodyError("The private application selection belongs to another guide.")
        if self._startup_selection is None:
            raise CustodyError("The original host has not retained its selected Fresh or Restore inputs.")
        self.require_current()
        if selected is None:
            selected = self._startup_selection(guide_id, deployment_id)
        if not isinstance(selected, ExactFile):
            raise CustodyError("The existing installation owner must retain its exact private startup selection.")
        original = selected.read()
        value = _json(original)
        from .deployment_application_bootstrap import _shape
        _shape(value)
        if value["kind"] == "fresh" and value.get("source_policy") != dict(self._protected)[self._policy_source_path].pin():
            raise CustodyError("Fresh startup changed the original selected application policy.")
        self.require_current()
        if selected.read() != original:
            raise CustodyError("The original private startup selection changed before admission.")
        return selected, value

    def _selected_startup(self, guide_id, deployment_id):
        return self._selected_startup_observation(guide_id, deployment_id)[1]

    def _deployment_callbacks(self, deployment, guide_id, deployment_id, names):
        """Bind direct callbacks or this registry's exact original host lookup."""
        selector = getattr(deployment, "application_host_selection", None)
        if selector is None:
            owner = self
        else:
            from .local_application_registry import LocalApplicationRegistry
            from .destination_application_factory import DestinationApplicationFactory
            owner = getattr(selector, "__self__", None)
            if (type(owner) not in (LocalApplicationRegistry, DestinationApplicationFactory)
                    or owner.deployment is not deployment
                    or selector != owner.selected_host or selector(guide_id, deployment_id) is not self):
                raise CustodyError("Application callbacks need this installation's original selected host.")
        callbacks = tuple(getattr(deployment, service_name, None) for service_name, _ in names)
        if any(callback != getattr(owner, host_name) for callback, (_, host_name) in zip(callbacks, names)):
            raise CustodyError("Preserve the original selected application callbacks.")
        return selector, callbacks

    async def continue_application_startup(self, deployment, guide_id, deployment_id):
        """Retain completed setup only inside an actual installation action.

        Ordinary status suppliers remain read-only. Stop joins the original
        runtime first, making the retained Restore visible to the next status
        read. A newly claimed Start may also complete this same handoff.
        """
        if not self.applies(guide_id) or self._startup_continuation is None:
            return
        if not deployment.lock.locked():
            raise CustodyError("Startup continuation requires its original locked application action.")
        names = (("application_setup_continuation", "continue_application_startup"),
            ("application_setup_selection", "_selected_startup"))
        callbacks = self._deployment_callbacks(deployment, guide_id, deployment_id, names)
        original = self._startup_continuation
        self.require_current()
        selected = await original(deployment, guide_id, deployment_id)
        if (self._startup_continuation != original
                or self._deployment_callbacks(deployment, guide_id, deployment_id, names) != callbacks):
            raise CustodyError("The original application continuation owner changed during its action.")
        self.require_current()
        if selected is not None:
            current, value = self._selected_startup_observation(guide_id, deployment_id)
            if type(selected) is not ExactFile or selected != current or value["kind"] != "restore":
                raise CustodyError("Startup continuation must retain its actual completed Restore selection.")

    def _selected_replacement(self, guide_id, deployment_id, update_id):
        """Read the installed owner's envelope, without initial acquisition admission.

        The supplier performs read-only installed-selection currentness checks.
        The original locked action admits source, predecessor, native inputs and
        retained attempts. This read cannot prepare inputs or choose Fresh replay.
        """
        if not self.applies(guide_id):
            return None
        # Helperless Connect supports only Fresh and Restore here. A
        # configured replacement supplier does not extend that contract.
        if "helper_identity" not in self._inventory:
            from .connect_installer_caller import HELPERLESS_UPDATE_UNAVAILABLE
            raise CustodyError(HELPERLESS_UPDATE_UNAVAILABLE)
        supplier = self._original_replacement_selection
        if self._replacement_selection is not supplier:
            raise CustodyError("The original installed replacement selection owner changed.")
        if supplier is None:
            raise CustodyError("The installed owner has not retained its selected replacement inputs.")
        if any(not isinstance(value, str) or not value or value.strip() != value
                for value in (deployment_id, update_id)):
            raise CustodyError("Replacement selection needs its exact installation and update identities.")
        selected = supplier(guide_id, deployment_id, update_id)
        if inspect.iscoroutine(selected):
            selected.close()
        if self._replacement_selection is not supplier:
            raise CustodyError("The original installed replacement selection owner changed during selection.")
        if (type(selected) is not ExactFile or not isinstance(selected.path, Path)
                or not isinstance(selected.sha256, str)):
            raise CustodyError("The installed owner must retain its exact private replacement envelope.")
        _private_parent(selected.path)
        original = selected.read()
        value = _json(original)
        from .deployment_application_startup import SELECTION_KEYS
        if (not isinstance(value, dict) or set(value) != SELECTION_KEYS
                or value["kind"] != "replacement"
                or value["expected_revision"] is not None and
                    (type(value["expected_revision"]) is not int or value["expected_revision"] < 0)):
            raise CustodyError("Replacement selection needs its existing exact envelope and native revision decision.")
        _private_parent(selected.path)
        if selected.read() != original:
            raise CustodyError("The retained replacement envelope changed before admission.")
        if self._replacement_selection is not supplier:
            raise CustodyError("The original installed replacement selection owner changed during validation.")
        return value

    def retain_installation_inputs(self, deployment, guide_id, deployment_id):
        """Retain private references after the original successful Install exit.

        This synchronous completion hook is never called by a setup GET. The
        trusted callback selects existing files and declarations for the one
        inactive Supervisor; it does not inventory or execute native setup.
        The original read-only supplier remains the input selection owner.
        """
        if not self.applies(guide_id) or self._retain_preparation_inputs is None:
            return
        if self._accepted_inventory_path is not None and self._accepted_inventory_pin is None:
            raise CustodyError("Install must bind its accepted native class source before preparation.")
        if not deployment.lock.locked() or self._installation_supervisor is not None:
            raise CustodyError("Retain the original Install inputs only once under its completion lock.")
        from .deployment import product_runtime_source
        from .deployment_application_bootstrap import require_bootstrap_history_settled
        from .deployment_updates import checked_package, require_replacement_history_settled
        from .deployment_worker import Supervisor
        from .templates.storage_setup import project_lock
        self.require_current()
        snapshot = deployment._installed_configuration(guide_id, deployment_id,
            **({"source_custody": self._product_source_custody}
                if self._product_source_custody is not None else {}))
        directory, receipt, destination, *_ = snapshot
        _, _, stopped = deployment._stopped_application_owner(guide_id, deployment_id)
        if (receipt.get("phase") != "configure" or not receipt.get("installed_candidate_runtime_proof")
                or parse_candidate_selection(receipt.get("runtime_candidate")) != self._target):
            raise CustodyError("Input retention needs the original successfully installed product.")
        if self._product_source_custody is None:
            context = project_lock(destination)
        else:
            from .product_runtime_custody import require_product_source, require_product_installation
            require_product_installation(self._product_source_custody, receipt)
            source = require_product_source(self._product_source_custody, self._target, destination=destination)
            context = nullcontext(source._lease)
        with context as lease:
            require_bootstrap_history_settled(directory)
            require_replacement_history_settled(directory)
            _, package = checked_package(directory, "package.zip", receipt["package_sha256"])
            artifacts = product_runtime_source(package, receipt.get("runtime_candidate"),
                **({"source_custody": self._product_source_custody}
                    if self._product_source_custody is not None else {}))
            if ((self._product_source_custody is None and artifacts.selection != self._target)
                    or not _same_json_value(_json(package["blueprint.json"][0]), self._blueprint)
                    or package["config/mob.toml"][0] != self._definition.encode("utf-8")
                    or any(pin.read() != package[name][0] for name, pin in self._protected)):
                raise CustodyError("Install completion differs from the original selected host source.")
            gateway = ProductGatewayFile.observe(ExactFile(
                destination / ".bin" / artifacts.prefix / "bin/toolkit_gateway",
                artifacts.candidate["binary"]["sha256"]), artifacts)
            stopped()
            lease.require_current()
            self.require_current()
            supervisor = Supervisor(directory, **({"source_custody": self._product_source_custody}
                if self._product_source_custody is not None else {}))
            # Even a refused trusted callback cannot authorize a replacement
            # Supervisor on retry. Preserve the one original inactive handle.
            self._installation_supervisor = supervisor
            chosen = self._retain_preparation_inputs(guide_id, deployment_id, supervisor)
            if inspect.iscoroutine(chosen):
                chosen.close()
            if type(chosen) is not ApplicationPreparationInputs or chosen.supervisor is not supervisor:
                raise CustodyError("The original supplier must retain these exact typed Install inputs.")
            self._retained_preparation_inputs = chosen
            self._retained_fresh_paths = deepcopy(chosen.fresh_paths)
            supplied = self._preparation_inputs(guide_id, deployment_id)
            if inspect.iscoroutine(supplied):
                supplied.close()
            if supplied is not chosen:
                raise CustodyError("The original supplier must retain these exact typed Install inputs.")
            current = self._selected_preparation(deployment, guide_id, deployment_id)
            self._require_preparation_outputs(current)
            gateway.require_current()
            stopped()
            lease.require_current()
            self.require_current()

    def _selected_preparation(self, deployment, guide_id, deployment_id, *, restore_inputs=None):
        """Read the original host's private inputs, without entering inventory."""
        from .deployment_worker import Supervisor
        if not self.applies(guide_id) or self._preparation_inputs is None:
            raise CustodyError("The original host has no preparation inputs for this installation.")
        if self._installation_supervisor is not None and self._retained_preparation_inputs is None:
            raise CustodyError("The original Install retainer has not supplied its exact typed inputs.")
        self.require_current()
        chosen = self._preparation_inputs(guide_id, deployment_id)
        if inspect.iscoroutine(chosen):
            chosen.close()
        if type(chosen) is not ApplicationPreparationInputs:
            raise CustodyError("The original host must retain exactly its private preparation inputs.")
        if self._retained_preparation_inputs is not None and (
                chosen is not self._retained_preparation_inputs or chosen.fresh_paths != self._retained_fresh_paths):
            raise CustodyError("The original supplier changed its retained Install inputs or Fresh paths.")
        if restore_inputs is not None and (restore_inputs is not chosen
                or restore_inputs is not self._retained_preparation_inputs
                or chosen.supervisor is not self._installation_supervisor):
            raise CustodyError("Restore needs this host's exact retained Install inputs and Supervisor.")
        model_configuration = chosen._require_model_configuration()
        if model_configuration is not None and sys.platform != "linux":
            raise CustodyError("Managed preparation requires its original Linux credential-root selection.")
        from .connect_application_startup import _managed_model_classes
        if (_managed_model_classes({"classes": self._blueprint["agent_classes"]})
                and model_configuration is None):
            raise CustodyError("Managed-host models require the original trusted preparation model configuration.")
        chosen = {"supervisor": chosen.supervisor, "declarations": chosen.declarations,
            "inventory_attempt_path": chosen.inventory_attempt_path,
            "preparation_plan_path": chosen.preparation_plan_path,
            "startup_selection_path": chosen.startup_selection_path,
            "activation_template": chosen.activation_template,
            "fresh_paths": deepcopy(chosen.fresh_paths), "command_timeout": chosen.command_timeout}
        if model_configuration is not None:
            chosen["model_configuration"] = model_configuration
        directory, receipt, stopped = deployment._stopped_application_owner(guide_id, deployment_id)
        supervisor = chosen["supervisor"]
        changed = "Preparation inputs differ from the original stopped product owner."
        from .deployment_application_bootstrap import same_installation_apart_from_staged_update
        # An update staged or blocked after Install changed nothing installed;
        # its pointer alone does not change the original stopped owner.
        if (not isinstance(supervisor, Supervisor) or supervisor.directory != directory
                or supervisor.destination != Path(receipt["destination"])
                or restore_inputs is None and not same_installation_apart_from_staged_update(
                    deployment._configuration_binding(supervisor.receipt),
                    deployment._configuration_binding(receipt), receipt)
                or parse_candidate_selection(receipt.get("runtime_candidate")) != self._target
                or deployment_id in deployment._application_preparations):
            raise CustodyError(changed)
        if restore_inputs is not None:
            # Start creates a new worker lifecycle. The stopped owner above
            # verifies its current cleanup; Restore retains the same installed
            # source, or its own completed updates, rather than the original
            # Install worker's lifecycle.
            from .deployment_application_bootstrap import bootstrap_installation_binding, require_owner_installation
            require_owner_installation(directory, receipt, bootstrap_installation_binding(supervisor.receipt), changed)
        if (supervisor.child is not None or getattr(supervisor, "_child_admission", None) is not None
                or getattr(supervisor, "_selected_sdk_install_task", None) is not None or supervisor.stop.is_set()):
            raise CustodyError("The original preparation Supervisor has unresolved child custody.")
        for name in ("inventory_attempt_path", "preparation_plan_path", "startup_selection_path"):
            path = _path(chosen[name])
            if supervisor.destination not in path.parents:
                raise CustodyError("Preparation must retain its original private installation paths.")
            _private_parent(path)
            chosen[name] = path
        template = chosen["activation_template"]
        if not isinstance(template, ExactFile):
            raise CustodyError("Preparation needs its original exact private activation template.")
        _private_parent(template.path)
        template.read()
        stopped()
        self.require_current()
        return chosen

    def _require_preparation_outputs(self, chosen):
        """Use the existing inert path checks before offering preparation."""
        from .deployment_application_bootstrap import _fresh_paths
        from .deployment_application_preparation import _synchronous
        _synchronous(chosen["declarations"])
        timeout = chosen["command_timeout"]
        if (isinstance(timeout, bool) or not isinstance(timeout, (int, float))
                or not math.isfinite(timeout) or timeout <= 0):
            raise CustodyError("Preparation needs its original finite positive command deadline.")
        if not isinstance(chosen["fresh_paths"], dict):
            raise CustodyError("Preparation needs its original private Fresh paths.")
        paths = _fresh_paths(chosen["fresh_paths"], chosen["supervisor"].destination)
        outputs = [_path(chosen[name], fresh=True) for name in
            ("inventory_attempt_path", "preparation_plan_path", "startup_selection_path")]
        _path(Path(str(outputs[0]) + ".outcome.json"), fresh=True)
        if len(set([*outputs, *paths.values(), chosen["activation_template"].path])) != len(outputs) + len(paths) + 1:
            raise CustodyError("Preparation must preserve separate original private inputs and outputs.")

    async def prepare_application(self, deployment, guide_id, deployment_id):
        """Use the existing locked action owner to retain its original Fresh draft."""
        if not deployment.lock.locked():
            raise CustodyError("Application preparation requires the original action lock.")
        chosen = self._selected_preparation(deployment, guide_id, deployment_id)
        if chosen["startup_selection_path"].exists():
            self._selected_startup(guide_id, deployment_id)
            raise CustodyError("The original setup envelope is already retained. Finish setup without preparing again.")
        try:
            original = self._startup_selection(guide_id, deployment_id)
        except FileNotFoundError:
            original = None
        if original is not None:
            if not isinstance(original, ExactFile):
                raise CustodyError("The original startup supplier changed its selected input type.")
            if original.path != chosen["startup_selection_path"]:
                self._selected_startup(guide_id, deployment_id)
                raise CustodyError("The original setup envelope is already retained. Finish setup without preparing again.")
        self._require_preparation_outputs(chosen)
        from .deployment_application_preparation import prepare_application
        result = await prepare_application(deployment, guide_id, deployment_id,
            application_host=self, **chosen)
        if self._selected_preparation(deployment, guide_id, deployment_id) != chosen:
            raise CustodyError("The original preparation inputs changed before setup readiness.")
        selected, value = self._selected_startup_observation(guide_id, deployment_id)
        if selected != result or value["kind"] != "fresh":
            raise CustodyError("The original startup supplier did not retain this exact prepared Fresh envelope.")
        if self._selected_preparation(deployment, guide_id, deployment_id) != chosen:
            raise CustodyError("The original preparation inputs changed during startup selection.")
        self.require_current()
        return result

    async def setup_status(self, deployment, guide_id, deployment_id):
        """Project selected inputs and saved settings, never native readiness."""
        return await self._setup_status(deployment, guide_id, deployment_id)

    async def _setup_status_locked(self, deployment, guide_id, deployment_id, *, source_custody):
        """Borrow only this host's entered original source and deployment lock."""
        from .deployment import DeploymentService
        from .remote_product_destination_owner import StoppedProductSource
        if (not isinstance(deployment, DeploymentService) or not deployment.lock.locked()
                or type(source_custody) is not StoppedProductSource
                or source_custody is not self._product_source_custody):
            raise CustodyError("Installed setup readback requires its actual locked source owner.")
        source_custody.require_owner_current()
        return await self._setup_status(deployment, guide_id, deployment_id,
            source_custody=source_custody)

    async def _setup_status(self, deployment, guide_id, deployment_id, *, source_custody=None):
        async def read_settings():
            if source_custody is None:
                return await deployment.settings_options(guide_id, deployment_id)
            source_custody.require_owner_current()
            result = await deployment._settings_options_locked(guide_id, deployment_id,
                project_lease=source_custody._lease, source_custody=source_custody)
            source_custody.require_owner_current()
            return result

        binding = {"guide_id": guide_id, "deployment_id": deployment_id}
        try:
            settings = await read_settings()
            binding = application_setup_binding(settings, guide_id, deployment_id)
            if not self.applies(guide_id):
                return {**binding, "available": False, "reason": "host_unavailable",
                    "detail": "Application setup has no trusted host selection for this guide."}
            if self._startup_selection is None:
                return {**binding, "available": False, "reason": "host_unavailable",
                    "detail": "The original host has not retained its selected Fresh or Restore inputs."}
            from .deployment_application_bootstrap import FRESH_ENTRY_NAME
            entry = deployment._directory(guide_id, deployment_id) / FRESH_ENTRY_NAME
            self.require_current()
            try:
                selected = self._startup_selection(guide_id, deployment_id)
            except FileNotFoundError:
                selected = None
            # Retained Fresh/Restore inputs belong to the original startup
            # supplier. Preparation-only inputs cannot invalidate that owner.
            prepare = selected is None and self._preparation_inputs is not None
            if prepare:
                if entry.exists() or entry.is_symlink():
                    raise CustodyError("The original setup attempt needs its retained startup selection.")
                chosen = self._selected_preparation(deployment, guide_id, deployment_id)
                self._require_preparation_outputs(chosen)
                value = {"kind": "fresh"}
            else:
                selected, value = self._selected_startup_observation(
                    guide_id, deployment_id, selected=selected)
            if value["kind"] == "fresh" and (entry.exists() or entry.is_symlink()):
                return {**binding, "available": False, "reason": "previous_attempt",
                    "detail": "The original setup attempt needs its completed Restore selection before starting again."}
            missing = [field["label"] for field in settings["fields"]
                if field.get("required") and not field.get("configured") and not field.get("automatic")]
            self.require_current()
            if prepare:
                current = self._selected_preparation(deployment, guide_id, deployment_id)
                if current != chosen or current["startup_selection_path"].exists():
                    raise CustodyError("The original preparation inputs changed while reading availability.")
            elif self._selected_startup(guide_id, deployment_id) != value:
                raise CustodyError("The retained application setup selection changed while reading availability.")
            if missing:
                return {**binding, "available": False, "reason": "private_settings_missing",
                    "detail": "Save the required private settings before finishing application setup.",
                    "missing_fields": missing}
            background = None
            if value["kind"] == "restore" and callable(getattr(deployment, "restored_service_capability", None)):
                selected_pin, selected_value = selected, deepcopy(value)
                names = (("application_setup_selection", "_selected_startup"),)
                callbacks = self._deployment_callbacks(deployment, guide_id, deployment_id, names)

                def current_restore():
                    if self._deployment_callbacks(deployment, guide_id, deployment_id, names) != callbacks:
                        raise CustodyError("The original application startup owner changed.")
                    pin, current = self._selected_startup_observation(guide_id, deployment_id)
                    if pin != selected_pin or not _same_json_value(current, selected_value):
                        raise CustodyError("The original service Restore selection changed.")

                if source_custody is None:
                    background = await deployment.restored_service_capability(guide_id, deployment_id,
                        application_startup=selected_value, require_owner_current=current_restore)
                else:
                    background = await deployment._restored_service_capability_locked(guide_id, deployment_id,
                        application_startup=selected_value, require_owner_current=current_restore,
                        source_custody=source_custody)
                # Preserve the original setup/source/private read across the
                # manager await, then fence the same retained supplier last.
                current_settings = await read_settings()
                if (application_setup_binding(current_settings, guide_id, deployment_id) != binding
                        or current_settings["fields"] != settings["fields"]):
                    raise CustodyError("The original installed setup changed while reading background availability.")
                # The retained supplier may call application-owned state;
                # keep that callback on this original task after the file read.
                current_restore()
            return {**binding, "available": True, **({"action": "prepare-setup"} if prepare else {}),
                **({"background": background} if background is not None else {}),
                "kind": value["kind"], "label": "Prepare application setup" if prepare else
                    "Finish application setup" if value["kind"] == "fresh" else "Start installed application",
                "scope": "selected-stopped-application-inputs",
                "detail": "Preparation retains the original private setup draft and keeps the application stopped." if prepare
                    else "Starting rechecks the original installation and retained setup inputs."}
        except OSError:
            return {**binding, "available": False, "reason": "source_unavailable",
                "detail": "The original private setup inputs are unavailable. Preserve this installation and its saved inputs."}
        except (CustodyError, DraftConflict, LookupError, ValueError) as error:
            return {**binding, "available": False, "reason": "source_unavailable", "detail": str(error)}

    async def start_application(self, deployment, guide_id, deployment_id):
        """Dispatch retained owner inputs, without browser paths or Fresh replay."""
        value = self._selected_startup(guide_id, deployment_id)
        return await deployment.start_application(guide_id, deployment_id,
            application_startup=deepcopy(value),
            **({"owner_source": self._product_source_custody._owner_source}
                if self._product_source_custody is not None else {}))

    async def start_service(self, deployment, guide_id, deployment_id):
        """Explicit trusted-owner handoff, using only its retained Restore."""
        pin, value = self._selected_startup_observation(guide_id, deployment_id)
        if value["kind"] != "restore":
            raise CustodyError("Background startup requires completed Restore inputs; Fresh is never replayed.")

        def require_current():
            current_pin, current = self._selected_startup_observation(guide_id, deployment_id)
            if current_pin != pin or not _same_json_value(current, value):
                raise CustodyError("The original owner changed its service Restore selection.")

        return await deployment.start_restored_service(guide_id, deployment_id,
            application_startup=deepcopy(value), require_owner_current=require_current,
            **({"owner_source": self._product_source_custody._owner_source}
                if self._product_source_custody is not None else {}))
