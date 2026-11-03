"""Original destination source choices and installation-owned setup inputs.

The trusted durable worker supplies an actual stopped source carrier. These
private selections retain its accepted bytes, not a local GuideStore or a
browser runtime choice. Existing application defaults declare logical intent;
native inventory/preparation remains the sole supplier of native bindings and
authority. Importing this module creates no files or runtime.
"""
from __future__ import annotations

from copy import deepcopy
import hashlib
import inspect
from pathlib import Path
import tomllib

from .blueprint import Blueprint
from .connect_application_callbacks import _source_definition
from .connect_application_finalizer import _sync_directory, _write_bytes
from .connect_installer_caller import (CustodyError, ExactFile, _inventory_input,
    _json, _path, _private_directory, _private_parent, _same_json_value)
from .connect_policy_input import application_tool_policy_selection
from .deployment import DeploymentService, read_receipt, private_configuration_snapshot
from .deployment_application_bootstrap import bootstrap_installation_binding
from .destination_application_factory import DestinationApplicationFactory
from .forge_application_host import ForgeApplicationHost
from .local_application_factory import LocalApplicationFactory
from .local_application_preparation_inputs import LocalApplicationPreparationOwner
from .local_application_source import (_application_acquisition_selection,
    _application_viewer_selection, _compiler_sources, _json_bytes, _missing_source_input)
from .product_runtime_custody import (await_product_validation, product_source_custody,
    require_product_installation)


SOURCE_DIRECTORY = ".mobkit/forge-destination-source"


def _source_contents(files, runtime_target):
    """Read the accepted protected choices before creating private projections."""
    from . import compiler
    from .templates_v2.application_configuration import validate_application_fields
    from .templates_v2.console_configuration import console_authentication
    spec = Blueprint.model_validate(_json(files["blueprint.json"][0]))
    if not compiler._product_application_supported(spec, runtime_target=runtime_target):
        raise CustodyError(_missing_source_input(spec))
    definition = files["config/mob.toml"][0]
    _source_definition({"format": "toml", "text": definition.decode("utf-8")})
    from .application_inventory_source import application_inventory_source, POLICY_PATH
    inventory_bytes = application_inventory_source({name: entry[0] for name, entry in files.items()})
    policy_path = POLICY_PATH if inventory_bytes is not None else "application/tool-policy.json"
    if inventory_bytes is None:
        inventory_bytes = compiler.application_inventory_input(spec)
    inventory = _inventory_input(inventory_bytes)
    policy = files[policy_path][0]
    application_tool_policy_selection(policy,
        selected_member_identities=tuple(row["identity"] for row in inventory["roster"]))
    configuration = _json(files["application/configuration.json"][0])
    fields = validate_application_fields(configuration,
        existing_keys=tuple(row["key"] for row in compiler.environment_fields(spec)))
    console = _json(files["application/console.json"][0])
    authentication = console_authentication(console, fields)
    access = tomllib.loads(files["config/access.toml"][0].decode("utf-8"))
    admins = access.get("admins")
    # This is the original explicit protected source operator, not a Google
    # account, SSH identity, discovered user or inferred native grant.
    if (authentication != "local-password" or access.get("enabled") is not True
            or not isinstance(admins, list) or len(admins) != 1
            or type(admins[0]) is not str or not admins[0] or admins[0].strip() != admins[0]
            or any(ord(char) < 32 or ord(char) == 127 for char in admins[0])
            or not any(row["key"] == "CONSOLE_USER_ID" and row.get("required") is True for row in fields)):
        raise CustodyError("Destination defaults need the original single protected Console operator choice.")
    contents = {"blueprint.json": files["blueprint.json"][0], "mob.toml": definition,
        "application-inventory-input.json": inventory_bytes, "tool-policy.json": policy,
        "configuration.json": files["application/configuration.json"][0],
        "console.json": files["application/console.json"][0], "access.toml": files["config/access.toml"][0]}
    return contents, {"subject": admins[0], "private_configuration_key": "CONSOLE_USER_ID"}


