"""Meta runtime object — the running instance returned by the builder."""
from __future__ import annotations

import asyncio
import base64
import hashlib
import hmac
import itertools
import json
import logging
import math
import mimetypes
import re
import uuid
import warnings
from pathlib import Path
import time
from typing import TYPE_CHECKING, Any, AsyncIterator, Callable, Mapping, Sequence
from urllib import request as urllib_request
from urllib.error import HTTPError, URLError

if TYPE_CHECKING:
    from .identity_first_models import RegisteredSpec

_log = logging.getLogger("meerkat_meta")

from .agent_builder import CallbackDispatcher, SessionAgentBuilder
from .errors import (
    CAPABILITY_UNAVAILABLE_CODE,
    CompletionCursorUnavailableError,
    REGISTERED_SPEC_CHANGED_CODE,
    MEMORY_BACKEND_UNAVAILABLE_CODE,
    MOB_EVENTS_STALE_CURSOR_CODE,
    STALE_DELIVERY_SCOPE_CODE,
    STORAGE_RESOLUTION_CODE,
    INIT_IN_PROGRESS_CODE,
    COMPOSITION_PROVENANCE_CODE,
    MEMBER_RELOAD_REQUIRED_CODE,
    MEMBER_RELOAD_REQUIRED_KIND,
    FLOW_ROLE_TARGETS_REFUSED_KIND,
    FLOW_RUN_IDEMPOTENCY_CONFLICT_KIND,
    WORKGRAPH_CONFLICT_CODE,
    WORKGRAPH_UNAVAILABLE_CODE,
    IDENTITY_HISTORY_UNAVAILABLE_CODE,
    IDENTITY_HOST_ADOPTED_CODE,
    IDENTITY_UNRESUMABLE_CODE,
    IDENTITY_WAKE_HELD_KINDS,
    CapabilityUnavailableError,
    CompositionProvenanceError,
    FlowRoleTargetsNotAppliedError,
    FlowRoleTargetsRefusedError,
    FlowRunIdempotencyConflictError,
    MemberReloadRequiredError,
    CONSOLE_TIMELINE_REPLAY_UNAVAILABLE_CODE,
    ConsoleTimelineReplayUnavailableError,
    RegisteredSpecChangedError,
    InitInProgressError,
    InitOutcomeUnknownError,
    MemoryBackendUnavailableError,
    MobEventsStaleError,
    NotConnectedError,
    RpcError,
    StaleScopeError,
    StorageResolutionError,
    TransportError,
    TransportReaderFailedError,
    GatewayChildEndedError,
    GatewayStartupCleanupError,
    IdentityHistoryUnavailableError,
    IdentityHostAdoptedError,
    IdentityUnresumableError,
    IdentityWakeHeldError,
    WorkGraphConflictError,
    WorkGraphUnavailableError,
    announcement_error_from_rpc,
)
from .events import AgentEvent, MobEvent
from .identity_first_models import (
    CompletionCursor,
    CompletionProgress,
    IdentityBootstrapStatus,
    _completion_cursor_from,
)
from .live import (
    LIVE_EXECUTION_IDENTITY_V1,
    ActiveLiveChannelConnection,
    ActiveLiveChannelHandle,
    ExperimentalLiveChannelStatus,
    LiveAssistantOutputAddress,
    LiveChannelHandle,
    LiveExecutionIdentityV1,
    LivePlaybackOwner,
    LivePlaybackOwnerReadiness,
    LivePlaybackCompleteResult,
    LiveReplacementRequired,
    PendingLiveChannelHandle,
    live_open_execution_identity_params,
    supports_live_execution_identity_v1,
    supports_live_execution_mode,
)
from ._sse import SseEvent, parse_sse_stream
from ._transport import (
    INIT_PROTOCOL_ACCEPTED_THEN_SETTLED,
    InitWatch,
    PersistentTransport,
    TransportTermination,
)
from .models import DiscoverySpec
from .types import (
    MemberRunStopReceipt,
    AgentMemoryForgetResult,
    AgentMemoryManifestResult,
    AgentMemoryRecallResult,
    AgentMemoryRecord,
    AgentMemoryRecordMeta,
    AgentMemoryUpdateResult,
    CallToolResult,
    CapabilitiesResult,
    FLOW_ROLE_TARGETS,
    FLOW_RUN_IDEMPOTENCY_KEY,
    DeliveryHistoryResult,
    DeliveryResult,
    GatingAuditEntry,
    GatingDecisionResult,
    GatingEvaluateResult,
    GatingPendingEntry,
    MemberHealth,
    MemberReloadResult,
    MemberSnapshot,
    MemoryIndexResult,
    MemoryQueryResult,
    MemoryStoreInfo,
    MobpackAgentDefinitionsResult,
    MobpackApplyOperationResult,
    MobpackCatalogsResult,
    MobpackDeployCommandResult,
    MobpackDeployResult,
    MobpackDraftDeleteResult,
    MobpackDraftGetResult,
    MobpackDraftHistoryResult,
    MobpackDraftListResult,
    MobpackDraftSaveResult,
    MobpackExportResult,
    MobpackImportResult,
    MobpackSkillsCatalogResult,
    MobpackSourceResult,
    MobpackTemplatesResult,
    MobpackToolsCatalogResult,
    MobpackValidationResult,
    MobStructuralEvent,
    ModelsCatalogResult,
    ReconcileEdgesReport,
    ReconcileResult,
    RediscoverReport,
    RoutingResolution,
    RuntimeRouteResult,
    ScheduleToolsResult,
    SendMessageResult,
    SpawnResult,
    StatusResult,
    SubscribeResult,
    WorkGraphAttentionBinding,
    WorkGraphAttentionReassignResult,
    WorkGraphEdge,
    WorkGraphEventEntry,
    WorkGraphGoalResult,
    WorkGraphItem,
    WorkGraphItemsResult,
    WorkGraphSnapshotResult,
    AnnounceResult,
    Announcement,
    AnnouncementAuditEntry,
    AnnouncementBatch,
    AnnouncementDeliveryObservation,
    AnnouncementMember,
    AnnouncementSource,
    AnnouncementSourcePutResult,
    AnnouncementSourceView,
    AnnouncementSubscription,
    AnnouncementSubscriptionChange,
    AnnouncementSubscriptionView,
    SourceAdvanceResult,
    AnnouncementEscalationsPage,
    AnnouncementMemberView,
    AnnouncementBatchHandling,
    AnnouncementDeclarationsView,
    AnnouncementRoleSelector,
    AnnouncementRoleTemplate,
    supports_flow_role_targets,
    supports_flow_run_idempotency_key,
)

from ._client import _build_request as _rpc_request

_request_counter = itertools.count(1)
_IDENTITY_BOOTSTRAP_WAIT_TRANSPORT_HEADROOM_SECONDS = 5.0
# JSON-RPC 2.0 "method not found": an older gateway without the method.
_JSONRPC_METHOD_NOT_FOUND = -32601
# The feature capability of a keyed ``meta/send`` that takes ``handling_mode``.
_KEYED_SEND_HANDLING_MODE = "keyed_send_handling_mode"
# The mob announcement methods a Meta without sources serves. Any other
# ``meta/mob_announcements/*`` method came with sources, so method-not-found
# on it means the gateway's Meta does not support sources.
_ANNOUNCEMENT_METHODS_BEFORE_SOURCES = frozenset(
    "meta/mob_announcements/" + name
    for name in (
        "announce",
        "get",
        "subscriptions/put",
        "subscriptions/pause",
        "subscriptions/resume",
        "subscriptions/cancel",
        "subscriptions/get",
        "subscriptions/list",
        "deliveries/list",
        "deliveries/observe",
        "deliveries/reconcile",
    )
)
# Transport allowance beyond a server-side wait's own deadline, so the
# gateway's typed answer at the deadline arrives before the transport gives up.
_SERVER_WAIT_TRANSPORT_HEADROOM_SECONDS = 5.0
# Backoff between exact-ticket observation retries after an admitted
# delivery's wait failed for a transport or RPC reason; bounded by the
# caller's remaining deadline.
_OBSERVATION_RETRY_INITIAL_SECONDS = 0.1
_OBSERVATION_RETRY_MAX_SECONDS = 2.0
# Failures of the observation itself, as opposed to the turn's typed outcome
# (failed, unknown, wait ended, deadline). The persistent transport reports a
# response timeout or a closed stdout as RuntimeError and a dead subprocess as
# an RPC error.
_OBSERVATION_FAILURES = (RpcError, TransportError, NotConnectedError, RuntimeError)


def _attach_admission(error: BaseException, admission: Any, ticket: str | None) -> None:
    """Record the admitted delivery on a failure raised after admission.

    The error keeps its type and is re-raised by the caller unchanged; only
    ``admission`` and ``ticket`` are added where not already set, so a caller
    can keep the receipt instead of redispatching the work."""
    try:
        if getattr(error, "admission", None) is None:
            error.admission = admission  # type: ignore[attr-defined]
        if getattr(error, "ticket", None) is None:
            error.ticket = ticket  # type: ignore[attr-defined]
    except AttributeError:
        # A custom exception type that rejects attribute assignment raises
        # AttributeError here and cannot carry the receipt; it still
        # propagates unchanged.
        pass
def _next_request_id(method: str) -> str:
    return f"{method}:{next(_request_counter)}"


# ``rt:{identity}:{generation}``: a generated runtime alias, which the gateway
# resolves to the durable identity it names (the generation is numeric).
_RUNTIME_ALIAS_GENERATION = re.compile(r"\+?[0-9]+")
_U64_MAX = 2**64 - 1


def _durable_member_alias(member: str) -> str:
    """The member id ``meta/run_flow`` echoes for a requested one: a generated
    runtime alias names its durable identity; any other id is kept."""
    if member.startswith("rt:"):
        identity, separator, generation = member[3:].rpartition(":")
        if (
            separator
            and identity
            and _RUNTIME_ALIAS_GENERATION.fullmatch(generation)
            and int(generation) <= _U64_MAX
        ):
            return identity
    return member


def _role_targets_request(role_targets: Mapping[str, Sequence[str]]) -> dict[str, list[str]]:
    """The ``role_targets`` parameter: each role with its member ids."""
    request: dict[str, list[str]] = {}
    for role, members in role_targets.items():
        if not isinstance(role, str):
            raise TypeError("role_targets roles must be strings")
        if isinstance(members, str) or not all(isinstance(member, str) for member in members):
            raise TypeError(f"role_targets[{role!r}] must be a sequence of member id strings")
        request[role] = list(members)
    return request


def _role_targets_echoed(requested: dict[str, list[str]], echoed: Any) -> bool:
    """Whether ``meta/run_flow`` echoed the binding it was sent."""
    if not isinstance(echoed, dict) or set(echoed) != set(requested):
        return False
    for role, members in requested.items():
        echoed_members = echoed[role]
        if not isinstance(echoed_members, list) or not all(
            isinstance(member, str) for member in echoed_members
        ):
            return False
        if set(echoed_members) != {_durable_member_alias(member) for member in members}:
            return False
    return True


def _rpc_error_from_payload(
    err: dict[str, Any],
    *,
    request_id: str,
    method: str,
) -> RpcError:
    code = int(err.get("code", -1))
    message = str(err.get("message", err))
    data = err.get("data")
    if code == CAPABILITY_UNAVAILABLE_CODE:
        return CapabilityUnavailableError(
            message,
            request_id=request_id,
            method=method,
            data=data,
        )
    if code == REGISTERED_SPEC_CHANGED_CODE:
        return RegisteredSpecChangedError(
            message,
            request_id=request_id,
            method=method,
            data=data,
        )
    if code == STALE_DELIVERY_SCOPE_CODE:
        return StaleScopeError(
            message,
            mismatch=(data.get("mismatch") if isinstance(data, dict) else None),
            request_id=request_id,
            method=method,
            data=data,
        )
    if code == MEMORY_BACKEND_UNAVAILABLE_CODE:
        return MemoryBackendUnavailableError(
            message,
            request_id=request_id,
            method=method,
            data=data,
        )
    if code == CONSOLE_TIMELINE_REPLAY_UNAVAILABLE_CODE:
        return ConsoleTimelineReplayUnavailableError(
            message,
            request_id=request_id,
            method=method,
            data=data,
        )
    if code == WORKGRAPH_UNAVAILABLE_CODE:
        return WorkGraphUnavailableError(
            message,
            request_id=request_id,
            method=method,
            data=data,
        )
    if code == WORKGRAPH_CONFLICT_CODE:
        return WorkGraphConflictError(
            message,
            detail=(data.get("detail") if isinstance(data, dict) else None),
            request_id=request_id,
            method=method,
            data=data,
        )
    if code == STORAGE_RESOLUTION_CODE:
        return StorageResolutionError(
            message,
            request_id=request_id,
            method=method,
            data=data,
        )
    if code == INIT_IN_PROGRESS_CODE:
        return InitInProgressError(
            message,
            request_id=request_id,
            method=method,
            data=data,
        )
    if code == COMPOSITION_PROVENANCE_CODE:
        return CompositionProvenanceError(
            message,
            request_id=request_id,
            method=method,
            data=data,
        )
    if code == IDENTITY_UNRESUMABLE_CODE:
        return IdentityUnresumableError(
            message,
            request_id=request_id,
            method=method,
            data=data,
        )
    if code == IDENTITY_HISTORY_UNAVAILABLE_CODE:
        return IdentityHistoryUnavailableError(
            message,
            request_id=request_id,
            method=method,
            data=data,
        )
    if code == IDENTITY_HOST_ADOPTED_CODE:
        return IdentityHostAdoptedError(
            message,
            request_id=request_id,
            method=method,
            data=data,
        )
    announcement_error = announcement_error_from_rpc(
        code, message, request_id=request_id, method=method, data=data
    )
    if announcement_error is not None:
        return announcement_error
    if (
        code == -32603
        and isinstance(data, dict)
        and data.get("kind") in IDENTITY_WAKE_HELD_KINDS
    ):
        return IdentityWakeHeldError(
            message,
            request_id=request_id,
            method=method,
            data=data,
        )
    if (
        code == MEMBER_RELOAD_REQUIRED_CODE
        and isinstance(data, dict)
        and data.get("kind") == MEMBER_RELOAD_REQUIRED_KIND
    ):
        return MemberReloadRequiredError(
            message,
            request_id=request_id,
            method=method,
            data=data,
        )
    if (
        code == -32602
        and isinstance(data, dict)
        and data.get("kind") == FLOW_ROLE_TARGETS_REFUSED_KIND
    ):
        return FlowRoleTargetsRefusedError(
            message,
            request_id=request_id,
            method=method,
            data=data,
        )
    if (
        code == -32602
        and isinstance(data, dict)
        and data.get("kind") == FLOW_RUN_IDEMPOTENCY_CONFLICT_KIND
    ):
        return FlowRunIdempotencyConflictError(
            message,
            request_id=request_id,
            method=method,
            data=data,
        )
    base = RpcError(
        code=code,
        message=message,
        request_id=request_id,
        method=method,
        data=data,
    )
    if code == MOB_EVENTS_STALE_CURSOR_CODE:
        return MobEventsStaleError.from_rpc_error(base)
    return base


def _read_upload_source(
    source: bytes | bytearray | memoryview | str | Path,
    *,
    media_type: str | None = None,
    filename: str | None = None,
) -> tuple[bytes, str, str]:
    if isinstance(source, (str, Path)):
        path = Path(source)
        data = path.read_bytes()
        resolved_filename = filename or path.name or "attachment"
        resolved_media_type = media_type or mimetypes.guess_type(resolved_filename)[0]
    else:
        data = bytes(source)
        resolved_filename = filename or "attachment.bin"
        resolved_media_type = media_type or mimetypes.guess_type(resolved_filename)[0]
    return data, resolved_media_type or "application/octet-stream", resolved_filename


def _normalize_upload_item(
    item: Any,
    index: int,
) -> tuple[bytes, str, str]:
    if isinstance(item, dict):
        source = item.get("data", item.get("bytes", item.get("path", item.get("file"))))
        if source is None:
            raise ValueError("attachment dict requires data, bytes, path, or file")
        return _read_upload_source(
            source,
            media_type=item.get("media_type"),
            filename=item.get("filename"),
        )
    if isinstance(item, tuple) and len(item) in (2, 3):
        source = item[0]
        media_type = str(item[1]) if item[1] is not None else None
        filename = str(item[2]) if len(item) == 3 and item[2] is not None else None
        return _read_upload_source(source, media_type=media_type, filename=filename)
    if isinstance(item, (str, Path)):
        return _read_upload_source(item)
    filename = f"attachment-{index + 1}.png"
    return _read_upload_source(item, media_type="image/png", filename=filename)


class JobsHandle:
    """Safe application-facing durable-job surface."""

    def __init__(self, runtime: MetaRuntime) -> None:
        self._runtime = runtime

    async def get(self, job_id: str) -> Any:
        return await self._runtime._rpc("jobs/get", {"job_id": job_id})

    async def list(
        self,
        *,
        session_id: str,
        limit: int | None = None,
    ) -> Any:
        params: dict[str, Any] = {"session_id": session_id}
        if limit is not None:
            params["limit"] = limit
        return await self._runtime._rpc("jobs/list", params)

    async def cancel(self, job_id: str) -> Any:
        return await self._runtime._rpc("jobs/cancel", {"job_id": job_id})

    async def progress(self, job_id: str) -> Any:
        return await self._runtime._rpc("jobs/progress", {"job_id": job_id})

    async def result(self, job_id: str) -> Any:
        return await self._runtime._rpc("jobs/result", {"job_id": job_id})

    async def artifacts(self, job_id: str) -> Any:
        return await self._runtime._rpc("jobs/artifacts", {"job_id": job_id})

    async def retry(self, job_id: str, *, retry_due_at_ms: int) -> Any:
        return await self._runtime._rpc(
            "jobs/retry",
            {"job_id": job_id, "retry_due_at_ms": retry_due_at_ms},
        )

    async def health(self) -> Any:
        return await self._runtime._rpc("jobs/health", {})

    async def subscribe(
        self,
        job_id: str,
        *,
        subscription_id: str,
        session_id: str,
        delivery: dict[str, Any],
    ) -> Any:
        return await self._runtime._rpc(
            "jobs/subscribe",
            {
                "job_id": job_id,
                "subscription_id": subscription_id,
                "session_id": session_id,
                "delivery": delivery,
            },
        )

    async def unsubscribe(self, job_id: str, *, subscription_id: str) -> Any:
        return await self._runtime._rpc(
            "jobs/unsubscribe",
            {"job_id": job_id, "subscription_id": subscription_id},
        )


class MonitorsHandle:
    """Durable monitor convenience surface; lifecycle remains job-owned."""

    def __init__(self, runtime: MetaRuntime) -> None:
        self._runtime = runtime

    async def start(
        self,
        *,
        session_id: str,
        submission_key: str,
        command: str,
        timeout_secs: int,
        restart_class: str,
        delivery: dict[str, Any],
        protocol: str = "framed_jsonl",
        working_dir: str | None = None,
        max_line_bytes: int | None = None,
        max_notifications_per_window: int | None = None,
        notification_window_ms: int | None = None,
        max_retained_diagnostic_bytes: int | None = None,
    ) -> Any:
        params: dict[str, Any] = {
            "session_id": session_id,
            "submission_key": submission_key,
            "command": command,
            "timeout_secs": timeout_secs,
            "protocol": protocol,
            "restart_class": restart_class,
            "delivery": delivery,
        }
        for key, value in (
            ("working_dir", working_dir),
            ("max_line_bytes", max_line_bytes),
            ("max_notifications_per_window", max_notifications_per_window),
            ("notification_window_ms", notification_window_ms),
            ("max_retained_diagnostic_bytes", max_retained_diagnostic_bytes),
        ):
            if value is not None:
                params[key] = value
        return await self._runtime._rpc("monitors/start", params)


