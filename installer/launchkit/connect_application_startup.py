"""Connect an approved application through the existing Forge host.

The installation owner supplies original file pins and its project lease. Fresh
installation also supplies its maintained command lane. This is not a browser
configuration loader, artifact installer, native authority issuer or
retained-member update transaction.
"""
from __future__ import annotations

import asyncio
from copy import deepcopy
from dataclasses import asdict, dataclass
import hashlib
import inspect
import json
import math
import os
from pathlib import Path
import re
import stat
import sys
from urllib.parse import urlsplit

from .connect_application_finalizer import ApplicationPolicyFinalizer, _Current, application_command_executor
from .connect_installer_caller import (
    CustodyError, ExactFile, ProductGatewayFile, NativeInstallerCaller, _artifact_pin,
    _json, _path, _private_directory, _read, _selection,
)


async def _join(task):
    """Join an entered callback or blocking SDK call despite cancellation."""
    while True:
        try:
            return await asyncio.shield(task)
        except asyncio.CancelledError:
            if task.cancelled():
                raise


class NativeOriginalOperations:
    def __init__(self, owner, application, runtime):
        self.owner, self.application, self.runtime = owner, application, runtime
        self.callbacks = owner.callbacks
        self._closed = False
        self._pending = set()

    def _current(self):
        self.owner._require_current()
        self.callbacks._current()
        context = self.application.context
        if (self._closed or self.owner.runtime is not self.runtime
                or context.runtime is not self.runtime
                or self.owner.application is not self.application
                or self.application._application_callbacks is not self.callbacks
                or self.owner._shutdown_task is not None or context.stop_event.is_set()):
            raise CustodyError("Native preparation requires its current connected application owner.")

    async def prepare(self, submission, request, *, retain_request, timeout=60.0):
        """Prepare exactly once after the app durably retains immutable JSON.

        A returned original is a preparation receipt, not admission or delivery.
        Unknown settlement preserves the same original, request and issuance IDs.
        retain_request is async; its bytes belong only to this original.
        """
        from meerkat_toolkit.host_control import (
            PrivateOriginalRef, PrivateOriginalSubmit, PrivateSourceEvidence,
        )

        self._current()
        if (not isinstance(submission, PrivateOriginalSubmit)
                or not isinstance(submission.original, PrivateOriginalRef)
                or not isinstance(submission.content, str)
                or not isinstance(submission.transport_authentication, PrivateSourceEvidence)
                or not isinstance(request, dict) or not callable(retain_request)):
            raise TypeError("Native preparation requires an authenticated submission and retention callback.")
        if timeout is not None and (isinstance(timeout, bool) or not isinstance(timeout, (int, float))
                or not math.isfinite(timeout) or timeout <= 0):
            raise ValueError("Native preparation timeout must be positive and finite.")
        # Preserve exact values without sharing mutable request aliases with
        # application callbacks. The public SDK validates the native vocabulary.
        try:
            from meerkat_toolkit.host_control import validate_native_original_operations_request as validate
        except ImportError as error:
            raise CustodyError("The selected Connect SDK does not support native operation request validation.") from error
        submission = deepcopy(submission)
        request = validate(deepcopy(request))
        if type(request) is not dict:
            raise CustodyError("The selected Connect SDK did not return a validated request object.")
        serialized = json.dumps({"submission": asdict(submission), "request": request},
            ensure_ascii=True, allow_nan=False, sort_keys=True, separators=(",", ":"))
        selected = json.loads(serialized)["request"]
        client = self.runtime.host_control
        prepare = getattr(client, "prepare_original_native_operations", None)
        if not callable(prepare):
            raise CustodyError("The selected Connect SDK does not support native operation preparation.")
        cancelled = asyncio.Event()

        async def enter():
            self._current()
            if cancelled.is_set():
                raise asyncio.CancelledError
            await retain_request(serialized)
            self._current()
            if cancelled.is_set():
                raise asyncio.CancelledError
            if self.runtime.host_control is not client:
                raise CustodyError("The selected private host client changed before native preparation.")
            # The blocking SDK call is retained until it returns. Native checks
            # current member routes and completed Add state at its own boundary.
            return await asyncio.to_thread(prepare, submission, selected, timeout=timeout)

        task = asyncio.create_task(enter())
        self._pending.add(task)
        try:
            return await asyncio.shield(task)
        except asyncio.CancelledError:
            cancelled.set()
            try:
                await _join(task)
            except BaseException:
                # Cancellation is not evidence that a native write rolled back.
                pass
            raise
        finally:
            self._pending.discard(task)

    async def close(self):
        """Close entry, then join callbacks before the application store closes."""
        self._closed = True
        pending = tuple(self._pending)
        if pending:
            # Each caller owns its native result/refusal. Closing only joins
            # custody; it does not turn an unknown outcome into success.
            await _join(asyncio.gather(*pending, return_exceptions=True))


def bind_native_operations(owner, application, runtime):
    """Same private host client for fixed/dynamic members on Fresh and Restore."""
    application.context._native_original_operations = NativeOriginalOperations(owner, application, runtime)


DYNAMIC_ROSTER_UNSUPPORTED = ("This application has a dynamic agent roster (desired_roster and desired_edges). "
    "The selected Meta Toolkit product's application host needs a fixed roster, so it cannot start this "
    "application. Rebuild it with a fixed roster.")