class DestinationApplicationInputs:
    """One real destination factory and its concrete original private supplier."""

    def __init__(self, *, source, deployment, require_current, package, pins, acquisition, viewer,
            model_configuration=None):
        source = product_source_custody(source)
        if (not isinstance(deployment, DeploymentService) or not callable(require_current)
                or deployment.root != Path(source._context["host"]["home"]) / ".local/state/meta-forge/deployments"):
            raise CustodyError("Destination inputs need their actual canonical deployment and original worker owner.")
        self.context, self.owner_source = deepcopy(source._context), source._owner_source
        self.runtime_target, self.guide_id = source.origin, source.receipt["guide_id"]
        self.deployment_id = source.receipt["deployment_id"]
        self.directory = deployment.root / self.guide_id / self.deployment_id
        self.package = deepcopy(package)
        self._package = deepcopy(package)
        self.console_setup_selection = None
        self.model_configuration = deepcopy(model_configuration)
        self._model_configuration_custody = None
        if model_configuration is not None:
            from .connect_application_startup import _ModelConfiguration
            self._model_configuration_custody = _ModelConfiguration.observe(self.model_configuration)
        self._original_model_configuration_custody = self._model_configuration_custody
        self._owner_current, self.deployment = require_current, deployment
        self._pins = tuple(pins.values()) + (acquisition, viewer)
        self._bytes = tuple(pin.read() for pin in self._pins)
        self._source_directory = _private_directory(acquisition.path.parent)
        self._configured = (deployment, deepcopy(self.context), self.owner_source,
            self.runtime_target, self.guide_id, self.deployment_id, require_current)
        self.factory = None
        self.preparation_owner = LocalApplicationPreparationOwner(self)
        self._installed_admission = None
        self._recovery_source = self.installation = None
        self.require_current()
        factory = DestinationApplicationFactory(deployment=deployment, context=self.context,
            owner_source=self.owner_source, acquisition_selection=acquisition, viewer_selection=viewer,
            require_current=self.require_current, startup_selection=self.preparation_owner.startup_selection,
            preparation_inputs=self.preparation_owner.preparation_inputs,
            retain_preparation_inputs=self._retain,
            startup_continuation=self.preparation_owner.retain_restore)
        self.factory = factory
        host = ForgeApplicationHost(guide_id=self.guide_id, runtime_target=source.origin,
            acquisition_selection=acquisition, viewer_selection=viewer, source_custody=source,
            require_current=factory._current, startup_selection=self.preparation_owner.startup_selection,
            preparation_inputs=self.preparation_owner.preparation_inputs,
            retain_preparation_inputs=self._retain,
            startup_continuation=self.preparation_owner.retain_restore)
        factory.host = host
        self.preparation_owner.bind_host(host)
        self._host = host
        host.require_current()

    @property
    def host(self):
        return None if self.factory is None else self.factory.host

    _read = staticmethod(LocalApplicationFactory._read)
    _console_choice = staticmethod(LocalApplicationFactory._console_choice)
    _selected_model_configuration = LocalApplicationFactory._selected_model_configuration

    def require_current(self):
        self._selected_model_configuration()
        if ((self.deployment, self.context, self.owner_source, self.runtime_target,
                self.guide_id, self.deployment_id, self._owner_current) != self._configured
                or self.package != self._package
                or _private_directory(self._source_directory[0]) != self._source_directory):
            raise CustodyError("The original destination input selection changed.")
        result = self._owner_current()
        if result is False or inspect.isawaitable(result) or inspect.isasyncgen(result):
            if inspect.iscoroutine(result):
                result.close()
            raise CustodyError("Destination input ownership must complete on its original calling task.")
        if tuple(pin.read() for pin in self._pins) != self._bytes:
            raise CustodyError("The original destination private source projections changed.")
        if self.factory is not None and (self.factory.deployment is not self.deployment
                or self.factory.context != self.context or self.factory.owner_source != self.owner_source
                or self.factory._owner_current != self.require_current
                or self.factory._retain_preparation_inputs != self._retain
                or self.factory._preparation_inputs != self.preparation_owner.preparation_inputs
                or self.factory._startup_selection != self.preparation_owner.startup_selection
                or self.factory._startup_continuation != self.preparation_owner.retain_restore):
            raise CustodyError("Preserve this input owner's original actual destination factory.")
        if (hasattr(self, "_host") and (self.host is not self._host
                or self.preparation_owner.host is not self._host)
                or self.console_setup_selection is not None):
            raise CustodyError("Preserve this owner's original host and automatic Console source choice.")

    def _retain(self, guide_id, deployment_id, supervisor):
        self.require_current()
        if self.preparation_owner.host is not self.host:
            raise CustodyError("Destination inputs lost their original successful Install host.")
        return self.preparation_owner.retain(guide_id, deployment_id, supervisor)

    def _require_recovery_current(self):
        self.require_current()
        source = product_source_custody(self._recovery_source)
        if (self._installed_admission is not self or source._closed
                or self.host._product_source_custody is not source
                or source._context != self.context or source._owner_source != self.owner_source
                or source.origin != self.runtime_target):
            raise CustodyError("Recovery needs this original live destination source admission.")
        source.require_owner_current()
        receipt = read_receipt(self.directory)
        if bootstrap_installation_binding(receipt) != self.installation:
            raise CustodyError("The original destination installation changed during input recovery.")

    def _require_recovery_files(self):
        """Full source/installation reader, only in its joined file lane."""
        source = product_source_custody(self._recovery_source)
        receipt = read_receipt(self.directory)
        require_product_installation(source, receipt)
        if bootstrap_installation_binding(receipt) != self.installation:
            raise CustodyError("The original destination installation changed during input recovery.")