class MetaRuntime:
    """Running Meta runtime instance.

    Supports both context-manager and explicit lifecycle patterns::

        # Context manager
        async with await Meta.builder().mob("mob.toml").build() as rt:
            handle = rt.mob_handle()
            status = await handle.status()

        # Explicit lifecycle
        rt = await Meta.builder().mob("mob.toml").build()
        await rt.connect()
        ...
        await rt.shutdown()
    """

    def __init__(self, config: Any, transport: PersistentTransport | None = None):
        self._config = config
        self._transport = transport
        self._running = False
        # The gateway child this runtime bootstrapped, kept for observation
        # after shutdown or a failed bootstrap detaches ``_transport``.
        self._gateway_lifetime: Any = None
        self._dispatcher = CallbackDispatcher()
        self._rust_http_base: str | None = None
        self._rust_http_public_base: str | None = None
        self._lifecycle_lock = asyncio.Lock()
        self._shutdown_task: asyncio.Task[None] | None = None
        # Set while a stop of ``_transport`` was requested and has not
        # succeeded: RPC admission is closed, and the runtime keeps that
        # transport (and its live child) until a stop succeeds.
        self._stop_pending = False
        # The gateway transport this runtime last bootstrapped, for its
        # termination record after a stop released ``_transport``.
        self._last_gateway_transport: PersistentTransport | None = None
        self.jobs = JobsHandle(self)
        self.monitors = MonitorsHandle(self)

    async def __aenter__(self) -> MetaRuntime:
        if not self._running:
            await self.connect()
        return self

    async def __aexit__(self, exc_type: Any, exc_val: Any, exc_tb: Any) -> None:
        await self.shutdown()

    @classmethod
    async def _create(cls, config: Any) -> MetaRuntime:
        runtime = cls(config)
        await runtime.connect()
        return runtime

    async def connect(self) -> None:
        """Explicitly connect to the runtime. Idempotent.

        A gateway transport whose earlier stop did not succeed is stopped
        again first, on the same child; while that stop fails, ``connect()``
        raises its error and starts no other gateway.

        A startup that fails stops its gateway transport. When that stop
        fails, ``GatewayStartupCleanupError`` carries the exact startup error
        and stop failure and this runtime's cleanup. A startup that is
        cancelled or interrupted (``asyncio.CancelledError``, a group holding
        one, ``KeyboardInterrupt``), or a caller cancelled while the stop
        runs, is not wrapped: while the stop has not succeeded, the exception
        that propagates (the same object, with its type and chain) carries
        ``gateway_cleanup``, this runtime's ``GatewayCleanup``, whose
        ``shutdown()`` joins a stop still running or retries a failed one and
        never starts a gateway. An exception that refuses the attribute
        propagates unchanged without it, and the refusal is logged. Under ``asyncio.timeout()`` or
        ``asyncio.wait_for()``, that exact exception is the ``__cause__`` of
        the ``TimeoutError`` on Python 3.11 and later; on 3.10, ``wait_for``
        raises from a fresh ``CancelledError`` whose ``__context__`` is it.
        Keep the cleanup from the first exception you observe: awaiting an
        already cancelled task again can raise a fresh ``CancelledError``
        without it, and a wrapper that drops the chain drops it too. A
        startup whose stop succeeded propagates its exception without it.
        """
        async with self._lifecycle_lock:
            shutdown_task = self._shutdown_task
            if shutdown_task is not None:
                try:
                    await asyncio.shield(shutdown_task)
                finally:
                    if shutdown_task.done() and self._shutdown_task is shutdown_task:
                        self._shutdown_task = None
            if self._running:
                return
            # A retained transport whose stop has not succeeded settles first,
            # on that same owner: no second gateway child starts beside a live
            # one, and no replacement transport is made until it does.
            await self._settle_retained_transport()
            await self._bootstrap()

    def _schedule_transport_stop(
        self, transport: PersistentTransport
    ) -> asyncio.Task[None]:
        # RPC admission closes at once. The runtime keeps ``transport`` until
        # this stop succeeds, so a failed stop is rejoined on that transport.
        self._stop_pending = True
        stop_task = asyncio.create_task(asyncio.to_thread(transport.stop))
        self._shutdown_task = stop_task

        def settle_shutdown_task(done: asyncio.Task[None]) -> None:
            if self._shutdown_task is done:
                self._shutdown_task = None
            if done.cancelled() or done.exception() is not None:
                return
            if self._transport is transport:
                self._transport = None
                self._stop_pending = False

        stop_task.add_done_callback(settle_shutdown_task)
        return stop_task

    async def _settle_retained_transport(self) -> None:
        """Rejoin the stop of a retained transport, under the lifecycle lock."""
        transport = self._transport
        if transport is not None and getattr(self, "_stop_pending", False):
            await asyncio.shield(self._schedule_transport_stop(transport))

    async def _cleanup_failed_bootstrap(
        self, transport: PersistentTransport, startup_error: BaseException
    ) -> None:
        """Stop a transport that failed before becoming usable.

        The runtime keeps the transport until a stop succeeds, and the stop
        task remains runtime-owned if the caller is cancelled: a later
        ``connect()`` settles it before it starts a replacement gateway. A
        failed stop raises ``GatewayStartupCleanupError`` with the exact
        startup error and the exact stop failure, which can be any exception,
        a group included. A cancelled or interrupted startup keeps its own
        exception, unchanged; while the stop has not succeeded, the exception
        that propagates carries this runtime's cleanup as ``gateway_cleanup``.
        connect() settled any earlier stop before it bootstrapped, so this
        stop is the only one in flight.
        """
        self._running = False
        self._rust_http_base = None
        self._rust_http_public_base = None

        stop_task = self._schedule_transport_stop(transport)
        try:
            await asyncio.shield(stop_task)
        except BaseException as observed:
            cleanup_error = _stop_failure(stop_task)
            if observed is not cleanup_error:
                # This caller was cancelled, or the stop task itself was: no
                # stop failure is made up, and the cancellation propagates.
                # Unless the stop already succeeded, it carries this
                # runtime's cleanup, and the stop's own failure, now or once
                # it comes, is reported rather than lost with this caller.
                if not _stop_succeeded(stop_task):
                    _attach_gateway_cleanup(observed, _RuntimeGatewayCleanup(self))
                    if stop_task.done():
                        _log_unobserved_stop_failure(stop_task)
                    else:
                        stop_task.add_done_callback(_log_unobserved_stop_failure)
                raise
            if not isinstance(startup_error, Exception):
                # A cancelled or interrupted startup stays exactly that; the
                # runtime keeps the transport, and its cleanup travels with
                # the startup exception, as no runtime may be returned.
                _attach_gateway_cleanup(startup_error, _RuntimeGatewayCleanup(self))
                _log.error(
                    "gateway cleanup failed after an interrupted bootstrap",
                    exc_info=cleanup_error,
                )
                return
            # Every failed stop keeps the transport with this runtime, so its
            # cleanup travels with the failure: no runtime may be returned. A
            # failure is not proof the process still runs; the stop's record
            # tells what it observed.
            raise GatewayStartupCleanupError(
                startup_error, cleanup_error, _RuntimeGatewayCleanup(self)
            ) from startup_error
        finally:
            if stop_task.done() and self._shutdown_task is stop_task:
                self._shutdown_task = None

    def on_schedule_fire(self, name: str, handler: Any) -> None:
        """Register the handler for a named host runnable.

        Pairs with ``MetaBuilder.host_runnables([...])``: when a durable
        schedule with a ``host_runnable`` target of that name fires, the
        gateway sends ``callback/schedule_fire`` and this handler runs with
        the occurrence dict (``schedule_id``, ``occurrence_id``, ``due_at``,
        optional ``payload``). Sync or async; raising fails the occurrence
        attempt in the durable schedule store. Register before or after
        :meth:`connect` — fires only start once a schedule targets the name.
        """
        self._dispatcher.register_schedule_fire_handler(name, handler)

    def _session_builder_to_register(self) -> SessionAgentBuilder | None:
        """Return the configured session builder, refusing a non-conforming one.

        ``meta/init`` carries ``has_session_builder``, which makes the gateway
        call back into this process for every build. An object that cannot
        answer ``build_agent`` (a bare function, a partial, a misnamed method)
        used to be skipped here in silence while that flag was still sent, so
        the first spawn failed later with "no SessionAgentBuilder registered".
        Refuse it typed, before any gateway starts.
        """
        builder = self._config.session_builder
        if builder is None:
            return None
        if not isinstance(builder, SessionAgentBuilder):
            raise TypeError(
                "session_service() requires a SessionAgentBuilder: an object with "
                "an async build_agent(options) method; got "
                f"{type(builder).__name__}"
            )
        return builder

    def _make_gateway_transport(self) -> PersistentTransport:
        """Compose one local process transport before any public init request.

        Application-owned subclasses may carry private local startup channels
        (``inherited_fds``, ``extra_args``, ``env``). The returned transport
        must finish its own trusted bootstrap in start(). Remote RPC and model
        parameters never select this native factory.
        """
        assert self._config.gateway_bin is not None
        return PersistentTransport(self._config.gateway_bin)

    async def _bootstrap(self) -> None:
        # Resolve the builder before anything is spawned: a refusal here costs
        # nothing to clean up.
        session_builder = self._session_builder_to_register()
        if self._config.gateway_bin:
            transport = self._make_gateway_transport()
            self._transport = transport
            self._last_gateway_transport = transport
            self._rust_http_base = None
            self._rust_http_public_base = None
            try:
                # Detached host work reports asynchronously through ordinary
                # gateway RPC after the short callback/job/start response.
                self._dispatcher.register_job_rpc(self._rpc)
                if self._config.job_credential_resolver is not None:
                    self._dispatcher.register_job_credential_resolver(
                        self._config.job_credential_resolver
                    )
                # Register builder FIRST - init may trigger callback/build_agent
                if session_builder is not None:
                    self._dispatcher.register_builder(session_builder)
                if self._config.error_callback is not None:
                    self._dispatcher.register_error_callback(self._config.error_callback)
                # Register identity-first providers before transport start —
                # restore_flow during init may trigger provider callbacks.
                if self._config.roster_provider is not None:
                    self._dispatcher.register_roster_provider(self._config.roster_provider)
                if self._config.topology_provider is not None:
                    self._dispatcher.register_topology_provider(
                        self._config.topology_provider
                    )
                if self._config.agent_customizer is not None:
                    self._dispatcher.register_agent_customizer(
                        self._config.agent_customizer
                    )
                transport.set_callback_handler(self._dispatcher.handle_callback)
                transport.start()
                self._gateway_lifetime = getattr(transport, "_lifetime", None)
                if not transport.is_running():
                    raise TransportError(
                        f"gateway binary failed to start: {self._config.gateway_bin}"
                    )
                try:
                    init_result = await self._run_init(transport)
                    if isinstance(init_result, dict):
                        self._rust_http_base = init_result.get("http_base_url")
                        public_base = init_result.get("http_public_base_url")
                        self._rust_http_public_base = (
                            public_base
                            if isinstance(public_base, str) and public_base
                            else None
                        )
                        if not self._rust_http_base:
                            _log.warning(
                                "meta/init did not return http_base_url — "
                                "SSE event streaming unavailable"
                            )
                except Exception as init_err:
                    # A structured JSON-RPC error response is authoritative
                    # even though the gateway exits right after writing it
                    # (fail-closed init refusals such as the typed
                    # StorageResolutionError do exactly that): re-raise it
                    # typed instead of reporting a transport failure. An
                    # unknown outcome is already typed and must not be
                    # mistaken for a refusal or a plain transport failure.
                    if isinstance(init_err, (RpcError, InitOutcomeUnknownError)):
                        raise
                    if not transport.is_running():
                        raise TransportError(
                            f"gateway process died during bootstrap: {init_err}"
                        ) from init_err
                    raise TransportError(
                        f"meta/init failed: {init_err}"
                    ) from init_err
            except BaseException as startup_error:
                await self._cleanup_failed_bootstrap(transport, startup_error)
                raise
        elif session_builder is not None:
            self._dispatcher.register_builder(session_builder)
        else:
            _log.warning(
                "Meta runtime started without gateway or session builder — "
                "RPC calls will fail with NotConnectedError"
            )
        self._running = True

    async def _run_init(self, transport: PersistentTransport) -> Any:
        """Run ``meta/init`` as accepted-then-settled (#550).

        The init id is generated and its watch registered before the request
        is written. A gateway that supports the protocol answers ``accepted``
        at once; readiness comes only from the correlated ``ready``
        settlement, and a ``failed`` settlement raises the typed error with
        ``data["durable_effects"]``. An older gateway answers once with the
        final result, which is used as before. Losing the answer, the reader
        or the gateway after the request was written raises
        ``InitOutcomeUnknownError``: the gateway may have changed native
        state, so it is never reported as a refusal.
        """
        init_id = f"init-{uuid.uuid4()}"
        params = self._build_init_params()
        params["init_protocol"] = INIT_PROTOCOL_ACCEPTED_THEN_SETTLED
        params["init_id"] = init_id
        watch = transport.open_init_watch(init_id)
        try:
            return await self._init_with_watch(transport, watch, params)
        finally:
            transport.close_init_watch(watch)

    async def _init_with_watch(
        self,
        transport: PersistentTransport,
        watch: InitWatch,
        params: dict[str, Any],
    ) -> Any:
        init_id = watch.init_id
        rid = _next_request_id("meta/init")
        request = _rpc_request(rid, "meta/init", params)
        try:
            response = await transport.send_async(
                request, timeout=self._config.gateway_init_timeout
            )
        except TransportReaderFailedError as exc:
            raise InitOutcomeUnknownError(init_id, watch.last_phase, str(exc)) from exc
        except RuntimeError as exc:
            # The response wait ran out after the request was written: the
            # acceptance (or an older gateway's final answer) never came.
            raise InitOutcomeUnknownError(
                init_id, watch.last_phase, f"no answer to meta/init: {exc}"
            ) from exc
        if "error" in response:
            # Refused before acceptance (or by an older gateway): typed, and
            # nothing past the pre-durable gates ran.
            raise _rpc_error_from_payload(
                response["error"], request_id=rid, method="meta/init"
            )
        result = response.get("result")
        if not (isinstance(result, dict) and result.get("init_state") == "accepted"):
            # An older gateway: its single response is the final init result.
            return result
        if result.get("init_id") != init_id:
            raise InitOutcomeUnknownError(
                init_id,
                watch.last_phase,
                f"the gateway accepted a different init_id ({result.get('init_id')!r})",
            )
        try:
            settled = await watch.settlement(self._config.init_deadline)
        except TransportReaderFailedError as exc:
            raise InitOutcomeUnknownError(init_id, watch.last_phase, str(exc)) from exc
        except asyncio.TimeoutError as exc:
            raise InitOutcomeUnknownError(
                init_id,
                watch.last_phase,
                f"the init deadline of {self._config.init_deadline}s ran out before init settled",
            ) from exc
        outcome = settled.get("outcome")
        if outcome == "ready":
            return {
                key: value
                for key, value in settled.items()
                if key not in ("init_id", "outcome")
            }
        if outcome == "failed":
            data = settled.get("data")
            error_data = dict(data) if isinstance(data, dict) else {}
            if data is not None and not isinstance(data, dict):
                error_data["detail"] = data
            error_data["durable_effects"] = settled.get("durable_effects", "possible")
            raise _rpc_error_from_payload(
                {
                    "code": settled.get("code", -32603),
                    "message": settled.get("message", "meta/init failed"),
                    "data": error_data,
                },
                request_id=rid,
                method="meta/init",
            )
        raise InitOutcomeUnknownError(
            init_id,
            watch.last_phase,
            f"unrecognized meta/init settlement outcome {outcome!r}",
        )

    def _build_init_params(self) -> dict[str, Any]:
        """Build init params dict from builder config for meta/init RPC."""
        params: dict[str, Any] = {}
        if self._config.prepared_application_bundle is not None:
            import base64

            params["prepared_application"] = {
                "bundle_base64": base64.b64encode(
                    self._config.prepared_application_bundle
                ).decode("ascii"),
                "digest": self._config.prepared_application_digest,
            }
        elif self._config.mob_config_inline:
            params["mob_config"] = self._config.mob_config_inline
        elif self._config.mob_config_path:
            with open(self._config.mob_config_path) as f:
                params["mob_config"] = f.read()
        if self._config.modules:
            params["modules"] = self._config.modules
        params["has_session_builder"] = bool(self._config.session_builder)
        runtime_options: dict[str, Any] = {}
        if self._config.gating_config_path is not None:
            runtime_options["gating_config_path"] = self._config.gating_config_path
        if self._config.access_config_path is not None:
            runtime_options["access_config_path"] = self._config.access_config_path
        if self._config.meerkat_config_path:
            runtime_options["meerkat_config_path"] = self._config.meerkat_config_path
        if self._config.provider_credentials_root:
            runtime_options["provider_credentials_root"] = self._config.provider_credentials_root
        if self._config.workgraph_enabled is not None:
            runtime_options["workgraph"] = self._config.workgraph_enabled
        if self._config.mob_announcements is not None:
            runtime_options["mob_announcements"] = dict(self._config.mob_announcements)
        if self._config.fork_instruction_policy is not None:
            runtime_options["fork_instruction_policy"] = self._config.fork_instruction_policy
        if self._config.routing_config_path:
            runtime_options["routing_config_path"] = self._config.routing_config_path
        if self._config.host_runnables:
            runtime_options["host_runnables"] = list(self._config.host_runnables)
        if self._config.installed_artifacts is not None:
            runtime_options["installed_artifacts"] = [
                artifact.to_wire() for artifact in self._config.installed_artifacts
            ]
        if self._config.memory_config:
            runtime_options["memory_config"] = _serialize_config(self._config.memory_config)
        if self._config.agent_memory_config is not None:
            runtime_options["agent_memory"] = _serialize_config(
                self._config.agent_memory_config
            )
        if self._config.auth_config:
            runtime_options["auth_config"] = _serialize_config(self._config.auth_config)
        if self._config.event_log:
            runtime_options["event_log"] = _serialize_config(self._config.event_log)
        if self._config.runtime_store:
            runtime_options["runtime_store"] = _serialize_config(
                self._config.runtime_store
            )
        if self._config.console_read_only is not None:
            runtime_options["console_read_only"] = self._config.console_read_only
        if self._config.console_fetch_timeout_ms is not None:
            runtime_options["console_fetch_timeout_ms"] = (
                self._config.console_fetch_timeout_ms
            )
        if self._config.console_require_app_auth is not None:
            runtime_options["console_require_app_auth"] = (
                self._config.console_require_app_auth
            )
        if self._config.console_config_path:
            runtime_options["console_config_path"] = self._config.console_config_path
        if self._config.http_listen:
            runtime_options["http_listen"] = self._config.http_listen
        if self._config.http_public_base_url:
            runtime_options["http_public_base_url"] = self._config.http_public_base_url
        if self._config.allow_remote is not None:
            runtime_options["allow_remote"] = self._config.allow_remote
        if self._config.implicit_delegate_idle_retire_configured:
            runtime_options["implicit_delegate_idle_retire_secs"] = (
                self._config.implicit_delegate_idle_retire_secs
            )
        if self._config.identity_bootstrap_mode is not None:
            runtime_options["identity_bootstrap_mode"] = (
                self._config.identity_bootstrap_mode.to_dict()
            )
        if self._config.experimental_live_config is not None:
            runtime_options["experimental_live"] = (
                self._config.experimental_live_config.to_dict()
            )
        if self._config.openai_live_config is not None:
            runtime_options["openai_live"] = self._config.openai_live_config.to_dict()
        if self._config.console_voice_config is not None:
            runtime_options["console_voice"] = self._config.console_voice_config.to_dict()
        if self._config.declare_spec_update_expected_revision is not None:
            runtime_options["mob_composition"] = {"authority": "authoritative"}
            runtime_options["declare_spec_update"] = {
                "expected_revision": (
                    self._config.declare_spec_update_expected_revision
                )
            }
        params["runtime_options"] = runtime_options
        if self._config.persistent_state:
            params["persistent_state"] = self._config.persistent_state
        if self._config.shutdown_in_flight_drain_ms is not None:
            params["shutdown_in_flight_drain_ms"] = (
                self._config.shutdown_in_flight_drain_ms
            )
        # Identity-first provider flags
        if self._config.roster_provider is not None:
            params["has_roster_provider"] = True
        if self._config.role_migrations:
            params["role_migrations"] = [
                dict(declaration) for declaration in self._config.role_migrations
            ]
        if self._config.application_tool_policies:
            # Canonical bytes travel verbatim as strings; see the builder docstring.
            params["application_tool_policies"] = list(
                self._config.application_tool_policies
            )
        if self._config.child_application_tool_policy is not None:
            params["child_application_tool_policy"] = dict(
                self._config.child_application_tool_policy
            )
        if self._config.topology_provider is not None:
            params["has_topology_provider"] = True
        if self._config.agent_customizer is not None:
            params["has_agent_customizer"] = True
        return params

    def _rpc_admission_closed(self) -> bool:
        # A transport kept only until its stop succeeds takes no requests.
        return self._transport is None or getattr(self, "_stop_pending", False)

    def _rpc_sync(self, method: str, params: dict[str, Any] | None = None) -> Any:
        if self._rpc_admission_closed():
            raise NotConnectedError("runtime not started — no transport available")
        rid = _next_request_id(method)
        response = self._transport.send_sync(_rpc_request(rid, method, params))
        if "error" in response:
            raise _rpc_error_from_payload(response["error"], request_id=rid, method=method)
        return response.get("result")

    async def _rpc(
        self,
        method: str,
        params: dict[str, Any] | None = None,
        *,
        transport_timeout: float | None = None,
    ) -> Any:
        if self._rpc_admission_closed():
            raise NotConnectedError("runtime not started — no transport available")
        rid = _next_request_id(method)
        request = _rpc_request(rid, method, params)
        if transport_timeout is None:
            response = await self._transport.send_async(request)
        else:
            response = await self._transport.send_async(
                request,
                timeout=transport_timeout,
            )
        if "error" in response:
            raise _rpc_error_from_payload(response["error"], request_id=rid, method=method)
        return response.get("result")

    @property
    def rust_http_base_url(self) -> str | None:
        return self._rust_http_base

    @property
    def rust_http_public_base_url(self) -> str | None:
        """Base URL for clients that reach the gateway through a proxy.

        The ``http_public_base_url`` the launch advertised (see
        ``MetaBuilder.http_public_base_url``), falling back to
        ``rust_http_base_url`` when none was declared. The SDK's own SSE,
        multipart and console RPC calls keep using ``rust_http_base_url``:
        this process spawned the gateway and shares its network namespace,
        so the same-host form is always the one that answers.
        """
        return self._rust_http_public_base or self._rust_http_base

    def set_rust_http_base(self, url: str) -> None:
        self._rust_http_base = url

    def mob_handle(self) -> MobHandle:
        return MobHandle(self)

    def sse_bridge(self) -> SseBridge:
        return SseBridge(self)

    def asgi(
        self,
        *,
        console: bool = True,
        auth: Any | None = None,
        extra_routes: Any | None = None,
    ) -> AsgiApp:
        return AsgiApp(
            runtime=self,
            console=console,
            auth_config=auth,
            fallback_app=extra_routes,
        )

    async def serve(
        self,
        app: Any = None,
        *,
        host: str = "0.0.0.0",
        port: int = 8080,
    ) -> None:
        if app is None:
            app = self.asgi()
        try:
            import uvicorn
            config = uvicorn.Config(app, host=host, port=port, log_level="info")
            server = uvicorn.Server(config)
            await server.serve()
        except ImportError:
            stop = asyncio.Event()
            import signal
            loop = asyncio.get_running_loop()
            for sig in (signal.SIGINT, signal.SIGTERM):
                loop.add_signal_handler(sig, stop.set)
            await stop.wait()
        finally:
            await self.shutdown()

    # -----------------------------------------------------------------
    # Identity-first runtime APIs (REQ-41)
    # -----------------------------------------------------------------

    def agent(self, identity: str) -> IdentityAgentHandle:
        """Return an identity-scoped agent handle."""
        return IdentityAgentHandle(self, identity)

    async def send(
        self,
        identity: str,
        content: "str | list",
        *,
        track_turn: bool = False,
        expected_spec_digest: str | None = None,
        idempotency_key: str | None = None,
        handling_mode: str | None = None,
    ) -> Any:
        """Send conversational content to an addressable identity.

        With ``track_turn=True`` the admitted turn is tracked by ticket: the
        result's ``turn_ticket`` names THIS turn (see :meth:`turn_result`),
        or ``turn_unavailable`` says why it could not be tracked.

        A host that authored ``content`` from the identity's registered row
        (:meth:`registered_spec`) passes that row's ``spec_digest`` as
        ``expected_spec_digest``. The gateway re-checks it against the
        registered row under the identity's lifecycle lock before delivery
        and raises :class:`~meerkat_meta.errors.RegisteredSpecChangedError`,
        with nothing submitted, when the row changed.

        With ``idempotency_key`` the send is keyed: the gateway commits the
        content as a member continuation under the key, which belongs to the
        identity (not a session), and returns a :class:`KeyedSendResult`,
        the continuation's receipt with its current delivery status. A retry
        under the same key never sends twice: it answers the first receipt,
        after a respawn, reset or gateway restart too, and other content
        under the key is refused (``data.kind``
        ``input_idempotency_conflict``). Keys never expire. Follow an
        ``admitted`` status with :meth:`send_status`. A key does not combine
        with ``track_turn`` or ``expected_spec_digest``. A gateway that does
        not advertise ``meta/send_status`` would ignore the key, so the key is
        refused with :class:`CapabilityUnavailableError` before anything is
        sent.

        ``handling_mode`` is ``"queue"`` (the default) or ``"steer"``. With a
        key it is committed with the content: ``"steer"`` delivers it into
        the identity's running turn at its next boundary, or starts a turn
        when the identity is idle, and a replay of the key in another mode is
        refused as other content is. A steer requires a key: an unkeyed send
        queues, so ``"queue"`` there is the default and ``"steer"`` is
        refused before any call. A gateway that does not advertise the
        ``keyed_send_handling_mode`` feature capability would ignore the mode
        and queue, so a keyed steer is refused there with
        :class:`CapabilityUnavailableError` before anything is sent.
        """
        from .identity_first_models import KeyedSendResult, SendResult
        if idempotency_key is not None and (track_turn or expected_spec_digest is not None):
            combined = "track_turn" if track_turn else "expected_spec_digest"
            raise ValueError(f"idempotency_key does not combine with {combined}")
        if handling_mode is not None:
            if handling_mode not in ("queue", "steer"):
                raise ValueError('handling_mode must be "queue" or "steer"')
            if handling_mode == "steer" and idempotency_key is None:
                raise ValueError(
                    'handling_mode "steer" needs idempotency_key: an unkeyed send queues'
                )
        if isinstance(content, str):
            wire_content = content
        else:
            wire_content = [b.to_dict() for b in content]
        params: dict[str, Any] = {"identity": identity, "content": wire_content}
        if idempotency_key is not None:
            await self._require_keyed_send(steer=handling_mode == "steer")
            params["idempotency_key"] = idempotency_key
            if handling_mode is not None:
                params["handling_mode"] = handling_mode
            raw = await self._rpc("meta/send", params)
            if not isinstance(raw, dict):
                raise ValueError("meta/send returned a non-object keyed result")
            return KeyedSendResult.from_dict(raw)
        if track_turn:
            params["track_turn"] = True
        if expected_spec_digest is not None:
            params["expected_spec_digest"] = expected_spec_digest
        raw = await self._rpc("meta/send", params)
        return SendResult.from_dict(raw) if isinstance(raw, dict) else raw

    async def _require_keyed_send(self, *, steer: bool = False) -> None:
        """Refuse a keyed send, typed, on a gateway that does not serve it:
        such a gateway ignores ``idempotency_key`` and would send unkeyed.
        Keyed sends ship with ``meta/send_status``, so its advertisement is
        the gate. A keyed steer also needs the ``keyed_send_handling_mode``
        feature capability: a gateway without it ignores the mode and
        queues."""
        raw = await self._rpc("meta/capabilities")
        methods = raw.get("methods") if isinstance(raw, dict) else None
        if not isinstance(methods, list) or "meta/send_status" not in methods:
            raise CapabilityUnavailableError(
                "this gateway does not serve keyed sends: it would ignore "
                "idempotency_key and send unkeyed",
                method="meta/send",
                data={"requires": "meta/send_status"},
            )
        if steer:
            features = raw.get("feature_capabilities") if isinstance(raw, dict) else None
            if not isinstance(features, list) or _KEYED_SEND_HANDLING_MODE not in features:
                raise CapabilityUnavailableError(
                    "this gateway does not serve keyed steers: it would ignore "
                    "handling_mode and queue",
                    method="meta/send",
                    data={"capability": _KEYED_SEND_HANDLING_MODE},
                )

    async def send_status(self, identity: str, session_id: str, input_id: str) -> Any:
        """The state of an input in a session ``identity`` has had
        (``meta/send_status``), as meerkat's ``session/input_status`` reports
        it: follow a keyed send's ``admitted`` status (its ``session_id`` and
        ``input_id``) to its terminal state.

        Returns a :class:`SendStatusResult` whose ``state`` is ``None`` when
        the session holds no such input. A session that is neither the
        identity's current session nor one it held before is refused
        (``data.kind`` ``session_not_held_by_identity``).
        """
        from .identity_first_models import SendStatusResult
        raw = await self._rpc(
            "meta/send_status",
            {"identity": identity, "session_id": session_id, "input_id": input_id},
        )
        if not isinstance(raw, dict):
            raise ValueError("meta/send_status returned a non-object result")
        return SendStatusResult.from_dict(raw)

    async def dispatch(
        self,
        identity: str,
        dispatch_input: Any,
        *,
        track_turn: bool = False,
        expected_scope: Any = None,
    ) -> Any:
        """Dispatch content to any identity (addressable or internal).

        ``track_turn`` as for :meth:`send`.

        With ``expected_scope`` (a :class:`DeliveryScope` from :meth:`status`,
        persisted beforehand) the dispatch is scope-bound: exact content to the
        session the scope pins, never materialized, repaired, retargeted or
        prepared. It needs both ``idempotency_key`` and ``correlation_id``,
        does not combine with ``track_turn``, returns a
        :class:`ScopedDispatchResult` carrying the admission receipt, and
        raises :class:`StaleScopeError` when the scope moved. Recover a lost
        reply with :meth:`recover_delivery` and the same scope.
        """
        from .identity_first_models import DispatchResult, ScopedDispatchResult
        params: dict[str, Any] = {
            "identity": identity,
            "dispatch_input": dispatch_input.to_dict(),
        }
        if expected_scope is not None:
            if track_turn:
                raise ValueError("expected_scope does not combine with track_turn")
            params["expected_scope"] = expected_scope.to_dict()
            raw = await self._rpc("meta/dispatch", params)
            return ScopedDispatchResult.from_dict(raw) if isinstance(raw, dict) else raw
        if track_turn:
            params["track_turn"] = True
        raw = await self._rpc("meta/dispatch", params)
        return DispatchResult.from_dict(raw) if isinstance(raw, dict) else raw

    async def recover_delivery(
        self,
        identity: str,
        scope: Any,
        *,
        idempotency_key: str,
        correlation_id: str,
        timeout: float | None = None,
    ) -> Any:
        """Read one scoped delivery's state from the scope's ORIGINAL session.

        Never reads the identity's current session and never resubmits.
        ``ABSENT`` is an authoritative point-in-time miss and ``UNRESOLVED``
        an unreachable original owner; neither is permission to retry.
        ``timeout`` bounds the evidence read (seconds; gateway default when
        ``None``).
        """
        from .identity_first_models import ScopedRecovery
        params: dict[str, Any] = {
            "identity": identity,
            "scope": scope.to_dict(),
            "idempotency_key": idempotency_key,
            "correlation_id": correlation_id,
        }
        if timeout is not None:
            params["timeout_ms"] = max(0, int(timeout * 1000))
        raw = await self._rpc("meta/recover_delivery", params)
        return ScopedRecovery.from_dict(raw) if isinstance(raw, dict) else raw

    async def dispatch_text(
        self,
        identity: str,
        text: str,
        *,
        origin: str = "system",
        correlation_id: str | None = None,
        idempotency_key: str | None = None,
        track_turn: bool = False,
        expected_scope: Any = None,
    ) -> Any:
        """Dispatch plain text; supply correlation and idempotency keys together.

        ``track_turn`` and ``expected_scope`` as for :meth:`dispatch`.
        """
        from .identity_first_models import DispatchInput
        di = DispatchInput(
            content=text, origin=origin, correlation_id=correlation_id,
            idempotency_key=idempotency_key,
        )
        return await self.dispatch(
            identity, di, track_turn=track_turn, expected_scope=expected_scope,
        )

    async def turn_result(self, identity: str, ticket: str) -> Any:
        """Read one ticketed turn (``meta/turn_result``): its state and,
        once completed, its OWN output."""
        from .identity_first_models import TurnResult
        raw = await self._rpc(
            "meta/turn_result", {"identity": identity, "ticket": ticket}
        )
        return TurnResult.from_dict(raw) if isinstance(raw, dict) else raw

    async def list_identities(self) -> list[Any]:
        """Typed roster listing via ``meta/console/list_identities``.

        One call returns every roster member as a
        :class:`~meerkat_meta.identity_first_models.ConsoleIdentityRecord`:
        ``identity``, current ``session_id`` binding, ``labels``,
        addressability and health. This is the supported identity -> session
        map.

        ``labels`` are display data. ``labels["role"]`` keeps whatever the
        roster provider supplied (often human role prose) and falls back to
        the profile name only when the provider set none; it is not the
        member's profile or instruction authority. A member's instructions
        come from the mob definition the runtime was started with (its
        profiles and skills) and the projected roster, not from labels.

        Served over the gateway's console HTTP surface, so it requires
        ``rust_http_base_url`` (available once the gateway is started).
        Call it OUTSIDE build/tool callbacks (once around boot/reconcile,
        cached) rather than per member build.
        """
        from .identity_first_models import ConsoleIdentityRecord

        raw = await self._console_rpc("meta/console/list_identities", {})
        records = raw.get("identities", []) if isinstance(raw, dict) else []
        return [
            ConsoleIdentityRecord.from_dict(record)
            for record in records
            if isinstance(record, dict)
        ]

    async def _console_rpc(self, method: str, params: dict[str, Any]) -> Any:
        """One JSON-RPC call over the gateway's console HTTP route."""
        base = self.rust_http_base_url
        if not base:
            raise NotConnectedError(
                f"{method} is served over the console HTTP surface and requires "
                "rust_http_base_url; start the gateway or call "
                "runtime.set_rust_http_base('http://127.0.0.1:8081')"
            )
        request_id = _next_request_id(method)
        payload = _rpc_request(request_id, method, params)
        req = urllib_request.Request(
            base.rstrip("/") + "/console/rpc",
            data=json.dumps(payload).encode("utf-8"),
            method="POST",
            headers={
                "content-type": "application/json",
                "accept": "application/json",
            },
        )

        def _post() -> str:
            with urllib_request.urlopen(req) as response:
                return response.read().decode("utf-8", errors="replace")

        try:
            response_text = await asyncio.to_thread(_post)
        except HTTPError as exc:
            detail = exc.read().decode("utf-8", errors="replace")
            raise TransportError(
                f"console RPC {method} failed (status={exc.code}): {detail}"
            ) from exc
        except URLError as exc:
            raise TransportError(f"console RPC {method} failed: {exc.reason}") from exc
        response = json.loads(response_text)
        if "error" in response:
            raise _rpc_error_from_payload(
                response["error"], request_id=request_id, method=method
            )
        return response.get("result")

    async def subscribe(self, identity: str) -> Any:
        """Subscribe to identity-scoped events."""
        raw = await self._rpc("meta/subscribe", {"identity": identity})
        return raw

    async def registered_spec(self, identity: str) -> RegisteredSpec:
        """Return the identity's REGISTERED roster row and its digest.

        The full row (including ``additional_instructions``) the identity
        runtime holds and delivers under: not the roster provider's desired
        row, and not proof that the member's actor adopted every field. The
        read is informational; pass its ``spec_digest`` to :meth:`send` as
        ``expected_spec_digest`` and the gateway re-checks it before delivery.
        """
        from .identity_first_models import RegisteredSpec
        raw = await self._rpc("meta/registered_spec", {"identity": identity})
        return RegisteredSpec.from_dict(raw)

    async def status(self, identity: str) -> Any:
        """Return IdentityStatus for the given identity."""
        from .identity_first_models import IdentityStatus
        raw = await self._rpc("meta/status_identity", {"identity": identity})
        return IdentityStatus.from_dict(raw)

    async def inspect_identity(self, identity: str) -> Any:
        """Return execution-level inspection (output_preview, peer connectivity)."""
        from .identity_first_models import IdentityInspection
        raw = await self._rpc("meta/inspect_identity", {"identity": identity})
        return IdentityInspection.from_dict(raw)

    async def completion_cursor(self, identity: str) -> CompletionCursor | None:
        """Read the identity's completion cursor without touching its session.

        ``meta/completion_cursor`` is a cursor-only read: the gateway
        answers from the identity's in-memory completion cursor and never
        reads the member session. :meth:`inspect_identity` does read it (an
        execution snapshot on the member's session task), which a tight
        completion poll must avoid because it can hold a staged run.

        Returns ``None`` for a live alias with no identity authority (nothing
        tracks its turns; do not read that as "no turns yet"). Against a
        gateway predating the method this falls back to
        :meth:`inspect_identity`.
        """
        try:
            raw = await self._rpc("meta/completion_cursor", {"identity": identity})
        except RpcError as err:
            if err.code != _JSONRPC_METHOD_NOT_FOUND:
                raise
            return (await self.inspect_identity(identity)).completion_cursor
        if not isinstance(raw, dict):
            return None
        return _completion_cursor_from(raw, "completion_cursor")

    async def _server_wait(
        self, method: str, params: dict[str, Any], timeout: float
    ) -> dict[str, Any] | None:
        """Run a server-side wait (``meta/wait_for_completion``,
        ``meta/wait_for_turn``) with ``timeout`` as the caller's deadline.

        The gateway answers when the awaited change happens (it waits on the
        runtime's typed change signal) or, typed, at the deadline. Returns
        ``None`` against a gateway predating the method.
        """
        remaining = max(0.0, timeout)
        try:
            raw = await self._rpc(
                method,
                {**params, "timeout_ms": int(remaining * 1000)},
                transport_timeout=remaining + _SERVER_WAIT_TRANSPORT_HEADROOM_SECONDS,
            )
        except RpcError as err:
            if err.code != _JSONRPC_METHOD_NOT_FOUND:
                raise
            return None
        return raw if isinstance(raw, dict) else {}

    async def respawn(self, identity: str) -> Any:
        """Non-destructive durable recovery for an identity."""
        return await self._rpc("meta/respawn", {"identity": identity})

    async def retire(self, identity: str) -> Any:
        """Retire an identity through the standard retirement pipeline."""
        return await self._rpc("meta/retire", {"identity": identity})

    async def reset(self, identity: str) -> Any:
        """Destructive reset: the identity continues on a fresh session."""
        return await self._rpc("meta/reset", {"identity": identity})

    async def activate_member_instruction(
        self,
        identity: str,
        activation: dict[str, Any],
        *,
        transport_timeout: float | None = None,
    ) -> dict[str, Any]:
        """Append one keyed instruction activation to a member's current session.

        Host-only. ``activation`` is Meerkat's ``InstructionActivationRequest``
        (``revision``, ``activation_id``, ``expectation``, optional
        ``supersedes``, ``body``). The activation is a new transcript row that
        the member's next turn sees; history is never rewritten. Returns
        ``{"identity", "receipt"}``, where ``receipt["disposition"]`` is
        ``"applied"`` or ``"duplicate"``.

        A refusal raises ``RpcError`` with ``data["class"]`` set to
        ``"admission"`` (plus ``data["instruction_activation_code"]``, for
        example ``"session_busy"`` or ``"live_channel_open"``), ``"session"``
        (plus ``data["session_error_code"]`` and, when the request itself was
        refused, ``data["instruction_activation_error_code"]`` such as
        ``"digest_mismatch"`` or ``"effective_activation_conflict"``),
        ``"runtime"`` or ``"owner_task"``. Nothing is retried: on a
        ``session_busy`` or ``live_channel_open`` refusal, retry at your own
        next boundary.

        A call made while the member is mid-turn BLOCKS until that turn
        finalizes, so it can wait for a whole turn. It then applies at the
        boundary, never inside the turn, or raises ``session_busy`` if more
        runtime work is already queued there. ``transport_timeout`` (seconds)
        sets how long this call waits for the reply; it defaults to the
        transport's request timeout, so raise it for members with long turns.
        If the wait times out, the activation may still apply at the boundary:
        re-send the identical activation, which answers ``applied``,
        ``duplicate`` or the typed refusal.
        """
        raw = await self._rpc(
            "meta/member_activate_instruction",
            {"identity": identity, "activation": activation},
            transport_timeout=transport_timeout,
        )
        return raw if isinstance(raw, dict) else {}

    async def member_instruction_activations(
        self,
        identity: str,
        *,
        namespace: str | None = None,
        key: str | None = None,
        offset: int | None = None,
        limit: int | None = None,
    ) -> dict[str, Any]:
        """Read the durable instruction activations of a member's current session.

        Host-only. Returns ``{"identity", "page"}`` with Meerkat's
        ``InstructionActivationReadPage``.
        """
        params: dict[str, Any] = {"identity": identity}
        for name, value in (
            ("namespace", namespace),
            ("key", key),
            ("offset", offset),
            ("limit", limit),
        ):
            if value is not None:
                params[name] = value
        raw = await self._rpc("meta/member_instruction_activations", params)
        return raw if isinstance(raw, dict) else {}

    async def reconcile(self) -> Any:
        """Re-run restore_flow with fresh roster from the provider.

        Picks up roster changes (added/removed/modified identities) and
        topology changes without restarting the runtime.
        """
        return await self._rpc("meta/reconcile_identity", {})

    async def identity_bootstrap_status(self) -> IdentityBootstrapStatus:
        """Return typed materialization status for the bootstrap roster."""
        raw = await self._rpc("meta/status_identity_bootstrap", {})
        return IdentityBootstrapStatus.from_dict(raw)

    async def wait_identity_bootstrap(
        self,
        *,
        timeout: float | None = None,
        target: str = "materialized",
    ) -> IdentityBootstrapStatus:
        """Wait for identity bootstrap materialization or startup readiness.

        ``target="materialized"`` waits for every roster identity to finish
        warming. ``target="startup_ready"`` additionally waits for the active
        members' startup-ready signals.
        """
        if target not in {"materialized", "startup_ready"}:
            raise ValueError("target must be 'materialized' or 'startup_ready'")
        if timeout is not None and (
            isinstance(timeout, bool)
            or not isinstance(timeout, (int, float))
            or not math.isfinite(timeout)
            or timeout < 0
        ):
            raise ValueError("timeout must be a non-negative finite number or None")
        params: dict[str, Any] = {"target": target}
        if self._rpc_admission_closed():
            raise NotConnectedError("runtime not started — no transport available")
        server_wait_seconds = (
            self._transport.request_timeout if timeout is None else timeout
        )
        # Always make the server-side deadline explicit. Otherwise ``None``
        # inherits the gateway handler's shorter generic RPC timeout while the
        # SDK waits against its transport default, silently truncating the
        # readiness barrier.
        params["timeout_ms"] = int(server_wait_seconds * 1000)
        raw = await self._rpc(
            "meta/wait_identity_bootstrap",
            params,
            transport_timeout=(
                server_wait_seconds
                + _IDENTITY_BOOTSTRAP_WAIT_TRANSPORT_HEADROOM_SECONDS
            ),
        )
        return IdentityBootstrapStatus.from_dict(raw)

    async def delete_identity(self, identity: str) -> Any:
        """Start the identity over: its earlier session is no longer resumed,
        and its next session starts empty.

        A live identity moves to a fresh session at once, without a wake. A
        retired identity stays retired and starts on a fresh session at its
        next wake. An identity with no history is left as it is, and an
        unlisted or retired identity is never made live. An identity a host
        intent declaration manages is refused with
        :class:`~meerkat_meta.errors.IdentityHostAdoptedError` (-32053).
        """
        return await self._rpc("meta/delete_identity", {"identity": identity})

    async def wait_until_ready(
        self,
        identities: list[str],
        *,
        timeout: float = 60,
        poll_interval: float = 1.5,
    ) -> None:
        """Wait until all identities have completed their autonomous kickoff turn.

        Readiness is "at least one turn has completed" on the identity's
        completion cursor. The wait is event-driven: one server-side
        ``meta/wait_for_completion`` per identity (no baseline), which the
        gateway answers when the first completion is recorded. It never reads
        the member session and does not poll. An identity that has not
        materialized yet is simply waited for. ``timeout`` is the overall
        deadline.

        Only as compatibility, a gateway predating the server-side wait, or a
        live alias with no completion cursor, falls back to the old polling
        proxy (``poll_interval``): the cursor where one exists, else a
        non-None ``output_preview``, which under-reports an agent whose
        kickoff turn committed no assistant text.

        Raises ``TimeoutError`` naming the identities that did not become
        ready; an RPC error for an identity (an invalid identity) is raised as
        is.
        """
        import time
        deadline = time.monotonic() + timeout

        async def ready(identity: str) -> str | None:
            """``None`` once ready, else why not."""
            while True:
                remaining = deadline - time.monotonic()
                raw = await self._server_wait(
                    "meta/wait_for_completion", {"identity": identity}, remaining,
                )
                if raw is None or raw.get("outcome") == "untracked":
                    ready_now = await self._poll_ready_compat(
                        identity, deadline, poll_interval,
                    )
                    return None if ready_now else "timed_out"
                outcome = raw.get("outcome")
                if outcome == "completed":
                    return None
                # A failed kickoff run is not readiness; a server deadline
                # shorter than ours (clamped) is re-issued. Both keep waiting
                # while the caller's deadline allows.
                if outcome in ("run_failed", "timed_out") and deadline - time.monotonic() > 0:
                    continue
                return str(outcome)

        results = await asyncio.gather(
            *(ready(identity) for identity in identities), return_exceptions=True
        )
        for result in results:
            if isinstance(result, BaseException):
                raise result
        remaining = sorted(
            f"{identity} ({reason})" if reason != "timed_out" else identity
            for identity, reason in zip(identities, results)
            if reason is not None
        )
        if remaining:
            raise TimeoutError(
                f"identities did not become ready within {timeout}s: {remaining}"
            )

    async def _poll_ready_compat(
        self, identity: str, deadline: float, poll_interval: float
    ) -> bool:
        """Compatibility readiness poll for a gateway predating
        ``meta/wait_for_completion``, or a live alias with no cursor."""
        import time
        while True:
            try:
                inspection = await self.inspect_identity(identity)
                cursor = inspection.completion_cursor
                if (
                    cursor.turns > 0
                    if cursor is not None
                    else inspection.output_preview is not None
                ):
                    return True
            except Exception:
                pass
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                return False
            await asyncio.sleep(min(poll_interval, remaining))

    async def shutdown(self) -> None:
        """Stop the runtime and its gateway. Idempotent.

        RPC admission closes at once. When the gateway's bounded cleanup does
        not settle, the error is raised and the runtime keeps the transport: a
        later ``shutdown()`` or ``connect()`` stops the same child again.
        """
        async with self._lifecycle_lock:
            self._running = False
            shutdown_task = self._shutdown_task
            if shutdown_task is None:
                transport = self._transport
                # Close RPC admission before the potentially long process
                # drain. Every concurrent shutdown below observes and awaits
                # the same task; connect waits for it before replacing the
                # gateway. The transport stays this runtime's until its stop
                # succeeds: a later shutdown() rejoins a failed stop on the
                # same transport and child.
                if transport is not None:
                    shutdown_task = self._schedule_transport_stop(transport)
        if shutdown_task is not None:
            # A cancelled caller must not cancel the runtime-owned cleanup.
            await asyncio.shield(shutdown_task)

    @property
    def is_running(self) -> bool:
        """Whether the runtime is connected and its gateway child, when it
        owns one, can still take requests.

        A runtime without a gateway (session-builder only) runs on its
        connected state alone. After the gateway child ended, ``connect()``
        alone does not replace it: call ``shutdown()``, then ``connect()``,
        which bootstraps a new child with the configured providers and init.
        """
        if not self._running:
            return False
        lifetime = getattr(self, "_gateway_lifetime", None)
        return lifetime is None or lifetime.error is None

    @property
    def gateway_ended(self) -> GatewayChildEndedError | None:
        """The bootstrapped gateway child's end, or ``None``.

        ``None`` while that child can take requests and for a runtime that
        never started one. Kept after ``shutdown()``; ``exit_code`` is
        ``None`` unless the process exit was observed.
        """
        lifetime = getattr(self, "_gateway_lifetime", None)
        return None if lifetime is None else lifetime.error

    @property
    def last_gateway_termination(self) -> TransportTermination | None:
        """What the last stop of this runtime's gateway transport observed.

        See ``PersistentTransport.last_termination``. Kept after
        ``shutdown()``; ``None`` while that gateway runs, before any stop, or
        without a gateway. An exit 0 without escalation is the process
        outcome only.
        """
        transport = self._transport
        if transport is None:
            transport = getattr(self, "_last_gateway_transport", None)
        return getattr(transport, "last_termination", None)

    @property
    def gateway_shutdown_horizon_ms(self) -> int | None:
        """The shutdown horizon this runtime's gateway advertised, as accepted.

        Milliseconds (see ``PersistentTransport.advertised_shutdown_horizon_ms``).
        A process that supervises this application waits at least this long,
        plus the transport's own TERM and KILL graces, before it kills the
        gateway. ``None`` without a running gateway or when the gateway
        advertised no valid horizon.
        """
        return getattr(self._transport, "advertised_shutdown_horizon_ms", None)

    async def wait_gateway_ended(
        self, timeout: float | None = None
    ) -> GatewayChildEndedError | None:
        """Wait until the bootstrapped gateway child ends and return its record.

        Returns at once with ``None`` for a runtime without a gateway child,
        and ``None`` on timeout. Cancelling the wait affects nothing else.
        """
        lifetime = getattr(self, "_gateway_lifetime", None)
        if lifetime is None:
            return None
        loop = asyncio.get_running_loop()
        deadline = None if timeout is None else loop.time() + timeout
        while True:
            ended = lifetime.error
            if ended is not None:
                return ended
            if deadline is not None and loop.time() >= deadline:
                return None
            await asyncio.sleep(0.05)


