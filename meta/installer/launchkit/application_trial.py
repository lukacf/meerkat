"""Source-bound application callbacks behind a persistent offline sandbox.

The native trial host owns agents and provider access. Generated application
code runs only in this separate process, with no runtime handle, credentials,
network, user files or source writes. Its results are application observations,
not acceptance receipts or proof that live adapters work.
"""
from __future__ import annotations

import asyncio
import copy
import json
import os
from pathlib import Path
import re
import sys
import uuid

from .deployment_deadlines import WORKER_STOP_TIMEOUT, PROCESS_FORCE_REAP_TIMEOUT
from .trial_samples import trial_samples
from .implementation_workspace import (
    ImplementationWorkspace, WorkspaceError, MAX_PROJECT_BYTES, MAX_PROJECT_FILES,
    MAX_FILE_BYTES, _atomic_write, _check_environment, _json, _now, _regular, _read_package_file,
    _safe_name, _sandbox_executable, _sandbox_launch, _sandbox_profile, _trusted_python_paths,
)

MAX_FIXTURE_BYTES = 100_000
MAX_MESSAGE_BYTES = 100_000
MAX_CATALOG_BYTES = 2_000_000
MAX_OBSERVATION_BYTES = 20_000_000
MAX_LOG_BYTES = 64_000
MAX_CALLS = 1000
START_SECONDS = 30
CALL_SECONDS = 30
STOP_SECONDS = 3
PREVIEW_SECONDS = 5
SCOPE = "Actual generated application callbacks with explicitly supplied fake I/O in an offline sandbox. Not live integration, acceptance or deployment proof."


class ApplicationTrialError(RuntimeError):
    pass


def validate_application_fixtures(value):
    value = {} if value is None else value
    if not isinstance(value, dict):
        raise ValueError("Application fixtures must be an ordinary JSON object.")
    try:
        encoded = json.dumps(value, ensure_ascii=True, allow_nan=False)
    except (TypeError, ValueError, RecursionError):
        raise ValueError("Application fixtures must contain finite ordinary JSON values.") from None
    if len(encoded.encode()) > MAX_FIXTURE_BYTES:
        raise ValueError("Application fixtures must fit within 100 KB.")
    return json.loads(encoded)


def validate_application_native_policy(value, deployment):
    """Keep authored desired startup policy within the frozen trial roster.

    The protected package host validates application hooks before projecting
    these native specifications. This transport boundary still rejects profile
    changes, extra identities or execution grants; MobKit admits the resulting
    roster and edges. Offline trials do not reconcile a changing desired roster.
    """
    from meerkat_meta.identity_first_models import DurableAgentSpec, ManagedPeerEdge

    if not isinstance(value, dict) or set(value) != {"roster", "edges"}:
        raise ApplicationTrialError("Application startup did not return its validated native policy.")
    roster, edges = value["roster"], value["edges"]
    fixed = {row["identity"]: row for row in deployment["agents"]}
    classes = {row["id"]: row for row in deployment["classes"]}
    if not isinstance(roster, list) or len(roster) != len(fixed) or not isinstance(edges, list):
        raise ApplicationTrialError("Offline application policy must retain exactly the frozen native roster; new identities are unavailable.")
    allowed = {"identity", "profile", "addressability", "display_name", "labels", "context",
               "additional_instructions", "runtime_mode_override"}
    result, seen = [], set()
    try:
        for row in roster:
            if not isinstance(row, dict) or set(row) - allowed:
                raise ValueError("Desired policy cannot override native grants or execution authority.")
            identity = row.get("identity")
            original = fixed.get(identity) if isinstance(identity, str) else None
            if original is None or identity in seen or row.get("profile") != original["profile"]:
                raise ValueError("Desired policy changed a frozen identity or profile.")
            if row.get("addressability") != "addressable" or row.get("runtime_mode_override") != "turn_driven":
                raise ValueError("Offline policy requires addressable, turn-driven native identities.")
            labels = row.get("labels")
            authority = {"class": original["classId"], "role": original["role"],
                         "console_group": classes[original["classId"]]["name"]}
            if (not isinstance(labels, dict)
                    or any(not isinstance(key, str) or not isinstance(item, str) for key, item in labels.items())
                    or any(labels.get(key) != item for key, item in authority.items())
                    or set(labels) & {"agent_identity", "spawned_by", "via_tool"}):
                raise ValueError("Desired policy changed frozen class labels or asserted native lineage.")
            name, instructions = row.get("display_name"), row.get("additional_instructions")
            if (not isinstance(name, str) or not name.strip() or not isinstance(instructions, list)
                    or any(not isinstance(item, str) for item in instructions)):
                raise ValueError("Desired policy needs ordinary display names and identity instructions.")
            result.append(DurableAgentSpec.from_dict(row).to_dict())
            seen.add(identity)
        if seen != set(fixed):
            raise ValueError("Desired policy omitted a frozen identity.")
        managed, edge_keys = [], set()
        for row in edges:
            if (not isinstance(row, dict) or set(row) != {"a", "b"}
                    or not isinstance(row["a"], str) or not isinstance(row["b"], str)
                    or row["a"] not in fixed or row["b"] not in fixed):
                raise ValueError("Desired peer policy references a foreign identity.")
            edge = ManagedPeerEdge.from_dict(row)
            if (edge.a, edge.b) in edge_keys:
                raise ValueError("Desired peer policy repeats an edge.")
            edge_keys.add((edge.a, edge.b))
            managed.append(edge.to_dict())
        # Context remains application-owned ordinary data, including accepted
        # identity overrides. It cannot carry nonfinite values over this seam.
        return json.loads(json.dumps({"roster": result, "edges": managed}, ensure_ascii=True, allow_nan=False))
    except (ValueError, TypeError, KeyError, RecursionError, UnicodeError) as error:
        raise ApplicationTrialError(str(error)) from None


