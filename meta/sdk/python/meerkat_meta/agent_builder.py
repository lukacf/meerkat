"""SessionAgentBuilder protocol - imperative mutation pattern matching the downstream app's."""
from __future__ import annotations

import asyncio
from dataclasses import dataclass
import inspect
import logging
import time
from typing import Any, Callable, Mapping, Protocol, runtime_checkable

from .jobs import (
    CredentialResolver,
    DetachedJobAuthority,
    DetachedJobContext,
    DetachedJobExecution,
    DetachedJobResult,
    DetachedJobRpc,
    _DetachedJobReporter,
)
from .models import SessionBuildOptions
from .live import LiveAssistantOutputAddress
from .types import ErrorEvent

_log = logging.getLogger("meerkat_meta")


@dataclass
class _RegisteredJobRunner:
    execution: DetachedJobExecution
    profile_name: str | None


@dataclass
class _ActiveJobAttempt:
    authority: DetachedJobAuthority
    runner_key: tuple[str, str]
    runner_handle: str
    cancellation: asyncio.Event
    task: asyncio.Task[None] | None
    adopted: bool = False
    superseded: bool = False


@dataclass
class _JobMutationLock:
    lock: asyncio.Lock
    users: int = 0


@runtime_checkable
class SessionAgentBuilder(Protocol):
    """Protocol for building agents during session creation.

    Uses the imperative mutation pattern: build_agent receives a mutable
    SessionBuildOptions and modifies it (sets profile_name, calls add_tools
    or register_tool).

    Example:
        class MyAgentBuilder(SessionAgentBuilder):
            async def build_agent(self, opts: SessionBuildOptions) -> None:
                opts.profile_name = "assistant"
                opts.register_tool("search", my_search_handler)
                opts.register_tool("calc", my_calc_handler)
    """

    async def build_agent(self, options: SessionBuildOptions) -> None:
        """Build an agent by mutating the given options.

        Args:
            options: Mutable SessionBuildOptions. Set profile_name,
                    additional_instructions, and call register_tool() or add_tools().
        """
        ...