class _RuntimeGatewayCleanup:
    """A runtime's gateway cleanup, for a caller that has no runtime."""

    def __init__(self, runtime: MetaRuntime):
        self._runtime = runtime

    async def shutdown(self) -> None:
        """Retry the retained gateway's stop: the runtime's ``shutdown()``."""
        await self._runtime.shutdown()

    @property
    def last_termination(self) -> TransportTermination | None:
        """The runtime's ``last_gateway_termination``."""
        return self._runtime.last_gateway_termination


def _stop_failure(task: asyncio.Task[Any]) -> BaseException | None:
    """The stop task's own failure, exactly as raised; ``None`` while it
    runs, once it was cancelled, or when it succeeded."""
    if not task.done() or task.cancelled():
        return None
    return task.exception()


def _stop_succeeded(task: asyncio.Task[Any]) -> bool:
    return task.done() and not task.cancelled() and task.exception() is None


def _attach_gateway_cleanup(error: BaseException, cleanup: _RuntimeGatewayCleanup) -> None:
    """Hand the runtime's cleanup to the exception that propagates, keeping
    its identity, type and chain.

    An exception that refuses the attribute (its ``__setattr__`` raises)
    propagates unchanged without it; the refusal is logged, and the runtime
    still keeps the gateway transport.
    """
    try:
        error.gateway_cleanup = cleanup  # type: ignore[attr-defined]
    except BaseException:
        # Whatever the refusal raised, it must never replace the exception
        # that is propagating.
        _log.warning(
            "could not attach gateway_cleanup to a propagating %s; the runtime "
            "keeps the gateway transport, unreachable from this exception",
            type(error).__name__,
            exc_info=True,
        )


def _log_unobserved_stop_failure(task: asyncio.Task[Any]) -> None:
    # The caller that waited for this stop was cancelled: its failure would
    # otherwise reach nobody. The runtime keeps the transport either way.
    failure = _stop_failure(task)
    if failure is not None:
        _log.error(
            "gateway stop failed after its caller was cancelled; the runtime "
            "keeps the transport",
            exc_info=failure,
        )


