"""Private initial installation under an actual staged destination owner.

The trusted host supplies its real acquisition, viewer and private settings
selection. Source staging alone cannot supply those selections or native inputs.
The caller keeps the original source context and project lease entered through
the returned installer's cleanup, including uncertainty. A durable receipt does
not make that in-memory source carrier live after its original scope closes.
"""
from __future__ import annotations

import asyncio
import hashlib
import inspect
import json
import os
from pathlib import Path
import stat

from .connect_installer_caller import CustodyError, _json, _same_json_value
from .product_runtime_custody import (
    await_product_validation, product_source_custody, require_product_installation,
    require_product_source)


class _DestinationInstallCleanup:
    """Keep the same installer's actual child, output and cleanup ownership."""

    def __init__(self, supervisor, source):
        self.supervisor, self.source = supervisor, source
        self.original_cleanup = supervisor.cleanup_child
        self.child, self.drain_tasks, self.cleanup_task = None, (), None
        self.wrapper = self.cleanup
        supervisor.cleanup_child = self.wrapper
        supervisor._destination_install_cleanup = self

    async def _join(self):
        interrupted = None
        while True:
            try:
                result = await asyncio.shield(self.cleanup_task)
                if interrupted is not None:
                    raise interrupted
                return result
            except asyncio.CancelledError as error:
                self.supervisor.stop.set()
                if self.cleanup_task.cancelled():
                    if interrupted is not None:
                        raise BaseExceptionGroup("Destination installer cleanup and its waiter were cancelled.",
                            [interrupted, error]) from None
                    raise
                if self.cleanup_task.done():
                    raise
                interrupted = interrupted or error
            except BaseException as error:
                if interrupted is not None:
                    raise BaseExceptionGroup("Destination installer cleanup and its waiter failed.",
                        [interrupted, error]) from None
                raise

    def require_settled(self):
        self.supervisor.require_child_slot()
        if (getattr(self.supervisor, "_selected_sdk_install_task", None) is not None
                or self.cleanup_task is not None and not self.cleanup_task.done()
                or any(not task.done() for task in self.drain_tasks)):
            raise CustodyError("The original destination installer child or joined output remains unsettled.")
        if self.cleanup_task is not None:
            if self.cleanup_task.cancelled():
                raise CustodyError("The original destination installer cleanup was cancelled.")
            failure = self.cleanup_task.exception()
            if failure is not None:
                raise failure

    async def _run_cleanup(self, process, tasks, kwargs):
        from .deployment import require_owned_group_absent
        result = await self.original_cleanup(process, tasks, **kwargs)
        await require_owned_group_absent(process)
        return result

    async def cleanup(self, process, tasks=(), **kwargs):
        supplied = tuple(tasks)
        if self.child is not None:
            if (self.cleanup_task is None or not self.cleanup_task.done()
                    or self.cleanup_task.cancelled()
                    or any(not task.done() for task in self.drain_tasks)
                    or self.child.returncode is None):
                raise CustodyError("The previous destination installer cleanup must settle before another child.")
            failure = self.cleanup_task.exception()
            if failure is not None:
                raise failure
        if self.supervisor.child is not process:
            raise CustodyError("Destination installer cleanup needs its original actual child.")
        self.child, self.drain_tasks = process, supplied
        if isinstance(kwargs.get("error"), asyncio.CancelledError):
            self.supervisor.stop.set()
        self.cleanup_task = asyncio.create_task(self._run_cleanup(process, supplied, kwargs))
        return await self._join()

    async def continue_cleanup(self):
        """Continue retained cleanup only; never rerun installation or select inputs.

        The trusted caller reaches this through its retained host installer after
        install raises, while keeping the same external source scope and lease.
        Changed source does not authorize another child or another install.
        """
        if self.source._closed:
            raise CustodyError("Retain the original source scope until destination installer cleanup settles.")
        self.source._lease.require_current()
        if self.supervisor.cleanup_child is not self.wrapper:
            raise CustodyError("The original destination installer cleanup callback changed.")
        if self.child is None:
            raise CustodyError("The destination installer has no original child cleanup to continue.")
        if self.supervisor.child is not None and self.supervisor.child is not self.child:
            raise CustodyError("The original destination installer child changed; no replacement was signalled.")
        self.supervisor.stop.set()
        if self.cleanup_task is not None and not self.cleanup_task.done():
            await self._join()
        elif self.supervisor.child is not None or any(not task.done() for task in self.drain_tasks):
            # A failed finished cleanup may continue only this same original
            # child and drain tuple, never an empty replacement tuple.
            self.cleanup_task = asyncio.create_task(self._run_cleanup(self.child, self.drain_tasks, {}))
            await self._join()
        if (self.supervisor.child is not None or getattr(self.supervisor, "_child_admission", None) is not None
                or getattr(self.supervisor, "_selected_sdk_install_task", None) is not None
                or any(not task.done() for task in self.drain_tasks)):
            raise CustodyError("The original destination installer cleanup remains unsettled.")
        from .deployment import require_owned_group_absent
        await require_owned_group_absent(self.child)
        self.source._lease.require_current()