def _host_composes_dynamic_roster(owner):
    """Whether this selected startup owner can compose a dynamic application.

    Fresh and installed restart can retain pinned native class declarations.
    Replacement and ordinary startup still require a fixed roster. This is the
    single eligibility check; preparation also joins the actual application.
    """
    if type(owner) not in (FreshApplicationStartup, RestoredApplicationStartup):
        return False
    from .connect_application_callbacks import SelectedApplicationCallbacks
    return any(item.get("purpose", "ordinary") == "ordinary"
               for item in SelectedApplicationCallbacks(owner.plan).member_classes.values())


def _require_fixed_roster(owner, application):
    """Refuse a dynamic roster by name, before a startup owner's other application checks.

    Every selected startup owner calls this first; it is the only place that
    decides whether a dynamic application may start under that owner.
    """
    if application.dynamic and not _host_composes_dynamic_roster(owner):
        raise CustodyError(DYNAMIC_ROSTER_UNSUPPORTED)


def member_mcp_runtime_options(application):
    """Older frozen applications have no participant; selected ones fail closed.

    Construction happens after application.load and before runtime.connect so
    native cold registration can read the same already-open admission store.
    """
    factory = getattr(application, "member_mcp_runtime_options", None)
    return factory() if factory is not None else {}


def _hosted_mcp_origin(base):
    """Check the Console mount only; Core validates the registered OAuth base.

    The trusted Python startup owner selects this value. Neither blueprint,
    environment, forwarded headers nor browser input can select or replace it.
    Preserve the original base bytes when passing them to the native child.
    """
    if base is None:
        return None
    try:
        if (type(base) is not str or not base.isascii() or not base
                or any(ord(c) <= 32 or ord(c) == 127 for c in base) or "\\" in base):
            raise ValueError
        parsed = urlsplit(base)
        if parsed.scheme != "https" or parsed.query or parsed.fragment or "?" in base or "#" in base:
            raise ValueError
        # Select the shipped layout explicitly. An editable installation can
        # otherwise resolve a missing submodule from an unrelated checkout.
        if (Path(__file__).parent / "templates_v2/console_auth.py").is_file():
            from .templates_v2.console_auth import _console_origin
        else:
            # Generated applications ship the protected Console at their root.
            from console_auth import _console_origin
        return _console_origin("https://" + parsed.netloc, None)
    except (TypeError, ValueError):
        raise CustodyError("The trusted hosted MCP base requires a valid HTTPS Console mount.") from None


def _select_hosted_mcp(owner, base, targets=()):
    if type(targets) is not tuple or len(targets) > 32 or targets and base is None:
        raise CustodyError("The trusted hosted MCP targets require their selected HTTPS base.")
    if targets:
        from meerkat_toolkit.mcp_auth import McpTarget
        if any(type(target) is not McpTarget for target in targets) or len(set(targets)) != len(targets):
            raise CustodyError("The trusted hosted MCP targets must be distinct configured McpTarget objects.")
    owner.console_public_origin = _hosted_mcp_origin(base)
    owner.hosted_mcp_redirect_base = base
    owner.hosted_mcp_targets = targets
    owner._original_hosted_mcp = (base, owner.console_public_origin, targets, deepcopy(targets))


def _hosted_mcp_runtime_options(owner):
    selected = (owner.hosted_mcp_redirect_base, owner.console_public_origin)
    if any(type(a) is not type(b) or a != b for a, b in zip(selected, owner._original_hosted_mcp)):
        raise CustodyError("The original hosted MCP startup selection changed.")
    if (owner.hosted_mcp_targets is not owner._original_hosted_mcp[2]
            or owner.hosted_mcp_targets != owner._original_hosted_mcp[3]):
        raise CustodyError("The original hosted MCP target selection changed.")
    return {} if selected[0] is None else {"hosted_mcp_redirect_base": selected[0]}


def _hosted_mcp_startup_options(owner):
    options = _hosted_mcp_runtime_options(owner)
    if options:
        options["hosted_mcp_targets"] = owner.hosted_mcp_targets
    return options


def _prepare_context_parents(root, project_lease, require_current, *, initialize):
    """Prepare only the deployment-owned parents required before native init.

    Native owns its stores, realm children, control files and grants. Restore
    verifies existing parents without creating or repairing any directory.
    """
    if type(initialize) is not bool or not callable(require_current):
        raise CustodyError("Context preparation needs its original startup owner and disposition.")
    root = _path(root)
    held = []

    def current():
        project_lease.require_current()
        result = require_current()
        if inspect.isawaitable(result):
            if inspect.iscoroutine(result):
                result.close()
            raise CustodyError("Context preparation needs a synchronous original owner fence.")
        if project_lease.root != root or result is False:
            raise CustodyError("Context preparation lost its original deployment owner.")
        for path, descriptor, identity in held:
            a, b = os.fstat(descriptor), path.lstat()
            actual = lambda info: (info.st_dev, info.st_ino, info.st_mode, info.st_uid)
            if actual(a) != identity or actual(b) != identity:
                raise CustodyError("A selected context parent changed during preparation.")

    def open_parent(path, parent=None, name=None):
        current()
        created = False
        if parent is not None and initialize:
            try:
                os.mkdir(name, mode=0o700, dir_fd=parent)
                created = True
            except FileExistsError:
                pass
        descriptor = os.open(path if parent is None else name,
            os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW, dir_fd=parent)
        try:
            info = os.fstat(descriptor)
            if (not stat.S_ISDIR(info.st_mode) or info.st_uid != os.getuid()
                    or (info.st_mode & 0o022 if parent is None
                        else stat.S_IMODE(info.st_mode) != 0o700)):
                raise CustodyError("Context parents must be owned ordinary private directories; no repair was performed.")
            identity = (info.st_dev, info.st_ino, info.st_mode, info.st_uid)
            held.append((path, descriptor, identity))
        except BaseException:
            os.close(descriptor)
            raise
        current()
        if created:
            os.fsync(parent)
            current()
        return descriptor

    try:
        base = open_parent(root)
        shared = open_parent(root / "shared", base, "shared")
        open_parent(root / "shared/data", shared, "data")
        open_parent(root / "shared/control", shared, "control")
        current()
    except OSError as error:
        raise CustodyError("The selected context parents are absent or unsafe; preserve this startup attempt.") from error
    finally:
        for _, descriptor, _ in reversed(held):
            os.close(descriptor)