class IdentityAgentHandle:
    """Identity-scoped agent handle for delivery, lifecycle, and observation."""

    def __init__(self, runtime: MetaRuntime, identity: str):
        self._runtime = runtime
        self._identity = identity

    @property
    def identity(self) -> str:
        return self._identity

    async def send(
        self,
        content: Any,
        *,
        track_turn: bool = False,
        expected_spec_digest: str | None = None,
        idempotency_key: str | None = None,
        handling_mode: str | None = None,
    ) -> Any:
        """Send conversational content (Addressable only; ``idempotency_key``
        and ``handling_mode`` as for :meth:`MetaRuntime.send`)."""
        return await self._runtime.send(
            self._identity,
            content,
            track_turn=track_turn,
            expected_spec_digest=expected_spec_digest,
            idempotency_key=idempotency_key,
            handling_mode=handling_mode,
        )

    async def send_status(self, session_id: str, input_id: str) -> Any:
        """An input's state in a session this identity has had (see
        :meth:`MetaRuntime.send_status`)."""
        return await self._runtime.send_status(self._identity, session_id, input_id)

    async def registered_spec(self) -> RegisteredSpec:
        """This identity's registered row (see :meth:`MetaRuntime.registered_spec`)."""
        return await self._runtime.registered_spec(self._identity)

    async def dispatch(
        self,
        dispatch_input: Any,
        *,
        track_turn: bool = False,
        expected_scope: Any = None,
    ) -> Any:
        """Dispatch with a DispatchInput object (``expected_scope`` as for
        :meth:`MetaRuntime.dispatch`)."""
        return await self._runtime.dispatch(
            self._identity,
            dispatch_input,
            track_turn=track_turn,
            expected_scope=expected_scope,
        )

    async def dispatch_text(
        self,
        text: str,
        *,
        origin: str = "system",
        correlation_id: str | None = None,
        idempotency_key: str | None = None,
        track_turn: bool = False,
        expected_scope: Any = None,
    ) -> Any:
        """Dispatch plain text without constructing DispatchInput."""
        return await self._runtime.dispatch_text(
            self._identity,
            text,
            origin=origin,
            correlation_id=correlation_id,
            idempotency_key=idempotency_key,
            track_turn=track_turn,
            expected_scope=expected_scope,
        )

    async def recover_delivery(
        self,
        scope: Any,
        *,
        idempotency_key: str,
        correlation_id: str,
        timeout: float | None = None,
    ) -> Any:
        """Recover one scoped delivery of this identity (see
        :meth:`MetaRuntime.recover_delivery`)."""
        return await self._runtime.recover_delivery(
            self._identity,
            scope,
            idempotency_key=idempotency_key,
            correlation_id=correlation_id,
            timeout=timeout,
        )

    async def turn_result(self, ticket: str) -> Any:
        """Read the ticketed turn ``ticket`` of this identity."""
        return await self._runtime.turn_result(self._identity, ticket)

    async def wait_for_turn(
        self,
        ticket: str,
        *,
        timeout: float = 90,
        poll_interval: float = 0.5,
    ) -> Any:
        """Wait for the turn ``ticket`` names and return ITS typed result.

        ``ticket`` is the ``turn_ticket`` of a send/dispatch made with
        ``track_turn=True``. Unlike :meth:`wait_for_completion`, no other
        delivery to this identity (a peer message, a scheduled turn, a fork
        completion wake) can satisfy this wait, and the output is the turn's
        own, from its committed run result.

        Returns the completed :class:`~meerkat_meta.TurnResult`. Read
        :attr:`~meerkat_meta.TurnResult.output_status` before the text: only
        ``TEXT`` carries ``output`` (``output_truncated`` says whether it was
        cut); ``EMPTY`` means the turn committed no text; ``NO_OWN_RESULT``
        means the runtime folded the input into a run already in progress or
        deduplicated it onto an earlier admission; ``UNAVAILABLE`` means the
        gateway cannot report per-turn output.

        Raises :class:`~meerkat_meta.errors.TurnFailedError` when the turn
        failed, :class:`~meerkat_meta.errors.TurnUnknownError` when the
        gateway knows no such turn for this identity, and ``TimeoutError``.

        Event-driven: one server-side ``meta/wait_for_turn``, which the
        gateway answers when the ticket settles. ``timeout`` is the deadline.
        ``poll_interval`` applies only against a gateway predating that
        method, which is polled with ``meta/turn_result`` instead.
        """
        import time
        from .errors import WaitEndedError
        from .identity_first_models import TurnResult
        deadline = time.monotonic() + timeout
        while True:
            raw = await self._runtime._server_wait(
                "meta/wait_for_turn",
                {"identity": self._identity, "ticket": ticket},
                deadline - time.monotonic(),
            )
            if raw is None:
                return await self._poll_turn_compat(
                    ticket, timeout=timeout, poll_interval=poll_interval
                )
            wait = raw.get("wait")
            if wait == "shutting_down" and raw.get("state") == "pending":
                raise WaitEndedError(self._identity, "shutting_down", f"turn {ticket}")
            # A server deadline shorter than ours (clamped) is re-issued.
            if (
                wait == "timed_out"
                and raw.get("state") == "pending"
                and deadline - time.monotonic() > 0
            ):
                continue
            return self._settled_turn(TurnResult.from_dict(raw), ticket, timeout)

    def _settled_turn(self, result: Any, ticket: str, timeout: float) -> Any:
        from .errors import TurnFailedError, TurnUnknownError
        from .identity_first_models import TurnState
        if result.state is TurnState.COMPLETED:
            return result
        if result.state is TurnState.FAILED:
            raise TurnFailedError(self._identity, ticket, result.error or "")
        if result.state is TurnState.UNKNOWN:
            raise TurnUnknownError(self._identity, ticket)
        raise TimeoutError(
            f"turn {ticket} of identity {self._identity!r} did not "
            f"complete within {timeout}s"
        )

    async def _poll_turn_compat(
        self, ticket: str, *, timeout: float, poll_interval: float
    ) -> Any:
        """Compatibility wait for a gateway predating ``meta/wait_for_turn``."""
        import time
        from .identity_first_models import TurnState
        deadline = time.monotonic() + timeout
        while True:
            result = await self.turn_result(ticket)
            remaining = deadline - time.monotonic()
            if result.state is not TurnState.PENDING or remaining <= 0:
                return self._settled_turn(result, ticket, timeout)
            await asyncio.sleep(min(poll_interval, remaining))

    async def status(self) -> Any:
        """Return IdentityStatus."""
        return await self._runtime.status(self._identity)

    async def inspect(self) -> Any:
        """Return execution-level inspection (output_preview, peers)."""
        return await self._runtime.inspect_identity(self._identity)

    async def completion_cursor(self) -> CompletionCursor | None:
        """Read this identity's completion cursor without touching its session
        (see :meth:`MetaRuntime.completion_cursor`)."""
        return await self._runtime.completion_cursor(self._identity)

    async def wait_until_ready(self, *, timeout: float = 60) -> None:
        """Wait until this identity's autonomous kickoff turn has completed."""
        await self._runtime.wait_until_ready(
            [self._identity], timeout=timeout,
        )

    async def wait_for_completion(
        self,
        baseline: CompletionCursor,
        *,
        timeout: float = 90,
        poll_interval: float = 0.5,
    ) -> str | None:
        """Wait until a turn completes past ``baseline``.

        ``baseline`` is the ``completion_baseline`` returned by :meth:`send`,
        :meth:`dispatch`, or :meth:`dispatch_text`. This compares cursors, so
        two consecutive turns emitting byte-identical text are still two
        distinct completions.

        IDENTITY-WIDE: any completion on this identity after ``baseline``
        satisfies the wait (a peer message, a scheduled turn, a fork
        completion wake), and the returned preview is the session's latest
        output, not necessarily your turn's. To wait for one specific turn,
        use :meth:`wait_for_turn` with a ticket (``track_turn=True``), as
        :meth:`send_and_wait` does.

        Do not mix ticket waits and cursor waits on one identity: a ticket can
        settle before the identity health monitor counts the same turn on the cursor,
        so a later ``wait_for_output(after=baseline)`` or ``wait_for_completion``
        can be satisfied by that earlier turn. Cursor waits stay identity-wide.

        Event-driven: one server-side ``meta/wait_for_completion``, which
        the gateway answers when a completion is recorded (it never reads the
        member session and does not poll); the output is then read once.
        ``timeout`` is the deadline. ``poll_interval`` applies only against a
        gateway predating that method, which is polled instead.

        Returns the ``output_preview`` read at completion (``None`` if the
        turn committed no assistant text).

        Raises ``TimeoutError`` if no turn completes in time, and
        ``RuntimeError`` if the identity's runtime incarnation changed — turn
        counts do not carry across incarnations, so that is reported rather
        than guessed at in either direction.
        """
        await self._await_cursor_past(baseline, timeout, poll_interval)
        return (await self.inspect()).output_preview

    async def _await_cursor_past(
        self, baseline: CompletionCursor, timeout: float, poll_interval: float
    ) -> CompletionCursor:
        """Wait server-side until this identity's cursor moves past
        ``baseline`` and return it; raise as :meth:`wait_for_completion`."""
        import time
        from .errors import WaitEndedError
        deadline = time.monotonic() + timeout
        while True:
            raw = await self._runtime._server_wait(
                "meta/wait_for_completion",
                {"identity": self._identity, "after": baseline.to_dict()},
                deadline - time.monotonic(),
            )
            if raw is None:
                return await self._poll_cursor_past_compat(baseline, timeout, poll_interval)
            cursor = _completion_cursor_from(raw, "completion_cursor")
            outcome = raw.get("outcome")
            if outcome == "completed" and cursor is not None:
                return cursor
            if cursor is None or outcome == "untracked":
                raise CompletionCursorUnavailableError(
                    self._identity, "this is a live alias with no identity authority"
                )
            if outcome == "incarnation_changed":
                raise RuntimeError(
                    f"completion baseline {baseline} for identity "
                    f"{self._identity!r} belongs to a superseded runtime "
                    f"incarnation (now {cursor}); capture a fresh baseline"
                )
            if outcome == "timed_out":
                # A server deadline shorter than ours (clamped) is re-issued.
                if deadline - time.monotonic() > 0:
                    continue
                raise TimeoutError(
                    f"identity {self._identity!r} did not complete a turn past "
                    f"{baseline} within {timeout}s"
                )
            raise WaitEndedError(self._identity, str(outcome), f"baseline {baseline}")

    async def _poll_cursor_past_compat(
        self, baseline: CompletionCursor, timeout: float, poll_interval: float
    ) -> CompletionCursor:
        """Compatibility wait for a gateway predating
        ``meta/wait_for_completion``."""
        import time
        deadline = time.monotonic() + timeout
        while True:
            cursor = await self.completion_cursor()
            if cursor is None:
                raise CompletionCursorUnavailableError(
                    self._identity,
                    "the gateway predates the completion contract or this is a "
                    "live alias with no identity authority",
                )
            progress = cursor.progress_since(baseline)
            if progress is CompletionProgress.COMPLETED:
                return cursor
            if progress is CompletionProgress.INCARNATION_CHANGED:
                raise RuntimeError(
                    f"completion baseline {baseline} for identity "
                    f"{self._identity!r} belongs to a superseded runtime "
                    f"incarnation (now {cursor}); capture a fresh baseline"
                )
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise TimeoutError(
                    f"identity {self._identity!r} did not complete a turn past "
                    f"{baseline} within {timeout}s"
                )
            await asyncio.sleep(min(poll_interval, remaining))

    async def send_and_wait(
        self,
        content: Any,
        *,
        timeout: float = 90,
        poll_interval: float = 0.5,
        require_attribution: bool = False,
    ) -> str | None:
        """Send, then wait for the turn that send started and return its output.

        Returns the text of :meth:`send_and_wait_outcome`; use that method for
        the typed :class:`~meerkat_meta.AwaitedTurn`, which says whether the
        text is attributed to this send. The contract:

        - Tracked (a ``turn_driven`` member): the send carries a turn ticket
          and the wait is per-admission, so another delivery to this identity
          cannot satisfy it and the text is this turn's own committed output
          (``None`` when it committed none). Text cut at the gateway's bound
          emits :class:`~meerkat_meta.errors.TurnOutputTruncatedWarning`; a
          turn that completed without output of its own returns ``None`` with
          :class:`~meerkat_meta.errors.TurnOutputUnavailableWarning`.
        - Untracked (an ``autonomous_host`` member, the default mode; an
          externally bound member; host human input; a bridge that cannot
          report output; ``runtime_refused``; ``session_rotated``; a gateway
          predating turn tickets): the call waits on the identity-wide
          completion cursor, as before, and emits
          :class:`~meerkat_meta.errors.TurnTrackingUnavailableWarning`
          naming the ``turn_unavailable`` code. That text is NOT attributed:
          another delivery's turn may have produced it
          (``AwaitedTurn.attributed`` is ``False`` and
          ``AwaitedTurn.untracked_code`` carries the code).
        - ``require_attribution=True`` raises
          :class:`~meerkat_meta.errors.TurnTrackingUnavailableError` in
          every untracked case instead of waiting identity-wide.
        - ``not_delivered`` (nothing ran) raises
          :class:`~meerkat_meta.errors.TurnNotDeliveredError`.

        The send happens exactly once; nothing here ever resends. Every
        failure after admission carries the send result as ``admission``:
        a transport or RPC failure of the exact-ticket wait is retried, for
        that ticket only, until ``timeout`` and then raises
        :class:`~meerkat_meta.errors.PostAdmissionObservationError` (with
        ``ticket``); ``TurnFailedError`` / ``TurnUnknownError`` /
        ``WaitEndedError`` carry it; a turn still pending at ``timeout``
        raises :class:`~meerkat_meta.errors.TurnWaitTimeoutError` (a
        ``TimeoutError``).
        """
        return (await self._send_outcome(
            content, timeout=timeout, poll_interval=poll_interval,
            require_attribution=require_attribution,
        )).text

    async def send_and_wait_outcome(
        self,
        content: Any,
        *,
        timeout: float = 90,
        poll_interval: float = 0.5,
        require_attribution: bool = False,
    ) -> Any:
        """:meth:`send_and_wait`, returning the typed
        :class:`~meerkat_meta.AwaitedTurn` (``text``, ``attributed``,
        ``admission``, ``ticket``, ``output_status``, ``untracked_code``)."""
        return await self._send_outcome(
            content, timeout=timeout, poll_interval=poll_interval,
            require_attribution=require_attribution,
        )

    async def dispatch_and_wait(
        self,
        dispatch_input: Any,
        *,
        timeout: float = 90,
        poll_interval: float = 0.5,
        require_attribution: bool = False,
    ) -> str | None:
        """Dispatch, then wait for the turn it started and return its output
        (the contract of :meth:`send_and_wait`; the typed form is
        :meth:`dispatch_and_wait_outcome`)."""
        return (await self._dispatch_outcome(
            dispatch_input, timeout=timeout, poll_interval=poll_interval,
            require_attribution=require_attribution,
        )).text

    async def dispatch_and_wait_outcome(
        self,
        dispatch_input: Any,
        *,
        timeout: float = 90,
        poll_interval: float = 0.5,
        require_attribution: bool = False,
    ) -> Any:
        """:meth:`dispatch_and_wait`, returning the typed
        :class:`~meerkat_meta.AwaitedTurn`."""
        return await self._dispatch_outcome(
            dispatch_input, timeout=timeout, poll_interval=poll_interval,
            require_attribution=require_attribution,
        )

    async def dispatch_text_and_wait(
        self,
        text: str,
        *,
        origin: str = "system",
        correlation_id: str | None = None,
        idempotency_key: str | None = None,
        timeout: float = 90,
        poll_interval: float = 0.5,
        require_attribution: bool = False,
    ) -> str | None:
        """:meth:`dispatch_text` plus the wait, in one call (the contract of
        :meth:`send_and_wait`; the typed form is
        :meth:`dispatch_text_and_wait_outcome`)."""
        return (await self._dispatch_text_outcome(
            text, origin=origin, correlation_id=correlation_id,
            idempotency_key=idempotency_key, timeout=timeout,
            poll_interval=poll_interval,
            require_attribution=require_attribution,
        )).text

    async def dispatch_text_and_wait_outcome(
        self,
        text: str,
        *,
        origin: str = "system",
        correlation_id: str | None = None,
        idempotency_key: str | None = None,
        timeout: float = 90,
        poll_interval: float = 0.5,
        require_attribution: bool = False,
    ) -> Any:
        """:meth:`dispatch_text_and_wait`, returning the typed
        :class:`~meerkat_meta.AwaitedTurn`."""
        return await self._dispatch_text_outcome(
            text, origin=origin, correlation_id=correlation_id,
            idempotency_key=idempotency_key, timeout=timeout,
            poll_interval=poll_interval,
            require_attribution=require_attribution,
        )

    # Each public method reaches the warning through exactly one of these
    # helpers, so the warnings point at the caller's line.

    async def _send_outcome(self, content: Any, **wait: Any) -> Any:
        result = await self.send(content, track_turn=True)
        return await self._await_admitted(result, "send", **wait)

    async def _dispatch_outcome(self, dispatch_input: Any, **wait: Any) -> Any:
        result = await self.dispatch(dispatch_input, track_turn=True)
        return await self._await_admitted(result, "dispatch", **wait)

    async def _dispatch_text_outcome(
        self,
        text: str,
        *,
        origin: str,
        correlation_id: str | None,
        idempotency_key: str | None,
        **wait: Any,
    ) -> Any:
        result = await self.dispatch_text(
            text, origin=origin, correlation_id=correlation_id,
            idempotency_key=idempotency_key, track_turn=True,
        )
        return await self._await_admitted(result, "dispatch", **wait)

    async def _await_admitted(
        self,
        result: Any,
        operation: str,
        *,
        timeout: float,
        poll_interval: float,
        require_attribution: bool,
    ) -> Any:
        """Wait for an admitted delivery and say whether the result is its own.

        Everything raised here happens after admission, so every exception
        (including a warning a caller escalated with
        ``filterwarnings("error")``) keeps its type and carries the admission
        and ticket."""
        try:
            return await self._await_admitted_inner(
                result, operation, timeout=timeout, poll_interval=poll_interval,
                require_attribution=require_attribution,
            )
        except BaseException as failure:
            _attach_admission(failure, result, getattr(result, "turn_ticket", None))
            raise

    async def _await_admitted_inner(
        self,
        result: Any,
        operation: str,
        *,
        timeout: float,
        poll_interval: float,
        require_attribution: bool,
    ) -> Any:
        """Warnings raised here are attributed five frames up: this method,
        :meth:`_await_admitted`, the ``_*_outcome`` helper, the public method,
        then the caller."""
        from .identity_first_models import AwaitedTurn
        ticket = getattr(result, "turn_ticket", None)
        if ticket is not None:
            turn = await self._observe_admitted_turn(
                result, ticket, operation, timeout=timeout, poll_interval=poll_interval,
            )
            return AwaitedTurn(
                text=self._text_of_turn(turn, operation, stacklevel=6),
                attributed=True,
                admission=result,
                ticket=ticket,
                output_status=turn.output_status,
            )
        from .errors import (
            TurnNotDeliveredError,
            TurnTrackingUnavailableError,
            TurnTrackingUnavailableWarning,
        )
        unavailable = getattr(result, "turn_unavailable", None)
        if unavailable is not None and not unavailable.delivered:
            refused = TurnNotDeliveredError(
                self._identity, operation, unavailable.code, unavailable.reason,
            )
            refused.admission = result
            raise refused
        code = unavailable.code if unavailable is not None else None
        reason = unavailable.reason if unavailable is not None else None
        if require_attribution:
            raise TurnTrackingUnavailableError(
                self._identity, operation, result, code, reason,
            )
        detail = (
            f"{code}: {reason}"
            if code is not None
            else "the gateway returned no turn ticket (it predates turn tickets)"
        )
        warnings.warn(
            f"{operation} for identity {self._identity!r} could not track its "
            f"own turn ({detail}); waiting on the identity-wide completion "
            "cursor instead, which another delivery's completion can also "
            "satisfy, so the result is not attributed to this delivery",
            TurnTrackingUnavailableWarning,
            stacklevel=5,
        )
        from .errors import PostAdmissionObservationError, TurnWaitTimeoutError
        try:
            text = await self._wait_for_admission(
                result, operation, timeout=timeout, poll_interval=poll_interval,
            )
        # RPC and transport failures are observation failures. The cursor
        # path's own errors (a wait that ended, no baseline, a superseded
        # incarnation, a live alias, the persistent transport's plain
        # RuntimeError on a response timeout or closed stdout) and caller
        # cancellation keep their type and semantics; each still carries the
        # admission, with no ticket, so the caller never redispatches.
        except (RpcError, TransportError, NotConnectedError) as err:
            raise PostAdmissionObservationError(
                self._identity, operation, result, None, attempts=1,
            ) from err
        except TimeoutError as expired:
            raise TurnWaitTimeoutError(
                self._identity, None, timeout, result,
            ) from expired
        except BaseException as failure:
            _attach_admission(failure, result, None)
            raise
        return AwaitedTurn(
            text=text, attributed=False, admission=result, untracked_code=code,
        )

    async def _observe_admitted_turn(
        self,
        result: Any,
        ticket: str,
        operation: str,
        *,
        timeout: float,
        poll_interval: float,
    ) -> Any:
        """Wait for an admitted delivery's own turn, by its ticket only.

        A transport or RPC failure of the wait retries the same exact-ticket
        observation, with bounded backoff, until the caller's deadline; the
        delivery is never repeated. The turn's typed outcomes (failed,
        unknown, wait ended, still pending at the deadline) are raised with
        the admission result attached."""
        deadline = time.monotonic() + timeout
        try:
            return await self._observe_admitted_turn_loop(
                result, ticket, operation, deadline, timeout, poll_interval,
            )
        # Caller cancellation (during the wait or a retry backoff) and any
        # unexpected failure keep their type and propagate unchanged, with
        # the admission and ticket attached. Typed outcomes already carry it.
        except BaseException as failure:
            _attach_admission(failure, result, ticket)
            raise

    async def _observe_admitted_turn_loop(
        self,
        result: Any,
        ticket: str,
        operation: str,
        deadline: float,
        timeout: float,
        poll_interval: float,
    ) -> Any:
        from .errors import (
            PostAdmissionObservationError,
            TurnFailedError,
            TurnUnknownError,
            TurnWaitTimeoutError,
            WaitEndedError,
        )
        attempts = 0
        while True:
            attempts += 1
            try:
                return await self.wait_for_turn(
                    ticket,
                    timeout=max(0.0, deadline - time.monotonic()),
                    poll_interval=poll_interval,
                )
            except (TurnFailedError, TurnUnknownError, WaitEndedError) as outcome:
                outcome.admission = result
                raise
            except TimeoutError as expired:
                raise TurnWaitTimeoutError(
                    self._identity, ticket, timeout, result,
                ) from expired
            except _OBSERVATION_FAILURES as failure:
                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    raise PostAdmissionObservationError(
                        self._identity, operation, result, ticket, attempts=attempts,
                    ) from failure
                _log.debug(
                    "%s for identity %r: observing admitted turn %s failed "
                    "(attempt %d), retrying the exact-ticket wait: %s",
                    operation, self._identity, ticket, attempts, failure,
                )
                backoff = min(
                    _OBSERVATION_RETRY_INITIAL_SECONDS * (2 ** min(attempts - 1, 5)),
                    _OBSERVATION_RETRY_MAX_SECONDS,
                    remaining,
                )
                await asyncio.sleep(backoff)

    def _text_of_turn(
        self, turn: Any, operation: str, *, stacklevel: int = 4,
    ) -> str | None:
        """The ``*_and_wait`` return value for a completed turn: its text, or
        ``None`` when it committed none. Truncated text and "no output of its
        own" are never returned silently: each emits a typed warning."""
        from .errors import TurnOutputTruncatedWarning, TurnOutputUnavailableWarning
        from .identity_first_models import TurnOutputStatus
        if turn.output_status is TurnOutputStatus.TEXT:
            if turn.output_truncated:
                warnings.warn(
                    f"{operation} for identity {self._identity!r}: turn "
                    f"{turn.ticket} returned text cut at the gateway's bound",
                    TurnOutputTruncatedWarning,
                    stacklevel=stacklevel,
                )
            return turn.output
        if turn.output_status is TurnOutputStatus.EMPTY:
            return None
        warnings.warn(
            f"{operation} for identity {self._identity!r}: turn {turn.ticket} "
            f"completed without output of its own "
            f"({turn.output_status.value if turn.output_status else 'unknown'}); "
            "returning None",
            TurnOutputUnavailableWarning,
            stacklevel=stacklevel,
        )
        return None

    async def _wait_for_admission(
        self,
        result: Any,
        operation: str,
        *,
        timeout: float,
        poll_interval: float,
    ) -> str | None:
        baseline = getattr(result, "completion_baseline", None)
        if baseline is None:
            raise RuntimeError(
                f"{operation} returned no completion_baseline for identity "
                f"{self._identity!r}; the gateway predates the completion "
                "contract, so the turn cannot be awaited without an unsound "
                "text comparison"
            )
        return await self.wait_for_completion(
            baseline, timeout=timeout, poll_interval=poll_interval,
        )

    async def wait_for_output(
        self,
        *,
        timeout: float = 90,
        poll_interval: float = 1.5,
        after: CompletionCursor | None = None,
        baseline: str | None = None,
        turn: str | None = None,
    ) -> str:
        """Wait until this identity produces an output_preview.

        No client-side polling: the member is read, then the call waits
        server-side (``meta/wait_for_completion``) for the next completion
        past the cursor that read saw, and reads again. Output only changes
        when a turn commits, so nothing is missed between reads.
        ``poll_interval`` applies only to gateways that predate the
        server-side wait.

        Pass ``turn`` (the ``turn_ticket`` of a send/dispatch made with
        ``track_turn=True``) to wait for that exact turn and return ITS
        output; this is the per-admission path. It raises
        :class:`~meerkat_meta.errors.TurnOutputUnavailableError` (with the
        typed ``status``) when that turn completed without text of its own,
        and warns :class:`~meerkat_meta.errors.TurnOutputTruncatedWarning`
        when the text was cut.

        Pass ``after`` (a :class:`CompletionCursor` from a send/dispatch
        result) to wait past a baseline, equivalent to
        :meth:`wait_for_completion` except that it also requires non-empty
        output. That is IDENTITY-WIDE: another delivery's completion also
        satisfies it, and the output is the session's latest.
        Do not mix ticket waits and cursor waits on one identity: a ticket can
        settle before the identity health monitor counts the same turn on the cursor,
        so a later ``wait_for_output(after=baseline)`` or ``wait_for_completion``
        can be satisfied by that earlier turn. Cursor waits stay identity-wide.

        ``baseline`` (a str) is DEPRECATED and UNSOUND: it waits until
        ``output_preview`` differs from the text you passed, so two
        consecutive turns that legitimately produce identical output are
        indistinguishable from no turn at all, and the call sleeps out its
        entire timeout. Use ``after``, :meth:`wait_for_completion`, or
        :meth:`send_and_wait` instead.

        With neither argument this returns as soon as the identity has ANY
        committed output — including output from an earlier turn.

        Raises TimeoutError if timeout expires.
        """
        import time
        if turn is not None:
            if after is not None or baseline is not None:
                raise ValueError("pass 'turn' alone, not with 'after' or 'baseline'")
            from .errors import TurnOutputUnavailableError
            from .identity_first_models import TurnOutputStatus
            result = await self.wait_for_turn(
                turn, timeout=timeout, poll_interval=poll_interval,
            )
            if result.output_status is not TurnOutputStatus.TEXT or not result.output:
                raise TurnOutputUnavailableError(
                    self._identity,
                    turn,
                    result.output_status.value if result.output_status else "unknown",
                )
            return self._text_of_turn(result, "wait_for_output") or result.output
        if baseline is not None:
            if after is not None:
                raise ValueError("pass either 'after' or the deprecated 'baseline', not both")
            warnings.warn(
                "wait_for_output(baseline=<text>) is unsound: consecutive turns "
                "may produce identical output, in which case it never fires and "
                "sleeps out the full timeout. Use wait_for_output(after=<cursor>) "
                "or send_and_wait()/dispatch_and_wait().",
                DeprecationWarning,
                stacklevel=2,
            )
        deadline = time.monotonic() + timeout
        if baseline is not None:
            def accept(preview: str) -> bool:
                return preview != baseline
        else:
            def accept(preview: str) -> bool:
                return True
        output = await self._wait_for_output_where(
            accept, deadline=deadline, poll_interval=poll_interval, after=after
        )
        if output is None:
            raise TimeoutError(
                f"identity {self._identity!r} did not produce output within {timeout}s"
            )
        return output

    async def wait_for_output_containing(
        self,
        needle: str,
        *,
        timeout: float = 90,
        poll_interval: float = 1.5,
    ) -> str:
        """Wait until output_preview contains the given substring.

        Event-driven, as :meth:`wait_for_output`: each completion is waited
        for server-side and the member is read once per completion.
        ``poll_interval`` applies only to gateways that predate the
        server-side wait.
        """
        import time
        deadline = time.monotonic() + timeout
        output = await self._wait_for_output_where(
            lambda preview: needle in preview,
            deadline=deadline,
            poll_interval=poll_interval,
        )
        if output is None:
            raise TimeoutError(
                f"identity {self._identity!r} did not produce output "
                f"containing {needle!r} within {timeout}s"
            )
        return output

    async def _wait_for_output_where(
        self,
        accept: Callable[[str], bool],
        *,
        deadline: float,
        poll_interval: float,
        after: CompletionCursor | None = None,
    ) -> str | None:
        """Read the member's output whenever it can have changed: now
        (unless ``after`` is given), then once per completion, each waited
        for server-side. Returns the first accepted output, or ``None`` at
        ``deadline``."""
        import time
        cursor = after
        if cursor is None:
            inspection = await self.inspect()
            if inspection.output_preview and accept(inspection.output_preview):
                return inspection.output_preview
            cursor = inspection.completion_cursor
            if cursor is None:
                raise CompletionCursorUnavailableError(
                    self._identity,
                    "this is a live alias with no identity authority, so there "
                    "is no completion to wait for",
                )
        while True:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                return None
            try:
                cursor = await self._await_cursor_past(cursor, remaining, poll_interval)
            except TimeoutError:
                return None
            inspection = await self.inspect()
            if inspection.output_preview and accept(inspection.output_preview):
                return inspection.output_preview
            observed = inspection.completion_cursor
            if (
                observed is not None
                and observed.progress_since(cursor) is CompletionProgress.COMPLETED
            ):
                # The read already reflects a later turn; wait past that one.
                cursor = observed

    async def subscribe(self) -> Any:
        return await self._runtime.subscribe(self._identity)

    async def respawn(self) -> Any:
        return await self._runtime.respawn(self._identity)

    async def retire(self) -> Any:
        return await self._runtime.retire(self._identity)

    async def reset(self) -> Any:
        return await self._runtime.reset(self._identity)

    async def delete_identity(self) -> Any:
        return await self._runtime.delete_identity(self._identity)


def _require_active_live_handle(
    handle: ActiveLiveChannelHandle | object,
) -> ActiveLiveChannelHandle:
    if not isinstance(handle, ActiveLiveChannelHandle):
        raise TypeError("operation requires an active live channel handle")
    handle.to_dict()
    return handle


def _announcement_member(member: AnnouncementMember | dict[str, str]) -> dict[str, str]:
    """A member param as the gateway takes it: ``{"mob_id", "identity"}``."""
    if isinstance(member, AnnouncementMember):
        return member.to_dict()
    return dict(member)


