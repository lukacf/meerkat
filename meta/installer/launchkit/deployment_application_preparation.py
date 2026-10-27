"""Private stopped-owner bridge from genuine inventory to retained Fresh inputs.

This bridge drafts and pins original inputs only. The existing Fresh startup
owns preparation, finalization and activation. Native inventory stays inside
the original project lease and is never returned to a detached caller.
"""
from __future__ import annotations

import asyncio
from contextlib import nullcontext
from copy import deepcopy
import inspect
import json
import math
from pathlib import Path
import sys
import uuid

from .connect_accepted_source import retain_accepted_application_source
from .connect_application_finalizer import _write_bytes
from .connect_installer_caller import (CustodyError, ExactFile, NativeInstallerCaller,
    ProductGatewayFile, _json, _path, _private_parent, _same_json_value, supervisor_executor)
from .deployment_application_bootstrap import (_fresh_paths, _observe_inputs,
    require_bootstrap_history_settled)
from .deployment_supervisor import pending_custody
from .deployment_updates import checked_package, require_replacement_history_settled
from .forge_application_host import ForgeApplicationHost
from .projects import DraftConflict


async def _join(task):
    """Drain the original task without allowing repeated waiter cancellation."""
    cancelled = False
    while True:
        try:
            result = await asyncio.shield(task)
            return result, cancelled
        except asyncio.CancelledError:
            if task.cancelled():
                raise
            cancelled = True


class _PreparationCustody:
    """Retain existing handles, never project native lifecycle or readiness."""

    def __init__(self, service, guide_id, deployment_id, supervisor, context, lease):
        self.service, self.guide_id, self.deployment_id = service, guide_id, deployment_id
        self.supervisor, self.context, self.lease = supervisor, context, lease
        self.source = None
        self.owner_task, self.active = asyncio.current_task(), True
        self.child, self.drain_tasks, self.cleanup_task = None, (), None
        self.cleanup_entered = False
        self.admission = None
        self.original_cleanup = supervisor.cleanup_child
        self.cleanup_attribute = supervisor.__dict__.get("cleanup_child")
        self.had_cleanup_attribute = "cleanup_child" in supervisor.__dict__

        async def retained_cleanup(process, tasks=(), **kwargs):
            if self.child is not None and self.child is not process:
                raise CustodyError("Preparation cleanup cannot replace its original child.")
            if self.cleanup_task is not None and not self.cleanup_task.done():
                raise CustodyError("The original preparation cleanup must settle before another teardown.")
            supplied = tuple(tasks)
            if self.cleanup_entered:
                if supplied and supplied != self.drain_tasks:
                    raise CustodyError("Preparation cleanup cannot replace its original drain tasks.")
            else:
                self.child, self.drain_tasks, self.cleanup_entered = process, supplied, True
            self.cleanup_task = asyncio.create_task(self.original_cleanup(process, self.drain_tasks, **kwargs))
            result, cancelled = await _join(self.cleanup_task)
            if cancelled:
                self.supervisor.stop.set()
                raise asyncio.CancelledError
            return result

        self.cleanup_wrapper = retained_cleanup
        supervisor.cleanup_child = retained_cleanup

    async def settle_child(self):
        """Cleanup ignores changed source, but never adopts a replacement child."""
        from .deployment import require_owned_group_absent
        if self.supervisor.cleanup_child is not self.cleanup_wrapper:
            raise CustodyError("The original preparation cleanup callback changed.")
        current = self.supervisor.child
        if self.child is None:
            self.child = current
        if current is not None and current is not self.child:
            raise CustodyError("The original preparation child changed; no replacement was signalled.")
        admission = getattr(self.supervisor, "_child_admission", None)
        if self.admission is not None and admission is not None and admission is not self.admission:
            raise CustodyError("The original preparation admission changed.")
        self.admission = admission
        if self.cleanup_task is not None and not self.cleanup_task.done():
            await _join(self.cleanup_task)
        # A completed failed teardown is retried only with its original handles.
        if self.supervisor.child is not None:
            self.cleanup_task = asyncio.create_task(self.original_cleanup(self.child, self.drain_tasks))
            await _join(self.cleanup_task)
        if (self.supervisor.child is not None or getattr(self.supervisor, "_child_admission", None) is not None
                or any(not task.done() for task in self.drain_tasks)
                or getattr(self.supervisor, "_selected_sdk_install_task", None) is not None
                    and not self.supervisor._selected_sdk_install_task.done()):
            raise CustodyError("The original preparation child or joined work remains unsettled.")
        if self.child is not None:
            await require_owned_group_absent(self.child)

    async def settle_command(self):
        """Release only terminal helper handles before this owner's next command."""
        def current():
            if (asyncio.current_task() is not self.owner_task or not self.active
                    or self.service._application_preparations.get(self.deployment_id) is not self
                    or self.supervisor.cleanup_child is not self.cleanup_wrapper
                    or self.supervisor.child is not None):
                raise CustodyError("The original preparation command has not returned to its owning task.")
            self.lease.require_current()
        current()
        if self.cleanup_task is not None:
            # Retrieve this command's terminal failure or cancellation before
            # admitting another helper. Explicit cleanup keeps its old proof.
            _, cancelled = await _join(self.cleanup_task)
            if cancelled:
                self.supervisor.stop.set()
            current()
        await self.settle_child()
        current()
        if self.supervisor.stop.is_set():
            raise asyncio.CancelledError
        # settle_child proved process/group absence and every drain. No entered
        # child or uncertain cleanup is discarded by this sequential transition.
        self.child, self.admission, self.drain_tasks = None, None, ()
        self.cleanup_task, self.cleanup_entered = None, False

    async def settle(self):
        await self.settle_child()
        if self.source is not None:
            self.source.close()
        self.context.__exit__(None, None, None)
        if self.had_cleanup_attribute:
            self.supervisor.cleanup_child = self.cleanup_attribute
        else:
            del self.supervisor.cleanup_child
        self.service._application_preparations.pop(self.deployment_id)


