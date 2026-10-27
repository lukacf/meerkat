"""Explicit local Docker execution of a canonical Toolkit Compose plan.

The caller owns authorization and exclusive custody of the workspace, its ancestors,
Docker endpoint and pinned tools. This module is not a distributed Docker fence.
Observations are current engine facts, never proof of domain or native readiness.
"""
from __future__ import annotations

import argparse
import asyncio
from collections.abc import AsyncIterable, Awaitable, Callable
from contextlib import contextmanager
from dataclasses import dataclass
import fcntl
import hashlib
import json
import os
from pathlib import Path
import re
import signal
import stat
import sys
import uuid
from typing import Any

MAX_INPUT = 2 * 1024 * 1024
MAX_OUTPUT = 4 * 1024 * 1024
MAX_PLAN = 8 * 1024 * 1024
MAX_STREAM = 16 * 1024 * 1024
IMAGE_PULL_TIMEOUT = 120.0
SUCCESSOR_MARKER = b'{"kind":"compose_successor_custody","schema_version":2}\n'
HEX = re.compile(r"[0-9a-f]{64}\Z")


class ComposeError(RuntimeError):
    """Fixed diagnostic without subprocess output, paths or configuration values."""


def _fail(code: str) -> None:
    raise ComposeError(code)


def _sha(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def _json(value: Any) -> bytes:
    return json.dumps(value, ensure_ascii=False, sort_keys=True, separators=(",", ":"),
                      allow_nan=False).encode("utf-8")


def _decode(data: bytes) -> Any:
    def pairs(values: list[tuple[str, Any]]) -> dict[str, Any]:
        result: dict[str, Any] = {}
        for key, value in values:
            if key in result:
                _fail("invalid_json")
            result[key] = value
        return result
    try:
        return json.loads(data, object_pairs_hook=pairs,
                          parse_constant=lambda _: _fail("invalid_json"))
    except (ValueError, UnicodeError):
        _fail("invalid_json")


def _safe_child(root: Path, relative: str) -> Path:
    if (not relative or relative.startswith("/") or "\\" in relative or
            any(part in ("", ".", "..") for part in relative.split("/"))):
        _fail("invalid_retained_path")
    current = root
    for part in relative.split("/")[:-1]:
        current /= part
        if not stat.S_ISDIR(current.lstat().st_mode):
            _fail("invalid_retained_path")
    return root / relative


def _read(path: Path, limit: int = MAX_OUTPUT) -> bytes:
    try:
        fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
        with os.fdopen(fd, "rb") as stream:
            info = os.fstat(stream.fileno())
            if not stat.S_ISREG(info.st_mode) or info.st_size > limit:
                _fail("invalid_retained_file")
            data = stream.read(limit + 1)
            after = os.fstat(stream.fileno())
            current = path.lstat()
            if (len(data) > limit or
                    (info.st_size, info.st_mtime_ns, info.st_ctime_ns) !=
                    (after.st_size, after.st_mtime_ns, after.st_ctime_ns) or
                    (info.st_dev, info.st_ino) != (current.st_dev, current.st_ino)):
                _fail("changed_retained_file")
            return data
    except OSError:
        _fail("retained_file_unavailable")


def _pin(path: str, digest: str) -> None:
    if not Path(path).is_absolute() or not HEX.fullmatch(digest):
        _fail("invalid_tool_binding")
    if _sha(_read(Path(path), 256 * 1024 * 1024)) != digest:
        _fail("tool_binding_changed")


def _bind_source_matches(host_source: str, observed_source: str, prefix: Any) -> bool:
    """Match only the exact paths allowed by the host's pinned engine profile."""
    if prefix is None:
        return observed_source == host_source
    if not isinstance(prefix, dict) or set(prefix) != {"local_root", "engine_root"}:
        _fail("invalid_bind_source_prefix")
    for root in prefix.values():
        if (not isinstance(root, str) or not 2 <= len(root) <= 257 or
                not root.startswith("/") or not re.fullmatch(r"[A-Za-z0-9._/-]+", root) or
                any(part in ("", ".", "..") for part in root[1:].split("/"))):
            _fail("invalid_bind_source_prefix")
    local, engine = prefix["local_root"], prefix["engine_root"]
    if local == engine or local.startswith(engine + "/") or engine.startswith(local + "/"):
        _fail("invalid_bind_source_prefix")
    if observed_source == host_source:
        return True
    return ((host_source == local or host_source.startswith(local + "/")) and
            observed_source == engine + host_source[len(local):])


def _write(path: Path, data: bytes, *, new: bool = False) -> None:
    # The host must retain exclusive custody of the private workspace/ancestors.
    flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW
    temporary = path if new else path.with_name(path.name + "." + uuid.uuid4().hex)
    fd = os.open(temporary, flags, 0o600)
    with os.fdopen(fd, "wb") as stream:
        stream.write(data)
        stream.flush()
        os.fsync(stream.fileno())
    if not new:
        os.replace(temporary, path)
    directory = os.open(path.parent, os.O_RDONLY | os.O_DIRECTORY)
    try:
        os.fsync(directory)
    finally:
        os.close(directory)


@dataclass(frozen=True)
class CommandResult:
    returncode: int
    stdout: bytes = b""
    stderr: bytes = b""


async def _joined(task: asyncio.Task) -> Any:
    """Join an owned task through repeated cancellation before releasing custody."""
    cancellation = None
    while not task.done():
        try:
            await asyncio.shield(task)
        except asyncio.CancelledError as error:
            cancellation = error
        except BaseException:
            break
    if cancellation is not None:
        if not task.cancelled():
            task.exception()
        raise cancellation
    return task.result()


def validate_proposal_host(host_bytes: bytes, *, acquire_images: bool = False) -> dict[str, Any]:
    """Validate only the host envelope, without workspace, planner or engine access.

    Component, placement and engine semantics remain canonical-planner inputs.
    This lets a host reject malformed acquisition scope before client bootstrap.
    """
    if type(acquire_images) is not bool:
        _fail("invalid_image_acquisition")
    if len(host_bytes) > MAX_INPUT:
        _fail("request_limit")
    host = _decode(host_bytes)
    pull_scope = "prepare_agent_selected_local_container_plan_with_image_pull"
    host_keys = {"schema_version", "scope", "owner", "placement", "host", "project", "engine"}
    if (not isinstance(host, dict) or
            set(host) != host_keys | ({"allowed_registries"} if host.get("scope") == pull_scope else set()) or
            type(host["schema_version"]) is not int or host["schema_version"] != 1 or
            not isinstance(host["scope"], str) or
            host["scope"] not in {"prepare_agent_selected_local_container_plan", pull_scope}):
        _fail("invalid_proposal")
    if host["scope"] == pull_scope:
        registries = host["allowed_registries"]
        if (not isinstance(registries, list) or len(registries) > 64 or
                any(not isinstance(value, str) or not re.fullmatch(r"[a-z0-9][a-z0-9.-]{0,252}", value)
                    for value in registries) or len(registries) != len(set(registries))):
            _fail("invalid_image_acquisition")
    if acquire_images and host["scope"] != pull_scope:
        _fail("image_acquisition_not_authorized")
    return host


class _PipeSink:
    """Nonblocking fd output; a stalled protocol consumer cannot freeze cleanup."""
    def __init__(self, fd: int) -> None:
        self.fd = fd
        self.was_blocking = os.get_blocking(fd)
        os.set_blocking(fd, False)

    async def write(self, data: bytes) -> None:
        loop = asyncio.get_running_loop()
        remaining = memoryview(data)
        while remaining:
            try:
                count = os.write(self.fd, remaining)
                if count == 0:
                    raise BrokenPipeError
                remaining = remaining[count:]
            except BlockingIOError:
                ready = loop.create_future()
                def wake() -> None:
                    if not ready.done():
                        ready.set_result(None)
                loop.add_writer(self.fd, wake)
                try:
                    await ready
                finally:
                    loop.remove_writer(self.fd)

    def close(self) -> None:
        os.set_blocking(self.fd, self.was_blocking)


def _environment(entries: list[str] | None) -> dict[str, str]:
    values: dict[str, str] = {}
    for entry in entries or []:
        key, separator, value = entry.partition("=")
        if not separator or not key or key in values:
            _fail("engine_observation_invalid")
        values[key] = value
    return values


def _manifest_descriptor(value: Any, platform: str) -> dict[str, Any] | None:
    if value is None:
        return None
    if (not isinstance(value, dict) or value.get("mediaType") not in {
            "application/vnd.oci.image.manifest.v1+json",
            "application/vnd.docker.distribution.manifest.v2+json"} or
            not isinstance(value.get("digest"), str) or
            not re.fullmatch(r"sha256:[0-9a-f]{64}", value["digest"])):
        _fail("engine_observation_invalid")
    selected = value.get("platform")
    if selected is not None:
        if (not isinstance(selected, dict) or set(selected) - {"os", "architecture", "variant"} or
                not all(isinstance(v, str) and 0 < len(v) <= 64 for v in selected.values()) or
                selected.get("os", "") + "/" + selected.get("architecture", "") != platform):
            _fail("engine_observation_invalid")
    return {"digest": value["digest"], "mediaType": value["mediaType"], "platform": selected}


def _aliases(value: Any) -> list[str]:
    if value is None:
        return []
    if not isinstance(value, list) or len(value) > 256:
        _fail("engine_observation_invalid")
    # Compose may repeat the service name alongside its explicit alias. DNS names
    # are ASCII case-insensitive, including one final root dot. Automatic Compose
    # container names may exceed 63 characters, so bound the whole name instead.
    normalized = set()
    for alias in value:
        if not isinstance(alias, str) or not 0 < len(alias) <= 254 or not alias.isascii():
            _fail("engine_observation_invalid")
        name = alias.removesuffix(".").lower()
        if (not 0 < len(name) <= 253 or not name.isascii() or
                any(not re.fullmatch(r"[a-z0-9](?:[a-z0-9-]*[a-z0-9])?", label)
                    for label in name.split("."))):
            _fail("engine_observation_invalid")
        normalized.add(name)
    return sorted(normalized)


class ProcessRunner:
    """Bounded direct-child owner; child completion says nothing about Docker state."""

    async def run(self, argv: list[str], *, cwd: Path, env: dict[str, str],
                  stdin: bytes = b"", timeout: float = 30.0,
                  limit: int = MAX_OUTPUT) -> CommandResult:
        child = await asyncio.create_subprocess_exec(
            *argv, cwd=cwd, env=env, stdin=asyncio.subprocess.PIPE,
            stdout=asyncio.subprocess.PIPE, stderr=asyncio.subprocess.PIPE, start_new_session=True)

        async def read(stream: asyncio.StreamReader) -> bytes:
            chunks: list[bytes] = []
            size = 0
            while chunk := await stream.read(65536):
                size += len(chunk)
                if size > limit:
                    _fail("command_output_limit")
                chunks.append(chunk)
            return b"".join(chunks)

        async def write() -> None:
            try:
                child.stdin.write(stdin)
                await child.stdin.drain()
            except (BrokenPipeError, ConnectionResetError):
                pass
            finally:
                child.stdin.close()

        tasks = [asyncio.create_task(read(child.stdout)),
                 asyncio.create_task(read(child.stderr)), asyncio.create_task(write())]
        try:
            async with asyncio.timeout(timeout):
                out, err, _ = await asyncio.gather(*tasks)
                code = await child.wait()
            return CommandResult(code, out, err)
        finally:
            async def cleanup() -> None:
                # A reaped leader's numeric group ID can be reused. Signal only
                # while this direct child still establishes ownership, without an
                # intervening await. Descendant completion is not inferred.
                if child.returncode is None:
                    try:
                        os.killpg(child.pid, signal.SIGKILL)
                    except ProcessLookupError:
                        pass
                for task in tasks:
                    task.cancel()
                await asyncio.gather(*tasks, return_exceptions=True)
                async def discard(stream: asyncio.StreamReader) -> None:
                    while await stream.read(65536):
                        pass
                async with asyncio.timeout(5.0):
                    await asyncio.gather(child.wait(), discard(child.stdout), discard(child.stderr))
            await _joined(asyncio.create_task(cleanup()))

    async def stream(self, argv: list[str], *, cwd: Path, env: dict[str, str],
                     stdin: AsyncIterable[bytes], stdout: Callable[[bytes], Awaitable[None]],
                     stderr: Callable[[bytes], Awaitable[None]], timeout: float,
                     limit: int = MAX_STREAM) -> int:
        child = await asyncio.create_subprocess_exec(
            *argv, cwd=cwd, env=env, stdin=asyncio.subprocess.PIPE,
            stdout=asyncio.subprocess.PIPE, stderr=asyncio.subprocess.PIPE, start_new_session=True)

        async def pump_in() -> None:
            size = 0
            try:
                async for chunk in stdin:
                    size += len(chunk)
                    if size > limit:
                        _fail("stream_input_limit")
                    child.stdin.write(chunk)
                    await child.stdin.drain()
            except (BrokenPipeError, ConnectionResetError):
                pass
            finally:
                child.stdin.close()

        async def pump_out(source: asyncio.StreamReader,
                           sink: Callable[[bytes], Awaitable[None]]) -> None:
            size = 0
            while chunk := await source.read(65536):
                size += len(chunk)
                if size > limit:
                    _fail("stream_output_limit")
                await sink(chunk)

        tasks = [asyncio.create_task(pump_in()),
                 asyncio.create_task(pump_out(child.stdout, stdout)),
                 asyncio.create_task(pump_out(child.stderr, stderr))]
        async def completed_output() -> None:
            await asyncio.gather(tasks[1], tasks[2], child.wait())
        completed = asyncio.create_task(completed_output())
        tasks.append(completed)
        try:
            async with asyncio.timeout(timeout):
                # Input errors must fail promptly, but an open input iterator must
                # not prevent a server that already exited from completing.
                await asyncio.wait((tasks[0], completed), return_when=asyncio.FIRST_COMPLETED)
                if tasks[0].done():
                    tasks[0].result()
                await completed
                # Cover input failure and output completion in the same loop turn.
                if tasks[0].done():
                    tasks[0].result()
            return child.returncode
        finally:
            async def cleanup() -> None:
                # A reaped leader's numeric group ID can be reused. Signal only
                # while this direct child still establishes ownership, without an
                # intervening await. Descendant completion is not inferred.
                if child.returncode is None:
                    try:
                        os.killpg(child.pid, signal.SIGKILL)
                    except ProcessLookupError:
                        pass
                for task in tasks:
                    task.cancel()
                await asyncio.gather(*tasks, return_exceptions=True)
                async def discard(stream: asyncio.StreamReader) -> None:
                    while await stream.read(65536):
                        pass
                async with asyncio.timeout(5.0):
                    await asyncio.gather(child.wait(), discard(child.stdout), discard(child.stderr))
            await _joined(asyncio.create_task(cleanup()))


class ComposeRuntime:
    def __init__(self, workspace: str | Path, planner: str | Path, planner_sha256: str,
                 *, runner: ProcessRunner | None = None) -> None:
        self.workspace = Path(workspace).absolute()
        self.planner = str(Path(planner).absolute())
        self.planner_sha256 = planner_sha256
        self.runner = runner or ProcessRunner()
        self._custody: dict[str, Any] | None = None
        self._roots: dict[str, Path] = {}

    def _env(self) -> dict[str, str]:
        return {"HOME": str(self.workspace), "PATH": "/usr/bin:/bin",
                "DOCKER_CONFIG": str(self.workspace / ".docker"),
                "COMPOSE_DISABLE_ENV_FILE": "1", "COMPOSE_MENU": "false",
                "LANG": "C.UTF-8"}

    @contextmanager
    def _lock(self):
        if self.workspace.is_symlink() or not self.workspace.is_dir():
            _fail("workspace_unavailable")
        fd = os.open(self.workspace / ".lock", os.O_RDWR | os.O_CREAT | os.O_NOFOLLOW, 0o600)
        try:
            try:
                fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
            except BlockingIOError:
                _fail("operation_in_progress")
            yield
        finally:
            os.close(fd)

    async def _plan(self, request: bytes) -> dict[str, Any]:
        if len(request) > MAX_INPUT:
            _fail("request_limit")
        _pin(self.planner, self.planner_sha256)
        result = await self.runner.run([self.planner], cwd=self.workspace,
                                       env=self._env(), stdin=request, limit=MAX_PLAN)
        _pin(self.planner, self.planner_sha256)
        if result.returncode != 0 or result.stderr:
            _fail("planning_failed")
        reply = _decode(result.stdout)
        if (not isinstance(reply, dict) or reply.get("schema_version") != 1 or
                reply.get("status") != "prepared" or
                not HEX.fullmatch(reply.get("plan_sha256", "")) or
                not isinstance(reply.get("plan"), dict)):
            _fail("invalid_planner_reply")
        return reply

    async def _plan_successor(self, request: bytes) -> dict[str, Any]:
        if len(request) > MAX_INPUT:
            _fail("request_limit")
        _pin(self.planner, self.planner_sha256)
        result = await self.runner.run([self.planner, "--successor"], cwd=self.workspace,
                                       env=self._env(), stdin=request, limit=MAX_PLAN)
        _pin(self.planner, self.planner_sha256)
        if result.returncode != 0 or result.stderr:
            _fail("successor_planning_failed")
        reply = _decode(result.stdout)
        if (not isinstance(reply, dict) or type(reply.get("schema_version")) is not int or
                reply["schema_version"] not in (1, 2) or
                reply.get("status") != "prepared_successor" or
                reply.get("transition_sha256") != _sha(request)):
            _fail("invalid_planner_reply")
        for side in ("predecessor", "successor"):
            item = reply.get(side)
            if (not isinstance(item, dict) or item.get("schema_version") != 1 or
                    item.get("status") != "prepared" or
                    item.get("plan_sha256") != _sha(_json(item.get("plan")))):
                _fail("invalid_planner_reply")
        return reply

    def _input_root(self, plan: dict[str, Any]) -> Path:
        return self._roots.get(plan.get("request_sha256", ""), self.workspace)

    def _verify_files(self, reply: dict[str, Any], root: Path) -> None:
        for relative, item in reply["plan"]["files"].items():
            path = _safe_child(root, relative)
            if (_read(path) != item["contents"].encode() or
                    stat.S_IMODE(path.stat().st_mode) != item["mode"]):
                _fail("inputs_changed")

    async def _selected(self, key: str) -> tuple[dict[str, Any], bytes, bytes]:
        assert self._custody is not None
        entry = self._custody["selections"][key]
        if entry == {"origin": True}:
            raw = _read(_safe_child(self.workspace, ".successors/origin/request.json"), MAX_INPUT)
            saved = _decode(_read(_safe_child(self.workspace, ".successors/origin/prepared.json"), MAX_PLAN))
            reply = await self._plan(raw)
            root, origin = self.workspace, raw
        elif set(entry) == {"transition"} and HEX.fullmatch(entry["transition"]):
            directory = ".successors/" + entry["transition"]
            raw = _read(_safe_child(self.workspace, directory + "/request.json"), MAX_INPUT)
            result = await self._plan_successor(raw)
            saved = _decode(_read(_safe_child(self.workspace, directory + "/prepared.json"), MAX_PLAN))
            if result != saved or result["transition_sha256"] != entry["transition"]:
                _fail("plan_changed")
            reply = result["successor"]
            envelope = _decode(raw)
            if envelope["schema_version"] == 2:
                origin, raw = envelope["requests"][0].encode(), envelope["requests"][-1].encode()
            else:
                origin, raw = envelope["origin_request"].encode(), envelope["successor_request"].encode()
            root = self.workspace / directory / "next"
            saved = saved["successor"]
        else:
            _fail("custody_changed")
        if reply != saved or reply["plan_sha256"] != key:
            _fail("plan_changed")
        self._verify_files(reply, root)
        self._roots[reply["plan"]["request_sha256"]] = root
        return reply, raw, origin

    async def _retained_requests(self) -> list[str]:
        """Exact ordered inputs from existing completed transition custody."""
        if self._custody is None:
            return [_read(self.workspace / "request.json", MAX_INPUT).decode()]
        origins = [key for key, value in self._custody["selections"].items() if value == {"origin": True}]
        if len(origins) != 1:
            _fail("custody_changed")
        current = origins[0]
        _, raw, _ = await self._selected(current)
        requests = [raw.decode()]
        for transition in self._custody["history"]:
            if transition["from"] != current or transition["result"] != "observed":
                _fail("custody_changed")
            entry = self._custody["selections"].get(transition["to"])
            if entry != {"transition": transition["id"]}:
                _fail("custody_changed")
            proposal, envelope_raw = await self._proposal(transition["id"])
            envelope = _decode(envelope_raw)
            if envelope["schema_version"] == 2:
                if envelope["requests"][:-1] != requests:
                    _fail("custody_changed")
                raw = envelope["requests"][-1]
            else:
                if envelope["origin_request"] != requests[0] or envelope["predecessor_request"] != requests[-1]:
                    _fail("custody_changed")
                raw = envelope["successor_request"]
            if proposal["predecessor"]["plan_sha256"] != current or proposal["successor"]["plan_sha256"] != transition["to"]:
                _fail("custody_changed")
            self._validate_migration_record(transition, proposal, completed=True)
            current = transition["to"]
            requests.append(raw)
        pending = self._custody["transition"]
        if current != (pending["from"] if pending is not None else self._custody["active"]) or len(requests) > 32:
            _fail("custody_changed")
        return requests

    async def propose(self, host_bytes: bytes, selection_bytes: bytes, *,
                      acquire_images: bool = False) -> dict[str, Any]:
        """Prepare an agent-selected graph, optionally acquiring host-authorized images.

        The host supplies placement and engine bindings, not component choices.
        The canonical planner remains the owner of graph and image-source rules.
        Only the final proposal receipt marks complete publication; an interrupted
        workspace is retained and must not be treated as an executable deployment.
        """
        if type(acquire_images) is not bool:
            _fail("invalid_image_acquisition")
        if len(host_bytes) > MAX_INPUT or len(selection_bytes) > MAX_INPUT:
            _fail("request_limit")
        host = validate_proposal_host(host_bytes, acquire_images=acquire_images)
        selected = _decode(selection_bytes)
        staged_keys = {"schema_version", "package_json", "manifest_json", "configuration", "policy", "files"}
        if not isinstance(selected, dict) or set(selected) != staged_keys | {"components", "volumes"}:
            _fail("invalid_proposal")
        try:
            manifest = _decode(selected["manifest_json"].encode("utf-8"))
            component_settings = selected["components"]
            if (not isinstance(component_settings, dict) or not 1 <= len(component_settings) <= 16 or
                    any(not isinstance(value, dict) or "image_id" in value for value in component_settings.values()) or
                    not isinstance(selected["files"], dict) or len(selected["files"]) > 64):
                _fail("invalid_proposal")
            # Provisional IDs are never published. For a local image source this
            # is its actual identity; registry sources are lowered by the planner.
            provisional = {c["id"]: "sha256:" + c["source"]["content_digest"]
                           for c in manifest["components"]}
            if set(provisional) != set(component_settings):
                _fail("invalid_proposal")
            profile = {key: host[key] for key in ("schema_version", "owner", "placement", "host", "project", "engine")}
            profile.update(components={key: dict(value, image_id=provisional[key])
                                       for key, value in component_settings.items()},
                           volumes=selected["volumes"],
                           files={key: _sha(value.encode("utf-8")) for key, value in selected["files"].items()})
            inputs = {key: selected[key] for key in staged_keys}
        except (KeyError, TypeError, ValueError, AttributeError):
            _fail("invalid_proposal")

        def assembled() -> tuple[bytes, bytes]:
            # The setup bridge uses this exact encoding, including its newline.
            profile_raw = _json(profile) + b"\n"
            request = dict(inputs, profile_json=profile_raw.decode("utf-8"), profile_sha256=_sha(profile_raw))
            return profile_raw, _json(request) + b"\n"

        # Refuse aliases and existing destinations, preserving prior attempts.
        if (self.workspace != self.workspace.resolve() or
                not self.workspace.parent.is_dir()):
            _fail("workspace_unavailable")
        try:
            self.workspace.mkdir(mode=0o700)
        except FileExistsError:
            _fail("workspace_exists")
        with self._lock():
            config = self.workspace / ".docker"
            config.mkdir(mode=0o700)
            _write(config / "config.json", b"{}\n", new=True)
            os.chmod(config / "config.json", 0o400)
            _, request = assembled()
            draft = await self._plan(request)
            registry_images = sorted({(component["image"], component["platform"])
                             for component in draft["plan"]["components"].values()
                             if not component["image"].startswith("sha256:")})
            if acquire_images and any(image.split("/", 1)[0] not in host["allowed_registries"]
                                      for image, _ in registry_images):
                _fail("image_registry_not_authorized")
            acquisition = None
            try:
                await self._engine(draft["plan"])
                if acquire_images:
                    # Retain the exact selection and full intent before any pull.
                    # A client exit is not proof that the engine made no changes.
                    _write(self.workspace / "host-profile.json", host_bytes, new=True)
                    _write(self.workspace / "selection.json", selection_bytes, new=True)
                    acquisition = {"schema_version": 1, "scope": "registry_image_acquisition_only",
                                   "status": "pending", "host_profile_sha256": _sha(host_bytes),
                                   "selection_sha256": _sha(selection_bytes),
                                   "draft_plan_sha256": draft["plan_sha256"],
                                   "images": [{"image": image, "platform": platform, "status": "not_started"}
                                              for image, platform in registry_images]}
                    _write(self.workspace / "image-acquisition.json", _json(acquisition) + b"\n", new=True)
                    for image in acquisition["images"]:
                        await self._engine(draft["plan"])
                        if _read(config / "config.json") != b"{}\n":
                            _fail("docker_config_changed")
                        image["status"] = "pending"
                        _write(self.workspace / "image-acquisition.json", _json(acquisition) + b"\n")
                        result = await self._docker(draft["plan"], ["image", "pull", "--platform",
                            image["platform"], image["image"]], timeout=IMAGE_PULL_TIMEOUT)
                        if result.returncode != 0:
                            _fail("image_acquisition_failed")
                        image["status"] = "client_completed"
                        _write(self.workspace / "image-acquisition.json", _json(acquisition) + b"\n")
                for key, component in draft["plan"]["components"].items():
                    images = await self._object(draft["plan"], ["image", "inspect", "--platform",
                                               component["platform"], component["image"]])
                    if (not isinstance(images, list) or len(images) != 1 or
                            not isinstance(images[0], dict) or
                            not isinstance(images[0].get("Id"), str) or
                            not images[0]["Id"].startswith("sha256:") or
                            not HEX.fullmatch(images[0]["Id"][7:]) or
                            images[0].get("Os", "") + "/" + images[0].get("Architecture", "") != component["platform"]):
                        _fail("engine_observation_invalid")
                    profile["components"][key]["image_id"] = images[0]["Id"]
                profile_raw, request = assembled()
                final = await self._plan(request)
                for key in final["plan"]["components"]:
                    await self._image_facts(final["plan"], key)
                await self._engine(final["plan"])
                # Recheck tool pins after the last observation, before publication.
                self._argv(final["plan"], [])
                _pin(self.planner, self.planner_sha256)
                if acquisition is not None:
                    if (_read(self.workspace / "host-profile.json", MAX_INPUT) != host_bytes or
                            _read(self.workspace / "selection.json", MAX_INPUT) != selection_bytes or
                            _read(config / "config.json") != b"{}\n"):
                        _fail("proposal_inputs_changed")
                    acquisition["status"] = "observed"
                    acquisition["plan_sha256"] = final["plan_sha256"]
                    for image in acquisition["images"]:
                        matching = [component for component in final["plan"]["components"].values()
                                    if (component["image"], component["platform"]) == (image["image"], image["platform"])]
                        image.update(status="observed", image_id=matching[0]["image_id"])
                    _write(self.workspace / "image-acquisition.json", _json(acquisition) + b"\n")
            except (KeyError, TypeError, ValueError, IndexError, AttributeError):
                if acquisition is not None:
                    acquisition["status"] = "unconfirmed"
                    _write(self.workspace / "image-acquisition.json", _json(acquisition) + b"\n")
                _fail("engine_observation_invalid")
            except BaseException:
                if acquisition is not None:
                    acquisition["status"] = "unconfirmed"
                    _write(self.workspace / "image-acquisition.json", _json(acquisition) + b"\n")
                raise
            outputs = {"host-profile.json": host_bytes, "selection.json": selection_bytes,
                       "backend-profile.json": profile_raw, "inputs.json": _json(inputs) + b"\n",
                       "request.json": request, "prepared.json": _json(final) + b"\n"}
            if acquisition is not None:
                outputs["image-acquisition.json"] = _json(acquisition) + b"\n"
            receipt = {"schema_version": 1, "status": "proposed", "scope": "compose_plan_for_host_review",
                       "plan_sha256": final["plan_sha256"],
                       "files": {name: _sha(raw) for name, raw in outputs.items()}}
            for name, raw in outputs.items():
                if acquisition is not None and name in {"host-profile.json", "selection.json", "image-acquisition.json"}:
                    if _read(self.workspace / name, MAX_PLAN) != raw:
                        _fail("proposal_inputs_changed")
                else:
                    _write(self.workspace / name, raw, new=True)
            _write(self.workspace / "proposal.json", _json(receipt) + b"\n", new=True)
            return receipt

    async def prepare(self, request: bytes) -> dict[str, Any]:
        try:
            self.workspace.mkdir(mode=0o700)
        except FileExistsError:
            _fail("workspace_exists")
        with self._lock():
            reply = await self._plan(request)
            plan = reply["plan"]
            for relative, item in plan["files"].items():
                path = Path(relative)
                if path.is_absolute() or any(p in (".", "..") for p in path.parts) or "\\" in relative:
                    _fail("invalid_planner_files")
                if relative != "compose.json" and not relative.startswith("inputs/"):
                    _fail("invalid_planner_files")
                data = item["contents"].encode()
                if _sha(data) != item["sha256"]:
                    _fail("invalid_planner_files")
                target = self.workspace / path
                target.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
                _write(target, data, new=True)
                os.chmod(target, item["mode"])
            if _read(self.workspace / "compose.json").decode() != plan["compose_json"]:
                _fail("invalid_planner_files")
            _write(self.workspace / "request.json", request, new=True)
            _write(self.workspace / "prepared.json", _json(reply), new=True)
            _write(self.workspace / "record.json", _json({"schema_version": 1,
                   "plan_sha256": reply["plan_sha256"], "attempted": False,
                   "pending": False, "containers": {}, "network": None,
                   "volumes": {}, "requests": {}}), new=True)
            config = self.workspace / ".docker"
            plugins = config / "cli-plugins"
            plugins.mkdir(mode=0o700, parents=True)
            _write(config / "config.json", b"{}\n", new=True)
            compose = plan["engine"]["compose"]
            _pin(compose["path"], compose["sha256"])
            os.symlink(compose["path"], plugins / "docker-compose")
            return reply

    async def _load(self, expected: str | None = None, *, allow_transition: bool = False
                    ) -> tuple[dict[str, Any], dict[str, Any]]:
        self._custody, self._roots = None, {}
        request = _read(self.workspace / "request.json", MAX_INPUT)
        if request == SUCCESSOR_MARKER:
            custody = _decode(_read(self.workspace / "custody.json", MAX_PLAN))
            if (type(custody.get("schema_version")) is not int or custody["schema_version"] not in (2, 3)
                    or custody.get("workspace") != str(self.workspace.resolve())
                    or not isinstance(custody.get("selections"), dict)
                    or not 1 <= len(custody["selections"]) <= 32):
                _fail("custody_changed")
            self._custody = custody
            reply, _, _ = await self._selected(custody["active"])
            record = custody["records"][custody["active"]]
            if custody["schema_version"] == 3:
                retained = await self._retained_requests()
            pending = custody["transition"]
            if pending is not None:
                if (pending["phase"] not in ("retire", "migrate", "activate") or
                        custody["active"] != pending["to" if pending["phase"] == "activate" else "from"]):
                    _fail("custody_changed")
                proposal, raw = await self._proposal(pending["id"])
                if (proposal["predecessor"]["plan_sha256"] != pending["from"] or
                        proposal["successor"]["plan_sha256"] != pending["to"]):
                    _fail("custody_changed")
                self._validate_migration_record(pending, proposal, completed=pending["phase"] == "activate")
                envelope = _decode(raw)
                if custody["schema_version"] == 3:
                    if envelope["schema_version"] == 2:
                        if envelope["requests"][:-1] != retained:
                            _fail("custody_changed")
                    elif envelope["origin_request"] != retained[0] or envelope["predecessor_request"] != retained[-1]:
                        _fail("custody_changed")
            if custody["transition"] is not None and not allow_transition:
                _fail("successor_unconfirmed")
        else:
            saved = _decode(_read(self.workspace / "prepared.json", MAX_PLAN))
            reply = await self._plan(request)
            if reply != saved:
                _fail("plan_changed")
            self._verify_files(reply, self.workspace)
            record = _decode(_read(self.workspace / "record.json"))
        if expected is not None and expected != reply["plan_sha256"]:
            _fail("plan_changed")
        plan = reply["plan"]
        if record.get("plan_sha256") != reply["plan_sha256"]:
            _fail("record_changed")
        config = self.workspace / ".docker"
        if set(p.name for p in config.iterdir()) != {"config.json", "cli-plugins"}:
            _fail("docker_config_changed")
        if set(p.name for p in (config / "cli-plugins").iterdir()) != {"docker-compose"}:
            _fail("docker_config_changed")
        if _read(_safe_child(self.workspace, ".docker/config.json")) != b"{}\n":
            _fail("docker_config_changed")
        plugin = config / "cli-plugins" / "docker-compose"
        if not plugin.is_symlink() or os.readlink(plugin) != plan["engine"]["compose"]["path"]:
            _fail("docker_config_changed")
        return reply, record

    def _save(self, record: dict[str, Any]) -> None:
        if self._custody is None:
            _write(self.workspace / "record.json", _json(record))
        else:
            if record["plan_sha256"] != self._custody["active"]:
                _fail("custody_changed")
            self._custody["records"][self._custody["active"]] = record
            self._save_custody()

    def _save_custody(self) -> None:
        if self._custody is None or _read(self.workspace / "request.json", MAX_INPUT) != SUCCESSOR_MARKER:
            _fail("custody_changed")
        data = _json(self._custody)
        if len(data) > MAX_PLAN:
            _fail("custody_limit")
        _write(self.workspace / "custody.json", data)

    def _argv(self, plan: dict[str, Any], args: list[str], *, compose: bool = False) -> list[str]:
        engine = plan["engine"]
        for tool in ("docker", "compose"):
            _pin(engine[tool]["path"], engine[tool]["sha256"])
        argv = [engine["docker"]["path"], "--host", engine["endpoint"], "--config",
                str(self.workspace / ".docker")]
        if compose:
            argv += ["compose", "--project-name", plan["project"], "--file",
                     str(self._input_root(plan) / "compose.json"), "--project-directory",
                     str(self._input_root(plan))]
        return argv + args

    async def _docker(self, plan: dict[str, Any], args: list[str], *, compose: bool = False,
                      timeout: float = 30.0, stdin: bytes = b"") -> CommandResult:
        return await self.runner.run(self._argv(plan, args, compose=compose),
                                     cwd=self.workspace, env=self._env(), timeout=timeout, stdin=stdin)

    async def _object(self, plan: dict[str, Any], args: list[str]) -> Any:
        result = await self._docker(plan, args)
        if result.returncode != 0:
            _fail("engine_observation_unavailable")
        return _decode(result.stdout)

    async def _ids(self, plan: dict[str, Any], args: list[str]) -> list[str]:
        result = await self._docker(plan, args)
        if result.returncode != 0:
            _fail("engine_observation_unavailable")
        try:
            ids = result.stdout.decode("ascii").splitlines()
        except UnicodeError:
            _fail("engine_observation_invalid")
        if len(ids) > 128 or len(ids) != len(set(ids)) or any(not x or len(x) > 256 for x in ids):
            _fail("engine_observation_invalid")
        return sorted(ids)

    async def _engine(self, plan: dict[str, Any]) -> None:
        info = await self._object(plan, ["info", "--format", "{{json .ID}}"])
        if info != plan["engine"]["id"]:
            _fail("engine_changed")

    def _container(self, plan: dict[str, Any], raw: dict[str, Any],
                   requests: dict[str, Any], image_environments: dict[str, Any],
                   image_descriptors: dict[str, Any] | None = None, *,
                   isolated: bool = False) -> dict[str, Any]:
        compose = _decode(plan["compose_json"].encode())
        config, host = raw["Config"], raw["HostConfig"]
        state = raw["State"]
        if (not isinstance(raw["Id"], str) or not HEX.fullmatch(raw["Id"]) or
                type(state["Running"]) is not bool or state["Status"] not in
                {"created", "running", "paused", "restarting", "removing", "exited", "dead"}):
            _fail("engine_observation_invalid")
        labels = config.get("Labels") or {}
        service = labels.get("com.docker.compose.service")
        selected = [(key, value) for key, value in plan["components"].items()
                    if value["service"] == service]
        if len(selected) != 1:
            _fail("resource_mismatch")
        component, expected = selected[0]
        name = raw["Name"].lstrip("/")
        request = expected["lifetime"] == "request"
        if request and (name not in requests or requests[name]["component"] != component):
            _fail("resource_mismatch")
        if not request and labels.get("com.docker.compose.oneoff", "false").lower() != "false":
            _fail("resource_mismatch")
        service_config = compose["services"][service]
        required_labels = dict(service_config.get("labels", {}))
        required_labels.update({"com.docker.compose.project": plan["project"],
                                "com.docker.compose.service": service})
        if any(labels.get(key) != value for key, value in required_labels.items()):
            _fail("resource_mismatch")
        descriptor = _manifest_descriptor(raw.get("ImageManifestDescriptor"), expected["platform"])
        inspected = (image_descriptors or {}).get(component)
        if descriptor is not None:
            if (inspected is None or descriptor["digest"] != inspected["digest"] or
                    descriptor["mediaType"] != inspected["mediaType"] or
                    (descriptor["platform"] is not None and inspected["platform"] is not None and
                     descriptor["platform"] != inspected["platform"])):
                _fail("resource_mismatch")
        if raw["Image"] != expected["image_id"]:
            # Some engines retain the immutable registry index on the container,
            # and expose its selected platform manifest separately. Join both
            # identities through the engine's platform-specific image inspection.
            source_digest = expected["image"].rpartition("@sha256:")[2]
            if (not HEX.fullmatch(source_digest) or raw["Image"] != "sha256:" + source_digest or
                    descriptor is None or inspected is None or descriptor["platform"] is None or
                    inspected["platform"] is None or descriptor != inspected):
                _fail("resource_mismatch")
        if (config.get("Image") != expected["image"] or
                (config.get("Entrypoint") or []) != service_config.get("entrypoint", []) or
                (config.get("Cmd") or []) != service_config.get("command", []) or
                config.get("WorkingDir", "") != service_config.get("working_dir", "") or
                config.get("User", "") != expected["user"]):
            _fail("resource_mismatch")
        environment = _environment(config.get("Env"))
        expected_environment = dict(image_environments[component])
        expected_environment.update(expected["environment"])
        if environment != expected_environment:
            _fail("resource_mismatch")
        if host.get("Privileged") or host.get("PublishAllPorts") or host.get("PortBindings"):
            _fail("resource_mismatch")
        if (host.get("Devices") or host.get("CapAdd") or not host.get("Init") or
                host.get("RestartPolicy", {}).get("Name") != "no" or
                host.get("MemorySwap") != service_config["memswap_limit"]):
            _fail("resource_mismatch")
        for key, expected_value in (("Memory", service_config["mem_limit"]),
                                    ("PidsLimit", service_config["pids_limit"]),
                                    ("NanoCpus", round(float(service_config["cpus"]) * 1_000_000_000))):
            if host.get(key) != expected_value:
                _fail("resource_mismatch")
        networks = raw.get("NetworkSettings", {}).get("Networks", {})
        if isolated:
            if (host.get("NetworkMode") != "none" or set(networks) not in (set(), {"none"}) or
                    host.get("ReadonlyRootfs") is not service_config["read_only"] or
                    host.get("CapDrop") != ["ALL"] or
                    host.get("SecurityOpt") != ["no-new-privileges"] or
                    config.get("OpenStdin") is not True):
                _fail("resource_mismatch")
            network_id, aliases = None, []
        else:
            network = compose["networks"]["default"]["name"]
            if set(networks) != {network}:
                _fail("resource_mismatch")
            network_id = networks[network]["NetworkID"]
            aliases = _aliases(networks[network].get("Aliases"))
            required_aliases = set(_aliases(service_config["networks"]["default"].get("aliases")))
            reserved_elsewhere = set()
            for other_service, definition in compose["services"].items():
                if other_service != service:
                    reserved_elsewhere.update(_aliases(definition["networks"]["default"].get("aliases")))
            if (reserved_elsewhere.intersection(aliases) or
                    (not request and state["Running"] and not required_aliases.issubset(aliases))):
                _fail("resource_mismatch")
        actual_mounts = raw.get("Mounts", [])
        mounts = service_config.get("volumes", [])
        if len(actual_mounts) != len(mounts):
            _fail("resource_mismatch")
        observed_mounts = []
        for mount in mounts:
            matching = [m for m in actual_mounts if m["Destination"] == mount["target"]]
            if len(matching) != 1:
                _fail("resource_mismatch")
            observed = matching[0]
            source = mount["source"]
            host_source = None
            if mount["type"] == "bind":
                source = str((self._input_root(plan) / source).resolve())
                host_source = source
                actual_source = observed["Source"]
                source_matches = _bind_source_matches(source, actual_source,
                    plan.get("engine", {}).get("bind_source_prefix"))
            else:
                source = compose["volumes"][source]["name"]
                actual_source = observed.get("Name")
                source_matches = actual_source == source
            if (observed["Type"] != mount["type"] or not source_matches or
                    observed["RW"] != (not mount.get("read_only", False))):
                _fail("resource_mismatch")
            item = {"type": observed["Type"], "source": actual_source,
                    "target": observed["Destination"], "rw": observed["RW"]}
            if host_source is not None:
                item["host_source"] = host_source
            observed_mounts.append(item)
        health_config = service_config["healthcheck"]
        actual_health = config.get("Healthcheck") or {}
        if health_config.get("disable"):
            if actual_health.get("Test") != ["NONE"]:
                _fail("resource_mismatch")
        else:
            for key, wanted in (("Test", health_config["test"]),
                                ("Retries", health_config["retries"]),
                                ("Interval", int(health_config["interval"][:-2]) * 1_000_000),
                                ("Timeout", int(health_config["timeout"][:-2]) * 1_000_000),
                                ("StartPeriod", int(health_config["start_period"][:-2]) * 1_000_000)):
                actual = actual_health.get(key)
                if key == "StartPeriod":
                    # Docker omits this integer duration when its value is zero.
                    if key not in actual_health and wanted == 0:
                        continue
                    if type(actual) is not int:
                        _fail("resource_mismatch")
                if actual != wanted:
                    _fail("resource_mismatch")
        health = raw.get("State", {}).get("Health", {}).get("Status")
        return {"id": raw["Id"], "name": name, "component": component,
                "kind": "request" if request else "deployment", "image_id": expected["image_id"],
                "container_image_id": raw["Image"], "image_manifest": descriptor,
                "running": raw["State"]["Running"], "status": raw["State"]["Status"],
                "exit_code": raw["State"].get("ExitCode"), "health": health,
                "network_id": network_id, "aliases": aliases, "mounts": observed_mounts}

    async def _observe(self, reply: dict[str, Any], record: dict[str, Any]) -> dict[str, Any]:
        try:
            return await self._inspect(reply, record)
        except (KeyError, TypeError, ValueError, IndexError, AttributeError):
            _fail("engine_observation_invalid")

    async def _image_facts(self, plan: dict[str, Any], component: str) -> tuple[dict[str, str], dict[str, Any] | None]:
        """Validate one selected image with the same checks used by full admission."""
        expected = plan["components"][component]
        compose = _decode(plan["compose_json"].encode())
        images = await self._object(plan, ["image", "inspect", "--platform",
                                          expected["platform"], expected["image"]])
        if (len(images) != 1 or images[0]["Id"] != expected["image_id"] or
                images[0]["Os"] + "/" + images[0]["Architecture"] != expected["platform"]):
            _fail("image_changed")
        config = images[0].get("Config")
        if config is None:
            config = {}
        if not isinstance(config, dict):
            _fail("engine_observation_invalid")
        volumes = config.get("Volumes")
        if volumes is None:
            volumes = {}
        if not isinstance(volumes, dict):
            _fail("engine_observation_invalid")
        targets = {mount["target"] for mount in
                   compose["services"][expected["service"]].get("volumes", [])}
        for target, options in volumes.items():
            if (not isinstance(target, str) or not target.startswith("/") or
                    "\\" in target or any(ord(c) < 32 or ord(c) == 127 for c in target) or
                    any(part in ("", ".", "..") for part in target[1:].split("/")) or
                    not isinstance(options, dict) or options):
                _fail("engine_observation_invalid")
            if target not in targets:
                # Docker would otherwise create anonymous state before the
                # post-creation mount observation can reject the container.
                _fail("image_volume_unbound")
        return (_environment(config.get("Env")),
                _manifest_descriptor(images[0].get("Descriptor"), expected["platform"]))

    async def _inspect(self, reply: dict[str, Any], record: dict[str, Any]) -> dict[str, Any]:
        plan = reply["plan"]
        await self._engine(plan)
        workers = await self._migration_workers()
        compose = _decode(plan["compose_json"].encode())
        image_environments = {}
        image_descriptors = {}
        for component in plan["components"]:
            image_environments[component], image_descriptors[component] = await self._image_facts(plan, component)
        project_filter = "label=com.docker.compose.project=" + plan["project"]
        ids = await self._ids(plan, ["ps", "--all", "--quiet", "--no-trunc", "--filter", project_filter])
        containers: dict[str, Any] = {}
        if ids:
            for raw in await self._object(plan, ["container", "inspect", *ids]):
                if raw["Id"] in workers:
                    continue
                observed = self._container(plan, raw, record["requests"], image_environments, image_descriptors)
                if observed["id"] not in ids or observed["id"] in containers:
                    _fail("engine_observation_invalid")
                containers[observed["id"]] = observed
        if set(containers) | set(workers) != set(ids):
            _fail("engine_observation_invalid")
        for identity, previous in record["containers"].items():
            # Disappearance is reported; an unexpected replacement is refused below.
            if identity in containers and previous["component"] != containers[identity]["component"]:
                _fail("resource_mismatch")
        for identity, item in containers.items():
            same_role = [old for old in record["containers"].values()
                         if (old["name"] == item["name"] or
                             (item["kind"] == "deployment" and old["component"] == item["component"]))
                         and old["id"] != identity]
            if same_role:
                _fail("resource_replaced")
        network_ids = await self._ids(plan, ["network", "ls", "--quiet", "--no-trunc",
                                            "--filter", project_filter])
        named_networks = await self._ids(plan, ["network", "ls", "--quiet", "--no-trunc",
            "--filter", "name=^" + re.escape(compose["networks"]["default"]["name"]) + "$"])
        if set(network_ids) != set(named_networks) or len(network_ids) > 1:
            _fail("resource_mismatch")
        network = None
        if network_ids:
            networks = await self._object(plan, ["network", "inspect", *network_ids])
            if len(networks) != 1 or networks[0]["Id"] != network_ids[0] or not HEX.fullmatch(network_ids[0]):
                _fail("engine_observation_invalid")
            raw = networks[0]
            if (raw["Name"] != compose["networks"]["default"]["name"] or
                    not raw["Internal"] or raw["Driver"] != "bridge" or
                    raw.get("Labels", {}).get("com.docker.compose.project") != plan["project"] or
                    any(raw.get("Labels", {}).get(k) != v for k, v in
                        compose["networks"]["default"].get("labels", {}).items())):
                _fail("resource_mismatch")
            if any(key not in containers for key in (raw.get("Containers") or {})):
                _fail("foreign_network_member")
            network = {"id": raw["Id"], "name": raw["Name"]}
            if record["network"] and record["network"] != network:
                _fail("resource_replaced")
        for item in containers.values():
            if item["running"] and (not network or item["network_id"] != network["id"]):
                _fail("resource_mismatch")
        volumes: dict[str, Any] = {}
        expected_names = {v["name"] for v in plan["volumes"].values()}
        project_volumes = await self._ids(plan, ["volume", "ls", "--quiet", "--filter", project_filter])
        if not set(project_volumes) <= expected_names:
            _fail("resource_mismatch")
        for namespace, expected in plan["volumes"].items():
            names = await self._ids(plan, ["volume", "ls", "--quiet", "--filter",
                                          "name=^" + re.escape(expected["name"]) + "$"])
            if names:
                if names != [expected["name"]]:
                    _fail("resource_mismatch")
                raw = (await self._object(plan, ["volume", "inspect", expected["name"]]))[0]
                if (raw["Name"] != expected["name"] or raw["Driver"] != "local" or raw.get("Options") or
                        raw.get("Labels", {}).get("com.docker.compose.project") != plan["project"] or
                        any(raw.get("Labels", {}).get(k) != v for k, v in
                            plan.get("retained_volume_labels", {}).get(namespace,
                                compose["volumes"][expected["name"]].get("labels", {})).items())):
                    _fail("resource_mismatch")
                item = {"name": raw["Name"], "created_at": raw.get("CreatedAt"),
                        "mountpoint": raw["Mountpoint"], "driver": raw["Driver"]}
                if namespace in record["volumes"] and record["volumes"][namespace] != item:
                    _fail("resource_replaced")
                volumes[namespace] = item
                users = await self._ids(plan, ["ps", "--all", "--quiet", "--no-trunc", "--filter",
                                               "volume=" + expected["name"]])
                allowed_workers = {identity for identity, worker in workers.items()
                                   if worker["volume"] == expected["name"]}
                if any(identity not in containers and identity not in allowed_workers for identity in users):
                    _fail("foreign_volume_user")
        availability: dict[str, bool] = {}
        for component in plan["deployment_order"]:
            expected = plan["components"][component]
            matches = [item for item in containers.values() if item["component"] == component]
            if len(matches) > 1:
                _fail("resource_mismatch")
            availability[component] = bool(len(matches) == 1 and matches[0]["running"] and matches[0]["status"] == "running" and
                (expected["readiness"] == "running" or matches[0]["health"] == "healthy") and
                all(availability.get(dep, False) for dep in expected["deployment_dependencies"]))
        observation = {"schema_version": 1, "plan_sha256": reply["plan_sha256"],
                       "engine_id": plan["engine"]["id"], "containers": containers,
                       "network": network, "volumes": volumes, "available": availability,
                       "pending_operation": record["pending"]}
        if self._custody is not None and self._custody["transition"] is not None and "migration" in self._custody["transition"]:
            observation["migration_workers"] = workers
        observation["observation_sha256"] = _sha(_json(observation))
        return observation

    @staticmethod
    def _prepared_summary(reply: dict[str, Any]) -> dict[str, Any]:
        """Closed metadata projection; its hash identifies the full verified plan."""
        plan = reply["plan"]
        summary = {key: plan[key] for key in (
            "request_sha256", "package_sha256", "manifest_sha256", "manifest_source_sha256",
            "profile_sha256", "configuration_sha256", "policy_sha256", "host_owner", "project",
            "initialization_order", "shutdown_order", "deployment_order", "request_components")}
        summary["components"] = {name: {key: item[key] for key in (
            "service", "lifetime", "image", "image_id", "platform", "dependencies",
            "deployment_dependencies", "readiness", "stop_timeout_ms")}
            for name, item in plan["components"].items()}
        summary["files"] = {name: {"sha256": item["sha256"], "mode": item["mode"]}
                            for name, item in plan["files"].items()}
        return {"schema_version": 1, "status": "prepared_summary", "plan_sha256": reply["plan_sha256"],
                "summary": summary}

    async def selection(self, transition_sha256: str | None = None) -> dict[str, Any]:
        """Read verified retained selection without observing or changing the engine.

        This projection grants no execution authority. A caller may separately
        select one exact retained proposal for host admission/rebind checks.
        """
        with self._lock():
            reply, record = await self._load(allow_transition=True)
            if type(record.get("pending")) is not bool:
                _fail("record_changed")
            pending = selected_from = None
            if self._custody is not None:
                selected_from = self._custody["selections"][self._custody["active"]].get("transition")
                transition = self._custody["transition"]
                if transition is not None:
                    pending = {"transition_sha256": transition["id"], "phase": transition["phase"],
                        "predecessor_plan_sha256": transition["from"],
                        "successor_plan_sha256": transition["to"]}
            proposal = None
            if transition_sha256 is not None:
                value, _ = await self._proposal(transition_sha256)
                proposal = {"schema_version": value["schema_version"], "status": "prepared_successor_summary",
                    "transition_sha256": value["transition_sha256"],
                    "predecessor": self._prepared_summary(value["predecessor"]),
                    "successor": self._prepared_summary(value["successor"])}
            return {"schema_version": 1, "status": "selected_plan", "selected": self._prepared_summary(reply),
                "state_namespaces": sorted(reply["plan"]["volumes"]),
                "selected_from_transition_sha256": selected_from,
                "pending_transition": pending, "pending_operation": record["pending"], "proposal": proposal}

    async def observe(self) -> dict[str, Any]:
        with self._lock():
            reply, record = await self._load(allow_transition=True)
            if self._custody is not None and self._custody["transition"] is not None:
                return await self._observe_transition(reply, record)
            return await self._observe(reply, record)

    def _capture(self, record: dict[str, Any], observation: dict[str, Any]) -> None:
        record["containers"].update(observation["containers"])
        record["network"] = observation["network"] or record["network"]
        record["volumes"].update(observation["volumes"])
        self._save(record)

    async def _finish(self, reply: dict[str, Any], record: dict[str, Any], *,
                      success: bool) -> dict[str, Any]:
        record["pending"] = not success
        self._save(record)
        try:
            observation = await self._observe(reply, record)
            self._capture(record, observation)
        except (ComposeError, KeyError, TypeError, ValueError, OSError, TimeoutError):
            return {"status": "unconfirmed", "plan_sha256": reply["plan_sha256"],
                    "observation": None}
        return {"status": "observed" if success else "unconfirmed", "observation": observation,
                "plan_sha256": reply["plan_sha256"]}

    async def start(self, plan_sha256: str) -> dict[str, Any]:
        with self._lock():
            reply, record = await self._load(plan_sha256)
            observation = await self._observe(reply, record)
            if record["pending"]:
                _fail("operation_unconfirmed")
            if not record["attempted"]:
                if observation["containers"] or observation["network"] or observation["volumes"]:
                    _fail("project_not_absent")
            else:
                deployments = [item for item in observation["containers"].values()
                               if item["kind"] == "deployment"]
                if len(deployments) != len(reply["plan"]["deployment_order"]):
                    _fail("resources_incomplete")
                if all(observation["available"].values()):
                    return {"status": "observed", "plan_sha256": plan_sha256,
                            "observation": observation}
                if any(item["running"] for item in deployments):
                    _fail("resources_not_stopped")
            return await self._start_effect(reply, record, observation)

    async def resume(self, plan_sha256: str, observation_sha256: str) -> dict[str, Any]:
        """Explicit new attempt after current readback, without replacing known resources."""
        with self._lock():
            reply, record = await self._load(plan_sha256)
            observation = await self._observe(reply, record)
            if observation["observation_sha256"] != observation_sha256:
                _fail("stale_observation")
            if not record["attempted"]:
                _fail("deployment_not_started")
            if any(item["kind"] == "request" and item["running"]
                   for item in observation["containers"].values()):
                _fail("request_in_progress")
            if any(item["kind"] == "deployment" and identity not in observation["containers"]
                   for identity, item in record["containers"].items()):
                _fail("resources_disappeared")
            return await self._start_effect(reply, record, observation)

    async def _start_effect(self, reply: dict[str, Any], record: dict[str, Any],
                            observation: dict[str, Any]) -> dict[str, Any]:
        record["attempted"] = True
        record["pending"] = True
        self._save(record)
        success = False
        try:
            success = True
            current = observation
            # Canonical dependencies gate each role, but not unrelated roles.
            for component in reply["plan"]["deployment_order"]:
                expected = reply["plan"]["components"][component]
                if any(not current["available"].get(dep, False)
                       for dep in expected["deployment_dependencies"]):
                    success = False
                    continue
                selected = [item for item in current["containers"].values()
                            if item["component"] == component]
                if current["available"].get(component):
                    continue
                if (self._custody is not None and self._custody["transition"] is not None
                        and self._custody["transition"]["phase"] == "activate"
                        and (not selected or not selected[0]["running"])):
                    attempted = self._custody["transition"]["attempted_components"]
                    if component in attempted:
                        _fail("successor_effect_unresolved")
                    attempted[component] = {"kind": "start" if selected else "create",
                        "container_id": selected[0]["id"] if selected else None}
                    self._save_custody()
                if selected:
                    result = (CommandResult(0) if selected[0]["running"] else
                              await self._docker(reply["plan"],
                                  ["container", "start", selected[0]["id"]], timeout=120))
                else:
                    result = await self._docker(reply["plan"],
                        ["up", "--detach", "--no-build", "--pull", "never", "--no-recreate",
                         "--no-deps", expected["service"]], compose=True, timeout=120)
                if result.returncode != 0:
                    success = False
                deadline = asyncio.get_running_loop().time() + 30.0
                while True:
                    current = await self._observe(reply, record)
                    self._capture(record, current)
                    if current["available"].get(component) or result.returncode != 0:
                        break
                    selected = [item for item in current["containers"].values()
                                if item["component"] == component]
                    if (not selected or not selected[0]["running"] or
                            selected[0]["health"] == "unhealthy" or
                            asyncio.get_running_loop().time() >= deadline):
                        success = False
                        break
                    await asyncio.sleep(0.1)
        except asyncio.CancelledError:
            await _joined(asyncio.create_task(self._finish(reply, record, success=False)))
            raise
        except (ComposeError, OSError, TimeoutError):
            success = False
        return await self._finish(reply, record, success=success)

    async def _destructive(self, plan_sha256: str, observation_sha256: str,
                           delete_state: set[str] | None) -> dict[str, Any]:
        with self._lock():
            reply, record = await self._load(plan_sha256)
            observation = await self._observe(reply, record)
            if observation["observation_sha256"] != observation_sha256:
                _fail("stale_observation")
            if not record["attempted"]:
                _fail("deployment_not_started")
            if delete_state is not None and not delete_state <= set(reply["plan"]["volumes"]):
                _fail("invalid_state_disposition")
            if any(c["kind"] == "request" and c["running"] for c in observation["containers"].values()):
                _fail("request_in_progress")
            if delete_state is not None and any(c["running"] for c in observation["containers"].values()):
                _fail("resources_not_stopped")
            record["pending"] = True
            self._save(record)
            success = False
            try:
                for component in reply["plan"]["shutdown_order"]:
                    for identity, item in observation["containers"].items():
                        if item["component"] != component:
                            continue
                        # Re-observe exact identities immediately before each effect.
                        current = await self._observe(reply, record)
                        if identity not in current["containers"]:
                            _fail("resource_replaced")
                        args = (["container", "rm", identity] if delete_state is not None else
                                ["container", "stop", "--time",
                                 str(reply["plan"]["components"][component]["stop_timeout_ms"] // 1000), identity])
                        result = await self._docker(reply["plan"], args,
                            timeout=(30 if delete_state is not None else
                                     reply["plan"]["components"][component]["stop_timeout_ms"] / 1000))
                        if result.returncode != 0:
                            _fail("effect_unconfirmed")
                if delete_state is not None:
                    current = await self._observe(reply, record)
                    if current["containers"]:
                        _fail("resources_incomplete")
                    if current["network"]:
                        result = await self._docker(reply["plan"], ["network", "rm", current["network"]["id"]])
                        if result.returncode != 0:
                            _fail("effect_unconfirmed")
                    for namespace in sorted(delete_state):
                        if namespace in current["volumes"]:
                            # The previous observation includes all current users of each volume.
                            result = await self._docker(reply["plan"], ["volume", "rm", current["volumes"][namespace]["name"]])
                            if result.returncode != 0:
                                _fail("effect_unconfirmed")
                success = True
            except asyncio.CancelledError:
                await _joined(asyncio.create_task(self._finish(reply, record, success=False)))
                raise
            except (ComposeError, OSError, TimeoutError):
                pass
            return await self._finish(reply, record, success=success)

    async def stop(self, plan_sha256: str, observation_sha256: str) -> dict[str, Any]:
        return await self._destructive(plan_sha256, observation_sha256, None)

    async def remove(self, plan_sha256: str, observation_sha256: str, *,
                     delete_state: set[str]) -> dict[str, Any]:
        return await self._destructive(plan_sha256, observation_sha256, delete_state)

    async def _proposal(self, transition: str) -> tuple[dict[str, Any], bytes]:
        if not isinstance(transition, str) or not HEX.fullmatch(transition):
            _fail("invalid_successor")
        directory = ".successors/" + transition
        try:
            raw = _read(_safe_child(self.workspace, directory + "/request.json"), MAX_INPUT)
        except OSError:
            _fail("successor_unavailable")
        reply = await self._plan_successor(raw)
        saved = _decode(_read(_safe_child(self.workspace, directory + "/prepared.json"), MAX_PLAN))
        if reply != saved or reply["transition_sha256"] != transition:
            _fail("plan_changed")
        self._verify_files(reply["successor"], self.workspace / directory / "next")
        return reply, raw

    async def prepare_successor(self, request: bytes, predecessor_plan_sha256: str) -> dict[str, Any]:
        """Retain an inert exact successor; the host must separately admit adoption."""
        with self._lock():
            previous, _ = await self._load(predecessor_plan_sha256)
            if self._custody is None:
                prior = origin = _read(self.workspace / "request.json", MAX_INPUT)
            else:
                if len(self._custody["selections"]) >= 32:
                    _fail("successor_limit")
                _, prior, origin = await self._selected(self._custody["active"])
            if self._custody is not None and self._custody["schema_version"] == 3:
                envelope = _json({"schema_version": 2,
                                  "requests": [*await self._retained_requests(), request.decode()]})
            else:
                envelope = _json({"schema_version": 1, "origin_request": origin.decode(),
                    "predecessor_request": prior.decode(), "successor_request": request.decode()})
            reply = await self._plan_successor(envelope)
            if reply["predecessor"] != previous or reply["successor"]["plan_sha256"] == predecessor_plan_sha256:
                _fail("plan_changed")
            parent = self.workspace / ".successors"
            parent.mkdir(mode=0o700, exist_ok=True)
            if parent.is_symlink():
                _fail("invalid_retained_path")
            directory = parent / reply["transition_sha256"]
            if directory.exists():
                verified, _ = await self._proposal(reply["transition_sha256"])
                if verified != reply:
                    _fail("plan_changed")
                return reply
            directory.mkdir(mode=0o700)
            target = directory / "next"
            target.mkdir(mode=0o700)
            for relative, item in reply["successor"]["plan"]["files"].items():
                path = Path(relative)
                if (path.is_absolute() or any(p in (".", "..") for p in path.parts) or
                        "\\" in relative or (relative != "compose.json" and not relative.startswith("inputs/"))):
                    _fail("invalid_planner_files")
                output = target / path
                output.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
                _write(output, item["contents"].encode(), new=True)
                os.chmod(output, item["mode"])
            _write(directory / "request.json", envelope, new=True)
            _write(directory / "prepared.json", _json(reply), new=True)
            self._verify_files(reply["successor"], target)
            return reply

    async def adopt_successor(self, transition_sha256: str, predecessor_plan_sha256: str,
                              observation_sha256: str) -> dict[str, Any]:
        """One explicit host admission; state compatibility does not grant B authority."""
        with self._lock():
            previous, record = await self._load(predecessor_plan_sha256)
            proposal, raw = await self._proposal(transition_sha256)
            if proposal["predecessor"] != previous:
                _fail("plan_changed")
            envelope = _decode(raw)
            retained = await self._retained_requests()
            if envelope["schema_version"] == 2:
                if envelope["requests"][:-1] != retained:
                    _fail("custody_changed")
            elif (envelope["origin_request"] != retained[0] or
                  envelope["predecessor_request"] != retained[-1] or
                  self._custody is not None and self._custody["schema_version"] == 3):
                _fail("custody_changed")
            migration = self._migration_spec(proposal)
            observation = await self._observe(previous, record)
            if observation["observation_sha256"] != observation_sha256:
                _fail("stale_observation")
            if not record["attempted"] or record["pending"]:
                _fail("operation_unconfirmed")
            if (set(record["containers"]) != set(observation["containers"]) or
                    record["network"] != observation["network"] or
                    set(observation["volumes"]) != set(previous["plan"]["volumes"]) or
                    record["volumes"] != observation["volumes"] or
                    any(c["kind"] == "request" and c["running"] for c in observation["containers"].values())):
                _fail("successor_resources_unconfirmed")
            for component in previous["plan"]["deployment_order"]:
                if sum(c["component"] == component for c in observation["containers"].values()) != 1:
                    _fail("successor_resources_unconfirmed")
            if migration is not None:
                for role in ("apply", "readback"):
                    worker_plan = self._migration_worker_plan(proposal["successor"], {"id": transition_sha256}, role)
                    await self._engine(worker_plan)
                    await self._image_facts(worker_plan, migration["component"])
            initial = self._custody is None
            if initial:
                origin = self.workspace / ".successors" / "origin"
                origin.mkdir(mode=0o700, exist_ok=True)
                if origin.is_symlink():
                    _fail("invalid_retained_path")
                for name, raw in (("request.json", _read(self.workspace / "request.json", MAX_INPUT)),
                                  ("prepared.json", _read(self.workspace / "prepared.json", MAX_PLAN))):
                    if (origin / name).exists():
                        if _read(origin / name, MAX_PLAN) != raw:
                            _fail("plan_changed")
                    else:
                        _write(origin / name, raw, new=True)
                self._custody = {"schema_version": 2, "workspace": str(self.workspace.resolve()),
                    "active": predecessor_plan_sha256, "selections": {predecessor_plan_sha256: {"origin": True}},
                    "records": {predecessor_plan_sha256: record}, "history": [], "transition": None}
            assert self._custody is not None
            successor = proposal["successor"]["plan_sha256"]
            if successor in self._custody["selections"]:
                _fail("successor_already_retained")
            self._custody["selections"][successor] = {"transition": transition_sha256}
            self._custody["transition"] = {"id": transition_sha256, "from": predecessor_plan_sha256,
                "to": successor, "phase": "retire", "admitted_observation": observation,
                "effects": [], "attempted_components": {}, "result": "pending"}
            if migration is not None:
                self._custody["schema_version"] = 3
                transition = self._custody["transition"]
                transition["migration"] = {"attempt_id": _sha(uuid.uuid4().bytes),
                    "apply": None, "readbacks": [], "receipt": None}
                transition["migration"]["binding"] = self._migration_binding(transition, migration)
            if initial:
                # Before this marker A remains current and no Docker effect is allowed.
                # A prior interrupted, unadmitted journal is evidence, never authority.
                if (self.workspace / "custody.json").exists():
                    _write(self.workspace / ".successors" / ("unadmitted-" + uuid.uuid4().hex + ".json"),
                           _read(self.workspace / "custody.json", MAX_PLAN), new=True)
                _write(self.workspace / "custody.json", _json(self._custody))
                _write(self.workspace / "request.json", SUCCESSOR_MARKER)
            else:
                self._save_custody()
            return await self._advance_successor(previous, record)

    @staticmethod
    def _migration_spec(proposal: dict[str, Any]) -> dict[str, Any] | None:
        before, after = (proposal[key]["plan"] for key in ("predecessor", "successor"))
        changed = [key for key in before["volumes"] if
                   before["volumes"][key]["format"] != after["volumes"][key]["format"]]
        if not changed:
            return None
        spec = after.get("state_migration")
        if (proposal["schema_version"] != 2 or len(changed) != 1 or not isinstance(spec, dict) or
                spec["namespace"] != changed[0] or spec["protocol_version"] != 1 or
                spec["from_format"] != before["volumes"][changed[0]]["format"] or
                spec["to_format"] != after["volumes"][changed[0]]["format"]):
            _fail("migration_contract_changed")
        return spec

    @staticmethod
    def _migration_binding(transition: dict[str, Any], spec: dict[str, Any]) -> dict[str, Any]:
        volume = transition["admitted_observation"]["volumes"][spec["namespace"]]
        return {"protocol_version": 1, "transition_sha256": transition["id"],
            "predecessor_plan_sha256": transition["from"], "successor_plan_sha256": transition["to"],
            **{key: spec[key] for key in ("namespace", "component", "owner", "from_format", "to_format",
                                         "operation", "artifact_digest")},
            "volume_name": volume["name"], "physical_volume_observation_sha256": _sha(_json(volume)),
            "attempt_id": transition["migration"]["attempt_id"]}

    def _validate_migration_record(self, transition: dict[str, Any], proposal: dict[str, Any], *, completed: bool) -> None:
        spec = self._migration_spec(proposal)
        migration = transition.get("migration")
        if spec is None:
            if migration is not None or transition["phase"] == "migrate":
                _fail("custody_changed")
            return
        if (self._custody["schema_version"] != 3 or not isinstance(migration, dict) or
                not isinstance(migration.get("attempt_id"), str) or not HEX.fullmatch(migration["attempt_id"]) or
                _json(migration.get("binding")) != _json(self._migration_binding(transition, spec))):
            _fail("migration_record_changed")
        if not isinstance(migration.get("readbacks"), list) or len(migration["readbacks"]) > 32:
            _fail("migration_record_changed")
        refs = [migration.get("apply"), *migration["readbacks"]]
        names = set()
        for index, ref in enumerate(refs):
            if ref is None:
                continue
            if (not isinstance(ref, dict) or ref.get("role") not in ("apply", "readback") or
                    ref.get("name") in names or not isinstance(ref.get("name"), str) or
                    any(type(ref.get(key)) is not bool for key in ("create_attempted", "start_attempted", "quiescent", "removed")) or
                    ref.get("id") is not None and (not isinstance(ref["id"], str) or not HEX.fullmatch(ref["id"]))):
                _fail("migration_record_changed")
            role = "apply" if index == 0 else "readback"
            expected_name = proposal["successor"]["plan"]["project"] + "-migration-" + transition["id"][:20] + "-" + role + "-" + str(max(0, index - 1))
            if (ref["role"] != role or ref["name"] != expected_name or
                    ref.get("remove_attempted", False) and not ref["quiescent"] or
                    ref["start_attempted"] and not ref["create_attempted"] or
                    ref["quiescent"] and (not ref["start_attempted"] or ref["id"] is None) or
                    ref["removed"] and not ref.get("remove_attempted")):
                _fail("migration_record_changed")
            names.add(ref["name"])
        receipt = migration.get("receipt")
        if receipt is not None:
            if (self._migration_result(receipt, migration["binding"]) != "committed" or
                    migration.get("receipt_sha256") != _sha(_json(receipt))):
                _fail("migration_record_changed")
        if completed and (receipt is None or migration["apply"] is None or
                not migration["apply"]["start_attempted"] or
                any(ref is not None and (not ref["quiescent"] or not ref["removed"]) for ref in refs)):
            _fail("migration_record_changed")

    @staticmethod
    def _migration_result(value: Any, binding: dict[str, Any]) -> str:
        if (not isinstance(value, dict) or set(value) != {"protocol_version", "status", "observed_format", "receipt"} or
                type(value["protocol_version"]) is not int or value["protocol_version"] != 1):
            _fail("migration_readback_invalid")
        if value["status"] == "ready" and value["observed_format"] == binding["from_format"] and value["receipt"] is None:
            return "ready"
        receipt = value["receipt"]
        if (value["status"] == "committed" and value["observed_format"] == binding["to_format"] and
                isinstance(receipt, dict) and set(receipt) == {"binding", "evidence_sha256"} and
                _json(receipt["binding"]) == _json(binding) and isinstance(receipt["evidence_sha256"], str) and
                HEX.fullmatch(receipt["evidence_sha256"])):
            return "committed"
        _fail("migration_unconfirmed")

    def _migration_worker_plan(self, target: dict[str, Any], transition: dict[str, Any],
                               role: str) -> dict[str, Any]:
        """Derive one isolated owner process from the already admitted B image."""
        plan = _decode(_json(target["plan"]))
        spec = plan["state_migration"]
        component = spec["component"]
        prepared = plan["components"][component]
        document = _decode(plan["compose_json"].encode())
        service = document["services"][prepared["service"]]
        volume = plan["volumes"][spec["namespace"]]["name"]
        state_mounts = [mount for mount in service["volumes"]
                        if mount["type"] == "volume" and mount["source"] == volume]
        if len(state_mounts) != 1:
            _fail("migration_contract_changed")
        mounts = [mount for mount in service["volumes"] if mount["type"] == "bind" and mount["read_only"]]
        mounts.append({**state_mounts[0], "read_only": role == "readback"})
        service.update(entrypoint=[spec[role]["executable"]], command=spec[role]["arguments"],
            volumes=mounts, healthcheck={"disable": True}, network_mode="none", read_only=role == "readback")
        service["labels"].update({"org.meerkat.migration": transition["id"], "org.meerkat.migration-role": role})
        service.pop("networks", None)
        document["services"] = {prepared["service"]: service}
        prepared.update(lifetime="deployment", dependencies=[], deployment_dependencies=[])
        plan["components"] = {component: prepared}
        plan["compose_json"] = _json(document).decode()
        return plan

    async def _migration_workers(self) -> dict[str, Any]:
        if self._custody is None or self._custody["transition"] is None:
            return {}
        transition = self._custody["transition"]
        migration = transition.get("migration")
        if migration is None:
            return {}
        target, _, _ = await self._selected(transition["to"])
        workers = {}
        for ref in [migration["apply"], *migration["readbacks"]]:
            if ref is None:
                continue
            plan = self._migration_worker_plan(target, transition, ref["role"])
            ids = await self._ids(plan, ["ps", "--all", "--quiet", "--no-trunc", "--filter",
                                       "name=^/" + re.escape(ref["name"]) + "$"])
            if not ids:
                continue
            if not ref["create_attempted"]:
                _fail("migration_worker_unowned")
            if len(ids) != 1 or ref["removed"] or ref["id"] is not None and ids != [ref["id"]]:
                _fail("migration_worker_changed")
            component = plan["state_migration"]["component"]
            environment, descriptor = await self._image_facts(plan, component)
            raw = await self._object(plan, ["container", "inspect", ids[0]])
            if len(raw) != 1 or raw[0]["Name"] != "/" + ref["name"]:
                _fail("migration_worker_changed")
            observed = self._container(plan, raw[0], {}, {component: environment},
                                       {component: descriptor}, isolated=True)
            if (observed["status"] in ("exited", "dead") and type(observed["exit_code"]) is not int or
                    observed["id"] != ids[0] or observed["id"] in workers):
                _fail("migration_worker_changed")
            workers[ids[0]] = {key: observed[key] for key in ("id", "name", "running", "status", "exit_code")}
            workers[ids[0]].update(role=ref["role"], volume=migration["binding"]["volume_name"])
        return workers

    async def _migration_process(self, target: dict[str, Any], transition: dict[str, Any],
                                 ref: dict[str, Any]) -> dict[str, Any]:
        plan = self._migration_worker_plan(target, transition, ref["role"])
        spec, binding = plan["state_migration"], transition["migration"]["binding"]
        workers = await self._migration_workers()
        matching = [worker for worker in workers.values() if worker["name"] == ref["name"]]
        if not matching:
            if ref["create_attempted"]:
                _fail("migration_worker_unresolved")
            predecessor, _, _ = await self._selected(transition["from"])
            await self._observe_transition(predecessor, self._custody["records"][transition["from"]])
            await self._engine(plan)
            await self._image_facts(plan, spec["component"])
            service = _decode(plan["compose_json"].encode())["services"][plan["components"][spec["component"]]["service"]]
            args = ["container", "create", "--name", ref["name"], "--interactive", "--pull", "never",
                    "--network", "none", "--init", "--restart", "no", "--cap-drop", "ALL",
                    "--security-opt", "no-new-privileges", "--log-driver", "json-file",
                    "--platform", plan["components"][spec["component"]]["platform"],
                    "--user", service["user"], "--workdir", service["working_dir"],
                    "--memory", str(service["mem_limit"]), "--memory-swap", str(service["memswap_limit"]),
                    "--pids-limit", str(service["pids_limit"]), "--cpus", str(service["cpus"]),
                    "--no-healthcheck", "--entrypoint", service["entrypoint"][0]]
            if service["read_only"]:
                args.append("--read-only")
            labels = {**service["labels"], "com.docker.compose.project": plan["project"],
                      "com.docker.compose.service": plan["components"][spec["component"]]["service"]}
            for key, value in sorted(labels.items()):
                args += ["--label", key + "=" + value]
            for key, value in sorted(service["environment"].items()):
                args += ["--env", key + "=" + value]
            for mount in service["volumes"]:
                source = mount["source"]
                if mount["type"] == "bind":
                    source = str((self._input_root(plan) / source).resolve())
                fields = ["type=" + mount["type"], "source=" + source, "target=" + mount["target"]]
                if mount["read_only"]:
                    fields.append("readonly")
                if mount["type"] == "volume":
                    fields.append("volume-nocopy")
                args += ["--mount", ",".join('"' + field.replace('"', '""') + '"' for field in fields)]
            args += [service["image"], *service["command"]]
            ref["create_attempted"] = True
            self._save_custody()
            result = await self._docker(plan, args)
            if result.returncode != 0:
                _fail("migration_worker_unresolved")
            matching = [worker for worker in (await self._migration_workers()).values() if worker["name"] == ref["name"]]
        if len(matching) != 1:
            _fail("migration_worker_unresolved")
        worker = matching[0]
        ref["id"] = worker["id"]
        self._save_custody()
        if not ref["start_attempted"]:
            if worker["status"] != "created" or worker["running"]:
                _fail("migration_worker_unresolved")
            ref["start_attempted"] = True
            self._save_custody()
            # This persisted entry is never repeated, even after a lost result.
            await self._docker(plan, ["container", "start", "--attach", "--interactive", ref["id"]],
                               timeout=spec["timeout_ms"] / 1000, stdin=_json(binding) + b"\n")
            worker = (await self._migration_workers()).get(ref["id"])
        if worker is None or worker["running"] or worker["status"] not in ("exited", "dead"):
            _fail("migration_worker_unresolved")
        ref["quiescent"] = True
        self._save_custody()
        return worker

    async def _remove_migration_worker(self, target: dict[str, Any], ref: dict[str, Any]) -> None:
        workers = await self._migration_workers()
        worker = workers.get(ref["id"])
        if worker is None:
            if not ref.get("remove_attempted"):
                _fail("migration_worker_unresolved")
        else:
            if worker["running"] or worker["status"] not in ("exited", "dead"):
                _fail("migration_worker_unresolved")
            ref["remove_attempted"] = True
            self._save_custody()
            result = await self._docker(target["plan"], ["container", "rm", ref["id"]])
            if result.returncode != 0 or ref["id"] in await self._migration_workers():
                _fail("migration_worker_unresolved")
        ref["removed"] = True
        self._save_custody()

    async def _migrate_successor(self, reply: dict[str, Any], record: dict[str, Any]) -> None:
        transition = self._custody["transition"]
        migration = transition["migration"]
        target, _, _ = await self._selected(transition["to"])
        await self._observe_transition(reply, record)
        def new_worker(role: str, index: int) -> dict[str, Any]:
            return {"role": role, "name": target["plan"]["project"] + "-migration-" +
                    transition["id"][:20] + "-" + role + "-" + str(index), "id": None,
                    "create_attempted": False, "start_attempted": False, "quiescent": False, "removed": False}
        apply = migration["apply"]
        if apply is not None and not apply["removed"]:
            # A prior apply must be concretely stopped before a reader or B writer.
            if not apply.get("remove_attempted"):
                await self._migration_process(target, transition, apply)
            await self._remove_migration_worker(target, apply)
        for _ in range(2):
            readers = migration["readbacks"]
            if not readers or readers[-1]["removed"]:
                if len(readers) >= 32:
                    _fail("migration_readback_limit")
                readers.append(new_worker("readback", len(readers)))
                self._save_custody()
            reader = readers[-1]
            if reader.get("remove_attempted"):
                await self._remove_migration_worker(target, reader)
                continue
            worker = await self._migration_process(target, transition, reader)
            result = await self._docker(target["plan"], ["container", "logs", reader["id"]])
            # Remove the exact reader even when its answer cannot resolve the attempt.
            await self._remove_migration_worker(target, reader)
            value = _decode(result.stdout) if result.returncode == 0 and len(result.stdout) <= 65536 else None
            if worker["exit_code"] != 0:
                _fail("migration_unconfirmed")
            status = self._migration_result(value, migration["binding"])
            if status == "committed":
                if migration["apply"] is None or not migration["apply"]["start_attempted"]:
                    _fail("migration_receipt_unadmitted")
                migration["receipt"] = value
                migration["receipt_sha256"] = _sha(_json(value))
                self._save_custody()
                return
            if migration["apply"] is not None:
                _fail("migration_unconfirmed")
            migration["apply"] = new_worker("apply", 0)
            self._save_custody()
            await self._migration_process(target, transition, migration["apply"])
            await self._remove_migration_worker(target, migration["apply"])
        _fail("migration_unconfirmed")

    async def _observe_transition(self, reply: dict[str, Any], record: dict[str, Any]) -> dict[str, Any]:
        assert self._custody is not None
        transition = self._custody["transition"]
        observation = await self._observe(reply, record)
        origin_volumes = transition["admitted_observation"]["volumes"]
        if observation["volumes"] != origin_volumes:
            _fail("successor_state_changed")
        if transition["phase"] == "retire":
            initial = transition["admitted_observation"]
            removals = {e["id"] for e in transition["effects"] if e["kind"] == "remove_container"}
            if (set(initial["containers"]) - set(observation["containers"])) - removals:
                _fail("resources_disappeared")
            if initial["network"] and not observation["network"] and not any(
                    e["kind"] == "remove_network" for e in transition["effects"]):
                _fail("resources_disappeared")
        elif transition["phase"] == "migrate":
            if observation["containers"] or observation["network"]:
                _fail("resources_incomplete")
        elif any(c["component"] not in transition["attempted_components"]
                 for c in observation["containers"].values()):
            _fail("resource_mismatch")
        value = {"status": "successor_pending", "transition_sha256": transition["id"],
                 "phase": transition["phase"], "observation": observation,
                 "custody_sha256": _sha(_json(self._custody))}
        value["observation_sha256"] = _sha(_json(value))
        return value

    async def reconcile_successor(self, transition_sha256: str, observation_sha256: str) -> dict[str, Any]:
        """Explicit current-resource reconciliation; absence cannot resolve an unknown start."""
        with self._lock():
            reply, record = await self._load(allow_transition=True)
            if self._custody is None or self._custody["transition"] is None:
                _fail("no_pending_successor")
            if self._custody["transition"]["id"] != transition_sha256:
                _fail("plan_changed")
            observed = await self._observe_transition(reply, record)
            if observed["observation_sha256"] != observation_sha256:
                _fail("stale_observation")
            return await self._advance_successor(reply, record)

    async def _advance_successor(self, reply: dict[str, Any], record: dict[str, Any]) -> dict[str, Any]:
        assert self._custody is not None
        transition = self._custody["transition"]
        try:
            if transition["phase"] == "retire":
                for component in reply["plan"]["shutdown_order"]:
                    original = transition["admitted_observation"]["containers"]
                    for identity, item in original.items():
                        if item["component"] != component:
                            continue
                        current = (await self._observe_transition(reply, record))["observation"]
                        if identity not in current["containers"]:
                            continue  # Only a retained exact removal intent permits this absence.
                        if current["containers"][identity]["running"]:
                            transition["effects"].append({"kind": "stop", "id": identity, "outcome": "unconfirmed"})
                            self._save_custody()
                            seconds = reply["plan"]["components"][component]["stop_timeout_ms"] // 1000
                            result = await self._docker(reply["plan"], ["container", "stop", "--time", str(seconds), identity], timeout=seconds)
                            current = (await self._observe_transition(reply, record))["observation"]
                            if result.returncode != 0 or current["containers"].get(identity, {}).get("running", True):
                                _fail("effect_unconfirmed")
                            transition["effects"][-1]["outcome"] = "observed_stopped"
                            self._save_custody()
                        transition["effects"].append({"kind": "remove_container", "id": identity, "outcome": "unconfirmed"})
                        self._save_custody()
                        result = await self._docker(reply["plan"], ["container", "rm", identity])
                        current = (await self._observe_transition(reply, record))["observation"]
                        if result.returncode != 0 or identity in current["containers"]:
                            _fail("effect_unconfirmed")
                        transition["effects"][-1]["outcome"] = "observed_absent"
                        self._save_custody()
                current = (await self._observe_transition(reply, record))["observation"]
                if current["containers"]:
                    _fail("resources_incomplete")
                if current["network"]:
                    identity = current["network"]["id"]
                    transition["effects"].append({"kind": "remove_network", "id": identity, "outcome": "unconfirmed"})
                    self._save_custody()
                    result = await self._docker(reply["plan"], ["network", "rm", identity])
                    current = (await self._observe_transition(reply, record))["observation"]
                    if result.returncode != 0 or current["network"]:
                        _fail("effect_unconfirmed")
                    transition["effects"][-1]["outcome"] = "observed_absent"
                    self._save_custody()
                if "migration" in transition:
                    transition["phase"] = "migrate"
                    self._save_custody()
            if transition["phase"] == "migrate":
                await self._migrate_successor(reply, record)
            if transition["phase"] in ("retire", "migrate"):
                # Selection changes only after retirement and committed owner readback.
                reply, _, _ = await self._selected(transition["to"])
                record = {"schema_version": 1, "plan_sha256": reply["plan_sha256"],
                    "attempted": True, "pending": True, "containers": {}, "network": None,
                    "volumes": transition["admitted_observation"]["volumes"], "requests": {}}
                self._custody["active"] = transition["to"]
                self._custody["records"][transition["to"]] = record
                transition["phase"] = "activate"
                self._save_custody()
            current = (await self._observe_transition(reply, record))["observation"]
            # A lost create/start with no running matching resource is not permission to replay.
            for component in transition["attempted_components"]:
                matches = [c for c in current["containers"].values() if c["component"] == component]
                if len(matches) != 1 or not matches[0]["running"]:
                    _fail("successor_effect_unresolved")
            self._capture(record, current)
            result = await self._start_effect(reply, record, current)
            if result["status"] == "observed" and all(result["observation"]["available"].values()):
                transition["result"] = "observed"
                self._custody["history"].append(transition)
                self._custody["transition"] = None
                self._save_custody()
                return {**result, "transition_sha256": transition["id"]}
        except asyncio.CancelledError:
            # Intent was saved before entry. Cancellation never starts another operation.
            transition["result"] = "cancelled_unconfirmed"
            self._save_custody()
            raise
        except (ComposeError, OSError, TimeoutError):
            transition["result"] = "unconfirmed"
            self._save_custody()
        return {"status": "unconfirmed", "transition_sha256": transition["id"],
                "plan_sha256": self._custody["active"], "observation": None}

    async def _close_request(self, reply: dict[str, Any], record: dict[str, Any],
                             component: str, name: str, code: int | None,
                             admitted_network: dict[str, Any] | None) -> dict[str, Any]:
        """Join only this request; a fresh graph observation is a separate action."""
        plan = reply["plan"]
        result = {"status": "unconfirmed", "plan_sha256": reply["plan_sha256"],
                  "observation": None, "request": {"name": name, "container_id": None,
                                                    "returncode": code, "stopped": False}}
        try:
            await self._engine(plan)
            environment, descriptor = await self._image_facts(plan, component)

            async def inspect(target: str, expected_id: str | None = None) -> dict[str, Any]:
                rows = await self._object(plan, ["container", "inspect", target])
                if not isinstance(rows, list) or len(rows) != 1:
                    _fail("engine_observation_invalid")
                item = self._container(plan, rows[0], record["requests"],
                                       {component: environment}, {component: descriptor})
                if (item["name"] != name or item["component"] != component or
                        item["kind"] != "request" or
                        (expected_id is not None and item["id"] != expected_id)):
                    _fail("resource_mismatch")
                for identity, previous in record["containers"].items():
                    if (previous["name"] == name and identity != item["id"]) or (
                            identity == item["id"] and previous["name"] != name):
                        _fail("resource_replaced")
                if admitted_network is not None and item["network_id"] != admitted_network["id"]:
                    _fail("resource_replaced")
                return item

            item = await inspect(name)
            if admitted_network is None:
                # A request-only graph can create its first network during run.
                # Inspect that exact network before recording ownership of it.
                compose = _decode(plan["compose_json"].encode())
                expected = compose["networks"]["default"]
                networks = await self._object(plan, ["network", "inspect", item["network_id"]])
                if not isinstance(networks, list) or len(networks) != 1:
                    _fail("engine_observation_invalid")
                raw = networks[0]
                members = raw.get("Containers")
                if members is None:
                    members = {}
                if (raw["Id"] != item["network_id"] or not HEX.fullmatch(raw["Id"]) or
                        raw["Name"] != expected["name"] or raw["Internal"] is not True or
                        raw["Driver"] != "bridge" or
                        not isinstance(members, dict) or
                        raw.get("Labels", {}).get("com.docker.compose.project") != plan["project"] or
                        any(raw.get("Labels", {}).get(k) != v for k, v in expected.get("labels", {}).items()) or
                        any(identity not in record["containers"] and identity != item["id"]
                            for identity in members)):
                    _fail("resource_mismatch")
                admitted_network = {"id": raw["Id"], "name": raw["Name"]}
                if record["network"] and record["network"] != admitted_network:
                    _fail("resource_replaced")
                record["network"] = admitted_network
            result["request"]["container_id"] = item["id"]
            record["containers"][item["id"]] = item
            # Save the exact identity while pending, before a stop may be sent.
            self._save(record)
            if item["running"]:
                seconds = plan["components"][component]["stop_timeout_ms"] // 1000
                stopped = await self._docker(plan, ["container", "stop", "--time", str(seconds), item["id"]], timeout=seconds)
                if stopped.returncode != 0:
                    _fail("effect_unconfirmed")
                item = await inspect(item["id"], item["id"])
            if item["running"]:
                _fail("effect_unconfirmed")
            record["containers"][item["id"]] = item
            record["pending"] = False
            self._save(record)
            result["request"]["stopped"] = True
            if code == 0:
                result["status"] = "observed"
        except (ComposeError, KeyError, TypeError, ValueError, IndexError, AttributeError, OSError, TimeoutError):
            # A replace followed by failed directory fsync is uncertain too.
            # Restore pending if possible; never report failed persistence as success.
            record["pending"] = True
            try:
                self._save(record)
            except OSError:
                pass
        return result

    async def run_stdio(self, component: str, plan_sha256: str, *,
                        stdin: AsyncIterable[bytes], stdout: Callable[[bytes], Awaitable[None]],
                        stderr: Callable[[bytes], Awaitable[None]], timeout: float = 300.0) -> dict[str, Any]:
        if not isinstance(timeout, (int, float)) or not 0 < timeout <= 3600:
            _fail("invalid_request_timeout")
        with self._lock():
            reply, record = await self._load(plan_sha256)
            plan = reply["plan"]
            if component not in plan["request_components"]:
                _fail("not_request_component")
            observation = await self._observe(reply, record)
            if record["pending"]:
                _fail("operation_unconfirmed")
            if any(not observation["available"].get(dep, False)
                   for dep in plan["components"][component]["deployment_dependencies"]):
                _fail("dependency_unavailable")
            if not record["attempted"]:
                _fail("deployment_not_started")
            if len(record["requests"]) >= 128:
                _fail("request_attempt_limit")
            name = plan["project"] + "-request-" + uuid.uuid4().hex
            record["requests"][name] = {"component": component}
            record["pending"] = True
            self._save(record)
            code = None
            cancelled = False
            try:
                code = await self.runner.stream(self._argv(plan, ["run", "--no-deps", "-T",
                    "--pull", "never", "--name", name, plan["components"][component]["service"]], compose=True),
                    cwd=self.workspace, env=self._env(), stdin=stdin, stdout=stdout, stderr=stderr,
                    timeout=timeout)
            except asyncio.CancelledError:
                cancelled = True
            except Exception:
                # A caller stream failure must not skip request-container readback.
                pass
            # Request cleanup does not re-admit or report freshness for the graph.
            result = await _joined(asyncio.create_task(self._close_request(
                reply, record, component, name, code, observation["network"])))
            if cancelled:
                raise asyncio.CancelledError
            return result


async def _main(args: argparse.Namespace) -> dict[str, Any] | None:
    runtime = ComposeRuntime(args.workspace, args.planner, args.planner_sha256)
    if args.action == "propose":
        _pin(args.host_profile, args.host_profile_sha256)
        host = _read(Path(args.host_profile), MAX_INPUT)
        if _sha(host) != args.host_profile_sha256:
            _fail("tool_binding_changed")
        return await runtime.propose(host, sys.stdin.buffer.read(MAX_INPUT + 1), acquire_images=args.acquire_images)
    if args.action == "prepare":
        request = sys.stdin.buffer.read(MAX_INPUT + 1)
        return await runtime.prepare(request)
    if args.action == "prepare-successor":
        return await runtime.prepare_successor(sys.stdin.buffer.read(MAX_INPUT + 1), args.predecessor_plan_sha256)
    if args.action == "adopt-successor":
        return await runtime.adopt_successor(args.transition_sha256, args.predecessor_plan_sha256,
                                             args.observation_sha256)
    if args.action == "reconcile-successor":
        return await runtime.reconcile_successor(args.transition_sha256, args.observation_sha256)
    if args.action == "observe":
        return await runtime.observe()
    if args.action == "selection":
        return await runtime.selection(args.transition_sha256)
    if args.action == "start":
        return await runtime.start(args.plan_sha256)
    if args.action == "resume":
        return await runtime.resume(args.plan_sha256, args.observation_sha256)
    if args.action == "stop":
        return await runtime.stop(args.plan_sha256, args.observation_sha256)
    if args.action == "remove":
        return await runtime.remove(args.plan_sha256, args.observation_sha256,
                                    delete_state=set(args.delete_state))
    loop = asyncio.get_running_loop()
    reader = asyncio.StreamReader()
    transport, _ = await loop.connect_read_pipe(lambda: asyncio.StreamReaderProtocol(reader), sys.stdin.buffer)
    async def source():
        while chunk := await reader.read(65536):
            yield chunk
    output_sink = _PipeSink(sys.stdout.fileno())
    diagnostic_sink = _PipeSink(sys.stderr.fileno())
    try:
        result = await runtime.run_stdio(args.component, args.plan_sha256,
            stdin=source(), stdout=output_sink.write, stderr=diagnostic_sink.write, timeout=args.timeout)
        # Keep lifecycle JSON off the MCP/protocol stdout.
        async with asyncio.timeout(1.0):
            await diagnostic_sink.write(_json(result) + b"\n")
        if (result.get("status") != "observed" or
                result.get("request", {}).get("stopped") is not True or
                result.get("request", {}).get("returncode") != 0):
            _fail("request_unconfirmed")
        return None
    finally:
        transport.close()
        output_sink.close()
        diagnostic_sink.close()


def _error(code: str) -> None:
    # A full diagnostic pipe must not keep an already-cleaned-up CLI alive.
    fd = sys.stderr.fileno()
    previous = os.get_blocking(fd)
    try:
        os.set_blocking(fd, False)
        os.write(fd, (code + "\n").encode("ascii"))
    except (BlockingIOError, BrokenPipeError):
        pass
    finally:
        os.set_blocking(fd, previous)


async def _cli(args: argparse.Namespace) -> dict[str, Any] | None:
    if args.action != "run-stdio" and not (args.action == "propose" and args.acquire_images):
        return await _main(args)
    loop = asyncio.get_running_loop()
    owner = asyncio.current_task()
    assert owner is not None
    previous = signal.getsignal(signal.SIGTERM)

    def terminate(signum, frame):
        # Cancel only this CLI operation. Its existing joined cleanup retains
        # the workspace lock through repeated TERM; SIGKILL cannot be handled.
        loop.call_soon_threadsafe(owner.cancel)

    signal.signal(signal.SIGTERM, terminate)
    try:
        return await _main(args)
    finally:
        signal.signal(signal.SIGTERM, previous)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--workspace", required=True)
    parser.add_argument("--planner", required=True)
    parser.add_argument("--planner-sha256", required=True)
    actions = parser.add_subparsers(dest="action", required=True)
    proposal = actions.add_parser("propose")
    proposal.add_argument("--host-profile", required=True)
    proposal.add_argument("--host-profile-sha256", required=True)
    proposal.add_argument("--acquire-images", action="store_true",
                          help="explicitly acquire immutable registry images under the host's image-pull scope")
    actions.add_parser("prepare")
    actions.add_parser("observe")
    selection = actions.add_parser("selection")
    selection.add_argument("--transition-sha256")
    successor = actions.add_parser("prepare-successor")
    successor.add_argument("--predecessor-plan-sha256", required=True)
    successor = actions.add_parser("adopt-successor")
    successor.add_argument("--predecessor-plan-sha256", required=True)
    successor.add_argument("--transition-sha256", required=True)
    successor.add_argument("--observation-sha256", required=True)
    successor = actions.add_parser("reconcile-successor")
    successor.add_argument("--transition-sha256", required=True)
    successor.add_argument("--observation-sha256", required=True)
    for action in ("start", "resume", "stop", "remove", "run-stdio"):
        sub = actions.add_parser(action)
        sub.add_argument("--plan-sha256", required=True)
        if action in ("resume", "stop", "remove"):
            sub.add_argument("--observation-sha256", required=True)
        if action == "remove":
            sub.add_argument("--delete-state", action="append", default=[])
        if action == "run-stdio":
            sub.add_argument("--component", required=True)
            sub.add_argument("--timeout", type=float, default=300.0)
    try:
        result = asyncio.run(_cli(parser.parse_args()))
        if result is not None:
            sys.stdout.buffer.write(_json(result) + b"\n")
        return 0
    except ComposeError as error:
        _error(str(error))
    except (OSError, KeyError, TypeError, ValueError, TimeoutError):
        _error("compose_unavailable")
    except (KeyboardInterrupt, asyncio.CancelledError):
        _error("compose_interrupted")
    return 1


if __name__ == "__main__":
    raise SystemExit(main())