def _current(callback):
    if not callable(callback):
        raise CustodyError("Destination installation needs its original current owner.")
    result = callback()
    if inspect.isawaitable(result):
        if inspect.iscoroutine(result):
            result.close()
        raise CustodyError("Destination installation currentness must remain synchronous.")
    if result is False:
        raise CustodyError("The original destination installation owner changed.")


def _inputs(source):
    """Read only the original full package under the existing file lane."""
    from .remote_product_destination_owner import MAX_APPLICATION_BYTES, _package_source
    from .runtime_candidates import _Inputs
    source.require_filesystem_current()
    held = _Inputs(source._store)
    _, payload = held.read(source._store / "package.zip", source.receipt["package_sha256"],
        MAX_APPLICATION_BYTES, size=source._context["package_size"])
    files, selection = _package_source(payload, source._context, source.origin)
    require_product_source(source, source.origin, destination=source._root, files=files)
    held.finish()
    source.require_filesystem_current()
    return payload, files, selection


def _directory(source):
    """Use the existing DeploymentService root and lookup shape, not intake state."""
    state = source._store.parent.parent
    expected = Path(source._context["host"]["home"]) / ".local/state/meta-forge"
    if state != expected or source._store != state / "product-sources" / source.receipt["deployment_id"]:
        raise CustodyError("The original source has no canonical Forge installation store.")
    return state, state / "deployments" / source.receipt["guide_id"] / source.receipt["deployment_id"]


def _private_directories(state, parent):
    from .connect_installer_caller import _private_directory
    _private_directory(state)
    current = state
    for name in parent.relative_to(state).parts:
        current /= name
        current.mkdir(mode=0o700, exist_ok=True)
        _private_directory(current)


def _fresh(directory, destination):
    from .deployment import OWNER_FILE
    paths = (directory, directory.parent / "latest.json", directory.parent / "latest.pending",
        destination / OWNER_FILE, (destination / OWNER_FILE).with_suffix(".pending"),
        *(destination / name for name in (".env", ".env.launch-pending", ".venv", ".bin", "data")))
    if any(os.path.lexists(path) for path in paths):
        raise CustodyError("The original destination installation already has owned or partial state. Preserve that attempt; no retry was admitted.")


def _host_source(host, source, files):
    from .artifact_binding import proposal_hash
    from .deployment_sources import read_package_source
    host.require_current()
    provenance = read_package_source(files)
    if (host.guide_id != source.receipt["guide_id"] or host._target != source.origin
            or not _same_json_value(_json(files["blueprint.json"][0]), host._blueprint)
            or files["config/mob.toml"][0] != host._definition.encode("utf-8")
            or any(pin.read() != files[name][0] for name, pin in host._protected)
            or provenance["guide_version"] != source._context["guide_version"]
            or provenance["build_id"] != source.receipt["build_id"]
            or provenance["source_digest"] != source.receipt["source_digest"]
            or provenance["blueprint_hash"] != proposal_hash(host._blueprint)):
        raise CustodyError("The staged application differs from this original host acquisition and viewer selection.")
    host.require_current()
    return provenance


