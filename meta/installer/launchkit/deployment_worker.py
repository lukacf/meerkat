"""Bounded local installer and owned native runtime supervisor, with private logs."""
from __future__ import annotations

import asyncio
from collections.abc import Callable
from concurrent.futures import CancelledError as ThreadCommandCancelled
from copy import deepcopy
from dataclasses import dataclass
import hashlib
import inspect
import json
import os
from pathlib import Path
import re
import signal
import stat
import sys
import uuid
from urllib.parse import urlparse

from .deployment import (MAX_PACKAGE_BYTES, await_validation, journal, materialize, now, package_files, read_receipt,
                         parse_setting, update_receipt, verify_destination)
from .projects import DraftConflict
from .admission_health import NativeAdmissionPending, PENDING_HEALTH_WINDOW, pending_admission
from .templates.storage_setup import project_lock
from .deployment_deadlines import (APPLICATION_STOP_TIMEOUT, LEGACY_USER_SERVICE_STOP_TIMEOUT,
                                   PROCESS_FORCE_REAP_TIMEOUT, USER_SERVICE_STOP_MARGIN, WORKER_STOP_MARGIN,
                                   reported_gateway_horizon, shutdown_budget)
from .deployment_ports import require_available_console_port, LocalConsolePortUnavailable
from .native_definition_continuity import valid_continuity_projection
from .owned_worker import end_after_owner_loss, serve_as_owned_worker, spawn_owned_worker