def _model_configuration_shape(value):
    """Closed owner-selected carrier, without interpreting native auth tables."""
    if (type(value) is not dict or set(value) != {"host_config", "credential_config_root"}
            or type(value["host_config"]) is not dict
            or type(value["credential_config_root"]) is not str
            or not value["credential_config_root"]
            or len(value["credential_config_root"].encode()) > 4096
            or "\0" in value["credential_config_root"]):
        raise CustodyError("Model configuration requires its exact selected config and credential root.")
    pin = _artifact_pin(value["host_config"])
    if (type(value["host_config"]["path"]) is not str
            or type(value["host_config"]["sha256"]) is not str
            or not value["host_config"]["path"]
            or len(value["host_config"]["path"].encode()) > 4096
            or "\0" in value["host_config"]["path"]
            or re.fullmatch(r"[0-9a-f]{64}", pin.sha256) is None
            or not pin.path.is_absolute()
            or not Path(value["credential_config_root"]).is_absolute()):
        raise CustodyError("Model configuration needs an absolute original config pin and credential root.")
    return pin, Path(value["credential_config_root"])


def _model_file_identity(path):
    info = _path(path).lstat()
    return (info.st_dev, info.st_ino, info.st_size, info.st_mtime_ns,
        info.st_ctime_ns, info.st_mode, info.st_uid, info.st_nlink)


def _model_directory_identity(path):
    _private_directory(path)
    info = path.stat()
    # Native credential persistence can add children. Its selected directory
    # identity and custody remain fixed; its contents are never inspected here.
    return (info.st_dev, info.st_ino, info.st_mode, info.st_uid)


@dataclass(frozen=True)
class _ModelConfiguration:
    """Original file/root custody only, not credential or provider readiness."""

    host_config: ExactFile
    credential_config_root: Path
    file_identity: tuple
    parent_identity: tuple
    root_identity: tuple

    @classmethod
    def capture(cls, value):
        pin, root = _model_configuration_shape(value)
        path, root = _path(pin.path), _path(root)
        identity = _model_file_identity(path)
        if (not stat.S_ISREG(identity[5]) or identity[6] != os.getuid()
                or identity[7] != 1 or identity[5] & 0o077
                or not 0 < identity[2] <= 256 * 1024):
            raise CustodyError("The selected model config must be private, owned and bounded.")
        result = cls(pin, root, identity, _model_directory_identity(path.parent),
            _model_directory_identity(root))
        result.require_current()
        return result

    @classmethod
    def observe(cls, value):
        result = cls.capture(value)
        result.require_filesystem_current()
        return result

    def selection(self):
        return {"host_config": self.host_config.pin(),
            "credential_config_root": str(self.credential_config_root)}

    def require_current(self):
        if (_model_file_identity(self.host_config.path) != self.file_identity
                or _model_directory_identity(self.host_config.path.parent) != self.parent_identity
                or _model_directory_identity(self.credential_config_root) != self.root_identity):
            raise CustodyError("The original model config or credential-root custody changed.")

    def require_filesystem_current(self):
        self.require_current()
        content = _read(self.host_config.path, 256 * 1024)
        if hashlib.sha256(content).hexdigest() != self.host_config.sha256:
            raise CustodyError("The original model config no longer matches its selected bytes.")
        self.require_current()

    def require_environment(self):
        if (sys.platform != "linux"
                or os.environ.get("XDG_CONFIG_HOME") != str(self.credential_config_root)):
            raise CustodyError("The selected model configuration requires its original Linux credential environment.")
        self.require_current()


def _managed_model_classes(spec):
    if (type(spec) is not dict or type(spec.get("classes")) is not list
            or any(type(row) is not dict or type(row.get("model_auth", "api_key")) is not str
                or row.get("model_auth", "api_key") not in {"api_key", "managed_host"}
                for row in spec["classes"])):
        raise CustodyError("The installed model-auth intent requires its original application classes.")
    return any(row.get("model_auth", "api_key") == "managed_host" for row in spec["classes"])


def _require_model_owner(owner):
    selected = owner._model_configuration
    if selected is not owner._original_model_configuration:
        raise CustodyError("The startup changed its original selected model configuration.")
    if selected is not None:
        selected.require_environment()


def _require_model_spec(owner, spec):
    required = _managed_model_classes(spec)
    owner._require_current()
    if required and owner._model_configuration is None:
        raise CustodyError("Managed-host models require the original trusted startup model configuration.")


def _model_builder_options(owner, options):
    selected = owner._model_configuration
    if selected is not None:
        expected = str(selected.host_config.path)
        value = options.get("meerkat_config_path")
        if value is not None and (type(value) is not str or value != expected):
            raise CustodyError("The builder cannot replace its original selected model config.")
        options["meerkat_config_path"] = expected


def _require_model_builder(owner, config):
    selected = owner._model_configuration
    if selected is not None:
        value = getattr(config, "meerkat_config_path", None)
        if type(value) is not str or value != str(selected.host_config.path):
            raise CustodyError("The runtime changed its original selected model config.")