def source_files(root):
    """Inspect an immutable host snapshot without importing authored code."""
    root = Path(root).absolute()
    if root.is_symlink() or not root.is_dir():
        raise WorkspaceError("Application trial source must be an ordinary frozen directory.")
    result, size = [], 0
    for directory, children, names in os.walk(root, followlinks=False):
        if any((Path(directory) / name).is_symlink() for name in children):
            raise WorkspaceError("Application trial source cannot contain directory links.")
        for name in names:
            path = Path(directory) / name
            relative = _safe_name(path.relative_to(root).as_posix())
            info = _regular(path)
            size += info.st_size
            if info.st_size > MAX_FILE_BYTES or size > MAX_PROJECT_BYTES or len(result) >= MAX_PROJECT_FILES:
                raise WorkspaceError("Application trial source exceeds the checked project bounds.")
            result.append(_read_package_file(path, relative))
    return sorted(result, key=lambda item: item.path)


class SandboxApplicationTrial:
    def __init__(self, directory, source_root, source_digest, deployment, fixtures=None):
        self.directory = Path(directory).resolve()
        self.source_root = Path(source_root).absolute()
        self.source_digest = source_digest
        self.deployment = json.loads(json.dumps(deployment, allow_nan=False))
        self.fixtures = validate_application_fixtures(fixtures)
        self.root = self.directory / "application-sandbox"
        self.process = None
        self.reader = None
        self.transport = None
        self.log_task = None
        self.lock = asyncio.Lock()
        self.catalog = []
        self.allowed = set()
        self.counter = 0
        self.calls = 0
        self.token = uuid.uuid4().hex
        self.unavailable = None
        self.dependencies = []
        self.stopped = False
        self.events = []
        self.observation_bytes = 0
        self.samples = None
        self.observation_revision = 0
        self.preview = None
        self.policy = None
        self.on_observation = None

    def native_policy(self):
        return copy.deepcopy(self.policy)

    def _record(self, kind, **data):
        if kind in {"application_started", "callback_requested", "callback_returned", "callback_failed"}:
            self.observation_revision += 1
        row = {"at": _now(), "kind": kind, "source_digest": self.source_digest,
               "scope": "offline-application", "observation_revision": self.observation_revision, **data}
        encoded = _json(row) + "\n"
        self.observation_bytes += len(encoded.encode())
        if self.observation_bytes > MAX_OBSERVATION_BYTES:
            raise ApplicationTrialError("The application trial exceeded its retained observation bound.")
        self.events.append(row)
        self.directory.mkdir(parents=True, mode=0o700, exist_ok=True)
        path = self.directory / "application-observations.jsonl"
        with path.open("a", encoding="utf-8") as output:
            os.chmod(path, 0o600)
            output.write(encoded)
        self._notify_observation()
        return row

    def _notify_observation(self):
        if callable(self.on_observation):
            self.on_observation()

    def application_preview(self):
        return json.loads(_json(self.preview)) if self.preview is not None else None

    def _preview_unavailable(self, status, detail):
        self.preview = {"status": status, "scope": "offline-application-preview",
            "sanitization": "inert-html-v1", "title": None, "html": None, "detail": detail,
            "live_ready": False, "source_digest": self.source_digest,
            "observation_revision": self.observation_revision}
        self._notify_observation()

    async def _capture_preview(self):
        """Observe this sandbox's current renderer without admitting another app."""
        from .application_preview import sanitize_application_preview
        if self.unavailable or self.stopped or not self.process or self.process.returncode is not None:
            self._preview_unavailable("failed", "The isolated application preview is unavailable after its process stopped.")
            return
        try:
            result = await self._exchange("render_preview", {}, PREVIEW_SECONDS)
            if isinstance(result, dict) and result == {"supported": False}:
                self._preview_unavailable("unsupported", "This application trial has no render_preview() hook.")
                return
            if not isinstance(result, dict) or set(result) != {"supported", "document"} or result["supported"] is not True:
                raise ValueError("Invalid preview response.")
            document = sanitize_application_preview(result["document"])
        except asyncio.CancelledError:
            raise
        except Exception:
            self._preview_unavailable("failed", "The application preview could not be rendered safely. Its callback observations remain available.")
            return
        self.preview = {**document, "source_digest": self.source_digest,
                        "observation_revision": self.observation_revision}
        self._notify_observation()

    def observations(self):
        return json.loads(_json(self.events))

    def status(self):
        return {"available": bool(self.allowed) and not self.unavailable,
                "running": bool(self.process) and self.process.returncode is None and not self.stopped,
                "detail": self.unavailable, "dependencies": list(self.dependencies),
                "source_digest": self.source_digest, "scope": SCOPE, "live_ready": False,
                "calls": self.calls, "catalog": self.catalog, "samples": self.samples}

    async def start(self):
        if self.process or self.stopped:
            raise ApplicationTrialError("An application sandbox can only start once.")
        sandbox = _sandbox_executable()
        if not sandbox:
            raise ApplicationTrialError("An isolated application checker is unavailable. No application code was executed.")
        files = source_files(self.source_root)
        if ImplementationWorkspace._digest(files) != self.source_digest:
            raise ApplicationTrialError("The frozen application source changed. Start a new source-bound trial.")
        self.samples = trial_samples(files, self.source_digest)
        names = {item.path for item in files}
        if not {"app.py", "deployment.json", "application/__init__.py"} <= names:
            raise ApplicationTrialError("A trial needs a frozen generated application and its protected native host.")
        if json.loads(next(item.content for item in files if item.path == "deployment.json")) != self.deployment:
            raise ApplicationTrialError("Application callbacks belong to a different native deployment.")
        self.directory.mkdir(parents=True, mode=0o700, exist_ok=True)
        self.root.mkdir(mode=0o700)
        project, scratch = self.root / "project", self.root / "scratch"
        project.mkdir(mode=0o700)
        scratch.mkdir(mode=0o700)
        for item in files:
            target = project / item.path
            target.parent.mkdir(parents=True, mode=0o700, exist_ok=True)
            _atomic_write(target, item.content, item.mode)
        data = project / "data"
        data.mkdir(mode=0o700, exist_ok=True)
        harness = self.root / "host-check.py"
        _atomic_write(harness, _HARNESS)
        from .rehearsal import REHEARSAL_SECONDS
        owner_lifetime = REHEARSAL_SECONDS + WORKER_STOP_TIMEOUT + PROCESS_FORCE_REAP_TIMEOUT
        supervisor = self.root / "supervisor.py"
        _atomic_write(supervisor, _SUPERVISOR)
        profile = _sandbox_profile(project, scratch, self.root)
        # Runtime data is private to this trial. Existing checked source files,
        # including any data fixtures, remain immutable even inside that tree.
        if sys.platform == "linux":
            from .linux_sandbox import PREFIX
            policy = json.loads(profile[len(PREFIX):])
            policy["writes"].append(str(data))
            policy["frozen"] = [str(project / item.path) for item in files if item.path.startswith("data/")]
            profile = PREFIX + _json(policy)
        else:
            profile += f"\n(allow file-write* (subpath {_json(str(data))}))\n"
            profile += f"(deny file-write* (literal {_json(str(data))}))\n"
            for item in files:
                if item.path.startswith("data/"):
                    profile += f"(deny file-write* (literal {_json(str(project / item.path))}))\n"
        read_fd, write_fd = os.pipe()
        pipe = os.fdopen(read_fd, "rb", buffering=0)
        self.reader = asyncio.StreamReader(limit=MAX_CATALOG_BYTES + 2000)
        protocol = asyncio.StreamReaderProtocol(self.reader)
        try:
            self.transport, _ = await asyncio.get_running_loop().connect_read_pipe(lambda: protocol, pipe)
            child = [str(Path(sys.executable).resolve()), "-I", "-S", str(harness), str(project),
                     _json(_trusted_python_paths()), str(scratch), str(write_fd), self.token]
            environment = _check_environment(scratch)
            with _sandbox_launch(child, sandbox, profile, project, environment) as (argv, descriptors, completed):
                passed = (write_fd, *descriptors, *((completed,) if completed is not None else ()))
                spawn = asyncio.create_task(asyncio.create_subprocess_exec(
                    str(Path(sys.executable).resolve()), "-I", "-S", str(supervisor),
                    _json(argv), str(write_fd), self.token, str(self.root / "process-receipt.json"),
                    str(owner_lifetime), _json(descriptors), str(completed if completed is not None else -1),
                    cwd=project, env=environment, stdin=asyncio.subprocess.PIPE,
                    stdout=asyncio.subprocess.PIPE, stderr=asyncio.subprocess.STDOUT,
                    pass_fds=passed, start_new_session=True))
                try:
                    self.process = await asyncio.shield(spawn)
                except asyncio.CancelledError:
                    # Retain ownership even when admission races cancellation.
                    self.process = await asyncio.shield(spawn)
                    if completed is not None:
                        os.close(descriptors.pop())
                    await asyncio.shield(self._terminate())
                    raise
                else:
                    if completed is not None:
                        os.close(descriptors.pop())
        except BaseException:
            if self.transport:
                self.transport.close()
            else:
                pipe.close()
            raise
        finally:
            os.close(write_fd)
        self.log_task = asyncio.create_task(self._logs())
        try:
            result = await self._exchange("start", {"fixtures": self.fixtures}, START_SECONDS)
            if not isinstance(result, dict) or not isinstance(result.get("dependencies"), list):
                raise ApplicationTrialError("Application startup did not return its bounded callback catalog.")
            self.catalog = self._catalog(result.get("catalog"))
            self.policy = validate_application_native_policy(result.get("native_policy"), self.deployment)
            self.allowed = {(row["identity"], tool["name"]) for row in self.catalog for tool in row["tools"]}
            self.dependencies = result.get("dependencies", [])
            self.unavailable = result.get("unavailable")
            self._record("application_started", catalog=self.catalog, dependencies=self.dependencies,
                         unavailable=self.unavailable)
            async with self.lock:
                await self._capture_preview()
            return self.catalog
        except BaseException:
            await self._terminate()
            raise

    def _catalog(self, value):
        identities = {row["identity"] for row in self.deployment["agents"]}
        if not isinstance(value, list) or len(value) != len(identities):
            raise ApplicationTrialError("Application callback catalog does not match the frozen native roster.")
        seen = set()
        for row in value:
            if not isinstance(row, dict) or row.get("identity") not in identities or row["identity"] in seen:
                raise ApplicationTrialError("Application callback catalog contains an unknown or repeated identity.")
            seen.add(row["identity"])
            if not isinstance(row.get("tools"), list):
                raise ApplicationTrialError("Application callbacks must be an ordinary list.")
            seen_tools = set()
            for tool in row["tools"]:
                if (not isinstance(tool, dict) or not isinstance(tool.get("name"), str)
                        or not re.fullmatch(r"[A-Za-z][A-Za-z0-9_-]{0,63}", tool["name"])
                        or tool["name"] in seen_tools or not isinstance(tool.get("description"), str)
                        or not isinstance(tool.get("schema"), dict)):
                    raise ApplicationTrialError("Application callback metadata is malformed or repeated.")
                seen_tools.add(tool["name"])
        return value

    async def _logs(self):
        path = self.root / "application.log"
        count = 0
        with path.open("wb") as output:
            os.chmod(path, 0o600)
            while chunk := await self.process.stdout.read(4096):
                remaining = MAX_LOG_BYTES - count
                output.write(chunk[:max(0, remaining)])
                count += len(chunk)
                if count > MAX_LOG_BYTES:
                    self.unavailable = "The generated application exceeded its bounded log output."
                    await self._terminate()
                    return

    async def _exchange(self, operation, values, timeout):
        self.counter += 1
        request_id = self.counter
        request = {"id": request_id, "operation": operation, **values}
        try:
            encoded = json.dumps(request, ensure_ascii=True, allow_nan=False).encode() + b"\n"
        except (ValueError, TypeError, RecursionError):
            raise ApplicationTrialError("Application trial arguments must be finite ordinary JSON.") from None
        if len(encoded) > MAX_MESSAGE_BYTES:
            raise ApplicationTrialError("Application trial arguments exceed the bounded message size.")
        if self.process.returncode is not None or not self.process.stdin:
            raise ApplicationTrialError("The isolated application process is unavailable.")
        try:
            async with asyncio.timeout(timeout):
                self.process.stdin.write(encoded)
                await self.process.stdin.drain()
                line = await self.reader.readline()
        except (TimeoutError, ValueError, ConnectionError) as error:
            self.unavailable = "The application callback timed out or exceeded its response bounds."
            await self._terminate()
            raise ApplicationTrialError(self.unavailable) from error
        if not line:
            self.unavailable = "The application process exited without completing its requested callback."
            await self._terminate()
            raise ApplicationTrialError(self.unavailable)
        try:
            result = json.loads(line, parse_constant=lambda value: (_ for _ in ()).throw(ValueError(value)))
        except (ValueError, RecursionError):
            self.unavailable = "The application returned invalid bounded JSON."
            await self._terminate()
            raise ApplicationTrialError(self.unavailable) from None
        limit = MAX_CATALOG_BYTES if operation == "start" else MAX_MESSAGE_BYTES
        if (not isinstance(result, dict) or result.get("token") != self.token
                or result.get("id") != request_id or len(line) > limit):
            self.unavailable = "The application returned an uncorrelated or oversized response."
            await self._terminate()
            raise ApplicationTrialError(self.unavailable)
        if not result.get("ok"):
            raise ApplicationTrialError(str(result.get("error") or "The application callback failed.")[:2000])
        return result.get("value")

    async def call(self, identity, name, args):
        if self.unavailable or self.stopped or (identity, name) not in self.allowed:
            raise ApplicationTrialError("This callback is unavailable for the selected native identity.")
        if self.calls >= MAX_CALLS:
            raise ApplicationTrialError("The application trial reached its bounded callback count.")
        try:
            encoded = json.dumps(args, ensure_ascii=True, allow_nan=False)
        except (TypeError, ValueError, RecursionError):
            raise ApplicationTrialError("Application trial arguments must be finite ordinary JSON.") from None
        if len(encoded.encode()) > MAX_MESSAGE_BYTES - 2000:
            raise ApplicationTrialError("Application trial arguments exceed the bounded message size.")
        async with self.lock:
            if self.stopped or self.unavailable:
                raise ApplicationTrialError("This application trial stopped before the callback could start.")
            if self.calls >= MAX_CALLS:
                raise ApplicationTrialError("The application trial reached its bounded callback count.")
            self.calls += 1
            self._record("callback_requested", identity=identity, name=name, arguments=args)
            try:
                result = await self._exchange("call", {"identity": identity, "name": name, "arguments": args}, CALL_SECONDS)
            except BaseException as error:
                self._record("callback_failed", identity=identity, name=name, error=type(error).__name__, detail=str(error)[:2000])
                if isinstance(error, asyncio.CancelledError):
                    await self._terminate()
                    self._preview_unavailable("failed", "The application preview was interrupted with its isolated callback.")
                else:
                    await self._capture_preview()
                raise
            self._record("callback_returned", identity=identity, name=name, result=result,
                         evidence="Application-returned offline value, not live adapter evidence.")
            await self._capture_preview()
            return result

    async def _terminate(self):
        if not self.process:
            return True
        if self.process.returncode is None:
            # EOF also handles cancellation immediately after admission, before
            # Python has installed the supervisor's signal handlers. A signal
            # in that startup window could exit without a child-reap receipt.
            if self.process.stdin:
                self.process.stdin.close()
            else:
                try:
                    self.process.terminate()
                except ProcessLookupError:
                    pass
        try:
            await asyncio.wait_for(self.process.wait(), STOP_SECONDS)
        except TimeoutError as error:
            raise ApplicationTrialError("Application sandbox termination could not be confirmed.") from error
        receipt = self.root / "process-receipt.json"
        if not receipt.is_file():
            raise ApplicationTrialError("Application sandbox termination has no trusted child receipt.")
        value = json.loads(receipt.read_text())
        if value.get("token") != self.token or value.get("reaped") is not True:
            raise ApplicationTrialError("Application sandbox child termination could not be confirmed.")
        return True

    async def stop(self):
        if self.stopped:
            return await self._terminate()
        self.stopped = True
        failure = None
        try:
            if self.process and self.process.returncode is None:
                try:
                    await asyncio.wait_for(self.lock.acquire(), STOP_SECONDS)
                except TimeoutError:
                    failure = "An application callback was active during shutdown; the sandbox was stopped."
                else:
                    try:
                        await self._exchange("stop", {}, STOP_SECONDS)
                    except Exception as error:
                        failure = str(error)[:2000]
                    finally:
                        self.lock.release()
        finally:
            await self._terminate()
            if self.log_task and self.log_task is not asyncio.current_task():
                await asyncio.gather(self.log_task, return_exceptions=True)
            if self.transport:
                self.transport.close()
        self._record("application_stopped", process_stop_confirmed=True, lifecycle_error=failure)
        return True