async def _select(source, deployment, require_current, *, recover, model_configuration=None):
    """Publish or read only the original source projection under its real lease."""
    from .destination_product_install import _fresh, _inputs
    source = product_source_custody(source)
    source.require_owner_current()
    if (not isinstance(deployment, DeploymentService) or not deployment.lock.locked()
            or not callable(require_current)
            or deployment.root != Path(source._context["host"]["home"]) / ".local/state/meta-forge/deployments"):
        raise CustodyError("Destination selection needs its original canonical deployment owner.")
    from .connect_application_startup import _ModelConfiguration
    selected_model = None if model_configuration is None else _ModelConfiguration.observe(
        deepcopy(model_configuration))

    def model_current():
        value = None if selected_model is None else selected_model.selection()
        if not _same_json_value(model_configuration, value):
            raise CustodyError("The original destination model configuration choice changed.")
        if selected_model is not None:
            selected_model.require_filesystem_current()
        return value

    model_current()
    _, package, _ = await await_product_validation(source, _inputs, source)
    model_current()
    source.require_owner_current()
    contents, operator = _source_contents(package, source.origin)
    compiler_sources = _compiler_sources()
    directory = _path(source._root / SOURCE_DIRECTORY, fresh=not recover)
    _private_directory(directory.parent)
    canonical = deployment.root / source.receipt["guide_id"] / source.receipt["deployment_id"]
    if recover:
        await await_product_validation(source, require_product_installation, source, read_receipt(canonical))
        _private_directory(directory)
    else:
        await await_product_validation(source, _fresh, canonical, source._root)
    source.require_owner_current()
    if not recover:
        directory.mkdir(mode=0o700)
        _sync_directory(directory.parent)
    observed = _private_directory(directory)

    def current():
        model_current()
        source.require_owner_current()
        result = require_current()
        if result is False or inspect.isawaitable(result) or inspect.isasyncgen(result):
            if inspect.iscoroutine(result):
                result.close()
            raise CustodyError("The original destination source owner changed during projection.")
        if _private_directory(directory) != observed or _compiler_sources() != compiler_sources:
            raise CustodyError("The selected destination source directory or compiler changed.")

    def retain(name, content):
        current()
        path = directory / name
        if recover:
            _private_parent(path)
            pin = ExactFile(path, hashlib.sha256(content).hexdigest())
            if pin.read() != content:
                raise CustodyError("Existing destination source differs from the original accepted bytes.")
        else:
            pin = _write_bytes(path, content)
        current()
        return pin

    pins = {name: retain(name, content) for name, content in contents.items()}
    acquisition_value = _application_acquisition_selection(pins, [item[0] for item in compiler_sources])
    from .application_inventory_source import selects_application_inventory, POLICY_PATH
    if selects_application_inventory({name: entry[0] for name, entry in package.items()}):
        acquisition_value["source_policy_path"] = POLICY_PATH
    acquisition = retain("source-selection.json", _json_bytes(acquisition_value))
    viewer = retain("APPLICATION-SELECTION.json", _json_bytes(_application_viewer_selection(
        pins, acquisition, operator)))
    current()
    result = DestinationApplicationInputs(source=source, deployment=deployment,
        require_current=require_current, package=package, pins=pins, acquisition=acquisition, viewer=viewer,
        model_configuration=model_current())
    current()
    return result


async def create_destination_application_inputs(source, *, deployment, require_current,
        model_configuration=None):
    """Create original private choices once before this actual factory's Install.

    The supplying source scope remains entered through this call. The caller
    closes it before invoking factory.install, which takes its own actual lease.
    Partial selection files are preserved on refusal or cancellation, never
    adopted as a successful Install or retried automatically.
    """
    result = await _select(source, deployment, require_current, recover=False,
        model_configuration=model_configuration)
    result.factory.bind()
    return result


