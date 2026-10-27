"""Start a fixed application through the selected ordinary Meta SDK gateway.

The original installation owner supplies accepted source, the ordinary gateway
and compiler-produced policy bytes. This adapter does not select a gateway,
compile native policy, prepare Toolkit, add an acquisition helper or qualify an
artifact. It is a separate consumer seam for the helperless installation path.
"""
from __future__ import annotations

from copy import deepcopy
import hashlib
import math
import os
from pathlib import Path
import stat

from .connect_installer_caller import CustodyError, ExactFile, MAX_INPUT_BYTES, _path
from .connect_application_startup import (
    _ModelConfiguration, _joined, _model_builder_options, _owned_shutdown, _require_fixed_roster,
    _require_model_builder, _require_model_owner, _require_model_spec,
    _source_checks,
)


def read_definition(pin):
    """Read accepted public code without applying private-policy file modes."""
    path = _path(pin.path)
    descriptor = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    with os.fdopen(descriptor, "rb") as stream:
        before = os.fstat(stream.fileno())
        if (not stat.S_ISREG(before.st_mode) or before.st_uid != os.getuid()
                or before.st_nlink != 1 or before.st_mode & 0o022
                or not 0 < before.st_size <= MAX_INPUT_BYTES):
            raise CustodyError("The accepted mob definition is not owned, ordinary and bounded.")
        content = stream.read(MAX_INPUT_BYTES + 1)
        after, current = os.fstat(stream.fileno()), path.lstat()
        def identity(info):
            return (info.st_dev, info.st_ino, info.st_mode, info.st_uid,
                info.st_nlink, info.st_size, info.st_mtime_ns, info.st_ctime_ns)
        if (len(content) != before.st_size or identity(before) != identity(after)
                or identity(after) != identity(current)
                or hashlib.sha256(content).hexdigest() != pin.sha256):
            raise CustodyError("The accepted mob definition changed during observation.")
    content.decode("utf-8")
    return content