async def _joined(function, *args, **kwargs):
    task = asyncio.create_task(asyncio.to_thread(function, *args, **kwargs))
    cancelled = False
    while True:
        try:
            result = await asyncio.shield(task)
            break
        except asyncio.CancelledError:
            if task.cancelled():
                raise
            cancelled = True
        except BaseException:
            if cancelled:
                raise asyncio.CancelledError
            raise
    if cancelled:
        raise asyncio.CancelledError
    return result


async def _settle_shutdown(runtime, *, task=None):
    """Keep the exact SDK cleanup owner alive through repeated cancellation."""
    if task is None:
        task = asyncio.create_task(runtime.shutdown())
    while True:
        try:
            return await asyncio.shield(task)
        except asyncio.CancelledError:
            if task.cancelled():
                raise


async def _continue_owned_shutdown(owner, runtime):
    """Join the SDK's retained transport, preserving the first stop failure.

    Meta's GatewayCleanup contract permits a subsequent bounded shutdown of
    the same retained transport after a failed stop. Toolkit composes its
    private endpoint join with that stop. A normal return is the lifetime join;
    the termination record alone never permits releasing the project lease.
    """
    context = getattr(getattr(owner, "application", None), "context", None)
    participant = getattr(context, "_native_original_operations", None)
    if participant is not None:
        await participant.close()
    while True:
        owner._shutdown_attempt = asyncio.create_task(runtime.shutdown())
        try:
            await _settle_shutdown(runtime, task=owner._shutdown_attempt)
        except BaseException as error:
            if owner._shutdown_error is None:
                owner._shutdown_error = error
            owner._shutdown_latest_error = error
            # Only the selected SDK's public retained-cleanup contract permits
            # continuation. Legacy/test-only owners without it keep their exact
            # failed task rather than treating a second no-op as settlement.
            if not isinstance(getattr(type(runtime), "last_gateway_termination", None), property):
                raise
            # Each call is bounded by the original SDK. Keep the original app
            # lease while it remains uncertain, without spinning on failures or
            # calling connect, constructing a transport or repeating Fresh.
            await _settle_shutdown(runtime, task=asyncio.create_task(asyncio.sleep(0.25)))
            continue
        if owner._shutdown_error is not None:
            raise owner._shutdown_error from None
        return


def shutdown_joined(owner):
    """Read the actual latest SDK join, independently of its retained error.

    This permits releasing lifetime custody, not accepting a failed native
    operation. The original shutdown error and SDK termination record remain.
    """
    task = getattr(owner, "_shutdown_attempt", None)
    return (owner.runtime is getattr(owner, "_shutdown_runtime", None)
        and owner.runtime is not None and isinstance(task, asyncio.Task)
        and task.done() and not task.cancelled() and task.exception() is None)


async def _owned_shutdown(owner, runtime):
    """Connect failure and later cleanup retain one composed runtime owner."""
    if runtime is not owner.runtime or runtime is None:
        raise CustodyError("Selected startup can only close its own retained runtime.")
    if owner._shutdown_task is None:
        owner._shutdown_runtime = runtime
        owner._shutdown_attempt = None
        owner._shutdown_error = owner._shutdown_latest_error = None
        owner._shutdown_task = asyncio.create_task(_continue_owned_shutdown(owner, runtime))
    elif runtime is not owner._shutdown_runtime:
        raise CustodyError("Selected startup cannot replace its original cleanup owner.")
    return await _settle_shutdown(runtime, task=owner._shutdown_task)


def _source_checks(source, project_lease):
    """Retain the trusted owner's full source fences, without loading a selector.

    The frozen installer owns this carrier through runtime shutdown. Generated
    hosts receive its checks through Python, never through environment or public
    blueprint fields. The owner check stays on the event loop. Full source
    validation is joined off-loop, with the original synchronous source fence
    retained at the final publication boundary.
    """
    names = ("require_owner_current", "require_current_async", "require_filesystem_current", "require_current")
    checks = tuple(getattr(source, name, None) for name in names)
    receipt = getattr(source, "receipt", None)
    if (any(not callable(check) for check in checks)
            or not isinstance(receipt, dict)
            or receipt.get("destination") != str(project_lease.root)):
        raise CustodyError("Selected startup requires its original accepted application source owner.")
    return checks