async def recover_destination_application_inputs(source, *, deployment, require_current,
        model_configuration=None):
    """Reconstruct existing private inputs without Install/default/Fresh replay.

    This entry needs the caller's already admitted actual installed source and
    stopped deployment action lock. It is never used by GET or file discovery.
    The original source scope and lease stay held until all joined reads settle.
    """
    from .deployment_application_bootstrap import (require_bootstrap_history_settled,
        require_restore_history_current)
    from .deployment_supervisor import pending_custody
    from .deployment_updates import require_replacement_history_settled
    from .deployment_worker import Supervisor
    from .runtime_dependencies import verify_runtime
    source = product_source_custody(source)
    if not isinstance(deployment, DeploymentService) or not deployment.lock.locked():
        raise CustodyError("Destination input recovery requires its actual claimed installation action.")
    directory, receipt, stopped = deployment._stopped_application_owner(
        source.receipt["guide_id"], source.receipt["deployment_id"])
    stopped()
    await await_product_validation(source, require_product_installation, source, receipt)
    if receipt["deployment_id"] in deployment._application_preparations:
        raise CustodyError("Settle the original preparation before reconstructing its input owner.")
    view = source._installed_view
    startup = deepcopy(view.inputs) if view is not None else None
    def inspect_history():
        if (source._installed_view is not view
                or view is not None and not _same_json_value(view.inputs, startup)):
            raise CustodyError("Recovery changed its original admitted startup selection.")
        if startup is None:
            require_bootstrap_history_settled(directory)
        else:
            # The installed source already admitted this exact private input
            # view. Reuse its original Restore lineage rather than infer an
            # ordinary restart from absent selection or publish a new admission.
            require_restore_history_current(directory, startup)
        require_replacement_history_settled(directory)
        if pending_custody(directory, receipt):
            raise CustodyError("The original destination support custody remains unresolved.")
    await await_product_validation(source, inspect_history)
    stopped()
    if startup is not None and not _same_json_value(startup.get("model_configuration"), model_configuration):
        raise CustodyError("Recovery needs the original admitted startup model configuration selection.")
    result = await _select(source, deployment, require_current, recover=True,
        model_configuration=model_configuration)
    result._installed_admission, result._recovery_source = result, source
    result.installation = deepcopy(bootstrap_installation_binding(receipt))
    owner, host = result.preparation_owner, result.host
    result._require_recovery_current()
    await await_product_validation(source, result._require_recovery_files)
    result._require_recovery_current()
    values, private = private_configuration_snapshot(source._root)
    host.validate_installed_settings(result.guide_id, receipt, source._root, result.package, values)
    proof = await await_product_validation(source, verify_runtime, source._root,
        result.runtime_target.meta.version, runtime_target=result.runtime_target, source_custody=source)
    stopped()
    result._require_recovery_current()
    await await_product_validation(source, result._require_recovery_files)
    result._require_recovery_current()
    if proof != receipt.get("installed_candidate_runtime_proof"):
        raise CustodyError("The existing destination runtime differs from its original installed proof.")
    retained = owner._recovery_inputs(result, receipt)
    _private_directory(source._root / ".mobkit/home")
    supervisor = Supervisor(directory, source_custody=source)
    inputs = owner._recover(result, supervisor, retained)
    host._installation_supervisor, host._retained_preparation_inputs = supervisor, inputs
    host._retained_fresh_paths = deepcopy(inputs.fresh_paths)
    inventory = await await_product_validation(source, owner._recover_startup, source._lease)
    stopped()
    if inventory is not None:
        expected = host.preparation_plan(inventory, **owner.declarations(inventory))
        await await_product_validation(source, owner._require_recovered_plan,
            inventory, expected, source._lease)
    result._require_recovery_current()
    owner._require(result.guide_id, result.deployment_id)
    if private_configuration_snapshot(source._root)[1] != private:
        raise CustodyError("The original destination private settings changed during recovery.")
    result.factory._install_entered = True
    result.factory._retain_startup_pin()
    if startup is not None and not _same_json_value(result.factory._view(), startup):
        raise CustodyError("Recovered startup differs from its original admitted private selection.")
    await await_product_validation(source, result._require_recovery_files)
    result._require_recovery_current()
    stopped()
    result.factory.bind()
    return result