class OrdinaryMetaStartup:
    """Retain one genuine Meta runtime through its actual shutdown settlement.

    This constructor is for the trusted deployment owner, never a blueprint,
    request field or environment selector. It does not admit an installation;
    the existing owner must join these inputs to its accepted application.
    """

    require_tool_policy_source = False
    # Its Console keeps the configured password login. Native Console login
    # enrollment needs a selected Toolkit runtime, which this path never has.
    console_login = False

    def __init__(self, *, project_lease, accepted_source, gateway,
                 definition, compiled_policies, startup_timeout,
                 model_configuration=None):
        if (not callable(getattr(project_lease, "require_current", None))
                or not isinstance(getattr(project_lease, "root", None), Path)
                or type(gateway) is not ExactFile or type(definition) is not ExactFile
                or type(compiled_policies) is not tuple
                or any(type(pin) is not ExactFile for pin in compiled_policies)
                or type(startup_timeout) not in (int, float)
                or not math.isfinite(startup_timeout) or startup_timeout <= 0):
            raise CustodyError("Ordinary Meta needs its original project, gateway, definition and compiled policies.")
        self.lease, self.root = project_lease, project_lease.root
        if definition.path != self.root / "config/mob.toml":
            raise CustodyError("Ordinary Meta needs the accepted application's exact mob definition.")
        self.gateway, self.definition, self.compiled_policies = gateway, definition, compiled_policies
        self.startup_timeout = startup_timeout
        self._selection = (project_lease, self.root, gateway, definition, compiled_policies, startup_timeout)
        (self._require_source_owner, self._require_source_current_async,
         self._require_source_filesystem, self._require_source_publication) = _source_checks(accepted_source, project_lease)
        self._model_configuration = (None if model_configuration is None else
            _ModelConfiguration.capture(deepcopy(model_configuration)))
        self._original_model_configuration = self._model_configuration
        self.application = self.config = self.runtime = None
        self._prepared = self._entered = False
        self._shutdown_task = self._shutdown_runtime = None
        self._definition_bytes = self._policy_bytes = None
        self._config_providers = self._http_options = None
        self._require_current()

    def _require_current(self):
        if (self.lease, self.root, self.gateway, self.definition,
                self.compiled_policies, self.startup_timeout) != self._selection:
            raise CustodyError("Preserve the original ordinary Meta startup selection.")
        self.lease.require_current()
        if self.lease.root != self.root:
            raise CustodyError("The ordinary Meta project owner changed its root.")
        self._require_source_owner()
        self._require_source_publication()
        _require_model_owner(self)

    def require_model_configuration(self, spec):
        _require_model_spec(self, spec)

    def _check_application(self, application, *, connected=False):
        _require_fixed_roster(self, application)
        if (application.context.root != self.root
                or application._application_callbacks is not None
                or application.context.runtime is not (self.runtime if connected else None)):
            raise CustodyError("Ordinary Meta needs its unconnected fixed application without Toolkit callbacks.")

    def _read_inputs(self):
        self._require_source_filesystem()
        self.gateway.verify(executable=True)
        # Generated mob.toml is public application code (normally 0644), not
        # a private native preparation artifact. The existing accepted-source
        # owner still joins it to this project and validates the full closure.
        definition = read_definition(self.definition)
        policies = tuple(pin.read() for pin in self.compiled_policies)
        # Native authenticates each compiled policy and its digest. Retain its
        # exact UTF-8 bytes; no Python reconstruction of the native wire format.
        definition.decode("utf-8")
        for content in policies:
            content.decode("utf-8")
        if self._model_configuration is not None:
            self._model_configuration.require_filesystem_current()
        self._require_source_filesystem()
        return definition, policies

    async def prepare(self, application):
        if self._prepared:
            raise CustodyError("Ordinary Meta startup preparation was already entered.")
        self._prepared = True
        self._require_current()
        self._check_application(application)
        content = await _joined(self._read_inputs)
        self._require_current()
        self._check_application(application)
        self._definition_bytes, self._policy_bytes = content
        self.application = application
        # An app copied from an earlier template installs the default Console
        # login factory for every startup. That default enrolls only through a
        # selected Toolkit runtime, so here it could only refuse each login.
        # Clear it only when no hook is present: the configured password session
        # and native_auth are this route's contract. A present hook, valid or
        # not, keeps the guarded factory and its own refusals.
        if getattr(getattr(application, "integration", None), "console_login_factory", None) is None:
            application.console_login_factory = None

    def builder_config(self, **options):
        from meerkat_meta.builder import MetaBuilderConfig
        if self.application is None or self.config is not None:
            raise CustodyError("Ordinary Meta configuration needs its one prepared application.")
        self._require_current()
        self._check_application(self.application)
        if (any(options.get(name) is not None for name in (
                "mob_config_path", "mob_config_inline", "application_tool_policies",
                "prepared_application_bundle", "prepared_application_digest",
                "declare_spec_update_expected_revision"))
                or options.get("gateway_bin") not in (None, str(self.gateway.path))
                or options.get("persistent_state") != str(self.root / ".mobkit/state")
                or options.get("roster_provider") is not self.application
                or options.get("topology_provider") is not self.application):
            raise CustodyError("Ordinary Meta cannot substitute another application, preparation or runtime.")
        if self._model_configuration is None and options.get("meerkat_config_path") is not None:
            raise CustodyError("A model config path requires its original pinned model configuration.")
        options.update(gateway_bin=str(self.gateway.path),
            mob_config_inline=self._definition_bytes.decode("utf-8"),
            application_tool_policies=([content.decode("utf-8") for content in self._policy_bytes]
                if self._policy_bytes else None),
            init_deadline=self.startup_timeout)
        _model_builder_options(self, options)
        self.config = MetaBuilderConfig(**options)
        self._config_providers = tuple(getattr(self.config, name) for name in (
            "session_builder", "runtime_store", "agent_customizer", "auth_config"))
        self._http_options = tuple(getattr(self.config, name) for name in (
            "http_listen", "http_public_base_url", "allow_remote", "console_require_app_auth",
            "console_config_path"))
        return self.config

    def _check_config(self, config, application, *, connected=False):
        from meerkat_meta.builder import MetaBuilderConfig
        self._check_application(application, connected=connected)
        _require_model_builder(self, config)
        if (type(config) is not MetaBuilderConfig or config is not self.config
                or application is not self.application
                or config.gateway_bin != str(self.gateway.path)
                or config.mob_config_path is not None
                or config.mob_config_inline != self._definition_bytes.decode("utf-8")
                or config.application_tool_policies != ([value.decode("utf-8") for value in self._policy_bytes]
                    if self._policy_bytes else None)
                or config.persistent_state != str(self.root / ".mobkit/state")
                or config.roster_provider is not application or config.topology_provider is not application
                or config.init_deadline != self.startup_timeout
                or config.prepared_application_bundle is not None
                or config.prepared_application_digest is not None
                or config.declare_spec_update_expected_revision is not None
                or self._model_configuration is None and config.meerkat_config_path is not None
                or any(getattr(config, name) is not original for name, original in zip(
                    ("session_builder", "runtime_store", "agent_customizer", "auth_config"), self._config_providers))
                or tuple(getattr(config, name) for name in (
                    "http_listen", "http_public_base_url", "allow_remote", "console_require_app_auth",
                    "console_config_path")) != self._http_options):
            raise CustodyError("The genuine Meta configuration changed its selected ordinary application.")

    async def connect(self, config, application):
        if self._entered or self.application is None:
            raise CustodyError("Ordinary Meta connection is unprepared or already entered.")
        self._entered = True
        self._require_current()
        self._check_config(config, application)
        await self._require_source_current_async()
        content = await _joined(self._read_inputs)
        self._require_current()
        self._check_config(config, application)
        if content != (self._definition_bytes, self._policy_bytes):
            raise CustodyError("Ordinary Meta definition or compiled policy changed before connection.")
        from meerkat_meta.runtime import MetaRuntime
        runtime = MetaRuntime(config)
        self.runtime = runtime
        application.context.runtime = runtime
        try:
            application.bind_schedule_runnables(runtime)
            await runtime.connect()
            await self._require_source_current_async()
            self._require_current()
            self._check_config(config, application, connected=True)
            if await _joined(self._read_inputs) != content:
                raise CustodyError("Ordinary Meta source changed during native startup.")
            return runtime
        except BaseException as original_failure:
            try:
                await self.shutdown(runtime)
            except BaseException as cleanup_error:
                raise BaseExceptionGroup("Ordinary Meta startup and retained cleanup failed.",
                    [original_failure, cleanup_error]) from None
            application.context.runtime = None
            raise

    async def shutdown(self, runtime):
        if runtime is None or runtime is not self.runtime:
            raise CustodyError("Ordinary Meta can only settle its original runtime.")
        return await _owned_shutdown(self, runtime)
