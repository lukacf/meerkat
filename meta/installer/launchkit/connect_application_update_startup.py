"""Connect one admitted replacement through Toolkit's existing Restore runtime.

The existing update owner supplies all selections under its project lease. File
custody is not native authorization, revision acceptance, member settlement or
Ready. No resources are prepared, current pointer published or attempt retried
here. The enclosing startup factory retains and closes accepted source after
this owner's exact runtime cleanup, before releasing the original project lease.
"""
from __future__ import annotations

import asyncio
import math
from pathlib import Path

from .connect_accepted_source import AcceptedApplicationSource
from .connect_application_callbacks import SelectedApplicationCallbacks
from .connect_application_finalizer import _Current
from .connect_application_startup import (
    _joined, _require_fixed_roster, _source_checks, member_mcp_runtime_options, _select_hosted_mcp,
    _hosted_mcp_runtime_options,
)
from .connect_installer_caller import (
    CustodyError, ExactFile, ProductGatewayFile, _application_update_selection, _json, _path,
    _private_directory, _same_json_value,
)


def _file_identity(path):
    info = _path(path).lstat()
    return (info.st_dev, info.st_ino, info.st_size, info.st_mtime_ns,
            info.st_ctime_ns, info.st_mode, info.st_uid, info.st_nlink)


def _observe(pin, *, executable=False):
    """Join verified bytes to cheap metadata fences on the loop-side handoff."""
    before = _file_identity(pin.path)
    value = pin.verify(executable=True) if executable else pin.read()
    if _file_identity(pin.path) != before:
        raise CustodyError("A replacement input changed during its byte observation.")
    return value, before


def _config_stamp(value):
    """Retain configuration values and opaque provider identities, not copies."""
    if isinstance(value, dict):
        return (dict, tuple((key, _config_stamp(item)) for key, item in value.items()))
    if isinstance(value, (list, tuple)):
        return (type(value), tuple(_config_stamp(item) for item in value))
    if type(value) in (str, int, bool, bytes, type(None)):
        return (type(value), value)
    if type(value) is float:
        return (float, value.hex())
    return (type(value), id(value))