class FreshApplicationStartup:
    """One same-runtime preparation, finalization, adoption and connection.

    This owner is deliberately supplied through Python by the trusted installer.
    A saved blueprint, environment flag or an existing finalization record cannot
    select it. Reconstructing this object does not authorize replay of an entered
    native attempt. The native driver and retained finalizer records enforce
    their existing admission and recovery contracts.
    """

    def __init__(self, *, project_lease, accepted_source, gateway: ExactFile,
            preparation_plan: ExactFile, source_policy: ExactFile,
            activation_template: ExactFile, command, command_timeout: float,
            startup_timeout: float, provision_attempt_path: Path,
            working_directory: Path, bundle_path: Path, activation_path: Path,
            source_files: tuple[ExactFile, ...] = (), model_configuration=None,
            hosted_mcp_redirect_base=None, hosted_mcp_targets=()):
        _select_hosted_mcp(self, hosted_mcp_redirect_base, hosted_mcp_targets)
        if (not callable(getattr(project_lease, "require_current", None))
                or not isinstance(getattr(project_lease, "root", None), Path)
                or any(not isinstance(pin, ExactFile) for pin in
                       (gateway, preparation_plan, source_policy, activation_template))
                or not callable(command)
                or not isinstance(source_files, tuple)
                or any(not isinstance(pin, ExactFile) for pin in source_files)):
            raise CustodyError("Fresh startup needs the existing project owner and original file pins.")
        for value in (command_timeout, startup_timeout):
            if (isinstance(value, bool) or not isinstance(value, (int, float))
                    or not math.isfinite(value) or value <= 0):
                raise CustodyError("The existing command and startup owners need finite deadlines.")
        self.lease, self.root = project_lease, project_lease.root
        self._model_configuration = (None if model_configuration is None else
            _ModelConfiguration.capture(deepcopy(model_configuration)))
        self._original_model_configuration = self._model_configuration
        (self._require_source_owner, self._require_source_current_async,
            self._require_source_filesystem, self._require_source_publication) = _source_checks(accepted_source, project_lease)
        self.gateway, self.plan = gateway, preparation_plan
        self._original_gateway = gateway
        self.startup_timeout = startup_timeout
        self._startup_timeout = startup_timeout
        self.caller = NativeInstallerCaller(gateway, execute=application_command_executor(
            command, root=project_lease.root, timeout=command_timeout),
            require_current=self._require_current,
            require_filesystem_current=(self._require_source_filesystem if self._model_configuration is None
                else self._require_filesystem_current),
            provision_attempt_path=provision_attempt_path)
        self._finalizer_inputs = dict(preparation_plan=preparation_plan,
            source_policy=source_policy, activation_template=activation_template,
            working_directory=working_directory, bundle_path=bundle_path,
            activation_path=activation_path, source_files=source_files,
            require_publication_current=self._require_source_publication)
        self._prepared = self._entered = False
        self._selected_dynamic = False
        self.application = self.callbacks = self.finalizer = self.runtime = None
        self.hold_timeout = None
        self._shutdown_task = self._shutdown_runtime = None

    def _require_owner(self):
        _hosted_mcp_runtime_options(self)
        self.lease.require_current()
        if self.lease.root != self.root:
            raise CustodyError("The fresh application project owner changed its selected root.")
        if self.startup_timeout != self._startup_timeout:
            raise CustodyError("The fresh application owner changed its selected startup deadline.")
        self._require_source_owner()
        _require_model_owner(self)

    def require_model_configuration(self, spec):
        """Check selected custody for managed intent, without checking credentials."""
        _require_model_spec(self, spec)

    def _require_filesystem_current(self):
        self._require_source_filesystem()
        if self._model_configuration is not None:
            self._model_configuration.require_filesystem_current()

    def _require_current(self):
        # Join full byte readers off-loop, then fence their handoff and native
        # admission with the original source owner's synchronous metadata check.
        self._require_owner()
        self._require_source_publication()
        if type(self._original_gateway) is ProductGatewayFile:
            if self.gateway is not self._original_gateway:
                raise CustodyError("The fresh startup changed its original selected product gateway.")
            self._original_gateway.require_current()

    def _check_unconnected_application(self, application):
        _require_fixed_roster(self, application)
        if (application.context.root != self.root
                or application.dynamic is not self._selected_dynamic
                or application.context.runtime is not None
                or application._application_callbacks is not None):
            raise CustodyError("Fresh startup requires its unconnected fixed application and project owner.")

    async def prepare(self, application):
        """Bind the approved finite helper before native host callbacks begin."""
        if self._prepared:
            raise CustodyError("This fresh application startup was already selected.")
        self._prepared = True
        self._selected_dynamic = application.dynamic
        self._require_current()
        self._check_unconnected_application(application)

        def read_inputs():
            from .connect_application_callbacks import SelectedApplicationCallbacks
            self._require_source_filesystem()
            if self._model_configuration is not None:
                self._model_configuration.require_filesystem_current()
            self.gateway.verify(executable=True)
            plan_bytes = self.plan.read()
            if len(plan_bytes) > 256 * 1024:
                raise CustodyError("The original preparation plan exceeds the native carrier limit.")
            value = _json(plan_bytes)
            if not isinstance(value, dict) or value.get("deployment_root") != str(self.root):
                raise CustodyError("The original plan selected a different deployment owner.")
            selected = value.get("application")
            realm = selected.get("realm") if isinstance(selected, dict) else None
            if (not isinstance(realm, dict)
                    or realm.get("context_root") != str(self.root)
                    or realm.get("state_root") != str(self.root / ".mobkit/state")):
                raise CustodyError("The selected realm must retain this generated host's storage roots.")
            startup = value.get("startup")
            if not isinstance(startup, dict) or startup.get("kind") not in {"initialize", "restore"}:
                raise CustodyError("Context preparation needs the plan's explicit native startup disposition.")
            milliseconds = value.get("hold_timeout_ms") if isinstance(value, dict) else None
            if type(milliseconds) is not int or not 0 < milliseconds <= 900_000:
                raise CustodyError("The original plan requires its bounded native hold deadline.")
            if milliseconds / 1000 >= self._startup_timeout:
                raise CustodyError("The native hold deadline leaves no time for the existing startup owner to observe Ready.")
            callbacks = SelectedApplicationCallbacks(self.plan)
            finalizer = ApplicationPolicyFinalizer(self.caller, **self._finalizer_inputs)
            self._require_source_filesystem()
            if self._model_configuration is not None:
                self._model_configuration.require_filesystem_current()
            return milliseconds / 1000, callbacks, finalizer, startup["kind"] == "initialize"

        hold_timeout, callbacks, finalizer, initialize = await _joined(read_inputs)
        self._require_current()
        self._check_unconnected_application(application)
        if application.dynamic:
            callbacks.dynamic_catalog(application.deployment)
        _prepare_context_parents(self.root, self.lease, self._require_current, initialize=initialize)
        self._require_current()
        self.hold_timeout, self.callbacks, self.finalizer = hold_timeout, callbacks, finalizer
        self.application = application
        application._application_callbacks = callbacks

    def builder_config(self, **options):
        """Use the installed genuine Meta builder, preserving host providers."""
        if self.finalizer is None:
            raise CustodyError("Fresh startup inputs must be selected before building the runtime.")
        self._require_current()
        from meerkat_meta.builder import MetaBuilderConfig
        if any(options.get(key) is not None for key in ("mob_config_path", "mob_config_inline",
                "application_tool_policies", "prepared_application_bundle", "prepared_application_digest",
                "declare_spec_update_expected_revision")):
            raise CustodyError("Fresh preparation cannot supply a competing definition, policy or update.")
        options["gateway_bin"] = str(self.gateway.path)
        _model_builder_options(self, options)
        options["init_deadline"] = self._startup_timeout
        if options.get("persistent_state") != str(self.root / ".mobkit/state"):
            raise CustodyError("The builder changed the selected application storage root.")
        return MetaBuilderConfig(**options)

    def _check_config(self, config, application):
        from meerkat_meta.builder import MetaBuilderConfig
        if not isinstance(config, MetaBuilderConfig):
            raise CustodyError("Fresh preparation requires the genuine installed Meta builder configuration.")
        _require_fixed_roster(self, application)
        _require_model_builder(self, config)
        if (application is not self.application or application.context.root != self.root
                or application.dynamic is not self._selected_dynamic
                or application.context.runtime is not None
                or application._application_callbacks is not self.callbacks
                or getattr(config, "roster_provider", None) is not application
                or getattr(config, "topology_provider", None) is not application
                or (application.dynamic and getattr(config, "agent_customizer", None) is not application)):
            raise CustodyError("The runtime changed the selected application provider or helper callbacks.")
        if application.dynamic:
            self.callbacks.dynamic_catalog(application.deployment)
        if (getattr(config, "gateway_bin", None) != str(self.gateway.path)
                or getattr(config, "init_deadline", None) != self._startup_timeout
                or getattr(config, "persistent_state", None) != str(self.root / ".mobkit/state")
                or any(getattr(config, key, None) is not None for key in
                    ("mob_config_path", "mob_config_inline", "application_tool_policies",
                     "prepared_application_bundle", "prepared_application_digest",
                     "declare_spec_update_expected_revision"))):
            raise CustodyError("The selected runtime changed the preparation, gateway or startup deadline.")

    async def connect(self, config, application):
        """Continue with the exact runtime whose native connect actually settles."""
        if self._entered or self.finalizer is None:
            raise CustodyError("Fresh application connection is unselected or already entered.")
        self._entered = True
        self._require_current()
        self._check_config(config, application)
        await self._require_source_current_async()
        self._require_current()
        self._check_config(config, application)
        await _joined(self.gateway.verify, executable=True)
        await _joined(self.plan.read)
        if self._model_configuration is not None:
            await _joined(self._model_configuration.require_filesystem_current)
        await self._require_source_current_async()
        self._require_current()
        self._check_config(config, application)
        from meerkat_toolkit.application_runtime import ToolkitApplicationRuntime
        from meerkat_toolkit.host_control import ApplicationPreparation, SelectedHostArtifact
        runtime = ToolkitApplicationRuntime(config,
            ApplicationPreparation(SelectedHostArtifact(str(self.plan.path), self.plan.sha256),
                timeout=self.hold_timeout), self.finalizer, **member_mcp_runtime_options(application),
            **_hosted_mcp_runtime_options(self))
        self.runtime = runtime
        application.context.runtime = runtime
        try:
            application.bind_schedule_runnables(runtime)
            await runtime.connect()
            if self._model_configuration is not None:
                await _joined(self._model_configuration.require_filesystem_current)
            await self._require_source_current_async()
            self._require_current()
            _require_model_builder(self, config)
            from .connect_member_class_requests import bind_member_class_requests
            bind_member_class_requests(self, application, runtime)
            bind_native_operations(self, application, runtime)
            return runtime
        except BaseException as original_failure:
            try:
                await self.shutdown(runtime)
            except BaseException as cleanup_error:
                if original_failure is not cleanup_error:
                    raise BaseExceptionGroup("Fresh connection and original cleanup failed.",
                        [original_failure, cleanup_error]) from None
                raise
            application.context.runtime = None
            raise

    async def shutdown(self, runtime):
        """Join cleanup of this exact connected runtime before its lease exits."""
        if runtime is not self.runtime or runtime is None:
            raise CustodyError("Fresh startup can only close its own retained runtime.")
        return await _owned_shutdown(self, runtime)


