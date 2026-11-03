"""Installation-owned local application defaults for the existing setup action.

Only the successful Install completion retainer creates private source inputs.
GET providers read the same retained Supervisor and files. The original native
inventory supplies actual realm/member bindings; native preparation resolves
controller keys and issues all requested authority. These declarations do not
authenticate principals, contain lineages or claim a completed setup.
"""
from __future__ import annotations

from copy import deepcopy
import hashlib
import json
from pathlib import Path
from uuid import UUID

from .connect_installer_caller import (CustodyError, ExactFile, NativeApplicationInventory,
    _json, _path, _private_directory, _same_json_value, _preparation_helper_catalog_request, _selected_console_domains)
from .forge_application_host import ApplicationPreparationInputs, ForgeApplicationHost
from .runtime_selection import parse_candidate_selection


# Match InstalledSetupLimits' one-hour budget for discovery, acquisition,
# inspection and protected review. Admission keeps its separate short timeout.
_LOCAL_SETUP_TIMEOUT_MS = 3_600_000
_LEGACY_LOCAL_SETUP_TIMEOUT_MS = 180_000


def _encode(value):
    return (json.dumps(value, ensure_ascii=True, allow_nan=False, sort_keys=True,
        separators=(",", ":")) + "\n").encode()


def _new_source(path, value):
    """Publish new declarations once, without repairing an existing attempt."""
    from .connect_application_finalizer import _write_bytes
    return _write_bytes(path, _encode(value))


def _principal(kind, identity, domain):
    # Public qualified PrincipalRef syntax. The application owns these logical
    # declarations; construction provides neither authentication nor a grant.
    return {"kind": kind, "id": identity,
        "qualification": {"kind": "qualified", "trust_domain_id": domain}}