INSTALL_TIMEOUT = 1200
CHECK_TIMEOUT = 180
START_TIMEOUT = 240
CONSOLE_START_TIMEOUT = 30
CONSOLE_START_RETRY_INTERVAL = 1
STOP_TIMEOUT = APPLICATION_STOP_TIMEOUT
MAX_LOG_BYTES = 64 * 1024
MAX_READY_BYTES = 2 * 1024 * 1024
MAX_PRIVATE_COMMAND_BYTES = 1024 * 1024
# A sanity bound on an installed gateway before its receipt hash, which reads
# the file in 1 MiB chunks. The installer already checked the release bytes
# against their published checksum; Meta's rpc_gateway is 471 MiB at 0.9.4.
MAX_NATIVE_BINARY_BYTES = 1024 * 1024 * 1024
# Two bounded SDK source/cache files encoded as base64, plus their JSON/path
# overhead. This stdin is never rendered or retained in the public log buffer.
MAX_PRIVATE_COMMAND_INPUT_BYTES = 2 * ((32 * 1024 * 1024 + 2) // 3 * 4) + 64 * 1024
NATIVE_DEFINITION_READ = (Path(__file__).with_name("native_definition_continuity.py").read_text() + "\n" + '''import json, sys
import app
original_connect = app.connect_application_runtime
async def observe_connect(config, application):
    runtime = await original_connect(config, application)
    # Pinned 0.8.42's typed capabilities projection drops newer fields. Read
    # the exact native result through its existing transport, as the host turn
    # carrier does; retain only the two typed booleans required here.
    capabilities = await runtime._rpc("mobkit/capabilities", {})
    support = capabilities.get("host_system_prompt", {}) if isinstance(capabilities, dict) else {}
    print(json.dumps({"mob_id": application.deployment["slug"],
        "source": "native runtime capabilities", "host_system_prompt": {
            "send": support.get("send") is True, "dispatch": support.get("dispatch") is True}}), flush=True)
    if application.deployment.get("mobkitVersion") in SUPPORTED_NATIVE_VERSIONS and not all(
            support.get(method) is True for method in ("send", "dispatch")):
        page = await runtime._rpc("mobkit/mob_events/query",
            continuity_query(application.deployment["slug"]))
        try:
            continuity = registry_role_continuity(page, application.deployment["slug"],
                native_version=application.deployment["mobkitVersion"])
        except ValueError:
            continuity = None
        if continuity is not None:
            verify_continuity_runtime(application.context.root, config.gateway_bin,
                native_version=application.deployment["mobkitVersion"])
        print(json.dumps({"mob_id": application.deployment["slug"],
            "source": "native role continuity observation", "continuity": continuity}), flush=True)
    return runtime
app.connect_application_runtime = observe_connect
sys.argv = ["app.py", "--definition-revision"]
app.main()
''')


def clean_environment():
    # Never inherit Forge provider credentials, Python module paths or proxies.
    allowed = {"PATH", "HOME", "USER", "LOGNAME", "LANG", "LC_ALL", "TMPDIR", "SHELL",
               "SSL_CERT_FILE", "SSL_CERT_DIR"}
    values = {key: value for key, value in os.environ.items() if key in allowed}
    values.update(PYTHONUNBUFFERED="1", PYTHONNOUSERSITE="1")
    return values


def read_settings(destination):
    values = {}
    path = destination / ".env"
    if path.is_symlink() or path.stat().st_size > 256 * 1024:
        raise ValueError("The private configuration path is unsafe or too large.")
    for line in path.read_text(encoding="utf-8").splitlines():
        if not line or line.startswith("#"):
            continue
        key, separator, value = line.partition("=")
        if not separator or not re.fullmatch(r"[A-Z][A-Z0-9_]*", key):
            raise ValueError("Private configuration contains an invalid name.")
        value = parse_setting(value)
        if not isinstance(value, str) or len(value) > 32768 or "\x00" in value:
            raise ValueError("Private configuration values must be bounded UTF-8 strings.")
        values[key] = value
    return values


class Redactor:
    def __init__(self, values):
        variants = set()
        for value in values:
            if isinstance(value, str) and value:
                variants.add(value)
                variants.add(json.dumps(value, ensure_ascii=True)[1:-1])
                variants.update(value.splitlines())
        self.values = sorted(variants, key=len, reverse=True)

    def __call__(self, text):
        for value in self.values:
            text = text.replace(value, "[private]")
        text = re.sub(r"(?i)(bearer\s+)[^\s\"']+", r"\1[private]", text)
        text = re.sub(r"(?i)((?:token|secret|password|api[_-]?key)[\"']?\s*[:=]\s*[\"']?)[^\s\"',}]+", r"\1[private]", text)
        text = re.sub(r"\b(?:sk-|AIza)[A-Za-z0-9_-]{8,}", "[private]", text)
        return text


def readiness(payload, definition, expected_pid, port):
    if not isinstance(payload, dict) or type(payload.get("pid")) is not int or payload["pid"] != expected_pid:
        raise ValueError("The application did not provide owned startup evidence.")
    bootstrap = payload.get("bootstrap")
    expected = {item["identity"] for item in definition["agents"]}
    if not isinstance(bootstrap, dict) or not isinstance(bootstrap.get("identities"), dict) or not expected.issubset(bootstrap["identities"]):
        raise ValueError("The native startup observation omitted a configured agent.")
    url = payload.get("console_url", "")
    if not isinstance(url, str):
        raise ValueError("The native console does not match this deployment.")
    parsed = urlparse(url)
    if (parsed.scheme != "http" or parsed.hostname != "127.0.0.1" or parsed.port != port
            or parsed.path != "/console" or parsed.query or parsed.fragment
            or parsed.username is not None or parsed.password is not None):
        raise ValueError("The native console does not match this deployment.")
    if "admission" in payload:
        admission = pending_admission(bootstrap, payload["admission"], seeds=expected)
        if admission is not None:
            raise NativeAdmissionPending(bootstrap, admission)
    if (not isinstance(bootstrap, dict) or bootstrap.get("complete") is not True
            or bootstrap.get("ready") is not True
            or not isinstance(bootstrap.get("counts"), dict)
            or type(bootstrap["counts"].get("broken")) is not int or bootstrap["counts"]["broken"] != 0
            or not expected.issubset(bootstrap.get("identities", {}))):
        raise ValueError("The native agents did not finish bootstrap.")
    return bootstrap, url


def knowledge_container_owned(labels, destination):
    if not isinstance(labels, dict):
        return False
    root = destination / "knowledge"
    return (labels.get("com.docker.compose.project.working_dir") == str(root)
            and labels.get("com.docker.compose.project.config_files") == str(root / "compose.json"))


def console_health_headers(destination):
    """Use only the trusted host helper, never imported application code."""
    if not (Path(destination) / "application/console.json").exists():
        return {}
    try:
        from .templates_v2.console_auth import health_headers
        return health_headers(Path(destination))
    except (OSError, ValueError, ImportError):
        raise ValueError("The private Console health authorization could not be verified.") from None



STARTUP_REFUSAL_GRACE = 2


def startup_refusal(line):
    """A child's custody refusal, reported on its readiness channel before exiting.

    The child writes {"refused": message} only for a source custody refusal
    before readiness, so the Start failure names the file and the remedy.
    """
    try:
        payload = json.loads(line)
    except (ValueError, UnicodeError, TypeError):
        return None
    message = payload.get("refused") if type(payload) is dict and set(payload) == {"refused"} else None
    if type(message) is not str or not message.strip():
        return None
    return "The application refused to start: " + message.strip()[:2000]

class Supervisor:
    # The horizon the current child's gateway advertised, reported at readiness.
    gateway_shutdown_horizon_ms = None
    # Whether this worker is the recorded operating-system service's own run.
    service_owned = False
    # The stop budget its manager enforces on that run: the unit's own until
    # this run's widening and manager reload succeed. None outside a service.
    enforced_stop_budget = None

    def __init__(self, directory, *, source_custody=None):
        self.directory = Path(directory)
        self.receipt = read_receipt(directory)
        self.destination = Path(self.receipt["destination"])
        self._product_source_custody = source_custody
        if source_custody is not None:
            from .product_runtime_custody import product_source_custody, require_product_installation
            product_source_custody(source_custody).require_owner_current()
            require_product_installation(source_custody, self.receipt)
        self.stop = asyncio.Event()
        self.child = None
        self._child_admission = None
        self._bootstrap_admission = None
        self._selected_sdk_install_task = None
        self.logs = []
        values = read_settings(self.destination)
        # Unknown existing settings are treated as private too.
        self.redact = Redactor(values.values())
        self.port = int(values["MOBKIT_PORT"])
        self.environment = clean_environment()
        # Runtime code reads its own .env. The environment contains no secrets.
        home = self.destination / ".mobkit/home"
        home.mkdir(mode=0o700, exist_ok=True)
        if home.is_symlink():
            raise ValueError("The deployment's private runtime home cannot be a link.")
        self.environment["HOME"] = str(home)

    def record_log(self, phase, line):
        text = self.redact(line)[:4000]
        if not text.strip():
            return
        self.logs.append({"at": now(), "phase": phase, "text": text})
        while self.logs and sum(len(item["text"].encode()) for item in self.logs) > MAX_LOG_BYTES:
            self.logs.pop(0)
        update_receipt(self.directory, logs=self.logs[-120:])

    async def output(self, pipe, phase, captured=None):
        buffer = b""
        discard = False
        while chunk := await pipe.read(4096):
            buffer += chunk
            while b"\n" in buffer:
                line, buffer = buffer.split(b"\n", 1)
                if not discard:
                    text = line.decode("utf-8", errors="replace")
                    self.record_log(phase, text)
                    if captured is not None:
                        captured.append(text)
                        if sum(len(item.encode()) for item in captured) > MAX_LOG_BYTES:
                            raise ValueError("The native definition observation exceeded its bounded host output.")
                discard = False
            if len(buffer) > MAX_LOG_BYTES:
                # Do not persist fragments that could split a secret value.
                buffer = b""
                discard = True
        if buffer and not discard:
            text = buffer.decode("utf-8", errors="replace")
            self.record_log(phase, text)
            if captured is not None:
                captured.append(text)

    async def terminate(self, timeout=None):
        if timeout is None:
            timeout = self.application_stop_window()
        process = self.child
        if not process:
            return
        if process.returncode is None:
            # Let the application close adapters and native state first. Installer
            # commands can terminate their whole owned group immediately.
            try:
                if timeout <= 10:
                    os.killpg(process.pid, signal.SIGTERM)
                else:
                    process.terminate()
                await asyncio.wait_for(self.parent_exit(process), timeout)
            except ProcessLookupError:
                pass
            except asyncio.TimeoutError:
                pass
        # Parent exit does not prove its installers/native children have exited.
        # This process group was created by us, never reconstructed from a PID.
        try:
            os.killpg(process.pid, signal.SIGTERM)
            await asyncio.sleep(0.2)
            os.killpg(process.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        try:
            await asyncio.wait_for(process.wait(), PROCESS_FORCE_REAP_TIMEOUT)
        except asyncio.TimeoutError:
            raise RuntimeError("The owned application process did not confirm exit after bounded force cleanup.") from None

    def service_folder_evidence(self):
        """This exact registered launchd instance's record that it read its own folders, or None."""
        from .deployment_service import service_folder_evidence
        receipt = read_receipt(self.directory)
        service = receipt.get("service")
        if (not isinstance(service, dict) or service.get("manager") != "launchd"
                or receipt.get("service_supervisor_pid") != os.getpid() or not receipt.get("service_instance_id")):
            return None
        return service_folder_evidence(service, receipt)

    def own_service(self):
        """Run as the recorded operating-system service's own worker."""
        self.service_owned = True
        service = read_receipt(self.directory).get("service") or {}
        budget = service.get("stop_timeout_seconds", LEGACY_USER_SERVICE_STOP_TIMEOUT)
        self.enforced_stop_budget = budget if service and type(budget) is int else None

    def application_stop_window(self):
        """The application's stop window, inside the stop budget its manager enforces.

        The force reap, this worker's margin and the service margin must fit
        too, so that the worker can record its cleanup before the manager
        kills the unit: 367 seconds under an earlier 397-second unit.
        """
        horizon_ms = self.gateway_shutdown_horizon_ms
        window = STOP_TIMEOUT if horizon_ms is None else shutdown_budget(horizon_ms)["application"]
        if self.enforced_stop_budget is not None:
            window = min(window, self.enforced_stop_budget - PROCESS_FORCE_REAP_TIMEOUT - WORKER_STOP_MARGIN
                         - USER_SERVICE_STOP_MARGIN)
        return window

    async def widen_service_stop_budget(self, horizon_ms):
        """The service's own run widens its unit's stop budget; any failure is only logged."""
        from .deployment_service import UserServiceManager, widen_service_stop_budget
        try:
            reload = widen_service_stop_budget(self.directory, horizon_ms)
            if reload is not None:
                await UserServiceManager().command(reload)
                # The manager enforces the widened budget once its reload succeeded.
                if self.enforced_stop_budget is not None:
                    self.enforced_stop_budget = max(self.enforced_stop_budget, shutdown_budget(horizon_ms)["service"])
        except Exception as error:
            # The widening never blocks native readiness.
            self.record_log("service", f"The owned service stop budget was not widened: {error}")

    async def parent_exit(self, process):
        # asyncio Process.wait() can wait for inherited stdout held by a
        # grandchild after its parent exited. Observe the owned parent's result
        # first, then terminate descendants and drain pipes in cleanup.
        while process.returncode is None:
            await asyncio.sleep(0.05)
        return process.returncode

    def require_child_slot(self):
        if getattr(self, "_child_admission", None) is not None:
            raise RuntimeError("A local child admission is still in progress; its original owner must settle it.")
        if self.child is not None:
            raise RuntimeError("The previous owned child has not confirmed cleanup; its exact handle is retained.")
        if self.stop.is_set():
            raise asyncio.CancelledError

    async def spawn_child(self, *args, **options):
        self.require_child_slot()
        admission = object()
        self._child_admission = admission
        try:
            process, cancelled = await spawn_owned_worker(*args, preserve_admission_errors=True, **options)
            self.child = process
            return process, cancelled
        finally:
            if self._child_admission is admission:
                self._child_admission = None

    async def cleanup_child(self, process, tasks=(), *, timeout=None, error=None):
        """Join one entered teardown without dropping custody on cancellation."""
        async def finish():
            failures = []
            try:
                if self.child is not process:
                    raise RuntimeError("The owned child changed before its retained cleanup; no replacement was signalled.")
                await self.terminate(timeout=timeout)
            except BaseException as failure:
                failures.append(failure)
            try:
                if tasks:
                    await asyncio.wait_for(asyncio.gather(*tasks, return_exceptions=True),
                                           PROCESS_FORCE_REAP_TIMEOUT)
            except BaseException as failure:
                failures.append(failure)
            if failures:
                if len(failures) == 1:
                    raise failures[0]
                raise BaseExceptionGroup("Owned child termination and output settlement failed.", failures)
            if self.child is process:
                self.child = None

        cleanup = asyncio.create_task(finish())
        interruption = None
        cleanup_error = None
        while True:
            try:
                await asyncio.shield(cleanup)
                break
            except asyncio.CancelledError as failure:
                if cleanup.cancelled():
                    cleanup_error = failure
                    break
                if interruption is None:
                    interruption = failure
            except BaseException as failure:
                cleanup_error = failure
                break
        failures = []
        if error is not None:
            failures.append(error)
        if interruption is not None and not isinstance(error, asyncio.CancelledError):
            failures.append(interruption)
        if cleanup_error is not None:
            failures.append(cleanup_error)
        if cleanup_error is not None or interruption is not None:
            if len(failures) == 1:
                raise failures[0]
            raise BaseExceptionGroup("Owned child operation was interrupted or cleanup failed.", failures)

    async def install_selected_sdk_pair(self, meta_path, toolkit_path, *, meta, toolkit,
            site_packages, sdk_python, require_current, dependency_requirements=None,
            dependency_wheels=None, timeout=INSTALL_TIMEOUT, require_owner_current=None):
        """Join the existing byte installer and every interpreter child owner.

        The trusted caller still owns its original project lease and artifact
        selection. This method adds no selection, receipt or native admission.
        Its staging worker cannot settle until the original command has joined
        cleanup, including across repeated cancellation of this waiter.
        """
        from .templates.install_candidate import install_meta_toolkit_wheels, CandidateArtifactError
        if (type(timeout) not in {int, float} or not 0 < timeout < float("inf")
                or not callable(require_current)
                or require_owner_current is not None and not callable(require_owner_current)):
            raise ValueError("Selected SDK installation needs its original fence and finite positive deadline.")
        self.require_child_slot()
        if getattr(self, "_selected_sdk_install_task", None) is not None:
            raise RuntimeError("The previous selected SDK staging worker has not settled.")
        loop = asyncio.get_running_loop()
        deadline = loop.time() + timeout

        def current():
            if self.stop.is_set():
                raise asyncio.CancelledError
            result = require_current()
            if inspect.isawaitable(result):
                if inspect.iscoroutine(result):
                    result.close()
                raise CandidateArtifactError("SDK installation needs a synchronous owner fence.")
            if result is False:
                raise CandidateArtifactError("The selected SDK installation owner is no longer current.")
            if self.stop.is_set():
                raise asyncio.CancelledError
            if loop.time() >= deadline:
                raise TimeoutError("The selected SDK installation exceeded its time limit.")

        async def command(arguments, *, input, pip, retained_child):
            current()
            remaining = deadline - loop.time()
            if remaining <= 0:
                raise TimeoutError("The selected SDK installation exceeded its time limit.")
            try:
                outcome = await self.command(arguments,
                    "Selected SDK dependencies" if pip else "Selected SDK bytecode",
                    remaining, raw_outcome=True, input=input, pip=pip)
            except BaseException as error:
                # A cancelled concurrent future does not carry its coroutine's
                # exception attributes. Keep the exact unreconciled handle for
                # the cancellation translation, never an unrelated child.
                if getattr(error, "_forge_retain_selected_staging", False):
                    retained_child.append(self.child)
                raise
            current()
            return outcome

        def execute_command(arguments, *, input=None, pip=False):
            current()
            retained_child = []
            future = asyncio.run_coroutine_threadsafe(
                command(arguments, input=input, pip=pip, retained_child=retained_child), loop)
            try:
                return future.result()
            except ThreadCommandCancelled:
                error = asyncio.CancelledError()
                if retained_child and self.child is retained_child[0] and self.child is not None:
                    error._forge_retain_selected_staging = True
                raise error from None

        if require_owner_current is not None:
            require_owner_current()
        current()
        task = asyncio.create_task(asyncio.to_thread(install_meta_toolkit_wheels,
            meta_path, toolkit_path, meta=meta, toolkit=toolkit,
            deployment_root=self.destination, site_packages=site_packages,
            sdk_python=sdk_python, require_current=current,
            dependency_requirements=dependency_requirements, dependency_wheels=dependency_wheels,
            execute_command=execute_command))
        self._selected_sdk_install_task = task
        interrupted = None
        owner_error = None
        try:
            while True:
                try:
                    if require_owner_current is not None and interrupted is None and owner_error is None:
                        try:
                            require_owner_current()
                        except BaseException as error:
                            self.stop.set()
                            owner_error = error
                    if require_owner_current is not None and not task.done():
                        await asyncio.wait((task,), timeout=0.05)
                        continue
                    result = await asyncio.shield(task)
                    break
                except asyncio.CancelledError as error:
                    if task.cancelled():
                        if owner_error is not None:
                            raise BaseExceptionGroup("Selected SDK installation and its original owner failed.",
                                [owner_error, error]) from None
                        raise
                    self.stop.set()
                    if interrupted is None:
                        interrupted = error
                except BaseException as error:
                    if owner_error is not None:
                        raise BaseExceptionGroup("Selected SDK installation and its original owner failed.",
                            [owner_error, error]) from None
                    if interrupted is not None:
                        raise BaseExceptionGroup("Selected SDK installation and its waiter failed.",
                            [interrupted, error]) from None
                    raise
            if interrupted is not None:
                raise interrupted
            if owner_error is not None:
                raise owner_error
            if require_owner_current is not None:
                require_owner_current()
            current()
            return result
        finally:
            if task.done() and self._selected_sdk_install_task is task:
                self._selected_sdk_install_task = None

    async def command(self, args, phase, timeout, *, installer=False, capture_output=False,
            raw_outcome=False, input=None, pip=False):
        self.require_child_slot()
        if type(raw_outcome) is not bool or raw_outcome and capture_output:
            raise ValueError("Private raw outcome cannot be combined with decoded output capture.")
        if (type(pip) is not bool or pip and not raw_outcome
                or input is not None and (not raw_outcome or type(input) is not bytes
                    or len(input) > MAX_PRIVATE_COMMAND_INPUT_BYTES)):
            raise ValueError("Selected interpreter input needs bounded raw bytes and fixed pip configuration.")
        if raw_outcome and (type(timeout) not in {int, float} or not 0 < timeout < float("inf")):
            raise ValueError("Private raw outcome needs a finite positive timeout.")
        environment = self.environment
        if installer:
            # install.sh already supports this explicit, quoted interpreter path.
            # Select the running host interpreter, never an ambient or private setting.
            environment = {**environment, "PYTHON_BIN": str(Path(sys.executable).resolve(strict=True))}
        if pip:
            environment = {**environment, "PIP_CONFIG_FILE": os.devnull}
        # The opt-in preserves exact receipt/error streams in private caller
        # memory and makes native create_new outputs private at creation time.
        private_options = {"umask": 0o077} if raw_outcome else {}
        if input is not None:
            private_options["stdin"] = asyncio.subprocess.PIPE
        process, cancelled = await self.spawn_child(*args, cwd=self.destination, env=environment,
            stdout=asyncio.subprocess.PIPE, stderr=asyncio.subprocess.PIPE if raw_outcome else asyncio.subprocess.STDOUT,
            **private_options)
        if raw_outcome:
            try:
                return await self._private_command_outcome(process, phase, timeout,
                    cancelled=cancelled, input=input)
            except BaseException as error:
                # Only this original process can retain this attempt's staging.
                # A foreign occupied child or failed preflight cannot do so.
                if self.child is process:
                    error._forge_retain_selected_staging = True
                raise
        captured = [] if capture_output else None
        output = asyncio.create_task(self.output(process.stdout, phase, captured))
        exited = asyncio.create_task(self.parent_exit(process))
        stopped = asyncio.create_task(self.stop.wait())
        deadline = asyncio.get_running_loop().time() + timeout
        pending_output = {output}
        try:
            if cancelled or self.stop.is_set():
                raise asyncio.CancelledError
            while True:
                done, _ = await asyncio.wait({exited, stopped, *pending_output},
                    timeout=max(0, deadline - asyncio.get_running_loop().time()),
                    return_when=asyncio.FIRST_COMPLETED)
                if stopped in done:
                    raise asyncio.CancelledError
                for drained in done & pending_output:
                    # A failed reader must end the command before its child
                    # can block on an undrained pipe or report success.
                    await drained
                    pending_output.remove(drained)
                if exited in done:
                    await exited
                    break
                if not done:
                    raise TimeoutError("The deployment step exceeded its time limit.")
            if process.returncode:
                raise RuntimeError(f"The {phase} step failed (exit {process.returncode}). Inspect the private, redacted logs.")
        finally:
            stopped.cancel()
            await self.cleanup_child(process, (output, exited, stopped), timeout=10, error=sys.exception())
        # Joined cleanup may observe a late drain error after the parent exits.
        await output
        if captured is not None:
            return "\n".join(captured)

    async def _private_command_output(self, pipe, phase):
        retained = bytearray()
        buffer = b""
        discard = False
        while chunk := await pipe.read(4096):
            if len(retained) + len(chunk) > MAX_PRIVATE_COMMAND_BYTES:
                raise ValueError("The private installer stream exceeded its bounded raw output.")
            retained.extend(chunk)
            buffer += chunk
            while b"\n" in buffer:
                line, buffer = buffer.split(b"\n", 1)
                if not discard:
                    # Display logs follow the same separate redaction owner.
                    # Exact retained bytes never pass through log projection.
                    self.record_log(phase, line.decode("utf-8", errors="replace"))
                discard = False
            if len(buffer) > MAX_LOG_BYTES:
                buffer = b""
                discard = True
        if buffer and not discard:
            self.record_log(phase, buffer.decode("utf-8", errors="replace"))
        return bytes(retained)

    async def _private_command_input(self, pipe, data):
        try:
            for offset in range(0, len(data), 64 * 1024):
                pipe.write(data[offset:offset + 64 * 1024])
                await pipe.drain()
        finally:
            pipe.close()
            await pipe.wait_closed()

    async def _private_command_outcome(self, process, phase, timeout, *, cancelled=False, input=None):
        outputs = (asyncio.create_task(self._private_command_output(process.stdout, phase)),
                   asyncio.create_task(self._private_command_output(process.stderr, phase)))
        exited = asyncio.create_task(self.parent_exit(process))
        stopped = asyncio.create_task(self.stop.wait())
        deadline = asyncio.get_running_loop().time() + timeout
        feed = (asyncio.create_task(self._private_command_input(process.stdin, input))
            if input is not None else None)
        io_tasks = (*outputs, feed) if feed is not None else outputs
        pending_output = set(io_tasks)
        try:
            if cancelled or self.stop.is_set():
                raise asyncio.CancelledError
            while True:
                done, _ = await asyncio.wait({exited, stopped, *pending_output},
                    timeout=max(0, deadline - asyncio.get_running_loop().time()),
                    return_when=asyncio.FIRST_COMPLETED)
                if stopped in done:
                    raise asyncio.CancelledError
                for output in done & pending_output:
                    # Overflow/read/logging errors end the entered operation
                    # immediately, even while the owned parent is still alive.
                    await output
                    pending_output.remove(output)
                if exited in done:
                    await exited
                    break
                if not done:
                    raise TimeoutError("The deployment step exceeded its time limit.")
        finally:
            stopped.cancel()
            await self.cleanup_child(process, (*io_tasks, exited, stopped), timeout=10, error=sys.exception())
        # A late drain failure must not disappear in cleanup's joined gather.
        stdout, stderr = await outputs[0], await outputs[1]
        if feed is not None:
            await feed
        return {"returncode": process.returncode, "stdout": stdout, "stderr": stderr}

    async def native_definition_revision(self, files, *, require_carrier=True, expected_revision=None):
        """Read one actual native epoch from this exact owned cold host."""
        if expected_revision is not None and (type(expected_revision) is not int or expected_revision < 1):
            raise RuntimeError("Native definition observation needs an exact positive revision.")
        definition = json.loads(files["deployment.json"][0])
        verify_destination(read_receipt(self.directory), files)
        from .product_runtime_custody import await_product_validation
        source_custody = getattr(self, "_product_source_custody", None)
        binary = (self.native_binary_provenance(files) if source_custody is None else
            await await_product_validation(source_custody, self.native_binary_provenance, files))
        text = await self.command([str(self.destination / ".venv/bin/python"), "-c", NATIVE_DEFINITION_READ],
                                  "Native definition revision", START_TIMEOUT, capture_output=True)
        verify_destination(read_receipt(self.directory), files)
        late_binary = (self.native_binary_provenance(files) if source_custody is None else
            await await_product_validation(source_custody, self.native_binary_provenance, files))
        if late_binary != binary:
            raise RuntimeError("The verified native binary changed during definition observation.")
        observations, capabilities, continuity = [], [], []
        for line in text.splitlines():
            try:
                value = json.loads(line)
            except ValueError:
                continue
            if (isinstance(value, dict) and value.get("mob_id") == definition["slug"]
                    and value.get("source") == "native structural event ledger"
                    and type(value.get("definition_revision")) is int
                    and value["definition_revision"] >= 1):
                observations.append(value["definition_revision"])
            if (isinstance(value, dict) and value.get("mob_id") == definition["slug"]
                    and value.get("source") == "native runtime capabilities"
                    and isinstance(value.get("host_system_prompt"), dict)):
                capabilities.append(value["host_system_prompt"])
            if (isinstance(value, dict) and value.get("mob_id") == definition["slug"]
                    and value.get("source") == "native role continuity observation"):
                continuity.append(value.get("continuity"))
        if len(observations) != 1 or len(capabilities) != 1:
            raise RuntimeError("The exact native host did not provide one unambiguous definition revision and capability observation. No revision was guessed.")
        if expected_revision is not None and observations[0] != expected_revision:
            raise RuntimeError("The observed native definition does not match the exact expected revision. Preserve the update and its original activation pin.")
        supported = {method: capabilities[0].get(method) is True for method in ("send", "dispatch")}
        unchanged_roles = (len(continuity) == 1
            and valid_continuity_projection(continuity[0], definition["slug"], observations[0])
            and binary.get("native_binary_version") == continuity[0].get("native_version")
            and (binary.get("native_binary_source") == "official-release" or
                 binary.get("native_binary_source") == "signed-candidate"
                 and isinstance(binary.get("candidate_binding"), dict)
                 and binary["candidate_binding"].get("native_version") == continuity[0].get("native_version")
                 and binary["candidate_binding"].get("core_version") == continuity[0].get("core_version")))
        if not all(supported.values()) and (require_carrier or expected_revision is None
                and observations[0] != 1 and not unchanged_roles):
            raise RuntimeError("The installed verified native gateway cannot author current role instructions on host send and keyed dispatch. Preserve the observed definition and transaction state.")
        return {"mob_id": definition["slug"], "definition_revision": observations[0],
                **({"native_role_continuity": continuity[0]} if unchanged_roles else {}),
                "host_system_prompt": supported, **binary,
                "package_sha256": read_receipt(self.directory)["package_sha256"], "observed_at": now()}

    def _candidate_runtime_target(self, files):
        """Read an already admitted target from owned source and its journal.

        An authored marker or native receipt cannot select a runtime. The
        private deployment owner must have published this full identity from
        the checked update, and the accepted framework must agree with it.
        """
        from .runtime_selection import (parse_candidate_selection, selected_runtime_version,
            selected_runtime_marker)
        from .deployment_updates import _same_toml
        current = read_receipt(self.directory)
        definition = json.loads(files["deployment.json"][0])
        selected = current.get("runtime_candidate")
        if "runtimeCandidate" not in definition:
            if "runtime_candidate" in current:
                raise RuntimeError("The admitted candidate target differs from this owned deployment source.")
            return None
        if selected is None:
            raise RuntimeError("This candidate source has no previously admitted host runtime selection.")
        target = parse_candidate_selection(selected)
        selection = json.loads(files["runtime-candidate.json"][0])
        report = json.loads(files["implementation-report.json"][0])
        if (not _same_toml(selection, selected)
                or not _same_toml(report.get("framework", {}).get("runtime_candidate"), selected)
                or definition.get("mobkitVersion") != selected_runtime_version(target)
                or definition["runtimeCandidate"] != selected_runtime_marker(target)):
            raise RuntimeError("The admitted candidate identity differs from its exact accepted package.")
        if any(read_settings(self.destination).get(name) for name in
                ("MOBKIT_RPC_GATEWAY_BIN", "MOBKIT_GATEWAY_BIN", "META_FORGE_RPC_GATEWAY_BIN")):
            raise RuntimeError("The selected candidate cannot use native gateway overrides.")
        return target

    def _verify_candidate_runtime(self, files, *, source_custody=None, retained=None, observation=None):
        target = self._candidate_runtime_target(files)
        if target is None:
            return None
        from .runtime_dependencies import verify_runtime
        verify_destination(read_receipt(self.directory), files)
        from .runtime_selection import selected_runtime_version
        if source_custody is None:
            source_custody = getattr(self, "_product_source_custody", None)
        if source_custody is not None:
            from .product_runtime_custody import require_product_source, require_product_installation
            require_product_installation(source_custody, read_receipt(self.directory))
            require_product_source(source_custody, target, destination=self.destination, files=files)
        # A carrier already made its reads with any retained inventory it was given.
        proof = verify_runtime(self.destination, selected_runtime_version(target), runtime_target=target,
            **({"source_custody": source_custody} if source_custody is not None else
                {"retained": retained} if retained is not None else {}),
            **({"observation": observation} if observation is not None else {}))
        verify_destination(read_receipt(self.directory), files)
        if self._candidate_runtime_target(files) != target:
            raise RuntimeError("The admitted candidate selection changed during runtime verification.")
        return proof

    @staticmethod
    async def _startup_refusal(started):
        """The refusal an exited child wrote to its readiness channel, if any."""
        try:
            line = await asyncio.wait_for(asyncio.shield(started), STARTUP_REFUSAL_GRACE)
        except (asyncio.TimeoutError, ValueError, OSError, asyncio.LimitOverrunError, asyncio.IncompleteReadError):
            return None
        return startup_refusal(line)

    def _keep_native_inventory(self, observation, proof):
        """Keep an install's fully verified prefix inventory for its later Starts."""
        from .native_prefix_receipt import RECEIPT_FIELD, mint
        artifacts = observation.get("artifacts")
        try:
            # The selection this install verified, not a later receipt read.
            named = None if artifacts is None else mint(self.directory, artifacts, proof=proof,
                runtime_candidate=self.receipt.get("runtime_candidate"),
                fingerprints=observation.get("fingerprints"))
        except (OSError, ValueError, TypeError):
            # Keeping the inventory is only a cache; the next Start verifies fully.
            named = None
        return {RECEIPT_FIELD: named} if named is not None else {}

    def _prune_native_inventories(self):
        from .native_prefix_receipt import prune
        try:
            prune(self.directory)
        except OSError:
            pass

    def _retained_native_inventory(self, definition):
        """The inventory receipt.json names for this Start, or None for the full check."""
        if "runtimeCandidate" not in definition:
            return None
        from .native_prefix_receipt import load
        return load(self.directory, read_receipt(self.directory))

    def _settle_native_inventory(self, retained, observation, proof, verified_receipt):
        """After a verified Start, hold a used inventory to the receipt or keep a new one.

        ``verified_receipt`` is the receipt the verification ran on. A new
        inventory records its selection, and is named only while the live
        receipt still selects it.
        """
        from .native_prefix_receipt import (RECEIPT_FIELD, installed_inventory_sha256, mint, record,
            same_selection)
        artifacts = observation.get("artifacts")
        if artifacts is None or proof is None:
            return
        receipt = read_receipt(self.directory)
        if observation.get("inventory_source") == "retained":
            if (retained is None or receipt.get(RECEIPT_FIELD) != retained.receipt_sha256
                    or installed_inventory_sha256(receipt) != retained.inventory_sha256
                    or not same_selection(receipt, retained.runtime_candidate_sha256)
                    or proof.get("prefix_inventory_sha256") != retained.inventory_sha256):
                raise RuntimeError("Forge's saved native runtime inventory changed during verification. Start again.")
            return
        # The full verification ran. Keep its inventory only for the digest the
        # installed proof already records; a failed verification never gets here.
        selected = verified_receipt.get("runtime_candidate")
        if installed_inventory_sha256(verified_receipt) != proof.get("prefix_inventory_sha256"):
            return
        try:
            named = mint(self.directory, artifacts, proof=proof, runtime_candidate=selected,
                fingerprints=observation.get("fingerprints"))
            if named is not None:
                record(self.directory, named, inventory_sha256=proof["prefix_inventory_sha256"],
                    runtime_candidate=selected)
        except (OSError, ValueError, TypeError):
            # Keeping the inventory is only a cache; the next Start verifies fully.
            pass

    def native_binary_provenance(self, files):
        """Retain the existing installer's owned artifact receipt, not a version guess."""
        if self._candidate_runtime_target(files) is not None:
            proof = self._verify_candidate_runtime(files)
            if proof["source"] == "meta-toolkit-product":
                return {"native_binary_sha256": proof["binaries"]["toolkit_gateway"]["binary_sha256"],
                    "native_binary_version": proof["version"], "native_binary_source": proof["source"],
                    "native_product_assembly_sha256": proof["assembly_sha256"]}
            return {"native_binary_sha256": proof["binaries"]["rpc_gateway"]["binary_sha256"],
                "native_binary_version": proof["version"], "native_binary_source": proof["source"],
                "candidate_binding": proof["candidate_binding"],
                "candidate_tuple_sha256": proof["candidate_tuple_sha256"]}
        binary, receipt = self.destination / ".bin/rpc_gateway", self.destination / ".bin/rpc_gateway.receipt.json"
        override = read_settings(self.destination).get("MOBKIT_RPC_GATEWAY_BIN")
        if override and Path(override).resolve() != binary.resolve():
            raise RuntimeError("Native definition observation cannot use a gateway outside this installation's verified artifact receipt.")
        for path, limit in ((binary, MAX_NATIVE_BINARY_BYTES), (receipt, 1024 * 1024)):
            info = path.lstat()
            if (path.is_symlink() or any(parent.is_symlink() for parent in path.parents)
                    or not path.is_file() or info.st_uid != os.getuid() or info.st_nlink != 1
                    or info.st_mode & 0o077 or not 0 < info.st_size <= limit):
                raise RuntimeError("The native binary or its existing private artifact receipt is not owned and bounded.")
        record = json.loads(receipt.read_text())
        definition = json.loads(files["deployment.json"][0])
        if (not isinstance(record, dict) or record.get("version") != definition["mobkitVersion"]
                or record.get("source") not in {None, "explicit_override"}
                or not re.fullmatch("[0-9a-f]{64}", str(record.get("binary_sha256", "")))
                or not os.access(binary, os.X_OK)):
            raise RuntimeError("The native artifact receipt does not match this pinned deployment.")
        hashed = hashlib.sha256()
        with binary.open("rb") as stream:
            while chunk := stream.read(1024 * 1024):
                hashed.update(chunk)
        if hashed.hexdigest() != record["binary_sha256"]:
            raise RuntimeError("The native binary changed outside its existing artifact receipt.")
        return {"native_binary_sha256": record["binary_sha256"], "native_binary_version": record["version"],
                "native_binary_source": "explicit_override" if record.get("source") else "official-release"}

    async def install(self, definition, *, product_inputs=None):
        if product_inputs is not None:
            files, artifacts, require_current = product_inputs
            from .deployment import product_runtime_source
            from .runtime_dependencies import verify_runtime
            from .runtime_selection import selected_runtime_version
            from .templates.install_candidate import verify_selected_application_dependencies
            from .product_runtime_custody import await_product_validation
            source_custody = getattr(self, "_product_source_custody", None)
            require_current()
            target = self._candidate_runtime_target(files)
            if ((source_custody is None and target != artifacts.selection)
                    or definition != json.loads(files["deployment.json"][0])):
                raise DraftConflict("The initial product inputs differ from the original admitted installation.")
            await await_product_validation(source_custody, product_runtime_source, files,
                self.receipt["runtime_candidate"], verified_artifacts=artifacts,
                **({"source_custody": source_custody} if source_custody is not None else {}))
            await await_product_validation(source_custody, verify_selected_application_dependencies,
                files.get("requirements.application.txt", (b"", 0o644))[0], artifacts)
            require_current()
            journal(self.directory, "Installing the exact selected Meta/Toolkit SDK pair and native prefix.",
                status="installing", phase="install")
            async def command(*args, **kwargs):
                require_current()
                result = await self.command(*args, **kwargs)
                require_current()
                return result
            paths = await _install_selected_product(self, artifacts, require_current, command,
                source_custody=source_custody, files=files,
                report_progress=lambda message: journal(self.directory, message))
            require_current()
            journal(self.directory, "Checking the installed runtime and application.")
            observation = {}
            proof = await await_product_validation(source_custody, verify_runtime, self.destination,
                selected_runtime_version(target), runtime_target=target, **paths,
                report_progress=lambda message: journal(self.directory, message), observation=observation,
                **({"source_custody": source_custody} if source_custody is not None else {}))
            require_current()
            await command([str(paths["sdk_python"]), "app.py", "--application-check"],
                "Installed application contract", CHECK_TIMEOUT)
            require_current()
            # Only this success continuation keeps the verified inventory; a
            # failed check above never reaches it.
            kept = await asyncio.to_thread(self._keep_native_inventory, observation, proof)
            journal(self.directory, "Selected runtime bytes and application contract are checked. Configure the original native startup before use.",
                installed_candidate_runtime_proof=proof, **kept)
            if kept:
                await asyncio.to_thread(self._prune_native_inventories)
            return
        if "runtimeCandidate" in definition:
            raise RuntimeError("A private runtime candidate requires its checked stopped-installation update; a fresh installation is not admitted.")
        journal(self.directory, "Installing the pinned runtime and application dependencies.", status="installing", phase="install")
        from .deployment_cache import seed_release_cache
        cached = await asyncio.to_thread(seed_release_cache, self.destination, definition)
        if cached:
            self.record_log("install", "Copied the pinned official runtime from Forge's verified local release cache. The package installer verifies these bytes again before reuse.")
        await self.command(["/bin/bash", "install.sh", "--non-interactive"], "install", INSTALL_TIMEOUT, installer=True)
        checks = []
        if definition["knowledge"]["mode"] == "local":
            await self.prepare_knowledge(definition)
            checks.append({"name": "Local knowledge service", "status": "pass",
                           "detail": "The generated knowledge service manager started the service and completed its authenticated MCP verification."})
            update_receipt(self.directory, checks=checks, knowledge_managed=True)
        for name, flag in [("Local prerequisites", "--doctor"), ("Application contract", "--application-check")]:
            journal(self.directory, f"Checking {name.lower()}.", status="checking", phase="check")
            try:
                await self.command([str(self.destination / ".venv/bin/python"), "app.py", flag], name, CHECK_TIMEOUT)
            except Exception:
                checks.append({"name": name, "status": "fail", "detail": "The installed host-owned check failed. Inspect its redacted output below."})
                update_receipt(self.directory, checks=checks)
                raise
            checks.append({"name": name, "status": "pass", "detail": "The installed host-owned check completed successfully."})
            update_receipt(self.directory, checks=checks)
        used = {grant["connection_id"] for item in definition["classes"] for grant in item["tools"]["mcp"]}
        if used or any(item["tools"].get("knowledge") for item in definition["classes"]):
            journal(self.directory, "Connecting selected MCPs and checking their actual tool grants.", status="checking", phase="connections")
            try:
                await self.command([str(self.destination / ".venv/bin/python"), "app.py", "--connections-check"], "MCP connections", CHECK_TIMEOUT)
            except Exception:
                checks.append({"name": "MCP connections", "status": "fail", "detail": "The actual MCP connection or advertised tool grant check failed."})
                update_receipt(self.directory, checks=checks)
                raise
            checks.append({"name": "MCP connections", "status": "pass", "detail": "The installed native host completed the configured MCP handshake and grant checks."})
            update_receipt(self.directory, checks=checks)

    async def query(self, args):
        """Bounded read-only host inspection; never publicize raw Docker metadata."""
        self.require_child_slot()
        process, cancelled = await self.spawn_child(*args, cwd=self.destination, env=self.environment,
            stdout=asyncio.subprocess.PIPE, stderr=asyncio.subprocess.DEVNULL)
        captured = None
        stopped = asyncio.create_task(self.stop.wait())
        try:
            if cancelled or self.stop.is_set():
                raise asyncio.CancelledError
            async def capture():
                output = bytearray()
                while chunk := await process.stdout.read(4096):
                    output.extend(chunk)
                    if len(output) > MAX_LOG_BYTES:
                        raise ValueError("Local service inspection exceeded its result limit.")
                await self.parent_exit(process)
                if process.returncode:
                    raise ValueError("Could not inspect the local knowledge service. Check Docker access before retrying.")
                return output.decode("utf-8")
            captured = asyncio.create_task(capture())
            done, _ = await asyncio.wait({captured, stopped}, timeout=20, return_when=asyncio.FIRST_COMPLETED)
            if stopped in done or self.stop.is_set():
                raise asyncio.CancelledError
            if captured not in done:
                raise TimeoutError
            return await captured
        finally:
            stopped.cancel()
            if captured is not None:
                captured.cancel()
            await self.cleanup_child(process, (stopped,) if captured is None else (captured, stopped),
                                     timeout=10, error=sys.exception())

    async def prepare_knowledge(self, definition, *, require_current=None,
            project_lease=None, source_custody=None, settle_command=None):
        """Apply private settings through this installation's checked manager."""
        from .deployment import ownership, private_configuration_snapshot
        from .deployment_updates import checked_package
        from contextlib import nullcontext
        from .templates.storage_setup import ProjectLockLease
        from .product_runtime_custody import product_source_custody, await_product_validation
        if project_lease is not None:
            if (type(project_lease) is not ProjectLockLease or project_lease.root != self.destination
                    or not callable(require_current)):
                raise DraftConflict("Knowledge preparation requires its actual original project lease and owner.")
            project_lease.require_current()
        if settle_command is not None:
            from .deployment_application_preparation import _PreparationCustody
            preparation = getattr(settle_command, "__self__", None)
            if (type(preparation) is not _PreparationCustody
                    or settle_command != preparation.settle_command
                    or preparation.supervisor is not self or preparation.lease is not project_lease
                    or preparation.owner_task is not asyncio.current_task() or not preparation.active):
                raise DraftConflict("Knowledge commands require their original active preparation cleanup owner.")
        if source_custody is not None:
            source_custody = product_source_custody(source_custody)
            source_custody.require_owner_current()
            if source_custody._lease is not project_lease or source_custody._root != self.destination:
                raise DraftConflict("Knowledge preparation cannot replace the original entered source lease.")
            from .product_installed_source import _InstalledSourceView
            if (type(source_custody._installed_view) is not _InstalledSourceView
                    or source_custody._installed_view.knowledge is None):
                raise DraftConflict("Knowledge preparation requires its actual installed local knowledge recipe.")
            await await_product_validation(source_custody, source_custody.require_filesystem_current)
        self.require_child_slot()
        context = nullcontext(project_lease) if project_lease is not None else project_lock(self.destination)
        with context as lease:
            receipt = read_receipt(self.directory)
            binding = ownership(receipt)
            custody = {key: deepcopy(receipt.get(key)) for key in (
                "knowledge_managed", "execution_mode", "interactive_lifecycle_id", "service",
                "service_supervisor_pid", "service_instance_id", "service_cleanup_confirmed",
                "startup_attempt_id", "update", "update_action")}
            _, files = checked_package(self.directory, "package.zip", receipt["package_sha256"])
            if json.loads(files["deployment.json"][0]) != definition:
                raise DraftConflict("The knowledge startup definition differs from its checked package.")
            settings = json.loads(files["knowledge/settings.json"][0])
            knowledge = definition["knowledge"]
            if (knowledge["mode"] != "local" or settings["mode"] != "local"
                    or settings["project"] != definition["slug"]
                    or any(settings[key] != knowledge[key] for key in ("token_env", "space_env"))):
                raise DraftConflict("The local knowledge manager differs from this checked deployment.")
            provider_key = {"openai": "OPENAI_API_KEY", "anthropic": "ANTHROPIC_API_KEY",
                            "gemini": "GEMINI_API_KEY"}[settings["provider"]]
            selected_keys = {"OPENAI_API_KEY", provider_key}
            generated = {settings["token_env"], settings["space_env"]}
            async def completed_read(function, *args):
                # The checked helper just performed its admitted writes. Its
                # exact completion reader fences all unchanged inputs before
                # retaining only those outputs, then restores the full check.
                from .remote_product_artifacts import _joined
                source_custody.require_owner_current()
                result = await _joined(function, *args,
                    _owner_current=source_custody.require_owner_current)
                source_custody.require_owner_current()
                return result

            async def private_snapshot(path, *, completed=False):
                if source_custody is None:
                    return private_configuration_snapshot(path)
                if completed:
                    return await completed_read(private_configuration_snapshot, path)
                return await await_product_validation(source_custody, private_configuration_snapshot, path)
            project_values, project_snapshot = await private_snapshot(self.destination)
            knowledge_values, knowledge_snapshot = await private_snapshot(self.destination / "knowledge")
            if receipt.get("knowledge_managed") is True and any(
                    not knowledge_values.get(key) for key in ("JWT_SECRET", "SURREALDB_PASS")):
                raise DraftConflict("The managed knowledge service is missing its saved signing or database credentials. "
                    "The application remains stopped and no knowledge setting was changed. Restore the original "
                    "private knowledge/.env before starting; new credentials cannot recover the existing service.")
            self.redact = Redactor([*self.redact.values, *project_values.values(), *knowledge_values.values()])

            def current(*, private=True):
                self.require_child_slot()
                lease.require_current()
                if source_custody is not None:
                    source_custody.require_owner_current()
                if require_current is not None:
                    require_current()
                fresh = read_receipt(self.directory)
                if ownership(fresh) != binding:
                    raise DraftConflict("The installed knowledge source changed during startup preparation.")
                if any(fresh.get(key) != value for key, value in custody.items()):
                    raise DraftConflict("The original knowledge startup ownership changed during preparation.")
                checked_package(self.directory, "package.zip", binding["package_sha256"])
                verify_destination(fresh, files)
                if private and (private_configuration_snapshot(self.destination)[1] != project_snapshot
                        or private_configuration_snapshot(self.destination / "knowledge")[1] != knowledge_snapshot):
                    raise DraftConflict("Private settings changed during knowledge preparation. The application remains stopped; reload its settings.")
                self.require_child_slot()

            current()
            prepared = (binding, project_snapshot, knowledge_snapshot)
            if getattr(self, "_knowledge_prepared", None) == prepared:
                return
            await self.knowledge_preflight(**({"settle_command": settle_command}
                if settle_command is not None else {}))
            current()
            journal(self.directory, "Preparing the owned local knowledge service with its saved project credentials.",
                    **({"status": "checking", "phase": "knowledge"} if project_lease is None else {}))
            current()
            await self.command([str(self.destination / ".venv/bin/python"), "knowledge/manage.py", "setup"],
                               "Knowledge setup", CHECK_TIMEOUT)
            if settle_command is not None:
                await settle_command()
            # Setup owns token renewal and its two private-file replacements.
            # Re-read them before another command, retaining all redactions.
            # The actual checked helper has exited after its two atomic writes.
            # Verify those permitted outputs before any full-source callback can
            # mistake this admitted mutation for foreign private-input drift.
            if source_custody is None:
                current(private=False)
            saved_project, saved_project_snapshot = await private_snapshot(self.destination, completed=True)
            saved_knowledge, saved_knowledge_snapshot = await private_snapshot(
                self.destination / "knowledge", completed=True)
            self.redact = Redactor([*self.redact.values, *saved_project.values(), *saved_knowledge.values()])
            if ({key: value for key, value in saved_project.items() if key not in generated}
                    != {key: value for key, value in project_values.items() if key not in generated}):
                raise DraftConflict("Project settings changed during knowledge setup. The application remains stopped; reload its settings.")
            allowed = generated | selected_keys | {"ELEPHANT_OPERATOR_TOKEN", "MCP_BEARER_TOKEN"} | {
                key for key in ("JWT_SECRET", "SURREALDB_PASS") if key not in knowledge_values}
            if ({key: value for key, value in saved_knowledge.items() if key not in allowed}
                    != {key: value for key, value in knowledge_values.items() if key not in allowed}):
                raise DraftConflict("Unrelated private knowledge settings changed during setup. The application remains stopped.")
            if any(project_values.get(key) and saved_knowledge.get(key) != project_values[key]
                   for key in selected_keys):
                raise RuntimeError("This installed knowledge helper kept different provider credentials. The application remains stopped and the knowledge service was not restarted. Update the accepted application source to a helper that applies saved project keys, then start again.")
            if project_lease is not None and (saved_project.get(settings["space_env"]) != settings["project"]
                    or saved_knowledge.get(settings["space_env"]) != settings["project"]
                    or saved_project.get(settings["token_env"]) != saved_knowledge.get(settings["token_env"])
                    or not saved_project.get(settings["token_env"])
                    or not saved_knowledge.get("ELEPHANT_OPERATOR_TOKEN")
                    or any(not saved_knowledge.get(key) for key in ("JWT_SECRET", "SURREALDB_PASS"))):
                raise DraftConflict("The checked knowledge helper did not retain its exact local credentials and space.")
            if source_custody is not None:
                from .product_installed_source import retain_installed_knowledge_settings
                await completed_read(retain_installed_knowledge_settings,
                    source_custody, project_snapshot, knowledge_snapshot, saved_project, saved_knowledge)
            project_snapshot, knowledge_snapshot = saved_project_snapshot, saved_knowledge_snapshot
            current()
            journal(self.directory, "Applying saved credentials to the local knowledge service and verifying authenticated access.",
                    **({"status": "checking", "phase": "knowledge"} if project_lease is None else {}))
            current()
            checkout = (source_custody._installed_view._knowledge_checkout_root
                if source_custody is not None else None)
            await self.command([str(self.destination / ".venv/bin/python"), "knowledge/manage.py", "up"],
                               "Knowledge service", INSTALL_TIMEOUT)
            if settle_command is not None:
                await settle_command()
            if source_custody is not None:
                from .product_installed_source import retain_installed_knowledge_checkout
                await completed_read(retain_installed_knowledge_checkout, source_custody, checkout)
            current()
            if project_lease is not None and receipt.get("status") == "stopped":
                # The original stopped preparation owns this result. File
                # presence cannot publish authenticated service preparation.
                update_receipt(self.directory, knowledge_managed=True)
                custody["knowledge_managed"] = True
                current()
            self._knowledge_prepared = (binding, project_snapshot, knowledge_snapshot)

    async def knowledge_preflight(self, *, settle_command=None):
        compose = json.loads((self.destination / "knowledge/compose.json").read_text())
        name = compose["name"]
        if not re.fullmatch(r"[a-z][a-z0-9-]{1,60}", name):
            raise ValueError("The local knowledge service has an invalid project name.")
        async def query(arguments):
            result = await self.query(arguments)
            if settle_command is not None:
                await settle_command()
            return result
        output = await query(["docker", "ps", "-a", "--filter", f"label=com.docker.compose.project={name}", "--format", "{{.ID}}"])
        identities = output.splitlines()
        for identity in identities:
            if not re.fullmatch(r"[a-f0-9]{12,64}", identity):
                raise ValueError("Docker returned an invalid local service identity.")
            labels = json.loads(await query(["docker", "inspect", "--format", "{{json .Config.Labels}}", identity]))
            if not knowledge_container_owned(labels, self.destination):
                raise ValueError("A local knowledge service with this name belongs to another folder. No container was changed. Choose a different project name or use its existing endpoint.")
        update_receipt(self.directory, knowledge_ownership={"project": name, "working_directory": str(self.destination / "knowledge"),
                                                           "existing_owned_containers": identities})

    async def startup_console(self, client, url, definition, process, exited, stopped):
        """Wait for actual authenticated identities without publishing readiness."""
        import httpx
        expected = {item["identity"] for item in definition["agents"]}
        deadline = asyncio.get_running_loop().time() + CONSOLE_START_TIMEOUT
        attempts = 0
        last_failure = {"category": "unavailable"}
        attempt_id = read_receipt(self.directory).get("startup_attempt_id")

        def diagnostic(status, failure=None):
            value = {"status": status, "attempts": attempts, "checked_at": now(), "attempt_id": attempt_id}
            if failure:
                value.update(failure)
            update_receipt(self.directory, startup_console_health=value)

        def fail():
            diagnostic("fail", last_failure)
            suffix = last_failure["category"]
            if "http_status" in last_failure:
                suffix += ", HTTP " + str(last_failure["http_status"])
            raise RuntimeError("Native startup reported readiness, but the actual Console identities health check failed ("
                               + suffix + ").")

        async def probe():
            try:
                headers = console_health_headers(self.destination)
            except ValueError:
                return {"category": "authorization_configuration"}
            try:
                response = await client.get(url.rsplit("/console", 1)[0] + "/console/identities", headers=headers)
                status = response.status_code
                if status != 200:
                    return {"category": "authentication" if status in (401, 403) else "http_status",
                            "http_status": status}
                payload = response.json()
                rows = payload["identities"]
                if not isinstance(rows, list) or any(not isinstance(item, dict) or not isinstance(item.get("identity"), str) for item in rows):
                    return {"category": "invalid_response"}
                identities = {item["identity"] for item in rows}
                return None if expected.issubset(identities) else {"category": "missing_identities"}
            except httpx.TimeoutException:
                return {"category": "timeout"}
            except httpx.TransportError:
                return {"category": "unavailable"}
            except (KeyError, TypeError, ValueError, UnicodeError):
                return {"category": "invalid_response"}
            except Exception:
                # Never retain exception text, response bodies or private headers.
                return {"category": "probe_error"}

        while True:
            if self.stop.is_set():
                diagnostic("interrupted")
                raise asyncio.CancelledError
            if process.returncode is not None:
                last_failure = {"category": "child_exited"}
                fail()
            remaining = deadline - asyncio.get_running_loop().time()
            if remaining <= 0:
                fail()
            attempts += 1
            request = asyncio.create_task(probe())
            try:
                done, _ = await asyncio.wait({request, exited, stopped}, timeout=remaining,
                                            return_when=asyncio.FIRST_COMPLETED)
                if stopped in done or self.stop.is_set():
                    diagnostic("interrupted")
                    raise asyncio.CancelledError
                if exited in done or process.returncode is not None:
                    last_failure = {"category": "child_exited"}
                    fail()
                if request not in done:
                    last_failure = {"category": "timeout"}
                    fail()
                failure = request.result()
            finally:
                if not request.done():
                    request.cancel()
                await asyncio.gather(request, return_exceptions=True)
            if failure is None:
                if asyncio.get_running_loop().time() >= deadline:
                    last_failure = {"category": "timeout"}
                    fail()
                diagnostic("pass")
                return
            last_failure = failure
            diagnostic("checking", failure)
            if failure["category"] in {"authentication", "authorization_configuration"}:
                fail()
            remaining = deadline - asyncio.get_running_loop().time()
            if remaining <= 0:
                fail()
            await asyncio.wait({exited, stopped}, timeout=min(CONSOLE_START_RETRY_INTERVAL, remaining),
                               return_when=asyncio.FIRST_COMPLETED)

    async def start(self, definition, *, on_ready=None, application_startup=None,
                    bootstrap_selection=None):
        self.require_child_slot()
        original_bootstrap = deepcopy(bootstrap_selection)
        if application_startup is not None and original_bootstrap is not None:
            raise ValueError("Bootstrap and replacement startup cannot share a child admission.")
        from .deployment_application_bootstrap import require_bootstrap_history_settled
        require_bootstrap_history_settled(self.directory, bootstrap_selection=original_bootstrap)
        bootstrap_selected = bootstrap_observation = None
        retained_bootstrap = getattr(self, "_bootstrap_admission", None)

        def require_bootstrap_original():
            if original_bootstrap is not None:
                from .deployment_application_bootstrap import (
                    bootstrap_installation_binding, bootstrap_lifecycle_binding,
                    require_bootstrap_admission_current,
                )
                from .connect_installer_caller import _same_json_value
                require_bootstrap_admission_current(self.directory, original_bootstrap,
                    bootstrap_selected, bootstrap_observation)
                if (self.destination != Path(bootstrap_selected["installation"]["destination"])
                        or not _same_json_value(bootstrap_installation_binding(self.receipt),
                            bootstrap_selected["installation"])):
                    raise DraftConflict("The bootstrap supervisor belongs to another original installation.")
                if bootstrap_lifecycle_binding(self.receipt) != bootstrap_observation.lifecycle_json:
                    raise DraftConflict("The original bootstrap supervisor lifecycle changed.")
                if retained_bootstrap is not None:
                    pin, selected, observed = retained_bootstrap
                    if pin != original_bootstrap:
                        raise DraftConflict("The child selected another original bootstrap admission.")
                    require_bootstrap_admission_current(self.directory, pin, selected, observed)

        arguments = ("app.py",)
        candidate_proof = None
        knowledge_prepared_in_source = False
        if original_bootstrap is not None:
            from .deployment_application_bootstrap import (
                bootstrap_child_arguments, installed_bootstrap_source, observe_bootstrap_admission,
            )
            from .product_runtime_custody import await_product_validation
            def current_source_owner():
                if self.stop.is_set():
                    raise asyncio.CancelledError
                self.require_child_slot()
                if retained_bootstrap is not None:
                    pin, selected, observed = retained_bootstrap
                    if pin != original_bootstrap:
                        raise DraftConflict("The original child source admission changed.")
                    from .deployment_application_bootstrap import require_bootstrap_admission_current
                    require_bootstrap_admission_current(self.directory, pin, selected, observed)
            retained = await asyncio.to_thread(self._retained_native_inventory, definition)
            async with installed_bootstrap_source(self.directory, original_bootstrap,
                    require_current=current_source_owner,
                    owner_source=getattr(self, "_bootstrap_owner_source", None),
                    retained=retained) as source_custody:
                bootstrap_selected, bootstrap_observation = await await_product_validation(
                    source_custody, observe_bootstrap_admission, self.directory, original_bootstrap,
                    **({"source_custody": source_custody} if source_custody is not None else {}))
                self.require_child_slot()
                require_bootstrap_original()
                def installed_definition():
                    with (self.destination / "deployment.json").open("rb") as source:
                        content = source.read(MAX_PACKAGE_BYTES + 1)
                    if len(content) > MAX_PACKAGE_BYTES:
                        raise ValueError("The original bootstrap definition exceeded its admitted source bound.")
                    return json.loads(content)
                original_definition = await await_product_validation(source_custody, installed_definition)
                require_bootstrap_original()
                if original_definition != definition:
                    raise DraftConflict("The bootstrap definition differs from its original installed source.")
                current = read_receipt(self.directory)
                if "runtimeCandidate" in definition or "runtime_candidate" in current:
                    from .deployment_updates import checked_package
                    observation = {}
                    def runtime_proof():
                        _, files = checked_package(self.directory, "package.zip", current["package_sha256"])
                        if json.loads(files["deployment.json"][0]) != definition:
                            raise RuntimeError("The candidate startup definition differs from its checked package.")
                        return self._verify_candidate_runtime(files, source_custody=source_custody,
                            retained=retained, observation=observation)
                    candidate_proof = await await_product_validation(source_custody, runtime_proof)
                    require_bootstrap_original()
                    await asyncio.to_thread(self._settle_native_inventory, retained, observation, candidate_proof,
                        current)
                if (source_custody is not None and current.get("knowledge_managed") is True
                        and definition["knowledge"]["mode"] == "local"):
                    await self.prepare_knowledge(definition, require_current=current_source_owner,
                        project_lease=source_custody._lease, source_custody=source_custody)
                    require_bootstrap_original()
                    knowledge_prepared_in_source = True
                arguments = bootstrap_child_arguments(self.directory, original_bootstrap,
                    lifecycle_id=self.receipt.get("interactive_lifecycle_id"),
                    **({"owner_source": source_custody._owner_source} if source_custody is not None else {}))
            # Both the temporary source scope and its project lock are gone
            # before the actual child imports or takes its original app lease.
            require_bootstrap_original()
        elif retained_bootstrap is not None:
            raise DraftConflict("The original bootstrap admission cannot fall back to ordinary startup.")
        else:
            if application_startup is not None:
                from .deployment_application_startup import child_arguments
                arguments = child_arguments(self.directory, application_startup["mode"],
                    application_startup["admission"], application_startup["attempt"])
            current = read_receipt(self.directory)
            if "runtimeCandidate" in definition or "runtime_candidate" in current:
                from .deployment_updates import checked_package
                _, files = checked_package(self.directory, "package.zip", current["package_sha256"])
                if json.loads(files["deployment.json"][0]) != definition:
                    raise RuntimeError("The candidate startup definition differs from its checked package.")
                from .runtime_selection import MetaToolkitRuntimeTarget
                if type(self._candidate_runtime_target(files)) is MetaToolkitRuntimeTarget and application_startup is None:
                    from .deployment import PRODUCT_CONFIGURE_REQUIRED
                    raise DraftConflict(PRODUCT_CONFIGURE_REQUIRED)
                candidate_proof = await asyncio.to_thread(self._verify_candidate_runtime, files)
        failure = None
        try:
            observation = require_available_console_port(self.port)
        except LocalConsolePortUnavailable as error:
            observation, failure = error.observation, error
        # An owned update can change package identity after construction.
        # Bind this actual prelaunch observation to the current source receipt.
        current = read_receipt(self.directory)
        require_bootstrap_history_settled(self.directory, bootstrap_selection=original_bootstrap)
        require_bootstrap_original()
        attempt_id = str(uuid.uuid4())
        # A new child reports its own horizon; until then the stop assumes none.
        self.gateway_shutdown_horizon_ms = None
        journal(self.directory, "Starting your application and bootstrapping its native agents.",
                status="starting", phase="start", startup_attempt_id=attempt_id, startup_console_health=None,
                gateway_shutdown_horizon_ms=None, admission_health=None,
                startup_port_observation={**observation, "attempt_id": attempt_id,
                    "deployment_id": current["deployment_id"], "package_sha256": current["package_sha256"]},
                **({"startup_candidate_runtime_proof": candidate_proof} if candidate_proof is not None else {}),
                health="unknown", health_checked_at=None)
        if failure is not None:
            raise failure
        require_bootstrap_original()
        if (not knowledge_prepared_in_source and current.get("knowledge_managed") is True
                and definition["knowledge"]["mode"] == "local"):
            await self.prepare_knowledge(definition, require_current=require_bootstrap_original)
            require_bootstrap_history_settled(self.directory, bootstrap_selection=original_bootstrap)
            require_bootstrap_original()
            self.require_child_slot()
        read_fd, write_fd = os.pipe()
        environment = {**self.environment, "MOBKIT_LAUNCH_READY_FD": str(write_fd)}
        process = None
        reader = asyncio.StreamReader(limit=MAX_READY_BYTES)
        protocol = asyncio.StreamReaderProtocol(reader)
        transport = None
        pipe = os.fdopen(read_fd, "rb", buffering=0)
        try:
            transport, _ = await asyncio.get_running_loop().connect_read_pipe(lambda: protocol, pipe)
            require_bootstrap_history_settled(self.directory, bootstrap_selection=original_bootstrap)
            require_bootstrap_original()
            process, cancelled = await self.spawn_child(str(self.destination / ".venv/bin/python"), *arguments,
                cwd=self.destination, env=environment, stdout=asyncio.subprocess.PIPE,
                stderr=asyncio.subprocess.STDOUT, pass_fds=(write_fd,))
        except BaseException:
            if transport:
                transport.close()
            else:
                pipe.close()
            raise
        finally:
            os.close(write_fd)
        if process is None:
            if transport:
                transport.close()
            else:
                pipe.close()
            return
        # Fresh intentionally changes its working records and current pointer.
        # After entry, the selected child factory and startup retain that owner;
        # the pre-entry observation cannot veto their legitimate publication.
        output = asyncio.create_task(self.output(process.stdout, "runtime"))
        started = asyncio.create_task(reader.readline())
        exited = asyncio.create_task(self.parent_exit(process))
        stopped = asyncio.create_task(self.stop.wait())
        try:
            if cancelled or self.stop.is_set():
                raise asyncio.CancelledError
            startup_timeout = START_TIMEOUT
            pending_budget_attempted = False
            while True:
                done, _ = await asyncio.wait({started, exited, stopped}, timeout=startup_timeout, return_when=asyncio.FIRST_COMPLETED)
                if stopped in done:
                    raise asyncio.CancelledError
                if exited in done:
                    # A child that refused its source custody said why before exiting.
                    refusal = await self._startup_refusal(started)
                    raise RuntimeError(refusal or f"The application stopped before startup verification (exit {process.returncode}).")
                if started not in done:
                    raise TimeoutError("The application did not finish native startup verification in time. Rebuild a package with the current host if it lacks Forge readiness reporting.")
                startup = started.result()
                if not startup:
                    raise RuntimeError("The application closed its startup verification channel before reporting readiness. Inspect the runtime logs for the native startup failure.")
                refusal = startup_refusal(startup)
                if refusal is not None:
                    raise RuntimeError(refusal)
                try:
                    payload = json.loads(startup)
                except (ValueError, UnicodeError):
                    raise ValueError("The application returned invalid startup evidence. Inspect the runtime logs and rebuild the checked package.") from None
                try:
                    bootstrap, url = readiness(payload, definition, process.pid, self.port)
                except NativeAdmissionPending as pending:
                    journal(self.directory, "Waiting for agent admission to finish. Your application is not ready yet.",
                        status="starting", phase="start", bootstrap=pending.bootstrap,
                        admission_health={"attempt_id": attempt_id, **pending.admission},
                        console_url=None, runtime_pid=process.pid, process_live=True,
                        health="pending", health_checked_at=now())
                    if self.service_owned and not pending_budget_attempted:
                        # Admission may remain pending while the gateway owns
                        # durable sessions. Size a Stop for the unreported
                        # horizon now, still capped by the enforced unit if
                        # widening fails. Heartbeats do not reload the unit.
                        await self.widen_service_stop_budget(None)
                        pending_budget_attempted = True
                    # Each actual child frame renews this bounded wait. A
                    # silent child must fail instead of holding startup open.
                    # This performs no admission or retry.
                    started = asyncio.create_task(reader.readline())
                    startup_timeout = PENDING_HEALTH_WINDOW
                    continue
                break
            horizon_ms = reported_gateway_horizon(payload)
            # Record before use: the host's wait for this worker reads it.
            journal(self.directory, "Native agents finished bootstrap. Checking the actual Console before publishing readiness.",
                    status="starting", phase="start", bootstrap=bootstrap, console_url=None, runtime_pid=process.pid,
                    process_live=True, health="unknown", health_checked_at=now(), admission_health=None,
                    gateway_shutdown_horizon_ms=horizon_ms)
            self.gateway_shutdown_horizon_ms = horizon_ms
            if self.service_owned:
                # An application that reports nothing is sized for the
                # unreported horizon.
                await self.widen_service_stop_budget(horizon_ms)
            import httpx
            async with httpx.AsyncClient(timeout=3, trust_env=False) as client:
                await self.startup_console(client, url, definition, process, exited, stopped)
                if self.stop.is_set() or process.returncode is not None:
                    raise asyncio.CancelledError
                if on_ready:
                    on_ready()
                evidence = self.service_folder_evidence() if self.service_owned else None
                journal(self.directory, "Your application passed native bootstrap and actual Console health.",
                        status="running", phase="run", bootstrap=bootstrap, console_url=url,
                        runtime_pid=process.pid, process_live=True, health="ready", health_checked_at=now(),
                        **({"service_folder_evidence": evidence} if evidence is not None else {}))
                while not self.stop.is_set() and process.returncode is None:
                    done, _ = await asyncio.wait({exited, stopped}, timeout=3, return_when=asyncio.FIRST_COMPLETED)
                    if done:
                        break
                    try:
                        response = await client.get(url.rsplit("/console", 1)[0] + "/console/identities",
                                                    headers=console_health_headers(self.destination))
                        response.raise_for_status()
                        identities = {item["identity"] for item in response.json()["identities"]}
                        expected = {item["identity"] for item in definition["agents"]}
                        healthy = expected.issubset(identities)
                    except Exception:
                        healthy = False
                    update_receipt(self.directory, process_live=process.returncode is None,
                        health="ready" if healthy else "unavailable", health_checked_at=now(),
                        console_url=url if healthy else None)
            if not self.stop.is_set():
                raise RuntimeError(f"The application exited (exit {process.returncode}). Files and native state are preserved.")
        finally:
            started.cancel()
            stopped.cancel()
            if transport:
                transport.close()
            await self.cleanup_child(process, (output, started, exited, stopped), error=sys.exception())


async def _joined_product_mutation(supervisor, function, *args, _owner_current=None, **kwargs):
    """Stop the original mutation fence on cancellation and join its worker."""
    if _owner_current is not None:
        def owner_current():
            try:
                _owner_current()
            except BaseException:
                supervisor.stop.set()
                raise
        owner_current()
        pending = asyncio.create_task(asyncio.to_thread(function, *args, **kwargs))
        try:
            while not pending.done():
                owner_current()
                await asyncio.wait((pending,), timeout=0.05)
            owner_current()
            return pending.result()
        except BaseException:
            supervisor.stop.set()
            while not pending.done():
                try:
                    await asyncio.shield(pending)
                except asyncio.CancelledError:
                    continue
                except Exception:
                    break
            if not pending.cancelled():
                pending.exception()
            raise
    task = asyncio.create_task(asyncio.to_thread(function, *args, **kwargs))
    cancelled = False
    while True:
        try:
            result = await asyncio.shield(task)
            break
        except asyncio.CancelledError:
            supervisor.stop.set()
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


@dataclass(frozen=True)
class _InitialProductFence:
    """Keep the initial owner's source fence and exact artifact observation separate."""
    artifacts: object
    source_current: Callable[[], object]

    def require_source_current(self):
        result = self.source_current()
        if inspect.isawaitable(result):
            if inspect.iscoroutine(result):
                result.close()
            raise RuntimeError("Initial product installation needs a synchronous source fence.")
        if result is False:
            raise RuntimeError("The original initial product source owner changed.")

    def __call__(self):
        self.require_source_current()
        from .runtime_selection import selected_runtime_artifacts
        selected_runtime_artifacts(self.artifacts.selection, verified_artifacts=self.artifacts)


MAX_VENV_CONFIG_BYTES = 64 * 1024


def existing_environment_refusal(venv, running=None):
    """Why an existing application venv cannot be reused by this Forge, or None.

    venv adds only missing links. Over a venv that another CPython created,
    bin/python keeps running that CPython, so pip would install this Forge's
    checked closure with an interpreter that nothing checked. The venv's own
    configuration, links and library folders all name the CPython it holds.
    """
    running = tuple(sys.version_info[:2] if running is None else running)
    unreadable = ("This application's environment does not show which CPython created it, so Forge cannot "
        "safely reuse it. Install into a new empty folder.")
    found = set()
    try:
        descriptor = os.open(venv / "pyvenv.cfg", os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    except FileNotFoundError:
        descriptor = None
    except OSError:
        return unreadable
    if descriptor is not None:
        with os.fdopen(descriptor, "rb") as stream:
            info = os.fstat(stream.fileno())
            content = stream.read(MAX_VENV_CONFIG_BYTES + 1)
        if not stat.S_ISREG(info.st_mode) or len(content) > MAX_VENV_CONFIG_BYTES:
            return unreadable
        for line in content.decode("utf-8", "replace").splitlines():
            key, separator, value = line.partition("=")
            if separator and key.strip() in {"version", "version_info"}:
                version = re.fullmatch(r"([0-9]+)\.([0-9]+)(\.[0-9A-Za-z.+-]*)?", value.strip())
                if version is None:
                    return unreadable
                found.add((int(version[1]), int(version[2])))
    for folder in (venv / "bin", venv / "lib"):
        if folder.is_dir() and not folder.is_symlink():
            for path in folder.iterdir():
                version = re.fullmatch(r"python([0-9]+)\.([0-9]+)", path.name)
                if version is not None:
                    found.add((int(version[1]), int(version[2])))
    others = sorted(found - {running})
    if not others:
        return None
    created = " and ".join(f"CPython {major}.{minor}" for major, minor in others)
    return (f"This application's environment was created by {created}, but this Forge runs CPython "
        f"{running[0]}.{running[1]}, so reusing it would install the application's packages for another "
        f"interpreter. Stop Forge, move only Forge's own .venv folder aside and start it again with PYTHON_BIN "
        f"set to a CPython {others[-1][0]}.{others[-1][1]} interpreter, as the README describes, or install "
        "into a new empty folder. Your saved setups are kept.")


async def _install_selected_product(supervisor, artifacts, require_current, command,
        *, source_custody=None, files=None, report_progress=lambda message: None):
    """Prepare selected bytes using the original child executor and fence.

    Only stdlib venv/site inspection and the existing selected SDK-pair owner
    run here. Neither an authored shell installer nor a native runtime starts.
    """
    from .connect_application_startup import _joined
    from .templates.install_candidate import (owned_directory, install_meta_toolkit_prefix,
        _sdk_location)
    from .product_runtime_custody import (await_product_validation, require_product_source,
        require_product_installation)
    from .deployment import _DestinationSourceObservation, selected_host_refusal, selected_interpreter_refusal
    source_observation = None
    def current(fence=require_current):
        if supervisor.stop.is_set():
            raise asyncio.CancelledError
        result = fence()
        if inspect.isawaitable(result):
            if inspect.iscoroutine(result):
                result.close()
            raise RuntimeError("Selected product installation needs a synchronous owner fence.")
        if result is False:
            raise RuntimeError("The original selected product installation owner changed.")
        if source_custody is not None:
            source_custody.require_owner_current()
    def files_current():
        if source_custody is None:
            return current()
        if supervisor.stop.is_set():
            raise asyncio.CancelledError
        require_product_source(source_custody, source_custody.origin,
            destination=supervisor.destination)
        receipt = read_receipt(supervisor.directory)
        require_product_installation(source_custody, receipt)
        source_observation.require_current(receipt, files)
        if supervisor._candidate_runtime_target(files) != source_custody.origin:
            raise DraftConflict("The original product installation selection changed.")
        if supervisor.stop.is_set():
            raise asyncio.CancelledError
    prefix_current = files_current
    if type(require_current) is _InitialProductFence:
        if source_custody is not None or require_current.artifacts is not artifacts:
            raise RuntimeError("Preserve the initial prefix's exact source and artifact owners.")
        def prefix_current():
            # The prefix writer observes all held artifacts after this source,
            # lease and cancellation fence, once at every original boundary.
            current(require_current.require_source_current)
    current()
    if source_custody is not None:
        require_product_source(source_custody, source_custody.origin,
            destination=supervisor.destination, files=files)
        source_observation = _DestinationSourceObservation(read_receipt(supervisor.directory), files)
        current()
    # The venv below inherits this interpreter, and the native prefix needs
    # the product's declared glibc: refuse either before anything is installed.
    refusal = selected_host_refusal(artifacts.glibc_floor) or selected_interpreter_refusal(artifacts.selection)
    if refusal is not None:
        raise DraftConflict(refusal)
    venv = supervisor.destination / ".venv"
    if not venv.exists() and not venv.is_symlink():
        venv.mkdir(mode=0o700)
    owned_directory(venv, private=True)
    # An existing venv keeps the interpreter that created it: check it before
    # the venv command below reuses it.
    refusal = existing_environment_refusal(venv)
    if refusal is not None:
        raise DraftConflict(refusal)
    current()
    report_progress("Preparing the application environment.")
    await command([sys.executable, "-I", "-m", "venv", str(venv)],
        "Selected SDK environment", CHECK_TIMEOUT)
    current()
    python = venv / "bin/python"
    # With -S, Debian Python can expose its system layout instead of the venv.
    # Select the venv scheme without running installed .pth or sitecustomize.
    site_json = await command([str(python), "-I", "-S", "-c",
        "import json,sysconfig,sys; from pathlib import Path; "
        "base=str(Path(sys.executable).parent.parent); "
        "scheme='posix_venv' if 'posix_venv' in sysconfig.get_scheme_names() else 'posix_prefix'; "
        "print(json.dumps(sysconfig.get_path('purelib',scheme=scheme,vars={'base':base,'platbase':base})))"],
        "Selected SDK site path", CHECK_TIMEOUT, capture_output=True)
    try:
        site = Path(json.loads(site_json))
    except (ValueError, TypeError):
        raise RuntimeError("The selected interpreter did not return one exact SDK site path.") from None
    if source_custody is None:
        await _joined(_sdk_location, supervisor.destination, python, site)
    else:
        await await_product_validation(source_custody, _sdk_location, supervisor.destination, python, site)
    current()
    target = artifacts.selection
    report_progress("Installing the agent software and its dependencies.")
    await supervisor.install_selected_sdk_pair(target.meta_wheel.path, target.toolkit_wheel.path,
        meta=target.meta, toolkit=target.toolkit, site_packages=site, sdk_python=python,
        dependency_requirements=target.dependency_requirements, dependency_wheels=target.dependency_wheels,
        require_current=files_current,
        **({"require_owner_current": current} if source_custody is not None else {}))
    current()
    report_progress("Copying and checking the native runtime files.")
    await _joined_product_mutation(supervisor, install_meta_toolkit_prefix, artifacts,
        deployment_root=supervisor.destination, require_current=prefix_current,
        report_progress=report_progress, _owner_current=current if source_custody is not None else None)
    current()
    if source_custody is not None:
        require_product_source(source_custody, source_custody.origin,
            destination=supervisor.destination, files=files)
        verify_destination(read_receipt(supervisor.directory), files)
        current()
    return {"sdk_python": python, "site_packages": site}


async def update_runtime(supervisor, mode, *, update_id=None, expected_binding=None, update_selection=None):
    """One source transaction, one verified startup, and at most one safe fallback."""
    from . import deployment_updates as updates
    from .deployment_application_bootstrap import require_update_bootstrap_history
    directory = supervisor.directory
    if update_selection is not None:
        selected = updates.read_update_admission(directory, mode, update_selection)
        if update_id is not None and update_id != selected["binding"]["update_id"]:
            raise DraftConflict("The worker selected a different original update admission.")
        update_id, expected_binding = selected["binding"]["update_id"], selected["binding"]
    expected_installation = selected["installation"] if update_selection is not None else None
    replacement = selected.get("application_startup") if update_selection is not None else None
    replacement_observation = None
    receipt, root, update = updates.transaction(directory,
        update_id or read_receipt(directory)["update"]["update_id"], expected_binding=expected_binding, expected_installation=expected_installation)
    # Existing trusted direct callers retain their first checked selection.
    # User-launched workers supply the earlier original admission explicitly.
    expected_binding = updates.update_binding(update) if expected_binding is None else updates.update_binding(expected_binding)
    # After setup, the completed Restore recorded at Stage is the only history
    # admission, and only its owner starts the application. This worker's own
    # transaction may count as lineage while it changes source.
    bootstrap_restore = updates.setup_restore_pin(directory, update)
    require_update_bootstrap_history(directory, bootstrap_restore, in_flight=expected_binding)
    # A helperless application on the selected Meta product starts only with
    # its owner's ordinary startup inputs, so this worker never starts it
    # either: it installs and checks the source, and the owner's Start runs it.
    from .ordinary_application_source import ordinary_installation
    ordinary_owner = (bootstrap_restore is None and replacement is None and ordinary_installation(receipt,
        updates.checked_package(directory, "package.zip", receipt["package_sha256"])[1]))
    owner_start = bootstrap_restore is not None or ordinary_owner

    def current_transaction(*, check_replacement=True):
        require_update_bootstrap_history(directory, bootstrap_restore, in_flight=expected_binding)
        if update_selection is not None:
            retained = updates.read_update_admission(directory, mode, update_selection)
            updates.require_update_binding(retained["binding"], expected_binding)
        transaction = updates.transaction(directory, identity, expected_binding=expected_binding, expected_installation=expected_installation)
        if check_replacement and replacement_observation is not None:
            replacement_observation.require_current()
        return transaction

    async def command(*args, **kwargs):
        current_transaction()
        value = await supervisor.command(*args, **kwargs)
        current_transaction()
        return value

    async def native_definition_revision(*args, **kwargs):
        current_transaction()
        value = await supervisor.native_definition_revision(*args, **kwargs)
        current_transaction()
        return value
    updates.require_application_policy_continuity(directory, update)
    updates.require_replacement_history_settled(directory)
    identity = update["update_id"]
    if owner_start and replacement is not None:
        raise DraftConflict("After setup, a replacement startup cannot follow its completed Restore. No source was changed.")
    if replacement is not None:
        from .deployment_application_startup import selected_inputs, ATTEMPT_NAME
        from .connect_application_startup import _joined
        if mode != "update" or getattr(supervisor, "start_service", None) is not None:
            raise DraftConflict("Selected replacement needs its original owned child; background restart propagation is not qualified.")
        if (root / ATTEMPT_NAME).exists() or (root / ATTEMPT_NAME).is_symlink():
            raise DraftConflict("This replacement has a retained attempt. Reconcile its exact native outcome before startup.")
        _, replacement_observation = await _joined(selected_inputs, replacement, directory=directory,
            binding=expected_binding, installation=expected_installation, retain_observation=True)
        current_transaction()
    activated = False
    ready = False
    rollback = mode == "rollback"
    recover_current = mode == "recover-current"
    native_update = update.get("native_definition_update") is True
    host_instruction_update = update.get("host_instruction_update") is True
    dependency_update = update.get("runtime_dependency_update")
    verification_kwargs = {}
    if dependency_update and dependency_update.get("runtime_candidate") is not None:
        from .runtime_selection import parse_candidate_selection
        verification_kwargs["runtime_target"] = parse_candidate_selection(dependency_update["runtime_candidate"])
    from .runtime_selection import MetaToolkitRuntimeTarget
    product_update = type(verification_kwargs.get("runtime_target")) is MetaToolkitRuntimeTarget
    if product_update and (replacement is None or replacement_observation is None):
        raise DraftConflict("The selected Meta/Toolkit update requires its original trusted replacement startup before changing runtime files.")
    registry_update = bool(dependency_update and dependency_update.get("native_registry_update") is True)
    # A source-copy interruption before native startup can restore its snapshot.
    # Once admission is possible, canonical native/domain receipts never rewind.
    restore_data = rollback and update.get("runtime_admission_possible") is False and (
        update.get("rollback_kind") == "failed-update" or update["status"] in {"applying", "verifying", "recovery-needed"})

    def publish(restored, *, started=True):
        nonlocal ready
        current = read_receipt(directory)
        _, _, bound = current_transaction()
        expected_hash = bound["previous_package_sha256"] if restored else bound["package_sha256"]
        expected_build = bound["from_build_id"] if restored else bound["to_build_id"]
        if current["package_sha256"] != expected_hash or current["build_id"] != expected_build:
            raise DraftConflict("The started source no longer matches the admitted update transaction.")
        _, source = updates.checked_package(directory, "package.zip", current["package_sha256"])
        verify_destination(current, source)
        updates.committed(directory, identity, restored=restored, started=started, setup=not ordinary_owner,
            expected_binding=expected_binding, expected_installation=expected_installation)
        ready = True

    async def observe_native_revision():
        current_receipt = read_receipt(directory)
        _, current_files = updates.checked_package(directory, "package.zip", current_receipt["package_sha256"])
        _, current_root, current = current_transaction()
        updates._require_runtime_framework_configuration(current_receipt, current)
        # A cold native connection can restore due work. Do not rewind native
        # or application receipts after even this read becomes possible.
        updates.checkpoint(directory, current_root, current, expected_binding=expected_binding, expected_installation=expected_installation, runtime_admission_possible=True,
            detail="Observing this exact owned native definition and runtime capability before any configuration replacement.")
        # Ordinary instruction updates retain the strict carrier contract.
        # A host-only refresh or model-only crossing may observe the legacy
        # initial epoch without that capability, enforced by the reader.
        observation = (await native_definition_revision(current_files)
            if native_update and not registry_update else
            await native_definition_revision(current_files, require_carrier=False))
        _, current_root, current = current_transaction()
        updates.checkpoint(directory, current_root, current, expected_binding=expected_binding, expected_installation=expected_installation, native_definition_observation=observation)
        return observation

    async def checked_start(files, restored, observation=None):
        definition = json.loads(files["deployment.json"][0])
        journal(directory, "Checking the installed application's current private configuration and source contract.",
                status="checking", phase="recovery" if recover_current else "update",
                startup_attempt_id=None, startup_console_health=None, health="unknown", health_checked_at=None)
        verify_destination(read_receipt(directory), files)
        _, root, current = current_transaction()
        # Application-check loads ordinary authored callbacks. Native startup
        # then restores schedules before readiness. Neither domain nor native
        # receipts may rewind once application execution is possible.
        updates.checkpoint(directory, root, current, expected_binding=expected_binding, expected_installation=expected_installation, runtime_admission_possible=True,
                           detail="Loading verified application source. Any native or application activity recorded from this point will be retained during source recovery.")
        # Authored contract checking can write local state. Exclude another
        # native host while checking; the runtime acquires the same lease itself.
        with project_lock(supervisor.destination) as project_lease:
            if product_update and not restored:
                from .runtime_selection import selected_runtime_artifacts, parse_candidate_selection
                from .runtime_dependencies import retain_runtime, verify_runtime
                from .templates.install_candidate import verify_meta_toolkit_prefix
                from .connect_application_startup import _joined
                target = verification_kwargs["runtime_target"]
                artifacts = await _joined(selected_runtime_artifacts, target)

                def require_product_owner_current():
                    if supervisor.stop.is_set():
                        raise asyncio.CancelledError
                    project_lease.require_current()
                    current_transaction()
                    selected_runtime_artifacts(target, verified_artifacts=artifacts)
                    if supervisor.stop.is_set():
                        raise asyncio.CancelledError

                require_product_owner_current()
                updates._require_runtime_framework_configuration(read_receipt(directory), current)
                from .templates.install_candidate import verify_selected_application_dependencies
                await _joined(verify_selected_application_dependencies,
                    files.get("requirements.application.txt", (b"", 0o644))[0], artifacts)
                require_product_owner_current()
                if any(read_settings(supervisor.destination).get(key) for key in (
                        "MOBKIT_RPC_GATEWAY_BIN", "MOBKIT_GATEWAY_BIN", "META_FORGE_RPC_GATEWAY_BIN")):
                    raise RuntimeError("The selected Meta/Toolkit product cannot use a native gateway override.")
                # Keep the application's original selected startup. Its gateway
                # already exists when admission freezes it; do not replace its
                # path with an unadmitted fresh-install startup after the copy.
                gateway = Path(replacement["gateway"]["path"])
                if (replacement["gateway"]["sha256"] != artifacts.candidate["binary"]["sha256"]
                        or gateway.name != "toolkit_gateway" or gateway.parent.name != "bin"):
                    raise DraftConflict("The original replacement gateway differs from the selected product.")
                selected_prefix = gateway.parent.parent
                if selected_prefix.is_relative_to(supervisor.destination / ".bin"):
                    raise DraftConflict("The original replacement gateway would move during runtime retention. Select its existing retained product prefix before this update.")
                await _joined(verify_meta_toolkit_prefix, artifacts,
                    deployment_root=supervisor.destination, prefix_path=selected_prefix)
                require_product_owner_current()
                _, previous_files = updates.checked_package(root, "previous.zip", current["previous_package_sha256"])
                predecessor = dependency_update.get("previous_runtime_candidate")
                previous_target = parse_candidate_selection(predecessor) if predecessor is not None else None
                previous_source = ({name: previous_files[name][0] for name in
                    ("deployment.json", "runtime-candidate.json")} if previous_target is not None else None)
                retained = await _joined_product_mutation(supervisor, retain_runtime, supervisor.destination, root,
                    expected_version=dependency_update["from_version"], runtime_target=previous_target,
                    next_runtime_target=target, previous_source=previous_source,
                    require_current=require_product_owner_current)
                require_product_owner_current()
                _, root, current = current_transaction()
                updates.checkpoint(directory, root, current, expected_binding=expected_binding,
                    expected_installation=expected_installation, runtime_files_retained=retained,
                    rollback_available=False,
                    detail="Previous runtime bytes are retained. Installing the admitted exact Meta/Toolkit SDK pair, offline dependencies and complete native prefix.")
                verification_kwargs.update(await _install_selected_product(supervisor, artifacts,
                    require_product_owner_current, command))
                verify_destination(read_receipt(directory), files)
                proof = await _joined(verify_runtime, supervisor.destination,
                    dependency_update["to_version"], **verification_kwargs)
                require_product_owner_current()
                _, root, current = current_transaction()
                updates.checkpoint(directory, root, current, expected_binding=expected_binding,
                    expected_installation=expected_installation,
                    runtime_artifacts={"sdk": proof["sdk"], "native": proof},
                    detail="Selected SDK and complete native prefix bytes match their admitted inputs. The existing native replacement owner still decides application activation.")
                await _joined(verify_meta_toolkit_prefix, artifacts,
                    deployment_root=supervisor.destination, prefix_path=selected_prefix)
                require_product_owner_current()
            elif dependency_update and not restored:
                updates._require_runtime_framework_configuration(read_receipt(directory), current)
                from .runtime_dependencies import retain_runtime, verify_runtime
                from .templates.storage_setup import upgrade_storage
                if any(read_settings(supervisor.destination).get(key) for key in (
                        "MOBKIT_RPC_GATEWAY_BIN", "MOBKIT_GATEWAY_BIN")):
                    raise RuntimeError("This qualified dependency update cannot use an external native gateway override. Preserve current settings and runtime files.")
                # install.sh ends with an authored application check. Retain
                # old runtime bytes first, then execute only the accepted new
                # pair. No old-runtime fallback is qualified after this point.
                retained = (await asyncio.to_thread(retain_runtime, supervisor.destination, root,
                    expected_version=dependency_update["from_version"]) if dependency_update["from_version"] != "0.8.42"
                    else await asyncio.to_thread(retain_runtime, supervisor.destination, root))
                _, root, current = current_transaction()
                updates.checkpoint(directory, root, current, expected_binding=expected_binding, expected_installation=expected_installation, runtime_files_retained=retained,
                    rollback_available=False,
                    detail="Previous runtime files are retained. Installing the accepted runtime while preserving private settings and all current work.")
                await command(["/bin/bash", "install.sh", "--non-interactive"],
                    "Runtime dependency installation", INSTALL_TIMEOUT, installer=True)
                verify_destination(read_receipt(directory), files)
                proof = await asyncio.to_thread(verify_runtime, supervisor.destination,
                    dependency_update["to_version"], **verification_kwargs)
                if verification_kwargs:
                    sdk = proof["sdk"]
                else:
                    text = await command([str(supervisor.destination / ".venv/bin/python"), "-c",
                        "import importlib.metadata,json; print(json.dumps({'package':'meerkat-mobkit','version':importlib.metadata.version('meerkat-mobkit')}))"],
                        "Installed SDK version", CHECK_TIMEOUT, capture_output=True)
                    try:
                        sdk = json.loads(text)
                    except (ValueError, TypeError):
                        raise RuntimeError("The installed SDK did not provide one unambiguous package version.") from None
                    if sdk != {"package": "meerkat-mobkit", "version": dependency_update["to_version"]}:
                        raise RuntimeError("The installed SDK does not match the accepted native runtime. No storage upgrade or startup was attempted.")
                _, root, current = current_transaction()
                updates.checkpoint(directory, root, current, expected_binding=expected_binding, expected_installation=expected_installation, runtime_artifacts={"sdk": sdk, "native": proof},
                    detail=("Installed exact SDK payload, metadata and both candidate native receipts match the accepted host selection. Native storage confirmation remains required."
                        if verification_kwargs else "Installed SDK version and both official native artifact receipts match the accepted tuple. Native storage confirmation remains required."))
                marker = json.loads((supervisor.destination / ".mobkit/launch-storage.json").read_text())
                if isinstance(marker, dict) and marker.get("version") == dependency_update["from_version"]:
                    def require_storage_owner_current():
                        if supervisor.stop.is_set():
                            raise asyncio.CancelledError
                        project_lease.require_current()
                        current_transaction()
                        if supervisor.stop.is_set():
                            raise asyncio.CancelledError

                    storage_task = asyncio.create_task(asyncio.to_thread(upgrade_storage,
                        supervisor.destination, dependency_update["from_version"],
                        dependency_update["to_version"],
                        require_owner_current=require_storage_owner_current))
                    interrupted = False
                    # Cancellation stops admission of another native apply.
                    # The entered thread must settle under this original lease.
                    while True:
                        try:
                            storage = await asyncio.shield(storage_task)
                            break
                        except asyncio.CancelledError:
                            if storage_task.cancelled():
                                raise
                            supervisor.stop.set()
                            interrupted = True
                        except BaseException:
                            if interrupted:
                                raise asyncio.CancelledError
                            raise
                    if interrupted:
                        raise asyncio.CancelledError
                    _, root, current = current_transaction()
                    updates.checkpoint(directory, root, current, expected_binding=expected_binding, expected_installation=expected_installation, runtime_storage_upgrade=storage)
                elif not updates._same_toml(marker, {"format": 1, "project": definition["slug"],
                                "version": dependency_update["to_version"], "phase": "ready"}):
                    raise RuntimeError("The retained storage marker does not match this exact runtime transition. Preserve its native reports and recover the accepted runtime.")
                # A ready new marker can survive an interruption before the
                # checkpoint above. Ordinary startup re-inspects native truth;
                # it does not repeat an already completed native apply.
            # A product owner's doctor needs that owner's startup, as at install.
            checks = [("Updated application contract", "--application-check")] if product_update or owner_start else [
                ("Update prerequisites", "--doctor"), ("Updated application contract", "--application-check")]
            for name, flag in checks:
                verify_destination(read_receipt(directory), files)
                await command([str(supervisor.destination / ".venv/bin/python"), "app.py", flag], name, CHECK_TIMEOUT)
        if supervisor.stop.is_set():
            raise asyncio.CancelledError
        verify_destination(read_receipt(directory), files)
        if dependency_update and not restored:
            if any(read_settings(supervisor.destination).get(key) for key in (
                    "MOBKIT_RPC_GATEWAY_BIN", "MOBKIT_GATEWAY_BIN")):
                raise RuntimeError("The selected native gateway changed during the qualified dependency update. Preserve state and recover the accepted runtime.")
            if await asyncio.to_thread(verify_runtime, supervisor.destination,
                    dependency_update["to_version"], **verification_kwargs) != proof:
                raise RuntimeError("The verified native runtime changed before startup. No runtime was started.")
        if native_update and replacement is None:
            expected_revision = observation.get("definition_revision") if isinstance(observation, dict) else None
            if type(expected_revision) is not int:
                raise RuntimeError("Native definition activation needs the actual revision observed for this exact attempt.")
            _, root, current = current_transaction()
            source_hash = read_receipt(directory)["package_sha256"]
            if observation.get("package_sha256") == source_hash and not observation.get("activation_replay"):
                # Ordinary connection verified this exact declared config
                # against native truth. Redeclaring it at its current epoch
                # would correctly fail native AlreadyMatching semantics.
                updates.checkpoint(directory, root, current, expected_binding=expected_binding, expected_installation=expected_installation, native_definition_recovery_observation=observation,
                    detail="The exact installed configuration already matches the observed canonical native definition. No duplicate activation was requested.")
            else:
                updates.checkpoint(directory, root, current, expected_binding=expected_binding, expected_installation=expected_installation, native_activation_expected_revision=expected_revision,
                    native_activation_package_sha256=source_hash, native_activation_observation=observation,
                    detail=("Removing the exact legacy duplicate model declaration through the accepted native revision update. Models, identities and continuing histories are preserved."
                        if registry_update else "Activating the accepted native instruction and flow-message configuration against its actually observed revision. Continuing histories are preserved."))
                updates._require_runtime_framework_configuration(read_receipt(directory), current)
                await command([str(supervisor.destination / ".venv/bin/python"), "app.py",
                                          "--activate-revision", str(expected_revision)],
                                         "Native definition activation", START_TIMEOUT)
                verify_destination(read_receipt(directory), files)
                # This exact registry-only successor keeps every profile
                # instruction unchanged. Bind its observed epoch without
                # requiring the separate host instruction update carrier.
                updates._require_runtime_framework_configuration(read_receipt(directory), current)
                observed = (await native_definition_revision(files,
                    require_carrier=False, expected_revision=expected_revision + 1)
                    if registry_update else await native_definition_revision(files))
                if observed["definition_revision"] <= expected_revision:
                    raise RuntimeError("Native definition activation did not produce an observed later revision. No adoption was inferred from source files.")
                if registry_update and observed["definition_revision"] != expected_revision + 1:
                    raise RuntimeError("The model registry activation did not converge its exact successor revision. Preserve history and recover the accepted runtime.")
                _, root, current = current_transaction()
                updates.checkpoint(directory, root, current, expected_binding=expected_binding, expected_installation=expected_installation, native_definition_activation={**observed,
                    "expected_revision": expected_revision, "restored_source": restored,
                    "instruction_adoption": ("Only the exact legacy duplicate model metadata was removed. Selected models and all profile instructions remain unchanged."
                        if registry_update else "Only subsequent new ordinary host send or keyed dispatch authors the current role into its existing session. Same-key retries replay their original admission. Native bootstrap alone is not role adoption.")})
            if restored and not updates.has_host_instruction_carrier(files):
                raise RuntimeError("Previous source and canonical configuration were restored as a new observed native revision, but its legacy host cannot restore continuing role instructions. Existing sessions and all current private records are preserved; recover the current accepted application rather than claiming role rollback.")
        updates._require_runtime_framework_configuration(read_receipt(directory), current)
        current_transaction()
        if owner_start:
            # The owner's next Start runs this checked source; a set-up
            # application restores its completed setup with it. No worker
            # child selects bootstrap or ordinary startup inputs.
            publish(restored, started=False)
        elif replacement is None:
            await supervisor.start(definition, on_ready=lambda: publish(restored))
        else:
            from .deployment_application_startup import selected_inputs, retain_attempt
            from .connect_application_startup import _joined
            _, observed = await _joined(selected_inputs, replacement, directory=directory,
                binding=expected_binding, installation=expected_installation, retain_observation=True)
            current_transaction()
            observed.require_current()
            attempt = retain_attempt(directory, update_selection, selected)
            updates.checkpoint(directory, root, current,
                expected_binding=expected_binding, expected_installation=expected_installation,
                application_replacement_attempt=attempt.pin(),
                detail="Starting the original selected replacement. Native policy publication and member settlement remain separate from Console health.")
            await supervisor.start(definition, on_ready=lambda: publish(restored),
                application_startup={"mode": mode, "admission": update_selection, "attempt": attempt})

    try:
        # Let the owned stdin/signal task observe cancellation before any copy.
        await asyncio.sleep(0)
        if supervisor.stop.is_set():
            raise asyncio.CancelledError
        if product_update and not rollback:
            from .runtime_selection import selected_runtime_artifacts
            from .templates.install_candidate import verify_selected_application_dependencies
            from .connect_application_startup import _joined
            _, selected_root, selected_update = current_transaction()
            _, selected_files = updates.checked_package(selected_root, "candidate.zip", selected_update["package_sha256"])
            artifacts = await _joined(selected_runtime_artifacts, verification_kwargs["runtime_target"])
            await _joined(verify_selected_application_dependencies,
                selected_files.get("requirements.application.txt", (b"", 0o644))[0], artifacts)
            current_transaction()
            if supervisor.stop.is_set():
                raise asyncio.CancelledError
        if before_change := getattr(supervisor, "before_source_change", None):
            await before_change()
        current_transaction()
        if dependency_update and rollback:
            raise DraftConflict("This runtime dependency crossing supports recovery with the current accepted runtime. Previous runtime files are retained; a downgrade is not qualified.")
        if native_update and rollback and update.get("runtime_admission_possible") is not False:
            _, previous_files = updates.checked_package(root, "previous.zip", update["previous_package_sha256"])
            if not updates.has_host_instruction_carrier(previous_files):
                raise DraftConflict("This previous application's legacy host cannot restore continuing role instructions after native admission. Keep current state and recover the current accepted application; no rollback source or canonical definition was changed.")
        observation = None
        if registry_update and recover_current:
            # The original observation came from the verified predecessor
            # before replacing its runtime. Keep its exact epoch for CAS
            # replay after a storage crossing; a new ordinary connection
            # cannot observe an as-yet unpinned candidate configuration.
            # The current pair is independently reverified in checked_start.
            previous_observation = update.get("native_definition_observation")
            if (not isinstance(previous_observation, dict)
                    or previous_observation.get("package_sha256") != update["previous_package_sha256"]
                    or previous_observation.get("native_binary_version") != dependency_update["from_version"]
                    or type(previous_observation.get("definition_revision")) is not int
                    or previous_observation["definition_revision"] < 1
                    or not re.fullmatch("[0-9a-f]{64}", str(previous_observation.get("native_binary_sha256", "")))):
                raise RuntimeError("The runtime registry update has no bound predecessor native revision. Preserve its source and history; no epoch was guessed.")
            _, previous_files = updates.checked_package(root, "previous.zip", update["previous_package_sha256"])
            if previous_observation.get("mob_id") != json.loads(previous_files["deployment.json"][0])["slug"]:
                raise RuntimeError("The retained native revision belongs to another application. No activation was requested.")
            original_pin = update.get("native_activation_expected_revision")
            if original_pin is not None and (type(original_pin) is not int
                    or original_pin != previous_observation["definition_revision"]
                    or update.get("native_activation_package_sha256") != update["package_sha256"]
                    or not updates._same_toml(update.get("native_activation_observation"), previous_observation)):
                raise RuntimeError("The registry activation pin changed after its original observation. Preserve the accepted update.")
            observation = {**previous_observation, "activation_replay": original_pin is not None}
        elif native_update and recover_current:
            # An interrupted activation may have advanced native truth or may
            # have lost its response before admission. The exact original CAS
            # pin converges both cases; a candidate-only ordinary connection
            # cannot observe the latter because its config is still unpinned.
            previous_observation = update.get("native_activation_observation")
            carrier = previous_observation.get("host_system_prompt") if isinstance(previous_observation, dict) else None
            if (receipt["package_sha256"] in {update["package_sha256"], update["previous_package_sha256"]}
                    and update.get("native_activation_package_sha256") == receipt["package_sha256"]
                    and isinstance(previous_observation, dict)
                    and type(update.get("native_activation_expected_revision")) is int
                    and previous_observation.get("definition_revision") == update["native_activation_expected_revision"]
                    and isinstance(carrier, dict) and all(carrier.get(method) is True for method in ("send", "dispatch"))):
                _, current_files = updates.checked_package(directory, "package.zip", receipt["package_sha256"])
                verify_destination(receipt, current_files)
                artifact = supervisor.native_binary_provenance(current_files)
                if any(previous_observation.get(key) != value for key, value in artifact.items()):
                    raise RuntimeError("The owned native runtime changed after this exact activation attempt. Re-observe its canonical configuration before recovery.")
                if receipt["package_sha256"] == update["package_sha256"]:
                    observation = {**previous_observation, "activation_replay": True}
                else:
                    # A fallback CAS may also lose its response after copying
                    # previous source. Converge that exact recorded attempt
                    # before its ordinary connection can read native truth.
                    # Do not start or claim role adoption by the legacy host.
                    await command([str(supervisor.destination / ".venv/bin/python"), "app.py",
                                              "--activate-revision", str(update["native_activation_expected_revision"])],
                                             "Interrupted fallback definition activation", START_TIMEOUT)
                    verify_destination(read_receipt(directory), current_files)
                    observation = await native_definition_revision(current_files)
                    if observation["definition_revision"] <= update["native_activation_expected_revision"]:
                        raise RuntimeError("Interrupted fallback activation did not produce an observed later native revision.")
                    _, root, current = current_transaction()
                    updates.checkpoint(directory, root, current, expected_binding=expected_binding, expected_installation=expected_installation, native_definition_observation=observation,
                        native_definition_activation={**observation, "expected_revision": update["native_activation_expected_revision"],
                            "restored_source": True, "instruction_adoption": "Only canonical fallback configuration converged. No previous application was started and no continuing role restoration is claimed."},
                        detail="Converged the exact interrupted fallback canonical activation. Retained all current history; proceeding forward to the accepted application using the newly observed native revision.")
        if replacement is None and observation is None and (not dependency_update or registry_update) and (native_update or host_instruction_update):
            observation = await observe_native_revision()
        if replacement is None and native_update and not observation.get("activation_replay"):
            _, root, current = current_transaction()
            updates.checkpoint(directory, root, current, expected_binding=expected_binding, expected_installation=expected_installation,
                native_activation_expected_revision=observation["definition_revision"],
                native_activation_package_sha256=(current["previous_package_sha256"] if rollback else current["package_sha256"]),
                native_activation_observation=observation,
                detail="Bound the exact target configuration to its observed native revision before source replacement. This original CAS pin remains usable after an interrupted source check or activation response loss.")
        current_transaction()
        activation_kwargs = ({"require_owner_current": replacement_observation.require_current}
            if replacement_observation is not None else {})
        files = (updates.retry_candidate(directory, identity, expected_binding=expected_binding, expected_installation=expected_installation) if recover_current else
                 updates.restore(directory, identity, restore_data=restore_data, expected_binding=expected_binding, expected_installation=expected_installation) if rollback else updates.activate(directory, identity, expected_binding=expected_binding, expected_installation=expected_installation, **activation_kwargs))
        activated = True
        await checked_start(files, rollback, observation)
    except BaseException as error:
        _, root, current = current_transaction(check_replacement=False)
        if replacement is not None:
            from .deployment_application_startup import ATTEMPT_NAME
            if ((root / ATTEMPT_NAME).exists() or (root / ATTEMPT_NAME).is_symlink()
                    or activated or current["status"] in {"snapshotting", "applying", "verifying"}):
                updates.checkpoint(directory, root, current,
                    expected_binding=expected_binding, expected_installation=expected_installation,
                    status="recovery-needed", rollback_available=False,
                    detail="The selected replacement needs reconciliation. Retained source and any original native attempt remain intact; no ordinary fallback, policy reset or replacement replay was performed.")
            # Selected startup cannot fall through to an ordinary host or its
            # cold native observation after a rejected owner/input fence.
            raise
        if (dependency_update and not ready and current.get("runtime_admission_possible") is not False
                and (activated or current["status"] in {"verifying", "recovering-current", "recovery-needed"})):
            detail = supervisor.redact(str(error))[:1500] if isinstance(error, (ValueError, RuntimeError, TimeoutError)) else type(error).__name__
            updates.checkpoint(directory, root, current, expected_binding=expected_binding, expected_installation=expected_installation, status="recovery-needed", rollback_available=False,
                recovery_error=detail,
                detail="The runtime upgrade has not passed storage confirmation and live native Console health. Current source, private settings, admitted work and previous runtime files are retained. Recover this exact accepted runtime; no downgrade or automatic request resend was performed.")
            raise
        if not ready and (activated or current["status"] in {
                "applying", "verifying", "restoring", "recovering-current"}):
            if before_restore := getattr(supervisor, "before_source_restore", None):
                try:
                    await before_restore()
                except BaseException as cleanup_error:
                    _, root, current = current_transaction()
                    cleaned = read_receipt(directory).get("service_cleanup_confirmed") is True
                    updates.checkpoint(directory, root, current, expected_binding=expected_binding, expected_installation=expected_installation, status="recovery-needed", rollback_available=True,
                        detail=("The owned service confirmed cleanup, but source recovery prerequisites or failed-attempt diagnostic preservation could not be verified."
                                if cleaned else "The owned service has not confirmed cleanup.")
                               + " Installed source and all current native and application records are preserved; no fallback source was copied.")
                    reason = "source recovery prerequisites or failed-attempt diagnostic preservation could not be verified" if cleaned else "background service cleanup is unconfirmed"
                    raise RuntimeError("The update needs recovery because " + reason + ". No source fallback was performed.") from cleanup_error
                _, root, current = current_transaction()
        if recover_current:
            if not ready and (activated or current["status"] == "recovering-current"):
                detail = supervisor.redact(str(error))[:1500] if isinstance(error, (ValueError, RuntimeError, TimeoutError)) else type(error).__name__
                updates.checkpoint(directory, root, current, expected_binding=expected_binding, expected_installation=expected_installation, status="recovery-needed", rollback_available=True,
                    recovery_error=detail,
                    detail="The current application recovery has not passed native bootstrap and live Console health. Latest native and application records, including any recovery activity, are retained; retry this exact candidate or restore the previous source.")
            # An explicit forward recovery never switches to another source or
            # rewinds state after a failed check/start or owner interruption.
            raise
        # A late runtime failure after verified publication is an operational
        # failure, not permission to rewind work that may have used live tools.
        if ready or not (activated or current["status"] in {"applying", "verifying", "restoring"}):
            raise
        cancelled = isinstance(error, asyncio.CancelledError) or supervisor.stop.is_set()
        detail = supervisor.redact(str(error))[:1500] if isinstance(error, (ValueError, RuntimeError, TimeoutError)) else type(error).__name__
        recovery_start_possible = False
        try:
            # Observe before restoring source. Copying a previous TOML file is
            # never a rollback of the canonical native definition. If the
            # current source cannot observe an uncertain activation, retain it
            # and require explicit recovery rather than guessing an epoch.
            recovery_observation = await observe_native_revision() if native_update else None
            if rollback and current.get("rollback_kind") == "manual":
                files = updates.recover_candidate(directory, identity, expected_binding=expected_binding, expected_installation=expected_installation)
                restored = False
            else:
                files = updates.restore(directory, identity, restore_data=current.get("runtime_admission_possible") is False,
                                        rollback_kind="failed-update", expected_binding=expected_binding, expected_installation=expected_installation)
                restored = True
            _, root, current = current_transaction()
            updates.checkpoint(directory, root, current, expected_binding=expected_binding, expected_installation=expected_installation, activation_error=detail)
            if cancelled:
                updates.checkpoint(directory, root, current, expected_binding=expected_binding, expected_installation=expected_installation, status="restored-stopped" if restored else "candidate-restored-stopped",
                    rollback_available=not restored,
                    detail="Verified source was restored after interruption. Current native and application state is retained. No restart or native health check has completed.",
                    activation_error=detail)
                raise asyncio.CancelledError
            journal(directory, "The requested source did not pass startup. Restored verified bytes; checking their actual runtime before claiming recovery.",
                    status="checking", phase="recovery")
            recovery_start_possible = True
            await checked_start(files, restored, recovery_observation)
        except asyncio.CancelledError:
            if recovery_start_possible and (before_restore := getattr(supervisor, "before_source_restore", None)):
                # A cancellation can arrive during the second service start,
                # after the first attempted instance was already cleaned up.
                await before_restore()
            raise
        except Exception as recovery_error:
            if recovery_start_possible and (before_restore := getattr(supervisor, "before_source_restore", None)):
                try:
                    await before_restore()
                except BaseException as cleanup_error:
                    recovery_error = cleanup_error
            _, root, current = current_transaction()
            recovery_detail = (supervisor.redact(str(recovery_error))[:1500]
                if isinstance(recovery_error, (ValueError, RuntimeError, TimeoutError)) else type(recovery_error).__name__)
            updates.checkpoint(directory, root, current, expected_binding=expected_binding, expected_installation=expected_installation, status="recovery-needed", rollback_available=True,
                activation_error=detail, recovery_error=recovery_detail,
                detail="The update did not pass startup and recovery has not passed native health. Current native and application state, previous packages and private backups are retained; stop and recover this exact transaction.")
            raise RuntimeError("The source update needs recovery. Its owned package and private state snapshots are preserved; no running-system recovery is claimed.") from recovery_error


async def owner_input(supervisor):
    reader = asyncio.StreamReader()
    await asyncio.get_running_loop().connect_read_pipe(lambda: asyncio.StreamReaderProtocol(reader), sys.stdin.buffer)
    stopped = False
    while True:
        line = await reader.readline()
        if not line:
            from .deployment import worker_stop_timeout
            end_after_owner_loss(worker_stop_timeout(supervisor.directory))
            supervisor.stop.set()
            return "stopped" if stopped else "owner_lost"
        try:
            if json.loads(line).get("action") == "stop":
                # Keep reading: the owner can still be lost during this stop.
                supervisor.stop.set()
                stopped = True
        except (ValueError, AttributeError):
            continue


async def run(directory, mode, *, update_selection=None, bootstrap_selection=None,
              bootstrap_lifecycle_id=None, bootstrap_owner_source_sha256=None):
    if mode not in {"install", "start", "service", "update", "rollback", "recover-current"}:
        raise ValueError("Unknown deployment worker mode.")
    original_bootstrap = deepcopy(bootstrap_selection)
    if original_bootstrap is not None and (mode not in {"start", "service"} or update_selection is not None):
        raise ValueError("Bootstrap needs its original start-only application admission.")
    if bootstrap_lifecycle_id is not None and original_bootstrap is None:
        raise ValueError("An original bootstrap lifecycle requires its selected bootstrap admission.")
    if bootstrap_owner_source_sha256 is not None and original_bootstrap is None:
        raise ValueError("An original source owner requires its selected bootstrap admission.")
    bootstrap_selected = bootstrap_observation = None
    if original_bootstrap is not None:
        from .deployment_application_bootstrap import (
            bootstrap_lifecycle_binding, observe_installed_bootstrap, require_bootstrap_admission_current,
            require_bootstrap_history_settled,
        )
        original_receipt = read_receipt(directory)
        if (bootstrap_lifecycle_id is not None
                and original_receipt.get("interactive_lifecycle_id") != bootstrap_lifecycle_id):
            raise DraftConflict("The original wrapper bootstrap lifecycle changed before worker entry.")
        original_lifecycle = bootstrap_lifecycle_binding(original_receipt)
        def require_selected_lifecycle():
            if bootstrap_lifecycle_binding(read_receipt(directory)) != original_lifecycle:
                raise DraftConflict("The original bootstrap lifecycle changed during source admission.")
        bootstrap_selected, bootstrap_observation, bootstrap_owner_source = await observe_installed_bootstrap(
            directory, original_bootstrap, require_current=require_selected_lifecycle,
            owner_source_sha256=bootstrap_owner_source_sha256)
        require_bootstrap_admission_current(directory, original_bootstrap,
            bootstrap_selected, bootstrap_observation)
        if bootstrap_lifecycle_binding(read_receipt(directory)) != original_lifecycle:
            raise DraftConflict("The original bootstrap worker lifecycle changed before supervisor admission.")
        require_bootstrap_history_settled(directory, bootstrap_selection=original_bootstrap)
    from .deployment_application_bootstrap import (require_bootstrap_history_settled,
        require_update_bootstrap_history)
    if update_selection is None:
        require_bootstrap_history_settled(directory, bootstrap_selection=original_bootstrap)
    expected = None
    if update_selection is not None:
        from .deployment_updates import read_update_admission, transaction, require_recovery_source
        expected = read_update_admission(directory, mode, update_selection)
        receipt, _, staged = transaction(directory, expected["binding"]["update_id"],
                                    expected_binding=expected["binding"], expected_installation=expected["installation"])
        # After setup, only the completed Restore recorded at Stage admits it.
        from .deployment_updates import setup_restore_pin
        require_update_bootstrap_history(directory, setup_restore_pin(directory, staged),
            in_flight=expected["binding"])
        require_recovery_source(directory, expected)
    if mode in {"update", "rollback", "recover-current"}:
        if expected is None:
            raise ValueError("The update worker has no original retained admission.")
        admission = receipt.get("update_action") or {}
        if (admission.get("action") != ("apply-update" if mode == "update" else mode)
                or admission.get("update_id") != expected["binding"]["update_id"]):
            raise ValueError("The original user-admitted update action changed.")

    initial_product = initial_binding = initial_observation = initial_files = None
    initial_source_observation = None

    def require_original_source():
        if original_bootstrap is not None:
            from .deployment_application_bootstrap import require_bootstrap_admission_current
            require_bootstrap_admission_current(directory, original_bootstrap,
                bootstrap_selected, bootstrap_observation)
        if initial_product is not None:
            from .deployment import DeploymentService
            from .deployment_application_bootstrap import require_bootstrap_history_settled
            from .deployment_updates import require_replacement_history_settled
            from .connect_application_update_startup import _file_identity
            from .connect_installer_caller import _private_directory
            current = read_receipt(directory)
            if current.get("update") is not None:
                raise DraftConflict("Use the original selected product update or recovery owner; initial installation cannot replace it.")
            require_bootstrap_history_settled(directory)
            require_replacement_history_settled(directory)
            if (DeploymentService._configuration_binding(current) != initial_binding
                    or any(_private_directory(path) != identity for path, identity in initial_observation[0])
                    or any(_file_identity(path) != identity for path, identity in initial_observation[1])):
                raise DraftConflict("The original selected product installation or source changed.")
            if initial_source_observation is None:
                verify_destination(current, initial_files, allow_missing=True)
            else:
                initial_source_observation.require_current(current, initial_files)
        if expected is not None:
            from .deployment_updates import read_update_admission, transaction, require_recovery_source
            read_update_admission(directory, mode, update_selection)
            transaction(directory, expected["binding"]["update_id"],
                        expected_binding=expected["binding"], expected_installation=expected["installation"])
            require_recovery_source(directory, expected)
    def require_original():
        require_original_source()
        if initial_product is not None:
            from .runtime_selection import selected_runtime_artifacts
            selected_runtime_artifacts(initial_product.selection, verified_artifacts=initial_product)
    if mode == "install":
        from .deployment_updates import checked_package
        receipt = read_receipt(directory)
        _, checked = checked_package(directory, "package.zip", receipt["package_sha256"])
        if "runtimeCandidate" in json.loads(checked["deployment.json"][0]):
            from .runtime_selection import MetaToolkitRuntimeTarget, parse_candidate_selection
            selected = receipt.get("runtime_candidate")
            if selected is None or type(parse_candidate_selection(selected)) is not MetaToolkitRuntimeTarget:
                raise RuntimeError("A private runtime candidate requires its checked stopped-installation update; a fresh installation is not admitted.")
            if update_selection is not None or receipt.get("update") is not None:
                raise DraftConflict("Use the original selected product update or recovery owner; initial installation cannot replace it.")
            if receipt.get("execution_mode") != "interactive" or receipt.get("service") is not None:
                from .deployment import PRODUCT_CONFIGURE_REQUIRED
                raise DraftConflict(PRODUCT_CONFIGURE_REQUIRED)
            from .deployment import DeploymentService, product_runtime_source, OWNER_FILE
            from .connect_application_update_startup import _file_identity
            from .connect_installer_caller import _private_directory
            from .templates.install_candidate import verify_selected_application_dependencies
            destination = Path(receipt["destination"])
            initial_binding = DeploymentService._configuration_binding(receipt)
            initial_observation = (
                tuple((path, _private_directory(path)) for path in (Path(directory), destination)),
                tuple((path, _file_identity(path)) for path in
                    (Path(directory) / "package.zip", destination / OWNER_FILE, destination / ".env")))
            for path, identity in initial_observation[1]:
                mode_bits, uid, links = identity[-3:]
                if not stat.S_ISREG(mode_bits) or uid != os.getuid() or links != 1 or mode_bits & 0o077:
                    raise DraftConflict("Initial product installation needs its original owned private package, settings and receipt.")
            initial_files = checked
            initial_product = await await_validation(product_runtime_source, checked, selected)
            await await_validation(verify_selected_application_dependencies,
                checked.get("requirements.application.txt", (b"", 0o644))[0], initial_product)
            require_original()
    if mode in {"start", "service"} and original_bootstrap is None and update_selection is None:
        from .runtime_selection import MetaToolkitRuntimeTarget, parse_candidate_selection
        selected = read_receipt(directory).get("runtime_candidate")
        if selected is not None and type(parse_candidate_selection(selected)) is MetaToolkitRuntimeTarget:
            from .deployment import PRODUCT_CONFIGURE_REQUIRED
            raise DraftConflict(PRODUCT_CONFIGURE_REQUIRED)
    if mode == "service":
        from .deployment_service import UserServiceManager, verify_service
        receipt = read_receipt(directory)
        service = verify_service(directory, receipt)
        observation = await UserServiceManager().observe(service)
        if observation.get("pid") != os.getpid():
            raise ValueError("Only the recorded operating-system service may own this background runtime.")
        require_original()
        if (original_bootstrap is not None
                and (receipt.get("service_supervisor_pid") is not None or receipt.get("service_instance_id") is not None)
                and receipt.get("service_cleanup_confirmed") is not True):
            raise DraftConflict("The original Restore service instance has not confirmed cleanup. Its lifetime is preserved.")
        # A new lifetime has no folder evidence until it reaches ready itself.
        update_receipt(directory, service_supervisor_pid=os.getpid(), service_instance_id=str(uuid.uuid4()),
                       service_cleanup_confirmed=False, service_folder_evidence=None,
                       service_observation={**observation, "owned": True},
                       status="starting", phase="service", runtime_pid=None, console_url=None, bootstrap=None,
                       startup_attempt_id=None, startup_console_health=None, health="unknown", health_checked_at=None)
        if original_bootstrap is not None:
            # Only the actual unit PID may establish this new service lifetime.
            # Reobserve the same immutable Restore pin after that transition.
            original_lifecycle = bootstrap_lifecycle_binding(read_receipt(directory))
            bootstrap_selected, bootstrap_observation, bootstrap_owner_source = await observe_installed_bootstrap(
                directory, original_bootstrap, require_current=require_selected_lifecycle,
                owner_source=bootstrap_owner_source)
            require_original()
    require_original()
    try:
        supervisor = Supervisor(directory)
        if mode == "service":
            supervisor.own_service()
    except Exception:
        if mode == "service":
            journal(directory, "The service could not read its private installed configuration. No native runtime was started.",
                    status="failed", service_cleanup_confirmed=True, runtime_pid=None, console_url=None,
                    process_live=False, health="failed", health_checked_at=now())
        raise
    if original_bootstrap is not None:
        require_original()
        supervisor._bootstrap_admission = (
            deepcopy(original_bootstrap), deepcopy(bootstrap_selected), bootstrap_observation)
        supervisor._bootstrap_owner_source = bootstrap_owner_source
    owner = asyncio.create_task(supervisor.stop.wait() if mode == "service" else owner_input(supervisor))
    loop = asyncio.get_running_loop()
    for sig in (signal.SIGINT, signal.SIGTERM):
        loop.add_signal_handler(sig, supervisor.stop.set)
    try:
        if mode in {"update", "rollback", "recover-current"}:
            await update_runtime(supervisor, mode, update_id=expected["binding"]["update_id"],
                                 update_selection=update_selection)
        else:
            require_original()
            if initial_product is not None:
                from .deployment_updates import require_replacement_history_settled
                require_replacement_history_settled(directory)
                with project_lock(supervisor.destination) as lease:
                    def current_initial_source():
                        if supervisor.stop.is_set():
                            raise asyncio.CancelledError
                        lease.require_current()
                        if lease.root != supervisor.destination:
                            raise DraftConflict("The initial product acquired another project's lease.")
                        require_original_source()
                    current_initial = _InitialProductFence(initial_product, current_initial_source)
                    current_initial()
                    files = await _joined_product_mutation(supervisor, materialize, directory,
                        require_current=current_initial)
                    current_initial()
                    from .deployment import _DestinationSourceObservation
                    initial_source_observation = _DestinationSourceObservation(read_receipt(directory), files)
                    initial_files = files
                    current_initial()
                    definition = json.loads(files["deployment.json"][0])
                    await supervisor.install(definition, product_inputs=(files, initial_product, current_initial))
                    current_initial()
                    verify_destination(read_receipt(directory), files)
                    current_initial()
                    journal(directory, "Runtime installation is prepared and stopped. Supply the original Configure inputs to start the application.",
                        status="stopped", phase="configure", console_url=None, runtime_pid=None,
                        process_live=False, health="unknown", health_checked_at=None)
                return 0
            from .deployment_updates import require_replacement_history_settled
            from .deployment_application_bootstrap import require_bootstrap_history_settled
            require_replacement_history_settled(directory)
            require_bootstrap_history_settled(directory, bootstrap_selection=original_bootstrap)
            if original_bootstrap is not None:
                def bootstrap_definition():
                    with (Path(directory) / "package.zip").open("rb") as source:
                        payload = source.read(MAX_PACKAGE_BYTES + 1)
                    if (len(payload) > MAX_PACKAGE_BYTES
                            or hashlib.sha256(payload).hexdigest() != supervisor.receipt["package_sha256"]):
                        raise ValueError("The original bootstrap package changed or exceeded its source bound.")
                    files = package_files(payload)
                    verify_destination(read_receipt(directory), files)
                    return json.loads(files["deployment.json"][0])
                definition = await await_validation(bootstrap_definition)
                require_original()
            else:
                payload = (Path(directory) / "package.zip").read_bytes()
                if hashlib.sha256(payload).hexdigest() != supervisor.receipt["package_sha256"]:
                    raise ValueError("The saved checked package changed. No installed source was executed.")
                files = materialize(directory) if mode == "install" else package_files(payload)
                verify_destination(read_receipt(directory), files)
                definition = json.loads(files["deployment.json"][0])
            if mode == "install":
                require_original()
                await supervisor.install(definition)
                require_original()
            if supervisor.receipt.get("execution_mode") == "background" and mode != "service":
                if supervisor.stop.is_set():
                    raise asyncio.CancelledError
                from .deployment_service import UserServiceManager, prepare_service
                manager = UserServiceManager()
                if not (await manager.capability())["available"]:
                    raise RuntimeError("The logged-in user's service manager is unavailable. No background service was installed.")
                require_original()
                service = prepare_service(directory, read_receipt(directory))
                require_original()
                journal(directory, "Starting this checked application as an owned operating-system background service.",
                        status="starting", phase="service", service_cleanup_confirmed=False)
                await manager.start(service)
                require_original()
                journal(directory, "The operating-system service was admitted. Waiting for its native startup evidence.")
                return 0
            if not supervisor.stop.is_set():
                require_original()
                callback = None
                if expected is not None and expected["recovery_status"] is not None:
                    from .deployment_updates import committed, transaction, read_update_admission

                    def callback():
                        require_original()
                        return committed(directory, expected["binding"]["update_id"],
                            restored=expected["recovery_status"] == "restored-stopped",
                            expected_binding=expected["binding"], expected_installation=expected["installation"])
                # A static OS unit has no original dynamic update admission.
                # Its actual startup health remains separate from completing
                # a restored transaction; the explicit update owner commits it.
                if original_bootstrap is not None:
                    await supervisor.start(definition, on_ready=callback,
                                           bootstrap_selection=original_bootstrap)
                else:
                    await supervisor.start(definition, on_ready=callback)
        reason = owner.result() if owner.done() and mode != "service" else "stopped"
        journal(directory, "Forge stopped; installed files and native state are preserved." if reason == "owner_lost"
                else "Your application is stopped. Installed files and native state are preserved.",
                status="interrupted" if reason == "owner_lost" else "stopped", phase="stop",
                runtime_pid=None, console_url=None, process_live=False)
    except asyncio.CancelledError:
        reason = owner.result() if owner.done() and mode != "service" else "stopped"
        journal(directory, "The deployment was interrupted. Installed files and native state are preserved.",
                status="interrupted" if reason == "owner_lost" else "stopped", runtime_pid=None,
                console_url=None, process_live=False)
    except Exception as error:
        message = str(error) if isinstance(error, (ValueError, RuntimeError, TimeoutError)) else f"Deployment failed ({type(error).__name__}). Inspect the redacted logs and retry."
        journal(directory, supervisor.redact(message), status="failed", runtime_pid=None, console_url=None,
                process_live=False, health="failed", health_checked_at=now())
        return 1
    finally:
        supervisor.stop.set()
        await supervisor.terminate()
        if mode == "service":
            update_receipt(directory, service_cleanup_confirmed=True)
        owner.cancel()
        await asyncio.gather(owner, return_exceptions=True)


if __name__ == "__main__":
    selection = ({"path": sys.argv[3], "sha256": sys.argv[4]} if len(sys.argv) == 5 else None)
    if len(sys.argv) not in {3, 5}:
        raise ValueError("The fixed deployment worker arguments changed.")
    serve_as_owned_worker()
    sys.exit(asyncio.run(run(Path(sys.argv[1]), sys.argv[2], update_selection=selection)) or 0)