class MobHandle:
    """Proxy for the Meerkat MobHandle API via JSON-RPC.

    Returns typed result objects instead of raw dicts.
    """

    def __init__(self, runtime: MetaRuntime):
        self._runtime = runtime

    async def status(self) -> StatusResult:
        """Return the current runtime status."""
        raw = await self._runtime._rpc("meta/status")
        return StatusResult.from_dict(raw)

    async def capabilities(self) -> CapabilitiesResult:
        """Return the runtime's advertised capabilities."""
        raw = await self._runtime._rpc("meta/capabilities")
        return CapabilitiesResult.from_dict(raw)

    async def spawn(self, spec: DiscoverySpec) -> SpawnResult:
        """Spawn a mob member from a full discovery spec."""
        raw = await self._runtime._rpc("meta/spawn_member", spec.to_dict())
        return SpawnResult.from_dict(raw)

    async def spawn_member(self, module_id: str) -> SpawnResult:
        """Spawn a mob member by module ID."""
        raw = await self._runtime._rpc("meta/spawn_member", {"module_id": module_id})
        return SpawnResult.from_dict(raw)

    async def reconcile(self, modules: list[str]) -> ReconcileResult:
        """Reconcile the mob to match the given module list."""
        raw = await self._runtime._rpc("meta/reconcile", {"modules": modules})
        return ReconcileResult.from_dict(raw)

    async def subscribe_events(
        self,
        scope: str = "mob",
        last_event_id: str | None = None,
        agent_id: str | None = None,
    ) -> SubscribeResult:
        """Subscribe to runtime events with an optional scope filter."""
        params: dict[str, Any] = {"scope": scope}
        if last_event_id is not None:
            params["last_event_id"] = last_event_id
        if agent_id is not None:
            params["agent_id"] = agent_id
        raw = await self._runtime._rpc("meta/events/subscribe", params)
        return SubscribeResult.from_dict(raw)

    async def resolve_routing(self, recipient: str, **kwargs: Any) -> RoutingResolution:
        """Resolve a routing target for the given recipient."""
        raw = await self._runtime._rpc(
            "meta/routing/resolve", {"recipient": recipient, **kwargs}
        )
        return RoutingResolution.from_dict(raw)

    async def send_delivery(self, **kwargs: Any) -> DeliveryResult:
        """Send a delivery payload through the routing layer."""
        raw = await self._runtime._rpc("meta/delivery/send", kwargs)
        return DeliveryResult.from_dict(raw)

    async def memory_query(
        self,
        query: str | dict[str, Any] | None = None,
        **kwargs: Any,
    ) -> MemoryQueryResult:
        """Query the assertion ledger by entity/topic/store filters.

        Passing a string sends it as ``query``; the stock runtime applies it
        as a case-insensitive substring filter across entity, topic, and fact
        (reason for conflict signals), after the exact ``entity``/``topic``/
        ``store`` filters.
        """
        if isinstance(query, dict):
            params = {**query, **kwargs}
        elif query is None:
            params = {**kwargs}
        elif isinstance(query, str):
            params = {"query": query, **kwargs}
        else:
            raise TypeError("memory_query query must be a string, dict, or None")
        raw = await self._runtime._rpc("meta/memory/query", params)
        return MemoryQueryResult.from_dict(raw)

    async def remember_agent_memory(
        self,
        identity: str,
        *,
        title: str,
        body: str,
        tags: list[str] | None = None,
        realm: str | None = None,
    ) -> AgentMemoryRecord:
        """Persist a durable memory record for an identity-scoped agent."""
        params: dict[str, Any] = {
            "identity": identity,
            "title": title,
            "body": body,
        }
        if realm is not None:
            params["realm"] = realm
        if tags is not None:
            params["tags"] = list(tags)
        raw = await self._runtime._rpc("meta/agent_memory/remember", params)
        return AgentMemoryRecord.from_dict(raw)

    async def recall_agent_memory(
        self,
        identity: str,
        *,
        realm: str | None = None,
        selection: str | None = None,
        query_text: str | None = None,
        query_terms: list[str] | None = None,
        max_entries: int | None = None,
    ) -> list[AgentMemoryRecord]:
        """Recall durable memory records for an identity-scoped agent."""
        params: dict[str, Any] = {"identity": identity}
        if realm is not None:
            params["realm"] = realm
        if selection is not None:
            params["selection"] = selection
        if query_text is not None:
            params["query_text"] = query_text
        if query_terms is not None:
            params["query_terms"] = list(query_terms)
        if max_entries is not None:
            params["max_entries"] = max_entries
        raw = await self._runtime._rpc("meta/agent_memory/recall", params)
        return list(AgentMemoryRecallResult.from_dict(raw).records)

    async def forget_agent_memory(
        self,
        identity: str,
        memory_id: str,
        *,
        realm: str | None = None,
    ) -> AgentMemoryForgetResult:
        """Delete a durable memory record for an identity-scoped agent."""
        params: dict[str, Any] = {"identity": identity, "memory_id": memory_id}
        if realm is not None:
            params["realm"] = realm
        raw = await self._runtime._rpc("meta/agent_memory/forget", params)
        return AgentMemoryForgetResult.from_dict(raw)

    async def update_agent_memory(
        self,
        identity: str,
        memory_id: str,
        *,
        title: str,
        body: str,
        tags: list[str] | None = None,
        realm: str | None = None,
    ) -> AgentMemoryUpdateResult:
        """Supersede a durable memory record within its lineage.

        The new title/body/tags become the active record; the prior record
        stays retrievable with provenance and is no longer recalled.
        """
        params: dict[str, Any] = {
            "identity": identity,
            "memory_id": memory_id,
            "title": title,
            "body": body,
        }
        if realm is not None:
            params["realm"] = realm
        if tags is not None:
            params["tags"] = list(tags)
        raw = await self._runtime._rpc("meta/agent_memory/update", params)
        return AgentMemoryUpdateResult.from_dict(raw)

    async def manifest_agent_memory(
        self,
        identity: str,
        *,
        realm: str | None = None,
        tier: str | None = None,
        k: int | None = None,
    ) -> list[AgentMemoryRecordMeta]:
        """List durable memory record metadata (id/kind/title/description/
        age/rank — never bodies).

        ``tier`` is ``"working_set"`` (default; top-K ranked plus the
        recent/unranked slice) or ``"full"``; ``k`` bounds the working set.
        """
        params: dict[str, Any] = {"identity": identity}
        if realm is not None:
            params["realm"] = realm
        if tier is not None:
            params["tier"] = tier
        if k is not None:
            params["k"] = k
        raw = await self._runtime._rpc("meta/agent_memory/manifest", params)
        return list(AgentMemoryManifestResult.from_dict(raw).records)

    async def call_tool(
        self, module_id: str, tool: str, arguments: dict[str, Any] | None = None
    ) -> CallToolResult:
        """Call an MCP tool on a loaded module."""
        params: dict[str, Any] = {"module_id": module_id, "tool": tool}
        if arguments:
            params["arguments"] = arguments
        raw = await self._runtime._rpc("meta/call_tool", params)
        return CallToolResult.from_dict(raw)

    async def models_catalog(self) -> ModelsCatalogResult:
        """Return the curated model catalog with provider defaults."""
        raw = await self._runtime._rpc("meta/models/catalog")
        return ModelsCatalogResult.from_dict(raw)

    async def schedule_tools(self) -> ScheduleToolsResult:
        """Return the definitions of Meerkat's durable schedule tools.

        Static: the gateway answers ``meta/schedule/tools`` from Meerkat's own
        definitions, without a schedule store or host. Whether this gateway
        attached the tools is the ``schedule`` slot of the ``storage``
        summary. Raises ``ValueError`` on a malformed result.
        """
        raw = await self._runtime._rpc("meta/schedule/tools")
        return ScheduleToolsResult.from_dict(raw)

    async def storage_doctor(
        self,
        state_dir: str | None = None,
        identity: str | None = None,
    ) -> Any:
        """Run the read-only storage doctor over a Meta state directory.

        Returns the raw wire result: ``state_dir``, ``diagnosis`` (with
        ``findings`` and ``inventory``), and ``storage`` (the live H1/H2
        durability summary, or ``None``). ``state_dir`` is required until the
        gateway can report its own persistent state directory (Phase M2);
        omitting it raises ``CapabilityUnavailableError``. ``identity``
        narrows the continuity checkpoint census to one identity.
        """
        params: dict[str, Any] = {}
        if state_dir is not None:
            params["state_dir"] = state_dir
        if identity is not None:
            params["identity"] = identity
        return await self._runtime._rpc("meta/storage/doctor", params)

    async def tools_catalog(self) -> MobpackToolsCatalogResult:
        """Return the Meta tool catalog used by mobpack authoring."""
        raw = await self._runtime._rpc("meta/tools/catalog")
        return MobpackToolsCatalogResult.from_dict(raw)

    async def skills_catalog(self) -> MobpackSkillsCatalogResult:
        """Return the Meta skill realms used by mobpack authoring."""
        raw = await self._runtime._rpc("meta/skills/catalog")
        return MobpackSkillsCatalogResult.from_dict(raw)

    async def agent_definitions(self) -> MobpackAgentDefinitionsResult:
        """Return Meta-owned agent definitions for the Agent Editor."""
        raw = await self._runtime._rpc("meta/agent_definitions/list")
        return MobpackAgentDefinitionsResult.from_dict(raw)

    async def mobpack_templates(self) -> MobpackTemplatesResult:
        """Return blank and sample mobpack templates."""
        raw = await self._runtime._rpc("meta/mobpacks/templates")
        return MobpackTemplatesResult.from_dict(raw)

    async def mobpack_catalogs(self) -> MobpackCatalogsResult:
        """Return the composed Meta mobpack authoring catalog snapshot."""
        raw = await self._runtime._rpc("meta/mobpacks/catalogs")
        return MobpackCatalogsResult.from_dict(raw)

    async def mobpack_validate(
        self,
        document: dict[str, Any],
        *,
        rkat_validate: bool | None = None,
    ) -> MobpackValidationResult:
        """Validate a mobpack authoring document."""
        params: dict[str, Any] = {"document": document}
        if rkat_validate is not None:
            params["rkat_validate"] = rkat_validate
        raw = await self._runtime._rpc("meta/mobpacks/validate", params)
        return MobpackValidationResult.from_dict(raw)

    async def mobpack_source(self, document: dict[str, Any]) -> MobpackSourceResult:
        """Render the deployable source files (mob.toml etc.) for a document."""
        raw = await self._runtime._rpc("meta/mobpacks/source", {"document": document})
        return MobpackSourceResult.from_dict(raw)

    async def mobpack_export(self, document: dict[str, Any]) -> MobpackExportResult:
        """Export a mobpack document as a base64-encoded archive."""
        raw = await self._runtime._rpc("meta/mobpacks/export", {"document": document})
        return MobpackExportResult.from_dict(raw)

    async def mobpack_import(
        self,
        *,
        mob_toml: str | None = None,
        content_base64: str | None = None,
        document: dict[str, Any] | None = None,
        source_name: str | None = None,
    ) -> MobpackImportResult:
        """Import a mob.toml, mobpack archive, or editor document."""
        params: dict[str, Any] = {}
        if mob_toml is not None:
            params["mob_toml"] = mob_toml
        if content_base64 is not None:
            params["content_base64"] = content_base64
        if document is not None:
            params["document"] = document
        if source_name is not None:
            params["source_name"] = source_name
        raw = await self._runtime._rpc("meta/mobpacks/import", params)
        return MobpackImportResult.from_dict(raw)

    async def mobpack_list(self) -> MobpackDraftListResult:
        """List mobpack draft registry rows."""
        raw = await self._runtime._rpc("meta/mobpacks/list", {})
        return MobpackDraftListResult.from_dict(raw)

    async def mobpack_get(self, draft_id: str) -> MobpackDraftGetResult:
        """Fetch a single mobpack draft registry row by id."""
        raw = await self._runtime._rpc("meta/mobpacks/get", {"id": draft_id})
        return MobpackDraftGetResult.from_dict(raw)

    async def mobpack_create(
        self,
        *,
        template: str | None = None,
        name: str | None = None,
        trigger: str | None = None,
    ) -> MobpackDraftSaveResult:
        """Create a new mobpack draft from a starter template."""
        params: dict[str, Any] = {}
        if template is not None:
            params["template"] = template
        if name is not None:
            params["name"] = name
        if trigger is not None:
            params["trigger"] = trigger
        raw = await self._runtime._rpc("meta/mobpacks/create", params)
        return MobpackDraftSaveResult.from_dict(raw)

    async def mobpack_save(
        self,
        draft_id: str,
        document: dict[str, Any],
        *,
        validation: dict[str, Any] | None = None,
        stage: str | None = None,
        expected_revision: int | None = None,
        expected_etag: str | None = None,
    ) -> MobpackDraftSaveResult:
        """Save a mobpack draft, optionally guarded by revision/etag."""
        params: dict[str, Any] = {"id": draft_id, "document": document}
        if validation is not None:
            params["validation"] = validation
        if stage is not None:
            params["stage"] = stage
        if expected_revision is not None:
            params["expected_revision"] = expected_revision
        if expected_etag is not None:
            params["expected_etag"] = expected_etag
        raw = await self._runtime._rpc("meta/mobpacks/save", params)
        return MobpackDraftSaveResult.from_dict(raw)

    async def mobpack_delete(
        self,
        draft_id: str,
        *,
        expected_revision: int | None = None,
    ) -> MobpackDraftDeleteResult:
        """Delete a mobpack draft, optionally guarded by revision."""
        params: dict[str, Any] = {"id": draft_id}
        if expected_revision is not None:
            params["expected_revision"] = expected_revision
        raw = await self._runtime._rpc("meta/mobpacks/delete", params)
        return MobpackDraftDeleteResult.from_dict(raw)

    async def mobpack_undo(
        self,
        draft_id: str,
        *,
        expected_revision: int | None = None,
        expected_etag: str | None = None,
    ) -> MobpackDraftHistoryResult:
        """Step a mobpack draft one entry back in its undo history."""
        params: dict[str, Any] = {"id": draft_id}
        if expected_revision is not None:
            params["expected_revision"] = expected_revision
        if expected_etag is not None:
            params["expected_etag"] = expected_etag
        raw = await self._runtime._rpc("meta/mobpacks/undo", params)
        return MobpackDraftHistoryResult.from_dict(raw)

    async def mobpack_redo(
        self,
        draft_id: str,
        *,
        expected_revision: int | None = None,
        expected_etag: str | None = None,
    ) -> MobpackDraftHistoryResult:
        """Step a mobpack draft one entry forward in its redo history."""
        params: dict[str, Any] = {"id": draft_id}
        if expected_revision is not None:
            params["expected_revision"] = expected_revision
        if expected_etag is not None:
            params["expected_etag"] = expected_etag
        raw = await self._runtime._rpc("meta/mobpacks/redo", params)
        return MobpackDraftHistoryResult.from_dict(raw)

    async def mobpack_apply_operation(
        self,
        document: dict[str, Any],
        operation: dict[str, Any],
        *,
        expected_catalog_snapshot_id: str | None = None,
    ) -> MobpackApplyOperationResult:
        """Apply a structured authoring operation to a mobpack document."""
        params: dict[str, Any] = {"document": document, "operation": operation}
        if expected_catalog_snapshot_id is not None:
            params["expected_catalog_snapshot_id"] = expected_catalog_snapshot_id
        raw = await self._runtime._rpc("meta/mobpacks/apply_operation", params)
        return MobpackApplyOperationResult.from_dict(raw)

    async def mobpack_deploy_command(
        self, document: dict[str, Any]
    ) -> MobpackDeployCommandResult:
        """Preview the ``rkat mob run`` deploy command for a mobpack document."""
        raw = await self._runtime._rpc(
            "meta/mobpacks/deploy_command", {"document": document}
        )
        return MobpackDeployCommandResult.from_dict(raw)

    async def mobpack_deploy(
        self,
        document: dict[str, Any],
        *,
        execute: bool | None = None,
    ) -> MobpackDeployResult:
        """Plan (and optionally execute) a mobpack deploy on the host."""
        params: dict[str, Any] = {"document": document}
        if execute is not None:
            params["execute"] = execute
        raw = await self._runtime._rpc("meta/mobpacks/deploy", params)
        return MobpackDeployResult.from_dict(raw)

    async def session_store_bigquery(self, **kwargs: Any) -> Any:
        """Run a BigQuery session-store RPC operation."""
        return await self._runtime._rpc("meta/session_store/bigquery", kwargs)

    def tool_caller(self, module_id: str) -> ToolCaller:
        """Return a callable scoped to one MCP module.

        Usage::

            gmail = mob_handle.tool_caller("google-workspace")
            messages = await gmail("gmail_search", query="is:unread")
        """
        return ToolCaller(self, module_id)

    # -----------------------------------------------------------------
    # Primary API — comms, observation, control plane
    # -----------------------------------------------------------------

    async def ensure_member(
        self, member_id: str, role: str, **kwargs: Any
    ) -> MemberSnapshot:
        """Ensure a mob member exists, spawning it if missing.

        Idempotent — returns the member snapshot whether it was just spawned
        or already existed. Use before ``send()`` when handling first contact
        from an unknown user (e.g. new Slack DM).

        Args:
            member_id: Agent identity for the member.
            role: Role (profile name from mob.toml) to spawn with.
            **kwargs: Optional fields (labels, context, resume_session_id,
                      additional_instructions).
        """
        params: dict[str, Any] = {"role": role, "agent_identity": member_id}
        if "labels" in kwargs:
            params["labels"] = kwargs["labels"]
        if "context" in kwargs:
            params["context"] = kwargs["context"]
        if "resume_session_id" in kwargs:
            params["resume_session_id"] = kwargs["resume_session_id"]
        if "additional_instructions" in kwargs:
            params["additional_instructions"] = kwargs["additional_instructions"]
        raw = await self._runtime._rpc("meta/ensure_member", params)
        return MemberSnapshot.from_dict(raw)

    async def find_members(
        self, label_key: str, label_value: str
    ) -> list[MemberSnapshot]:
        """Find members matching a label key-value pair.

        Example::

            # Find all initiative agents
            initiatives = await handle.find_members("agent_type", "initiative")

            # Find the agent for a specific owner
            agents = await handle.find_members("owner_id", "user-123")
            if agents:
                agent_identity = agents[0].agent_identity
        """
        raw = await self._runtime._rpc(
            "meta/find_members",
            {"label_key": label_key, "label_value": label_value},
        )
        if isinstance(raw, list):
            return [MemberSnapshot.from_dict(m) for m in raw]
        return []

    async def rediscover(self) -> RediscoverReport | None:
        """Reset the mob and re-run discovery + edge reconciliation.

        Sequence: reset mob (retire all, clear state) → re-run Discovery →
        spawn discovered members → reconcile edges.

        Returns ``None`` if no Discovery was configured on the builder.

        Use for "nuke everything and start fresh" scenarios — e.g. a config
        reload, admin reset command, or recovery from a bad state.
        """
        raw = await self._runtime._rpc("meta/rediscover")
        if isinstance(raw, dict) and "status" in raw:
            return None
        return RediscoverReport.from_dict(raw)

    async def reconcile_edges(self) -> ReconcileEdgesReport:
        """Converge the definition's declared peer wiring on the live roster.

        The desired edges come from mob.toml's ``[wiring]`` block
        (``auto_wire_orchestrator`` and ``role_wiring``), which the gateway
        installs by itself; there is no builder knob to set. Wire and unwire
        operations are applied so the live topology matches it, and the report
        names them.

        When it is needed: after ``runtime.reconcile()`` or an identity-first
        boot, and only when the definition declares ``auto_wire_orchestrator``
        or ``role_wiring``. Those restore paths embody identities concurrently
        and do not converge the declared edges themselves, so the wiring is
        otherwise bring-up-order dependent. ``ensure_member`` already runs this
        after every successful materialization, so it is redundant there. With
        no declared wiring the report is empty. Host ``topology_provider``
        edges are converged by the identity plane on every restore and are not
        this call's concern.
        """
        raw = await self._runtime._rpc("meta/reconcile_edges")
        return ReconcileEdgesReport.from_dict(raw)

    async def send(
        self,
        member_id: str,
        message: str | None = None,
        *,
        content: list[dict[str, Any]] | None = None,
        attachments: list[Any] | None = None,
        handling_mode: str | None = None,
    ) -> SendMessageResult:
        """Send a message to a mob member and return the accepting session.

        Args:
            member_id: Target member ID.
            message: Plain text message (simple path).
            content: Multimodal content blocks, e.g.
                ``[{"type": "text", "text": "describe this"},
                  {"type": "image", "media_type": "image/png",
                   "source": "inline", "data": "<base64>"}]``

        Either ``message`` or ``content`` must be provided. If both are given,
        ``content`` takes precedence so multimodal callers are not shadowed by
        stale text.
        """
        params: dict[str, Any] = {"member_id": member_id}
        if handling_mode is not None:
            params["handling_mode"] = handling_mode
        uploads = [_normalize_upload_item(item, idx) for idx, item in enumerate(attachments or [])]
        if content is not None:
            blocks: list[dict[str, Any]] = list(content)
        elif message is not None:
            blocks = [{"type": "text", "text": message}] if uploads else []
            if not uploads:
                params["message"] = message
        else:
            if not uploads:
                raise ValueError("message, content, or attachments must be provided")
            blocks = []
        if uploads:
            for idx, (_data, media_type, _filename) in enumerate(uploads):
                blocks.append(
                    {
                        "type": "image_upload",
                        "upload_id": f"upload-{idx + 1}",
                        "media_type": media_type,
                    }
                )
            params["content"] = blocks
            raw = await self._multipart_rpc("meta/send_message", params, uploads)
        else:
            raw = await self._runtime._rpc("meta/send_message", params)
        return SendMessageResult.from_dict(raw)

    async def get_blob(self, blob_id: str) -> dict[str, Any]:
        """Fetch a blob through the JSON-RPC compatibility boundary.

        The returned ``data`` field is base64 because JSON-RPC is not binary;
        Meta stores and serves the blob internally as raw bytes.
        """
        raw = await self._runtime._rpc("meta/blob/get", {"blob_id": blob_id})
        return raw if isinstance(raw, dict) else {}

    async def upload_blob(
        self,
        file: bytes | bytearray | memoryview | str | Path,
        *,
        media_type: str | None = None,
        filename: str | None = None,
    ) -> dict[str, Any]:
        """Upload one image blob through the multipart console route.

        Returns ``{"blob_id", "media_type", "size"}``.
        """
        data, resolved_media_type, resolved_filename = _read_upload_source(
            file,
            media_type=media_type,
            filename=filename,
        )
        raw = await self._multipart_rpc(
            "meta/blob/upload",
            {
                "upload": {
                    "type": "image_upload",
                    "upload_id": "upload-1",
                    "media_type": resolved_media_type,
                }
            },
            [(data, resolved_media_type, resolved_filename)],
        )
        return raw if isinstance(raw, dict) else {}

    async def _multipart_rpc(
        self,
        method: str,
        params: dict[str, Any],
        uploads: list[tuple[bytes, str, str]],
    ) -> Any:
        base = self._runtime.rust_http_base_url
        if not base:
            raise NotConnectedError(
                "multipart RPC requires rust_http_base_url; start the gateway or call "
                "runtime.set_rust_http_base('http://127.0.0.1:8081')"
            )
        request_id = _next_request_id(method)
        payload = _rpc_request(request_id, method, params)
        boundary = f"meta-{uuid.uuid4().hex}"
        body = self._encode_multipart_body(boundary, payload, uploads)
        req = urllib_request.Request(
            base.rstrip("/") + "/console/rpc/multipart",
            data=body,
            method="POST",
            headers={
                "content-type": f"multipart/form-data; boundary={boundary}",
                "accept": "application/json",
            },
        )
        try:
            response_text = await asyncio.to_thread(self._post_multipart_request, req)
        except HTTPError as exc:
            response_text = exc.read().decode("utf-8", errors="replace")
            raise TransportError(
                f"multipart RPC failed (status={exc.code}): {response_text}"
            ) from exc
        except URLError as exc:
            raise TransportError(f"multipart RPC failed: {exc.reason}") from exc
        response = json.loads(response_text)
        if "error" in response:
            raise _rpc_error_from_payload(
                response["error"], request_id=request_id, method=method
            )
        return response.get("result")

    @staticmethod
    def _encode_multipart_body(
        boundary: str,
        payload: dict[str, Any],
        uploads: list[tuple[bytes, str, str]],
    ) -> bytes:
        lines: list[bytes] = []

        def add(value: str | bytes) -> None:
            lines.append(value if isinstance(value, bytes) else value.encode("utf-8"))

        add(f"--{boundary}\r\n")
        add('Content-Disposition: form-data; name="payload"\r\n')
        add("Content-Type: application/json\r\n\r\n")
        add(json.dumps(payload))
        add("\r\n")
        for idx, (data, media_type, filename) in enumerate(uploads):
            upload_id = f"upload-{idx + 1}"
            safe_filename = filename.replace('"', "")
            add(f"--{boundary}\r\n")
            add(
                f'Content-Disposition: form-data; name="file:{upload_id}"; '
                f'filename="{safe_filename}"\r\n'
            )
            add(f"Content-Type: {media_type}\r\n\r\n")
            add(data)
            add("\r\n")
        add(f"--{boundary}--\r\n")
        return b"".join(lines)

    @staticmethod
    def _post_multipart_request(req: urllib_request.Request) -> str:
        with urllib_request.urlopen(req, timeout=60) as response:
            return response.read().decode("utf-8")

    async def query_events(
        self,
        *,
        since_ms: int | None = None,
        until_ms: int | None = None,
        member_id: str | None = None,
        event_types: list[str] | None = None,
        limit: int | None = None,
        after_seq: int | None = None,
    ) -> list[PersistedEvent]:
        """Query persisted operational events from the event log.

        Returns an empty list if no event log is configured.
        """
        from .types import EventQuery, PersistedEvent
        query = EventQuery(
            since_ms=since_ms,
            until_ms=until_ms,
            member_id=member_id,
            event_types=event_types or [],
            limit=limit,
            after_seq=after_seq,
        )
        raw = await self._runtime._rpc("meta/query_events", query.to_dict())
        events = raw
        if isinstance(raw, dict) and raw.get("status") == "no_event_log_configured":
            events = raw.get("events", [])
        if isinstance(events, list):
            return [PersistedEvent.from_dict(e) for e in events]
        return []

    async def query_mob_events(
        self,
        query: "EventQuery | dict[str, Any] | None" = None,
    ) -> list[MobStructuralEvent]:
        """Query structural mob events from the meerkat ledger.

        Returns events matching ``query``. Pass the highest seen
        ``cursor`` as ``EventQuery.after_seq`` to paginate. Without
        ``after_seq`` the call returns the latest matching events
        (default ``limit = 256``).

        Raises :class:`MobEventsStaleError` when ``after_seq`` is past
        the current ledger frontier; the exception carries
        ``after_cursor`` and ``latest_cursor`` so callers can rewind.
        """
        from .types import EventQuery, MobStructuralEvent
        if query is None:
            params: dict[str, Any] = {}
        elif isinstance(query, EventQuery):
            params = query.to_dict()
        elif isinstance(query, dict):
            params = dict(query)
        else:
            raise TypeError(f"unsupported query type: {type(query).__name__}")
        try:
            raw = await self._runtime._rpc("meta/mob_events/query", params)
        except RpcError as err:
            if err.code == MOB_EVENTS_STALE_CURSOR_CODE:
                raise MobEventsStaleError.from_rpc_error(err) from err
            raise
        events: list[Any] = []
        if isinstance(raw, dict):
            maybe = raw.get("events")
            if isinstance(maybe, list):
                events = maybe
        elif isinstance(raw, list):
            events = raw
        return [MobStructuralEvent.from_dict(e) for e in events if isinstance(e, dict)]

    async def subscribe_mob_events(
        self,
        query: "EventQuery | dict[str, Any] | None" = None,
    ) -> AsyncIterator[MobStructuralEvent]:
        """Replay structural mob events as an async iterator.

        Returns an async iterator over the snapshot frame returned by
        ``meta/mob_events/subscribe``. Live tailing for production
        streaming uses the SSE bridge at ``/meta/mob_events/stream``;
        this method is the JSON-RPC snapshot equivalent of
        :meth:`query_mob_events` for clients that prefer an iterator.

        Raises :class:`MobEventsStaleError` when ``after_seq`` is past
        the current ledger frontier.
        """
        from .types import EventQuery, MobStructuralEvent
        if query is None:
            params: dict[str, Any] = {}
        elif isinstance(query, EventQuery):
            params = query.to_dict()
        elif isinstance(query, dict):
            params = dict(query)
        else:
            raise TypeError(f"unsupported query type: {type(query).__name__}")
        try:
            raw = await self._runtime._rpc("meta/mob_events/subscribe", params)
        except RpcError as err:
            if err.code == MOB_EVENTS_STALE_CURSOR_CODE:
                raise MobEventsStaleError.from_rpc_error(err) from err
            raise
        events: list[Any] = []
        if isinstance(raw, dict):
            maybe = raw.get("events")
            if isinstance(maybe, list):
                events = maybe
        elif isinstance(raw, list):
            # Wire shape parity with `query_mob_events`: a server that
            # returns a bare list (older gateway, future shape change)
            # must still yield events instead of silently dropping them.
            events = raw
        for entry in events:
            if isinstance(entry, dict):
                yield MobStructuralEvent.from_dict(entry)

    # -----------------------------------------------------------------
    # Roster — member lifecycle
    # -----------------------------------------------------------------

    async def list_members(self) -> list[MemberSnapshot]:
        """List all members in the mob roster (active + retiring)."""
        raw = await self._runtime._rpc("meta/list_members")
        if isinstance(raw, list):
            return [MemberSnapshot.from_dict(m) for m in raw]
        return []

    async def get_member(self, member_id: str) -> MemberSnapshot:
        """Get a single member snapshot by ID. Raises RpcError if not found."""
        raw = await self._runtime._rpc("meta/get_member", {"member_id": member_id})
        return MemberSnapshot.from_dict(raw)

    async def retire_member(self, member_id: str) -> None:
        """Retire a member (transition to retiring state)."""
        await self._runtime._rpc("meta/retire_member", {"member_id": member_id})

    async def respawn_member(self, member_id: str) -> None:
        """Respawn a member (replace with fresh instance).

        On an identity-first gateway this is DESTRUCTIVE: the member starts
        a fresh session under an advanced generation. To clear a
        durability-degraded member without losing its transcript use
        :meth:`reload_member`.
        """
        await self._runtime._rpc("meta/respawn_member", {"member_id": member_id})

    async def reload_member(self, member_id: str) -> MemberReloadResult:
        """Non-destructive cold reload of an identity's live member.

        Same identity, same durable session id, same generation:
        the live runtime registration is discarded and the same session is
        re-materialized from durable truth. This is the repair for a member
        whose sends fail with the reload-required class (meerkat's
        ``RecoveryRepairBlocked``). Identity-first gateways only.
        """
        raw = await self._runtime._rpc("meta/reload_member", {"member_id": member_id})
        return MemberReloadResult.from_dict(raw if isinstance(raw, dict) else {})

    async def member_health(self, member_id: str) -> MemberHealth:
        """Lifecycle and delivery health for one identity, read in-process.

        Unlike :meth:`member_status` this never crosses the mob actor loop,
        so it answers while the loop is stalled and reports the open
        ``stall_id`` and the last delivery error class.
        """
        raw = await self._runtime._rpc("meta/member_health", {"member_id": member_id})
        return MemberHealth.from_dict(raw if isinstance(raw, dict) else {})

    # -----------------------------------------------------------------
    # Routing — route management
    # -----------------------------------------------------------------

    async def list_routes(self) -> list[RuntimeRouteResult]:
        """List all configured runtime routes."""
        raw = await self._runtime._rpc("meta/routing/routes/list")
        routes = raw.get("routes", []) if isinstance(raw, dict) else []
        return [RuntimeRouteResult.from_dict(r) for r in routes]

    async def add_route(
        self,
        route_key: str,
        recipient: str,
        sink: str,
        target_module: str,
        channel: str | None = None,
    ) -> RuntimeRouteResult:
        """Add or update a route. Overwrites on duplicate route_key."""
        params: dict[str, Any] = {
            "route_key": route_key,
            "recipient": recipient,
            "sink": sink,
            "target_module": target_module,
        }
        if channel is not None:
            params["channel"] = channel
        raw = await self._runtime._rpc("meta/routing/routes/add", params)
        route_data = raw.get("route", raw) if isinstance(raw, dict) else raw
        return RuntimeRouteResult.from_dict(route_data)

    async def delete_route(self, route_key: str) -> RuntimeRouteResult:
        """Delete a route by key. Returns the deleted route."""
        raw = await self._runtime._rpc("meta/routing/routes/delete", {"route_key": route_key})
        deleted_data = raw.get("deleted", raw) if isinstance(raw, dict) else raw
        return RuntimeRouteResult.from_dict(deleted_data)

    # -----------------------------------------------------------------
    # Delivery — history
    # -----------------------------------------------------------------

    async def delivery_history(
        self,
        recipient: str | None = None,
        sink: str | None = None,
        limit: int = 20,
    ) -> DeliveryHistoryResult:
        """Query delivery history with optional recipient/sink filters."""
        params: dict[str, Any] = {"limit": limit}
        if recipient is not None:
            params["recipient"] = recipient
        if sink is not None:
            params["sink"] = sink
        raw = await self._runtime._rpc("meta/delivery/history", params)
        return DeliveryHistoryResult.from_dict(raw)

    # -----------------------------------------------------------------
    # Gating — policy enforcement
    # -----------------------------------------------------------------

    async def gating_evaluate(
        self,
        action: str,
        actor_id: str,
        **kwargs: Any,
    ) -> GatingEvaluateResult:
        """Evaluate an action against configured gating policies."""
        params: dict[str, Any] = {"action": action, "actor_id": actor_id, **kwargs}
        raw = await self._runtime._rpc("meta/gating/evaluate", params)
        return GatingEvaluateResult.from_dict(raw)

    async def gating_pending(self) -> list[GatingPendingEntry]:
        """List gating decisions awaiting approval."""
        raw = await self._runtime._rpc("meta/gating/pending")
        entries = raw.get("pending", []) if isinstance(raw, dict) else []
        return [GatingPendingEntry.from_dict(e) for e in entries]

    async def gating_decide(
        self,
        pending_id: str,
        decision: str,
        approver_id: str,
        **kwargs: Any,
    ) -> GatingDecisionResult:
        """Approve or reject a pending gating action."""
        params: dict[str, Any] = {
            "pending_id": pending_id,
            "decision": decision,
            "approver_id": approver_id,
            **kwargs,
        }
        raw = await self._runtime._rpc("meta/gating/decide", params)
        return GatingDecisionResult.from_dict(raw)

    async def gating_audit(self, limit: int = 100) -> list[GatingAuditEntry]:
        """Query the gating audit log."""
        raw = await self._runtime._rpc("meta/gating/audit", {"limit": limit})
        entries = raw.get("entries", []) if isinstance(raw, dict) else []
        return [GatingAuditEntry.from_dict(e) for e in entries]

    # -----------------------------------------------------------------
    # Memory — store management
    # -----------------------------------------------------------------

    async def memory_stores(self) -> list[MemoryStoreInfo]:
        """List available memory stores with record counts."""
        raw = await self._runtime._rpc("meta/memory/stores")
        stores = raw.get("stores", []) if isinstance(raw, dict) else []
        return [MemoryStoreInfo.from_dict(s) for s in stores]

    async def memory_index(
        self,
        entity: str,
        topic: str,
        store: str,
        **kwargs: Any,
    ) -> MemoryIndexResult:
        """Index an assertion into a memory store."""
        params: dict[str, Any] = {
            "entity": entity,
            "topic": topic,
            "store": store,
            **kwargs,
        }
        raw = await self._runtime._rpc("meta/memory/index", params)
        return MemoryIndexResult.from_dict(raw)

    # -----------------------------------------------------------------
    # WorkGraph — collaborative work-item graph
    # -----------------------------------------------------------------

    async def workgraph_snapshot(self, **kwargs: Any) -> WorkGraphSnapshotResult:
        """Return a full WorkGraph snapshot (items, edges, attention, ready ids)."""
        raw = await self._runtime._rpc("meta/workgraph/snapshot", dict(kwargs))
        return WorkGraphSnapshotResult.from_dict(raw)

    async def workgraph_list(self, **kwargs: Any) -> WorkGraphItemsResult:
        """List WorkGraph items matching the given filter."""
        raw = await self._runtime._rpc("meta/workgraph/list", dict(kwargs))
        return WorkGraphItemsResult.from_dict(raw)

    async def workgraph_get(self, id: str, **kwargs: Any) -> WorkGraphItem:
        """Fetch a single WorkGraph item by id."""
        params: dict[str, Any] = {"id": id, **kwargs}
        raw = await self._runtime._rpc("meta/workgraph/get", params)
        item_data = raw.get("item", raw) if isinstance(raw, dict) else raw
        return WorkGraphItem.from_dict(item_data)

    async def workgraph_ready(self, **kwargs: Any) -> WorkGraphItemsResult:
        """List WorkGraph items that are ready to claim (no unresolved blockers)."""
        raw = await self._runtime._rpc("meta/workgraph/ready", dict(kwargs))
        return WorkGraphItemsResult.from_dict(raw)

    async def workgraph_events(self, **kwargs: Any) -> list[WorkGraphEventEntry]:
        """Query the WorkGraph event log."""
        raw = await self._runtime._rpc("meta/workgraph/events", dict(kwargs))
        events = raw.get("events", []) if isinstance(raw, dict) else []
        return [WorkGraphEventEntry.from_dict(e) for e in events]

    async def workgraph_attention_list(
        self, **kwargs: Any
    ) -> list[WorkGraphAttentionBinding]:
        """List attention bindings matching the given filter."""
        raw = await self._runtime._rpc("meta/workgraph/attention/list", dict(kwargs))
        bindings = raw.get("attention", []) if isinstance(raw, dict) else []
        return [WorkGraphAttentionBinding.from_dict(b) for b in bindings]

    async def workgraph_goal_status(
        self, binding_id: str, **kwargs: Any
    ) -> WorkGraphGoalResult:
        """Fetch the goal work item + attention binding for a binding id."""
        params: dict[str, Any] = {"binding_id": binding_id, **kwargs}
        raw = await self._runtime._rpc("meta/workgraph/goal/status", params)
        return WorkGraphGoalResult.from_dict(raw)

    async def workgraph_create(self, title: str, **kwargs: Any) -> WorkGraphItem:
        """Create a new WorkGraph item."""
        params: dict[str, Any] = {"title": title, **kwargs}
        raw = await self._runtime._rpc("meta/workgraph/create", params)
        item_data = raw.get("item", raw) if isinstance(raw, dict) else raw
        return WorkGraphItem.from_dict(item_data)

    async def workgraph_update(
        self, id: str, expected_revision: int, **kwargs: Any
    ) -> WorkGraphItem:
        """Update a WorkGraph item's mutable fields (CAS via ``expected_revision``)."""
        params: dict[str, Any] = {
            "id": id,
            "expected_revision": expected_revision,
            **kwargs,
        }
        raw = await self._runtime._rpc("meta/workgraph/update", params)
        item_data = raw.get("item", raw) if isinstance(raw, dict) else raw
        return WorkGraphItem.from_dict(item_data)

    async def workgraph_claim(
        self,
        id: str,
        expected_revision: int,
        owner: dict[str, Any],
        **kwargs: Any,
    ) -> WorkGraphItem:
        """Claim a WorkGraph item for an owner (CAS via ``expected_revision``)."""
        params: dict[str, Any] = {
            "id": id,
            "expected_revision": expected_revision,
            "owner": owner,
            **kwargs,
        }
        raw = await self._runtime._rpc("meta/workgraph/claim", params)
        item_data = raw.get("item", raw) if isinstance(raw, dict) else raw
        return WorkGraphItem.from_dict(item_data)

    async def workgraph_release(
        self, id: str, expected_revision: int, **kwargs: Any
    ) -> WorkGraphItem:
        """Release a claimed WorkGraph item (CAS via ``expected_revision``)."""
        params: dict[str, Any] = {
            "id": id,
            "expected_revision": expected_revision,
            **kwargs,
        }
        raw = await self._runtime._rpc("meta/workgraph/release", params)
        item_data = raw.get("item", raw) if isinstance(raw, dict) else raw
        return WorkGraphItem.from_dict(item_data)

    async def workgraph_close(
        self, id: str, expected_revision: int, **kwargs: Any
    ) -> WorkGraphItem:
        """Close a WorkGraph item (default status ``completed``)."""
        params: dict[str, Any] = {
            "id": id,
            "expected_revision": expected_revision,
            **kwargs,
        }
        raw = await self._runtime._rpc("meta/workgraph/close", params)
        item_data = raw.get("item", raw) if isinstance(raw, dict) else raw
        return WorkGraphItem.from_dict(item_data)

    async def workgraph_block(
        self, id: str, expected_revision: int, **kwargs: Any
    ) -> WorkGraphItem:
        """Mark a WorkGraph item blocked (CAS via ``expected_revision``)."""
        params: dict[str, Any] = {
            "id": id,
            "expected_revision": expected_revision,
            **kwargs,
        }
        raw = await self._runtime._rpc("meta/workgraph/block", params)
        item_data = raw.get("item", raw) if isinstance(raw, dict) else raw
        return WorkGraphItem.from_dict(item_data)

    async def workgraph_link(
        self, kind: str, from_id: str, to_id: str, **kwargs: Any
    ) -> WorkGraphEdge:
        """Link two WorkGraph items with an edge (e.g. ``blocks``, ``parent``)."""
        params: dict[str, Any] = {
            "kind": kind,
            "from_id": from_id,
            "to_id": to_id,
            **kwargs,
        }
        raw = await self._runtime._rpc("meta/workgraph/link", params)
        edge_data = raw.get("edge", raw) if isinstance(raw, dict) else raw
        return WorkGraphEdge.from_dict(edge_data)

    async def workgraph_add_evidence(
        self,
        id: str,
        expected_revision: int,
        evidence: dict[str, Any],
        **kwargs: Any,
    ) -> WorkGraphItem:
        """Attach an evidence reference to a WorkGraph item."""
        params: dict[str, Any] = {
            "id": id,
            "expected_revision": expected_revision,
            "evidence": evidence,
            **kwargs,
        }
        raw = await self._runtime._rpc("meta/workgraph/evidence/add", params)
        item_data = raw.get("item", raw) if isinstance(raw, dict) else raw
        return WorkGraphItem.from_dict(item_data)

    async def workgraph_escalate_policy(
        self,
        binding_id: str,
        id: str,
        expected_revision: int,
        completion_policy: dict[str, Any],
        **kwargs: Any,
    ) -> WorkGraphItem:
        """Escalate a WorkGraph item's completion policy under a goal's authority.

        The witness (``AttentionContextProjection``) is fetched server-side
        from ``binding_id`` — the SDK does not build or forward it.
        """
        params: dict[str, Any] = {
            "binding_id": binding_id,
            "id": id,
            "expected_revision": expected_revision,
            "completion_policy": completion_policy,
            **kwargs,
        }
        raw = await self._runtime._rpc("meta/workgraph/policy/escalate", params)
        item_data = raw.get("item", raw) if isinstance(raw, dict) else raw
        return WorkGraphItem.from_dict(item_data)

    async def workgraph_goal_create(
        self, title: str, target: dict[str, Any], **kwargs: Any
    ) -> WorkGraphGoalResult:
        """Create a goal work item plus its attention binding.

        ``target`` is ``{"kind": "session", "session_id": ...}``,
        ``{"kind": "identity", "identity": ...}`` (lowered server-side to an
        owner key against the runtime's mob id), or
        ``{"kind": "owner", "owner_key": {"kind": ..., "id": ...}}``.
        """
        params: dict[str, Any] = {"title": title, "target": target, **kwargs}
        raw = await self._runtime._rpc("meta/workgraph/goal/create", params)
        return WorkGraphGoalResult.from_dict(raw)

    async def workgraph_goal_confirm(
        self, binding_id: str, expected_revision: int, **kwargs: Any
    ) -> WorkGraphGoalResult:
        """Confirm a goal's completion (CAS via ``expected_revision``)."""
        params: dict[str, Any] = {
            "binding_id": binding_id,
            "expected_revision": expected_revision,
            **kwargs,
        }
        raw = await self._runtime._rpc("meta/workgraph/goal/confirm", params)
        return WorkGraphGoalResult.from_dict(raw)

    async def workgraph_goal_request_close(
        self, binding_id: str, expected_revision: int, **kwargs: Any
    ) -> WorkGraphGoalResult:
        """Request closure of a goal (default status ``completed``)."""
        params: dict[str, Any] = {
            "binding_id": binding_id,
            "expected_revision": expected_revision,
            **kwargs,
        }
        raw = await self._runtime._rpc("meta/workgraph/goal/request_close", params)
        return WorkGraphGoalResult.from_dict(raw)

    async def workgraph_attention_pause(
        self, binding_id: str, expected_revision: int, **kwargs: Any
    ) -> WorkGraphAttentionBinding:
        """Pause an attention binding (optionally until a given time)."""
        params: dict[str, Any] = {
            "binding_id": binding_id,
            "expected_revision": expected_revision,
            **kwargs,
        }
        raw = await self._runtime._rpc("meta/workgraph/attention/pause", params)
        binding_data = raw.get("attention", raw) if isinstance(raw, dict) else raw
        return WorkGraphAttentionBinding.from_dict(binding_data)

    async def workgraph_attention_resume(
        self, binding_id: str, expected_revision: int, **kwargs: Any
    ) -> WorkGraphAttentionBinding:
        """Resume a paused attention binding."""
        params: dict[str, Any] = {
            "binding_id": binding_id,
            "expected_revision": expected_revision,
            **kwargs,
        }
        raw = await self._runtime._rpc("meta/workgraph/attention/resume", params)
        binding_data = raw.get("attention", raw) if isinstance(raw, dict) else raw
        return WorkGraphAttentionBinding.from_dict(binding_data)

    async def workgraph_attention_reassign(
        self,
        binding_id: str,
        expected_revision: int,
        target: dict[str, Any],
        **kwargs: Any,
    ) -> WorkGraphAttentionReassignResult:
        """Reassign an attention binding to a new target.

        The witness is fetched server-side from ``binding_id``, mirroring
        ``workgraph_escalate_policy``.
        """
        params: dict[str, Any] = {
            "binding_id": binding_id,
            "expected_revision": expected_revision,
            "target": target,
            **kwargs,
        }
        raw = await self._runtime._rpc("meta/workgraph/attention/reassign", params)
        return WorkGraphAttentionReassignResult.from_dict(raw)

    async def workgraph_attention_prune(
        self, updated_before: str | None = None, **kwargs: Any
    ) -> int:
        """Prune TERMINAL (superseded/stopped) attention binding rows.

        The workgraph event stream keeps the audit history; binding rows
        otherwise grow monotonically with reassignment churn.  Pass an
        RFC3339 ``updated_before`` to prune only rows last updated strictly
        before that instant.  Returns the number of rows pruned.
        """
        params: dict[str, Any] = dict(kwargs)
        if updated_before is not None:
            params["updated_before"] = updated_before
        raw = await self._runtime._rpc("meta/workgraph/attention/prune", params)
        pruned = raw.get("pruned", 0) if isinstance(raw, dict) else 0
        return int(pruned)

    # -----------------------------------------------------------------
    # Mob announcements: meta/mob_announcements/*
    # -----------------------------------------------------------------

    async def _announcement_rpc(
        self,
        method: str,
        params: dict[str, Any],
        optional: dict[str, Any] | None = None,
    ) -> dict[str, Any]:
        """Call one mob announcement method.

        ``optional`` params are sent only when set: the gateway's params are
        strict, and an absent one takes the gateway's default. Method-not-found
        on a method that came with sources surfaces as "Meta does not support
        sources".
        """
        sent = dict(params)
        sent.update({key: value for key, value in (optional or {}).items() if value is not None})
        try:
            raw = await self._runtime._rpc(f"meta/mob_announcements/{method}", sent)
        except RpcError as err:
            full = f"meta/mob_announcements/{method}"
            if err.code != _JSONRPC_METHOD_NOT_FOUND or full in _ANNOUNCEMENT_METHODS_BEFORE_SOURCES:
                raise
            raise RpcError(
                code=err.code,
                message=f"Meta does not support sources: the gateway has no {full}",
                request_id=err.request_id,
                method=err.method or full,
                data=err.data,
            ) from err
        return raw if isinstance(raw, dict) else {}

    async def announce(
        self,
        source_key: str,
        topics: list[str] | tuple[str, ...],
        body: str,
        *,
        mob_id: str | None = None,
        correlation_id: str | None = None,
        causes: list[dict[str, str]] | None = None,
    ) -> AnnounceResult:
        """Announce as the host into a mob hosted here (this gateway's own by
        default).

        ``source_key`` is the announcement's stable identity in its mob: the
        same key with the same content replays (``replayed``), with other
        content it is refused (:class:`AnnouncementConflictError`).
        ``causes`` names earlier announcements (``{"mob_id", "source_key"}``).
        """
        raw = await self._announcement_rpc(
            "announce",
            {"source_key": source_key, "topics": list(topics), "body": body},
            {"mob_id": mob_id, "correlation_id": correlation_id, "causes": causes},
        )
        return AnnounceResult.from_dict(raw)

    async def announcement_get(self, mob_id: str, source_key: str) -> Announcement | None:
        """One announcement by its mob and source key, or ``None``."""
        raw = await self._announcement_rpc("get", {"mob_id": mob_id, "source_key": source_key})
        announcement = raw.get("announcement")
        return Announcement.from_dict(announcement) if isinstance(announcement, dict) else None

    async def announcement_subscription_put(
        self,
        operation_id: str,
        subscription_id: str,
        member: AnnouncementMember | dict[str, str],
        topics: list[str] | tuple[str, ...],
        *,
        mobs: list[str] | tuple[str, ...] | None = None,
        start: Any | None = None,
        expected_revision: int | None = None,
        delivery: dict[str, Any] | None = None,
    ) -> AnnouncementSubscription:
        """Create a subscription, or replace its selector at
        ``expected_revision``.

        ``mobs`` are the mobs hosted here whose announcements it takes (this
        gateway's own when absent); ``start`` is its start frontier (now by
        default). ``delivery`` is ``{"await_handling": bool}``: whether the
        member's next batch waits until the turn of its previous one has
        ended, for at most 15 minutes. Left out, the gateway decides: on for
        the triage member of a triage route that takes a selected topic, off
        otherwise. ``operation_id`` makes a retry safe: the same operation
        replays its answer.
        """
        raw = await self._announcement_rpc(
            "subscriptions/put",
            {
                "operation_id": operation_id,
                "subscription_id": subscription_id,
                "member": _announcement_member(member),
                "topics": list(topics),
            },
            {
                "mobs": list(mobs) if mobs is not None else None,
                "start": start,
                "expected_revision": expected_revision,
                "delivery": dict(delivery) if delivery is not None else None,
            },
        )
        return AnnouncementSubscription.from_dict(raw.get("subscription"))

    async def announcement_role_template_put(
        self,
        operation_id: str,
        template_id: str,
        members: AnnouncementRoleSelector | dict[str, str],
        topics: list[str] | tuple[str, ...],
        *,
        mobs: list[str] | tuple[str, ...] | None = None,
        expected_revision: int | None = None,
        delivery: dict[str, Any] | None = None,
    ) -> AnnouncementRoleTemplate:
        """Create a role template, or replace its selector at
        ``expected_revision``: ``subscriptions/put`` with ``members``
        (``{"mob_id", "role"}``) instead of one member.

        Each member of the mob with the role captures through a child
        subscription, made at the first announcement it matches. A template
        starts now. Its id is at most 200 bytes, and a subscription and a
        role template never share an id (``subscription_id_taken``). The
        subscription verbs and ``announcement_subscription_get`` take its id.
        ``delivery`` (``{"await_handling": bool}``) is every child's, as for
        ``announcement_subscription_put``; it is sent only when set.
        """
        selector = members.to_dict() if isinstance(members, AnnouncementRoleSelector) else dict(members)
        raw = await self._announcement_rpc(
            "subscriptions/put",
            {
                "operation_id": operation_id,
                "subscription_id": template_id,
                "members": selector,
                "topics": list(topics),
            },
            {
                "mobs": list(mobs) if mobs is not None else None,
                "expected_revision": expected_revision,
                "delivery": dict(delivery) if delivery is not None else None,
            },
        )
        return AnnouncementRoleTemplate.from_dict(raw.get("subscription"))

    async def _announcement_subscription_change(
        self, change: str, operation_id: str, subscription_id: str, expected_revision: int
    ) -> AnnouncementSubscriptionChange:
        raw = await self._announcement_rpc(
            f"subscriptions/{change}",
            {
                "operation_id": operation_id,
                "subscription_id": subscription_id,
                "expected_revision": expected_revision,
            },
        )
        return AnnouncementSubscriptionChange.from_dict(raw)

    async def announcement_subscription_pause(
        self, operation_id: str, subscription_id: str, expected_revision: int
    ) -> AnnouncementSubscriptionChange:
        """Pause a subscription: it keeps capturing and delivers nothing new
        until resumed."""
        return await self._announcement_subscription_change(
            "pause", operation_id, subscription_id, expected_revision
        )

    async def announcement_subscription_resume(
        self, operation_id: str, subscription_id: str, expected_revision: int
    ) -> AnnouncementSubscriptionChange:
        """Resume a paused subscription."""
        return await self._announcement_subscription_change(
            "resume", operation_id, subscription_id, expected_revision
        )

    async def announcement_subscription_cancel(
        self, operation_id: str, subscription_id: str, expected_revision: int
    ) -> AnnouncementSubscriptionChange:
        """Cancel a subscription; terminal. Sealed batches settle from their
        own native evidence (``unresolved_batches``)."""
        return await self._announcement_subscription_change(
            "cancel", operation_id, subscription_id, expected_revision
        )

    async def announcement_subscription_get(self, subscription_id: str) -> AnnouncementSubscriptionView:
        """A subscription (``None`` when unknown) and why its member's
        deliveries wait, if they do."""
        raw = await self._announcement_rpc("subscriptions/get", {"subscription_id": subscription_id})
        return AnnouncementSubscriptionView.from_dict(raw)

    async def announcement_subscription_list(
        self,
        *,
        member: AnnouncementMember | dict[str, str] | None = None,
        after: str | None = None,
        limit: int | None = None,
    ) -> list[AnnouncementSubscription]:
        """A page of subscriptions, optionally one member's, after a
        subscription id."""
        raw = await self._announcement_rpc(
            "subscriptions/list",
            {},
            {
                "member": _announcement_member(member) if member is not None else None,
                "after": after,
                "limit": limit,
            },
        )
        rows = raw.get("subscriptions")
        return [AnnouncementSubscription.from_dict(row) for row in rows] if isinstance(rows, list) else []

    async def announcement_delivery_list(
        self,
        *,
        member: AnnouncementMember | dict[str, str] | None = None,
        subscription_id: str | None = None,
        after: str | None = None,
        limit: int | None = None,
    ) -> list[AnnouncementBatch]:
        """A page of delivery batches, by member or subscription."""
        raw = await self._announcement_rpc(
            "deliveries/list",
            {},
            {
                "member": _announcement_member(member) if member is not None else None,
                "subscription_id": subscription_id,
                "after": after,
                "limit": limit,
            },
        )
        rows = raw.get("deliveries")
        return [AnnouncementBatch.from_dict(row) for row in rows] if isinstance(rows, list) else []

    async def announcement_delivery_observe(self, batch_id: str) -> AnnouncementDeliveryObservation:
        """Read a batch and its native delivery status live, without
        changing anything."""
        raw = await self._announcement_rpc("deliveries/observe", {"batch_id": batch_id})
        return AnnouncementDeliveryObservation.from_dict(raw)

    async def announcement_delivery_reconcile(self, batch_id: str) -> AnnouncementDeliveryObservation:
        """Read a batch's native delivery status and settle the batch from
        that evidence."""
        raw = await self._announcement_rpc("deliveries/reconcile", {"batch_id": batch_id})
        return AnnouncementDeliveryObservation.from_dict(raw)

    async def announcement_delivery_redeliver(self, operation_id: str, batch_id: str) -> AnnouncementBatchHandling:
        """Owe a batch's held items again, for the member's next batch, after
        a fresh coverage check; only a batch whose handling is ``unhandled``
        with held items (``not_held`` otherwise)."""
        raw = await self._announcement_rpc(
            "deliveries/redeliver", {"operation_id": operation_id, "batch_id": batch_id}
        )
        return AnnouncementBatchHandling.from_dict(raw.get("handling"))

    async def announcement_delivery_dismiss(self, operation_id: str, batch_id: str) -> AnnouncementBatchHandling:
        """Settle a batch's held items as handled by an operator; only a batch
        whose handling is ``unhandled`` with held items (``not_held``
        otherwise)."""
        raw = await self._announcement_rpc(
            "deliveries/dismiss", {"operation_id": operation_id, "batch_id": batch_id}
        )
        return AnnouncementBatchHandling.from_dict(raw.get("handling"))

    async def announcement_source_put(
        self,
        operation_id: str,
        source_id: str,
        identity: dict[str, Any],
        observation: dict[str, Any],
        topics: list[str] | tuple[str, ...],
        audience: list[dict[str, Any]],
        *,
        mob_id: str | None = None,
        expected_revision: int | None = None,
        new_generation: bool | None = None,
        connection: dict[str, Any] | None = None,
        routes: list[dict[str, Any]] | None = None,
        conversation_members: list[dict[str, str]] | None = None,
        start: dict[str, Any] | None = None,
        delivery: dict[str, Any] | None = None,
        health_policy: dict[str, Any] | None = None,
        runner: dict[str, Any] | None = None,
    ) -> AnnouncementSourcePutResult:
        """Declare a source, replace its declaration at
        ``expected_revision``, or start its next generation
        (``new_generation``).

        ``identity`` is ``{"provider", "principal", "scope"}``;
        ``observation`` is ``{"baseline", "deletions"}``; ``audience`` lists
        who may receive its items. ``routes`` defaults to one direct route.
        ``runner`` (``{"host_runnable", "every_seconds"}``) names the host
        runnable a schedule service runs to collect it.
        """
        raw = await self._announcement_rpc(
            "sources/put",
            {
                "operation_id": operation_id,
                "source_id": source_id,
                "identity": identity,
                "observation": observation,
                "topics": list(topics),
                "audience": list(audience),
            },
            {
                "mob_id": mob_id,
                "expected_revision": expected_revision,
                "new_generation": new_generation,
                "connection": connection,
                "routes": routes,
                "conversation_members": conversation_members,
                "start": start,
                "delivery": delivery,
                "health_policy": health_policy,
                "runner": runner,
            },
        )
        return AnnouncementSourcePutResult.from_dict(raw)

    async def _announcement_source_change(
        self,
        change: str,
        operation_id: str,
        source_id: str,
        expected_revision: int,
        optional: dict[str, Any],
    ) -> AnnouncementSource:
        raw = await self._announcement_rpc(
            f"sources/{change}",
            {"operation_id": operation_id, "source_id": source_id, "expected_revision": expected_revision},
            optional,
        )
        return AnnouncementSource.from_dict(raw.get("source"))

    async def announcement_source_pause(
        self, operation_id: str, source_id: str, expected_revision: int, *, mob_id: str | None = None
    ) -> AnnouncementSource:
        """Pause a source: it takes no collector step until resumed and keeps
        its cursor."""
        return await self._announcement_source_change(
            "pause", operation_id, source_id, expected_revision, {"mob_id": mob_id}
        )

    async def announcement_source_resume(
        self,
        operation_id: str,
        source_id: str,
        expected_revision: int,
        *,
        mob_id: str | None = None,
        rebaseline: bool | None = None,
    ) -> AnnouncementSource:
        """Resume a paused source from its kept cursor, or from a fresh
        baseline with ``rebaseline=True``."""
        return await self._announcement_source_change(
            "resume", operation_id, source_id, expected_revision, {"mob_id": mob_id, "rebaseline": rebaseline}
        )

    async def announcement_source_end(
        self, operation_id: str, source_id: str, expected_revision: int, *, mob_id: str | None = None
    ) -> AnnouncementSource:
        """End a source's generation. A later declaration starts the next
        one."""
        return await self._announcement_source_change(
            "end", operation_id, source_id, expected_revision, {"mob_id": mob_id}
        )

    async def announcement_source_get(
        self, source_id: str, *, mob_id: str | None = None, include_streams: bool | None = None
    ) -> AnnouncementSourceView:
        """One source (``None`` when the mob has none by that id), with its
        cursor token and failure detail, and its streams when
        ``include_streams`` is set."""
        raw = await self._announcement_rpc(
            "sources/get", {"source_id": source_id}, {"mob_id": mob_id, "include_streams": include_streams}
        )
        return AnnouncementSourceView.from_dict(raw)

    async def announcement_source_list(
        self,
        *,
        mob_id: str | None = None,
        after: str | None = None,
        state: str | None = None,
        limit: int | None = None,
        escalated: bool | None = None,
    ) -> list[AnnouncementSource]:
        """A page of a mob's sources, or with ``escalated=True`` only those
        with an open escalation. A list shows each principal as its kind and
        digest only, and neither cursor tokens nor failure details."""
        raw = await self._announcement_rpc(
            "sources/list",
            {},
            {"mob_id": mob_id, "after": after, "state": state, "limit": limit, "escalated": escalated},
        )
        rows = raw.get("sources")
        return [AnnouncementSource.from_dict(row) for row in rows] if isinstance(rows, list) else []

    async def announcement_source_advance(
        self,
        operation_id: str,
        source_id: str,
        expected_revision: int,
        expected_version: int,
        principal: dict[str, Any],
        next_cursor: str | None,
        *,
        mob_id: str | None = None,
        page: dict[str, Any] | None = None,
        coverage: str | dict[str, Any] | None = None,
        items: list[dict[str, Any]] | None = None,
        streams: list[dict[str, Any]] | None = None,
        rebaseline: dict[str, Any] | None = None,
        observed: dict[str, Any] | None = None,
        collected_at_ms: int | None = None,
    ) -> SourceAdvanceResult:
        """One collector step: the items observed since the cursor, committed
        with the next cursor at ``expected_revision`` and
        ``expected_version``, under ``principal``.

        The final page of a baseline, a backfill or a stream onboarding says what
        it covered: ``coverage="complete"`` or ``{"gap": {"reason": ...}}``. A
        step that lost a race raises :class:`SourceVersionConflictError` or
        :class:`SourceStaleRevisionError` with the source's current position.
        """
        raw = await self._announcement_rpc(
            "sources/advance",
            {
                "operation_id": operation_id,
                "source_id": source_id,
                "expected_revision": expected_revision,
                "expected_version": expected_version,
                "principal": principal,
                "next_cursor": next_cursor,
            },
            {
                "mob_id": mob_id,
                "page": page,
                "coverage": coverage,
                "items": items,
                "streams": streams,
                "rebaseline": rebaseline,
                "observed": observed,
                "collected_at_ms": collected_at_ms,
            },
        )
        return SourceAdvanceResult.from_dict(raw)

    async def announcement_source_record_failure(
        self,
        operation_id: str,
        source_id: str,
        expected_revision: int,
        expected_version: int,
        principal: dict[str, Any],
        failure: dict[str, Any],
        *,
        mob_id: str | None = None,
    ) -> AnnouncementSource:
        """Record a collector failure (``{"class", "retry_after_ms"?,
        "detail"?}``), fenced like a step."""
        raw = await self._announcement_rpc(
            "sources/record_failure",
            {
                "operation_id": operation_id,
                "source_id": source_id,
                "expected_revision": expected_revision,
                "expected_version": expected_version,
                "principal": principal,
                "failure": failure,
            },
            {"mob_id": mob_id},
        )
        return AnnouncementSource.from_dict(raw.get("source"))

    async def announcement_member_get(
        self, member: AnnouncementMember | dict[str, str]
    ) -> AnnouncementMemberView:
        """A member's backlog, why its deliveries wait, and its open
        escalations."""
        raw = await self._announcement_rpc("members/get", {"member": _announcement_member(member)})
        return AnnouncementMemberView.from_dict(raw)

    async def announcement_member_list(
        self,
        *,
        after: AnnouncementMember | dict[str, str] | None = None,
        limit: int | None = None,
        escalated: bool | None = None,
    ) -> list[AnnouncementMemberView]:
        """A page of the members with an active or paused subscription, a
        suspend marker or an open escalation, in member order after
        ``after``, or with ``escalated=True`` only those with an open
        escalation."""
        raw = await self._announcement_rpc(
            "members/list",
            {},
            {
                "after": _announcement_member(after) if after is not None else None,
                "limit": limit,
                "escalated": escalated,
            },
        )
        rows = raw.get("members")
        return [AnnouncementMemberView.from_dict(row) for row in rows] if isinstance(rows, list) else []

    async def announcement_escalations_list(
        self,
        *,
        after: int | None = None,
        limit: int | None = None,
        open: bool | None = None,
    ) -> AnnouncementEscalationsPage:
        """The escalation events after the event cursor ``after`` (from the
        first one kept by default), or with ``open=True`` the open
        escalations. After a reconnect, read on from the last cursor a
        ``meta/on_error`` escalation event or this page's ``next`` named."""
        raw = await self._announcement_rpc(
            "escalations/list", {}, {"after": after, "limit": limit, "open": open}
        )
        return AnnouncementEscalationsPage.from_dict(raw)

    async def announcement_declarations_apply(
        self,
        declarations: dict[str, Any] | None = None,
        *,
        revision: int | None = None,
    ) -> AnnouncementDeclarationsView:
        """Apply declared sources and subscriptions now, and answer what is
        applied.

        ``declarations`` is ``{"revision", "sources"?, "subscriptions"?}`` in
        wire form, for a gateway that declares none or declares them inline;
        ``revision`` defaults to its own. Called with neither, a gateway whose
        declarations come from a sidecar file reads the file again. Any other
        combination is ``-32602``. A lower revision than the applied one
        raises ``AnnouncementRefusedError`` with ``declaration_rollback``, and
        the applied revision with other declarations
        ``declaration_digest_conflict``.
        """
        params: dict[str, Any] = {}
        if declarations is not None:
            params["declarations"] = dict(declarations)
            params["revision"] = revision if revision is not None else declarations.get("revision")
        elif revision is not None:
            params["revision"] = revision
        raw = await self._announcement_rpc("declarations/apply", params)
        return AnnouncementDeclarationsView.from_dict(raw)

    async def announcement_declarations_get(self) -> AnnouncementDeclarationsView:
        """The applied declarations' revision (0 before any) and digest, and
        where each record they manage stands."""
        raw = await self._announcement_rpc("declarations/get", {})
        return AnnouncementDeclarationsView.from_dict(raw)

    async def announcement_audit_list(
        self,
        *,
        mob_id: str | None = None,
        source_id: str | None = None,
        member: AnnouncementMember | dict[str, str] | None = None,
        action: str | None = None,
        after: int | None = None,
        limit: int | None = None,
    ) -> list[AnnouncementAuditEntry]:
        """A page of a mob's counted source audit entries, by source, member,
        action, or any of them."""
        raw = await self._announcement_rpc(
            "audit/list",
            {},
            {
                "mob_id": mob_id,
                "source_id": source_id,
                "member": _announcement_member(member) if member is not None else None,
                "action": action,
                "after": after,
                "limit": limit,
            },
        )
        rows = raw.get("entries")
        return [AnnouncementAuditEntry.from_dict(row) for row in rows] if isinstance(rows, list) else []

    # -----------------------------------------------------------------
    # Live (realtime) sessions — meta/live/* (mobkit 0.7.31)
    # -----------------------------------------------------------------

    async def live_open(
        self, identity: str, **kwargs: Any
    ) -> dict[str, Any]:
        """Open a realtime (live) channel on a member's session.

        Returns the transport bootstrap::

            {"channel_id": ..., "transport": {"type": "websocket",
             "url": ..., "token": ...}, "capabilities": {...},
             "continuity": {...}}

        The token is single-use with a short TTL — hand the URL to the
        client (device, satellite process) immediately.  Optional kwargs are
        forwarded (e.g. ``model=`` to override the realtime model,
        ``turning_mode=``).
        """
        params: dict[str, Any] = {"identity": identity, **kwargs}
        raw = await self._runtime._rpc("meta/live/open", params)
        return raw if isinstance(raw, dict) else {}

    async def live_open_typed(
        self,
        identity: str,
        execution_identity: LiveExecutionIdentityV1 | None = None,
        **kwargs: Any,
    ) -> LiveChannelHandle | PendingLiveChannelHandle:
        """Open a live channel and return the strict typed channel handle.

        ``execution_identity`` is serialized through the versioned v1
        envelope. Legacy gateways remain readable for ordinary live opens by
        projecting the already-authorized caller target into an absent
        ``target_identity`` response field.
        """
        params: dict[str, Any] = {"identity": identity, **kwargs}
        advertised_feature_capabilities: list[str] | None = None
        if execution_identity is not None:
            forbidden = {
                "mode",
                "execution_mode",
                "profile",
                "profile_id",
                "execution_profile",
                "execution_profile_id",
                "delegation",
                "delegation_type",
                "delegation_model",
                "responses",
                "responses_model",
                "responses_tools",
                "responses_instructions",
                "bridge_model",
                "bridge_tools",
                "bridge_instructions",
                "auth_binding",
                "self_hosted_server_id",
                "provider_params",
                "tools",
                "instructions",
                "member_id",
                "session_id",
            }.intersection(kwargs)
            if forbidden:
                field = sorted(forbidden)[0]
                raise ValueError(
                    f"experimental live/open does not accept {field}"
                )
            capabilities = await self.capabilities()
            advertised_feature_capabilities = capabilities.feature_capabilities
            if not supports_live_execution_identity_v1(
                capabilities.feature_capabilities
            ):
                raise CapabilityUnavailableError(
                    f"capability {LIVE_EXECUTION_IDENTITY_V1} is not available",
                    method="meta/live/open",
                    data={"capability": LIVE_EXECUTION_IDENTITY_V1},
                )
            if "model" in kwargs or "provider" in kwargs:
                raise ValueError(
                    "execution_identity conflicts with legacy top-level model/provider"
                )
            params.update(live_open_execution_identity_params(execution_identity))
        raw = await self._runtime._rpc("meta/live/open", params)
        if not isinstance(raw, dict):
            raise ValueError("meta/live/open returned a non-object channel handle")
        if execution_identity is not None:
            pending = PendingLiveChannelHandle.from_dict(raw)
            if pending.target_identity != identity:
                raise ValueError(
                    "strict meta/live/open returned a different target identity"
                )
            if not supports_live_execution_mode(
                advertised_feature_capabilities or [], pending.execution_mode
            ):
                raise CapabilityUnavailableError(
                    "resolved live execution mode is not advertised",
                    method="meta/live/open",
                    data={"execution_mode": pending.execution_mode},
                )
            return pending
        if "target_identity" not in raw:
            raw = {**raw, "target_identity": identity}
        return LiveChannelHandle.from_dict(raw)

    async def live_playback_owner_register(
        self, pending: PendingLiveChannelHandle
    ) -> LivePlaybackOwnerReadiness:
        """Register local gated playback before WebRTC answer acceptance."""
        raw = await self._runtime._rpc(
            "meta/live/playback_owner/register",
            {
                "identity": pending.target_identity,
                "channel_id": pending.channel_id,
                "pending_receipt": pending.pending_receipt,
            },
        )
        if not isinstance(raw, dict):
            raise ValueError("playback owner registration returned a non-object")
        readiness = LivePlaybackOwnerReadiness.from_dict(raw)
        if readiness.channel_id != pending.channel_id:
            raise ValueError("playback owner readiness channel mismatch")
        return readiness

    async def live_playback_owner_revoke(
        self,
        pending: PendingLiveChannelHandle,
        readiness: LivePlaybackOwnerReadiness,
        active: ActiveLiveChannelHandle | None = None,
    ) -> ExperimentalLiveChannelStatus:
        """Revoke one exact pending or active local playback owner."""
        if readiness.channel_id != pending.channel_id:
            raise ValueError("playback owner readiness channel mismatch")
        params: dict[str, Any] = {
            "identity": pending.target_identity,
            "channel_id": pending.channel_id,
            "pending_receipt": pending.pending_receipt,
            "readiness_receipt": readiness.readiness_receipt,
        }
        if active is not None:
            active = _require_active_live_handle(active)
            if (
                active.channel_id != pending.channel_id
                or active.target_identity != pending.target_identity
                or active.execution_mode != pending.execution_mode
            ):
                raise ValueError("active playback owner does not match pending custody")
            params["activation_receipt"] = active.activation_receipt
        raw = await self._runtime._rpc(
            "meta/live/playback_owner/revoke",
            params,
        )
        if not isinstance(raw, dict):
            raise ValueError("playback owner revoke returned a non-object phase")
        status = ExperimentalLiveChannelStatus.from_dict(raw)
        if status.phase != "revoked":
            raise ValueError("playback owner revoke did not return revoked custody")
        return status

    async def live_experimental_status(
        self,
        handle: PendingLiveChannelHandle | ActiveLiveChannelHandle,
    ) -> ExperimentalLiveChannelStatus:
        params: dict[str, Any] = {
            "identity": handle.target_identity,
            "channel_id": handle.channel_id,
        }
        if isinstance(handle, PendingLiveChannelHandle):
            params["pending_receipt"] = handle.pending_receipt
        else:
            params["activation_receipt"] = handle.activation_receipt
        raw = await self._runtime._rpc("meta/live/status", params)
        if not isinstance(raw, dict):
            raise ValueError("meta/live/status returned a non-object phase")
        status = ExperimentalLiveChannelStatus.from_dict(raw)
        if status.handle is not None:
            if (
                status.handle.channel_id != handle.channel_id
                or status.handle.target_identity != handle.target_identity
                or status.handle.execution_mode != handle.execution_mode
            ):
                raise ValueError("active live handle does not match pending custody")
        return status

    async def live_connect(
        self,
        identity: str,
        execution_identity: LiveExecutionIdentityV1,
        playback_owner: LivePlaybackOwner,
        *,
        activation_poll_interval: float = 0.05,
        activation_attempts: int = 200,
        **kwargs: Any,
    ) -> ActiveLiveChannelConnection:
        """Open and activate one experimental channel in custody order.

        The method returns only after generated active authority exists and the
        local playback owner has been told it may release gated media.
        """
        if activation_poll_interval < 0:
            raise ValueError("activation_poll_interval must be non-negative")
        if activation_attempts <= 0:
            raise ValueError("activation_attempts must be positive")
        pending: PendingLiveChannelHandle | None = None
        try:
            opened = await self.live_open_typed(identity, execution_identity, **kwargs)
            if not isinstance(opened, PendingLiveChannelHandle):
                raise ValueError("experimental live/open did not return a pending handle")
            pending = opened
            if pending.transport.transport != "webrtc" or pending.transport.token is None:
                raise ValueError("experimental live/open did not return a WebRTC bootstrap")
            offer_sdp = await playback_owner.prepare(pending)
            if not isinstance(offer_sdp, str) or not offer_sdp.strip():
                raise ValueError("playback owner returned an invalid SDP offer")
            readiness = await self.live_playback_owner_register(pending)
            answer_sdp = await self.live_webrtc_answer_pending(
                pending, readiness, offer_sdp
            )
            await playback_owner.accept_answer(answer_sdp)
            for _ in range(activation_attempts):
                status = await self.live_experimental_status(pending)
                if status.phase == "active" and status.handle is not None:
                    await playback_owner.activate(status.handle)
                    active = status.handle
                    revocation_task: asyncio.Task[ExperimentalLiveChannelStatus] | None = None

                    async def owner_lost() -> ExperimentalLiveChannelStatus:
                        nonlocal revocation_task
                        if revocation_task is None:
                            revocation_task = asyncio.create_task(
                                self.live_playback_owner_revoke(
                                    pending, readiness, active
                                )
                            )
                        revoked = await asyncio.shield(revocation_task)
                        try:
                            await playback_owner.abort()
                        except Exception as error:
                            _log.warning(
                                "local playback owner cleanup failed after revoke: %s",
                                error,
                            )
                        return revoked

                    connection = ActiveLiveChannelConnection(
                        active,
                        pending.pending_receipt,
                        readiness.readiness_receipt,
                        owner_lost,
                    )
                    wait_for_loss = getattr(playback_owner, "wait_for_loss", None)
                    if callable(wait_for_loss):
                        async def supervise_owner_loss() -> None:
                            try:
                                await wait_for_loss()
                            except BaseException:
                                pass
                            try:
                                await connection.owner_lost()
                            except Exception as error:
                                _log.error(
                                    "failed to revoke lost playback owner: %s",
                                    error,
                                )

                        asyncio.create_task(supervise_owner_loss())
                    return connection
                if status.phase in {"revoked", "closed"}:
                    raise RuntimeError(
                        f"experimental live channel {status.phase} before activation"
                    )
                await asyncio.sleep(activation_poll_interval)
            raise TimeoutError("experimental live channel did not activate")
        except BaseException:
            try:
                await playback_owner.abort()
            finally:
                if pending is not None:
                    try:
                        await self.live_close(pending)
                    except Exception:
                        pass
            raise

    async def live_webrtc_answer(
        self, channel_id: str, token: str, offer_sdp: str
    ) -> str:
        """Complete the one-use WebRTC signaling bootstrap for a live channel."""
        raw = await self._runtime._rpc(
            "live/webrtc/answer",
            {"channel_id": channel_id, "token": token, "offer_sdp": offer_sdp},
        )
        if not isinstance(raw, dict) or not isinstance(raw.get("answer_sdp"), str):
            raise ValueError("live/webrtc/answer returned an invalid answer")
        return raw["answer_sdp"]

    async def live_webrtc_answer_pending(
        self,
        pending: PendingLiveChannelHandle,
        readiness: LivePlaybackOwnerReadiness,
        offer_sdp: str,
    ) -> str:
        """Answer only under exact pending and playback-owner readiness custody."""
        if readiness.channel_id != pending.channel_id:
            raise ValueError("playback owner readiness channel mismatch")
        raw = await self._runtime._rpc(
            "live/webrtc/answer",
            {
                "identity": pending.target_identity,
                "channel_id": pending.channel_id,
                "pending_receipt": pending.pending_receipt,
                "readiness_receipt": readiness.readiness_receipt,
                "token": pending.transport.token,
                "offer_sdp": offer_sdp,
            },
        )
        if not isinstance(raw, dict) or not isinstance(raw.get("answer_sdp"), str):
            raise ValueError("live/webrtc/answer returned an invalid answer")
        return raw["answer_sdp"]

    async def live_replacement_required(
        self, active: ActiveLiveChannelHandle
    ) -> LiveReplacementRequired:
        """Read pending replacement signaling under current active authority.

        This method never answers the offer or starts renegotiation
        automatically. A lost response can be retried until the fresh channel
        binds, after which the pending bootstrap disappears.
        """
        active = _require_active_live_handle(active)
        raw = await self._runtime._rpc(
            "meta/live/replacement_required",
            {
                "identity": active.target_identity,
                "channel_id": active.channel_id,
                "activation_receipt": active.activation_receipt,
            },
        )
        if not isinstance(raw, dict):
            raise ValueError(
                "meta/live/replacement_required returned a non-object result"
            )
        return LiveReplacementRequired.from_dict(raw)

    async def live_outputs(
        self,
        active: ActiveLiveChannelHandle,
        *,
        capacity: int = 16,
    ) -> AsyncIterator[LiveAssistantOutputAddress]:
        """Yield opaque output addresses with bounded, loss-intolerant delivery.

        The iterator must be active before output begins. Queue overflow or a
        missing consumer rejects the server callback. Closing the iterator
        closes the exact channel so pending output remains Unmeasured.
        """
        active = _require_active_live_handle(active)
        queue = self._runtime._dispatcher.register_live_output_queue(
            active.channel_id, capacity
        )
        try:
            while True:
                yield await queue.get()
        finally:
            self._runtime._dispatcher.unregister_live_output_queue(
                active.channel_id, queue
            )
            await self.live_close(active)

    async def live_playback_complete(
        self,
        active: ActiveLiveChannelHandle,
        output: LiveAssistantOutputAddress,
    ) -> LivePlaybackCompleteResult:
        """Report measured playback completion for an exact opaque output."""
        active = _require_active_live_handle(active)
        if output.channel_id != active.channel_id:
            raise ValueError("assistant output does not belong to active channel")
        raw = await self._runtime._rpc(
            "meta/live/playback_complete",
            {
                "identity": active.target_identity,
                "channel_id": active.channel_id,
                "activation_receipt": active.activation_receipt,
                "output_id": output.output_id,
            },
        )
        if not isinstance(raw, dict):
            raise ValueError(
                "meta/live/playback_complete returned a non-object result"
            )
        return LivePlaybackCompleteResult.from_dict(raw)

    async def live_status(self, identity: str, **kwargs: Any) -> dict[str, Any]:
        """Status of the member's live channel (open/closed, channel id)."""
        params: dict[str, Any] = {"identity": identity, **kwargs}
        raw = await self._runtime._rpc("meta/live/status", params)
        return raw if isinstance(raw, dict) else {}

    async def live_close(
        self,
        handle: str | PendingLiveChannelHandle | ActiveLiveChannelHandle,
        **kwargs: Any,
    ) -> dict[str, Any]:
        """Close an ordinary channel ID or an exact experimental handle."""
        if isinstance(handle, PendingLiveChannelHandle):
            params: dict[str, Any] = {
                "identity": handle.target_identity,
                "channel_id": handle.channel_id,
                "pending_receipt": handle.pending_receipt,
                **kwargs,
            }
        elif isinstance(handle, ActiveLiveChannelHandle):
            params = {
                "identity": handle.target_identity,
                "channel_id": handle.channel_id,
                "activation_receipt": handle.activation_receipt,
                **kwargs,
            }
        else:
            params = {"channel_id": handle, **kwargs}
        raw = await self._runtime._rpc("meta/live/close", params)
        return raw if isinstance(raw, dict) else {}

    async def live_send_input_image(
        self,
        identity: str,
        idempotency_key: str,
        mime: str,
        data_base64: str,
        **kwargs: Any,
    ) -> dict[str, Any]:
        """Send a still image into the member's open live channel (meerkat
        0.7.27+).  ``idempotency_key`` must be caller-stable within the
        session — retries with the same key are exact-retry deduplicated.
        """
        params: dict[str, Any] = {
            "identity": identity,
            "chunk": {
                "kind": "image",
                "idempotency_key": idempotency_key,
                "mime": mime,
                "data": data_base64,
            },
            **kwargs,
        }
        raw = await self._runtime._rpc("meta/live/send_input", params)
        return raw if isinstance(raw, dict) else {}

    async def live_refresh(self, identity: str, **kwargs: Any) -> dict[str, Any]:
        """Push refreshed mutable config (instructions/tools/audio) into an
        open live channel without rebuilding the transport.  Model/provider
        swaps require close + reopen."""
        params: dict[str, Any] = {"identity": identity, **kwargs}
        raw = await self._runtime._rpc("meta/live/refresh", params)
        return raw if isinstance(raw, dict) else {}

    async def live_truncate(
        self,
        active: ActiveLiveChannelHandle,
        output: LiveAssistantOutputAddress,
        audio_played_ms: int,
        **kwargs: Any,
    ) -> dict[str, Any]:
        """Truncate an exact opaque output at measured playback progress."""
        active = _require_active_live_handle(active)
        if output.channel_id != active.channel_id:
            raise ValueError("assistant output does not belong to active channel")
        params: dict[str, Any] = {
            "identity": active.target_identity,
            "channel_id": active.channel_id,
            "activation_receipt": active.activation_receipt,
            "output_id": output.output_id,
            "audio_played_ms": audio_played_ms,
            **kwargs,
        }
        raw = await self._runtime._rpc("meta/live/truncate", params)
        return raw if isinstance(raw, dict) else {}

    async def live_refresh_active(
        self, active: ActiveLiveChannelHandle, **kwargs: Any
    ) -> dict[str, Any]:
        active = _require_active_live_handle(active)
        params = {
            "identity": active.target_identity,
            "channel_id": active.channel_id,
            "activation_receipt": active.activation_receipt,
            **kwargs,
        }
        raw = await self._runtime._rpc("meta/live/refresh", params)
        return raw if isinstance(raw, dict) else {}

    async def live_send_input_image_active(
        self,
        active: ActiveLiveChannelHandle,
        idempotency_key: str,
        mime: str,
        data_base64: str,
    ) -> dict[str, Any]:
        active = _require_active_live_handle(active)
        raw = await self._runtime._rpc(
            "meta/live/send_input",
            {
                "identity": active.target_identity,
                "channel_id": active.channel_id,
                "activation_receipt": active.activation_receipt,
                "chunk": {
                    "kind": "image",
                    "idempotency_key": idempotency_key,
                    "mime": mime,
                    "data": data_base64,
                },
            },
        )
        return raw if isinstance(raw, dict) else {}

    async def live_commit_input_active(
        self, active: ActiveLiveChannelHandle, **kwargs: Any
    ) -> dict[str, Any]:
        active = _require_active_live_handle(active)
        raw = await self._runtime._rpc(
            "meta/live/commit_input",
            {
                "identity": active.target_identity,
                "channel_id": active.channel_id,
                "activation_receipt": active.activation_receipt,
                **kwargs,
            },
        )
        return raw if isinstance(raw, dict) else {}

    async def live_interrupt_active(
        self, active: ActiveLiveChannelHandle
    ) -> dict[str, Any]:
        active = _require_active_live_handle(active)
        raw = await self._runtime._rpc(
            "meta/live/interrupt",
            {
                "identity": active.target_identity,
                "channel_id": active.channel_id,
                "activation_receipt": active.activation_receipt,
            },
        )
        return raw if isinstance(raw, dict) else {}

    # -----------------------------------------------------------------
    # Cross-mob operations
    # -----------------------------------------------------------------

    async def wire_cross_mob(
        self,
        local_member_id: str,
        remote_member_id: str,
        remote_handle: "MobHandle",
    ) -> None:
        """Wire a local member to a member on another mob handle.

        Uses peer_info + wire_local on both sides.  Both mob handles must
        live in the **same process** — ``peer_info()`` always returns an
        ``inproc://`` address, which is only reachable within the same
        process.  For cross-process peering, call ``wire_local()`` on each
        handle directly, supplying the remote gateway's routable TCP or UDS
        address in place of the inproc address from ``peer_info()``.

        Args:
            local_member_id: Member on this mob to wire.
            remote_member_id: Member on the remote mob to wire.
            remote_handle: MobHandle for the remote mob (must be same-process).
        """
        local_info = await self.peer_info(local_member_id)
        remote_info = await remote_handle.peer_info(remote_member_id)
        await self.wire_local(
            local_member_id,
            remote_info["comms_name"],
            remote_info["peer_id"],
            remote_info["address"],
        )
        try:
            await remote_handle.wire_local(
                remote_member_id,
                local_info["comms_name"],
                local_info["peer_id"],
                local_info["address"],
            )
        except Exception:
            # Best-effort rollback: undo the local wire using the same
            # low-level API — no contact directory or peer handles needed.
            try:
                await self.unwire_local(
                    local_member_id,
                    remote_info["comms_name"],
                    remote_info["peer_id"],
                    remote_info["address"],
                )
            except Exception:
                pass
            raise

    async def send_cross_mob(
        self,
        remote_member_id: str,
        remote_handle: "MobHandle",
        message: str | None = None,
        *,
        content: list[dict[str, Any]] | None = None,
    ) -> SendMessageResult:
        """Send a message to a member on another mob handle.

        This is an app-level injection — the remote agent receives the
        message but does not know the sender. For agent-to-agent
        communication with sender identity and reply path, use
        ``wire_cross_mob()`` to set up peering, then agents communicate
        directly via their comms ``send`` tool.

        Args:
            remote_member_id: Target member on the remote mob.
            remote_handle: MobHandle for the remote mob.
            message: Plain text message.
            content: Multimodal content blocks.
        """
        return await remote_handle.send(remote_member_id, message, content=content)

    async def list_external_mobs(self) -> list:
        """List known external mobs from the contact directory."""
        from .types import CrossMobContactEntry

        raw = await self._runtime._rpc("meta/cross_mob/directory")
        mobs = raw.get("mobs", []) if isinstance(raw, dict) else []
        return [CrossMobContactEntry.from_dict(m) for m in mobs]

    async def peer_info(self, member_id: str) -> dict[str, str]:
        """Get comms peer info for a local member.

        Returns ``{"member_id", "mob_id", "comms_name", "peer_id", "address"}``.
        Used to build peer specs for cross-mob wiring via ``wire_local``.
        """
        raw = await self._runtime._rpc(
            "meta/cross_mob/peer_info",
            {"member_id": member_id},
        )
        return raw if isinstance(raw, dict) else {}

    async def peer_pubkey(self) -> str:
        """Return the local gateway's Ed25519 signing pubkey, base64.

        Used to bootstrap trust before populating a peer meta's
        contact directory with this gateway's pubkey. Raises
        :class:`CapabilityUnavailableError` from the underlying
        transport if the local gateway is inproc-only and never
        configured a keypair.
        """
        raw = await self._runtime._rpc("meta/peer_pubkey")
        if isinstance(raw, dict):
            value = raw.get("pubkey_b64")
            if isinstance(value, str):
                return value
        return ""

    async def wire_local(
        self,
        local_member_id: str,
        remote_comms_name: str,
        remote_peer_id: str,
        remote_address: str,
        *,
        remote_pubkey_b64: str | None = None,
    ) -> None:
        """Wire a local member to a remote peer (local side only).

        For same-process (inproc) cross-mob wiring::

            # Get peer info from each side
            a_info = await core.peer_info("research")
            b_info = await gw.peer_info("calendar")

            # Wire each side to the other (inproc address from peer_info)
            await core.wire_local("research", b_info["comms_name"], b_info["peer_id"], b_info["address"])
            await gw.wire_local("calendar", a_info["comms_name"], a_info["peer_id"], a_info["address"])

        For cross-process (TCP/UDS), replace the address with the remote
        gateway's transport endpoint — peer_info always returns inproc.
        Pass ``remote_pubkey_b64`` (base64 of the peer gateway's Ed25519
        verifying key, fetched via :meth:`peer_pubkey`) to stamp a real
        signing pubkey on the descriptor; the gateway rejects non-inproc
        wires without one.
        """
        params: dict[str, str] = {
            "local_member_id": local_member_id,
            "remote_comms_name": remote_comms_name,
            "remote_peer_id": remote_peer_id,
            "remote_address": remote_address,
        }
        if remote_pubkey_b64 is not None:
            params["remote_pubkey_b64"] = remote_pubkey_b64
        await self._runtime._rpc("meta/cross_mob/wire_local", params)

    async def unwire_local(
        self,
        local_member_id: str,
        remote_comms_name: str,
        remote_peer_id: str,
        remote_address: str,
        *,
        remote_pubkey_b64: str | None = None,
    ) -> None:
        """Undo a wire_local — unwire a local member from a previously wired peer (local side only)."""
        params: dict[str, str] = {
            "local_member_id": local_member_id,
            "remote_comms_name": remote_comms_name,
            "remote_peer_id": remote_peer_id,
            "remote_address": remote_address,
        }
        if remote_pubkey_b64 is not None:
            params["remote_pubkey_b64"] = remote_pubkey_b64
        await self._runtime._rpc("meta/cross_mob/unwire_local", params)

    # -----------------------------------------------------------------
    # Rich member inspection
    # -----------------------------------------------------------------

    async def member_status(self, member_id: str) -> RichMemberSnapshot:
        """Return rich execution status for a member."""
        from .types import RichMemberSnapshot
        raw = await self._runtime._rpc("meta/member_status", {"member_id": member_id})
        return RichMemberSnapshot.from_dict(raw)

    async def identity_resolved_tools(self, identity: str) -> list[str]:
        """Return the tools currently visible to an identity's live session."""
        raw = await self._runtime._rpc(
            "meta/identity/resolved_tools",
            {"identity": identity},
        )
        return [str(tool) for tool in raw.get("tools", [])]

    async def identity_resolved_tools_detail(self, identity: str):
        """Return the full resolved-tools diagnostic payload for an identity."""
        from .types import IdentityResolvedToolsResult
        raw = await self._runtime._rpc(
            "meta/identity/resolved_tools",
            {"identity": identity},
        )
        return IdentityResolvedToolsResult.from_dict(raw)

    async def identity_routing_status(self, identity: str):
        """Return meerkat's typed model-routing status for an identity's session.

        Raises through the transport if the runtime machine does not hold the
        session; an absent status is deliberately not softened into an empty
        one, because an empty status is indistinguishable on the wire from a
        genuinely pre-hydration session.
        """
        from .types import IdentityRoutingStatusResult
        raw = await self._runtime._rpc(
            "meta/identity/routing_status",
            {"identity": identity},
        )
        return IdentityRoutingStatusResult.from_dict(raw)

    async def force_cancel_member(self, member_id: str) -> None:
        """Force-cancel a running member immediately.

        A mob refusal raises with meerkat's typed code and data; a member
        whose runtime registration must be reloaded first raises
        :class:`MemberReloadRequiredError`.
        """
        await self._runtime._rpc("meta/force_cancel_member", {"member_id": member_id})

    async def stop_member_run(
        self, member_id: str, run_id: str, *, reason: str
    ) -> MemberRunStopReceipt:
        """Stop one exact run of a member (meerkat's run-fenced Stop).

        The member's runtime stops ``run_id`` and terminalizes every input
        already bound to it, including durable steers that joined the run.
        Returns the typed :class:`MemberRunStopReceipt` (``stopped`` with
        contributors, ``not_current``, or ``not_stoppable``). Raises
        ``ValueError`` on a malformed receipt.
        """
        raw = await self._runtime._rpc(
            "meta/stop_member_run",
            {"member_id": member_id, "run_id": run_id, "reason": reason},
        )
        receipt = raw.get("receipt") if isinstance(raw, dict) else None
        return MemberRunStopReceipt.from_dict(receipt)

    # -----------------------------------------------------------------
    # Helper convenience
    # -----------------------------------------------------------------

    async def spawn_helper(
        self,
        agent_identity: str,
        task: str,
        *,
        result_label: str,
        max_text_bytes: int,
        role: str | None = None,
        runtime_mode: str | None = None,
        backend: str | None = None,
        transport_timeout: float | None = None,
    ) -> HelperResult:
        """Spawn a short-lived helper member and return its bounded result.

        ``result_label`` and ``max_text_bytes`` are required by meerkat
        0.8.22's exact helper contract; the byte bound is validated
        upstream before admission.

        The gateway replies only after it has provisioned the helper, the
        helper's turn has finished and its retirement has been attempted, and
        it sets no deadline of its own on any of that. ``transport_timeout``
        (seconds) sets how long this call waits for the reply; it defaults to
        the transport's request timeout (60 seconds for the stock transport).
        Size it for all three steps, with margin. It bounds only this client's
        wait: it is not sent to the gateway, and the helper is not stopped
        when the wait runs out.

        When the wait runs out, the stock transport raises
        ``RuntimeError("persistent transport: timeout after <n>s waiting for
        response")`` and the helper's outcome is unknown: it may still be
        running and may still finish, and its reply is dropped. Do not retry
        blindly; reconcile the original helper's status and effects first. A
        retry under a fresh ``agent_identity`` is a new operation that starts
        another helper: it neither cancels nor deduplicates the first. A retry
        under the same ``agent_identity`` queues behind the first helper on
        identity-first gateways.
        """
        from .types import HelperResult
        params: dict[str, Any] = {
            "agent_identity": agent_identity,
            "task": task,
            "result_label": result_label,
            "max_text_bytes": max_text_bytes,
        }
        options: dict[str, Any] = {}
        if role is not None:
            options["role"] = role
        if runtime_mode is not None:
            options["runtime_mode"] = runtime_mode
        if backend is not None:
            options["backend"] = backend
        if options:
            params["options"] = options
        raw = await self._runtime._rpc(
            "meta/spawn_helper",
            params,
            transport_timeout=transport_timeout,
        )
        return HelperResult.from_dict(raw)

    async def fork_helper(
        self,
        source_member_id: str,
        agent_identity: str,
        task: str,
        *,
        result_label: str,
        max_text_bytes: int,
        fork_context: dict | None = None,
        role: str | None = None,
        runtime_mode: str | None = None,
        backend: str | None = None,
        transport_timeout: float | None = None,
    ) -> HelperResult:
        """Fork a helper from an existing member's context.

        ``result_label`` and ``max_text_bytes`` are required by meerkat
        0.8.22's exact helper contract; the byte bound is validated
        upstream before admission.

        ``transport_timeout`` works as in :meth:`spawn_helper`: it bounds only
        this client's wait for the forked helper's result, and the same sizing
        applies. When it runs out the forked helper's outcome is unknown and it
        may still be running; the retry advice of :meth:`spawn_helper` applies.
        """
        from .types import HelperResult
        params: dict[str, Any] = {
            "source_member_id": source_member_id,
            "agent_identity": agent_identity,
            "task": task,
            "result_label": result_label,
            "max_text_bytes": max_text_bytes,
        }
        if fork_context is not None:
            params["fork_context"] = fork_context
        options: dict[str, Any] = {}
        if role is not None:
            options["role"] = role
        if runtime_mode is not None:
            options["runtime_mode"] = runtime_mode
        if backend is not None:
            options["backend"] = backend
        if options:
            params["options"] = options
        raw = await self._runtime._rpc(
            "meta/fork_helper",
            params,
            transport_timeout=transport_timeout,
        )
        return HelperResult.from_dict(raw)

    # -----------------------------------------------------------------
    # Session attachment
    # -----------------------------------------------------------------

    async def attach_session(
        self,
        role: str,
        agent_identity: str,
        session_id: str,
    ) -> RichMemberSnapshot:
        """Attach a member to an existing session (resume mode)."""
        from .types import RichMemberSnapshot
        params: dict[str, Any] = {
            "role": role,
            "agent_identity": agent_identity,
            "session_id": session_id,
        }
        raw = await self._runtime._rpc("meta/attach_existing_session", params)
        return RichMemberSnapshot.from_dict(raw)

    # -----------------------------------------------------------------
    # Flow lifecycle
    # -----------------------------------------------------------------

    async def cancel_flow(self, run_id: str) -> None:
        """Cancel a running flow by run ID."""
        await self._runtime._rpc("meta/cancel_flow", {"run_id": run_id})

    async def flow_status(self, run_id: str) -> MobRunSnapshot | None:
        """Get flow run status. Returns None if run not found."""
        from .types import MobRunSnapshot
        raw = await self._runtime._rpc("meta/flow_status", {"run_id": run_id})
        if raw is None:
            return None
        if isinstance(raw, dict) and raw.get("status") == "not_found":
            return None
        return MobRunSnapshot.from_dict(raw)

    async def list_flows(self) -> list[str]:
        """List all configured flow IDs in this mob definition.

        Relays meerkat 0.6's ``MobHandle::list_flows``. Returns the flow IDs
        declared by the mob's ``[flows.*]`` tables, in unspecified order.
        """
        raw = await self._runtime._rpc("meta/list_flows")
        if isinstance(raw, dict):
            flows = raw.get("flows", [])
        elif isinstance(raw, list):
            flows = raw
        else:
            flows = []
        return [str(flow_id) for flow_id in flows]

    async def list_runs(self, flow_id: str | None = None) -> list[MobRun]:
        """List flow runs for this mob.

        Relays ``MobHandle::list_runs``. When ``flow_id`` is given,
        only runs for that flow are returned. The returned :class:`MobRun`
        carries the full meerkat ledger projection — ``step_ledger``,
        ``failure_ledger``, ``frames``, ``loops``, ``loop_iteration_ledger``,
        ``flow_state``, ``activation_params``, etc. — verbatim from the
        on-the-wire JSON.
        """
        from .types import MobRun
        params: dict[str, Any] = {}
        if flow_id is not None:
            params["flow_id"] = flow_id
        raw = await self._runtime._rpc("meta/list_runs", params)
        runs: list[Any] = []
        if isinstance(raw, dict):
            maybe = raw.get("runs")
            if isinstance(maybe, list):
                runs = maybe
        elif isinstance(raw, list):
            runs = raw
        return [MobRun.from_dict(r) for r in runs if isinstance(r, dict)]

    async def run_flow(
        self,
        flow_id: str,
        params: Any = None,
        *,
        role_targets: Mapping[str, Sequence[str]] | None = None,
        idempotency_key: str | None = None,
    ) -> str:
        """Start a flow run and return its run ID.

        Relays meerkat 0.6's ``MobHandle::run_flow``. ``params`` is forwarded
        verbatim as the flow's activation params (any JSON value, defaults to
        ``None``). The returned ``run_id`` can be passed to
        :meth:`flow_status` and :meth:`cancel_flow`.

        ``role_targets`` binds every role the flow's steps use to existing
        members of this mob, for example ``{"reviewer": ["rev-a1",
        "rev-a2"]}``. Each step then dispatches only to the bound members of
        its role that are still active, never to another member of the role,
        and the run fails (``insufficient_targets``) when they can no longer
        meet the step's collection policy. The gateway must advertise the
        ``flow_role_targets`` feature capability: without it this raises
        :class:`CapabilityUnavailableError` and starts nothing, since an
        older gateway would ignore the binding and run the flow role-wide. A
        binding the mob refuses raises :class:`FlowRoleTargetsRefusedError`.
        A run the gateway started without echoing the same binding is
        canceled, and :class:`FlowRoleTargetsNotAppliedError` is raised.

        ``idempotency_key`` admits the run once under a caller key: a replay
        with the same key, ``params`` and ``role_targets`` returns the same
        run, also when the first answer was lost, and the same key with other
        params or another binding raises
        :class:`FlowRunIdempotencyConflictError` without a second run. The
        gateway must advertise the ``flow_run_idempotency_key`` feature
        capability: without it this raises :class:`CapabilityUnavailableError`
        and starts nothing, since an older gateway would ignore the key.
        """
        rpc_params: dict[str, Any] = {"flow_id": flow_id, "params": params}
        requested: dict[str, list[str]] | None = None
        if role_targets is not None:
            requested = _role_targets_request(role_targets)
        if idempotency_key is not None and not isinstance(idempotency_key, str):
            raise TypeError("idempotency_key must be a string")
        if requested is not None or idempotency_key is not None:
            capabilities = await self.capabilities()
            missing = None
            if requested is not None and not supports_flow_role_targets(capabilities):
                missing = FLOW_ROLE_TARGETS
            elif idempotency_key is not None and not supports_flow_run_idempotency_key(
                capabilities
            ):
                missing = FLOW_RUN_IDEMPOTENCY_KEY
            if missing is not None:
                raise CapabilityUnavailableError(
                    f"capability {missing} is not available",
                    method="meta/run_flow",
                    data={"capability": missing},
                )
        if requested is not None:
            rpc_params["role_targets"] = requested
        if idempotency_key is not None:
            rpc_params["idempotency_key"] = idempotency_key
        raw = await self._runtime._rpc("meta/run_flow", rpc_params)
        run_id = raw.get("run_id") if isinstance(raw, dict) else None
        if not isinstance(run_id, str):
            raise RuntimeError(f"unexpected run_flow response: {raw!r}")
        if requested is not None:
            echoed = raw.get("role_targets")
            if not _role_targets_echoed(requested, echoed):
                cancel_error: BaseException | None = None
                try:
                    await self.cancel_flow(run_id)
                except Exception as error:  # reported on the raised error
                    cancel_error = error
                raise FlowRoleTargetsNotAppliedError(
                    f"meta/run_flow started run {run_id} without echoing its role_targets; "
                    + (
                        "the run was canceled"
                        if cancel_error is None
                        else f"canceling it failed: {cancel_error}"
                    ),
                    run_id=run_id,
                    requested=requested,
                    echoed=echoed,
                    cancel_error=cancel_error,
                )
        return run_id

    # -----------------------------------------------------------------
    # Mob/run labels — meta-side sidecar metadata
    # -----------------------------------------------------------------

    async def set_mob_labels(self, labels: dict[str, str]) -> None:
        """Replace the label set associated with this mob.

        Meta owns these labels — they are not part of meerkat-mob.
        Replacement is wholesale; existing labels not present in
        ``labels`` are dropped. Pass ``{}`` to clear.
        """
        await self._runtime._rpc("meta/mob_labels/set", {"labels": dict(labels)})

    async def get_mob_labels(self) -> dict[str, str]:
        """Return the label set associated with this mob (or ``{}``)."""
        raw = await self._runtime._rpc("meta/mob_labels/get")
        if isinstance(raw, dict):
            labels = raw.get("labels", {})
            if isinstance(labels, dict):
                return {str(k): str(v) for k, v in labels.items()}
        return {}

    async def delete_mob_labels(self) -> None:
        """Remove the label set associated with this mob."""
        await self._runtime._rpc("meta/mob_labels/delete")

    async def set_run_labels(self, run_id: str, labels: dict[str, str]) -> None:
        """Replace the label set associated with ``run_id`` under this mob.

        Replacement is wholesale (see :meth:`set_mob_labels`).
        """
        await self._runtime._rpc(
            "meta/run_labels/set", {"run_id": run_id, "labels": dict(labels)}
        )

    async def get_run_labels(self, run_id: str) -> dict[str, str]:
        """Return the label set for ``run_id`` (or ``{}``)."""
        raw = await self._runtime._rpc("meta/run_labels/get", {"run_id": run_id})
        if isinstance(raw, dict):
            labels = raw.get("labels", {})
            if isinstance(labels, dict):
                return {str(k): str(v) for k, v in labels.items()}
        return {}

    async def delete_run_labels(self, run_id: str) -> None:
        """Remove the label set for ``run_id``."""
        await self._runtime._rpc("meta/run_labels/delete", {"run_id": run_id})

    # -----------------------------------------------------------------
    # Batch
    # -----------------------------------------------------------------

    async def collect_completed(self) -> list[tuple[str, RichMemberSnapshot]]:
        """Collect all members that have reached a final state."""
        from .types import RichMemberSnapshot
        raw = await self._runtime._rpc("meta/collect_completed")
        results: list[tuple[str, RichMemberSnapshot]] = []
        entries = raw.get("completed", []) if isinstance(raw, dict) else raw if isinstance(raw, list) else []
        for entry in entries:
            member_id = entry.get("member_id", "")
            snapshot = RichMemberSnapshot.from_dict(entry.get("snapshot", entry))
            results.append((member_id, snapshot))
        return results

    # -----------------------------------------------------------------
    # Server-side readiness
    # -----------------------------------------------------------------

    async def wait_ready(
        self,
        *,
        timeout: float | None = None,
    ) -> dict[str, Any]:
        """Wait until all current mob members are startup-ready for orchestration.

        Relays meerkat 0.6's ``MobHandle::wait_for_ready``. Returns a dict
        ``{"ready": [{"agent_identity", "snapshot"}], "timeout": bool}``.
        ``timeout=True`` means partial readiness within the deadline; the
        ``ready`` list will be empty in that case.

        :param timeout: optional seconds to wait. Omit/``None`` waits up to a
            generous server-side default ceiling (~10 minutes); pass an explicit
            value to override. The wait returns as soon as members converge — the
            ceiling is only the wall before ``timeout=True`` is reported.
        """
        params: dict[str, Any] = {}
        if timeout is not None:
            params["timeout_ms"] = int(timeout * 1000)
        raw = await self._runtime._rpc("meta/wait_ready", params)
        if not isinstance(raw, dict):
            return {"ready": [], "timeout": False}
        return {
            "ready": list(raw.get("ready", [])),
            "timeout": bool(raw.get("timeout", False)),
        }

    # -----------------------------------------------------------------
    # Polling helpers (client-side)
    # -----------------------------------------------------------------

    async def wait_one(
        self,
        member_id: str,
        *,
        poll_interval: float = 1.0,
        timeout: float | None = None,
    ) -> RichMemberSnapshot:
        """Poll member_status until the member reaches a final state.

        Raises ``TimeoutError`` if *timeout* seconds elapse before completion.
        """
        import asyncio
        import time

        from .types import RichMemberSnapshot

        deadline = time.monotonic() + timeout if timeout is not None else None
        while True:
            snapshot = await self.member_status(member_id)
            if snapshot.is_final:
                return snapshot
            if deadline is not None and time.monotonic() >= deadline:
                raise TimeoutError(
                    f"member {member_id!r} did not reach final state "
                    f"within {timeout}s"
                )
            await asyncio.sleep(poll_interval)

    async def wait_all(
        self,
        member_ids: list[str],
        *,
        poll_interval: float = 1.0,
        timeout: float | None = None,
    ) -> list[RichMemberSnapshot]:
        """Wait for all listed members to reach final state.

        Polls in parallel via ``asyncio.gather``. Raises ``TimeoutError``
        if *timeout* seconds elapse before all members complete.
        """
        import asyncio

        from .types import RichMemberSnapshot

        tasks = [
            self.wait_one(mid, poll_interval=poll_interval, timeout=timeout)
            for mid in member_ids
        ]
        return list(await asyncio.gather(*tasks))

    # Alias for backward compatibility
    send_message = send

    async def subscribe_agent(self, member_id: str) -> AsyncIterator[AgentEvent]:
        """Stream events for one agent. Pure observation."""
        bridge = self._runtime.sse_bridge()
        async for event in bridge.agent_events(member_id):
            yield AgentEvent.from_sse(event, agent_id=member_id)

    async def subscribe_mob(self) -> AsyncIterator[MobEvent]:
        """Stream mob-wide events. Pure observation."""
        bridge = self._runtime.sse_bridge()
        async for event in bridge.mob_events():
            yield MobEvent.from_sse(event)