_SUPERVISOR = r'''import json, os, pathlib, selectors, signal, subprocess, sys, time
# Trusted process only: never import generated application code here. Monitoring
# the owner's pipe independently keeps shutdown effective even if that code
# blocks its event loop or holds the Python interpreter lock.
command, response_fd, token, receipt_name, owner_lifetime, extra_fds, cleanup_fd = sys.argv[1:]
command, extra_fds, cleanup_fd = json.loads(command), json.loads(extra_fds), int(cleanup_fd)
receipt = pathlib.Path(receipt_name)
child = None
stopping = False
def request_stop(signum, frame):
    global stopping
    stopping = True
signal.signal(signal.SIGTERM, request_stop)
signal.signal(signal.SIGINT, request_stop)
selector = selectors.DefaultSelector()
try:
    child = subprocess.Popen(command,
                              stdin=subprocess.PIPE, stdout=sys.stdout.buffer,
                              stderr=subprocess.STDOUT, pass_fds=(int(response_fd), *extra_fds),
                              start_new_session=True, env=dict(os.environ))
    os.close(int(response_fd))
    for descriptor in extra_fds:
        os.close(descriptor)
    owner_fd = sys.stdin.fileno()
    input_fd = child.stdin.fileno()
    os.set_blocking(owner_fd, False)
    os.set_blocking(input_fd, False)
    selector.register(owner_fd, selectors.EVENT_READ, "owner")
    pending = bytearray()
    writing = False
    deadline = time.monotonic() + float(owner_lifetime)
    while child.poll() is None and not stopping and time.monotonic() < deadline:
        for key, event in selector.select(.1):
            if key.data == "owner":
                chunk = os.read(owner_fd, 8192)
                if not chunk:
                    stopping = True
                    break
                pending.extend(chunk)
                if len(pending) > 200000:
                    stopping = True
                    break
                if not writing:
                    selector.register(input_fd, selectors.EVENT_WRITE, "child")
                    writing = True
            else:
                try:
                    count = os.write(input_fd, pending)
                except BlockingIOError:
                    continue
                except BrokenPipeError:
                    stopping = True
                    break
                del pending[:count]
                if not pending:
                    selector.unregister(input_fd)
                    writing = False
finally:
    selector.close()
    reaped = child is None
    if child is not None:
        if child.poll() is None:
            try:
                os.killpg(child.pid, signal.SIGKILL)
            except (ProcessLookupError, PermissionError):
                if child.poll() is None:
                    child.kill()
        try:
            child.wait(timeout=2)
            reaped = True
        except subprocess.TimeoutExpired:
            reaped = False
        child.stdin.close()
    if cleanup_fd >= 0:
        import select
        try:
            reaped = reaped and bool(select.select([cleanup_fd], [], [], 2)[0]) and os.read(cleanup_fd, 1) == b""
        finally:
            os.close(cleanup_fd)
    # The sandbox cannot write this host-owned directory. Exit alone is not a
    # cleanup receipt: the parent requires this positive child-reap evidence.
    receipt.write_text(json.dumps({"token": token, "pid": child.pid if child else None,
                                   "reaped": reaped}))
    receipt.chmod(0o600)
    if not reaped:
        raise SystemExit(75)
'''