async def cleanup_application_preparation(service, guide_id, deployment_id, *, supervisor):
    """Continue one retained teardown, with no selection, signal or replay grant."""
    custody = service._application_preparations.get(deployment_id)
    if (custody is None or custody.guide_id != guide_id or custody.supervisor is not supervisor
            or custody.active):
        raise CustodyError("Cleanup needs this exact inactive preparation owner and retained Supervisor.")
    supervisor.stop.set()
    _, cancelled = await _join(asyncio.create_task(custody.settle()))
    if cancelled:
        raise asyncio.CancelledError


async def shutdown_preparations(service):
    failures = []
    for custody in tuple(service._application_preparations.values()):
        try:
            await cleanup_application_preparation(service, custody.guide_id,
                custody.deployment_id, supervisor=custody.supervisor)
        except BaseException as failure:
            failures.append(failure)
    if len(failures) == 1:
        raise failures[0]
    if failures:
        raise BaseExceptionGroup("Original preparation cleanup remains unsettled.", failures)


def _synchronous(callback):
    call = getattr(callback, "__call__", None)
    if (not callable(callback) or inspect.iscoroutinefunction(callback) or inspect.isasyncgenfunction(callback)
            or inspect.iscoroutinefunction(call) or inspect.isasyncgenfunction(call)):
        raise CustodyError("Original preparation declarations need a synchronous trusted owner callback.")
    try:
        inspect.signature(callback).bind(object())
    except (TypeError, ValueError) as error:
        raise CustodyError("The trusted declarations callback must accept its original inventory.") from error