class ToolCaller:
    """Bound callable scoped to one MCP module.

    Wraps ``MobHandle.call_tool`` with a fixed ``module_id`` and unwraps
    the result so callers get raw data instead of ``CallToolResult``.
    """

    def __init__(self, mob_handle: MobHandle, module_id: str) -> None:
        self._mob_handle = mob_handle
        self._module_id = module_id

    async def __call__(self, tool: str, **kwargs: Any) -> Any:
        """Call a tool on the bound MCP module, return unwrapped result.

        Raises whatever ``call_tool`` raises on failure (e.g. ``RpcError``).
        """
        result = await self._mob_handle.call_tool(self._module_id, tool, kwargs or None)
        return result.result


class SseBridge:
    """Bridge for streaming SSE from the Rust backend's HTTP server."""

    def __init__(self, runtime: MetaRuntime):
        self._runtime = runtime

    def _base_url(self) -> str:
        base = self._runtime.rust_http_base_url
        if base is None:
            raise NotConnectedError(
                "SSE bridge requires rust_http_base_url — set it via "
                "runtime.set_rust_http_base('http://127.0.0.1:8081') or "
                "ensure the Rust binary reports it during bootstrap"
            )
        return base

    async def agent_events(self, agent_id: str) -> AsyncIterator[SseEvent]:
        url = f"{self._base_url()}/agents/{agent_id}/events"
        async for event in self._stream_sse(url):
            yield event

    async def mob_events(self) -> AsyncIterator[SseEvent]:
        url = f"{self._base_url()}/mob/events"
        async for event in self._stream_sse(url):
            yield event

    async def _stream_sse(
        self,
        url: str,
        *,
        method: str = "GET",
        body: bytes | None = None,
    ) -> AsyncIterator[SseEvent]:
        async def _read_chunks() -> AsyncIterator[bytes]:
            req = urllib_request.Request(url, method=method, data=body)
            req.add_header("Accept", "text/event-stream")
            if body is not None:
                req.add_header("Content-Type", "application/json")
            response = await asyncio.to_thread(urllib_request.urlopen, req)
            try:
                while True:
                    chunk = await asyncio.to_thread(response.read, 4096)
                    if not chunk:
                        break
                    yield chunk
            finally:
                response.close()

        async for event in parse_sse_stream(_read_chunks()):
            yield event