class ReplacementApplicationStartup:
    """One replacement connection inside the original admitted transaction.

    expected_revision is the original owner's actual native declaration decision:
    an observed integer for activation, or explicit None for its recovery/restore
    decision. This class does not infer that decision from a policy, association
    file, selected hash or a previous startup. prior_tool_policy must be the
    actual last accepted desired policy, including after intervening updates.
    """

    def __init__(self, *, project_lease, accepted_source, gateway: ExactFile,
            selection_file: ExactFile, prior_tool_policy: ExactFile,
            preparation_plan: ExactFile, association_directory: Path,
            expected_revision: int | None, startup_timeout: float,
            hosted_mcp_redirect_base=None, hosted_mcp_targets=()):
        _select_hosted_mcp(self, hosted_mcp_redirect_base, hosted_mcp_targets)
        if (not callable(getattr(project_lease, "require_current", None))
                or not isinstance(getattr(project_lease, "root", None), Path)
                or not isinstance(accepted_source, AcceptedApplicationSource)
                or any(not isinstance(pin, ExactFile) for pin in
                       (gateway, selection_file, prior_tool_policy, preparation_plan))
                or not isinstance(association_directory, Path)):
            raise CustodyError("Replacement startup requires its actual source owner, lease and exact input pins.")
        if (expected_revision is not None
                and (type(expected_revision) is not int or expected_revision < 0)):
            raise CustodyError("The native expected revision must retain the original owner's explicit decision.")
        if (isinstance(startup_timeout, bool) or not isinstance(startup_timeout, (int, float))
                or not math.isfinite(startup_timeout) or startup_timeout <= 0):
            raise CustodyError("Replacement startup requires its finite existing startup deadline.")
        self.lease, self.root, self.source = project_lease, project_lease.root, accepted_source
        (self._require_source_owner, self._require_source_current_async,
            self._require_source_filesystem, self._require_source_publication) = _source_checks(accepted_source, project_lease)
        self.gateway, self.selection_file = gateway, selection_file
        self.prior_tool_policy, self.plan = prior_tool_policy, preparation_plan
        self.association_directory = association_directory
        self.expected_revision, self.startup_timeout = expected_revision, startup_timeout
        self._original = (project_lease, accepted_source, self.root, gateway, selection_file,
            prior_tool_policy, preparation_plan, association_directory, expected_revision, startup_timeout)
        self._prepared = self._entered = False
        self._observed = None
        self.application = self.callbacks = self.runtime = self.config = None
        self.native_outcome = None
        self._shutdown_task = None
        self._config_values = None

    def _require_owner(self):
        _hosted_mcp_runtime_options(self)
        original = (self.lease, self.source, self.lease.root, self.gateway, self.selection_file,
            self.prior_tool_policy, self.plan, self.association_directory,
            self.expected_revision, self.startup_timeout)
        if any(type(a) is not type(b) or a != b for a, b in zip(original, self._original)):
            raise CustodyError("The original replacement owner or selected inputs changed.")
        self.lease.require_current()
        self._require_source_owner()

    def _require_current(self):
        self._require_owner()
        self._require_source_publication()
        if type(self.gateway) is ProductGatewayFile:
            self.gateway.require_current()
        if self._observed is not None:
            directories, files, current = self._observed
            if (any(_private_directory(path) != expected for path, expected in directories.items())
                    or any(_file_identity(pin.path) != identity for pin, identity in files)
                    or _Current.observe(self.root) != current):
                raise CustodyError("The retained replacement inputs or private output custody changed.")

    def _unconnected(self, application):
        _require_fixed_roster(self, application)
        if (application.context.root != self.root
                or application.context.runtime is not None
                or application._application_callbacks is not None):
            raise CustodyError("Replacement startup requires its unconnected fixed application.")

    def _read_inputs(self):
        """Verify byte/root joins; Toolkit retains all native semantic checks."""
        self._require_source_filesystem()
        gateway_parent = (self.gateway.enclosing[0] if type(self.gateway) is ProductGatewayFile
            else self.gateway.path.parent)
        directories = {path: _private_directory(path) for path in
            (self.root, self.association_directory, gateway_parent,
             self.selection_file.path.parent, self.prior_tool_policy.path.parent, self.plan.path.parent)}
        _, gateway_identity = _observe(self.gateway, executable=True)
        selection_bytes, selection_identity = _observe(self.selection_file)
        selected, artifacts = _application_update_selection(selection_bytes)
        if selected["predecessor"]["deployment_root"] != str(self.root):
            raise CustodyError("Replacement predecessor belongs to a different project root.")
        activation, manifest, local_policy, compilation, candidate_policy = artifacts
        bundle = manifest.path.parent
        if (activation.path.parent != self.root or self.root not in bundle.parents
                or local_policy.path.parent != bundle):
            raise CustodyError("Replacement predecessor artifacts lost their original deployment/bundle roots.")
        files = [(self.gateway, gateway_identity), (self.selection_file, selection_identity)]
        contents = []
        for pin in (*artifacts, self.prior_tool_policy, self.plan):
            directories.setdefault(pin.path.parent, _private_directory(pin.path.parent))
            value, identity = _observe(pin)
            contents.append(value)
            files.append((pin, identity))
        current = _Current.observe(self.root)
        if current.activation != activation:
            raise CustodyError("Replacement startup requires its exact original current activation.")
        plan_bytes = contents[-1]
        if len(plan_bytes) > 256 * 1024:
            raise CustodyError("The candidate callback plan exceeds its existing native carrier bound.")
        plan = _json(plan_bytes)
        application = plan.get("application") if isinstance(plan, dict) else None
        realm = application.get("realm") if isinstance(application, dict) else None
        if (not isinstance(plan, dict) or plan.get("deployment_root") != str(self.root)
                or not isinstance(realm, dict) or realm.get("context_root") != str(self.root)
                or realm.get("state_root") != str(self.root / ".mobkit/state")):
            raise CustodyError("Replacement callback plan must retain the original application storage roots.")
        if not _same_json_value(application, _json(contents[3])):
            raise CustodyError("The candidate callback plan differs from its exact selected compilation input.")
        if "helper" not in application:
            from .connect_installer_caller import HELPERLESS_UPDATE_UNAVAILABLE
            raise CustodyError(HELPERLESS_UPDATE_UNAVAILABLE)
        observed = (directories, tuple(files), current)
        if (any(_private_directory(path) != expected for path, expected in directories.items())
                or any(_file_identity(pin.path) != identity for pin, identity in files)
                or _Current.observe(self.root) != current):
            raise CustodyError("Replacement inputs changed during observation.")
        self._require_source_filesystem()
        if self._observed is not None and observed != self._observed:
            raise CustodyError("The original admitted replacement inputs changed before connection.")
        return observed

    async def prepare(self, application):
        """Bind candidate callbacks only, without preparing original resources."""
        if self._prepared:
            raise CustodyError("This replacement startup was already selected.")
        self._prepared = True
        self._require_current()
        self._unconnected(application)

        def observe():
            inputs = self._read_inputs()
            return inputs, SelectedApplicationCallbacks(self.plan)

        observed, callbacks = await _joined(observe)
        self._observed = observed
        self._require_current()
        self._unconnected(application)
        self.application, self.callbacks = application, callbacks
        application._application_callbacks = callbacks

    def builder_config(self, **options):
        """Preserve the host builder and the owner's actual native revision pin."""
        if self.callbacks is None or self.config is not None:
            raise CustodyError("Replacement builder is unselected or already supplied.")
        self._require_current()
        if any(options.get(key) is not None for key in ("mob_config_path", "mob_config_inline",
                "application_tool_policies", "prepared_application_bundle", "prepared_application_digest")):
            raise CustodyError("Replacement startup cannot select a competing definition, policy or carrier.")
        if (options.get("persistent_state") != str(self.root / ".mobkit/state")
                or any(options.get(key) is not self.application for key in ("roster_provider", "topology_provider"))):
            raise CustodyError("Replacement builder changed its application providers or storage root.")
        supplied = options.get("declare_spec_update_expected_revision")
        if supplied is not None and (type(supplied) is not type(self.expected_revision) or supplied != self.expected_revision):
            raise CustodyError("Replacement builder changed its native revision decision.")
        options.update(gateway_bin=str(self.gateway.path), init_deadline=self.startup_timeout,
                       declare_spec_update_expected_revision=self.expected_revision)
        # The generated host fills these same conventional options after the
        # builder returns. Resolve them here so its unchanged writes remain valid
        # while all selected configuration values can be fenced as one object.
        for field, relative in (("gating_config_path", "config/gating.toml"),
                ("access_config_path", "config/access.toml"),
                ("routing_config_path", "deployment/routing.toml")):
            path = self.root / relative
            if path.is_file() and options.get(field) is None:
                options[field] = str(path)
        from meerkat_meta.builder import MetaBuilderConfig
        self.config = MetaBuilderConfig(**options)
        self._config_values = _config_stamp(vars(self.config))
        return self.config

    def _check_config(self, config, application, *, connected=False):
        from meerkat_meta.builder import MetaBuilderConfig
        _require_fixed_roster(self, application)
        if (not isinstance(config, MetaBuilderConfig) or config is not self.config
                or _config_stamp(vars(config)) != self._config_values
                or application is not self.application or application.context.root != self.root
                or application._application_callbacks is not self.callbacks
                or application.context.runtime is not (self.runtime if connected else None)):
            raise CustodyError("Replacement changed its same config, application or selected callbacks.")

    async def connect(self, config, application):
        """Return the exact runtime only after its actual native connect result."""
        if self._entered or self.callbacks is None:
            raise CustodyError("Replacement connection is unselected or already entered.")
        self._entered = True
        self._require_current()
        self._check_config(config, application)
        await _joined(self._read_inputs)
        self._require_current()
        self._check_config(config, application)
        from meerkat_toolkit.application_update import ToolkitApplicationUpdateRuntime
        from meerkat_toolkit.host_control import ApplicationUpdateStartup, SelectedHostArtifact
        startup = ApplicationUpdateStartup(
            SelectedHostArtifact(str(self.selection_file.path), self.selection_file.sha256),
            SelectedHostArtifact(str(self.prior_tool_policy.path), self.prior_tool_policy.sha256),
            str(self.association_directory))
        runtime = ToolkitApplicationUpdateRuntime(config, startup, **member_mcp_runtime_options(application),
            **_hosted_mcp_runtime_options(self))
        self.runtime = runtime
        application.context.runtime = runtime
        try:
            application.bind_schedule_runnables(runtime)
            self.native_outcome = await runtime.connect()
            await _joined(self._read_inputs)
            self._require_current()
            self._check_config(config, application, connected=True)
            return runtime
        except BaseException as error:
            try:
                await self.shutdown(runtime)
            except BaseException as cleanup_error:
                # Keep the exact unconfirmed runtime in both owners. The caller
                # must inspect actual native and cleanup outcomes before recovery.
                raise BaseExceptionGroup("Replacement connect and same-runtime cleanup failed.",
                    [error, cleanup_error]) from None
            application.context.runtime = None
            raise

    async def shutdown(self, runtime):
        """Join only this owner's exact runtime; associations prove no outcome."""
        if runtime is None or runtime is not self.runtime:
            raise CustodyError("Replacement startup can only close its own retained runtime.")
        if self._shutdown_task is None:
            self._shutdown_task = asyncio.create_task(runtime.shutdown())
        while True:
            try:
                return await asyncio.shield(self._shutdown_task)
            except asyncio.CancelledError:
                if self._shutdown_task.cancelled():
                    raise