class RestoredApplicationStartup:
    """Restore one installed selection without repeating fresh preparation.

    The trusted installation owner supplies the retained selection and completed
    setup pin. Their byte custody does not prove native permission or Ready.
    Toolkit authenticates the selected bundle, restores existing native stores,
    and projects each actual provider result. This owner never publishes current,
    provisions stores, finalizes a candidate or falls back to fresh preparation.
    """

    def __init__(self, *, project_lease, accepted_source, gateway: ExactFile,
            selection_file: ExactFile, preparation_plan: ExactFile,
            completed_setup: ExactFile, startup_timeout: float, model_configuration=None,
            hosted_mcp_redirect_base=None, hosted_mcp_targets=()):
        _select_hosted_mcp(self, hosted_mcp_redirect_base, hosted_mcp_targets)
        if (not callable(getattr(project_lease, "require_current", None))
                or not isinstance(getattr(project_lease, "root", None), Path)
                or any(not isinstance(pin, ExactFile) for pin in
                       (gateway, selection_file, preparation_plan, completed_setup))):
            raise CustodyError("Installed restart needs the existing project owner and retained file pins.")
        if (isinstance(startup_timeout, bool) or not isinstance(startup_timeout, (int, float))
                or not math.isfinite(startup_timeout) or startup_timeout <= 0):
            raise CustodyError("Installed restart needs its finite startup deadline.")
        self.lease, self.root = project_lease, project_lease.root
        self._model_configuration = (None if model_configuration is None else
            _ModelConfiguration.capture(deepcopy(model_configuration)))
        self._original_model_configuration = self._model_configuration
        (self._require_source_owner, self._require_source_current_async,
            self._require_source_filesystem, self._require_source_publication) = _source_checks(accepted_source, project_lease)
        self.gateway, self.selection_file = gateway, selection_file
        self._original_gateway = gateway
        self.plan, self.setup = preparation_plan, completed_setup
        self.startup_timeout = startup_timeout
        self._startup_timeout = startup_timeout
        self._prepared = self._entered = False
        self._selected_dynamic = False
        self._root_directory = self._current = self._selected = self._setup_bytes = None
        self.application = self.callbacks = self.runtime = None
        self._shutdown_task = self._shutdown_runtime = None

    def _require_owner(self):
        _hosted_mcp_runtime_options(self)
        self.lease.require_current()
        if self.lease.root != self.root:
            raise CustodyError("The installed restart project owner changed its selected root.")
        if self.startup_timeout != self._startup_timeout:
            raise CustodyError("The installed restart owner changed its selected startup deadline.")
        self._require_source_owner()
        _require_model_owner(self)

    def require_model_configuration(self, spec):
        """Check selected custody for managed intent, without checking credentials."""
        _require_model_spec(self, spec)

    def _require_current(self):
        # The worker's source observation cannot cover a later loop-side
        # mutation. Recheck the original full source before the next action.
        self._require_owner()
        self._require_source_publication()
        if type(self._original_gateway) is ProductGatewayFile:
            if self.gateway is not self._original_gateway:
                raise CustodyError("The restored startup changed its original selected product gateway.")
            self._original_gateway.require_current()

    def _read_inputs(self):
        """Observe retained bytes and current under the existing project lease."""
        self._require_source_filesystem()
        if self._model_configuration is not None:
            self._model_configuration.require_filesystem_current()
        root_directory = _private_directory(self.root)
        self.gateway.verify(executable=True)
        selected, artifacts = _selection(self.selection_file.read())
        if selected["deployment_root"] != str(self.root):
            raise CustodyError("The retained restart selection has a different deployment owner.")
        activation, manifest, local_policy = artifacts
        current = _Current.observe(self.root)
        if current.activation != activation:
            raise CustodyError("Installed restart requires current to select its exact retained activation.")
        contents = tuple(pin.read() for pin in artifacts)
        bundle = manifest.path.parent
        if (self.root not in bundle.parents or local_policy.path.parent != bundle
                or self.setup.path.parent != bundle):
            raise CustodyError("The retained restart artifacts do not share their selected bundle root.")
        _private_directory(bundle)
        # Check received manifest membership only. Native owns the setup's typed
        # reference, policy meaning and authentication of the whole closure.
        value = _json(contents[1])
        if (not isinstance(value, dict) or set(value) != {"artifacts"}
                or not isinstance(value["artifacts"], list)):
            raise CustodyError("The retained restart manifest lacks its exact artifact membership.")
        membership = {}
        for row in value["artifacts"]:
            if (not isinstance(row, dict) or set(row) != {"path", "sha256"}
                    or not isinstance(row["path"], str) or row["path"] in membership):
                raise CustodyError("The retained restart manifest has ambiguous artifact membership.")
            membership[row["path"]] = row["sha256"]
        if (membership.get(self.setup.path.name) != self.setup.sha256
                or membership.get(local_policy.path.name) != local_policy.sha256):
            raise CustodyError("The retained restart setup or policy differs from its selected manifest.")
        setup_bytes = self.setup.read()
        plan_bytes = self.plan.read()
        if len(plan_bytes) > 256 * 1024:
            raise CustodyError("The original callback plan exceeds its native carrier limit.")
        plan = _json(plan_bytes)
        application = plan.get("application") if isinstance(plan, dict) else None
        realm = application.get("realm") if isinstance(application, dict) else None
        if (not isinstance(plan, dict) or plan.get("deployment_root") != str(self.root)
                or not isinstance(realm, dict) or realm.get("context_root") != str(self.root)
                or realm.get("state_root") != str(self.root / ".mobkit/state")):
            raise CustodyError("The original restart callback plan changed its selected storage roots.")
        if _Current.observe(self.root) != current:
            raise CustodyError("The native current pointer changed during restart observation.")
        self._require_source_filesystem()
        if self._model_configuration is not None:
            self._model_configuration.require_filesystem_current()
        if self._root_directory is not None and (root_directory != self._root_directory
                or current != self._current or selected != self._selected
                or setup_bytes != self._setup_bytes):
            raise CustodyError("The retained installed restart selection changed before connection.")
        return root_directory, current, selected, setup_bytes

    async def prepare(self, application):
        """Bind original callbacks without starting a preparation or runtime."""
        if self._prepared:
            raise CustodyError("This installed restart was already selected.")
        self._prepared = True
        self._selected_dynamic = application.dynamic
        self._require_current()
        _require_fixed_roster(self, application)
        if (application.context.root != self.root
                or application.dynamic is not self._selected_dynamic
                or application.context.runtime is not None
                or application._application_callbacks is not None):
            raise CustodyError("Installed restart requires its unconnected fixed application and project owner.")

        def read_inputs():
            from .connect_application_callbacks import SelectedApplicationCallbacks
            observed = self._read_inputs()
            callbacks = SelectedApplicationCallbacks(self.plan)
            callbacks.require_completed_classes(observed[3])
            return observed, callbacks

        observed, callbacks = await _joined(read_inputs)
        self._require_current()
        _require_fixed_roster(self, application)
        if (application.context.root != self.root
                or application.dynamic is not self._selected_dynamic
                or application.context.runtime is not None
                or application._application_callbacks is not None):
            raise CustodyError("The selected fixed application changed during restart observation.")
        self._root_directory, self._current, self._selected, self._setup_bytes = observed
        if application.dynamic:
            callbacks.dynamic_catalog(application.deployment)
        _prepare_context_parents(self.root, self.lease, self._require_current, initialize=False)
        self._require_current()
        self.application, self.callbacks = application, callbacks
        application._application_callbacks = callbacks

    def builder_config(self, **options):
        """Carry the literal completed setup through the genuine Meta builder."""
        if self.callbacks is None:
            raise CustodyError("Installed restart inputs must be selected before building the runtime.")
        self._require_current()
        from meerkat_meta.builder import MetaBuilderConfig
        if any(options.get(key) is not None for key in ("mob_config_path", "mob_config_inline",
                "application_tool_policies", "prepared_application_bundle", "prepared_application_digest",
                "declare_spec_update_expected_revision")):
            raise CustodyError("Installed restart cannot supply a competing definition, policy or update.")
        if options.get("persistent_state") != str(self.root / ".mobkit/state"):
            raise CustodyError("The restart builder changed its selected application storage root.")
        options.update(gateway_bin=str(self.gateway.path), init_deadline=self._startup_timeout,
            prepared_application_bundle=self._setup_bytes, prepared_application_digest=self.setup.sha256)
        _model_builder_options(self, options)
        return MetaBuilderConfig(**options)

    def _check_config(self, config, application):
        from meerkat_meta.builder import MetaBuilderConfig
        if not isinstance(config, MetaBuilderConfig):
            raise CustodyError("Installed restart requires the genuine installed Meta builder configuration.")
        _require_fixed_roster(self, application)
        _require_model_builder(self, config)
        if (application is not self.application or application.context.root != self.root
                or application.dynamic is not self._selected_dynamic
                or application.context.runtime is not None
                or application._application_callbacks is not self.callbacks
                or getattr(config, "roster_provider", None) is not application
                or getattr(config, "topology_provider", None) is not application
                or (application.dynamic and getattr(config, "agent_customizer", None) is not application)):
            raise CustodyError("The restart changed its selected application provider or callbacks.")
        if application.dynamic:
            self.callbacks.dynamic_catalog(application.deployment)
        if (getattr(config, "gateway_bin", None) != str(self.gateway.path)
                or getattr(config, "init_deadline", None) != self._startup_timeout
                or getattr(config, "persistent_state", None) != str(self.root / ".mobkit/state")
                or type(getattr(config, "prepared_application_bundle", None)) is not bytes
                or config.prepared_application_bundle != self._setup_bytes
                or getattr(config, "prepared_application_digest", None) != self.setup.sha256
                or any(getattr(config, key, None) is not None for key in
                    ("mob_config_path", "mob_config_inline", "application_tool_policies",
                     "declare_spec_update_expected_revision"))):
            raise CustodyError("The restart changed its selected carrier, gateway or startup configuration.")

    async def connect(self, config, application):
        """Restore only the retained native selection in this exact runtime."""
        if self._entered or self.callbacks is None:
            raise CustodyError("Installed restart connection is unselected or already entered.")
        self._entered = True
        self._require_current()
        self._check_config(config, application)
        await _joined(self._read_inputs)
        self._require_current()
        self._check_config(config, application)
        from meerkat_toolkit.host_control import (
            HostBootstrapSelection, SelectedHostArtifact, ToolkitMobKitRuntime,
        )
        selection = HostBootstrapSelection(deployment_root=self._selected["deployment_root"],
            **{name: SelectedHostArtifact(**self._selected[name]) for name in
                ("activation", "policy_manifest", "local_policy")})
        runtime = ToolkitMobKitRuntime(config, selection, **member_mcp_runtime_options(application),
            **_hosted_mcp_runtime_options(self))
        self.runtime = runtime
        application.context.runtime = runtime
        try:
            application.bind_schedule_runnables(runtime)
            await runtime.connect()
            self._require_current()
            _require_model_builder(self, config)
            await _joined(self._read_inputs)
            self._require_current()
            _require_model_builder(self, config)
            from .connect_member_class_requests import bind_member_class_requests
            bind_member_class_requests(self, application, runtime)
            bind_native_operations(self, application, runtime)
            return runtime
        except BaseException as original_failure:
            try:
                await self.shutdown(runtime)
            except BaseException as cleanup_error:
                if original_failure is not cleanup_error:
                    raise BaseExceptionGroup("Restore connection and original cleanup failed.",
                        [original_failure, cleanup_error]) from None
                raise
            application.context.runtime = None
            raise

    async def shutdown(self, runtime):
        """Join the same restored runtime before the project lease exits."""
        if runtime is not self.runtime or runtime is None:
            raise CustodyError("Installed restart can only close its own retained runtime.")
        return await _owned_shutdown(self, runtime)