class AsgiApp:
    """ASGI app: REST handled directly, SSE proxied to Rust backend."""

    def __init__(
        self,
        runtime: MetaRuntime,
        console: bool = True,
        auth_config: Any | None = None,
        fallback_app: Any | None = None,
    ):
        if auth_config is not None:
            config_dict = _auth_config_to_dict(auth_config)
            if config_dict.get("provider") == "google":
                raise ValueError(
                    "GoogleAuthConfig cannot be used with the Python ASGI facade — "
                    "asymmetric OIDC/JWKS verification requires the Rust gateway. "
                    "Use auth=auth.jwt(...) for direct ASGI deployments, or route "
                    "through the Rust gateway for Google auth."
                )
        self._runtime = runtime
        self._console = console
        self._auth_config = auth_config
        self._fallback_app = self._normalize_fallback(fallback_app)

    @staticmethod
    def _normalize_fallback(app: Any) -> Any:
        if app is None:
            return None
        if callable(app):
            return app
        if isinstance(app, list):
            try:
                from starlette.applications import Starlette
            except ImportError:
                raise ImportError(
                    "extra_routes is a list of Route objects but starlette is not installed. "
                    "Install starlette or pass an ASGI app directly."
                )
            return Starlette(routes=app)
        return app

    async def __call__(self, scope: dict[str, Any], receive: Any, send: Any) -> None:
        if scope["type"] != "http":
            if self._fallback_app is not None:
                await self._fallback_app(scope, receive, send)
            return

        path: str = scope.get("path", "/")
        method: str = scope.get("method", "GET")

        if path == "/healthz":
            await _send_response(send, 200, b"ok", content_type=b"text/plain")
            return

        # Gate console/observation routes when console=False.
        if not self._console:
            is_console_route = (
                path.startswith("/console")
                or path == "/mob/events"
                or (path.startswith("/agents/") and path.endswith("/events"))
            )
            if is_console_route:
                await _send_response(send, 404, b'{"error":"not found"}')
                return

        # Enforce auth when auth_config is provided.
        if self._auth_config is not None and path != "/healthz":
            headers = dict(scope.get("headers", []))
            auth_header = headers.get(
                b"authorization", b""
            ).decode("utf-8", errors="replace")
            if not auth_header.startswith("Bearer ") or not auth_header[7:].strip():
                resp = json.dumps({"error": "unauthorized"}).encode()
                await _send_response(send, 401, resp)
                return
            token = auth_header[7:].strip()
            if not _validate_bearer_token(token, self._auth_config):
                resp = json.dumps({"error": "unauthorized", "reason": "invalid_token"}).encode()
                await _send_response(send, 401, resp)
                return

        if path == "/rpc" and method == "POST":
            body = await _read_body(receive)
            request_id = None
            try:
                parsed = json.loads(body)
            except (json.JSONDecodeError, ValueError) as exc:
                resp = json.dumps({
                    "jsonrpc": "2.0",
                    "id": None,
                    "error": {"code": -32700, "message": f"Parse error: {exc}"},
                }).encode()
                await _send_response(send, 200, resp)
                return
            if not isinstance(parsed, dict) or "method" not in parsed:
                resp = json.dumps({
                    "jsonrpc": "2.0",
                    "id": parsed.get("id") if isinstance(parsed, dict) else None,
                    "error": {"code": -32600, "message": "Invalid Request: must be a JSON object with a method field"},
                }).encode()
                await _send_response(send, 200, resp)
                return
            request_id = parsed.get("id")
            try:
                result = await self._runtime._rpc(
                    parsed.get("method", ""),
                    parsed.get("params"),
                )
                resp = json.dumps({
                    "jsonrpc": "2.0",
                    "id": request_id,
                    "result": result,
                }).encode()
                await _send_response(send, 200, resp)
            except RpcError as exc:
                # Pre-fix this used `exc.message`, an attribute that
                # never existed on `RpcError`. The handler then fell
                # through to the generic `except Exception` arm and
                # returned -32603 with an `AttributeError` message,
                # masking the real RPC code/message.
                error_payload: dict[str, Any] = {"code": exc.code, "message": str(exc)}
                if exc.data is not None:
                    error_payload["data"] = exc.data
                resp = json.dumps({
                    "jsonrpc": "2.0",
                    "id": request_id,
                    "error": error_payload,
                }).encode()
                await _send_response(send, 200, resp)
            except Exception as exc:
                resp = json.dumps({
                    "jsonrpc": "2.0",
                    "id": request_id,
                    "error": {"code": -32603, "message": str(exc)},
                }).encode()
                await _send_response(send, 200, resp)
            return

        if path.startswith("/agents/") and path.endswith("/events") and method == "GET":
            parts = path.split("/")
            if len(parts) >= 4:
                agent_id = parts[2]
                bridge = self._runtime.sse_bridge()
                await self._proxy_sse(send, bridge.agent_events(agent_id))
                return

        if path == "/mob/events" and method == "GET":
            bridge = self._runtime.sse_bridge()
            await self._proxy_sse(send, bridge.mob_events())
            return

        if self._fallback_app is not None:
            await self._fallback_app(scope, receive, send)
            return

        await _send_response(send, 404, b'{"error":"not found"}')

    async def _proxy_sse(
        self,
        send: Any,
        event_stream: AsyncIterator[SseEvent],
    ) -> None:
        try:
            first_event: SseEvent | None = None
            async for event in event_stream:
                first_event = event
                break

            if first_event is None:
                await _send_response(send, 204, b"")
                return
        except Exception as exc:
            err = json.dumps({"error": f"SSE backend unavailable: {exc}"}).encode()
            await _send_response(send, 502, err)
            return

        await send({
            "type": "http.response.start",
            "status": 200,
            "headers": [
                [b"content-type", b"text/event-stream"],
                [b"cache-control", b"no-cache"],
                [b"connection", b"keep-alive"],
            ],
        })

        await send({
            "type": "http.response.body",
            "body": first_event.encode().encode("utf-8"),
            "more_body": True,
        })

        try:
            async for event in event_stream:
                chunk = event.encode().encode("utf-8")
                await send({"type": "http.response.body", "body": chunk, "more_body": True})
        except Exception:
            pass
        await send({"type": "http.response.body", "body": b""})