class CallbackDispatcher:
    """Routes incoming JSON-RPC callback requests from the Rust runtime
    to the registered SessionAgentBuilder and tool handlers.

    Tool handlers are scoped by a build-level scope_id to prevent
    cross-session handler bleed in concurrent sessions.
    """

    def __init__(self) -> None:
        self._builder: SessionAgentBuilder | None = None
        self._error_callback: Callable | None = None
        # Keyed by (scope_id, tool_name) to isolate concurrent sessions
        self._tool_handlers: dict[tuple[str, str], Any] = {}
        # Track scope_ids so we can clean up handlers when a scope is released
        self._scope_tools: dict[str, list[str]] = {}
        # customize_build is re-invoked with a fresh scope on every restore; track
        # the latest scope per identity so we can release the previous one (the
        # gateway has no scope-release signal — newest-wins is the semantics).
        self._customizer_scope_by_identity: dict[str, str] = {}
        # Identity-first providers (REQ-45)
        self._roster_provider: Any | None = None
        self._topology_provider: Any | None = None
        self._agent_customizer: Any | None = None
        # Host-runnable schedule-fire handlers, keyed by runnable name
        # (callback/schedule_fire from runtime_options.host_runnables targets)
        self._schedule_fire_handlers: dict[str, Any] = {}
        # Detached jobs remain machine-owned in Meerkat. These maps hold only
        # the host process's mechanical runner/task projection.
        self._job_rpc: DetachedJobRpc | None = None
        self._job_credential_resolver: CredentialResolver | None = None
        self._job_runners: dict[tuple[str, str], _RegisteredJobRunner] = {}
        self._job_attempts: dict[str, _ActiveJobAttempt] = {}
        self._job_highest_fence: dict[str, int] = {}
        self._job_tasks: set[asyncio.Task[None]] = set()
        self._job_mutation_locks: dict[str, _JobMutationLock] = {}
        self._live_output_queues: dict[
            str, asyncio.Queue[LiveAssistantOutputAddress]
        ] = {}

    def register_builder(self, builder: SessionAgentBuilder) -> None:
        self._builder = builder

    def register_error_callback(self, callback: Callable) -> None:
        self._error_callback = callback

    def register_roster_provider(self, provider: Any) -> None:
        self._roster_provider = provider

    def register_topology_provider(self, provider: Any) -> None:
        self._topology_provider = provider

    def register_agent_customizer(self, provider: Any) -> None:
        self._agent_customizer = provider

    def register_schedule_fire_handler(self, name: str, handler: Any) -> None:
        """Register the handler for a named host runnable.

        The gateway invokes it via ``callback/schedule_fire`` when a schedule
        with a ``host_runnable`` target of that name fires. The handler
        receives the occurrence dict (``schedule_id``, ``occurrence_id``,
        ``due_at``, optional ``payload``); raising marks the occurrence
        attempt failed in the durable schedule store.
        """
        if not isinstance(name, str) or not name.strip():
            raise TypeError(f"runnable name must be a non-empty string, got {name!r}")
        if not callable(handler):
            raise TypeError(f"handler must be callable, got {type(handler).__name__}: {handler!r}")
        self._schedule_fire_handlers[name] = handler

    def register_job_rpc(self, rpc: DetachedJobRpc) -> None:
        """Bind the ordinary gateway RPC path used by asynchronous reporters."""
        if not callable(rpc):
            raise TypeError(f"job rpc must be callable, got {type(rpc).__name__}")
        self._job_rpc = rpc

    def register_job_credential_resolver(self, resolver: CredentialResolver) -> None:
        """Bind execution-time credential resolution for detached attempts."""
        if not callable(resolver):
            raise TypeError(
                f"job credential resolver must be callable, got {type(resolver).__name__}"
            )
        self._job_credential_resolver = resolver

    async def wait_for_job_tasks(self) -> None:
        """Wait for the current in-process host tasks (test/shutdown helper)."""
        while self._job_tasks:
            tasks = list(self._job_tasks)
            await asyncio.gather(*tasks, return_exceptions=True)

    def release_scope(self, scope_id: str) -> None:
        """Remove all tool handlers for a scope. Call when a session ends."""
        for tool_name in self._scope_tools.pop(scope_id, []):
            self._tool_handlers.pop((scope_id, tool_name), None)

    def register_live_output_queue(
        self, channel_id: str, capacity: int
    ) -> asyncio.Queue[LiveAssistantOutputAddress]:
        if not channel_id.strip():
            raise ValueError("channel_id must be non-empty")
        if capacity <= 0:
            raise ValueError("live output queue capacity must be positive")
        if channel_id in self._live_output_queues:
            raise RuntimeError(f"live output consumer already registered for {channel_id}")
        queue: asyncio.Queue[LiveAssistantOutputAddress] = asyncio.Queue(capacity)
        self._live_output_queues[channel_id] = queue
        return queue

    def unregister_live_output_queue(
        self,
        channel_id: str,
        queue: asyncio.Queue[LiveAssistantOutputAddress],
    ) -> None:
        if self._live_output_queues.get(channel_id) is queue:
            self._live_output_queues.pop(channel_id, None)

    async def handle_callback(self, method: str, params: dict[str, Any]) -> Any:
        if method == "meta/live/assistant_output_available":
            output = LiveAssistantOutputAddress.from_dict(params)
            queue = self._live_output_queues.get(output.channel_id)
            if queue is None:
                raise RuntimeError(
                    f"no live output consumer registered for {output.channel_id}"
                )
            try:
                queue.put_nowait(output)
            except asyncio.QueueFull as exc:
                raise RuntimeError(
                    f"live output consumer queue is full for {output.channel_id}"
                ) from exc
            return {"accepted": True}

        if method == "meta/on_error":
            if self._error_callback is not None:
                event = ErrorEvent.from_dict(params)
                try:
                    result = self._error_callback(event)
                    if inspect.isawaitable(result):
                        await result
                except Exception as exc:
                    _log.warning("error callback failed: %s", exc)
            return None

        if method == "callback/after_create":
            if self._builder is not None and hasattr(self._builder, "after_create"):
                from .models import SessionCreatedContext

                session_id = params.get("session_id", "")
                context = SessionCreatedContext.from_dict(params)
                try:
                    result = self._builder.after_create(session_id, context)
                    if asyncio.iscoroutine(result):
                        await result
                except Exception as exc:
                    _log.warning("after_create callback failed: %s", exc)
            return None

        if method == "callback/build_agent":
            if self._builder is None:
                raise ValueError("no SessionAgentBuilder registered")
            raw_options = dict(params.get("options", {}))
            scope_id = raw_options.pop("scope_id", None)
            if not scope_id:
                raise ValueError("callback/build_agent requires scope_id in options")
            # Rust sends extra context (model, prompt) that is informational
            # only; the decoder keeps SessionBuildOptions fields and types
            # fork_source.
            opts = SessionBuildOptions.from_callback_options(raw_options)
            await self._builder.build_agent(opts)
            for t in opts.tools:
                if not isinstance(t, str):
                    raise TypeError(
                        f"build_agent produced non-string tool {type(t).__name__}: {t!r}"
                    )
            # Capture tool handlers scoped to this build's scope_id
            tool_names = []
            for name, handler in opts.tool_handlers.items():
                self._tool_handlers[(scope_id, name)] = handler
                tool_names.append(name)
            self._scope_tools[scope_id] = tool_names
            for execution in opts.job_executions.values():
                self._register_job_runner(execution, opts.profile_name)
            return opts.to_dict()

        if method == "callback/call_tool":
            scope_id = params.get("scope_id")
            if not scope_id:
                raise ValueError("callback/call_tool requires scope_id")
            tool_name = params.get("tool", "")
            arguments = params.get("arguments", {})
            handler = self._tool_handlers.get((scope_id, tool_name))
            if handler is None:
                raise ValueError(
                    f"no handler registered for tool: {tool_name} (scope: {scope_id})"
                )
            result = handler(arguments)
            if asyncio.iscoroutine(result):
                result = await result
            # Rich content is opt-in: only an explicit ToolResultContent is
            # delivered as content blocks (images / multi-block). Any other
            # return keeps the legacy single-text-block behavior.
            from .tool_content import ToolResultContent

            if isinstance(result, ToolResultContent):
                return {"content_blocks": result.blocks}
            return {"content": result}

        if method == "callback/job/start":
            return await self._start_job(params)

        if method == "callback/job/reconcile":
            return await self._reconcile_jobs(params)

        if method == "callback/job/cancel":
            return await self._cancel_job(params)

        if method == "callback/schedule_fire":
            runnable = params.get("runnable", "")
            handler = self._schedule_fire_handlers.get(runnable)
            if handler is None:
                # The error crosses the bridge and fails the occurrence in the
                # durable schedule store — never silently complete a fire the
                # app has no handler for.
                raise ValueError(
                    f"no schedule-fire handler registered for runnable: {runnable}"
                )
            occurrence = params.get("occurrence", {})
            result = handler(occurrence)
            if inspect.isawaitable(result):
                result = await result
            return result

        # ----- Identity-first provider routing (REQ-45) -----
        if method.startswith("callback/roster_provider/"):
            return await self._handle_roster_provider(method, params)
        if method.startswith("callback/topology_provider/"):
            return await self._handle_topology_provider(method, params)
        if method.startswith("callback/agent_customizer/"):
            return await self._handle_agent_customizer(method, params)

        raise ValueError(f"unknown callback method: {method}")

    # --- Detached callback job shell -------------------------------------

    def _register_job_runner(
        self,
        execution: DetachedJobExecution,
        profile_name: str | None,
    ) -> None:
        key = execution.runner_key
        existing = self._job_runners.get(key)
        if existing is not None and existing.profile_name != profile_name:
            raise ValueError(
                f"detached runner {execution.runner!r}@{execution.version} is already "
                f"bound to profile {existing.profile_name!r}; refusing conflicting "
                f"profile {profile_name!r}"
            )
        self._job_runners[key] = _RegisteredJobRunner(execution, profile_name)

    @staticmethod
    def _runner_key(params: Mapping[str, Any]) -> tuple[str, str]:
        runner = params.get("runner")
        if not isinstance(runner, Mapping):
            raise ValueError("job callback requires a runner object")
        name = runner.get("name")
        version = runner.get("version")
        if not isinstance(name, str) or not name:
            raise ValueError("job callback runner requires a non-empty name")
        if not isinstance(version, str) or not version:
            raise ValueError("job callback runner requires a non-empty version")
        return (name, version)

    async def _resolve_job_credentials(
        self,
        registration: _RegisteredJobRunner,
        scopes: tuple[str, ...],
    ) -> Mapping[str, Any]:
        if not scopes:
            return {}
        resolver = self._job_credential_resolver
        if resolver is None:
            raise ValueError(
                "detached callback requires credential scopes but no execution-time "
                "credential resolver is configured"
            )
        resolved = resolver(registration.profile_name, scopes)
        if inspect.isawaitable(resolved):
            resolved = await resolved
        if not isinstance(resolved, Mapping):
            raise TypeError("job credential resolver must return a mapping")
        return resolved

    async def _start_job(self, params: dict[str, Any]) -> dict[str, Any]:
        authority_raw = params.get("authority")
        if not isinstance(authority_raw, Mapping):
            raise ValueError("callback/job/start requires authority")
        authority = DetachedJobAuthority.from_dict(authority_raw)
        mutation = self._job_mutation_locks.setdefault(
            authority.job_id,
            _JobMutationLock(asyncio.Lock()),
        )
        mutation.users += 1
        try:
            async with mutation.lock:
                return await self._start_job_locked(params, authority)
        finally:
            mutation.users -= 1
            if mutation.users == 0:
                self._job_mutation_locks.pop(authority.job_id, None)

    async def _start_job_locked(
        self,
        params: dict[str, Any],
        authority: DetachedJobAuthority,
    ) -> dict[str, Any]:
        runner_key = self._runner_key(params)
        registration = self._job_runners.get(runner_key)
        runner_handle = params.get("runner_handle")
        if not isinstance(runner_handle, str) or not runner_handle:
            raise ValueError("callback/job/start requires a non-empty runner_handle")
        if registration is None or self._job_rpc is None:
            return {"accepted": False, "runner_handle": runner_handle}

        highest = self._job_highest_fence.get(authority.job_id)
        active = self._job_attempts.get(authority.job_id)
        if highest is not None and authority.fence < highest:
            return {"accepted": False, "runner_handle": runner_handle}
        if highest == authority.fence:
            if (
                active is not None
                and active.authority == authority
                and active.runner_handle == runner_handle
                and (active.task is None or not active.task.done())
            ):
                return {"accepted": True, "runner_handle": runner_handle}
            # A completed/foreign attempt at the same fence is never replayed
            # merely because its start callback was delivered again.
            return {"accepted": False, "runner_handle": runner_handle}

        scopes_raw = params.get("credential_scopes", [])
        if not isinstance(scopes_raw, list) or not all(
            isinstance(scope, str) and scope for scope in scopes_raw
        ):
            raise ValueError("credential_scopes must be a list of non-empty strings")
        try:
            credentials = await self._resolve_job_credentials(
                registration,
                tuple(scopes_raw),
            )
        except Exception:
            # Credential material never crosses the callback boundary. A
            # resolution failure is a clean start rejection so Meerkat's
            # machine can classify the committed attempt as attention-needed.
            return {"accepted": False, "runner_handle": runner_handle}

        # A strictly newer fence can only arrive from a later committed
        # machine claim. Supersede the old host shell; this does not mint or
        # mutate authority.
        if active is not None:
            active.superseded = True
            active.cancellation.set()
            if active.task is not None and not active.task.done():
                active.task.cancel()

        cancellation = asyncio.Event()
        reporter = _DetachedJobReporter(authority, self._job_rpc)
        context = DetachedJobContext(
            authority=authority,
            runner_handle=runner_handle,
            arguments=params.get("arguments"),
            credentials=credentials,
            resume_checkpoint=(
                params.get("resume_checkpoint")
                if isinstance(params.get("resume_checkpoint"), str)
                else None
            ),
            cancellation=cancellation,
            reporter=reporter,
        )
        attempt = _ActiveJobAttempt(
            authority=authority,
            runner_key=runner_key,
            runner_handle=runner_handle,
            cancellation=cancellation,
            task=None,
        )
        self._job_highest_fence[authority.job_id] = authority.fence
        self._job_attempts[authority.job_id] = attempt
        task = asyncio.create_task(
            self._run_job(attempt, registration.execution.handler, context, reporter),
            name=f"meta-job-{authority.job_id}-{authority.attempt_id}",
        )
        attempt.task = task
        self._job_tasks.add(task)
        task.add_done_callback(self._job_tasks.discard)
        return {"accepted": True, "runner_handle": runner_handle}

    async def _run_job(
        self,
        attempt: _ActiveJobAttempt,
        runner: Any,
        context: DetachedJobContext,
        reporter: _DetachedJobReporter,
    ) -> None:
        run = getattr(runner, "run", runner)
        try:
            if inspect.iscoroutinefunction(run):
                result = await run(context)
            else:
                result = await asyncio.to_thread(run, context)
                if inspect.isawaitable(result):
                    result = await result
            if attempt.superseded:
                return
            if context.cancelled:
                await reporter.cancel_ack()
            elif isinstance(result, DetachedJobResult):
                await reporter.complete(result.result_ref)
            elif result is None:
                await reporter.complete(None)
            else:
                raise TypeError(
                    "detached runner must return DetachedJobResult or None; "
                    "persist result bytes and return only their reference"
                )
        except asyncio.CancelledError:
            if not attempt.superseded:
                context.cancellation.set()
                try:
                    await reporter.cancel_ack()
                except Exception:
                    _log.exception("detached job cancel acknowledgement failed")
        except Exception:
            if not attempt.superseded:
                try:
                    # Do not copy exception text across the durable boundary:
                    # it can contain arguments or resolved secret material.
                    await reporter.fail("host_runner_failed")
                except Exception:
                    _log.exception("detached job failure report failed")
        finally:
            current = self._job_attempts.get(attempt.authority.job_id)
            if current is attempt and not attempt.adopted:
                self._job_attempts.pop(attempt.authority.job_id, None)

    async def _renew_reconciled_attempt(
        self,
        authority: DetachedJobAuthority,
    ) -> bool:
        if self._job_rpc is None:
            return False
        now_ms = time.time_ns() // 1_000_000
        try:
            await _DetachedJobReporter(authority, self._job_rpc).heartbeat(
                heartbeat_at_ms=now_ms,
                lease_expires_at_ms=now_ms + 120_000,
            )
        except Exception:
            return False
        return True

    async def _reconcile_jobs(self, params: dict[str, Any]) -> dict[str, Any]:
        raw_attempts = params.get("attempts")
        if not isinstance(raw_attempts, list):
            raise ValueError("callback/job/reconcile requires attempts")
        live: list[dict[str, Any]] = []
        for raw_attempt in raw_attempts:
            if not isinstance(raw_attempt, dict):
                raise ValueError("reconcile attempts must be objects")
            authority_raw = raw_attempt.get("authority")
            if not isinstance(authority_raw, Mapping):
                raise ValueError("reconcile attempt requires authority")
            authority = DetachedJobAuthority.from_dict(authority_raw)
            runner_key = self._runner_key(raw_attempt)
            runner_handle = raw_attempt.get("runner_handle")
            if not isinstance(runner_handle, str) or not runner_handle:
                raise ValueError("reconcile attempt requires runner_handle")
            active = self._job_attempts.get(authority.job_id)
            if active is not None and (
                active.authority == authority
                and active.runner_key == runner_key
                and active.runner_handle == runner_handle
            ):
                if (
                    not active.cancellation.is_set()
                    and (active.task is None or not active.task.done())
                    and await self._renew_reconciled_attempt(authority)
                ):
                    live.append(authority.to_dict())
                continue

            registration = self._job_runners.get(runner_key)
            if raw_attempt.get("restart_class") != "adoptable":
                # A reconstruction hook may adopt only work whose generated
                # lifecycle declaration permits adoption. Replay/resume is a
                # later machine-authorized claim, never a host inference.
                continue
            reconcile = (
                getattr(registration.execution.handler, "reconcile", None)
                if registration is not None
                else None
            )
            if not callable(reconcile):
                continue
            adopted = reconcile(raw_attempt)
            if inspect.isawaitable(adopted):
                adopted = await adopted
            if adopted is not True:
                continue
            highest = self._job_highest_fence.get(authority.job_id)
            if highest is not None and authority.fence < highest:
                continue
            self._job_highest_fence[authority.job_id] = authority.fence
            self._job_attempts[authority.job_id] = _ActiveJobAttempt(
                authority=authority,
                runner_key=runner_key,
                runner_handle=runner_handle,
                cancellation=asyncio.Event(),
                task=None,
                adopted=True,
            )
            if not await self._renew_reconciled_attempt(authority):
                current = self._job_attempts.get(authority.job_id)
                if current is not None and current.authority == authority:
                    self._job_attempts.pop(authority.job_id, None)
                continue
            live.append(authority.to_dict())
        return {"live_attempts": live}

    async def _cancel_job(self, params: dict[str, Any]) -> dict[str, Any]:
        authority_raw = params.get("authority")
        if not isinstance(authority_raw, Mapping):
            raise ValueError("callback/job/cancel requires authority")
        authority = DetachedJobAuthority.from_dict(authority_raw)
        active = self._job_attempts.get(authority.job_id)
        if active is None or active.authority != authority:
            return {"accepted": False}
        active.cancellation.set()
        registration = self._job_runners.get(active.runner_key)
        cancel = (
            getattr(registration.execution.handler, "cancel", None)
            if registration is not None
            else None
        )
        if callable(cancel):
            result = cancel(authority.to_dict())
            if inspect.isawaitable(result):
                await result
        if active.task is not None and not active.task.done():
            active.task.cancel()
        return {"accepted": True}

    # --- Provider dispatch helpers ---

    async def _handle_roster_provider(self, method: str, params: dict[str, Any]) -> Any:
        from .identity_first_models import RosterContext

        provider = self._roster_provider
        if provider is None:
            raise ValueError("no roster provider registered")

        op = method.rsplit("/", 1)[-1]

        if op == "roster":
            # The gateway nests the Rust `RosterContext` under `context`, the
            # same envelope as the topology and customizer callbacks.
            context = RosterContext.from_dict(params.get("context") or {})
            specs = await provider.roster(context)
            return [s.to_dict() for s in specs]

        raise ValueError(f"unknown roster_provider operation: {op}")

    async def _handle_topology_provider(self, method: str, params: dict[str, Any]) -> Any:
        provider = self._topology_provider
        if provider is None:
            raise ValueError("no topology provider registered")

        op = method.rsplit("/", 1)[-1]

        if op == "compute_edges":
            edges = await provider.compute_edges(
                params["target_identities"],
                params.get("context", {}),
            )
            return [e.to_dict() for e in edges]

        raise ValueError(f"unknown topology_provider operation: {op}")

    async def _handle_agent_customizer(self, method: str, params: dict[str, Any]) -> Any:
        from .identity_first_models import (
            AgentBuildContext,
            AgentBuildDraft,
            DurableAgentSpec,
        )
        from .models import SessionCreatedContext

        customizer = self._agent_customizer
        if customizer is None:
            raise ValueError("no agent customizer registered")

        op = method.rsplit("/", 1)[-1]

        if op == "customize_build":
            scope_id = params.get("scope_id")
            context = AgentBuildContext.from_dict(params["context"])
            spec = DurableAgentSpec.from_dict(params["spec"])
            draft = AgentBuildDraft.from_dict(params["draft"])
            await customizer.customize_build(context, spec, draft)
            # Capture any tool handlers the customizer registered via
            # draft.register_tool(), keyed by this build's scope — the same
            # (scope_id, tool) map build_agent uses, so callback/call_tool
            # dispatches to them. Guard on scope_id for forward/backward compat
            # with a gateway that does not yet send it.
            handlers = draft.tool_handlers
            if scope_id and handlers:
                # Release the previous scope for this identity before registering
                # the new one — customize_build is re-invoked per restore and the
                # gateway never signals scope release, so this bounds growth to one
                # live scope per identity (newest wins).
                prior = self._customizer_scope_by_identity.get(context.identity)
                if prior and prior != scope_id:
                    self.release_scope(prior)
                self._customizer_scope_by_identity[context.identity] = scope_id
                tool_names = self._scope_tools.setdefault(scope_id, [])
                for name, handler in handlers.items():
                    self._tool_handlers[(scope_id, name)] = handler
                    tool_names.append(name)
            return draft.to_dict()

        if op == "after_create":
            identity = params["identity"]
            session_id = params["session_id"]
            # The gateway sends the context's fields beside the identity and
            # the session, as for callback/after_create; a nested ``context``
            # object is read when one is present.
            context = SessionCreatedContext.from_dict(params.get("context") or params)
            if hasattr(customizer, "after_create"):
                result = customizer.after_create(identity, session_id, context)
                if asyncio.iscoroutine(result):
                    await result
            return None

        raise ValueError(f"unknown agent_customizer operation: {op}")