_HARNESS = r'''import asyncio, importlib.util, inspect, json, os, pathlib, re, runpy, sys, traceback
root = pathlib.Path(sys.argv[1]).resolve()
sys.path[:0] = [str(root), *json.loads(sys.argv[2])]
scratch = pathlib.Path(sys.argv[3]).resolve()
channel = os.fdopen(int(sys.argv[4]), "w", buffering=1)
token = sys.argv[5]
sys.argv = [str(root)]
deployment = json.loads((root / "deployment.json").read_text())
stop_event = None
production = None
integration = None
handlers = {}

async def value(item):
    return await item if inspect.isawaitable(item) else item

def encode(item):
    return json.dumps(item, ensure_ascii=True, allow_nan=False)

async def start(fixtures):
    global production, integration, handlers, stop_event
    namespace = runpy.run_path(str(root / "app.py"), run_name="__application_trial_host__")
    stop_event = asyncio.Event()
    context = namespace["ApplicationContext"](root, deployment, stop_event)
    context.scratch_root = scratch
    context.source_root = root
    production = namespace["Application"](context)
    await production.load()
    import hashlib
    module_name = "_mobkit_application_" + hashlib.sha256(str(root.resolve()).encode()).hexdigest()[:16]
    module = sys.modules[module_name]
    hook = getattr(module, "rehearse", None)
    empty = [{"identity": row["identity"], "tools": []} for row in deployment["agents"]]
    if not callable(hook):
        return {"catalog": empty, "dependencies": [], "unavailable": "This source has no application.rehearse(context, fixtures) hook. Application callbacks remain unavailable; no fake adapter behavior was invented.",
                "native_policy": await native_policy()}
    integration = await value(hook(context, fixtures))
    if integration is None or not callable(getattr(integration, "tools_for_agent", None)):
        raise ValueError("rehearse(context, fixtures) must return an integration with tools_for_agent(agent).")
    catalog, dependencies = [], []
    for agent in deployment["agents"]:
        granted = {tool["name"]: tool for tool in production.tools[agent["profile"]]}
        tools = await value(integration.tools_for_agent(agent))
        if not isinstance(tools, list):
            raise ValueError("Rehearsal tools must be a list of actual application callbacks.")
        selected = []
        seen = set()
        for tool in tools:
            if not isinstance(tool, dict) or tool.get("name") not in granted or tool["name"] in seen:
                raise ValueError("Rehearsal callbacks must keep exact production names and per-identity grants.")
            original = granted[tool["name"]]
            if tool.get("schema") != original["schema"] or not callable(tool.get("handler")):
                raise ValueError("Rehearsal callbacks must keep the exact production input schema.")
            seen.add(tool["name"])
            # New package hosts use the same argument bridge as installed tools.
            # Older frozen hosts retain their original one-dictionary contract.
            bridge = namespace.get("application_callback")
            handlers[(agent["identity"], tool["name"])] = bridge(tool["handler"]) if bridge else tool["handler"]
            selected.append({"name": tool["name"], "description": original["description"], "schema": original["schema"]})
        missing = sorted(set(granted) - seen)
        if missing:
            dependencies.append(agent["identity"] + ": unavailable callbacks " + ", ".join(missing))
        catalog.append({"identity": agent["identity"], "tools": selected})
    lifecycle = getattr(integration, "start", None)
    if lifecycle is not None:
        if not callable(lifecycle):
            raise ValueError("Rehearsal start must be callable.")
        await value(lifecycle())
    return {"catalog": catalog, "dependencies": dependencies, "unavailable": None,
            "native_policy": await native_policy()}

async def native_policy():
    # Read protected host projections, never raw authored desired-hook rows.
    # Older immutable packages without desired hooks keep their frozen policy;
    # an older host unable to validate supplied hooks must not invent a fallback.
    has_roster = callable(getattr(production.integration, "desired_roster", None))
    has_edges = callable(getattr(production.integration, "desired_edges", None))
    if has_roster != has_edges:
        raise ValueError("Application desired_roster and desired_edges must be provided together.")
    roster_hook, edges_hook = getattr(production, "roster", None), getattr(production, "compute_edges", None)
    if callable(roster_hook) and callable(edges_hook):
        roster = await value(roster_hook(None))
        identities = [row.identity for row in roster]
        edges = await value(edges_hook(identities, None))
        return {"roster": [row.to_dict() for row in roster], "edges": [row.to_dict() for row in edges]}
    if has_roster:
        raise ValueError("This frozen application host cannot validate its desired native policy. Rebuild the application before trying it.")
    from meerkat_meta.identity_first_models import DurableAgentSpec, ManagedPeerEdge
    classes = {row["id"]: row for row in deployment["classes"]}
    roster = [DurableAgentSpec(identity=row["identity"], profile=row["profile"], display_name=row["name"],
        addressability="addressable", runtime_mode_override="turn_driven",
        labels={"class": row["classId"], "role": row["role"], "console_group": classes[row["classId"]]["name"]})
        for row in deployment["agents"]]
    return {"roster": [row.to_dict() for row in roster],
            "edges": [ManagedPeerEdge(a=a, b=b).to_dict() for a,b in deployment["connections"]]}

async def close():
    if stop_event is not None:
        stop_event.set()
    for candidate in (integration, production):
        hook = getattr(candidate, "stop", None)
        if callable(hook):
            await value(hook())

async def main():
    for line in sys.stdin:
        request = json.loads(line)
        try:
            op = request["operation"]
            if op == "start":
                result = await start(request["fixtures"])
            elif op == "call":
                handler = handlers[(request["identity"], request["name"])]
                result = await value(handler(request["arguments"]))
            elif op == "render_preview":
                hook = getattr(integration, "render_preview", None)
                if hook is None:
                    result = {"supported": False}
                elif not callable(hook):
                    raise ValueError("Application render_preview must be callable.")
                else:
                    document = await value(hook())
                    if (not isinstance(document, dict) or set(document) - {"title", "html"}
                            or not isinstance(document.get("html"), str)
                            or "title" in document and not isinstance(document["title"], str)):
                        raise ValueError("Application render_preview must return title and HTML only.")
                    if len(document["html"].encode()) > 64000:
                        raise ValueError("Application preview exceeds its bounded input size.")
                    result = {"supported": True, "document": document}
            elif op == "stop":
                await close()
                result = {"stopped": True}
            else:
                raise ValueError("Unknown application trial operation.")
            response = {"id": request["id"], "token": token, "ok": True, "value": result}
            encoded = (json.dumps(response, ensure_ascii=False, allow_nan=False)
                       if op == "render_preview" else encode(response))
            limit = 2000000 if op == "start" else 100000
            if len(encoded.encode()) > limit:
                raise ValueError("Application callback result exceeds 100 KB.")
        except BaseException as error:
            response = {"id": request.get("id"), "token": token, "ok": False,
                        "error": type(error).__name__ + ": " + str(error)[:1500]}
            encoded = encode(response)
        channel.write(encoded + "\n")
        channel.flush()
        if request.get("operation") == "stop":
            return

asyncio.run(main())
'''