async def install_staged_product_source(host, *, settings, require_current):
    """Publish one real queued install and run its original Supervisor to completion.

    This is called only by the actual trusted host method, never by an HTTP
    runtime/path pair. A future destination host supplier must forward its real
    source custody and private selections; the stock origin factory does not.
    No native preparation, Fresh/Restore inputs or installation service is
    selected here. The later genuine native supplier retains its own inputs.
    """
    from .forge_application_host import ForgeApplicationHost
    from .deployment import (OWNER_FILE, journal, new_installation_receipt, ownership,
        package_configuration_fields, package_console_scope, prepare_private_settings,
        product_runtime_source, read_receipt, write_private_settings, _serialized_private_configuration)
    from .deployment_ports import require_available_console_port
    from .deployment_worker import Supervisor
    from .runtime_candidates import _Inputs
    from .templates.storage_setup import write_private
    if (type(host) is not ForgeApplicationHost
            or any(not hasattr(host, name) for name in ("_product_source_custody", "_target",
                "_protected", "_blueprint", "_definition", "_pins", "_require_owner_current",
                "_destination_installation_entered", "_destination_installation_supervisor"))):
        raise CustodyError("Initial destination installation needs its actual trusted Forge application host.")
    source = product_source_custody(host._product_source_custody)
    if (host._destination_installation_entered
            or host._destination_installation_supervisor is not None):
        raise CustodyError("The original destination installer is already retained. Settle that exact attempt; no replacement was admitted.")

    def current():
        _current(require_current)
        source.require_owner_current()
        host.require_current()
        require_product_source(source, host._target, destination=source._root)
        source.require_owner_current()
        _current(require_current)

    current()
    payload, files, selected = await await_product_validation(source, _inputs, source)
    current()
    provenance = _host_source(host, source, files)
    definition = json.loads(files["deployment.json"][0])
    artifacts = await await_product_validation(source, product_runtime_source, files, selected,
        verified_artifacts=source.artifacts, source_custody=source)
    current()
    host.validate_settings(host.guide_id, settings)
    fields = package_configuration_fields(definition, files)
    authentication = package_console_scope(files, fields, {})["authentication"]
    state, directory = _directory(source)
    _fresh(directory, source._root)
    values = prepare_private_settings(source._root, fields, settings, False,
        port=definition["port"], console_authentication=authentication)
    host.validate_settings(host.guide_id, values)
    require_available_console_port(int(values["MOBKIT_PORT"]))
    current()
    require_product_source(source, host._target, destination=source._root, files=files)
    _fresh(directory, source._root)
    if host._destination_installation_entered:
        raise CustodyError("The original destination installer entered during source validation.")
    receipt = new_installation_receipt(source.receipt["deployment_id"], host.guide_id, provenance,
        source._context["guide_version"], source.receipt["build_id"], source.receipt["package_sha256"],
        source._root, "interactive", files, "application", runtime_selection=selected)
    # There is no async gap from the final owner/source/settings checks through
    # publication. Partial publication is preserved under this fixed identity.
    host._destination_installation_entered = True
    _private_directories(state, directory.parent)
    _fresh(directory, source._root)
    directory.mkdir(mode=0o700)
    write_private(directory / "receipt.json", receipt)
    write_private(source._root / OWNER_FILE, ownership(receipt))
    descriptor = os.open(directory / "package.zip", os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
    with os.fdopen(descriptor, "wb") as output:
        output.write(payload)
        output.flush()
        os.fsync(output.fileno())
    write_private_settings(source._root, values)
    journal(directory, "Installation admitted for this checked package.", status="queued", phase="install",
        console_url=None, runtime_pid=None, bootstrap=None, checks=[],
        startup_attempt_id=None, startup_console_health=None, startup_port_observation=None,
        health="unknown", health_checked_at=None, process_live=False)
    write_private(directory.parent / "latest.json", {"deployment_id": receipt["deployment_id"]})

    retained = _Inputs(directory)
    retained.read(directory / "package.zip", receipt["package_sha256"], len(payload), size=len(payload), keep=False)
    owner = _Inputs(source._root)
    private_bytes = ((source._root / OWNER_FILE,
        (json.dumps(ownership(receipt), indent=2) + "\n").encode()),
        (source._root / ".env", _serialized_private_configuration(values).encode("utf-8")))
    for path, content in private_bytes:
        info = path.lstat()
        if (not stat.S_ISREG(info.st_mode) or info.st_uid != os.getuid()
                or info.st_nlink != 1 or info.st_mode & 0o077):
            raise CustodyError("The original installed ownership or private settings are not private and owned.")
        owner.read(path, hashlib.sha256(content).hexdigest(), len(content), size=len(content), keep=False)
    binding = {name: receipt.get(name) for name in ("deployment_id", "guide_id", "guide_version",
        "admitted_guide_version", "build_id", "blueprint_hash", "brief_hash", "package_sha256",
        "destination", "target", "execution_mode", "runtime_candidate")}

    def installed_current():
        current()
        retained.finish()
        owner.finish()
        fresh = read_receipt(directory)
        if any(not _same_json_value(fresh.get(name), value) for name, value in binding.items()):
            raise CustodyError("The original destination installation journal changed its admitted source.")
        require_product_installation(source, fresh)

    installed_current()
    supervisor = Supervisor(directory, source_custody=source)
    # The accepted application inventory stays closed. Its check may import
    # authored Python modules but must not add unselected source cache files.
    supervisor.environment["PYTHONDONTWRITEBYTECODE"] = "1"
    # Retain even a failing/cancelled installer. The native preparation handle
    # has its own original once-only slot and remains unoccupied here.
    host._destination_installation_supervisor = supervisor
    cleanup = _DestinationInstallCleanup(supervisor, source)
    try:
        installed_current()
        await supervisor.install(definition, product_inputs=(files, artifacts, installed_current))
        installed_current()
        cleanup.require_settled()
        fresh = read_receipt(directory)
        proof = fresh.get("installed_candidate_runtime_proof")
        if (type(proof) is not dict or proof.get("source") != "meta-toolkit-product"
                or proof.get("version") != source.origin.meta.version
                or proof.get("assembly_sha256") != source.origin.assembly.sha256
                or proof.get("archive_sha256") != artifacts.archive.sha256
                or proof.get("catalog_sha256") != source.origin.model_catalog.sha256
                or proof.get("resources_sha256") != artifacts.candidate["resources_sha256"]):
            raise CustodyError("The original installer did not retain its checked selected runtime and application completion.")
        journal(directory, "Runtime installation is prepared and stopped. Supply the original Configure inputs to start the application.",
            status="stopped", phase="configure", console_url=None, runtime_pid=None,
            process_live=False, health="unknown", health_checked_at=None)
        installed_current()
        return supervisor
    except BaseException:
        supervisor.stop.set()
        # Supervisor.install owns and joins its actual child/file cleanup. Its
        # retained handle and fixed journal survive failure; no retry or native
        # completion is inferred, and no source scope/lease is released here.
        raise