async def prepare_application(service, guide_id, deployment_id, *, application_host,
        supervisor, declarations, inventory_attempt_path, preparation_plan_path,
        startup_selection_path, activation_template, fresh_paths, command_timeout,
        model_configuration=None):
    from .deployment import (await_validation, private_configuration_snapshot,
        product_runtime_source)
    from .deployment_worker import Supervisor
    from .templates.storage_setup import project_lock
    from .product_runtime_custody import (await_product_validation, require_product_source,
        require_product_installation)
    from .connect_application_startup import _ModelConfiguration, _managed_model_classes

    _synchronous(declarations)
    if (type(application_host) is not ForgeApplicationHost or not application_host.applies(guide_id)
            or not isinstance(supervisor, Supervisor) or not isinstance(activation_template, ExactFile)
            or not isinstance(fresh_paths, dict)
            or isinstance(command_timeout, bool) or not isinstance(command_timeout, (int, float))
            or not math.isfinite(command_timeout) or command_timeout <= 0):
        raise CustodyError("Preparation needs this original host, Supervisor and explicit private input selection.")
    selected_model = None if model_configuration is None else _ModelConfiguration.observe(model_configuration)
    if selected_model is not None and sys.platform != "linux":
        raise CustodyError("Managed preparation requires its original Linux credential-root selection.")
    if (_managed_model_classes({"classes": application_host._blueprint["agent_classes"]})
            and selected_model is None):
        raise CustodyError("Managed-host models require the original trusted preparation model configuration.")

    def model_current(*, filesystem=False):
        original = None if selected_model is None else selected_model.selection()
        if not _same_json_value(model_configuration, original):
            raise CustodyError("The original preparation model configuration changed.")
        if selected_model is not None:
            if filesystem:
                selected_model.require_filesystem_current()
            else:
                selected_model.require_current()

    model_current()
    if deployment_id in service._application_preparations:
        raise CustodyError("Settle the original preparation custody before any new preparation.")
    directory, receipt, stopped = service._stopped_application_owner(guide_id, deployment_id)
    from .deployment_application_bootstrap import same_installation_apart_from_staged_update
    # The install-time Supervisor's receipt predates an update staged after
    # Install; that pointer alone does not change the installation it owns.
    if (supervisor.directory != directory or supervisor.destination != Path(receipt["destination"])
            or not same_installation_apart_from_staged_update(service._configuration_binding(supervisor.receipt),
                service._configuration_binding(receipt), receipt)):
        raise CustodyError("Preparation cannot substitute a Supervisor from another installation.")
    supervisor.require_child_slot()
    if getattr(supervisor, "_selected_sdk_install_task", None) is not None:
        raise CustodyError("Preparation cannot adopt another entered SDK installation worker.")
    application_host.require_current()
    source_custody = application_host._product_source_custody
    if getattr(supervisor, "_product_source_custody", None) is not source_custody:
        raise CustodyError("Preparation cannot substitute another original product source owner.")
    if source_custody is not None:
        require_product_installation(source_custody, receipt)
    if await await_product_validation(source_custody, pending_custody, directory, deepcopy(receipt)):
        raise DraftConflict("Complete the original installation support custody before preparation.")
    model_current()
    stopped()
    snapshot = await await_product_validation(source_custody, service._installed_configuration, guide_id, deployment_id,
        **({"source_custody": source_custody} if source_custody is not None else {}))
    model_current()
    stopped()
    destination = snapshot[2]
    context = (project_lock(destination) if source_custody is None else
        nullcontext(require_product_source(source_custody,
            application_host._target, destination=destination)._lease))
    # The relocated path borrows the actual externally held lease. Its trusted
    # supplier must retain that source scope through original child cleanup,
    # including retained uncertain custody; this context cannot replace it.
    lease = context.__enter__()
    custody = _PreparationCustody(service, guide_id, deployment_id, supervisor, context, lease)
    service._application_preparations[deployment_id] = custody

    def owner_current():
        stopped()
        lease.require_current()
        application_host.require_current()
        model_current()
        if supervisor.stop.is_set():
            raise asyncio.CancelledError

    try:
        if snapshot[3].get("knowledge", {}).get("mode") == "local":
            # Configure has already retained the original provider keys. Prepare
            # owns the checked helper and its child cleanup before freezing .env
            # or entering native inventory. Byte Install remains credential-free.
            owner_current()
            await supervisor.prepare_knowledge(snapshot[3], require_current=owner_current,
                project_lease=lease, source_custody=source_custody,
                settle_command=custody.settle_command)
            owner_current()
        selected_target = application_host.initial_build_options(guide_id)["runtime_target"]
        def inspect_source():
            lease.require_current()
            require_bootstrap_history_settled(directory)
            require_replacement_history_settled(directory)
            if pending_custody(directory, deepcopy(receipt)):
                raise CustodyError("The original installation support custody is still pending.")
            _, package = checked_package(directory, "package.zip", receipt["package_sha256"])
            artifacts = product_runtime_source(package, receipt.get("runtime_candidate"),
                **({"source_custody": source_custody} if source_custody is not None else {}))
            if source_custody is None and artifacts.selection != selected_target:
                raise CustodyError("The original host differs from the installed selected product.")
            if (not _same_json_value(_json(package["blueprint.json"][0]), application_host._blueprint)
                    or package["config/mob.toml"][0] != application_host._definition.encode("utf-8")
                    or any(pin.read() != package[name][0] for name, pin in application_host._protected)):
                raise CustodyError("The installed package differs from the original host's exact selected source.")
            gateway_path = destination / ".bin" / artifacts.prefix / "bin/toolkit_gateway"
            gateway = ProductGatewayFile.observe(ExactFile(gateway_path,
                artifacts.candidate["binary"]["sha256"]), artifacts)
            paths = _fresh_paths(fresh_paths, destination)
            extra = {name: _path(value, fresh=True) for name, value in {
                "inventory": inventory_attempt_path, "plan": preparation_plan_path,
                "selection": startup_selection_path}.items()}
            reserved = [*paths.values(), *extra.values(),
                Path(str(extra["inventory"]) + ".outcome.json"),
                Path(str(paths["provision_attempt_path"]) + ".outcome.json"),
                destination / (paths["activation_path"].name + ".current.next"),
                *(paths["working_directory"] / name for name in
                    ("finalization.json", "prepare.json", "validate.json", "publication.json"))]
            if (len(set(reserved)) != len(reserved)
                    or any(destination not in path.parents for path in extra.values())
                    or activation_template.path in reserved
                    or any(pin.path in reserved for pin in application_host._pins)):
                raise CustodyError("Preparation needs separate original private inputs and attempt paths.")
            for path in extra.values():
                _private_parent(path)
            template = activation_template.read()
            configuration = private_configuration_snapshot(destination)[1]
            return package, gateway, paths, extra, template, configuration, set(reserved)

        # Keep the trusted source callback on its owning loop.
        package, gateway, paths, extra, template, configuration, reserved = await await_product_validation(
            source_custody, inspect_source)
        owner_current()
        custody.source = await retain_accepted_application_source(directory, receipt,
            project_lease=lease, require_owner_current=owner_current,
            custody_directory=directory / ("application-preparation-source-" + str(uuid.uuid4())))
        source = custody.source
        policy = dict(application_host._protected)[application_host._policy_source_path]
        if policy.read() != package[application_host._policy_source_path][0]:
            raise CustodyError("Preparation lost the exact accepted original source policy.")

        def files_current():
            model_current(filesystem=True)
            if source_custody is not None:
                source_custody.require_filesystem_current()
            source.require_filesystem_current()
            gateway.require_current()
            require_bootstrap_history_settled(directory)
            require_replacement_history_settled(directory)
            if pending_custody(directory, deepcopy(receipt)):
                raise CustodyError("Original installation support custody changed during preparation.")
            for pin in application_host._pins:
                pin.read()
            if (activation_template.read() != template
                    or private_configuration_snapshot(destination)[1] != configuration):
                raise CustodyError("Original private settings or activation template changed during preparation.")

        caller = NativeInstallerCaller(gateway,
            execute=supervisor_executor(supervisor, timeout=command_timeout),
            require_current=owner_current, require_filesystem_current=files_current,
            provision_attempt_path=paths["provision_attempt_path"])
        source_files = tuple(source.exact_file(name) for name in (
            "config/mob.toml", application_host._policy_source_path, "application/configuration.json",
            "application/console.json", "config/access.toml"))
        outcome = await caller.lower_application_inventory(application_host._inventory_pin,
            attempt_path=extra["inventory"], source_files=source_files)
        owner_current()
        if outcome.status != "receipt_verified" or outcome.inventory is None:
            raise CustodyError("The original native inventory is unconfirmed; no Fresh inputs were selected.")
        chosen = declarations(outcome.inventory)
        if inspect.isawaitable(chosen) or inspect.isasyncgen(chosen):
            if inspect.iscoroutine(chosen):
                chosen.close()
            raise CustodyError("The trusted declarations callback must return its explicit inputs synchronously.")
        if not isinstance(chosen, dict):
            raise CustodyError("Preparation needs the original owner's explicit declarations.")
        chosen = deepcopy(chosen)
        output = _path(chosen.get("output"), fresh=True)
        if (destination not in output.parents or output in reserved
                or output == activation_template.path or output == gateway.path
                or output in {pin.path for pin in application_host._pins}):
            raise CustodyError("Preparation output must preserve separate original project inputs.")
        _private_parent(output)
        plan = application_host.preparation_plan(outcome.inventory, **chosen)
        from .deployment_application_bootstrap import _plan_source
        await await_product_validation(source_custody, _plan_source, _json(plan), package, destination)
        owner_current()

        async def write(path, content):
            owner_current()
            def commit():
                if supervisor.stop.is_set():
                    raise CustodyError("The original preparation owner stopped before private publication.")
                files_current()
                if supervisor.stop.is_set():
                    raise CustodyError("The original preparation owner stopped before private publication.")
                pin = _write_bytes(path, content)
                files_current()
                return pin
            task = asyncio.create_task(asyncio.to_thread(commit))
            try:
                if source_custody is not None:
                    while not task.done():
                        owner_current()
                        await asyncio.wait((task,), timeout=0.05)
                pin = await asyncio.shield(task)
            except asyncio.CancelledError as cancelled:
                supervisor.stop.set()
                try:
                    await _join(task)
                except Exception as failure:
                    # Keep the drained worker's refusal or failure with the
                    # original waiter cancellation, rather than discarding it.
                    raise cancelled from failure
                raise
            except BaseException:
                supervisor.stop.set()
                await _join(task)
                raise
            owner_current()
            return pin

        plan_pin = await write(extra["plan"], plan)
        selection = {"kind": "fresh", "gateway": gateway.pin(), "preparation_plan": plan_pin.pin(),
            "source_policy": policy.pin(), "activation_template": activation_template.pin(),
            **{name: str(path) for name, path in paths.items()}}
        if selected_model is not None:
            selection["model_configuration"] = selected_model.selection()
        await await_product_validation(source_custody, _observe_inputs, directory, receipt, selection,
            **({"source_custody": source_custody} if source_custody is not None else {}))
        owner_current()
        # A supplier may already know this path. Do not make its envelope
        # visible until the actual child, group and drain tasks have settled.
        _, cancelled = await _join(asyncio.create_task(custody.settle_child()))
        if cancelled:
            supervisor.stop.set()
            raise asyncio.CancelledError
        owner_current()
        result = await write(extra["selection"],
            (json.dumps(selection, ensure_ascii=True, allow_nan=False, separators=(",", ":")) + "\n").encode())
        await await_product_validation(source_custody, files_current)
        owner_current()
        return result
    except asyncio.CancelledError:
        supervisor.stop.set()
        raise
    finally:
        custody.active = False
        # This join has no source-currentness requirement: lost source does not
        # authorize a replacement signal or prevent the original child's cleanup.
        _, cancelled = await _join(asyncio.create_task(custody.settle()))
        if cancelled:
            supervisor.stop.set()
            raise asyncio.CancelledError
