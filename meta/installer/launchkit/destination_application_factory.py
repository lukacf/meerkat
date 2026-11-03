"""Trusted destination host composition under original operation leases.

The durable destination worker supplies its existing private source selections
and preparation callbacks. This module does not construct native authority,
select a cloud account, receive browser paths or qualify a runtime. Public
remote product admission and the Linux source intake contract remain unchanged.
"""
from __future__ import annotations

import asyncio
from contextlib import AsyncExitStack, asynccontextmanager
from copy import deepcopy
import inspect
from pathlib import Path

from .connect_installer_caller import CustodyError, ExactFile, _json, _same_json_value
from .deployment import DeploymentService
from .forge_application_host import ForgeApplicationHost
from .remote_product_destination_owner import _context, open_stopped_product_source
from .templates.storage_setup import project_lock


class DestinationApplicationFactory:
    """One original worker's concrete host, without retaining a stale lease.

    The caller injects original native-input suppliers with the existing host
    signatures. Their private choices, principal and completion evidence remain
    their own authority. No default native constructor or source reconstruction
    is inferred from the public staged package.
    """

    def __init__(self, *, deployment, context, owner_source, acquisition_selection,
            viewer_selection, require_current, startup_selection, preparation_inputs,
            retain_preparation_inputs, startup_continuation=None):
        if (not isinstance(deployment, DeploymentService)
                or type(acquisition_selection) is not ExactFile
                or type(viewer_selection) is not ExactFile
                or not callable(require_current)):
            raise CustodyError("Destination composition needs its actual deployment owner and private source pins.")
        selected = _context(context, owner_source)
        expected = Path(selected["host"]["home"]) / ".local/state/meta-forge/deployments"
        if deployment.root != expected:
            raise CustodyError("Destination composition differs from its original canonical deployment owner.")
        self.deployment, self.context = deployment, deepcopy(selected)
        self.owner_source = owner_source
        self.acquisition_selection, self.viewer_selection = acquisition_selection, viewer_selection
        self._owner_current = require_current
        self._startup_selection, self._preparation_inputs = startup_selection, preparation_inputs
        self._retain_preparation_inputs, self._startup_continuation = retain_preparation_inputs, startup_continuation
        self._configured = (deployment, deepcopy(selected), owner_source, acquisition_selection,
            viewer_selection, require_current, startup_selection, preparation_inputs,
            retain_preparation_inputs, startup_continuation)
        self._pin_bytes = (acquisition_selection.read(), viewer_selection.read())
        self.host = self._source = self._pending_scope = self._pending_task = None
        self._startup_pin = None
        self._install_entered = False
        self._current()

    @property
    def guide_id(self):
        return self.context["guide_id"]

    @property
    def deployment_id(self):
        return self.context["deployment_id"]

    @property
    def directory(self):
        return self.deployment.root / self.guide_id / self.deployment_id

    def _current(self):
        actual = (self.deployment, self.context, self.owner_source, self.acquisition_selection,
            self.viewer_selection, self._owner_current, self._startup_selection,
            self._preparation_inputs, self._retain_preparation_inputs, self._startup_continuation)
        if actual != self._configured:
            raise CustodyError("Preserve the original destination factory selections and callbacks.")
        result = self._owner_current()
        if inspect.isawaitable(result) or inspect.isasyncgen(result):
            if inspect.iscoroutine(result):
                result.close()
            raise CustodyError("Destination owner currentness must complete synchronously.")
        if result is False:
            raise CustodyError("The original destination worker owner changed.")
        if (self.acquisition_selection.read(), self.viewer_selection.read()) != self._pin_bytes:
            raise CustodyError("The original destination private source selections changed.")

    def _identity(self, deployment, guide_id, deployment_id):
        self._current()
        if (deployment is not self.deployment or guide_id != self.guide_id
                or deployment_id != self.deployment_id or not deployment.lock.locked()):
            raise CustodyError("Destination setup requires its exact locked original installation action.")
        self._callbacks_current()

    def bind(self):
        """Bind once in the trusted durable worker, never through HTTP selection."""
        self._current()
        rows = self._callbacks()
        if any(getattr(self.deployment, name, None) not in (None, callback)
                for name, callback in rows):
            raise CustodyError("The original destination application callbacks are already selected.")
        for name, callback in rows:
            setattr(self.deployment, name, callback)

    def _callbacks(self):
        return (("application_setup_selection", self._selected_startup),
            ("application_setup_preparation", self.prepare_application),
            ("application_setup_continuation", self.continue_application_startup),
            ("application_setup_start", self.start_application),
            ("application_host_selection", self.selected_host))

    def _callbacks_current(self):
        if any(getattr(self.deployment, name, None) != callback for name, callback in self._callbacks()):
            raise CustodyError("The original destination worker application callbacks changed.")

    def selected_host(self, guide_id, deployment_id):
        self._current()
        if guide_id != self.guide_id or deployment_id != self.deployment_id:
            return None
        return self.host

    def _view(self):
        if self._startup_pin is None:
            return None
        return _json(self._startup_pin.read())

    def _retain_startup_pin(self):
        try:
            selected = self._startup_selection(self.guide_id, self.deployment_id)
        except FileNotFoundError:
            return None
        if selected is None:
            return None
        pin, value = self.host._selected_startup_observation(
            self.guide_id, self.deployment_id, selected=selected)
        self._startup_pin = pin
        return value

    def _settled(self):
        if self.host is None:
            return
        installer = self.host._destination_installation_supervisor
        if installer is not None:
            cleanup = installer._destination_install_cleanup
            if cleanup.cleanup_task is not None and cleanup.cleanup_task.done():
                if cleanup.cleanup_task.cancelled():
                    raise CustodyError("The original destination installer cleanup was cancelled.")
                error = cleanup.cleanup_task.exception()
                if error is not None:
                    raise error
            if (installer.child is not None or installer._child_admission is not None
                    or installer._selected_sdk_install_task is not None
                    or any(not task.done() for task in cleanup.drain_tasks)
                    or cleanup.cleanup_task is not None and not cleanup.cleanup_task.done()):
                raise CustodyError("The original destination installer cleanup remains unsettled.")
        supervisor = self.host._installation_supervisor
        if supervisor is not None:
            if (supervisor.child is not None or supervisor._child_admission is not None
                    or supervisor._selected_sdk_install_task is not None):
                raise CustodyError("The original destination preparation child or byte worker remains entered.")
        if self.deployment_id in self.deployment._application_preparations:
            raise CustodyError("The original destination native preparation cleanup remains entered.")

    @asynccontextmanager
    async def _operation(self, *, installed):
        if self._source is not None or self._pending_scope is not None:
            raise CustodyError("Settle the original destination source operation before another admission.")
        self._current()
        task, loop = asyncio.current_task(), asyncio.get_running_loop()
        stack = AsyncExitStack()
        stopped = None
        original_error = None

        def current():
            if asyncio.get_running_loop() is not loop or asyncio.current_task() is not task:
                raise CustodyError("Destination source ownership must stay on its original worker task.")
            self._current()
            self._callbacks_current()
            if stopped is not None:
                stopped()

        try:
            lease = stack.enter_context(project_lock(Path(self.context["host"]["destination"])))
            if installed:
                from .product_installed_source import open_installed_product_source
                _, _, stopped = self.deployment._stopped_application_owner(self.guide_id, self.deployment_id)
                stopped()
                source = await stack.enter_async_context(open_installed_product_source(self.directory,
                    project_lease=lease, require_current=current, owner_source=self.owner_source,
                    application_startup=self._view()))
            else:
                source = stack.enter_context(open_stopped_product_source(self.context,
                    project_lease=lease, require_current=current, owner_source=self.owner_source))
            if source is None or not _same_json_value(source._context, self.context):
                raise CustodyError("Destination operation lost its exact original selected source.")
            self._source = source
            if self.host is None:
                self.host = ForgeApplicationHost(guide_id=self.guide_id, runtime_target=source.origin,
                    acquisition_selection=self.acquisition_selection, viewer_selection=self.viewer_selection,
                    require_current=self._current, source_custody=source,
                    startup_selection=self._startup_selection, preparation_inputs=self._preparation_inputs,
                    retain_preparation_inputs=self._retain_preparation_inputs,
                    startup_continuation=self._startup_continuation)
            else:
                self.host._bind_destination_source(source, factory=self)
            yield self.host
        except BaseException as error:
            original_error = error
            raise
        finally:
            try:
                self._settled()
            except BaseException as error:
                # Keep the actual entered scope and lease with the original
                # worker. Another operation cannot replace uncertain custody.
                self._pending_scope, self._pending_task = stack, task
                if original_error is not None and original_error is not error:
                    raise BaseExceptionGroup("Destination operation and its retained cleanup remain unsettled.",
                        [original_error, error]) from None
                raise
            else:
                try:
                    if original_error is None:
                        await stack.aclose()
                    else:
                        # Pass the original refusal through entered source
                        # contexts, preserving their exceptional-exit behavior.
                        await stack.__aexit__(type(original_error), original_error,
                            original_error.__traceback__)
                except BaseException as error:
                    if original_error is not None and original_error is not error:
                        raise BaseExceptionGroup("Destination operation and source closure retained their failures.",
                            [original_error, error]) from None
                    raise
                finally:
                    self._source = None

    async def install(self, *, settings):
        """Run one original byte Install, retaining genuine private setup inputs."""
        self._current()
        if self._install_entered:
            raise CustodyError("The original destination Install is retained; automatic retry is unavailable.")
        self._callbacks_current()
        async with self.deployment.lock:
            async with self._operation(installed=False) as host:
                self._install_entered = True
                supervisor = await host.install_staged_source(settings=settings, require_current=self._current)
                host.retain_installation_inputs(self.deployment, self.guide_id, self.deployment_id)
                self._retain_startup_pin()
                return supervisor

    async def cleanup_installation(self):
        """Join the retained byte installer, without another Install or source."""
        if (self._pending_scope is None or asyncio.current_task() is not self._pending_task
                or self.host is None or self.host._destination_installation_supervisor is None):
            raise CustodyError("Continue only this worker's original uncertain destination installer.")
        await self.host.cleanup_staged_installation(supervisor=self.host._destination_installation_supervisor)
        await self._release_pending()

    async def cleanup_preparation(self):
        """Join only the original preparation's retained child and source lease."""
        if (self._pending_scope is None or asyncio.current_task() is not self._pending_task
                or self.host is None or self.host._installation_supervisor is None
                or self.deployment_id not in self.deployment._application_preparations):
            raise CustodyError("Continue only this worker's original uncertain destination preparation.")
        await self.deployment.cleanup_application_preparation(self.guide_id, self.deployment_id,
            supervisor=self.host._installation_supervisor)
        await self._release_pending()

    async def _release_pending(self):
        self._settled()
        stack = self._pending_scope
        try:
            await stack.aclose()
        finally:
            self._pending_scope = self._pending_task = self._source = None

    async def prepare_application(self, deployment, guide_id, deployment_id):
        self._identity(deployment, guide_id, deployment_id)
        async with self._operation(installed=True) as host:
            result = await host.prepare_application(deployment, guide_id, deployment_id)
            self._retain_startup_pin()
            return result

    async def continue_application_startup(self, deployment, guide_id, deployment_id):
        self._identity(deployment, guide_id, deployment_id)
        async with self._operation(installed=True) as host:
            await host.continue_application_startup(deployment, guide_id, deployment_id)
            self._retain_startup_pin()

    def _selected_startup(self, guide_id, deployment_id):
        self._current()
        self._callbacks_current()
        if guide_id != self.guide_id or deployment_id != self.deployment_id or self.host is None:
            raise CustodyError("Select only this destination owner's original startup inputs.")
        if self._source is not None and not self._source._closed:
            return self.host._selected_startup(guide_id, deployment_id)
        # The entered operation authenticated this exact private pin. Its bytes
        # remain input data after that scope closes, never a renewed source
        # lease or native completion. The next admission opens its own source.
        pin = self._startup_pin
        if type(pin) is not ExactFile:
            raise CustodyError("The original destination owner has not retained its private startup pin.")
        content = pin.read()
        value = _json(content)
        from .deployment_application_bootstrap import _shape
        _shape(value)
        for original in self.host._pins:
            original.read()
        self._current()
        self._callbacks_current()
        if self._startup_pin is not pin or pin.read() != content:
            raise CustodyError("The original destination private startup pin changed during selection.")
        return deepcopy(value)

    async def start_application(self, deployment, guide_id, deployment_id):
        """Select on the owner task, then close before Start's actual own lease."""
        self._identity(deployment, guide_id, deployment_id)
        async with self._operation(installed=True) as host:
            pin, selected = host._selected_startup_observation(guide_id, deployment_id)
            owner_source = self._source._owner_source
        self._identity(deployment, guide_id, deployment_id)
        if _json(pin.read()) != selected:
            raise CustodyError("The original private startup pin changed while closing destination source custody.")
        return await deployment._start_application_locked(guide_id, deployment_id,
            application_startup=selected, owner_source=owner_source, destination_owner=self)

    async def status(self, deployment, guide_id, deployment_id):
        """Observe this guest's original installation, never origin design truth."""
        if deployment is not self.deployment:
            raise CustodyError("Destination status belongs to another deployment owner.")
        async with deployment.lock:
            self._identity(deployment, guide_id, deployment_id)
            from .deployment import read_receipt
            receipt = read_receipt(self.directory)
            stopped = (receipt.get("status") == "stopped"
                and receipt.get("execution_mode", "interactive") == "interactive" and not receipt.get("service"))
            if stopped:
                async with self._operation(installed=True):
                    return await deployment.status(guide_id, _joined_file_checks=True, destination_owner=self)
            # A live child owns its own project lease. Status does not reopen
            # parent custody or use the host's closed staging carrier.
            return await deployment.status(guide_id, _joined_file_checks=True, destination_owner=self)

    async def action(self, deployment, guide_id, deployment_id, action, *, action_request_id):
        """Use the original canonical UUID journal and selected lifecycle callbacks."""
        if deployment is not self.deployment or guide_id != self.guide_id or deployment_id != self.deployment_id:
            raise CustodyError("Destination actions belong to another installed owner.")
        self._current()
        self._callbacks_current()
        return await deployment.action(guide_id, deployment_id, action,
            action_request_id=action_request_id, destination_owner=self)

    async def settings_options(self, deployment, guide_id, deployment_id, *, console_port=None):
        """Read only the original installed package through its genuine guest owner."""
        if deployment is not self.deployment:
            raise CustodyError("Destination settings belong to another deployment owner.")
        async with deployment.lock:
            self._identity(deployment, guide_id, deployment_id)
            async with self._operation(installed=True):
                return await deployment._settings_options_locked(guide_id, deployment_id,
                    console_port=console_port, project_lease=self._source._lease,
                    source_custody=self._source)


    async def configure(self, deployment, guide_id, deployment_id, *, action_request_id,
            version, build_id, source_digest, package_sha256, settings=None, remove_fields=None,
            use_host_provider_credentials=False):
        """One original UUID-owned stopped Configure, never setup/native readiness."""
        if deployment is not self.deployment:
            raise CustodyError("Destination Configure belongs to another deployment owner.")
        async with deployment.lock:
            self._identity(deployment, guide_id, deployment_id)
            deployment._directory(guide_id, deployment_id)
            if (type(version) is not int or version != self.context["guide_version"]
                    or build_id != self.context["build_id"] or source_digest != self.context["source_digest"]
                    or package_sha256 != self.context["package_sha256"]):
                raise CustodyError("Configure must select this original installed source and version.")
            binding = {"action_request_id": action_request_id, "guide_id": guide_id,
                "deployment_id": deployment_id, "action": "configure", "update_id": None,
                "guide_version": version, "build_id": build_id, "source_digest": source_digest,
                "package_sha256": package_sha256}
            value, claimed = deployment._claim_action_request(binding)
            if not claimed:
                from .deployment import read_receipt
                return {**read_receipt(self.directory), "action_request": value}
            try:
                async with self._operation(installed=True) as host:
                    result = await deployment._configure_locked(guide_id, version, deployment_id,
                        build_id, source_digest, package_sha256, settings, remove_fields,
                        use_host_provider_credentials,
                        application_settings_validator=host.validate_installed_settings,
                        project_lease=self._source._lease, source_custody=self._source)
                    self._identity(deployment, guide_id, deployment_id)
            except asyncio.CancelledError:
                # A lost reply never permits another write for this UUID.
                raise
            except Exception:
                deployment._settle_action_request(binding, "error")
                raise
            value = deployment._settle_action_request(binding, "returned")
            return {**result, "action_request": value}


    async def setup_status(self, deployment, guide_id, deployment_id):
        """Read the original installed inputs without retaining or preparing any."""
        if deployment is not self.deployment or guide_id != self.guide_id or deployment_id != self.deployment_id:
            raise CustodyError("Destination setup status belongs to another installed owner.")
        async with deployment.lock:
            self._callbacks_current()
            async with self._operation(installed=True) as host:
                return await host._setup_status_locked(deployment, guide_id, deployment_id,
                    source_custody=self._source)