class LocalApplicationPreparationOwner:
    """One original local installation's concrete finite preparation selection.

    These initial defaults offer Console self-use, acquisition for the explicit
    non-helper application roster, and a dedicated helper's public-service and
    reporting workflow. New account request entry follows the exact helper's
    retained source invocation choice; recovery keeps its original plan choice.
    Standing policies, provider channels and
    service-parent administration stay unselected. The original native driver
    must authenticate and issue the requested parents independently.
    """

    def __init__(self, factory):
        self.factory, self.host = factory, None
        self._inputs = self._choices_pin = self._choices_bytes = None
        self._guide_id = self._deployment_id = self._installation = None
        self._fresh_pin = self._fresh_value = None
        self._restore_pin = self._restore_value = None
        self._directories = self._directory = self._fresh_paths = None
        self._entered = False

    def bind_host(self, host):
        if type(host) is not ForgeApplicationHost or self.host is not None:
            raise CustodyError("Retain the local preparation owner's one original host.")
        self.host = host

    def _source_options(self):
        source = self.host._product_source_custody
        return {"source_custody": source} if source is not None else {}

    def _recovery_admission(self, admission):
        from .local_application_recovery import _InstalledLocalAdmission
        if type(admission) is _InstalledLocalAdmission:
            pass
        else:
            from .destination_application_inputs import DestinationApplicationInputs
            if type(admission) is not DestinationApplicationInputs or self.factory is not admission:
                raise CustodyError("Recover only the original admitted installation inputs.")
            admission._require_recovery_current()
        return admission

    @staticmethod
    def _binding(receipt):
        from .deployment_application_bootstrap import bootstrap_installation_binding
        return bootstrap_installation_binding(receipt)

    def _require(self, guide_id, deployment_id):
        if (self.host is None or guide_id != self._guide_id or deployment_id != self._deployment_id
                or self._inputs is None):
            raise CustodyError("The original successful Install has not retained local setup inputs.")
        from .deployment_worker import read_receipt
        from .deployment_application_bootstrap import require_owner_installation
        self.host.require_current()
        receipt = read_receipt(self._inputs.supervisor.directory)
        changed = "The original installation or its selected local preparation policy changed."
        if (parse_candidate_selection(receipt.get("runtime_candidate")) != self.factory.runtime_target
                or self._inputs.supervisor.directory != self._directory
                or self._inputs.supervisor.destination != Path(receipt["destination"])
                or self._inputs.fresh_paths != self._fresh_paths
                or any(_private_directory(path) != observed for path, observed in self._directories)
                or self._choices_pin.read() != self._choices_bytes):
            raise CustodyError(changed)
        require_owner_installation(self._directory, receipt, self._installation, changed)
        if not _same_json_value(self._inputs._require_model_configuration(),
                self.factory._selected_model_configuration()):
            raise CustodyError("The retained preparation changed its original model configuration.")
        self._inputs.activation_template.read()
        from .deployment_ports import installed_console_port
        selected_port = installed_console_port(self._inputs.supervisor.destination, self.host._blueprint["advanced"]["port"])
        if _json(self._choices_bytes)["console_setup"]["origin"] != "http://127.0.0.1:" + str(selected_port):
            raise CustodyError("The selected local Console origin differs from the installation's current private port.")
        return receipt

    def _initial_choices(self, guide_id, deployment_id, console, console_pin):
        """The original concrete local source defaults, without publication."""
        helper_identity = self.host._inventory.get("helper_identity")
        setup_identities = [row["identity"] for row in self.host._inventory["roster"]
            if row["identity"] != helper_identity]
        if helper_identity is not None and "member_classes" in self.host._inventory:
            # This newly supported composition selects only explicit seed
            # callers. Preserve the original fixed-only defaults on Restore.
            callers = {row["member_identity"] for row in _json(self.host._policy_bytes)["members"]
                if any(grant["tool_name"] == "find_and_install" and grant["action"] == "invoke"
                    for grant in row["grants"])}
            setup_identities = [identity for identity in setup_identities if identity in callers]
        choices = {"deployment_id": deployment_id, "guide_id": guide_id,
            "console_operator": self.host.console_operator_declaration(),
            "principal_domain": "meta-forge-installation:" + deployment_id,
            "initial_setup_identities": setup_identities,
            "console_setup": console, "console_setup_source": None if console_pin is None else console_pin.pin(),
            "namespace": deployment_id, "generation": 1,
            "backend_generation": "forge-workgraph:" + deployment_id,
            "output_audience": "forge-console:" + deployment_id,
            "limits": {"setup_timeout_ms": _LOCAL_SETUP_TIMEOUT_MS, "max_workers": 4,
                "max_effect_attempts": 4, "admission_timeout_ms": 10000}}
        if helper_identity is None:
            for name in ("initial_setup_identities", "backend_generation", "output_audience", "limits"):
                del choices[name]
        model = self.factory._selected_model_configuration()
        if model is not None:
            choices["model_configuration"] = model
        return choices

    def retain(self, guide_id, deployment_id, supervisor):
        """Called only by the real locked successful-Install completion owner."""
        from .deployment_worker import Supervisor, read_receipt
        if (self.host is None or guide_id != self.factory.guide_id
                or type(supervisor) is not Supervisor or self._entered
                or self.host._installation_supervisor is not supervisor):
            raise CustodyError("Retain the original local Install Supervisor once through its actual host.")
        self.host.require_current()
        receipt = read_receipt(supervisor.directory)
        if (receipt.get("guide_id") != guide_id or receipt.get("deployment_id") != deployment_id
                or str(UUID(deployment_id)) != deployment_id
                or receipt.get("status") != "stopped" or receipt.get("phase") != "configure"
                or not receipt.get("installed_candidate_runtime_proof")
                or parse_candidate_selection(receipt.get("runtime_candidate")) != self.factory.runtime_target
                or supervisor.destination != Path(receipt["destination"])
                or supervisor.child is not None or supervisor.stop.is_set()):
            raise CustodyError("Local defaults require the actual successfully installed stopped product.")
        root = _path(supervisor.destination)
        _private_directory(root)
        _private_directory(root / ".mobkit")
        from .deployment_ports import installed_console_port
        selected_port = installed_console_port(root, self.host._blueprint["advanced"]["port"])
        console_pin = self.factory.console_setup_selection
        console = ({"origin": "http://127.0.0.1:" + str(selected_port),
            "access_management": "requester_issued_leaves"} if console_pin is None
            else _json(self.factory._read(console_pin)))
        self.factory._console_choice(console)
        if console["origin"] != "http://127.0.0.1:" + str(selected_port):
            raise CustodyError("The protected Console source differs from the actual installed Console port.")
        choices = self._initial_choices(guide_id, deployment_id, console, console_pin)
        self.host.require_current()
        self._entered = True
        home = _path(root / ".mobkit" / "forge-local-setup", fresh=True)
        home.mkdir(mode=0o700)
        from .connect_application_finalizer import _sync_directory
        _sync_directory(home.parent)
        _private_directory(home)
        work = home / "finalization"
        work.mkdir(mode=0o700)
        _sync_directory(home)
        _private_directory(work)
        template = _new_source(home / "activation-template.json", {"activation_id": deployment_id})
        choice_pin = _new_source(home / "local-application-policy.json", choices)
        fresh = {"provision_attempt_path": home / "provision.json", "working_directory": work,
            "bundle_path": root / ("forge-bundle-" + deployment_id),
            "activation_path": root / ("forge-activation-" + deployment_id + ".json")}
        inputs = ApplicationPreparationInputs(supervisor=supervisor,
            declarations=self.declarations, inventory_attempt_path=home / "inventory.json",
            preparation_plan_path=home / "preparation-plan.json",
            startup_selection_path=home / "startup-selection.json", activation_template=template,
            fresh_paths=fresh, command_timeout=60.0,
            model_configuration=self.factory._selected_model_configuration())
        self._choices_pin, self._choices_bytes = choice_pin, choice_pin.read()
        self._installation = self._binding(receipt)
        self._directory, self._fresh_paths = supervisor.directory, deepcopy(fresh)
        self._directories = tuple((path, _private_directory(path)) for path in (root, root / ".mobkit", home, work))
        self._guide_id, self._deployment_id, self._inputs = guide_id, deployment_id, inputs
        self._require(guide_id, deployment_id)
        return inputs

    def _recovery_inputs(self, admission, receipt):
        """Authenticate existing private declarations before owner construction.

        The accepted original source admission selects this installation. Files
        must equal its concrete local policy; their presence never selects it.
        """
        from .local_application_recovery import _pin
        self._recovery_admission(admission)
        if (self.host is None
                or self.factory._installed_admission is not admission or self._entered
                or receipt["guide_id"] != admission.guide_id
                or self._binding(receipt) != admission.installation):
            raise CustodyError("Recover only the original admitted local installation inputs.")
        root = _path(receipt["destination"])
        home, work = root / ".mobkit/forge-local-setup", root / ".mobkit/forge-local-setup/finalization"
        directories = tuple((path, _private_directory(path)) for path in (root, root / ".mobkit", home, work))
        from .deployment_ports import installed_console_port
        origin = "http://127.0.0.1:" + str(installed_console_port(root,
            self.host._blueprint["advanced"]["port"]))
        console_pin = self.factory.console_setup_selection
        console = ({"origin": origin, "access_management": "requester_issued_leaves"}
            if console_pin is None else _json(self.factory._read(console_pin)))
        self.factory._console_choice(console)
        if console["origin"] != origin:
            raise CustodyError("The retained Console source differs from the original installed port.")
        choices = self._initial_choices(admission.guide_id, admission.deployment_id, console, console_pin)
        choice_pin, template = _pin(home / "local-application-policy.json"), _pin(home / "activation-template.json")
        retained = _json(choice_pin.read())
        if "limits" in choices:
            limits = retained.get("limits") if isinstance(retained, dict) else None
            timeout_ms = limits.get("setup_timeout_ms") if isinstance(limits, dict) else None
            if (type(timeout_ms) is not int
                    or timeout_ms not in {_LEGACY_LOCAL_SETUP_TIMEOUT_MS, _LOCAL_SETUP_TIMEOUT_MS}):
                raise CustodyError("The original local setup policy or activation source changed.")
            # Recognize the exact older default without upgrading its retained
            # choice, prepared plan or any already admitted operation's deadline.
            choices["limits"]["setup_timeout_ms"] = timeout_ms
        if (choice_pin.read() != _encode(choices)
                or template.read() != _encode({"activation_id": admission.deployment_id})):
            raise CustodyError("The original local setup policy or activation source changed.")
        fresh = {"provision_attempt_path": home / "provision.json", "working_directory": work,
            "bundle_path": root / ("forge-bundle-" + admission.deployment_id),
            "activation_path": root / ("forge-activation-" + admission.deployment_id + ".json")}
        startup = home / "startup-selection.json"
        if not startup.exists() and not startup.is_symlink():
            from .deployment_application_bootstrap import FRESH_ENTRY_NAME, _fresh_paths
            _fresh_paths(fresh, root)
            for path in (home / "inventory.json", home / "inventory.json.outcome.json",
                    home / "preparation-plan.json", home / "prepared-application",
                    home / "restore-selection.json", admission.directory / FRESH_ENTRY_NAME):
                _path(path, fresh=True)
        return choice_pin, template, fresh, directories

    def _recover(self, admission, supervisor, retained):
        """Retain one new inactive handle for the authenticated original input.

        This does not call the successful-Install retainer or create defaults.
        The caller owns the stopped installation and actual project lease.
        """
        from .deployment_worker import Supervisor
        self._recovery_admission(admission)
        if (type(supervisor) is not Supervisor
                or self.factory._installed_admission is not admission or self._entered
                or supervisor.directory != admission.directory or supervisor.child is not None
                or supervisor.stop.is_set()):
            raise CustodyError("Recover the exact admitted stopped preparation owner once.")
        choice_pin, template, fresh, directories = retained
        if (any(_private_directory(path) != identity for path, identity in directories)
                or retained != self._recovery_inputs(admission, supervisor.receipt)):
            raise CustodyError("The original private setup inputs changed before recovery retention.")
        home = choice_pin.path.parent
        inputs = ApplicationPreparationInputs(supervisor=supervisor, declarations=self.declarations,
            inventory_attempt_path=home / "inventory.json", preparation_plan_path=home / "preparation-plan.json",
            startup_selection_path=home / "startup-selection.json", activation_template=template,
            fresh_paths=fresh, command_timeout=60.0,
            model_configuration=self.factory._selected_model_configuration())
        self._choices_pin, self._choices_bytes = choice_pin, choice_pin.read()
        self._installation, self._directory = deepcopy(admission.installation), admission.directory
        self._fresh_paths, self._directories = deepcopy(fresh), directories
        self._guide_id, self._deployment_id, self._inputs = admission.guide_id, admission.deployment_id, inputs
        self._entered = True
        return inputs

    def _recover_startup(self, lease):
        """Read original Fresh/Restore closure in a joined filesystem lane.

        No original owner callback or native operation runs in this lane. The
        action brackets it with the same stopped owner and held project lease.
        """
        from .local_application_recovery import _pin
        from .deployment_application_bootstrap import (FRESH_ENTRY_NAME, _artifact_pin,
            _observe_inputs, _shape, read_original_fresh_admission, require_restore_history_current)
        from .deployment_worker import read_receipt
        directory, inputs = self._directory, self._inputs
        path, entry = inputs.startup_selection_path, directory / FRESH_ENTRY_NAME
        restore_path = path.parent / "restore-selection.json"
        lease.require_current()
        entered = entry.exists() or entry.is_symlink()
        if not path.exists() and not path.is_symlink():
            self.host._require_preparation_outputs({"supervisor": inputs.supervisor,
                "declarations": inputs.declarations, "command_timeout": inputs.command_timeout,
                "fresh_paths": inputs.fresh_paths, "activation_template": inputs.activation_template,
                "inventory_attempt_path": inputs.inventory_attempt_path,
                "preparation_plan_path": inputs.preparation_plan_path,
                "startup_selection_path": path})
            if entered or restore_path.exists() or restore_path.is_symlink():
                raise CustodyError("Original bootstrap history is present without its selected Fresh source.")
            lease.require_current()
            return None
        pin, receipt = _pin(path), read_receipt(directory)
        fresh = _json(pin.read())
        if (_shape(fresh) != "fresh"
                or _artifact_pin(fresh["preparation_plan"]).path != inputs.preparation_plan_path
                or fresh["activation_template"] != inputs.activation_template.pin()
                or not _same_json_value(fresh.get("model_configuration"), inputs._require_model_configuration())
                or any(fresh[name] != str(value) for name, value in inputs.fresh_paths.items())
                or _artifact_pin(fresh["source_policy"]).read() != self.host._policy_bytes):
            raise CustodyError("The retained Fresh envelope differs from the original local input owner.")
        observed = _observe_inputs(directory, receipt, fresh, entered=entered, **self._source_options())
        entry_pin = None
        if entered:
            entry_pin = _pin(entry)
            record = _json(entry_pin.read())
            if (not isinstance(record, dict) or set(record) != {"kind", "admission", "provision_attempt_path"}
                    or record["kind"] != "application-bootstrap-entry"
                    or record["provision_attempt_path"] != fresh["provision_attempt_path"]
                    or read_original_fresh_admission(directory, record["admission"])["application_startup"] != fresh):
                raise CustodyError("The entered original Fresh lineage changed during recovery.")
        observed.require_current()
        from .connect_installer_caller import NativeInstallerCaller, ProductGatewayFile
        if type(observed.gateway) is not ProductGatewayFile:
            raise CustodyError("Recovery needs the original selected product inventory gateway.")
        attempt = _pin(inputs.inventory_attempt_path)
        completion = _pin(Path(str(inputs.inventory_attempt_path) + ".outcome.json"))
        record = _json(attempt.read())
        names = ("config/mob.toml", self.host._policy_source_path, "application/configuration.json",
            "application/console.json", "config/access.toml")
        if (not isinstance(record, dict) or not isinstance(record.get("input_files"), list)
                or len(record["input_files"]) != 2 + len(names)
                or record["input_files"][:2] != [observed.gateway.pin(), self.host._inventory_pin.pin()]):
            raise CustodyError("The retained inventory lost its original gateway, compiler or source projections.")
        sources = tuple(_artifact_pin(value) for value in record["input_files"][2:])
        projection = _path(sources[0].path).parent.parent
        prefix = "application-preparation-source-"
        try:
            suffix = projection.name.removeprefix(prefix)
            original_projection = projection.name.startswith(prefix) and str(UUID(suffix)) == suffix
        except ValueError:
            original_projection = False
        if (projection.parent != directory or not original_projection
                or any(_path(pin.path) != projection / name for name, pin in zip(names, sources))):
            raise CustodyError("Recover only the original accepted-source private inventory projections.")
        projection_identity = _private_directory(projection)
        package = self.factory._installed_admission.package
        selected_bytes = tuple(package[name][0] for name in names)
        protected = dict(self.host._protected)
        restored = []

        def files_current():
            lease.require_current()
            if (_private_directory(projection) != projection_identity
                    or any(pin.read() != content for pin, content in zip(sources, selected_bytes))
                    or any(protected[name].read() != content for name, content in zip(names, selected_bytes)
                        if name in protected)
                    or selected_bytes[0] != self.host._definition.encode()
                    or self._choices_pin.read() != self._choices_bytes
                    or entered != (entry.exists() or entry.is_symlink())):
                raise CustodyError("The retained inventory differs from the original accepted local source.")
            if entry_pin is not None:
                entry_pin.read()
            for restore_pin, completed in restored:
                restore_pin.read()
                completed.require_current()
            observed.require_current()
            pin.read()
            lease.require_current()

        def owner_current():
            self._require(self._guide_id, self._deployment_id)
            lease.require_current()

        async def no_replay(argv):
            raise CustodyError("Recovery cannot repeat the original native inventory or preparation.")

        caller = NativeInstallerCaller(observed.gateway, execute=no_replay,
            require_current=owner_current, require_filesystem_current=files_current,
            provision_attempt_path=inputs.fresh_paths["provision_attempt_path"])
        inventory = NativeApplicationInventory(caller, observed.gateway, self.host._inventory_pin,
            sources, attempt, completion)
        files_current()
        original, _ = inventory._authenticate_files()
        if not _same_json_value(original, self.host._inventory):
            raise CustodyError("The recovered inventory differs from the original compiler roster.")
        self._fresh_pin, self._fresh_value = pin, deepcopy(fresh)
        if restore_path.exists() or restore_path.is_symlink():
            if not entered:
                raise CustodyError("Restore cannot replace an unentered original local Fresh selection.")
            from .local_application_restore import _completed_restore
            value, completed = _completed_restore(self, directory, receipt, fresh, lease,
                self.host._product_source_custody)
            restore_pin = _pin(restore_path)
            if restore_pin.read() != _encode(value):
                raise CustodyError("The retained Restore differs from the actual original native finalization.")
            require_restore_history_current(directory, value)
            completed.require_current()
            self._restore_pin, self._restore_value = restore_pin, deepcopy(value)
            restored.append((restore_pin, completed))
        lease.require_current()
        observed.require_current()
        pin.read()
        files_current()
        inventory._authenticate_files()
        return inventory

    def _require_recovered_plan(self, inventory, expected, lease):
        """Complete the original-task translation join in the file-only lane."""
        from .connect_installer_caller import _artifact_pin
        if (type(inventory) is not NativeApplicationInventory
                or inventory.compiler_input != self.host._inventory_pin
                or inventory._caller.provision_attempt_path != self._inputs.fresh_paths["provision_attempt_path"]):
            raise CustodyError("Preserve the authenticated original local preparation inventory.")
        lease.require_current()
        inventory._caller._require_filesystem_current()
        inventory._authenticate_files()
        plan = _artifact_pin(self._fresh_value["preparation_plan"])
        if not _same_json_value(_json(plan.read()), _json(expected)):
            raise CustodyError("The retained preparation plan differs from the original local declarations and inventory.")
        if self._restore_pin is not None:
            self._restore_pin.read()
        self._fresh_pin.read()
        inventory._authenticate_files()
        inventory._caller._require_filesystem_current()
        plan.read()
        lease.require_current()

    def preparation_inputs(self, guide_id, deployment_id):
        self._require(guide_id, deployment_id)
        return self._inputs

    def declarations(self, inventory):
        """Translate original defaults using the genuine inventory observation."""
        self._require(self._guide_id, self._deployment_id)
        if type(inventory) is not NativeApplicationInventory:
            raise CustodyError("Local defaults need the existing native inventory owner.")
        original, actual = inventory.authenticate()
        if (inventory.compiler_input != self.host._inventory_pin
                or not _same_json_value(original, self.host._inventory)):
            raise CustodyError("Local defaults cannot substitute another application roster.")
        choices = _json(self._choices_bytes)
        domain, deployment = choices["principal_domain"], self._deployment_id
        human = choices["console_operator"]
        administrator = _principal("runtime_host", "installation-administrator", domain)
        ingress = _principal("runtime_host", "console-ingress", domain)
        authentication = {"authority": deepcopy(ingress), "namespace": "console-authentication"}
        event = {"authority": deepcopy(ingress), "namespace": "console-events"}
        has_helper = "helper_identity" in original
        members, routes, controllers, setup_requests = [], [], [], []
        if has_helper:
            controllers.append({"target": {"kind": "helper"},
                "issuance_id": "forge:" + deployment + ":helper-controller"})
        helper = None
        for index, row in enumerate(actual["members"]):
            logical = _principal("shared_agent", row["identity"], domain)
            binding = deepcopy(row["member"])
            members.append({"member": binding, "logical_owner": logical,
                "context_generation": 1, "audience": {"audience": "principal", "principal": deepcopy(human)}})
            if has_helper and row["identity"] == original["helper_identity"]:
                helper = {"member": binding, "ingress_actor": deepcopy(ingress),
                    "logical_executor": logical, "represented_subject": deepcopy(human),
                    "controller_parent": [], "operation_basis": {
                        "kind": "targeted_grant_lineages", "base": None, "targets": []}}
            else:
                route = "forge-console-" + str(index)
                routes.append({"id": route, "origin": {"kind": "local_console", "member": binding,
                    "requester": deepcopy(human), "ingress_actor": deepcopy(ingress)},
                    "application_parent": None, "controller_parent": None,
                    "logical_executor": logical, "represented_subject": deepcopy(human),
                    "source_authority": deepcopy(ingress),
                    "authentication_domain": deepcopy(authentication),
                    "event_domain": deepcopy(event)})
                controllers.append({"target": {"kind": "original_route", "route": route},
                    "issuance_id": "forge:" + deployment + ":controller:" + str(index)})
                if has_helper and row["identity"] in choices["initial_setup_identities"]:
                    setup_requests.append({"member": binding, "route": route,
                        "issuance_id": "forge:" + deployment + ":setup:" + str(index)})
        if has_helper and (helper is None or not routes or not setup_requests):
            raise CustodyError("Local setup needs its dedicated helper and explicit Console application route.")
        result = {"deployment_root": self._inputs.supervisor.destination,
            "owner": {"deployment": deployment, "components": [], "channels": [], "native_host": {
                "realm": actual["realm_id"], "grant_administrator": administrator,
                "remote_operator": deepcopy(human),
                "ingress_domain": {"authority": deepcopy(ingress), "namespace": "console-ingress"},
                "authentication_domain": deepcopy(authentication), "event_domain": deepcopy(event),
                "members": members, "routes": routes}},
            "startup": {"kind": "initialize", "namespace": choices["namespace"], "generation": choices["generation"]},
            "authorities": {"controllers": controllers},
            "realm_storage": {"context_root": str(self._inputs.supervisor.destination),
                "state_root": str(self._inputs.supervisor.destination / ".mobkit/state")},
            "output": str(self._inputs.startup_selection_path.parent / "prepared-application"),
            "hold_timeout_ms": 180000, "source_id": "forge-local-installation:" + deployment}
        recovering = self.factory._installed_admission is not None and self._fresh_pin is not None
        if recovering:
            from .connect_installer_caller import _artifact_pin
            self._recovery_admission(self.factory._installed_admission)
            if not _same_json_value(_json(self._fresh_pin.read()), self._fresh_value):
                raise CustodyError("The original prepared Fresh selection changed.")
            retained_plan = _artifact_pin(self._fresh_value["preparation_plan"])
            retained_owner = _json(retained_plan.read()).get("owner")
            retained_native = retained_owner.get("native_host") if isinstance(retained_owner, dict) else None
            if not isinstance(retained_native, dict):
                raise CustodyError("Recovery requires its original native installation selection.")
            if _selected_console_domains(retained_native) is None:
                # Restore reads the original selection; it cannot migrate an
                # accepted plan or supply authority for an ordinary Add.
                for name in ("authentication_domain", "event_domain"):
                    del result["owner"]["native_host"][name]
            retained_plan.read()
            self._fresh_pin.read()
        if not has_helper:
            # The same recovered-plan join checks these deterministic original
            # declarations. There are no optional helper choices to recover.
            inventory.authenticate()
            self._require(self._guide_id, self._deployment_id)
            return result
        audience = choices["output_audience"]
        result["authorities"].update({"agent_setup": setup_requests,
                "helper_report": {"member": deepcopy(helper["member"]),
                    "issuance_id": "forge:" + deployment + ":helper-report",
                    "public_issuance_id": "forge:" + deployment + ":helper-public-service"},
                "helper_mcp": {"issuance_id": "forge:" + deployment + ":helper-mcp"},
                "helper_catalog": {"issuance_id": "forge:" + deployment + ":helper-catalog"}})
        result.update({"helper": helper, "acquisition": {"console_setup": deepcopy(choices["console_setup"]),
                "backend_generation": choices["backend_generation"],
                "proposal_ceiling": {"actions": [], "services": [], "frontends": [],
                    "output_audiences": [audience]}, "output_audience": [audience],
                "limits": deepcopy(choices["limits"])}})
        # Recovery preserves the optional choice from the authenticated original
        # Fresh plan. An older custom policy may allow the account tool without
        # having selected its parent. All other declarations use the same
        # inventory join and retain the full recovered-plan comparison.
        if recovering:
            retained_authorities = _json(retained_plan.read()).get("authorities")
            retained_report = (retained_authorities.get("helper_report")
                if isinstance(retained_authorities, dict) else None)
            if (not isinstance(retained_report, dict)
                    or not _same_json_value(retained_report.get("member"), helper["member"])):
                raise CustodyError("The retained account choice differs from the actual original helper.")
            if "account_issuance_id" in retained_report:
                result["authorities"]["helper_report"]["account_issuance_id"] = deepcopy(
                    retained_report["account_issuance_id"])
            retained_catalog = retained_authorities.get("helper_catalog")
            successors = (retained_catalog is not None
                and _preparation_helper_catalog_request(retained_catalog) == "current_helper_successors")
            if successors:
                # Preserve only an already authenticated explicit selection.
                # Never opt in, renew an ID or migrate accepted bounds here.
                result["authorities"]["helper_catalog"] = deepcopy(retained_catalog)
            retained_requests = retained_authorities.get("agent_setup")
            if not isinstance(retained_requests, list) or len(retained_requests) != len(setup_requests):
                raise CustodyError("Recovery lost its original contributing setup requests.")
            for request in setup_requests:
                matches = [row for row in retained_requests if isinstance(row, dict) and all(
                    _same_json_value(row.get(key), request[key]) for key in
                    (("member", "route") if successors else ("member", "route", "issuance_id")))]
                if len(matches) != 1:
                    raise CustodyError("Recovery changed an original contributing setup route.")
                if successors:
                    # Changed catalog bounds require independent target and
                    # outer IDs selected before native preparation. Keep those
                    # exact original IDs, not today's generated default names.
                    request["issuance_id"] = deepcopy(matches[0]["issuance_id"])
                if "helper_targets" in matches[0]:
                    request["helper_targets"] = deepcopy(matches[0]["helper_targets"])
            retained_plan.read()
            self._fresh_pin.read()
        # A genuinely unprepared source selects the request only from its exact
        # protected helper invocation policy, never another member's choice.
        elif any(row["member_identity"] == original["helper_identity"] and any(
                grant["tool_name"] == "toolkit_request_service_account"
                and grant["action"] == "invoke" for grant in row["grants"])
                for row in _json(self.host._policy_bytes)["members"]):
            result["authorities"]["helper_report"]["account_issuance_id"] = (
                "forge:" + deployment + ":helper-service-account")
        if not recovering:
            # Connect's selected preparation protocol names its MCP server
            # toolkit-setup. Native compilation validates these requests against
            # the actual helper resources and issues the separate parent scopes.
            targets = [(None, "catalog"), ({"kind": "mcp_server", "server": "toolkit-setup"}, "mcp"),
                ({"kind": "registered_tool_bundle_operation", "bundle": "meerkat_toolkit.setup_accounts",
                    "operation": "toolkit_request_public_service"}, "public-service")]
            if "account_issuance_id" in result["authorities"]["helper_report"]:
                targets.append(({"kind": "registered_tool_bundle_operation", "bundle": "meerkat_toolkit.setup_accounts",
                    "operation": "toolkit_request_service_account"}, "service-account"))
            targets.append(({"kind": "registered_tool_bundle_operation", "bundle": "meerkat_toolkit.setup_results",
                "operation": "setup_report"}, "report"))
            for request in setup_requests:
                request["helper_targets"] = [{"target": deepcopy(target),
                    "issuance_id": request["issuance_id"] + ":helper-" + label} for target, label in targets]
        inventory.authenticate()
        self._require(self._guide_id, self._deployment_id)
        return result

    async def retain_restore(self, deployment, guide_id, deployment_id):
        """Publish the completed original setup only through its locked action."""
        if self._inputs is None:
            return None
        self._require(guide_id, deployment_id)
        if not deployment.lock.locked():
            raise CustodyError("Local Restore retention requires its original application action lock.")
        if self._restore_pin is not None:
            return self.startup_selection(guide_id, deployment_id)
        from .local_application_restore import retain_local_restore
        selected = await retain_local_restore(self, deployment, guide_id, deployment_id)
        if selected is not None:
            if type(selected) is not ExactFile:
                raise CustodyError("Local Restore publication must return its actual exact retained envelope.")
            self._restore_pin, self._restore_value = selected, _json(selected.read())
            return self.startup_selection(guide_id, deployment_id)
        return None

    def startup_selection(self, guide_id, deployment_id):
        """Read the original Prepare output or action-retained completed Restore.

        Missing output is not created by a GET. Only the original action
        publisher may retain Restore from the verified finalizer history.
        """
        self._require(guide_id, deployment_id)
        if self._restore_pin is not None:
            from .deployment_application_bootstrap import _observe_inputs, _shape, require_restore_history_current
            value = _json(self._restore_pin.read())
            if _shape(value) != "restore" or not _same_json_value(value, self._restore_value):
                raise CustodyError("The original completed local Restore selection changed.")
            from .deployment_worker import read_receipt
            directory = self._inputs.supervisor.directory
            require_restore_history_current(directory, value)
            observed = _observe_inputs(directory, read_receipt(directory), value, **self._source_options())
            observed.require_current()
            self._require(guide_id, deployment_id)
            self._restore_pin.read()
            return self._restore_pin
        path = self._inputs.startup_selection_path
        if not path.exists() and not path.is_symlink():
            if self._fresh_pin is not None:
                raise CustodyError("The original prepared Fresh selection disappeared.")
            raise FileNotFoundError(path)
        if self._fresh_pin is None:
            from .connect_installer_caller import _read, MAX_INPUT_BYTES
            raw = _read(path, MAX_INPUT_BYTES)
            pin, value = ExactFile(path, hashlib.sha256(raw).hexdigest()), _json(raw)
            from .deployment_application_bootstrap import _artifact_pin, _observe_inputs, _shape
            if (_shape(value) != "fresh"
                    or _artifact_pin(value["preparation_plan"]).path != self._inputs.preparation_plan_path
                    or value["activation_template"] != self._inputs.activation_template.pin()
                    or not _same_json_value(value.get("model_configuration"),
                        self._inputs._require_model_configuration())
                    or any(value[name] != str(selected) for name, selected in self._inputs.fresh_paths.items())
                    or _artifact_pin(value["source_policy"]).read() != self.host._policy_bytes):
                raise CustodyError("The prepared Fresh envelope differs from the original local installation inputs.")
            from .deployment_worker import read_receipt
            observed = _observe_inputs(self._inputs.supervisor.directory,
                read_receipt(self._inputs.supervisor.directory), value, **self._source_options())
            observed.require_current()
            pin.read()
            self._fresh_pin, self._fresh_value = pin, deepcopy(value)
        if not _same_json_value(_json(self._fresh_pin.read()), self._fresh_value):
            raise CustodyError("The original prepared Fresh selection changed.")
        self._require(guide_id, deployment_id)
        return self._fresh_pin