def _serialize_config(config: Any, _seen: set[int] | None = None) -> Any:
    """Serialize a config object to a JSON-compatible dict.

    Calls to_dict() on dataclass configs (GoogleAuthConfig, JwtAuthConfig,
    etc.) so json.dumps won't fail with TypeError during meta/init.
    Non-serializable leaves (e.g. storage backend instances) are converted
    to their qualified class name so the gateway receives a meaningful
    string instead of crashing the transport.  Cycle-safe via id-set.
    """
    if config is None or isinstance(config, (bool, int, float, str)):
        return config
    obj_id = id(config)
    if _seen is None:
        _seen = set()
    if obj_id in _seen:
        return f"[circular:{type(config).__qualname__}]"
    _seen.add(obj_id)
    if hasattr(config, "to_dict"):
        return config.to_dict()
    if isinstance(config, dict):
        return {k: _serialize_config(v, _seen) for k, v in config.items()}
    if isinstance(config, (list, tuple)):
        return [_serialize_config(v, _seen) for v in config]
    # Non-serializable object — use qualified class name so the gateway
    # gets a meaningful identifier instead of a TypeError.
    return f"{type(config).__module__}.{type(config).__qualname__}"


def _auth_config_to_dict(auth_config: Any) -> dict[str, Any]:
    """Normalize an auth config to a plain dict.

    Accepts JwtAuthConfig/GoogleAuthConfig (with to_dict()), plain dicts,
    or anything else (returns empty dict for fail-closed behavior).
    """
    if hasattr(auth_config, "to_dict"):
        return auth_config.to_dict()
    if isinstance(auth_config, dict):
        return auth_config
    return {}


def _validate_bearer_token(token: str, auth_config: Any) -> bool:
    """Validate a bearer token against the auth config.

    For JwtAuthConfig / {"provider": "jwt"} (shared-secret HMAC): full
    signature verification plus iss, aud, exp, and nbf claim checks
    with leeway.

    Google and unknown providers are rejected (fail closed). Google auth
    is blocked at AsgiApp construction time, but this is a defense-in-depth
    fallback.
    """
    config_dict = _auth_config_to_dict(auth_config)
    provider = config_dict.get("provider", "")

    if provider != "jwt":
        return False

    parts = token.split(".")
    if len(parts) != 3:
        return False

    try:
        header_b64 = parts[0] + "=" * (-len(parts[0]) % 4)
        header = json.loads(base64.urlsafe_b64decode(header_b64))
    except Exception:
        return False

    # Header must be a dict with alg field.
    if not isinstance(header, dict):
        return False

    secret = config_dict.get("shared_secret", "")
    if not secret:
        return False
    if header.get("alg") != "HS256":
        return False

    # Verify HMAC-SHA256 signature
    signing_input = f"{parts[0]}.{parts[1]}".encode("utf-8")
    expected_sig = base64.urlsafe_b64encode(
        hmac.new(secret.encode("utf-8"), signing_input, hashlib.sha256).digest()
    ).rstrip(b"=").decode("utf-8")
    if not hmac.compare_digest(expected_sig, parts[2]):
        return False

    # Decode and validate claims — must be a JSON object.
    try:
        payload_b64 = parts[1] + "=" * (-len(parts[1]) % 4)
        claims = json.loads(base64.urlsafe_b64decode(payload_b64))
    except Exception:
        return False

    if not isinstance(claims, dict):
        return False

    if config_dict.get("issuer") and claims.get("iss") != config_dict["issuer"]:
        return False
    expected_aud = config_dict.get("audience")
    if expected_aud:
        token_aud = claims.get("aud")
        # JWT aud may be a string or an array of strings (RFC 7519 §4.1.3).
        if isinstance(token_aud, list):
            if expected_aud not in token_aud:
                return False
        elif token_aud != expected_aud:
            return False

    # Enforce expiry with leeway — exp/nbf must be numeric.
    leeway = config_dict.get("leeway_seconds", 60)
    now = time.time()
    exp = claims.get("exp")
    if exp is not None:
        if not isinstance(exp, (int, float)):
            return False
        if now > exp + leeway:
            return False
    nbf = claims.get("nbf")
    if nbf is not None:
        if not isinstance(nbf, (int, float)):
            return False
        if now < nbf - leeway:
            return False

    return True


async def _read_body(receive: Any) -> bytes:
    body = b""
    while True:
        message = await receive()
        body += message.get("body", b"")
        if not message.get("more_body", False):
            break
    return body


async def _send_response(
    send: Any,
    status: int,
    body: bytes,
    content_type: bytes = b"application/json",
) -> None:
    await send({
        "type": "http.response.start",
        "status": status,
        "headers": [[b"content-type", content_type]],
    })
    await send({"type": "http.response.body", "body": body})
